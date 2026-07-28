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

/// Whether this process is running from an application bundle.
///
/// It changes the advice materially. A bundle is granted permission under its
/// own name and is relaunched on its own; a bare binary borrows the terminal's
/// grant and needs the terminal restarted. Telling someone to restart a
/// terminal they are not using is how a correct message becomes a wrong one.
pub fn is_bundled() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

/// A human-readable explanation for when the grant is missing.
pub fn permission_help() -> String {
    if is_bundled() {
        "Screen Recording permission is required.\n\
         \n\
         Open System Settings > Privacy & Security > Screen & System Audio\n\
         Recording and enable Annex, then open Annex again.\n\
         \n\
         macOS applies this permission only when an app starts, so it has to be\n\
         relaunched after you allow it.\n\
         \n\
         If Annex is already listed but still refused, this build was signed\n\
         ad-hoc and macOS sees each rebuild as a different app. Remove the old\n\
         entry and add this one."
            .to_string()
    } else {
        "Screen Recording permission is required.\n\
         \n\
         Running from a terminal means macOS attaches the grant to the terminal\n\
         rather than to Annex. Open System Settings > Privacy & Security >\n\
         Screen & System Audio Recording and enable your terminal.\n\
         \n\
         Then quit and reopen the terminal completely. TCC decisions are read\n\
         when a process starts, so a running shell keeps the old answer.\n\
         \n\
         Building the app with scripts/bundle.sh avoids this: the grant then\n\
         belongs to Annex itself."
            .to_string()
    }
}
