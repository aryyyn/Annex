//! Configuration types, shared so that no crate has to depend on another just
//! to name its own settings.

use std::net::SocketAddr;

/// Shape of the virtual monitor to create. Consumed by `annex-virtual-display`.
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualDisplayConfig {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
    /// True gives a Retina backing store at twice the logical resolution.
    pub hidpi: bool,
    /// Physical size in millimetres. macOS derives the default DPI from this,
    /// so it decides how large text looks on the client.
    pub size_mm: (f64, f64),
}

impl Default for VirtualDisplayConfig {
    fn default() -> Self {
        Self {
            name: "Annex Display".to_string(),
            width: 1920,
            height: 1080,
            refresh_hz: 60.0,
            hidpi: true,
            size_mm: (600.0, 340.0),
        }
    }
}

/// Pixel format requested from ScreenCaptureKit.
///
/// Prefer [`PixFmt::Nv12`]: it is VideoToolbox's native input, so choosing it
/// avoids a colour conversion between capture and encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixFmt {
    Nv12,
    Bgra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// Constrained Baseline or Main, Annex-B. The safe universal choice.
    H264,
    /// Roughly 30 to 40 percent smaller, but only for clients that negotiate it.
    Hevc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub codec: Codec,
    /// Keyframes also arrive on demand via a PLI from the client, so this can
    /// be generous. Frequent unforced keyframes just waste bitrate.
    pub keyframe_interval: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 12_000,
            codec: Codec::H264,
            keyframe_interval: 120,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostConfig {
    pub display: VirtualDisplayConfig,
    pub encoder: EncoderConfig,
    pub bind_addr: SocketAddr,
    /// Shared secret shown in the tray UI and baked into the QR code. Anyone on
    /// the LAN can reach the port, so sessions without it must be rejected.
    pub auth_token: Option<String>,
    /// Phase-2 gate. Leave false until CGEvent injection is trusted.
    pub allow_input: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            display: VirtualDisplayConfig::default(),
            encoder: EncoderConfig::default(),
            bind_addr: "0.0.0.0:8787".parse().expect("valid literal"),
            auth_token: None,
            allow_input: false,
        }
    }
}
