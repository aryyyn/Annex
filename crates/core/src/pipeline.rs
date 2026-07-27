//! Owns the whole host-side chain and its teardown.
//!
//! The single most important behaviour in this file is Drop. The
//! `CGVirtualDisplay` object *is* the monitor: if it leaks, the user is left
//! with a ghost display they cannot remove without logging out. Every exit
//! path, including a panic, has to run this.

use crate::config::HostConfig;
use crate::error::Result;

pub struct Pipeline {
    cfg: HostConfig,
    // M0: virtual_display: annex_virtual_display::VirtualDisplay,
    // M1: capturer:        annex_capture::Capturer,
    // M2: encoder:         annex_encoder::Encoder,
    // M3: transport:       annex_transport::Server,
}

impl Pipeline {
    /// Brings the chain up in dependency order: display, then capture, then
    /// encode, then transport. Any failure unwinds the stages already started.
    pub fn start(cfg: HostConfig) -> Result<Self> {
        let _ = &cfg;
        todo!("M0: create the virtual display, then build up the rest")
    }

    /// The URL to show in the tray menu and encode into the QR code.
    pub fn connect_url(&self) -> String {
        let _ = &self.cfg;
        todo!("M3: LAN IP plus bind port plus auth token")
    }

    /// Explicit teardown, for when the caller wants to observe failures. Drop
    /// covers the same ground for every other exit path.
    pub fn shutdown(self) -> Result<()> {
        todo!("stop capture, drain the encoder, close sessions, drop the display")
    }
}
