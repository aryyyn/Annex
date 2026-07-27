//! Translating browser key identifiers into macOS virtual key codes.
//!
//! # Why `KeyboardEvent.code` and not `key`
//!
//! The browser offers two identifiers and they mean different things. `key` is
//! the character produced, so it already has the layout and modifiers applied:
//! pressing the same physical key gives `a`, `A`, or `å` depending on state.
//! `code` is the physical key, always `KeyA` wherever that key sits and
//! whatever it prints.
//!
//! macOS virtual key codes are also physical, so `code` maps to them directly
//! and the Mac's own keyboard layout then decides what the keypress produces.
//! That is the behaviour you want: a Mac set to Dvorak should interpret the
//! keypress as Dvorak, not receive a character the client already decided.
//!
//! Using `key` would mean undoing the client's layout and reapplying the host's,
//! which cannot be done correctly in general.
//!
//! The constants are the ANSI positions from `HIToolbox/Events.h`. They are
//! stable, and unusually for this project they are properly documented by
//! Apple.

/// Maps a `KeyboardEvent.code` value to a macOS virtual key code.
///
/// Returns `None` for keys with no equivalent, which are then dropped rather
/// than guessed at. A wrong keycode types the wrong character, which is worse
/// than typing nothing.
pub fn code_to_virtual(code: &str) -> Option<u16> {
    Some(match code {
        // Letters, in the order Apple defines them, which is not alphabetical
        // because it follows the original physical layout.
        "KeyA" => 0x00,
        "KeyS" => 0x01,
        "KeyD" => 0x02,
        "KeyF" => 0x03,
        "KeyH" => 0x04,
        "KeyG" => 0x05,
        "KeyZ" => 0x06,
        "KeyX" => 0x07,
        "KeyC" => 0x08,
        "KeyV" => 0x09,
        "KeyB" => 0x0B,
        "KeyQ" => 0x0C,
        "KeyW" => 0x0D,
        "KeyE" => 0x0E,
        "KeyR" => 0x0F,
        "KeyY" => 0x10,
        "KeyT" => 0x11,
        "KeyO" => 0x1F,
        "KeyU" => 0x20,
        "KeyI" => 0x22,
        "KeyP" => 0x23,
        "KeyL" => 0x25,
        "KeyJ" => 0x26,
        "KeyK" => 0x28,
        "KeyN" => 0x2D,
        "KeyM" => 0x2E,

        // Digit row.
        "Digit1" => 0x12,
        "Digit2" => 0x13,
        "Digit3" => 0x14,
        "Digit4" => 0x15,
        "Digit6" => 0x16,
        "Digit5" => 0x17,
        "Digit9" => 0x19,
        "Digit7" => 0x1A,
        "Digit8" => 0x1C,
        "Digit0" => 0x1D,

        // Punctuation.
        "Equal" => 0x18,
        "Minus" => 0x1B,
        "BracketRight" => 0x1E,
        "BracketLeft" => 0x21,
        "Quote" => 0x27,
        "Semicolon" => 0x29,
        "Backslash" => 0x2A,
        "Comma" => 0x2B,
        "Slash" => 0x2C,
        "Period" => 0x2F,
        "Backquote" => 0x32,

        // Editing and whitespace.
        "Enter" | "NumpadEnter" => 0x24,
        "Tab" => 0x30,
        "Space" => 0x31,
        "Backspace" => 0x33,
        "Escape" => 0x35,
        "Delete" => 0x75,

        // Navigation.
        "Home" => 0x73,
        "End" => 0x77,
        "PageUp" => 0x74,
        "PageDown" => 0x79,
        "ArrowLeft" => 0x7B,
        "ArrowRight" => 0x7C,
        "ArrowDown" => 0x7D,
        "ArrowUp" => 0x7E,

        // Function row.
        "F1" => 0x7A,
        "F2" => 0x78,
        "F3" => 0x63,
        "F4" => 0x76,
        "F5" => 0x60,
        "F6" => 0x61,
        "F7" => 0x62,
        "F8" => 0x64,
        "F9" => 0x65,
        "F10" => 0x6D,
        "F11" => 0x67,
        "F12" => 0x6F,

        // Numeric keypad.
        "Numpad0" => 0x52,
        "Numpad1" => 0x53,
        "Numpad2" => 0x54,
        "Numpad3" => 0x55,
        "Numpad4" => 0x56,
        "Numpad5" => 0x57,
        "Numpad6" => 0x58,
        "Numpad7" => 0x59,
        "Numpad8" => 0x5B,
        "Numpad9" => 0x5C,
        "NumpadDecimal" => 0x41,
        "NumpadMultiply" => 0x43,
        "NumpadAdd" => 0x45,
        "NumpadDivide" => 0x4B,
        "NumpadSubtract" => 0x4E,
        "NumpadEqual" => 0x51,

        // Modifiers are posted as flags on other events rather than as
        // keystrokes, so they are deliberately absent here. See `flags.rs`.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_keys_people_actually_press() {
        assert_eq!(code_to_virtual("KeyA"), Some(0x00));
        assert_eq!(code_to_virtual("Space"), Some(0x31));
        assert_eq!(code_to_virtual("Enter"), Some(0x24));
        assert_eq!(code_to_virtual("ArrowUp"), Some(0x7E));
        assert_eq!(code_to_virtual("Escape"), Some(0x35));
    }

    #[test]
    fn both_enter_keys_agree() {
        // A laptop has one Enter, a full keyboard has two, and they should not
        // behave differently.
        assert_eq!(code_to_virtual("Enter"), code_to_virtual("NumpadEnter"));
    }

    #[test]
    fn unknown_keys_are_dropped_not_guessed() {
        // Typing the wrong character is worse than typing nothing.
        assert_eq!(code_to_virtual("MediaPlayPause"), None);
        assert_eq!(code_to_virtual(""), None);
        assert_eq!(code_to_virtual("Nonsense"), None);
    }

    #[test]
    fn modifiers_are_not_keystrokes() {
        // They travel as flags on other events, so mapping them here would
        // double-apply them.
        assert_eq!(code_to_virtual("ShiftLeft"), None);
        assert_eq!(code_to_virtual("MetaLeft"), None);
    }
}
