//! Offline samples exercise the guest list class, detail actions and the
//! same playback page models as the desktop shell.

use std::sync::{Arc, Mutex};

use super::adapter::{Clip, FilmMeta, Library, Player, Source};
use super::overlay::Track;
use super::playback_page::{self, Command, Divergence, Jump, Keyframes, Menu};
use super::prepare::{Preparation, Status as PrepareStatus};
use super::{Control, DETAIL, Entry, FILMS, Load, OVERLAY, PAUSE_MENU, Tab, Theater, pages};
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{Focus, ListEffect, ListPage, Live, Page, described};
use crate::ui_kit::picker;
use crate::ui_kit::spec::ScreenSpec;
use pages::BROWSE_ROWS;

struct SampleLibrary(Mutex<Vec<FilmMeta>>);

impl Library for SampleLibrary {
    fn list(&self, _: &crate::theater::Installation) -> Result<Vec<FilmMeta>, String> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn save(&self, id: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|f| f.id == id)
            .for_each(|f| f.saved = true);
        Ok(())
    }

    fn unsave(&self, id: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|f| f.id == id)
            .for_each(|f| f.saved = false);
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.lock().unwrap().retain(|f| f.id != id);
        Ok(())
    }
}

fn fixture(id: &str, map: &str, variant: &str, playable: bool) -> FilmMeta {
    FilmMeta {
        id: id.into(),
        map: map.into(),
        game_type: super::adapter::game_type(variant),
        mode: Some("Custom Game".into()),
        variant_name: variant.into(),
        started_ms: 1_795_089_600_000,
        length_ticks: 15_200,
        scoreboard: [
            ("Chief", "Red", 21, 17, 8),
            ("Marathon", "Blue", 18, 15, 10),
            ("Echo", "Red", 13, 12, 9),
            ("Haven", "Blue", 11, 9, 12),
        ]
        .into_iter()
        .map(|(name, team, score, kills, deaths)| Player {
            name: name.into(),
            team: Some(team.into()),
            score: Some(score),
            kills: Some(kills),
            deaths: Some(deaths),
            ..Player::default()
        })
        .collect(),
        source: Source::Host,
        image_stamp: if playable {
            "retail 5849 · clang"
        } else {
            "retail 5849 · fast"
        }
        .replace('·', "/"),
        compatibility: if playable {
            crate::theater::CompatibilityState::Playable
        } else {
            crate::theater::CompatibilityState::Unavailable
        },
        compatibility_reason: (!playable).then(|| "Film predates a breaking simulation change.".into()),
        saved: id == "game-1",
        result: Some(if playable { "Won" } else { "Lost" }.into()),
        host: Some("Live Europe".into()),
        ..FilmMeta::default()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Films,
    Detail,
    Playback,
    Pause,
}

impl Kind {
    fn states(self) -> &'static [&'static str] {
        match self {
            Self::Films => &[
                "list",
                "saved",
                "clips",
                "empty",
                "empty-saved",
                "empty-clips",
                "no-matches",
                "loading",
                "full-library",
                "unplayable",
            ],
            Self::Detail => &["list", "unplayable", "summary", "players", "confirm-delete", "preparing"],
            Self::Playback => &[
                "playing", "paused", "opening", "jumping", "divergence", "unavailable",
                "hidden", "preparing", "pause-menu",
            ],
            Self::Pause => &[
                "menu", "no-stats", "no-marks", "player-picker", "camera-picker",
                "mark-picker", "export-picker", "share-picker", "end-dialog",
            ],
        }
    }
}

struct Preview {
    initial: Kind,
    show: Kind,
    theater: Theater,
    focus: Focus,
    parent_focus: Focus,
    /// The kit's modal, above the library whose focus stays on the option.
    picker: Option<(ListPage, Focus)>,
    track: Option<Track>,
    menu: Option<Menu>,
    ready: bool,
}

impl Preview {
    fn new(kind: Kind) -> Self {
        let mut view = Self {
            initial: kind,
            show: kind,
            theater: Theater::default(),
            focus: Focus::default(),
            parent_focus: Focus::default(),
            picker: None,
            track: None,
            menu: None,
            ready: true,
        };
        view.go(kind.states()[0]);
        view
    }

    fn play_input(&mut self, input: Input) -> String {
        let Some(track) = &mut self.track else { return String::new() };
        if let Some(menu) = &mut self.menu {
            let command = menu.press(track, input);
            match command {
                Some(Command::Resume) => {
                    track.paused = false;
                    self.menu = None;
                }
                Some(Command::Restart) => {
                    track.tick = track.bounds().0;
                    self.menu = None;
                }
                Some(Command::Player(player)) => track.selected_player = player,
                Some(Command::Camera(mode)) => track.camera = mode,
                Some(Command::MarkIn) => track.marks = (Some(track.tick), None),
                Some(Command::MarkOut) => track.marks.1 = Some(track.tick),
                Some(Command::ClearMarks) => track.marks = (None, None),
                Some(Command::End) => self.show = Kind::Detail,
                _ => {}
            }
            return format!("{command:?}");
        }
        let control = match input {
            Input::A => Control::Pause,
            Input::B => Control::CancelJump,
            Input::Up => Control::Screenshot,
            Input::Down => Control::Step,
            Input::Left => Control::Skip(-5),
            Input::Right => Control::Skip(5),
            Input::X => Control::Mark,
            Input::Y => Control::Camera,
            Input::Back => Control::Hide,
            Input::Start => {
                track.paused = true;
                self.menu = Some(Menu::default());
                return "Opened pause menu".into();
            }
        };
        track.simulate(control);
        format!("{control:?}")
    }

}

impl Live for Preview {
    fn press(&mut self, input: Input) -> String {
        if let Some((page, focus)) = &mut self.picker {
            let effect = page.step(focus, input);
            if matches!(effect, ListEffect::Close | ListEffect::Action) {
                self.picker = None;
            }
            return format!("{effect:?}");
        }
        let list = matches!(self.show, Kind::Films);
        if matches!(self.show, Kind::Playback | Kind::Pause) {
            return self.play_input(input);
        }
        let effect = pages::preview_page(&self.theater, list).step(&mut self.focus, input);
        match effect {
            ListEffect::Open(picker::NAME) => {
                let picker = Arc::new(picker::Picker::new(false));
                let page = ListPage::new(picker.class(), described(&picker));
                let current = picker::current().map_or(0, |pick| pick.current);
                self.picker = Some((page, Focus(current)));
                return format!("{effect:?}");
            }
            ListEffect::Open(DETAIL) => {
                self.parent_focus = self.focus;
                self.show = Kind::Detail;
                self.focus = Focus(0);
            }
            ListEffect::Close if !list => {
                self.show = Kind::Films;
                self.focus = self.parent_focus;
            }
            ListEffect::Action if !list => {
                let entry = self.theater.state().selected.clone();
                if let Some(entry) = entry
                    .filter(|e| pages::Action::at(e, self.focus.0) == Some(pages::Action::Watch))
                {
                    self.track = Some(Track::from_entry(&entry));
                    self.menu = None;
                    self.show = Kind::Playback;
                } else if self.theater.step_detail(self.focus.0 as i16) {
                    self.show = Kind::Films;
                    self.focus = self.parent_focus;
                }
            }
            _ => {}
        }
        self.focus.0 = self.focus.0.min(self.spec().rows.len().saturating_sub(1));
        format!("{effect:?}")
    }

    fn spec(&self) -> ScreenSpec {
        if let Some((page, focus)) = &self.picker {
            let mut dialog = page.spec(focus);
            let mut library = pages::spec(&self.theater, true);
            library.focus = self.focus.0;
            dialog.underlay = Some(Box::new(library));
            return dialog;
        }
        match self.show {
            Kind::Films | Kind::Detail => ScreenSpec {
                focus: self.focus.0,
                ..pages::spec(&self.theater, matches!(self.show, Kind::Films))
            },
            Kind::Playback | Kind::Pause => {
                let track = self.track.as_ref().expect("playback sample has a track");
                if let Some(menu) = &self.menu {
                    menu.spec(track, None)
                } else {
                    playback_page::bar(track, self.ready, None)
                }
            }
        }
    }

    fn summary(&self) -> String {
        self.spec().log_line()
    }

    fn state_names(&self) -> Vec<String> {
        self.initial
            .states()
            .iter()
            .map(|&s| s.to_owned())
            .collect()
    }

    fn specs(&self) -> Vec<(String, ScreenSpec)> {
        self.initial
            .states()
            .iter()
            .map(|&s| {
                let mut sample = Self::new(self.initial);
                sample.go(s);
                (s.into(), sample.spec())
            })
            .collect()
    }

    fn go(&mut self, name: &str) -> bool {
        if !self.initial.states().contains(&name) {
            return false;
        }
        self.show = self.initial;
        self.focus = Focus(0);
        self.parent_focus = Focus(0);
        self.picker = None;
        self.track = None;
        self.menu = None;
        self.ready = name != "opening";
        let first = fixture("game-1", "lockout", "Team Slayer", true);
        let second = fixture("game-2", "zanzibar", "CTF", false);
        let mut films = vec![first.clone(), second.clone()];
        if name == "full-library" {
            films.extend((3..=25).map(|n| {
                let mut film = first.clone();
                film.id = format!("game-{n}");
                film
            }));
        }
        let last_film = BROWSE_ROWS + films.len() - 1;
        let mut state = super::State {
            load: match name {
                "empty" | "empty-saved" | "empty-clips" => Load::Ready(Vec::new()),
                "loading" => Load::Loading,
                _ => Load::Ready(films.clone()),
            },
            tab: match name {
                "saved" | "empty-saved" => Tab::Saved,
                "clips" | "empty-clips" => Tab::Clips,
                _ => Tab::Recent,
            },
            ..Default::default()
        };
        state.clips.push(Clip {
            film: first.id.clone(),
            from_tick: 600,
            to_tick: 1800,
        });
        if name == "no-matches" {
            state.selector.map = Some("Lockout".into());
            state.selector.game_type = Some("CTF".into());
        }
        if matches!(self.initial, Kind::Films) {
            self.focus.0 = match name {
                "empty" | "empty-saved" | "empty-clips" | "loading" | "no-matches" => 0,
                "unplayable" => BROWSE_ROWS + 1,
                "full-library" => last_film,
                _ => BROWSE_ROWS,
            };
        }
        let mut entry = Entry {
            film: if matches!(name, "unplayable" | "unavailable") { second } else { first },
            clip: None,
            archive_clip: None,
        };
        match name {
            "summary" => {
                entry.film.id = "live:summary".into();
                entry.film.source = Source::Archive;
                entry.film.compatibility = crate::theater::CompatibilityState::Unavailable;
                entry.film.saved = false;
                entry.film.scoreboard.clear();
            }
            "players" => {
                entry.film.scoreboard = (1..=10)
                    .map(|i| Player {
                        name: format!("Player {i}"),
                        ..Player::default()
                    })
                    .collect();
            }
            _ => {}
        }
        state.select(entry.clone());
        if name == "confirm-delete" {
            state.confirm_delete = true;
            state.status = "Press A again to delete".into();
            self.focus = Focus(2);
        }
        if name == "preparing" {
            self.show = Kind::Detail;
            self.focus = Focus(3);
            state.preparation = Some(Preparation {
                id: entry.film.id.clone(),
                tick: 4800,
                length_ticks: entry.film.length_ticks,
                status: PrepareStatus::Running,
            });
        } else if matches!(self.initial, Kind::Playback | Kind::Pause) {
            let mut track = Track::from_entry(&entry);
            track.tick = 4_692;
            track.paused = matches!(name, "paused" | "pause-menu") || matches!(self.initial, Kind::Pause);
            track.speed_reached = Some(0.98);
            track.kills = vec![800, 1024, 4400, 4800, 7900, 12_500];
            track.keyframes = Keyframes { sidecar: vec![501, 6000], ring: vec![3600, 4100, 4500] };
            track.marks = (Some(4380), Some(4620));
            match name {
                "jumping" => {
                    track.seeking = true;
                    track.jump = Some(Jump { target: 9_000, from_tick: 3_000, remaining_ticks: 4_308 });
                }
                "divergence" => track.divergence = Some(Divergence { tick: 4692, kind: "simulation digest".into() }),
                "hidden" => track.visible = false,
                "no-stats" => {
                    track.scoreboard.clear();
                    track.kills.clear();
                }
                "no-marks" => track.marks = (None, None),
                _ => {}
            }
            if name == "pause-menu" || matches!(self.initial, Kind::Pause) {
                let mut menu = Menu::default();
                let row = match name {
                    "player-picker" => Some(2),
                    "camera-picker" => Some(3),
                    "mark-picker" => Some(5),
                    "export-picker" => Some(6),
                    "share-picker" => Some(8),
                    "end-dialog" => Some(9),
                    _ => None,
                };
                if let Some(row) = row {
                    for _ in 0..row {
                        menu.press(&track, Input::Down);
                    }
                    menu.press(&track, Input::A);
                }
                self.menu = Some(menu);
            }
            self.track = Some(track);
        }
        *self.theater.0.library.lock().unwrap() = Some(Box::new(SampleLibrary(Mutex::new(films))));
        *self.theater.state() = state;
        true
    }
}

pub fn live(name: &str) -> Option<Box<dyn Live>> {
    let kind = match name {
        FILMS => Kind::Films,
        DETAIL => Kind::Detail,
        OVERLAY => Kind::Playback,
        PAUSE_MENU => Kind::Pause,
        _ => return None,
    };
    Some(Box::new(Preview::new(kind)))
}

#[cfg(test)]
mod tests;
