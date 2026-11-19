use std::path::PathBuf;
use std::sync::Arc;

use app::theater_overlay as shell;
use boot::theater_ui::{Key, Theater};

pub(super) fn overlay(theater: Theater, screenshots: PathBuf) -> shell::Overlay {
    shell::Overlay(Arc::new(Controller {
        theater,
        screenshots,
    }))
}

struct Controller {
    theater: Theater,
    screenshots: PathBuf,
}

impl shell::Controller for Controller {
    fn update(&self) -> shell::View {
        self.theater.pump();
        let mut view = shell::View {
            web_url: self.theater.web_url(),
            ..shell::View::default()
        };
        if let Some(overlay) = self.theater.overlay() {
            view.players = overlay.players;
            view.camera = overlay.top_right;
            view.transport = overlay.bottom;
        }
        view
    }

    fn key(&self, key: shell::Key, down: bool, repeat: bool) -> bool {
        let key = match key {
            shell::Key::Space => Key::Space,
            shell::Key::Left => Key::Left,
            shell::Key::Right => Key::Right,
            shell::Key::Up => Key::Up,
            shell::Key::Down => Key::Down,
            shell::Key::Slower => Key::Slower,
            shell::Key::Faster => Key::Faster,
            shell::Key::Camera => Key::Camera,
            shell::Key::Hide => Key::Hide,
            shell::Key::Hud => Key::Hud,
            shell::Key::Clip => Key::Clip,
            shell::Key::Screenshot => Key::Screenshot,
            shell::Key::Leave => Key::Leave,
        };
        self.theater.key(key, down, repeat)
    }

    fn release_keys(&self) {
        self.theater.release_keys();
    }

    fn take_screenshot(&self) -> Option<PathBuf> {
        self.theater
            .screenshot_requested()
            .then(|| self.screenshots.clone())
    }

    fn screenshot_saved(&self, result: Result<PathBuf, String>) {
        self.theater.screenshot_answer(result);
    }

    fn take_leave(&self) -> bool {
        self.theater.leave_requested()
    }
}
