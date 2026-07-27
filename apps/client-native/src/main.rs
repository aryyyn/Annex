//! Phase 2, milestone M7. Native client.
//!
//! Speaks the same signalling protocol as the browser client and reuses
//! `annex-transport` unchanged, so this is a rendering and decoding problem
//! only. The gain over a browser is a smaller jitter buffer, no compositor in
//! the way, and real full-screen behaviour.
//!
//! Cross platform in principle. Hardware decode is the part that is not: expect
//! Media Foundation on Windows and VideoToolbox on macOS behind one trait.

fn main() {
    todo!("M7: winit window, wgpu surface, hardware decode, then paint")
}
