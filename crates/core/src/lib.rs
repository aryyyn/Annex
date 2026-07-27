//! Shared vocabulary for every Annex crate.
//!
//! Everything here is plain Rust with no Apple or network dependencies, so the
//! future Windows client can reuse it unchanged. Platform-specific code lives
//! in `annex-capture`, `annex-encoder`, `annex-virtual-display` and
//! `annex-input`.

pub mod config;
pub mod error;
pub mod frame;
pub mod input;
pub mod pipeline;
pub mod protocol;

pub use config::{Codec, EncoderConfig, HostConfig, PixFmt, VirtualDisplayConfig};
pub use error::{Error, Result};
pub use frame::{EncodedSample, Timestamp};
pub use input::InputEvent;
pub use pipeline::Pipeline;
