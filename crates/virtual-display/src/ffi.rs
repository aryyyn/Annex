//! Runtime bindings to the private CoreGraphics display classes.
//!
//! These four classes live inside CoreGraphics.framework but are absent from
//! the SDK headers, so there is nothing to link against. We look them up by
//! name with the Objective-C runtime instead, which means no special linker
//! flags: CoreGraphics is already loaded into the process.
//!
//! # Provenance
//!
//! Cross-checked against the two MIT references on 27 July 2026, per rule 4 in
//! CLAUDE.md:
//!
//! - `Stengo/DeskPad`, `DeskPad/CGVirtualDisplayPrivate.h`
//! - `waydabber/BetterDummy` branch `opensource`, `BetterDummy/Bridging-Header.h`
//!
//! They disagree, and the disagreement matters. DeskPad declares
//! `initWithWidth:height:refreshRate:` as taking `NSUInteger` (64 bit) and
//! `CGFloat`. BetterDummy declares `unsigned int` (32 bit) and `double`, and
//! backs it with a class dump showing the real ivars:
//!
//! ```objc
//! unsigned int _width;
//! unsigned int _height;
//! double _refreshRate;
//! ```
//!
//! The ivar layout settles it: width and height are 32 bit. DeskPad's
//! declaration happens to work on arm64 because the low half of the register
//! carries the value for any sane resolution, but it is wrong, and we follow
//! BetterDummy. This is the exact drift that rule 4 exists to catch.
//!
//! Two further corrections against appendix A of the design doc, which was
//! written from memory and was wrong on both counts:
//!
//! - `CGVirtualDisplaySettings` has no `rotation` property. Only `modes` and
//!   `hiDPI`.
//! - `CGVirtualDisplayDescriptor` has four colour primary properties the
//!   appendix never mentioned: `whitePoint`, `redPrimary`, `greenPrimary` and
//!   `bluePrimary`, all `CGPoint`.
//!
//! # The shape, as verified
//!
//! ```objc
//! @interface CGVirtualDisplayMode : NSObject
//! @property(readonly, nonatomic) double refreshRate;
//! @property(readonly, nonatomic) unsigned int height;
//! @property(readonly, nonatomic) unsigned int width;
//! - (id)initWithWidth:(unsigned int)w height:(unsigned int)h refreshRate:(double)r;
//! @end
//!
//! @interface CGVirtualDisplaySettings : NSObject
//! @property(nonatomic) unsigned int hiDPI;
//! @property(retain, nonatomic) NSArray *modes;
//! - (id)init;
//! @end
//!
//! @interface CGVirtualDisplayDescriptor : NSObject
//! @property(retain, nonatomic) id queue;
//! @property(retain, nonatomic) NSString *name;
//! @property(nonatomic) struct CGPoint whitePoint, redPrimary, greenPrimary, bluePrimary;
//! @property(nonatomic) unsigned int maxPixelsHigh, maxPixelsWide;
//! @property(nonatomic) struct CGSize sizeInMillimeters;
//! @property(nonatomic) unsigned int serialNum, productID, vendorID;
//! @property(copy, nonatomic) id terminationHandler;
//! - (id)init;
//! - (id)dispatchQueue;
//! - (void)setDispatchQueue:(id)arg1;
//! @end
//!
//! @interface CGVirtualDisplay : NSObject
//! @property(readonly, nonatomic) unsigned int displayID;
//! @property(readonly, nonatomic) unsigned int hiDPI;
//! @property(readonly, nonatomic) NSArray *modes;
//! - (id)initWithDescriptor:(id)arg1;
//! - (BOOL)applySettings:(id)arg1;
//! @end
//! ```

use crate::VdError;
use objc2::runtime::{AnyClass, AnyObject};
use std::ffi::CStr;

pub const CLS_MODE: &CStr = c"CGVirtualDisplayMode";
pub const CLS_DESCRIPTOR: &CStr = c"CGVirtualDisplayDescriptor";
pub const CLS_SETTINGS: &CStr = c"CGVirtualDisplaySettings";
pub const CLS_DISPLAY: &CStr = c"CGVirtualDisplay";

pub const ALL_CLASSES: [&CStr; 4] = [CLS_MODE, CLS_DESCRIPTOR, CLS_SETTINGS, CLS_DISPLAY];

/// Quality of service for the descriptor's dispatch queue.
///
/// BetterDummy uses `.userInteractive`, which is the right tier: this queue
/// services display lifecycle callbacks and should not sit behind background
/// work.
pub const QOS_CLASS_USER_INTERACTIVE: isize = 0x21;

// libdispatch is already in the process. `dispatch_queue_t` is an Objective-C
// object on macOS, so the returned pointer goes straight into `setQueue:`.
unsafe extern "C" {
    pub fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut AnyObject;
}

/// Whether all four private classes resolve.
///
/// Worth calling at startup so the app can fail with a clear message on a macOS
/// version that moved them, rather than crashing mid-setup.
pub fn is_available() -> bool {
    ALL_CLASSES.iter().all(|n| AnyClass::get(n).is_some())
}

/// Per-class availability, for a diagnostic that says which class vanished.
pub fn availability() -> Vec<(&'static str, bool)> {
    ALL_CLASSES
        .iter()
        .map(|n| (n.to_str().unwrap_or("?"), AnyClass::get(n).is_some()))
        .collect()
}

/// Looks a class up, naming it in the error if it is gone.
pub(crate) fn class(name: &'static CStr) -> Result<&'static AnyClass, VdError> {
    AnyClass::get(name).ok_or(VdError::ClassNotFound(match name.to_str() {
        Ok(s) => s,
        Err(_) => "?",
    }))
}
