//! The play window hands its keys and mouse to the Forge session. The
//! monitor HUD and the Forge screens are the game's, so the window draws
//! no Forge page (docs/game/forge-firefight-ui.md).

use std::sync::Arc;

use app::forge_overlay::{Controller, Overlay};
use input::forge::Key;

pub(super) fn overlay() -> Overlay {
    Overlay(Arc::new(Window))
}

struct Window;

impl Controller for Window {
    fn key(&self, key: Key, down: bool, repeat: bool) -> bool {
        boot::forge_controls::key(key, down, repeat)
    }

    fn release_keys(&self) {
        boot::forge_controls::release_keys();
    }

    fn mouse_look(&self, dx: f32, dy: f32) -> bool {
        boot::forge_controls::mouse_look(dx, dy)
    }

    fn mouse_button(&self, left: bool, down: bool) -> bool {
        boot::forge_controls::mouse_button(left, down)
    }
}
