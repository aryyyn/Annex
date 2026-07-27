//! What travels from the encoder to the transport.
//!
//! Note what is deliberately absent: there is no raw-frame type here. Raw
//! frames stay as `CVPixelBuffer` on the GPU all the way from ScreenCaptureKit
//! into VideoToolbox, so they never cross this boundary and never touch the
//! CPU. Only compressed bytes reach this crate.

use std::time::Duration;

/// Presentation timestamp in microseconds on the capture clock.
pub type Timestamp = i64;

/// One compressed access unit, ready to hand to the WebRTC payloader.
///
/// `data` is Annex-B: NAL units separated by start codes. Keyframes must carry
/// their SPS and PPS in band, otherwise a client joining mid-stream has nothing
/// to initialise its decoder with.
#[derive(Debug, Clone)]
pub struct EncodedSample {
    pub data: Vec<u8>,
    pub pts: Timestamp,
    pub dur: Duration,
    pub keyframe: bool,
}

impl EncodedSample {
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}
