//! Serves the client page, runs signalling, and owns one peer connection per
//! client.
//!
//! Platform neutral by design. The native Windows client in
//! `apps/client-native` reuses this crate unchanged, which is why nothing Apple
//! specific may leak in here.
//!
//! # Being LAN-only buys a lot
//!
//! ICE is configured with no STUN and no TURN servers, so only host candidates
//! are gathered and the two machines connect directly. That removes the entire
//! NAT traversal problem, every external dependency, and the usual multi-second
//! connection setup. It is the single largest simplification in the design.

pub mod session;
pub mod signaling;

pub use session::{RtcConfig, Session};

#[derive(Debug)]
pub enum RtcError {
    Bind(std::io::Error),
    /// A client failed or omitted the shared-secret handshake.
    Unauthorised,
    Negotiation(String),
    PeerClosed,
}

/// The HTTP and WebSocket server. One port serves the static client page and
/// upgrades `/signal` to a WebSocket, so the user only has one URL to type.
pub struct Server {
    _private: (),
}

impl Server {
    pub async fn bind(cfg: RtcConfig) -> Result<Self, RtcError> {
        let _ = cfg;
        todo!("M3: axum router, static files at /, ws upgrade at /signal")
    }

    /// Fans one encoded sample out to every connected client.
    ///
    /// Back pressure lives here. The channel feeding this is bounded, and when
    /// it fills the correct response is to drop the oldest frame and ask the
    /// encoder for a keyframe. Queueing instead trades a brief glitch for
    /// permanently growing latency, which is much worse to use.
    pub async fn broadcast(&self, sample: &annex_core::EncodedSample) {
        let _ = sample;
        todo!("M3: write_sample on each session's track")
    }
}
