//! The client to host wire protocol, JSON over the signalling WebSocket.
//!
//! # Shape on the wire
//!
//! Every message is a JSON object with a `type` discriminator, which is what
//! the browser client switches on:
//!
//! ```json
//! {"type":"hello","token":null}
//! {"type":"offer","sdp":"v=0\r\n..."}
//! {"type":"answer","sdp":"v=0\r\n..."}
//! {"type":"ice","candidate":{"candidate":"candidate:...","sdpMid":"0","sdpMLineIndex":0}}
//! {"type":"config","w":1920,"h":1080,"fps":30}
//! {"type":"error","message":"bad token"}
//! ```
//!
//! The ICE candidate field names are camelCase deliberately: they mirror the
//! browser's own `RTCIceCandidateInit`, so the client can pass the object
//! straight to `addIceCandidate` without translating it.
//!
//! # Who offers
//!
//! The **host** creates the offer. It is the side with a media track to
//! describe, so it has something to say first; the browser only has to answer.

use crate::input::InputEvent;
use serde::{Deserialize, Serialize};

/// Session description, opaque to us: we only shuttle it between the browser
/// and webrtc-rs.
pub type Sdp = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceCandidate {
    pub candidate: String,
    #[serde(rename = "sdpMid", default, skip_serializing_if = "Option::is_none")]
    pub sdp_mid: Option<String>,
    #[serde(
        rename = "sdpMLineIndex",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sdp_mline_index: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMsg {
    /// Must arrive first. The host drops the socket if the token does not match.
    Hello {
        token: Option<String>,
    },
    Answer {
        sdp: Sdp,
    },
    Ice {
        candidate: IceCandidate,
    },
    /// Only honoured when `HostConfig::allow_input` is set. The DataChannel is
    /// the real path for these; this variant exists for debugging.
    Input {
        event: InputEvent,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HostMsg {
    Offer {
        sdp: Sdp,
    },
    Ice {
        candidate: IceCandidate,
    },
    /// Sent once the stream is configured, so the client can size itself.
    Config {
        w: u32,
        h: u32,
        fps: u32,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trips() {
        let m = ClientMsg::Hello {
            token: Some("abc".into()),
        };
        let j = serde_json::to_string(&m).unwrap();
        assert_eq!(j, r#"{"type":"hello","token":"abc"}"#);
        assert_eq!(serde_json::from_str::<ClientMsg>(&j).unwrap(), m);
    }

    #[test]
    fn hello_without_token_parses() {
        let m: ClientMsg = serde_json::from_str(r#"{"type":"hello"}"#).unwrap();
        assert_eq!(m, ClientMsg::Hello { token: None });
    }

    #[test]
    fn ice_uses_browser_field_names() {
        // These must match RTCIceCandidateInit exactly, so the browser can pass
        // the object straight to addIceCandidate with no translation.
        let m = HostMsg::Ice {
            candidate: IceCandidate {
                candidate: "candidate:1 1 udp".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
            },
        };
        let j = serde_json::to_string(&m).unwrap();
        assert!(j.contains(r#""sdpMid":"0""#), "{j}");
        assert!(j.contains(r#""sdpMLineIndex":0"#), "{j}");
    }

    #[test]
    fn answer_from_browser_shape_parses() {
        let j = r#"{"type":"answer","sdp":"v=0\r\n"}"#;
        match serde_json::from_str::<ClientMsg>(j).unwrap() {
            ClientMsg::Answer { sdp } => assert!(sdp.starts_with("v=0")),
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
