//! VideoToolbox compression, tuned for real time rather than for file size.
//!
//! # The settings that decide whether this feels laggy
//!
//! - `RealTime = true`. Tells the encoder to prioritise latency over quality.
//! - `AllowFrameReordering = false`. No B-frames. A B-frame references a future
//!   frame, so emitting one means holding the current frame back until that
//!   future frame exists. That is a whole frame interval of latency bought for
//!   compression we do not need.
//! - Annex-B output, so the WebRTC H.264 payloader can consume it directly.
//! - SPS and PPS in band with every keyframe, so a client joining mid-stream
//!   has something to initialise its decoder with.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

use annex_capture::RawFrame;
use annex_core::{EncodedSample, EncoderConfig, Timestamp};

#[derive(Debug)]
pub enum EncError {
    SessionCreateFailed(i32),
    PropertyRejected(&'static str),
    EncodeFailed(i32),
    /// Built for a platform with no CoreGraphics. Unused until the workspace
    /// gains target gating at M7; kept so the cross-platform story is explicit.
    Unsupported,
}

/// Where finished samples go. Called on VideoToolbox's callback thread, so the
/// implementation should push onto a channel and return rather than do work.
pub type EncodedSink = Box<dyn FnMut(EncodedSample) + Send + 'static>;

pub struct Encoder {
    // M2: VTCompressionSession, released on drop.
    _private: (),
}

impl Encoder {
    pub fn new(cfg: EncoderConfig, out: EncodedSink) -> Result<Self, EncError> {
        let _ = (cfg, out);
        todo!("M2: VTCompressionSessionCreate, then set the real-time properties")
    }

    /// Feeds one frame. The pixel buffer is passed straight through, so this
    /// stays a GPU to GPU handoff with no copy.
    pub fn encode(&self, frame: &RawFrame, pts: Timestamp) -> Result<(), EncError> {
        let _ = (frame, pts);
        todo!("M2: VTCompressionSessionEncodeFrame")
    }

    /// Forces the next frame to be an IDR.
    ///
    /// Call on two triggers: a new client connecting, which needs a decodable
    /// starting point, and a PLI arriving from an existing client, which means
    /// its decoder lost sync and everything until the next keyframe is garbage.
    pub fn request_keyframe(&self) {
        todo!("M2: kVTEncodeFrameOptionKey_ForceKeyFrame on the next frame")
    }

    /// Adjusts the target bitrate.
    ///
    /// Wire this to WebRTC's congestion control estimate. Ignoring that signal
    /// on a congested link produces exactly the growing queue of stale frames
    /// that the drop-oldest policy exists to prevent.
    pub fn set_bitrate(&self, kbps: u32) {
        let _ = kbps;
        todo!("M2: kVTCompressionPropertyKey_AverageBitRate")
    }
}
