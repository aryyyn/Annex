//! Phase-2 input events, travelling client to host over a DataChannel.
//!
//! Coordinates are normalised to 0.0 to 1.0 across the virtual display rather
//! than sent in pixels. The client does not know the display's HiDPI scale or
//! its origin in the global arrangement, and both can change mid-session, so
//! the host is the only place that can map them correctly.

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InputEvent {
    MouseMove {
        x: f64,
        y: f64,
    },
    MouseButton {
        btn: MouseButton,
        down: bool,
    },
    Scroll {
        dx: f64,
        dy: f64,
    },
    Key {
        code: u32,
        down: bool,
        mods: KeyMods,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Modifier bitflags, matching the browser's `KeyboardEvent` booleans rather
/// than any Apple constant. The host translates to `CGEventFlags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct KeyMods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}
