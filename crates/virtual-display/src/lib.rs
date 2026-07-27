//! Creates and destroys a macOS virtual monitor.
//!
//! # The blast radius rule
//!
//! This is the only crate in the workspace allowed to touch undocumented Apple
//! APIs. Everything else consumes the plain `u32` display id that
//! [`VirtualDisplay::display_id`] hands out and stays entirely on public
//! frameworks. When a macOS release changes the shape of `CGVirtualDisplay`,
//! exactly one file needs attention.
//!
//! # Why this is possible at all
//!
//! Screen capture can only see pixels macOS is already rendering somewhere. To
//! get a *new* desktop area rather than a copy of an existing one, some display
//! has to exist for the windows to occupy. Hardware does this over HDMI and
//! EDID, a dummy plug fakes it in hardware, and `CGVirtualDisplay` fakes it in
//! software. No kext, no DriverKit, no root, and no disabling SIP.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

use annex_core::VirtualDisplayConfig;

mod ffi;

#[derive(Debug)]
pub enum VdError {
    /// `AnyClass::get` came back empty. Either the macOS version moved these
    /// classes, or something is very wrong with the CoreGraphics load.
    ClassNotFound(&'static str),
    /// `initWithDescriptor:` returned nil.
    CreateFailed,
    /// `applySettings:` returned NO. Usually an impossible mode.
    ApplySettingsFailed,
    /// Built for a platform with no CoreGraphics. Unused until the workspace
    /// gains target gating at M7; kept so the cross-platform story is explicit.
    Unsupported,
}

/// A live virtual monitor.
///
/// The wrapped Objective-C object *is* the display. Hold this value for as long
/// as you want the monitor to exist, and drop it to remove the monitor. Losing
/// it early leaves the user with a ghost display.
pub struct VirtualDisplay {
    #[allow(dead_code)]
    display_id: u32,
    // M0: retained *mut CGVirtualDisplay, released in Drop.
}

impl VirtualDisplay {
    /// Looks the private classes up at runtime, builds a descriptor and a
    /// settings object, and applies them.
    ///
    /// Recipe, spelled out in appendix A of the design doc:
    ///
    /// 1. `AnyClass::get` each of the four classes.
    /// 2. Descriptor: name, `maxPixelsWide`, `maxPixelsHigh`,
    ///    `sizeInMillimeters`, vendor and product ids, and a dispatch queue.
    ///    Some macOS versions insist on the queue being set.
    /// 3. `alloc` then `initWithDescriptor:`.
    /// 4. Settings: one `CGVirtualDisplayMode`, plus the `hiDPI` flag.
    /// 5. `applySettings:`, then read `displayID` back off the object.
    ///
    /// Copy the header from DeskPad or BetterDummy and cross-check both.
    /// Field signatures have drifted across releases, so do not transcribe it
    /// from memory. Do not copy from SimpleDisplay: it is GPL-3.0 and would
    /// relicense this crate.
    pub fn create(cfg: &VirtualDisplayConfig) -> Result<Self, VdError> {
        let _ = cfg;
        todo!("M0: the whole milestone lives here")
    }

    /// The `CGDirectDisplayID`. Hand this to `annex-capture` so ScreenCaptureKit
    /// knows which display to filter its stream to.
    pub fn display_id(&self) -> u32 {
        self.display_id
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        // Releasing the Objective-C object removes the monitor. This must run
        // on every exit path, including panics, or the user is left with a
        // display they cannot get rid of without logging out.
    }
}
