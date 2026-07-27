//! One error type per layer, funnelled into [`Error`] at the pipeline boundary.
//!
//! Library crates should keep their own specific errors (`VdError`,
//! `CaptureError`, and so on) and convert here, so a caller can tell a missing
//! Screen Recording permission from a bind failure.

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// The private CGVirtualDisplay API refused or is not present.
    VirtualDisplay(String),
    /// ScreenCaptureKit could not start. Usually a missing TCC grant.
    Capture(String),
    /// VideoToolbox rejected the session or a frame.
    Encode(String),
    /// Signalling, ICE, or the peer connection failed.
    Transport(String),
    /// CGEvent injection failed. Usually a missing Accessibility grant.
    Input(String),
    Config(String),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VirtualDisplay(m) => write!(f, "virtual display: {m}"),
            Self::Capture(m) => write!(f, "capture: {m}"),
            Self::Encode(m) => write!(f, "encoder: {m}"),
            Self::Transport(m) => write!(f, "transport: {m}"),
            Self::Input(m) => write!(f, "input: {m}"),
            Self::Config(m) => write!(f, "config: {m}"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
