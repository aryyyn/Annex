//! Phase-2 input events, travelling client to host over a DataChannel.
//!
//! Coordinates are normalised to 0.0 to 1.0 across the virtual display rather
//! than sent in pixels. The client does not know the display's HiDPI scale or
//! its origin in the global arrangement, and both can change mid-session, so
//! the host is the only place that can map them correctly.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    /// A physical key, named the way `KeyboardEvent.code` names it: `KeyA`,
    /// `Space`, `ArrowUp`.
    ///
    /// Deliberately the physical key and not the character produced. The
    /// character already has the *client's* keyboard layout applied, and
    /// undoing that to reapply the host's cannot be done correctly in general.
    /// Sending the position instead lets the Mac's own layout decide, which is
    /// what someone typing on a Dvorak Mac expects.
    Key {
        code: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// These are the exact payloads `web/client/client.js` sends.
    ///
    /// Worth pinning: the client and this enum are edited in different files
    /// and different languages, so nothing but a test connects them. An
    /// earlier revision declared `code` as a `u32` while the client sent
    /// `"KeyA"`, which meant every keystroke silently failed to parse.
    #[test]
    fn browser_payloads_deserialize() {
        let move_ev: InputEvent =
            serde_json::from_str(r#"{"kind":"mouseMove","x":0.5,"y":0.25}"#).unwrap();
        assert_eq!(move_ev, InputEvent::MouseMove { x: 0.5, y: 0.25 });

        let click: InputEvent =
            serde_json::from_str(r#"{"kind":"mouseButton","btn":"right","down":true}"#).unwrap();
        assert_eq!(
            click,
            InputEvent::MouseButton {
                btn: MouseButton::Right,
                down: true
            }
        );

        let scroll: InputEvent =
            serde_json::from_str(r#"{"kind":"scroll","dx":0.0,"dy":-40.0}"#).unwrap();
        assert_eq!(scroll, InputEvent::Scroll { dx: 0.0, dy: -40.0 });

        let key: InputEvent = serde_json::from_str(
            r#"{"kind":"key","code":"KeyA","down":true,"mods":{"shift":false,"ctrl":false,"alt":false,"meta":true}}"#,
        )
        .unwrap();
        assert_eq!(
            key,
            InputEvent::Key {
                code: "KeyA".into(),
                down: true,
                mods: KeyMods {
                    meta: true,
                    ..Default::default()
                }
            }
        );
    }

    #[test]
    fn every_mouse_button_name_matches_the_client() {
        for (json, want) in [
            ("left", MouseButton::Left),
            ("middle", MouseButton::Middle),
            ("right", MouseButton::Right),
        ] {
            let ev: InputEvent = serde_json::from_str(&format!(
                r#"{{"kind":"mouseButton","btn":"{json}","down":false}}"#
            ))
            .unwrap();
            assert_eq!(
                ev,
                InputEvent::MouseButton {
                    btn: want,
                    down: false
                }
            );
        }
    }
}
