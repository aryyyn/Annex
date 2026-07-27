//! ScreenCaptureKit capture of a single display.
//!
//! Requires the Screen Recording permission. See [`permission`], including the
//! note about `cargo run` attributing the grant to your terminal rather than to
//! Annex.
//!
//! # Two things matter for latency here
//!
//! Ask for NV12 rather than BGRA once the encoder exists, because it is
//! VideoToolbox's native input and saves a colour conversion. And keep the
//! buffers on the GPU: they arrive as `CVPixelBuffer` and go straight into the
//! encoder as `CVPixelBuffer`, never touching the CPU.
//!
//! M1 deliberately breaks the second rule. To write a PNG we have to read the
//! pixels, which means locking the buffer and copying it to the CPU. That is
//! the price of proving the source works, and it goes away at M2 when the
//! encoder consumes the buffer directly.
//!
//! # Threading
//!
//! Frames arrive on a dispatch queue we create, not the main thread and not a
//! tokio worker. The sink closure runs there. Do not block in it.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

use annex_core::PixFmt;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamOutput, SCStreamOutputType, SCWindow,
};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

pub mod frame;
pub mod permission;

pub use frame::RawFrame;
pub use permission::has_permission;

#[derive(Debug)]
pub enum CaptureError {
    /// The Screen Recording grant is missing. Not recoverable in process: the
    /// user has to flip it in System Settings and relaunch.
    PermissionDenied,
    /// No `SCDisplay` matched the requested id. The virtual display may have
    /// gone away underneath us, or ScreenCaptureKit has not noticed it yet.
    DisplayNotFound(u32),
    /// `SCShareableContent` never called back, or returned an error.
    ContentQueryFailed(String),
    StreamFailed(String),
    /// Built for a platform with no ScreenCaptureKit.
    Unsupported,
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "Screen Recording permission not granted"),
            Self::DisplayNotFound(id) => {
                write!(f, "ScreenCaptureKit cannot see a display with id {id}")
            }
            Self::ContentQueryFailed(m) => write!(f, "SCShareableContent failed: {m}"),
            Self::StreamFailed(m) => write!(f, "SCStream failed: {m}"),
            Self::Unsupported => write!(f, "no ScreenCaptureKit on this platform"),
        }
    }
}

impl std::error::Error for CaptureError {}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptureConfig {
    pub display_id: u32,
    pub fps: u32,
    pub pixel_format: PixFmt,
    /// Output size in pixels. `None` derives it from the display's own size
    /// and [`CaptureConfig::scale`].
    pub size: Option<(u32, u32)>,
    /// Multiplier on the display's reported point size, ignored when `size` is
    /// set explicitly.
    ///
    /// `SCDisplay` reports **points**, not pixels. On a HiDPI display the
    /// backing store is twice that in each axis, so capturing at scale 1 would
    /// throw away half the resolution in each direction.
    ///
    /// # Leave this at 1 for the Annex virtual display
    ///
    /// Measured 27 July 2026 on macOS 26.5: the virtual display has **no**
    /// HiDPI backing store. `CGDisplayModeGetPixelWidth` equals
    /// `CGDisplayModeGetWidth`, and that holds with `hiDPI` set to 0 or 1 and
    /// with modes of 1920x1080 or 3840x2160. Capturing at scale 2 costs four
    /// times the pixels and bandwidth and returns a bicubic upscale: the
    /// captured 2x frame differs from an upscaled 1x frame by a mean of
    /// 0.85/255, which is resampling noise.
    ///
    /// Raise it only for a display that genuinely reports a 2x backing store.
    /// `apps/host/src/displays.rs::mode_geometry` is how to check.
    pub scale: u32,
    /// Drawing the cursor is a nice touch on a second monitor, but it makes
    /// every frame with a moving pointer differ, which costs bitrate.
    pub show_cursor: bool,
}

impl CaptureConfig {
    pub fn for_display(display_id: u32) -> Self {
        Self {
            display_id,
            fps: 60,
            pixel_format: PixFmt::Bgra,
            size: None,
            scale: 1,
            show_cursor: false,
        }
    }
}

/// Where frames go. Runs on ScreenCaptureKit's dispatch queue.
pub type FrameSink = Box<dyn FnMut(RawFrame) + Send + 'static>;

// ---------------------------------------------------------------------------
// The output delegate
// ---------------------------------------------------------------------------
//
// ScreenCaptureKit hands frames to an Objective-C object conforming to
// SCStreamOutput. There is no way around defining a real Objective-C class, so
// objc2's `define_class!` builds one at runtime and lets us store Rust state
// inside it as "ivars".

/// Rust state carried by the delegate object.
struct SinkIvars {
    sink: Mutex<FrameSink>,
}

define_class!(
    // SAFETY: the superclass is NSObject, the class name is unique to this
    // crate, and the only method implemented is the SCStreamOutput callback.
    #[unsafe(super(NSObject))]
    #[name = "AnnexStreamOutput"]
    #[ivars = SinkIvars]
    struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen {
                return;
            }

            // Not every delivered sample carries pixels. ScreenCaptureKit also
            // emits status-only samples, for example "the screen is idle and
            // this frame is unchanged", and those have no image buffer.
            // Dropping them is correct, not a lost frame.
            let Some(frame) = frame::from_sample(sample) else {
                return;
            };

            // The sink runs on SCK's queue. A panic here would unwind into
            // Objective-C, which is undefined behaviour, so contain it.
            if let Ok(mut sink) = self.ivars().sink.lock() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (sink)(frame);
                }));
            }
        }
    }
);

impl StreamOutput {
    fn new(sink: FrameSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SinkIvars {
            sink: Mutex::new(sink),
        });
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// The capturer
// ---------------------------------------------------------------------------

/// A running capture session. Dropping this stops delivery.
pub struct Capturer {
    stream: Retained<SCStream>,
    /// Kept alive deliberately. `SCStream` does not own its outputs strongly
    /// enough to keep ours around, and dropping it early stops frames silently.
    _output: Retained<StreamOutput>,
    /// Likewise the delivery queue.
    _queue: DispatchRetained<DispatchQueue>,
}

impl Capturer {
    /// Opens a stream filtered to one display.
    ///
    /// The steps, in order, because each depends on the last:
    ///
    /// 1. Check the TCC grant, so a missing permission reports itself instead
    ///    of looking like a stall.
    /// 2. Ask `SCShareableContent` what is capturable. This is asynchronous and
    ///    is the only way to obtain an `SCDisplay`, which is what the filter
    ///    needs. A raw `CGDirectDisplayID` will not do.
    /// 3. Find the `SCDisplay` whose `displayID` matches ours.
    /// 4. Build a filter capturing that display and excluding nothing.
    /// 5. Configure size, pixel format and frame interval.
    /// 6. Create the stream, attach our output on a dedicated queue, and start.
    pub fn start(cfg: CaptureConfig, sink: FrameSink) -> Result<Self, CaptureError> {
        if !permission::has_permission() {
            permission::request_permission();
            if !permission::has_permission() {
                return Err(CaptureError::PermissionDenied);
            }
        }

        let display = find_display(cfg.display_id)?;

        unsafe {
            // ---- filter --------------------------------------------------
            let empty: Retained<NSArray<SCWindow>> = NSArray::new();
            let filter = SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &empty,
            );

            // ---- configuration -------------------------------------------
            let config = SCStreamConfiguration::new();

            let (w, h) = match cfg.size {
                Some(wh) => wh,
                // SCDisplay reports points, not pixels. On a HiDPI display the
                // backing store is scale times larger in each axis.
                None => {
                    let s = cfg.scale.max(1);
                    (display.width() as u32 * s, display.height() as u32 * s)
                }
            };
            config.setWidth(w as usize);
            config.setHeight(h as usize);

            config.setPixelFormat(pixel_format_code(cfg.pixel_format));
            config.setShowsCursor(cfg.show_cursor);

            // A floor on the interval, so a ceiling on the frame rate.
            // ScreenCaptureKit only emits on change, so a still desktop
            // produces nothing at all and costs nothing.
            config.setMinimumFrameInterval(CMTime {
                value: 1,
                timescale: cfg.fps.max(1) as i32,
                flags: objc2_core_media::CMTimeFlags::Valid,
                epoch: 0,
            });

            // How many frames SCK will hold before dropping. Small is right for
            // a live pipeline: a deep queue converts jitter into latency.
            config.setQueueDepth(3);

            // ---- stream ---------------------------------------------------
            let output = StreamOutput::new(sink);
            let proto = ProtocolObject::from_ref(&*output);

            let stream = SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                None,
            );

            let queue = dispatch2::DispatchQueue::new("app.annex.capture", None);

            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    proto,
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|e| CaptureError::StreamFailed(e.localizedDescription().to_string()))?;

            // startCapture is asynchronous. Block until it reports, so a
            // failure surfaces here rather than as silence.
            let (tx, rx) = mpsc::channel::<Option<String>>();
            let handler = block2::RcBlock::new(move |err: *mut NSError| {
                let msg = if err.is_null() {
                    None
                } else {
                    Some((*err).localizedDescription().to_string())
                };
                let _ = tx.send(msg);
            });
            stream.startCaptureWithCompletionHandler(Some(&handler));

            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(None) => {}
                Ok(Some(e)) => return Err(CaptureError::StreamFailed(e)),
                Err(_) => {
                    return Err(CaptureError::StreamFailed(
                        "startCapture did not call back within 10s".into(),
                    ))
                }
            }

            Ok(Self {
                stream,
                _output: output,
                _queue: queue,
            })
        }
    }

    /// Stops the stream and waits for confirmation.
    pub fn stop(self) {
        let (tx, rx) = mpsc::channel::<()>();
        let handler = block2::RcBlock::new(move |_err: *mut NSError| {
            let _ = tx.send(());
        });
        unsafe {
            self.stream.stopCaptureWithCompletionHandler(Some(&handler));
        }
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }
}

/// Asks ScreenCaptureKit for the display matching `display_id`.
///
/// `SCShareableContent` is asynchronous with no synchronous variant, so this
/// blocks on a channel. Acceptable because it happens once at startup, never
/// per frame.
fn find_display(display_id: u32) -> Result<Retained<SCDisplay>, CaptureError> {
    let displays = shareable_displays()?;
    displays
        .into_iter()
        .find(|d| unsafe { d.displayID() } == display_id)
        .ok_or(CaptureError::DisplayNotFound(display_id))
}

fn shareable_displays() -> Result<Vec<Retained<SCDisplay>>, CaptureError> {
    let (tx, rx) = mpsc::channel::<Result<Vec<Retained<SCDisplay>>, String>>();

    let handler =
        block2::RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
            let result = unsafe {
                if !err.is_null() {
                    Err((*err).localizedDescription().to_string())
                } else if content.is_null() {
                    Err("SCShareableContent returned nil with no error".to_string())
                } else {
                    Ok((*content).displays().iter().collect::<Vec<_>>())
                }
            };
            let _ = tx.send(result);
        });

    unsafe {
        SCShareableContent::getShareableContentWithCompletionHandler(&handler);
    }

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(d)) => Ok(d),
        Ok(Err(e)) => Err(CaptureError::ContentQueryFailed(e)),
        // The usual cause is a missing Screen Recording grant: the callback
        // simply never fires.
        Err(_) => Err(CaptureError::ContentQueryFailed(
            "no callback within 10s, which usually means Screen Recording is denied".into(),
        )),
    }
}

/// Every display ScreenCaptureKit can currently see, as `(id, width, height)`.
///
/// Useful for the diagnostic when [`CaptureError::DisplayNotFound`] fires.
pub fn list_displays() -> Result<Vec<(u32, u32, u32)>, CaptureError> {
    Ok(shareable_displays()?
        .into_iter()
        .map(|d| unsafe { (d.displayID(), d.width() as u32, d.height() as u32) })
        .collect())
}

/// FourCC codes ScreenCaptureKit expects for `pixelFormat`.
///
/// These are `OSType` values: four ASCII characters packed into a u32. `BGRA`
/// is literally the bytes 'B','G','R','A'. NV12 is the odd one out, coded
/// '420v' for 4:2:0 video-range.
fn pixel_format_code(fmt: PixFmt) -> u32 {
    match fmt {
        PixFmt::Bgra => u32::from_be_bytes(*b"BGRA"),
        PixFmt::Nv12 => u32::from_be_bytes(*b"420v"),
    }
}

/// Renders a `CVPixelBuffer` format code readably, for diagnostics.
pub fn format_name(code: u32) -> String {
    let b = code.to_be_bytes();
    if b.iter().all(|c| c.is_ascii_graphic()) {
        String::from_utf8_lossy(&b).into_owned()
    } else {
        format!("0x{code:08X}")
    }
}
