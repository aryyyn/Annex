//! Choosing which resolution the display actually runs at.
//!
//! # Entirely public API
//!
//! Nothing here is private. These are Quartz Display Services calls that work
//! on any monitor, virtual or otherwise. They live in this crate because it
//! owns the display's lifecycle, not because they need the same containment as
//! `ffi.rs`.
//!
//! # The distinction that cost an investigation
//!
//! `applySettings:` does not choose a resolution. It establishes what the
//! display *can* do: macOS takes `maxPixelsWide` and `maxPixelsHigh` from the
//! descriptor and synthesises a ladder of standard modes up to that ceiling.
//! Ask for 3840x2160 and the ladder really does reach 3840x2160.
//!
//! What it does not do is make that the *current* mode. macOS picks its own
//! default, which in practice is 1920x1080 whatever ceiling you set. Measuring
//! the current mode and concluding the request was ignored is the wrong
//! inference, and it is the one that was drawn here first: the ladder was
//! correct all along and only the selection was missing.
//!
//! So creating the display sets the ceiling, and [`set_mode`] picks from it.

use std::ffi::c_void;

pub type CGDirectDisplayID = u32;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCopyAllDisplayModes(
        display: CGDirectDisplayID,
        options: *const c_void,
    ) -> *const c_void;
    fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> *mut c_void;
    fn CGDisplaySetDisplayMode(
        display: CGDirectDisplayID,
        mode: *mut c_void,
        options: *const c_void,
    ) -> i32;
    fn CGDisplayModeGetWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetHeight(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: *mut c_void) -> f64;
    fn CGDisplayModeRelease(mode: *mut c_void);
    fn CFArrayGetCount(arr: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(arr: *const c_void, idx: isize) -> *mut c_void;
    fn CFRelease(cf: *const c_void);
}

/// One resolution a display can run at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayMode {
    /// Logical size, which is what windows are laid out in.
    pub width: u32,
    pub height: u32,
    /// Physical size of the backing store. Twice the logical size on a HiDPI
    /// mode, equal to it otherwise.
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_hz: f64,
}

impl DisplayMode {
    pub fn is_hidpi(&self) -> bool {
        self.pixel_width == self.width * 2
    }

    /// How this should read in a menu.
    pub fn label(&self) -> String {
        if self.is_hidpi() {
            format!("{} x {} (HiDPI)", self.width, self.height)
        } else {
            format!("{} x {}", self.width, self.height)
        }
    }
}

/// Every mode this display offers, largest first.
pub fn available(id: CGDirectDisplayID) -> Vec<DisplayMode> {
    let mut out = Vec::new();
    unsafe {
        let arr = CGDisplayCopyAllDisplayModes(id, std::ptr::null());
        if arr.is_null() {
            return out;
        }
        for i in 0..CFArrayGetCount(arr) {
            let m = CFArrayGetValueAtIndex(arr, i);
            if m.is_null() {
                continue;
            }
            out.push(DisplayMode {
                width: CGDisplayModeGetWidth(m) as u32,
                height: CGDisplayModeGetHeight(m) as u32,
                pixel_width: CGDisplayModeGetPixelWidth(m) as u32,
                pixel_height: CGDisplayModeGetPixelHeight(m) as u32,
                refresh_hz: CGDisplayModeGetRefreshRate(m),
            });
        }
        CFRelease(arr);
    }
    // Largest first, since that is the order a resolution menu wants.
    out.sort_by_key(|m| std::cmp::Reverse((m.width, m.height)));
    out.dedup();
    out
}

/// The mode the display is running now.
pub fn current(id: CGDirectDisplayID) -> Option<DisplayMode> {
    unsafe {
        let m = CGDisplayCopyDisplayMode(id);
        if m.is_null() {
            return None;
        }
        let out = DisplayMode {
            width: CGDisplayModeGetWidth(m) as u32,
            height: CGDisplayModeGetHeight(m) as u32,
            pixel_width: CGDisplayModeGetPixelWidth(m) as u32,
            pixel_height: CGDisplayModeGetPixelHeight(m) as u32,
            refresh_hz: CGDisplayModeGetRefreshRate(m),
        };
        CGDisplayModeRelease(m);
        Some(out)
    }
}

/// Switches the display to the closest available mode to `width` by `height`.
///
/// Closest rather than exact, because the ladder macOS synthesises contains
/// standard sizes and an arbitrary request may not appear in it verbatim.
/// Returns the mode actually selected.
pub fn set_mode(id: CGDirectDisplayID, width: u32, height: u32) -> Option<DisplayMode> {
    let target = (width as i64, height as i64);
    let modes = available(id);
    if modes.is_empty() {
        return None;
    }

    // Rank by squared distance in pixel count, so a 16:9 request does not
    // silently land on a 4:3 mode of similar area.
    let best = modes.iter().min_by_key(|m| {
        let dw = m.width as i64 - target.0;
        let dh = m.height as i64 - target.1;
        dw * dw + dh * dh
    })?;

    unsafe {
        let arr = CGDisplayCopyAllDisplayModes(id, std::ptr::null());
        if arr.is_null() {
            return None;
        }
        let mut applied = None;
        for i in 0..CFArrayGetCount(arr) {
            let m = CFArrayGetValueAtIndex(arr, i);
            if m.is_null() {
                continue;
            }
            if CGDisplayModeGetWidth(m) as u32 == best.width
                && CGDisplayModeGetHeight(m) as u32 == best.height
                && CGDisplayModeGetPixelWidth(m) as u32 == best.pixel_width
            {
                // kCGErrorSuccess is 0.
                if CGDisplaySetDisplayMode(id, m, std::ptr::null()) == 0 {
                    applied = Some(*best);
                }
                break;
            }
        }
        CFRelease(arr);
        applied
    }
}
