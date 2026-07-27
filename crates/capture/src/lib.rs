//! ScreenCaptureKit capture of a single display.
//!
//! Requires the Screen Recording permission. macOS prompts on first use and the
//! app then appears under System Settings, Privacy and Security, Screen
//! Recording. Until it is granted the stream starts but delivers no frames,
//! which looks exactly like a hung pipeline, so check for it explicitly.
//!
//! Two things matter for latency here. Ask for NV12 rather than BGRA, because
//! it is VideoToolbox's native input and saves a colour conversion. And keep
//! the frame buffers on the GPU: they arrive as `CVPixelBuffer` and go straight
//! into the encoder as `CVPixelBuffer`, never touching the CPU.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

use annex_core::PixFmt;

#[derive(Debug)]
pub enum CaptureError {
    /// The Screen Recording grant is missing. Not recoverable in process: the
    /// user has to flip it in System Settings and relaunch.
    PermissionDenied,
    /// No `SCDisplay` matched the requested id. The virtual display may have
    /// gone away underneath us.
    DisplayNotFound(u32),
    StreamFailed(String),
    /// Built for a platform with no CoreGraphics. Unused until the workspace
    /// gains target gating at M7; kept so the cross-platform story is explicit.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptureConfig {
    pub display_id: u32,
    pub fps: u32,
    pub pixel_format: PixFmt,
}

/// Callback for delivered frames.
///
/// This runs on ScreenCaptureKit's own dispatch queue, not the main thread and
/// not a tokio worker. Do not block in here: encode and hand off, or forward
/// and return. Blocking stalls capture and drops frames.
pub type FrameSink = Box<dyn FnMut(RawFrame) + Send + 'static>;

/// A borrowed GPU frame. Deliberately opaque: nothing outside this crate and
/// the encoder should be able to reach the pixels, because reaching them means
/// copying them off the GPU.
pub struct RawFrame {
    // M1: pixel_buffer: CVPixelBuffer, pts: CMTime
    _private: (),
}

pub struct Capturer {
    // M1: SCStream plus its delegate, kept alive for the session.
    _private: (),
}

impl Capturer {
    /// Resolves the `SCDisplay` whose id matches, builds a content filter for
    /// it alone, and starts the stream.
    ///
    /// Set `minimumFrameInterval` from `cfg.fps`. ScreenCaptureKit only emits
    /// on change, so a still desktop costs almost nothing and the frame rate is
    /// a ceiling rather than a target.
    pub fn start(cfg: CaptureConfig, sink: FrameSink) -> Result<Self, CaptureError> {
        let _ = (cfg, sink);
        todo!("M1: SCShareableContent, then SCContentFilter, then SCStream")
    }

    pub fn stop(self) {
        // M1: stopCaptureWithCompletionHandler, then release the stream.
    }
}

/// Whether the Screen Recording grant is already in place.
///
/// Worth calling before starting, so the tray UI can explain the prompt instead
/// of the user staring at a black screen.
pub fn has_permission() -> bool {
    todo!("M1: CGPreflightScreenCaptureAccess")
}
