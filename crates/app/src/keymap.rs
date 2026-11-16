//! Window input to `input::backend::keyboard::KeyboardMouse` names.
//!
//! Keys go by physical position (`winit::keyboard::KeyCode`, the W3C
//! `KeyboardEvent.code` set), so WASD stays WASD on AZERTY and Dvorak
//! layouts. Every name produced here is one `input::bindings` accepts; a
//! test checks that against `canonical_key`.

use input::bindings::MouseButton;
use winit::keyboard::KeyCode;

/// The logical text path is separate from physical pad bindings. Preedit
/// belongs to the OS; never submit it or let its editing keys drive the pad.
pub(crate) fn text_input(
    keyboard: &input::backend::keyboard::KeyboardMouse,
    name: Option<&str>,
    logical: &winit::keyboard::Key,
    text: Option<&str>,
    down: bool,
    composing: bool,
) -> bool {
    use input::backend::keyboard::TextAction;
    use winit::keyboard::{Key, NamedKey};
    let control = match logical {
        Key::Named(NamedKey::Backspace) => Some(TextAction::Backspace),
        Key::Named(NamedKey::Enter) => Some(TextAction::Accept),
        Key::Named(NamedKey::Escape) => Some(TextAction::Cancel),
        _ => None,
    };
    let candidate = composing
        || control.is_some()
        || matches!(
            logical,
            Key::Character(_)
                | Key::Dead(_)
                | Key::Named(
                    NamedKey::Shift
                        | NamedKey::Control
                        | NamedKey::Alt
                        | NamedKey::AltGraph
                        | NamedKey::CapsLock
                )
        );
    let action = if composing {
        None
    } else {
        control.or_else(|| text.map(|s| TextAction::Insert(s.into())))
    };
    keyboard.text_key(name, down, candidate, action)
}

/// The W3C `code` name for a key the input layer knows, or `None` for keys
/// it has no binding name for (and for Cmd/Super, which the shell keeps for
/// host shortcuts).
pub fn key_name(code: KeyCode) -> Option<&'static str> {
    use KeyCode::*;
    Some(match code {
        KeyA => "KeyA",
        KeyB => "KeyB",
        KeyC => "KeyC",
        KeyD => "KeyD",
        KeyE => "KeyE",
        KeyF => "KeyF",
        KeyG => "KeyG",
        KeyH => "KeyH",
        KeyI => "KeyI",
        KeyJ => "KeyJ",
        KeyK => "KeyK",
        KeyL => "KeyL",
        KeyM => "KeyM",
        KeyN => "KeyN",
        KeyO => "KeyO",
        KeyP => "KeyP",
        KeyQ => "KeyQ",
        KeyR => "KeyR",
        KeyS => "KeyS",
        KeyT => "KeyT",
        KeyU => "KeyU",
        KeyV => "KeyV",
        KeyW => "KeyW",
        KeyX => "KeyX",
        KeyY => "KeyY",
        KeyZ => "KeyZ",
        Digit0 => "Digit0",
        Digit1 => "Digit1",
        Digit2 => "Digit2",
        Digit3 => "Digit3",
        Digit4 => "Digit4",
        Digit5 => "Digit5",
        Digit6 => "Digit6",
        Digit7 => "Digit7",
        Digit8 => "Digit8",
        Digit9 => "Digit9",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        Space => "Space",
        Enter | NumpadEnter => "Enter",
        Escape => "Escape",
        Tab => "Tab",
        Backspace => "Backspace",
        CapsLock => "CapsLock",
        Delete => "Delete",
        Insert => "Insert",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        ArrowUp => "ArrowUp",
        ArrowDown => "ArrowDown",
        ArrowLeft => "ArrowLeft",
        ArrowRight => "ArrowRight",
        ShiftLeft => "ShiftLeft",
        ShiftRight => "ShiftRight",
        ControlLeft => "ControlLeft",
        ControlRight => "ControlRight",
        AltLeft => "AltLeft",
        AltRight => "AltRight",
        Comma => "Comma",
        Period => "Period",
        Slash => "Slash",
        Semicolon => "Semicolon",
        Quote => "Quote",
        BracketLeft => "BracketLeft",
        BracketRight => "BracketRight",
        Backslash => "Backslash",
        Minus => "Minus",
        Equal => "Equal",
        Backquote => "Backquote",
        _ => return None,
    })
}

/// The input layer's mouse button for a window mouse button.
pub fn mouse_button(b: winit::event::MouseButton) -> Option<MouseButton> {
    use winit::event::MouseButton as W;
    Some(match b {
        W::Left => MouseButton::Left,
        W::Right => MouseButton::Right,
        W::Middle => MouseButton::Middle,
        W::Back => MouseButton::Back,
        W::Forward => MouseButton::Forward,
        W::Other(_) => return None,
    })
}

/// Host shortcuts, recognised while Cmd (Super) is held. Keys pressed with
/// Cmd never reach the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// Cmd+Q: quit.
    Quit,
    /// Cmd+F or Cmd+Enter: toggle borderless fullscreen.
    Fullscreen,
    /// Cmd+P: pause or resume the guest.
    Pause,
}

pub fn shortcut(code: KeyCode) -> Option<Shortcut> {
    match code {
        KeyCode::KeyQ => Some(Shortcut::Quit),
        KeyCode::KeyF | KeyCode::Enter => Some(Shortcut::Fullscreen),
        KeyCode::KeyP => Some(Shortcut::Pause),
        _ => None,
    }
}

/// What the shell knows when a mouse button changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseContext {
    /// `AppConfig::mouse_capture`.
    pub capture_enabled: bool,
    /// The cursor is captured now.
    pub captured: bool,
    /// Cmd (Super) is held: the mouse belongs to the host.
    pub super_held: bool,
    /// Pressed (true) or released.
    pub down: bool,
    /// The left button.
    pub left: bool,
}

/// What to do with a mouse button event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    /// Capture the cursor; the click itself is not forwarded.
    Capture,
    /// Forward to the keyboard+mouse pad.
    Forward,
    /// Drop it.
    Ignore,
}

/// Mouse routing. While Cmd is held nothing is captured or forwarded (Cmd
/// releases capture, and a Cmd-click must not grab it back). Without
/// capture enabled, buttons always go to the pad.
pub fn mouse_action(c: MouseContext) -> MouseAction {
    if c.super_held {
        MouseAction::Ignore
    } else if !c.capture_enabled || c.captured {
        MouseAction::Forward
    } else if c.down && c.left {
        MouseAction::Capture
    } else {
        MouseAction::Ignore
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input::backend::keyboard::KeyboardMouse;
    use input::bindings::{Bindings, canonical_key};
    use input::host::HostButton;

    /// Every `KeyCode` winit can deliver that we name.
    fn all_named() -> Vec<(KeyCode, &'static str)> {
        use KeyCode::*;
        let codes = [
            KeyA,
            KeyB,
            KeyC,
            KeyD,
            KeyE,
            KeyF,
            KeyG,
            KeyH,
            KeyI,
            KeyJ,
            KeyK,
            KeyL,
            KeyM,
            KeyN,
            KeyO,
            KeyP,
            KeyQ,
            KeyR,
            KeyS,
            KeyT,
            KeyU,
            KeyV,
            KeyW,
            KeyX,
            KeyY,
            KeyZ,
            Digit0,
            Digit1,
            Digit2,
            Digit3,
            Digit4,
            Digit5,
            Digit6,
            Digit7,
            Digit8,
            Digit9,
            F1,
            F2,
            F3,
            F4,
            F5,
            F6,
            F7,
            F8,
            F9,
            F10,
            F11,
            F12,
            Space,
            Enter,
            NumpadEnter,
            Escape,
            Tab,
            Backspace,
            CapsLock,
            Delete,
            Insert,
            Home,
            End,
            PageUp,
            PageDown,
            ArrowUp,
            ArrowDown,
            ArrowLeft,
            ArrowRight,
            ShiftLeft,
            ShiftRight,
            ControlLeft,
            ControlRight,
            AltLeft,
            AltRight,
            Comma,
            Period,
            Slash,
            Semicolon,
            Quote,
            BracketLeft,
            BracketRight,
            Backslash,
            Minus,
            Equal,
            Backquote,
        ];
        codes
            .into_iter()
            .map(|c| (c, key_name(c).unwrap_or_else(|| panic!("{c:?} unnamed"))))
            .collect()
    }

    #[test]
    fn every_name_is_accepted_by_the_input_layer() {
        for (code, name) in all_named() {
            assert!(
                canonical_key(name).is_some(),
                "{code:?} -> {name} rejected by input::bindings"
            );
        }
    }

    #[test]
    fn names_match_the_w3c_code_spelling() {
        // winit's KeyCode Debug spelling is the W3C `code` value.
        for (code, name) in all_named() {
            if code != KeyCode::NumpadEnter {
                assert_eq!(format!("{code:?}"), name);
            }
        }
    }

    #[test]
    fn host_keys_are_not_forwarded() {
        assert_eq!(key_name(KeyCode::SuperLeft), None);
        assert_eq!(key_name(KeyCode::SuperRight), None);
        assert_eq!(key_name(KeyCode::Fn), None);
    }

    #[test]
    fn default_bindings_through_the_map() {
        let kb = KeyboardMouse::new(Bindings::default());
        assert!(kb.key_down(key_name(KeyCode::Space).unwrap()));
        assert!(kb.key_down(key_name(KeyCode::KeyW).unwrap()));
        assert!(kb.key_down(key_name(KeyCode::Escape).unwrap()));
        assert!(kb.key_down(key_name(KeyCode::ControlLeft).unwrap()));
        let s = kb.state();
        assert!(s.buttons.contains(HostButton::South), "Space is A");
        assert!(s.buttons.contains(HostButton::Start), "Escape is Start");
        assert!(
            s.buttons.contains(HostButton::LeftStick),
            "Ctrl is crouch (left thumb)"
        );
        assert_eq!(s.left_stick, (0.0, 1.0), "W is forward");
        kb.key_up("KeyW");
        assert_eq!(kb.state().left_stick, (0.0, 0.0));
        kb.mouse_button(mouse_button(winit::event::MouseButton::Left).unwrap(), true);
        assert_eq!(kb.state().right_trigger, 1.0, "left mouse fires");
        assert_eq!(mouse_button(winit::event::MouseButton::Other(9)), None);
    }

    #[test]
    fn shortcuts() {
        assert_eq!(shortcut(KeyCode::KeyQ), Some(Shortcut::Quit));
        assert_eq!(shortcut(KeyCode::Enter), Some(Shortcut::Fullscreen));
        assert_eq!(shortcut(KeyCode::KeyW), None);
    }
    #[test]
    fn mouse_routing() {
        let c = MouseContext {
            capture_enabled: true,
            captured: false,
            super_held: false,
            down: true,
            left: true,
        };
        assert_eq!(mouse_action(c), MouseAction::Capture);
        assert_eq!(
            mouse_action(MouseContext {
                super_held: true,
                ..c
            }),
            MouseAction::Ignore,
            "Cmd-click must not recapture"
        );
        assert_eq!(
            mouse_action(MouseContext {
                super_held: true,
                captured: true,
                ..c
            }),
            MouseAction::Ignore,
            "nothing is forwarded while Cmd is held"
        );
        assert_eq!(
            mouse_action(MouseContext {
                super_held: true,
                capture_enabled: false,
                ..c
            }),
            MouseAction::Ignore
        );
        assert_eq!(
            mouse_action(MouseContext { left: false, ..c }),
            MouseAction::Ignore,
            "only left captures"
        );
        assert_eq!(
            mouse_action(MouseContext { down: false, ..c }),
            MouseAction::Ignore
        );
        assert_eq!(
            mouse_action(MouseContext {
                captured: true,
                ..c
            }),
            MouseAction::Forward
        );
        assert_eq!(
            mouse_action(MouseContext {
                captured: true,
                down: false,
                ..c
            }),
            MouseAction::Forward
        );
        assert_eq!(
            mouse_action(MouseContext {
                capture_enabled: false,
                left: false,
                ..c
            }),
            MouseAction::Forward
        );
    }
}

#[cfg(test)]
mod text_tests {
    use super::*;
    use input::{
        backend::keyboard::{KeyboardMouse, TextAction},
        bindings::Bindings,
    };
    use winit::keyboard::{Key, NamedKey};

    #[test]
    fn logical_text_uses_layout_and_controls_are_captured_only_in_a_field() {
        let kb = KeyboardMouse::new(Bindings::default());
        let character = Key::Character("É".into());
        assert!(!text_input(
            &kb,
            Some("KeyW"),
            &character,
            Some("É"),
            true,
            false
        ));
        kb.set_text_target(Some(9));
        assert!(text_input(
            &kb,
            Some("KeyW"),
            &character,
            Some("É"),
            true,
            false
        ));
        assert!(text_input(
            &kb,
            Some("ShiftLeft"),
            &Key::Named(NamedKey::Shift),
            None,
            true,
            false
        ));
        for (key, name) in [
            (NamedKey::Backspace, "Backspace"),
            (NamedKey::Enter, "Enter"),
            (NamedKey::Escape, "Escape"),
        ] {
            assert!(text_input(
                &kb,
                Some(name),
                &Key::Named(key),
                None,
                true,
                false
            ));
        }
        assert_eq!(
            kb.take_text(9),
            vec![
                TextAction::Insert("É".into()),
                TextAction::Backspace,
                TextAction::Accept,
                TextAction::Cancel
            ]
        );
        assert!(!text_input(
            &kb,
            Some("ArrowLeft"),
            &Key::Named(NamedKey::ArrowLeft),
            None,
            true,
            false
        ));
        kb.set_text_target(None);
        assert!(
            text_input(
                &kb,
                Some("Enter"),
                &Key::Named(NamedKey::Enter),
                None,
                true,
                false
            ),
            "held repeat"
        );
        text_input(
            &kb,
            Some("Enter"),
            &Key::Named(NamedKey::Enter),
            None,
            false,
            false,
        );
        assert!(!text_input(
            &kb,
            Some("Enter"),
            &Key::Named(NamedKey::Enter),
            None,
            true,
            false
        ));
    }
    #[test]
    fn ime_preedit_and_dead_keys_do_not_type_or_drive_the_pad() {
        let kb = KeyboardMouse::new(Bindings::default());
        kb.set_text_target(Some(8));
        assert!(text_input(
            &kb,
            Some("Quote"),
            &Key::Dead(Some('´')),
            None,
            true,
            false
        ));
        assert!(text_input(
            &kb,
            Some("Enter"),
            &Key::Named(NamedKey::Enter),
            Some("x"),
            true,
            true
        ));
        assert!(kb.take_text(8).is_empty());
        assert!(kb.commit_text("中é"));
        assert_eq!(kb.take_text(8), vec![TextAction::Insert("中é".into())]);
    }
}
