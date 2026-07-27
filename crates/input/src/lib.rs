//! Phase 2. Turns client input events into macOS events.
//!
//! Requires the Accessibility permission, which is a separate grant from Screen
//! Recording and prompts separately.
//!
//! # Coordinate mapping is where the bugs live
//!
//! The client sends coordinates normalised to its own view. Landing them on the
//! right pixel means going through the virtual display's origin in the global
//! display arrangement, which is not the origin of the main screen and moves
//! whenever the user rearranges displays in System Settings, and then through
//! the HiDPI scale factor, because a Retina backing store has twice the pixels
//! of its logical size. Getting either wrong gives a cursor with a constant
//! offset or one that drifts as it moves.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

use annex_core::InputEvent;

#[derive(Debug)]
pub enum InputError {
    /// The Accessibility grant is missing. The user has to add the app under
    /// System Settings, Privacy and Security, Accessibility.
    PermissionDenied,
    PostFailed,
    /// Built for a platform with no CoreGraphics. Unused until the workspace
    /// gains target gating at M7; kept so the cross-platform story is explicit.
    Unsupported,
}

pub struct Injector {
    display_id: u32,
}

impl Injector {
    pub fn new(display_id: u32) -> Result<Self, InputError> {
        let _ = display_id;
        todo!("M5: check the Accessibility grant, cache the display bounds")
    }

    /// Maps into the virtual display's coordinate space and posts.
    ///
    /// Must run on the main thread.
    pub fn inject(&self, ev: InputEvent) -> Result<(), InputError> {
        let _ = (&self.display_id, ev);
        todo!("M5: CGEventCreateMouseEvent or KeyboardEvent, then CGEventPost")
    }
}

/// Whether the Accessibility grant is already in place.
pub fn has_permission() -> bool {
    todo!("M5: AXIsProcessTrusted")
}
