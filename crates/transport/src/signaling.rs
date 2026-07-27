//! The JSON over WebSocket handshake that bootstraps WebRTC.
//!
//! Signalling exists because two peers cannot describe themselves to each other
//! over a connection that does not exist yet. This is the out of band channel
//! that carries the session descriptions and candidates until the real peer
//! connection takes over.
//!
//! The media path is always encrypted by DTLS-SRTP. This socket is plain ws on
//! the LAN by default, which is a deliberate choice: it carries no media, and
//! an attacker already on your network who can read it still cannot decrypt
//! anything. Upgrade to wss with a self-signed certificate if that trade does
//! not suit.

use annex_core::protocol::{ClientMsg, HostMsg};

pub fn encode(msg: &HostMsg) -> String {
    let _ = msg;
    todo!("M3: serde_json once protocol.rs has its derives")
}

pub fn decode(raw: &str) -> Option<ClientMsg> {
    let _ = raw;
    todo!("M3")
}
