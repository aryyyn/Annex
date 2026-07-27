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
use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_foundation::NSString;

pub mod ffi;

pub use ffi::{availability, is_available};

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

impl std::fmt::Display for VdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClassNotFound(c) => {
                write!(
                    f,
                    "private class {c} not found: this macOS version moved it"
                )
            }
            Self::CreateFailed => write!(f, "initWithDescriptor: returned nil"),
            Self::ApplySettingsFailed => write!(f, "applySettings: returned NO"),
            Self::Unsupported => write!(f, "no CoreGraphics on this platform"),
        }
    }
}

impl std::error::Error for VdError {}

/// Colour primaries, lifted from Generic RGB Profile.icc.
///
/// BetterDummy sets these and we follow. Leaving them at zero gives macOS a
/// degenerate colour space to derive a profile from, which is not worth the
/// risk for four constants.
const WHITE_POINT: CGPoint = CGPoint { x: 0.950, y: 1.000 };
const RED_PRIMARY: CGPoint = CGPoint { x: 0.454, y: 0.242 };
const GREEN_PRIMARY: CGPoint = CGPoint { x: 0.353, y: 0.674 };
const BLUE_PRIMARY: CGPoint = CGPoint { x: 0.157, y: 0.084 };

/// Arbitrary but stable vendor id, matching BetterDummy's choice. macOS only
/// uses it to tell displays apart.
const VENDOR_ID: u32 = 0xF0F0;

/// A live virtual monitor.
///
/// The wrapped Objective-C object *is* the display. Hold this value for as long
/// as you want the monitor to exist, and drop it to remove the monitor. Losing
/// it early leaves the user with a ghost display.
///
/// Not `Send` or `Sync`: the underlying object is tied to the dispatch queue
/// handed to its descriptor, and the auto traits are correctly not derived
/// because of the raw pointer.
pub struct VirtualDisplay {
    /// Owned, +1 from `alloc`/`initWithDescriptor:`. Released in `Drop`.
    display: *mut AnyObject,
    display_id: u32,
}

impl VirtualDisplay {
    /// Looks the private classes up at runtime, builds a descriptor and a
    /// settings object, and applies them.
    ///
    /// The sequence, which is not obvious and is easy to get subtly wrong:
    ///
    /// 1. Build and populate `CGVirtualDisplayDescriptor`. The dispatch queue
    ///    matters: some macOS versions refuse to create the display without one.
    /// 2. `initWithDescriptor:`. At this point the display object exists but has
    ///    no modes, so macOS does not show a monitor yet.
    /// 3. Build `CGVirtualDisplayMode` for each resolution you want to offer.
    /// 4. `applySettings:` with those modes. **This** is the call that makes the
    ///    monitor appear.
    ///
    /// Note step 4. Creating the object is not enough, which is a good thing to
    /// know before spending an hour wondering why System Settings is unchanged.
    pub fn create(cfg: &VirtualDisplayConfig) -> Result<Self, VdError> {
        let cls_desc = ffi::class(ffi::CLS_DESCRIPTOR)?;
        let cls_display = ffi::class(ffi::CLS_DISPLAY)?;
        let cls_mode = ffi::class(ffi::CLS_MODE)?;
        let cls_settings = ffi::class(ffi::CLS_SETTINGS)?;

        autoreleasepool(|_| unsafe {
            // ---- 1. descriptor -------------------------------------------
            let desc: *mut AnyObject = msg_send![cls_desc, alloc];
            let desc: *mut AnyObject = msg_send![desc, init];
            if desc.is_null() {
                return Err(VdError::CreateFailed);
            }

            let queue = ffi::dispatch_get_global_queue(ffi::QOS_CLASS_USER_INTERACTIVE, 0);
            let _: () = msg_send![desc, setQueue: queue];

            let name = NSString::from_str(&cfg.name);
            let _: () = msg_send![desc, setName: &*name];

            // maxPixelsWide/High is the ceiling for any mode, not the mode
            // itself. Keep it equal to the one mode we offer for now.
            let _: () = msg_send![desc, setMaxPixelsWide: cfg.width];
            let _: () = msg_send![desc, setMaxPixelsHigh: cfg.height];

            let size = CGSize {
                width: cfg.size_mm.0,
                height: cfg.size_mm.1,
            };
            let _: () = msg_send![desc, setSizeInMillimeters: size];

            let _: () = msg_send![desc, setWhitePoint: WHITE_POINT];
            let _: () = msg_send![desc, setRedPrimary: RED_PRIMARY];
            let _: () = msg_send![desc, setGreenPrimary: GREEN_PRIMARY];
            let _: () = msg_send![desc, setBluePrimary: BLUE_PRIMARY];

            let _: () = msg_send![desc, setVendorID: VENDOR_ID];
            let _: () = msg_send![desc, setProductID: 0x1234u32];
            let _: () = msg_send![desc, setSerialNum: 0x0001u32];

            // ---- 2. the display object -----------------------------------
            let display: *mut AnyObject = msg_send![cls_display, alloc];
            let display: *mut AnyObject = msg_send![display, initWithDescriptor: desc];
            let _: () = msg_send![desc, release];

            if display.is_null() {
                return Err(VdError::CreateFailed);
            }

            // ---- 3. modes ------------------------------------------------
            // Width and height are 32 bit here. See the provenance note in
            // ffi.rs: DeskPad's header says NSUInteger and is wrong.
            let mode: *mut AnyObject = msg_send![cls_mode, alloc];
            let mode: *mut AnyObject = msg_send![
                mode,
                initWithWidth: cfg.width,
                height: cfg.height,
                refreshRate: cfg.refresh_hz,
            ];
            if mode.is_null() {
                let _: () = msg_send![display, release];
                return Err(VdError::CreateFailed);
            }

            let cls_array = objc2::runtime::AnyClass::get(c"NSArray")
                .ok_or(VdError::ClassNotFound("NSArray"))?;
            let modes = [mode];
            let arr: *mut AnyObject = msg_send![cls_array, alloc];
            let arr: *mut AnyObject =
                msg_send![arr, initWithObjects: modes.as_ptr(), count: modes.len()];
            // The array retains the mode, so our +1 is now surplus.
            let _: () = msg_send![mode, release];

            // ---- 4. settings, which is what makes the monitor appear ------
            let settings: *mut AnyObject = msg_send![cls_settings, alloc];
            let settings: *mut AnyObject = msg_send![settings, init];
            let _: () = msg_send![settings, setHiDPI: u32::from(cfg.hidpi)];
            let _: () = msg_send![settings, setModes: arr];

            let ok: bool = msg_send![display, applySettings: settings];

            let _: () = msg_send![settings, release];
            let _: () = msg_send![arr, release];

            if !ok {
                let _: () = msg_send![display, release];
                return Err(VdError::ApplySettingsFailed);
            }

            let display_id: u32 = msg_send![display, displayID];

            Ok(Self {
                display,
                display_id,
            })
        })
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
        if !self.display.is_null() {
            unsafe {
                let _: () = msg_send![self.display, release];
            }
            self.display = std::ptr::null_mut();
        }
    }
}
