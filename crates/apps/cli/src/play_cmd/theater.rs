use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use app::theater_overlay as shell;
use boot::theater::playable_entry;
use boot::theater_ui::{
    Key, Theater,
    adapter::{FilmMeta, Library, Player, Source, game_type},
};

pub(super) fn open_library(dir: &Path) -> Result<Box<dyn Library>, String> {
    let library = film::Library::open(dir).map_err(|e| e.to_string())?;
    Ok(Box::new(FilmLibrary(library)))
}

struct FilmLibrary(film::Library);

impl Library for FilmLibrary {
    fn list(&self, stamps: &[String]) -> Result<Vec<FilmMeta>, String> {
        let entries = self.0.list().map_err(|e| e.to_string())?;
        Ok(entries
            .into_iter()
            .map(|entry| {
                let selected = stamps.iter().find(|stamp| playable_entry(&entry, stamp));
                film_entry(entry, selected.map_or("", String::as_str))
            })
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

/// Both the library selector and the playback transport use the sealed header
/// for tape length and identity and the match summary for match-only results.
pub(crate) fn film_entry(entry: film::LibraryEntry, stamp: &str) -> FilmMeta {
    let playable_here = playable_entry(&entry, stamp);
    let meta = entry.meta;
    let host = meta.source == film::Source::Host;
    let mut row = FilmMeta {
        id: meta.id,
        map: meta.map,
        game_type: game_type(&meta.variant),
        mode: (host && entry.lease_id.is_none()).then(|| "System Link".into()),
        variant_name: meta.variant,
        started_ms: meta.started_ms,
        length_ticks: meta.length_ticks,
        scoreboard: meta
            .players
            .into_iter()
            .map(|p| Player {
                name: p.name,
                team: (p.team >= 0).then(|| p.team.to_string()),
                score: Some(p.score),
                ..Player::default()
            })
            .collect(),
        source: if host { Source::Host } else { Source::Local },
        version: Some(entry.version),
        image_stamp: meta.image_stamp,
        playable_here,
        saved: entry.saved,
        live_game_id: entry.game_id.filter(|_| host),
        ticks_per_second: entry.ticks_per_second,
        ..FilmMeta::default()
    };
    if let Some(summary) = entry.summary {
        row.apply_summary(summary);
    }
    row
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
        if let Some(input) = self
            .input
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            input.sync();
        }
        self.theater.pump();
        let mut view = shell::View {
            playing: self.theater.playing(),
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
            shell::Key::Step => Key::Step,
            shell::Key::Slower => Key::Slower,
            shell::Key::Faster => Key::Faster,
            shell::Key::Hide => Key::Hide,
            shell::Key::Hud => Key::Hud,
            shell::Key::Clip => Key::Clip,
            shell::Key::Screenshot => Key::Screenshot,
            shell::Key::Camera => Key::Camera,
            shell::Key::Forward => Key::Forward,
            shell::Key::Backward => Key::Backward,
            shell::Key::StrafeLeft => Key::StrafeLeft,
            shell::Key::StrafeRight => Key::StrafeRight,
            shell::Key::Rise => Key::Rise,
            shell::Key::Fall => Key::Fall,
            shell::Key::Leave => Key::Leave,
        };
        self.theater.key(key, down, repeat)
    }

    fn release_keys(&self) {
        self.theater.release_keys();
    }

    fn mouse_look(&self, dx: f64, dy: f64) -> bool {
        self.theater.mouse_look(dx, dy)
    }

    fn mouse_zoom(&self, amount: f32) -> bool {
        self.theater.mouse_zoom(amount)
    }

    fn scrub(&self, fraction: f32) -> bool {
        self.theater.scrub(fraction)
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
    fn exact_host_films_use_the_image_stamp_and_retain_the_live_id() {
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
        let entry = film::LibraryEntry {
            meta,
            summary: None,
            version: 1,
            ticks_per_second: None,
            runtime_stamp: String::new(),
            xbe_sha256: String::new(),
            guest_image: None,
            saved: false,
            lease_id: Some("lease".into()),
            game_id: Some("hl-game".into()),
            path: "host.h2film".into(),
        };
        let playable = film_entry(entry.clone(), "image stamp");
        assert!(playable.playable_here);
        assert_eq!(playable.id, "local-file-id");
        assert_eq!(playable.live_game_id.as_deref(), Some("hl-game"));
        assert!(!film_entry(entry, "other image").playable_here);
    }

    #[test]
    fn engine_row_uses_match_results_and_preserves_the_tape_clock() {
        let entry = film::LibraryEntry {
            meta: film::FilmMeta {
                id: "file".into(),
                map: "menu".into(),
                variant: "old variant".into(),
                started_ms: 100,
                length_ticks: 9000,
                players: vec![],
                source: film::Source::Local,
                image_stamp: "local image".into(),
            },
            summary: Some(film::blam::Summary {
                level: "a30".into(),
                mode: film::blam::Mode::CoopCampaign,
                game_type: "campaign".into(),
                variant_name: "Co-operative".into(),
                duration_ticks: 3600,
                players: vec![film::blam::PlayerSummary {
                    seat: 3,
                    name: "Chief".into(),
                    team: 1,
                    score: i64::MAX,
                    medals: [("double_kill".into(), 2)].into_iter().collect(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ticks_per_second: Some(60),
            version: 3,
            runtime_stamp: "h2-sim/1".into(),
            xbe_sha256: String::new(),
            guest_image: None,
            lease_id: None,
            game_id: None,
            saved: true,
            path: "local.h2film".into(),
        };
        let row = film_entry(entry, "local image");
        assert!(row.playable_here && row.saved);
        assert_eq!(row.map, "a30");
        assert_eq!(row.mode.as_deref(), Some("Co-op"));
        assert_eq!(row.game_type.as_deref(), Some("Campaign"));
        assert_eq!(row.variant_name, "Co-operative");
        assert_eq!(
            (row.length_ticks, row.duration_ticks, row.ticks_per_second),
            (9000, Some(3600), Some(60))
        );
        assert_eq!(row.scoreboard[0].seat, Some(3));
        assert_eq!(row.scoreboard[0].score, Some(i64::MAX));
        assert_eq!(row.scoreboard[0].medals["double_kill"], 2);
    }
}
