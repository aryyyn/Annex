//! A display census using only public Quartz Display Services.
//!
//! This exists so M0 can *prove* its result instead of asking you to squint at
//! System Settings. Nothing here touches a private API, which is the point: if
//! a display the public API can see appears and then disappears, the private
//! side genuinely worked.

use std::ffi::c_void;

pub type CGDirectDisplayID = u32;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active: *mut CGDirectDisplayID,
        count: *mut u32,
    ) -> i32;
    fn CGDisplayPixelsWide(display: CGDirectDisplayID) -> usize;
    fn CGDisplayPixelsHigh(display: CGDirectDisplayID) -> usize;
    fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> i32;
    fn CGDisplayVendorNumber(display: CGDirectDisplayID) -> u32;

    // The pair that settles whether a HiDPI backing store exists.
    // `GetWidth` is points, `GetPixelWidth` is real pixels. On a Retina display
    // the second is twice the first. Nothing else reports this distinction.
    fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> *mut c_void;
    fn CGDisplayModeGetWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetHeight(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *mut c_void) -> usize;
    fn CGDisplayModeRelease(mode: *mut c_void);
}

/// Points and backing pixels for a display's current mode, as
/// `(point_w, point_h, pixel_w, pixel_h)`.
///
/// When the pixel figures are double the point figures, macOS is rendering a
/// Retina backing store and a capture at 1x throws away three quarters of the
/// pixels it drew.
pub fn mode_geometry(id: CGDirectDisplayID) -> Option<(usize, usize, usize, usize)> {
    unsafe {
        let mode = CGDisplayCopyDisplayMode(id);
        if mode.is_null() {
            return None;
        }
        let g = (
            CGDisplayModeGetWidth(mode),
            CGDisplayModeGetHeight(mode),
            CGDisplayModeGetPixelWidth(mode),
            CGDisplayModeGetPixelHeight(mode),
        );
        CGDisplayModeRelease(mode);
        Some(g)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub id: CGDirectDisplayID,
    pub width: usize,
    pub height: usize,
    pub builtin: bool,
    pub vendor: u32,
}

/// Every display macOS currently considers active.
pub fn active() -> Vec<DisplayInfo> {
    const MAX: u32 = 32;
    let mut ids = [0u32; MAX as usize];
    let mut count = 0u32;

    let err = unsafe { CGGetActiveDisplayList(MAX, ids.as_mut_ptr(), &mut count) };
    if err != 0 {
        return Vec::new();
    }

    ids[..count as usize]
        .iter()
        .map(|&id| unsafe {
            DisplayInfo {
                id,
                width: CGDisplayPixelsWide(id),
                height: CGDisplayPixelsHigh(id),
                builtin: CGDisplayIsBuiltin(id) != 0,
                vendor: CGDisplayVendorNumber(id),
            }
        })
        .collect()
}

pub fn render(list: &[DisplayInfo], highlight: Option<u32>) -> String {
    list.iter()
        .map(|d| {
            let mark = if Some(d.id) == highlight {
                " <-- ours"
            } else {
                ""
            };
            let kind = if d.builtin { "built-in" } else { "external" };
            format!(
                "      {:>10}  {:>5} x {:<5} {:<9} vendor 0x{:04X}{}",
                d.id, d.width, d.height, kind, d.vendor, mark
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
