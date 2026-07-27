//! VideoToolbox compression, tuned for real time rather than for file size.
//!
//! # The settings that decide whether this feels laggy
//!
//! - `RealTime = true`. Tells the encoder to prioritise latency over quality.
//! - `AllowFrameReordering = false`. No B-frames. A B-frame references a *future*
//!   frame, so emitting one means holding the current frame back until that
//!   future frame exists. That is a whole frame interval of latency bought for
//!   compression we do not need.
//! - Annex-B output, so the WebRTC H.264 payloader can consume it directly. See
//!   [`annexb`], since VideoToolbox does not produce it natively.
//! - SPS and PPS in band with every keyframe, so a client joining mid-stream has
//!   something to initialise its decoder with.
//!
//! # This is where zero-copy pays off
//!
//! [`Encoder::encode`] takes the `CVPixelBuffer` straight from
//! `annex-capture` and hands it to VideoToolbox. The pixels stay on the GPU
//! from the compositor to the media engine and are never read by the CPU. M1's
//! `to_bgra_vec` was a deliberate exception for writing PNGs; nothing in the
//! streaming path calls it.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

pub mod annexb;

use annex_capture::RawFrame;
use annex_core::{Codec, EncodedSample, EncoderConfig, Timestamp};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{kCMTimeInvalid, CMSampleBuffer, CMTime, CMTimeFlags, CMVideoCodecType};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_Main_AutoLevel,
    VTCompressionSession, VTEncodeInfoFlags,
};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;

/// `VTSession` is an alias for `CFType`, and every VideoToolbox session type is
/// a CoreFoundation object, so the property functions take the same reference
/// after a cast.
fn as_session(s: &VTCompressionSession) -> &objc2_video_toolbox::VTSession {
    // SAFETY: VTCompressionSessionRef is a CFTypeRef by construction.
    unsafe { &*(s as *const VTCompressionSession as *const objc2_video_toolbox::VTSession) }
}

#[derive(Debug)]
pub enum EncError {
    SessionCreateFailed(i32),
    PropertyRejected(&'static str, i32),
    EncodeFailed(i32),
    /// The compressed sample had no block buffer, or its NAL lengths did not
    /// parse. Should not happen with a healthy encoder.
    MalformedOutput,
    /// Built for a platform with no VideoToolbox.
    Unsupported,
}

impl std::fmt::Display for EncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionCreateFailed(s) => write!(f, "VTCompressionSessionCreate failed: {s}"),
            Self::PropertyRejected(k, s) => write!(f, "encoder rejected property {k}: {s}"),
            Self::EncodeFailed(s) => write!(f, "VTCompressionSessionEncodeFrame failed: {s}"),
            Self::MalformedOutput => write!(f, "compressed sample could not be parsed"),
            Self::Unsupported => write!(f, "no VideoToolbox on this platform"),
        }
    }
}

impl std::error::Error for EncError {}

/// Where finished samples go.
///
/// Called on VideoToolbox's own callback thread, not the caller's. The
/// implementation should push onto a channel and return rather than do work.
pub type EncodedSink = Box<dyn FnMut(EncodedSample) + Send + 'static>;

/// State reachable from the C callback.
struct Shared {
    sink: Mutex<EncodedSink>,
    stats: Mutex<Stats>,
    /// Presentation timestamp of the previous output sample, used to derive
    /// each sample's real duration. See [`Encoder`] for why that matters.
    last_pts: Mutex<Option<i64>>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub frames_in: u64,
    pub frames_out: u64,
    pub keyframes: u64,
    pub bytes_out: u64,
    pub dropped_malformed: u64,
}

pub struct Encoder {
    session: CFRetained<VTCompressionSession>,
    /// Leaked deliberately for the session's lifetime: VideoToolbox holds this
    /// pointer as its callback context and may invoke it until the session is
    /// invalidated. Reclaimed in `Drop`, after invalidation.
    shared: *mut Shared,
}

// SAFETY: `VTCompressionSession` is documented as safe to use from multiple
// threads: `VTCompressionSessionEncodeFrame` serialises internally, and the
// output callback already runs on a VideoToolbox-owned thread rather than the
// caller's. The `shared` pointer is only ever dereferenced to reach `Mutex`
// fields, and it stays valid until `Drop` invalidates the session first.
//
// `Sync` matters in practice because the capture queue holds an `Arc<Encoder>`
// and calls `encode` from ScreenCaptureKit's thread while the main thread calls
// `stats` and `flush`.
unsafe impl Send for Encoder {}
unsafe impl Sync for Encoder {}

impl Encoder {
    pub fn new(cfg: EncoderConfig, out: EncodedSink) -> Result<Self, EncError> {
        let shared = Box::into_raw(Box::new(Shared {
            sink: Mutex::new(out),
            stats: Mutex::new(Stats::default()),
            last_pts: Mutex::new(None),
        }));

        let codec = match cfg.codec {
            // 'avc1' and 'hvc1' as FourCC.
            Codec::H264 => u32::from_be_bytes(*b"avc1") as CMVideoCodecType,
            Codec::Hevc => u32::from_be_bytes(*b"hvc1") as CMVideoCodecType,
        };

        let mut raw: *mut VTCompressionSession = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(
                None,
                cfg.width as i32,
                cfg.height as i32,
                codec,
                None,
                None,
                None,
                Some(output_callback),
                shared as *mut c_void,
                NonNull::from(&mut raw),
            )
        };

        if status != 0 || raw.is_null() {
            unsafe { drop(Box::from_raw(shared)) };
            return Err(EncError::SessionCreateFailed(status));
        }

        let session = unsafe { CFRetained::from_raw(NonNull::new(raw).unwrap()) };
        let enc = Self { session, shared };
        enc.configure(&cfg)?;
        Ok(enc)
    }

    /// The properties that make this a low-latency encoder rather than a
    /// file-compression one.
    fn configure(&self, cfg: &EncoderConfig) -> Result<(), EncError> {
        // Prioritise latency over compression efficiency.
        self.set_bool(
            unsafe { kVTCompressionPropertyKey_RealTime },
            "RealTime",
            true,
        )?;

        // No B-frames. This is the single most important latency setting: a
        // B-frame references a future frame, so the encoder would have to hold
        // the current one back until that future frame arrives.
        self.set_bool(
            unsafe { kVTCompressionPropertyKey_AllowFrameReordering },
            "AllowFrameReordering",
            false,
        )?;

        if cfg.codec == Codec::H264 {
            // Main is universally decodable and slightly more efficient than
            // Constrained Baseline. Drop to Baseline if a client refuses it.
            let profile = unsafe { kVTProfileLevel_H264_Main_AutoLevel };
            let status = unsafe {
                objc2_video_toolbox::VTSessionSetProperty(
                    as_session(&self.session),
                    kVTCompressionPropertyKey_ProfileLevel,
                    Some(profile.as_ref() as &CFType),
                )
            };
            if status != 0 {
                return Err(EncError::PropertyRejected("ProfileLevel", status));
            }
        }

        self.set_i32(
            unsafe { kVTCompressionPropertyKey_MaxKeyFrameInterval },
            "MaxKeyFrameInterval",
            cfg.keyframe_interval as i32,
        )?;
        self.set_i32(
            unsafe { kVTCompressionPropertyKey_AverageBitRate },
            "AverageBitRate",
            (cfg.bitrate_kbps * 1000) as i32,
        )?;
        self.set_i32(
            unsafe { kVTCompressionPropertyKey_ExpectedFrameRate },
            "ExpectedFrameRate",
            cfg.fps as i32,
        )?;
        Ok(())
    }

    fn set_bool(&self, key: &CFString, name: &'static str, v: bool) -> Result<(), EncError> {
        let value = CFBoolean::new(v);
        let status = unsafe {
            objc2_video_toolbox::VTSessionSetProperty(
                as_session(&self.session),
                key,
                Some(value.as_ref() as &CFType),
            )
        };
        if status != 0 {
            return Err(EncError::PropertyRejected(name, status));
        }
        Ok(())
    }

    fn set_i32(&self, key: &CFString, name: &'static str, v: i32) -> Result<(), EncError> {
        let n = CFNumber::new_i32(v);
        let status = unsafe {
            objc2_video_toolbox::VTSessionSetProperty(
                as_session(&self.session),
                key,
                Some(n.as_ref() as &CFType),
            )
        };
        if status != 0 {
            return Err(EncError::PropertyRejected(name, status));
        }
        Ok(())
    }

    /// Feeds one frame.
    ///
    /// The pixel buffer is handed through untouched, so this stays a GPU to GPU
    /// operation. The call returns as soon as the frame is queued; the encoded
    /// result arrives later on the callback thread.
    pub fn encode(&self, frame: &RawFrame, pts: Timestamp) -> Result<(), EncError> {
        let time = CMTime {
            value: pts,
            timescale: 1_000_000, // pts is in microseconds
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };

        let status = unsafe {
            self.session.encode_frame(
                frame.pixel_buffer(),
                time,
                kCMTimeInvalid,
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(EncError::EncodeFailed(status));
        }
        unsafe { (*self.shared).stats.lock().unwrap().frames_in += 1 };
        Ok(())
    }

    /// Forces the next frame to be an IDR.
    ///
    /// Two triggers in practice: a new client connecting, which needs a
    /// decodable starting point, and a PLI arriving from an existing client,
    /// which means its decoder lost sync and everything until the next keyframe
    /// is garbage.
    pub fn encode_keyframe(&self, frame: &RawFrame, pts: Timestamp) -> Result<(), EncError> {
        let key = unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame };
        let props =
            CFDictionary::from_slices(&[key as &CFType], &[CFBoolean::new(true) as &CFType]);
        // from_slices gives a typed dictionary; the FFI takes the untyped form.
        let props: &CFDictionary =
            unsafe { &*(&*props as *const CFDictionary<CFType, CFType> as *const CFDictionary) };
        let time = CMTime {
            value: pts,
            timescale: 1_000_000,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };
        let status = unsafe {
            self.session.encode_frame(
                frame.pixel_buffer(),
                time,
                kCMTimeInvalid,
                Some(props),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(EncError::EncodeFailed(status));
        }
        unsafe { (*self.shared).stats.lock().unwrap().frames_in += 1 };
        Ok(())
    }

    /// Adjusts the target bitrate.
    ///
    /// Wire this to WebRTC's congestion control estimate at M3. Ignoring that
    /// signal on a congested link produces exactly the growing queue of stale
    /// frames the drop-oldest policy exists to prevent.
    pub fn set_bitrate(&self, kbps: u32) -> Result<(), EncError> {
        self.set_i32(
            unsafe { kVTCompressionPropertyKey_AverageBitRate },
            "AverageBitRate",
            (kbps * 1000) as i32,
        )
    }

    /// Waits for every queued frame to come out.
    ///
    /// Even with reordering disabled the encoder pipelines internally, so
    /// without this the last few frames are still in flight when the process
    /// exits and the file is short.
    pub fn flush(&self) {
        unsafe {
            let _ = self.session.complete_frames(kCMTimeInvalid);
        }
    }

    pub fn stats(&self) -> Stats {
        unsafe { *(*self.shared).stats.lock().unwrap() }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // Order matters. Drain first, then invalidate so VideoToolbox stops
        // calling back, and only then free the context the callback reads.
        unsafe {
            let _ = self.session.complete_frames(kCMTimeInvalid);
            self.session.invalidate();
            drop(Box::from_raw(self.shared));
        }
    }
}

/// VideoToolbox's C callback. Runs on its own thread.
unsafe extern "C-unwind" fn output_callback(
    ctx: *mut c_void,
    _frame_ref: *mut c_void,
    status: i32,
    _flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    if ctx.is_null() || status != 0 || sample.is_null() {
        return;
    }
    let shared = unsafe { &*(ctx as *const Shared) };

    // Unwinding out of here would cross a C frame, which is undefined
    // behaviour, so nothing below is allowed to escape.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sample = unsafe { &*sample };
        match extract(sample) {
            Some(mut encoded) => {
                // Fill in the real duration: the gap since the previous frame.
                //
                // This has to be measured, not assumed. ScreenCaptureKit emits
                // only on change, so a still desktop produces nothing and then
                // one frame after an arbitrary gap. Reporting a fixed 1/fps
                // here would advance the receiver's RTP clock far more slowly
                // than wall time, and its jitter buffer grows without bound to
                // compensate. That is felt as latency that gets worse the
                // longer the session runs.
                {
                    let mut last = shared.last_pts.lock().unwrap();
                    let dur_us = match *last {
                        Some(prev) if encoded.pts > prev => (encoded.pts - prev) as u64,
                        // First frame, or a non-monotonic timestamp. One frame
                        // at 60 fps is a safe assumption for a single sample.
                        _ => 16_667,
                    };
                    // Clamp so a pathological gap cannot desynchronise the
                    // receiver, while still allowing genuinely long still
                    // periods to advance the clock honestly.
                    let dur_us = dur_us.clamp(1_000, 5_000_000);
                    encoded.dur = std::time::Duration::from_micros(dur_us);
                    *last = Some(encoded.pts);
                }
                {
                    let mut s = shared.stats.lock().unwrap();
                    s.frames_out += 1;
                    s.bytes_out += encoded.data.len() as u64;
                    if encoded.keyframe {
                        s.keyframes += 1;
                    }
                }
                if let Ok(mut sink) = shared.sink.lock() {
                    (sink)(encoded);
                }
            }
            None => {
                shared.stats.lock().unwrap().dropped_malformed += 1;
            }
        }
    }));
}

/// Pulls one access unit out of a compressed sample and converts it to Annex-B.
fn extract(sample: &CMSampleBuffer) -> Option<EncodedSample> {
    let block = unsafe { sample.data_buffer() }?;
    let len = unsafe { block.data_length() };
    let mut ptr: *mut i8 = std::ptr::null_mut();
    let status = unsafe {
        block.data_pointer(
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut ptr as *mut *mut i8,
        )
    };
    if status != 0 || ptr.is_null() {
        return None;
    }
    let avcc = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };

    let (nal_len_size, param_sets) = format_info(sample).unwrap_or((4, Vec::new()));
    let mut data = annexb::avcc_to_annexb(avcc, nal_len_size)?;

    // Read keyframe status out of the bytes rather than the sample's attachment
    // dictionary. A NAL of type 5 is an IDR, which is what "keyframe" means, so
    // this describes the stream we actually produced.
    let keyframe = annexb::scan(&data).iter().any(|(t, _)| *t == 5);

    // Parameter sets go in band ahead of every keyframe, so a client that joins
    // mid-stream can initialise its decoder from the next keyframe alone.
    if keyframe && !param_sets.is_empty() {
        let mut with_params = annexb::parameter_sets_to_annexb(&param_sets);
        with_params.append(&mut data);
        data = with_params;
    }

    let pts = unsafe { sample.presentation_time_stamp() };
    let pts_us = if pts.timescale > 0 {
        (pts.value as i128 * 1_000_000 / pts.timescale as i128) as i64
    } else {
        0
    };

    Some(EncodedSample {
        data: data.into(),
        pts: pts_us,
        dur: std::time::Duration::ZERO,
        keyframe,
    })
}

/// Reads the NAL length field size and the parameter sets from the sample's
/// format description.
fn format_info(sample: &CMSampleBuffer) -> Option<(usize, Vec<Vec<u8>>)> {
    let fd = unsafe { sample.format_description() }?;

    let mut count: usize = 0;
    let mut nal_len: i32 = 4;
    let status = unsafe {
        objc2_core_media::CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            &fd,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            NonNull::from(&mut count).as_ptr(),
            NonNull::from(&mut nal_len).as_ptr(),
        )
    };
    if status != 0 {
        return None;
    }

    let mut sets = Vec::with_capacity(count);
    for i in 0..count {
        let mut ptr: *const u8 = std::ptr::null();
        let mut size: usize = 0;
        let st = unsafe {
            objc2_core_media::CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                &fd,
                i,
                NonNull::from(&mut ptr).as_ptr().cast(),
                NonNull::from(&mut size).as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if st == 0 && !ptr.is_null() && size > 0 {
            sets.push(unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec());
        }
    }
    Some((nal_len as usize, sets))
}
