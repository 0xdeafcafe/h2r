//! The desktop input the Forge session consumes. The window draws no Forge
//! page: the monitor HUD and the Forge screens are the game's
//! (docs/game/forge-firefight-ui.md).

use std::sync::Arc;

use input::forge::Key;

pub trait Controller: Send + Sync {
    fn key(&self, key: Key, down: bool, repeat: bool) -> bool;
    fn release_keys(&self);
    fn mouse_look(&self, dx: f32, dy: f32) -> bool;
    fn mouse_button(&self, left: bool, down: bool) -> bool;
}

#[derive(Clone)]
pub struct Overlay(pub Arc<dyn Controller>);

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForgeOverlay").finish_non_exhaustive()
    }
}

/// The keys the Forge bindings table names
/// (`boot::forge_controls::bindings::BINDINGS`), by the key that sends each.
/// A key the session does not take (Escape with nothing to cancel) reaches
/// the game.
#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub(crate) fn key(code: winit::keyboard::KeyCode, command: bool, shift: bool) -> Option<Key> {
    use winit::keyboard::KeyCode as K;
    if command {
        return (code == K::KeyZ).then_some(if shift { Key::Redo } else { Key::Undo });
    }
    Some(match code {
        K::Tab => Key::Toggle,
        K::KeyW => Key::Forward,
        K::KeyS => Key::Backward,
        K::KeyA => Key::StrafeLeft,
        K::KeyD => Key::StrafeRight,
        K::KeyE => Key::Rise,
        K::KeyQ => Key::Fall,
        K::ShiftLeft => Key::Thrust,
        K::ControlLeft => Key::Fine,
        K::KeyF => Key::Objects,
        K::KeyT => Key::Tools,
        K::KeyG => Key::Settings,
        K::KeyH => Key::Overlays,
        K::KeyM => Key::Menu,
        K::Delete => Key::Delete,
        K::Escape => Key::Back,
        K::Enter | K::NumpadEnter | K::Space => Key::Accept,
        _ => return None,
    })
}

#[cfg(all(test, not(any(target_os = "tvos", target_os = "ios"))))]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    #[test]
    fn monitor_keys_leave_window_shortcuts_available() {
        assert_eq!(key(KeyCode::Tab, false, false), Some(Key::Toggle));
        assert_eq!(key(KeyCode::KeyQ, false, false), Some(Key::Fall));
        assert_eq!(key(KeyCode::KeyQ, true, false), None);
        assert_eq!(key(KeyCode::KeyZ, true, false), Some(Key::Undo));
        assert_eq!(key(KeyCode::KeyZ, true, true), Some(Key::Redo));
        assert_eq!(key(KeyCode::KeyF, false, false), Some(Key::Objects));
        assert_eq!(key(KeyCode::KeyT, false, false), Some(Key::Tools));
        assert_eq!(key(KeyCode::KeyH, false, false), Some(Key::Overlays));
        assert_eq!(key(KeyCode::KeyM, false, false), Some(Key::Menu));
        assert_eq!(key(KeyCode::ShiftLeft, false, false), Some(Key::Thrust));
        assert_eq!(key(KeyCode::Escape, false, false), Some(Key::Back));
    }
}
