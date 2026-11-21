//! Offline samples exercise the guest list class, detail actions and the
//! same transport model as the desktop plates.

use std::sync::{Arc, Mutex};

use debug_menu::Tone;

use super::adapter::{Clip, FilmMeta, Library, Player, Source};
use super::overlay::Track;
use super::{Control, DETAIL, Entry, FILMS, Load, OVERLAY, Tab, Theater, pages};
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{Focus, ListEffect, ListPage, Live, Page, described};
use crate::ui_kit::picker;
use crate::ui_kit::spec::{PaneText, ScreenSpec};
use pages::BROWSE_ROWS;

struct SampleLibrary(Mutex<Vec<FilmMeta>>);

impl Library for SampleLibrary {
    fn list(&self, _: &[String]) -> Result<Vec<FilmMeta>, String> {
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
        playable_here: playable,
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
            Self::Detail => &["list", "unplayable", "summary", "players", "confirm-delete"],
            Self::Playback => &["playing", "paused"],
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
        };
        view.go(kind.states()[0]);
        view
    }

    fn overlay_spec(&self) -> ScreenSpec {
        let mut spec = ScreenSpec::new(OVERLAY, "");
        let Some(track) = &self.track else {
            return spec;
        };
        if !track.visible {
            return spec;
        }
        let view = track.view(None);
        // The disc's body font is 19 pixels tall. Leave a gutter between
        // rows and grow the transport upward when it has a clip or notice.
        const ROW_STEP: i16 = 22;
        let transport_top = -208 + view.bottom.len() as i16 * ROW_STEP;
        for (lines, top, left, right) in [
            (&view.players, 148, -260, 8),
            (&view.top_right, 190, 24, 264),
            (&view.bottom, transport_top, -220, 220),
        ] {
            for (i, line) in lines.iter().enumerate() {
                let top = top - i as i16 * ROW_STEP;
                let colour = match line.tone {
                    Tone::Title => [1.0, 0.84, 0.47],
                    Tone::Selected => [1.0, 1.0, 1.0],
                    Tone::Greyed => [0.65, 0.68, 0.73],
                    _ => [0.83, 0.88, 0.94],
                };
                spec.texts.push(PaneText {
                    id: format!("transport.{}", spec.texts.len()),
                    rect: [top, left, top - ROW_STEP, right],
                    font: 1,
                    right: false,
                    colour,
                    text: line.text.clone(),
                });
            }
        }
        spec
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
        if matches!(self.show, Kind::Playback) {
            let control = match input {
                Input::A => Control::Pause,
                Input::B => {
                    self.show = Kind::Detail;
                    return "Left playback".into();
                }
                Input::Up => Control::Player(false),
                Input::Down => Control::Player(true),
                Input::Left => Control::Skip(-5),
                Input::Right => Control::Skip(5),
                Input::X => Control::Mark,
                Input::Back => Control::Hide,
                Input::Start | Input::Y => return "No action".into(),
            };
            if let Some(track) = &mut self.track {
                track.simulate(control);
            }
            return format!("{control:?}");
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
            Kind::Playback => self.overlay_spec(),
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
                _ => Load::Ready(films),
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
            film: if name == "unplayable" { second } else { first },
            clip: None,
            archive_clip: None,
        };
        match name {
            "summary" => {
                entry.film.id = "live:summary".into();
                entry.film.source = Source::Archive;
                entry.film.playable_here = false;
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
        if matches!(self.initial, Kind::Playback) {
            let mut track = Track::from_entry(&entry);
            track.tick = 4_692;
            track.paused = name == "paused";
            track.kills = vec![800, 1024, 4400, 4800, 7900, 12_500];
            track.bookmarks = vec![501, 6000];
            track.marks.0 = Some(4380);
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
        _ => return None,
    };
    Some(Box::new(Preview::new(kind)))
}

#[cfg(test)]
mod tests;
