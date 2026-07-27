//! One connected client.

use crate::RtcError;
use annex_core::{EncodedSample, InputEvent};
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct RtcConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
    pub allow_input: bool,
}

pub struct Session {
    // M3: RTCPeerConnection plus an Arc<TrackLocalStaticSample>.
    _private: (),
}

impl Session {
    /// Completes the handshake: check the token, build the peer connection,
    /// add the video track, exchange SDP, then trickle ICE.
    pub async fn accept(cfg: &RtcConfig) -> Result<Self, RtcError> {
        let _ = cfg;
        todo!("M3")
    }

    pub async fn write_sample(&self, sample: &EncodedSample) -> Result<(), RtcError> {
        let _ = sample;
        todo!("M3: TrackLocalStaticSample::write_sample")
    }

    /// Registers the phase-2 input handler. Only ever called when
    /// `RtcConfig::allow_input` is set.
    pub fn on_input(&self, cb: impl Fn(InputEvent) + Send + 'static) {
        let _ = cb;
        todo!("M5: DataChannel on_message, decode, hop to the main thread")
    }

    /// Fires when the client's decoder loses sync and needs a fresh IDR. Wire
    /// straight to `Encoder::request_keyframe`.
    pub fn on_picture_loss(&self, cb: impl Fn() + Send + 'static) {
        let _ = cb;
        todo!("M3: RTCP PLI handler")
    }
}
