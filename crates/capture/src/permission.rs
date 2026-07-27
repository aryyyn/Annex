//! The Screen Recording gate (TCC).
//!
//! ScreenCaptureKit will happily build a stream and start it without the
//! permission. It just never delivers a frame. That failure looks exactly like
//! a hung pipeline, so always check here first and say so plainly.
//!
//! # The part that bites during development
//!
//! TCC attributes the permission to the *application*, and a bare binary run
//! from a shell is not one. `cargo run` therefore asks macOS to trust your
//! terminal, and the grant lands on Terminal or iTerm or the IDE, not on Annex.
//! That is fine for M1 but it means:
//!
//! - The prompt may name your terminal rather than this program.
//! - Granting it usually requires **restarting the terminal**, because TCC
//!   decisions are read at process start.
//! - Shipping needs a real signed `.app` bundle so the grant attaches to Annex
//!   itself. That is an M6 concern.

use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

/// Whether the Screen Recording grant is already in place.
///
/// Does not prompt, so it is safe to call on every start.
pub fn has_permission() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Triggers the system prompt if the grant is missing.
///
/// Returns whether permission is held *now*. Note it usually returns false the
/// first time even when the user says yes: the decision is picked up at process
/// start, so the answer only takes effect on the next launch.
pub fn request_permission() -> bool {
    CGRequestScreenCaptureAccess()
}

/// A human-readable explanation for when the grant is missing.
pub fn permission_help() -> &'static str {
    "Screen Recording permission is required.\n\
     \n\
     macOS should have shown a prompt. If it did not, or you dismissed it, open\n\
     System Settings > Privacy & Security > Screen & System Audio Recording and\n\
     enable the entry for your terminal.\n\
     \n\
     Then restart the terminal completely. TCC decisions are read when a process\n\
     starts, so a running shell keeps the old answer."
}
