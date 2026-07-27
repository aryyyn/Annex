//! The client to host wire protocol, JSON over the signalling WebSocket.
//!
//! Kept free of serde derives for now so the workspace builds with no external
//! crates. Add `#[derive(Serialize, Deserialize)]` and `#[serde(tag = "type")]`
//! at M3, when this first has to cross a socket.

use crate::input::InputEvent;

/// Session description, opaque to us: we only shuttle it between the browser
/// and webrtc-rs.
pub type Sdp = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientMsg {
    /// Must arrive first. The host drops the socket if the token does not match.
    Hello {
        token: Option<String>,
    },
    Answer(Sdp),
    Ice(IceCandidate),
    /// Only honoured when `HostConfig::allow_input` is set. The DataChannel is
    /// the real path for these; this variant exists for debugging.
    Input(InputEvent),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostMsg {
    Offer(Sdp),
    Ice(IceCandidate),
    /// Sent once the virtual display exists, so the client can size itself.
    Config {
        w: u32,
        h: u32,
        fps: u32,
    },
    Error(String),
}
