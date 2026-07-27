//! Turns client input events into macOS events.
//!
//! Requires the **Accessibility** permission, which is a separate grant from
//! Screen Recording and prompts separately.
//!
//! # This is the crate that lets a remote machine act on yours
//!
//! Everything else in Annex is read-only: a client watches. This one moves the
//! cursor and types. It is therefore gated twice over, by the shared token that
//! guards every session and again by `allow_input`, which defaults to off. The
//! menu bar shows when it is on, because a capability like this should never be
//! invisible to the person whose machine it is.
//!
//! # Coordinate mapping is where the bugs live
//!
//! The client sends coordinates normalised to 0.0 to 1.0 across the display it
//! is viewing, never pixels. It cannot send pixels correctly: it does not know
//! the display's origin in the global arrangement, which is not the origin of
//! the main screen and moves whenever displays are rearranged, and it does not
//! know the backing scale. Both can change mid-session.
//!
//! So the host does the mapping, reading the display's current bounds every
//! time rather than caching them. Caching is exactly how you get a cursor with
//! a constant offset after someone drags a display in System Settings.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

pub mod keymap;

use annex_core::input::{KeyMods, MouseButton};
use annex_core::InputEvent;
use std::ffi::c_void;

#[derive(Debug)]
pub enum InputError {
    /// The Accessibility grant is missing. The user has to add the app under
    /// System Settings, Privacy and Security, Accessibility.
    PermissionDenied,
    /// The display vanished, so there is nothing to map coordinates onto.
    NoDisplay,
    PostFailed,
    /// Built for a platform with no CoreGraphics.
    Unsupported,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "Accessibility permission not granted"),
            Self::NoDisplay => write!(f, "the target display is gone"),
            Self::PostFailed => write!(f, "CGEventPost failed"),
            Self::Unsupported => write!(f, "no CoreGraphics on this platform"),
        }
    }
}

impl std::error::Error for InputError {}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

// All public Quartz Display Services and Quartz Event Services.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        position: CGPoint,
        button: u32,
    ) -> *mut c_void;
    fn CGEventCreateScrollWheelEvent(
        source: *const c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
    ) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(
        source: *const c_void,
        virtual_key: u16,
        key_down: bool,
    ) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventSetIntegerValueField(event: *mut c_void, field: u32, value: i64);
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

// CGEventType values.
const MOUSE_MOVED: u32 = 5;
const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;
const RIGHT_DOWN: u32 = 3;
const RIGHT_UP: u32 = 4;
const OTHER_DOWN: u32 = 25;
const OTHER_UP: u32 = 26;
const LEFT_DRAGGED: u32 = 6;
const RIGHT_DRAGGED: u32 = 7;
const OTHER_DRAGGED: u32 = 27;

// CGMouseButton values.
const BUTTON_LEFT: u32 = 0;
const BUTTON_RIGHT: u32 = 1;
const BUTTON_CENTER: u32 = 2;

/// `kCGHIDEventTap`: post as though the hardware generated it, so the event
/// reaches every application rather than only the focused one.
const HID_EVENT_TAP: u32 = 0;

/// `kCGScrollEventUnitPixel`. Line units would quantise a trackpad's smooth
/// scrolling into jumps.
const SCROLL_UNIT_PIXEL: u32 = 0;

/// `kCGMouseEventClickState`: how many clicks in this sequence. Without setting
/// it, a double click reads as two unrelated single clicks and nothing that
/// depends on double clicking works.
const FIELD_CLICK_STATE: u32 = 1;

// CGEventFlags.
const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CONTROL: u64 = 0x0004_0000;
const FLAG_ALTERNATE: u64 = 0x0008_0000;
const FLAG_COMMAND: u64 = 0x0010_0000;

fn flags_for(m: KeyMods) -> u64 {
    let mut f = 0;
    if m.shift {
        f |= FLAG_SHIFT;
    }
    if m.ctrl {
        f |= FLAG_CONTROL;
    }
    if m.alt {
        f |= FLAG_ALTERNATE;
    }
    if m.meta {
        f |= FLAG_COMMAND;
    }
    f
}

/// Whether the Accessibility grant is in place.
///
/// Unlike Screen Recording there is no "request" call that prompts usefully:
/// macOS shows its own dialogue the first time an app tries to post an event,
/// and the grant only applies after a relaunch.
pub fn has_permission() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn permission_help() -> &'static str {
    "Accessibility permission is required to control this Mac from the client.\n\
     \n\
     Open System Settings > Privacy & Security > Accessibility and enable Annex,\n\
     then relaunch. macOS applies this permission only when an app starts."
}

/// Injects events into the Mac, targeted at one display.
pub struct Injector {
    display_id: u32,
    /// Which button is currently held, so a move can be sent as a drag.
    ///
    /// macOS distinguishes moving from dragging. Sending a plain move while a
    /// button is down means text selection and drag-and-drop simply do not
    /// work, which is a confusing failure because clicking itself looks fine.
    held: Option<MouseButton>,
    /// Recent clicks, for synthesising double and triple clicks.
    last_click: Option<(std::time::Instant, CGPoint, u32)>,
}

impl Injector {
    pub fn new(display_id: u32) -> Result<Self, InputError> {
        if !has_permission() {
            return Err(InputError::PermissionDenied);
        }
        Ok(Self {
            display_id,
            held: None,
            last_click: None,
        })
    }

    /// The display can be swapped when the resolution changes.
    pub fn set_display(&mut self, display_id: u32) {
        self.display_id = display_id;
    }

    /// Maps normalised coordinates onto the display's global position.
    ///
    /// Read fresh every time rather than cached: the user can rearrange
    /// displays in System Settings mid-session, and a cached origin then puts
    /// the cursor at a constant offset from where it belongs.
    fn to_global(&self, nx: f64, ny: f64) -> CGPoint {
        let b = unsafe { CGDisplayBounds(self.display_id) };
        let (x, y) = map_normalised(nx, ny, b.origin.x, b.origin.y, b.size.width, b.size.height);
        CGPoint { x, y }
    }

    /// Applies one event.
    ///
    /// Must run on the main thread: `CGEventPost` is documented as requiring
    /// it, and posting from a tokio worker produces events that are silently
    /// dropped rather than an error.
    pub fn inject(&mut self, ev: InputEvent) -> Result<(), InputError> {
        match ev {
            InputEvent::MouseMove { x, y } => {
                let pos = self.to_global(x, y);
                // A move with a button held has to be a drag, or selection and
                // drag-and-drop do nothing.
                let (kind, button) = match self.held {
                    Some(MouseButton::Left) => (LEFT_DRAGGED, BUTTON_LEFT),
                    Some(MouseButton::Right) => (RIGHT_DRAGGED, BUTTON_RIGHT),
                    Some(MouseButton::Middle) => (OTHER_DRAGGED, BUTTON_CENTER),
                    None => (MOUSE_MOVED, BUTTON_LEFT),
                };
                self.post_mouse(kind, pos, button, 0)
            }

            InputEvent::MouseButton { btn, down } => {
                // Without a position of its own, a click lands wherever the
                // cursor already is, which is what the client intends: it
                // always sends a move first.
                let pos = self.cursor_or(0.5, 0.5);
                let (kind, button) = match (btn, down) {
                    (MouseButton::Left, true) => (LEFT_DOWN, BUTTON_LEFT),
                    (MouseButton::Left, false) => (LEFT_UP, BUTTON_LEFT),
                    (MouseButton::Right, true) => (RIGHT_DOWN, BUTTON_RIGHT),
                    (MouseButton::Right, false) => (RIGHT_UP, BUTTON_RIGHT),
                    (MouseButton::Middle, true) => (OTHER_DOWN, BUTTON_CENTER),
                    (MouseButton::Middle, false) => (OTHER_UP, BUTTON_CENTER),
                };
                self.held = if down { Some(btn) } else { None };
                let clicks = if down {
                    self.click_count(pos)
                } else {
                    self.last_clicks()
                };
                self.post_mouse(kind, pos, button, clicks)
            }

            InputEvent::Scroll { dx, dy } => unsafe {
                // Negated because the browser reports how far the content
                // moved, and macOS wants how far the wheel turned.
                let ev = CGEventCreateScrollWheelEvent(
                    std::ptr::null(),
                    SCROLL_UNIT_PIXEL,
                    2,
                    -dy as i32,
                    -dx as i32,
                );
                if ev.is_null() {
                    return Err(InputError::PostFailed);
                }
                CGEventPost(HID_EVENT_TAP, ev);
                release(ev);
                Ok(())
            },

            InputEvent::Key { code, down, mods } => {
                // Unknown keys are dropped rather than guessed at: typing the
                // wrong character is worse than typing nothing.
                let Some(vk) = keymap::code_to_virtual(&code) else {
                    return Ok(());
                };
                unsafe {
                    let ev = CGEventCreateKeyboardEvent(std::ptr::null(), vk, down);
                    if ev.is_null() {
                        return Err(InputError::PostFailed);
                    }
                    CGEventSetFlags(ev, flags_for(mods));
                    CGEventPost(HID_EVENT_TAP, ev);
                    release(ev);
                }
                Ok(())
            }
        }
    }

    fn post_mouse(
        &self,
        kind: u32,
        pos: CGPoint,
        button: u32,
        clicks: u32,
    ) -> Result<(), InputError> {
        unsafe {
            let ev = CGEventCreateMouseEvent(std::ptr::null(), kind, pos, button);
            if ev.is_null() {
                return Err(InputError::PostFailed);
            }
            if clicks > 0 {
                CGEventSetIntegerValueField(ev, FIELD_CLICK_STATE, clicks as i64);
            }
            CGEventPost(HID_EVENT_TAP, ev);
            release(ev);
        }
        Ok(())
    }

    /// Where the cursor is now, since a click carries no position of its own.
    fn cursor_or(&self, nx: f64, ny: f64) -> CGPoint {
        self.last_click
            .map(|(_, p, _)| p)
            .unwrap_or_else(|| self.to_global(nx, ny))
    }

    /// Counts clicks in a sequence, so double and triple clicks work.
    ///
    /// macOS decides what a double click is from the `ClickState` field, not
    /// from timing on its side. Leaving it at one means nothing that needs a
    /// double click ever fires, which looks like the click being ignored.
    fn click_count(&mut self, pos: CGPoint) -> u32 {
        const INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
        const SLOP: f64 = 5.0;

        let now = std::time::Instant::now();
        let count = match self.last_click {
            Some((t, p, n))
                if now.duration_since(t) < INTERVAL
                    && (p.x - pos.x).abs() < SLOP
                    && (p.y - pos.y).abs() < SLOP =>
            {
                // Three is as far as anything meaningful goes.
                (n % 3) + 1
            }
            _ => 1,
        };
        self.last_click = Some((now, pos, count));
        count
    }

    /// The release must carry the same click count as the press, or the pair
    /// is not recognised as one click.
    fn last_clicks(&self) -> u32 {
        self.last_click.map(|(_, _, n)| n).unwrap_or(1)
    }
}

/// Maps normalised coordinates onto a display's global position.
///
/// Split out from the `CGDisplayBounds` call so the arithmetic can be tested
/// without a real display attached. The origin is the interesting part: a
/// second display sits at an offset in the global space, and it is negative
/// when the display is arranged above or to the left of the main one, which is
/// the case people forget.
fn map_normalised(
    nx: f64,
    ny: f64,
    origin_x: f64,
    origin_y: f64,
    width: f64,
    height: f64,
) -> (f64, f64) {
    (
        origin_x + nx.clamp(0.0, 1.0) * width,
        origin_y + ny.clamp(0.0, 1.0) * height,
    )
}

/// `CFRelease`, which is what owns a `CGEventRef`.
unsafe fn release(ev: *mut c_void) {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    unsafe { CFRelease(ev) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_flags_combine() {
        let none = flags_for(KeyMods::default());
        assert_eq!(none, 0);

        let cmd_shift = flags_for(KeyMods {
            shift: true,
            meta: true,
            ..Default::default()
        });
        assert_eq!(cmd_shift, FLAG_SHIFT | FLAG_COMMAND);
    }

    #[test]
    fn maps_onto_a_display_at_the_origin() {
        // Centre of a 1920x1080 display at (0,0).
        assert_eq!(
            map_normalised(0.5, 0.5, 0.0, 0.0, 1920.0, 1080.0),
            (960.0, 540.0)
        );
        assert_eq!(
            map_normalised(0.0, 0.0, 0.0, 0.0, 1920.0, 1080.0),
            (0.0, 0.0)
        );
        assert_eq!(
            map_normalised(1.0, 1.0, 0.0, 0.0, 1920.0, 1080.0),
            (1920.0, 1080.0)
        );
    }

    #[test]
    fn maps_onto_a_display_offset_to_the_right() {
        // The virtual display sits to the right of a 1440-wide main screen, so
        // its coordinates start at 1440 and not at zero.
        let (x, y) = map_normalised(0.5, 0.5, 1440.0, 0.0, 1920.0, 1080.0);
        assert_eq!((x, y), (2400.0, 540.0));
    }

    #[test]
    fn handles_a_display_arranged_left_of_the_main_one() {
        // macOS gives that display a negative origin. Assuming origins are
        // positive puts the cursor on the wrong screen entirely.
        let (x, _) = map_normalised(0.0, 0.0, -1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(x, -1920.0);
        let (x, _) = map_normalised(1.0, 0.0, -1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(x, 0.0);
    }

    #[test]
    fn out_of_range_input_is_clamped_not_wrapped() {
        // A client with a stale video size can send values outside 0..1. They
        // must land on the edge, never on another display.
        let (x, y) = map_normalised(1.7, -0.4, 0.0, 0.0, 1920.0, 1080.0);
        assert_eq!((x, y), (1920.0, 0.0));
    }

    #[test]
    fn all_four_modifiers_are_distinct() {
        // A collision here would make one modifier silently act as another.
        let all = [FLAG_SHIFT, FLAG_CONTROL, FLAG_ALTERNATE, FLAG_COMMAND];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_eq!(a & b, 0, "modifier flags overlap");
            }
        }
    }
}
