use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use app::theater_overlay as shell;
use agent_proto::theater::CompatibilityState;
use boot::theater::Installation;
use boot::theater_ui::{
    Key, Theater,
    adapter::{FilmMeta, Library, Player, Source, game_type, team_name},
    stats,
};
use boot::ui_kit::{preview, spec::ScreenSpec};

pub(super) fn open_library(dir: &Path) -> Result<Box<dyn Library>, String> {
    let library = film::Library::open(dir).map_err(|e| e.to_string())?;
    Ok(Box::new(FilmLibrary(library)))
}

struct FilmLibrary(film::Library);

impl Library for FilmLibrary {
    fn list(&self, installation: &Installation) -> Result<Vec<FilmMeta>, String> {
        let entries = self.0.list().map_err(|e| e.to_string())?;
        Ok(entries
            .into_iter()
            .map(|entry| {
                let compatibility = installation.compatibility(&entry);
                let recording = entry.path.clone();
                let mut film = film_entry(entry);
                if let Err(error) = stats::read_summary(&recording, &mut film) {
                    eprintln!("theater: reading stats for {}: {error:#}", film.id);
                }
                film.apply_compatibility(compatibility);
                film
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
pub(crate) fn film_entry(entry: film::LibraryEntry) -> FilmMeta {
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
        roster: meta
            .players
            .into_iter()
            .map(|p| Player {
                name: p.name,
                team: (p.team >= 0).then(|| team_name(p.team)),
                ..Player::default()
            })
            .collect(),
        source: if host { Source::Host } else { Source::Local },
        version: Some(entry.version),
        image_stamp: meta.image_stamp,
        compatibility: CompatibilityState::Playable,
        saved: entry.saved,
        live_game_id: entry.game_id.filter(|_| host),
        ticks_per_second: entry.ticks_per_second,
        ..FilmMeta::default()
    };
    if let Some(summary) = entry.summary {
        row.apply_summary(summary);
        for player in std::mem::take(&mut row.scoreboard) {
            if let Some(seat) = row.roster.iter_mut().find(|p| p.name == player.name && p.seat.is_none()) {
                seat.seat = player.seat;
                seat.team = player.team;
            } else {
                row.roster.push(Player {
                    name: player.name,
                    seat: player.seat,
                    team: player.team,
                    ..Player::default()
                });
            }
        }
    }
    row
}

pub(crate) fn overlay(theater: Theater, screenshots: PathBuf, disc: Option<&Path>) -> (shell::Overlay, Arc<Controller>) {
    let disc = disc.and_then(|path| match preview::Disc::open(path) {
        Ok(disc) => Some(disc),
        Err(error) => {
            eprintln!("theater: UI fonts and art failed to load from the disc, no overlay is drawn: {error:#}");
            None
        }
    });
    if disc.is_none() {
        eprintln!("theater: no disc given for the overlay's fonts, no overlay is drawn");
    }
    let controller = Arc::new(Controller {
        theater,
        screenshots,
        input: Mutex::new(None),
        pages: Mutex::new(Pages { disc, drawn: None, keys: None }),
    });
    (shell::Overlay(controller.clone()), controller)
}

pub(crate) struct Controller {
    theater: Theater,
    screenshots: PathBuf,
    input: Mutex<Option<input::InputManager>>,
    pages: Mutex<Pages>,
}

struct Pages {
    disc: Option<preview::Disc>,
    drawn: Option<(ScreenSpec, bool, Arc<shell::Page>)>,
    /// The keyboard's keycaps, added to the disc's fonts the first time
    /// the keyboard drives a film.
    keys: Option<boot::icons::Icons>,
}

impl Pages {
    /// `spec` drawn for the shell; with `keyboard`, its button glyphs are
    /// the Theater's keys (`theater_ui::playback_icons`).
    fn render(&mut self, spec: ScreenSpec, keyboard: bool) -> Option<Arc<shell::Page>> {
        if spec.name == boot::theater_ui::OVERLAY && spec.texts.is_empty() {
            self.drawn = None;
            return None;
        }
        if let Some((old, was, page)) = &self.drawn
            && *old == spec
            && *was == keyboard
        {
            return Some(page.clone());
        }
        if keyboard && self.keys.is_none() {
            let icons = boot::theater_ui::playback_icons();
            let rewrite = icons.font_rewrite();
            if let Some(disc) = &mut self.disc {
                disc.fonts.patch(|bytes| rewrite.patch(bytes));
            }
            self.keys = Some(icons);
        }
        // Without the disc's fonts the layout source draws every word as a
        // block, so there is no page rather than a page of boxes.
        let from = preview::From::Disc(self.disc.as_ref()?);
        let mut view = from.view(&spec, "playback").inspect_err(|error| {
            eprintln!("theater: laying out {}: {error:#}", spec.name);
        }).ok()?;
        if let Some(icons) = self.keys.as_ref().filter(|_| keyboard) {
            view.map_text(|text| {
                let units: Vec<u16> = text.encode_utf16().map(|u| icons.shown(u)).collect();
                String::from_utf16_lossy(&units)
            });
        }
        let canvas = preview::render_overlay(&view, from.source());
        let page = Arc::new(shell::Page {
            rgba: canvas.rgba,
            size: (canvas.width, canvas.height),
        });
        self.drawn = Some((spec, keyboard, page.clone()));
        Some(page)
    }
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
        let playing = self.theater.playing();
        // The host's page: nothing while the game draws the film's HUD and
        // its pause menu itself (docs/game/theater.md, "Who draws what").
        let page = (self.theater.film_loaded() || self.theater.film_loading()).then(|| self.theater.host_page()).flatten().and_then(|spec| {
            let keyboard = self.theater.keyboard_in_use();
            self.pages.lock().unwrap_or_else(|e| e.into_inner()).render(spec, keyboard)
        });
        let timeline = self.theater.playback_timeline().map(|(rect, canvas)| shell::Timeline {
            rect,
            canvas: canvas.or(page.as_ref().map(|p| p.size)).unwrap_or(boot::theater_ui::HUD_CANVAS),
        });
        shell::View { playing, page, timeline }
    }

    fn key(&self, key: shell::Key, down: bool, repeat: bool) -> bool {
        let key = match key {
            shell::Key::Space => Key::Space,
            shell::Key::Accept => Key::Accept,
            shell::Key::Step => Key::Step,
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
    fn no_playback_page_is_drawn_while_no_film_is_loaded() {
        use shell::Controller as _;
        let (_, controller) = overlay(Theater::default(), PathBuf::new(), None);
        let view = controller.update();
        assert!(view.page.is_none(), "the menus are not drawn over");
        assert!(view.timeline.is_none(), "no film, no rail to click");
        assert!(!view.playing);
    }

    #[test]
    fn without_the_disc_fonts_no_page_of_word_blocks_is_drawn() {
        let mut spec = ScreenSpec::new("theater_test", "Test");
        spec.texts.push(boot::ui_kit::spec::PaneText {
            id: "t".into(),
            rect: [100, -100, 80, 100],
            text: "Hello".into(),
            font: 1,
            right: false,
            colour: [1.0; 3],
        });
        let mut pages = Pages { disc: None, drawn: None, keys: None };
        assert!(pages.render(spec, false).is_none());
    }

    fn local_entry(map: &str) -> film::LibraryEntry {
        film::LibraryEntry {
            meta: film::FilmMeta {
                id: "local".into(),
                map: map.into(),
                variant: String::new(),
                started_ms: 1,
                length_ticks: 100,
                players: Vec::new(),
                source: film::Source::Local,
                image_stamp: String::new(),
            },
            summary: None,
            version: 3,
            ticks_per_second: None,
            runtime_stamp: String::new(),
            simulation_hooks: None,
            xbe_sha256: String::new(),
            guest_image: None,
            saved: false,
            lease_id: None,
            game_id: None,
            path: "local.h2film".into(),
        }
    }

    #[test]
    fn the_library_lists_games_not_nameless_films() {
        assert!(!local_entry("").is_game(), "no map, no players");
        assert!(local_entry("zanzibar").is_game());
        let mut scored = local_entry("");
        scored.meta.players.push(film::Player { name: "Chief".into(), team: 0, score: 1 });
        assert!(scored.is_game());
        let mut summarised = local_entry("");
        summarised.summary = Some(film::blam::Summary { level: "a30".into(), ..Default::default() });
        assert!(summarised.is_game());
    }

    #[test]
    fn exact_host_films_retain_the_live_id() {
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
            simulation_hooks: None,
            xbe_sha256: String::new(),
            guest_image: None,
            saved: false,
            lease_id: Some("lease".into()),
            game_id: Some("hl-game".into()),
            path: "host.h2film".into(),
        };
        let row = film_entry(entry);
        assert_eq!(row.image_stamp, "image stamp");
        assert_eq!(row.id, "local-file-id");
        assert_eq!(row.live_game_id.as_deref(), Some("hl-game"));
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
            runtime_stamp: "h2-sim/2".into(),
            simulation_hooks: Some("fixture".into()),
            xbe_sha256: String::new(),
            guest_image: None,
            lease_id: None,
            game_id: None,
            saved: true,
            path: "local.h2film".into(),
        };
        let row = film_entry(entry);
        assert!(row.saved);
        assert_eq!(row.map, "a30");
        assert_eq!(row.mode.as_deref(), Some("Co-op"));
        assert_eq!(row.game_type.as_deref(), Some("Campaign"));
        assert_eq!(row.variant_name, "Co-operative");
        assert_eq!(
            (row.length_ticks, row.duration_ticks, row.ticks_per_second),
            (9000, Some(3600), Some(60))
        );
        // The film's own summary names the seats; scores and medals come
        // from the stats file, which `stats::read_summary` applies later.
        assert_eq!(row.roster.len(), 1);
        assert_eq!(row.roster[0].name, "Chief");
        assert_eq!(row.roster[0].seat, Some(3));
        assert_eq!(row.roster[0].team.as_deref(), Some("Blue"));
        assert!(row.scoreboard.is_empty());
    }
}
