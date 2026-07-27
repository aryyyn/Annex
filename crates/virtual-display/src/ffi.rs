//! Runtime bindings to the private CoreGraphics display classes.
//!
//! These four classes live inside CoreGraphics.framework but are absent from
//! the SDK headers, so there is nothing to link against. We look them up by
//! name with the Objective-C runtime instead, which means no special linker
//! flags: CoreGraphics is already loaded into the process.
//!
//! The shape below is here so you recognise it, not as an authority. Copy the
//! real thing from DeskPad or BetterDummy, both MIT, and cross-check them
//! against each other.
//!
//! ```objc
//! @interface CGVirtualDisplayMode : NSObject
//! - (instancetype)initWithWidth:(uint32_t)w height:(uint32_t)h refreshRate:(double)r;
//! @end
//!
//! @interface CGVirtualDisplayDescriptor : NSObject
//! @property(retain) dispatch_queue_t queue;
//! @property(copy)   NSString *name;
//! @property uint32_t maxPixelsWide, maxPixelsHigh;
//! @property CGSize   sizeInMillimeters;
//! @property uint32_t vendorID, productID, serialNum;
//! @property(copy)   void (^terminationHandler)(id, id);
//! @end
//!
//! @interface CGVirtualDisplaySettings : NSObject
//! @property(retain) NSArray<CGVirtualDisplayMode *> *modes;
//! @property uint32_t hiDPI;
//! @property uint32_t rotation;
//! @end
//!
//! @interface CGVirtualDisplay : NSObject
//! - (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)d;
//! - (BOOL)applySettings:(CGVirtualDisplaySettings *)s;
//! @property(readonly) CGDirectDisplayID displayID;
//! @end
//! ```

#![allow(dead_code)]

pub const CLS_MODE: &str = "CGVirtualDisplayMode";
pub const CLS_DESCRIPTOR: &str = "CGVirtualDisplayDescriptor";
pub const CLS_SETTINGS: &str = "CGVirtualDisplaySettings";
pub const CLS_DISPLAY: &str = "CGVirtualDisplay";

/// Cheap probe for whether the private API is present at all.
///
/// Worth calling at startup so the app can fail with a clear message on a macOS
/// version that moved these classes, rather than crashing mid-setup.
pub fn is_available() -> bool {
    todo!("M0: AnyClass::get on all four names, return whether every one resolved")
}
