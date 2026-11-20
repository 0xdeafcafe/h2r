use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use app::theater_overlay as shell;
use boot::theater_ui::{
    Key, Theater,
    adapter::{FilmMeta, Library, Player, Source},
};

pub(super) fn open_library(dir: &Path) -> Result<Box<dyn Library>, String> {
    let library = film::Library::open(dir).map_err(|e| e.to_string())?;
    Ok(Box::new(FilmLibrary(library)))
}

struct FilmLibrary(film::Library);

impl Library for FilmLibrary {
    fn list(&self, stamp: &str) -> Result<Vec<FilmMeta>, String> {
        let entries = self.0.list().map_err(|e| e.to_string())?;
        Ok(entries
            .into_iter()
            .map(|entry| film_meta(entry.meta, entry.saved, stamp, entry.lease_id))
            .collect())
    }

    fn save(&self, id: &str) -> Result<(), String> {
        self.0.save(id).map_err(|e| e.to_string())
    }

    fn unsave(&self, id: &str) -> Result<(), String> {
        self.0.unsave(id).map_err(|e| e.to_string())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.delete(id).map_err(|e| e.to_string())
    }
}

pub(crate) fn film_meta(
    meta: film::FilmMeta,
    saved: bool,
    stamp: &str,
    lease_id: Option<String>,
) -> FilmMeta {
    let host_film = meta.source == film::Source::Host;
    let playable_here = host_film && meta.playable_here(stamp);
    FilmMeta {
        id: meta.id,
        map: meta.map,
        variant: meta.variant,
        started_ms: meta.started_ms,
        length_ticks: meta.length_ticks,
        players: meta
            .players
            .into_iter()
            .map(|player| Player {
                name: player.name,
                team: (player.team >= 0).then(|| player.team.to_string()),
                score: i32::try_from(player.score).ok(),
                ..Player::default()
            })
            .collect(),
        source: match meta.source {
            film::Source::Host => Source::Host,
            film::Source::Local => Source::Local,
        },
        image_stamp: meta.image_stamp,
        playable_here,
        saved,
        result: None,
        host: None,
        web_game_id: lease_id.filter(|_| host_film),
    }
}

pub(crate) fn overlay(theater: Theater, screenshots: PathBuf) -> (shell::Overlay, Arc<Controller>) {
    let controller = Arc::new(Controller {
        theater,
        screenshots,
        input: Mutex::new(None),
    });
    (shell::Overlay(controller.clone()), controller)
}

pub(crate) struct Controller {
    theater: Theater,
    screenshots: PathBuf,
    input: Mutex<Option<input::InputManager>>,
}

impl Controller {
    pub(crate) fn attach_input(&self, mut input: input::InputManager) {
        input.set_gate(self.theater.pad_gate(None));
        *self.input.lock().unwrap_or_else(|e| e.into_inner()) = Some(input);
    }
}

impl shell::Controller for Controller {
    fn update(&self) -> shell::View {
        if let Some(input) = self.input.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            input.sync();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_host_films_use_the_image_stamp_and_the_lease_web_link() {
        let meta = film::FilmMeta {
            id: "local-file-id".into(),
            map: "lockout".into(),
            variant: "Slayer".into(),
            started_ms: 100,
            length_ticks: 1800,
            players: Vec::new(),
            source: film::Source::Host,
            image_stamp: "image stamp".into(),
        };
        let playable = film_meta(meta.clone(), false, "image stamp", Some("hl-game".into()));
        assert!(playable.playable_here);
        assert_eq!(playable.id, "local-file-id");
        assert_eq!(playable.web_game_id.as_deref(), Some("hl-game"));
        let other = film_meta(meta, false, "other image", None);
        assert!(!other.playable_here);
        assert!(other.web_game_id.is_none());
    }
}
