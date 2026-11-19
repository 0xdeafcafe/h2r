//! Offline samples exercise the guest list class, detail actions and the
//! same transport model as the desktop plates.

use std::sync::Mutex;

use debug_menu::Tone;

use super::adapter::{Clip, FilmMeta, Library, Player, Source};
use super::overlay::Track;
use super::{Control, DETAIL, Entry, FILMS, Load, OVERLAY, Tab, Theater, pages};
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{Focus, ListEffect, Live, Page};
use crate::ui_kit::spec::{PaneText, ScreenSpec};

struct SampleLibrary(Mutex<Vec<FilmMeta>>);

impl Library for SampleLibrary {
    fn list(&self, _: &str) -> Result<Vec<FilmMeta>, String> {
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
        variant: variant.into(),
        started_ms: 1_795_089_600_000,
        length_ticks: 15_200,
        players: [
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
            Self::Films => &["list", "empty", "loading", "unplayable", "saved", "clips"],
            Self::Detail => &["list", "unplayable", "web", "confirm-delete"],
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
            ListEffect::Open(DETAIL) => {
                self.parent_focus = self.focus;
                self.show = Kind::Detail;
                self.focus = Focus(0);
            }
            ListEffect::Close if !list => {
                self.show = Kind::Films;
                self.focus = self.parent_focus;
            }
            ListEffect::Action if !list && self.focus.0 == 0 => {
                if let Some(entry) = self.theater.state().selected.as_ref() {
                    self.track = Some(Track::from_entry(entry));
                    self.show = Kind::Playback;
                }
            }
            ListEffect::Action if !list => {
                if self.focus.0 == 2
                    && self
                        .theater
                        .state()
                        .selected
                        .as_ref()
                        .is_some_and(|e| e.clip.is_some())
                {
                    return "Clip uses the local library".into();
                }
                if self.theater.step_detail(self.focus.0 as i16) {
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
        self.track = None;
        let first = fixture("game-1", "lockout", "Team Slayer", true);
        let second = fixture("game-2", "zanzibar", "CTF", false);
        let films = vec![first.clone(), second.clone()];
        let mut state = super::State::default();
        state.load = match name {
            "empty" => Load::Ready(Vec::new()),
            "loading" => Load::Loading,
            _ => Load::Ready(films.clone()),
        };
        state.tab = match name {
            "saved" => Tab::Saved,
            "clips" => Tab::Clips,
            _ => Tab::Recent,
        };
        state.clips.push(Clip {
            film: first.id.clone(),
            from_tick: 600,
            to_tick: 1800,
        });
        if matches!(self.initial, Kind::Films) {
            self.focus.0 = match name {
                "empty" | "loading" => 0,
                "unplayable" => 2,
                _ => 1,
            };
        }
        let entry = Entry {
            film: if name == "unplayable" { second } else { first },
            clip: None,
        };
        state.select(entry.clone());
        if name == "web" {
            state.web = true;
            self.focus = Focus(3);
        }
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
mod tests {
    use super::*;

    #[test]
    fn theater_tabs_only_show_matching_entries() {
        let mut view = live(FILMS).unwrap();
        assert!(view.go("saved"));
        assert_eq!(view.spec().rows.len(), 2);
        assert!(
            view.spec().rows[1].label.contains("Lockout")
                || view.spec().rows[1].label.contains("lockout")
        );
        assert!(view.go("clips"));
        assert!(view.spec().rows[1].shown_value().unwrap().contains("0:40"));
        assert!(view.go("unplayable"));
        assert!(view.spec().rows[2].label.starts_with("* "));
    }

    #[test]
    fn theater_tab_walkthrough_restores_the_parent_focus() {
        let mut view = live(FILMS).unwrap();
        view.press(Input::Up);
        view.press(Input::Right);
        assert_eq!(view.spec().rows.len(), 2);
        assert!(view.spec().texts[0].text.contains("[ Saved ]"));
        view.press(Input::Down);
        view.press(Input::A);
        assert_eq!(view.spec().name, DETAIL);
        view.press(Input::B);
        assert_eq!(view.spec().name, FILMS);
        assert_eq!(view.spec().focus, 1);
    }

    #[test]
    fn theater_unplayable_film_can_be_saved_but_cannot_watch() {
        let mut view = live(DETAIL).unwrap();
        assert!(view.go("unplayable"));
        view.press(Input::A);
        assert_eq!(view.spec().name, DETAIL);
        view.press(Input::Down);
        view.press(Input::A);
        assert_eq!(view.spec().rows[1].label, "Unsave");
    }

    #[test]
    fn theater_delete_is_confirmed_and_web_link_is_optional() {
        let mut view = live(DETAIL).unwrap();
        view.press(Input::Down);
        view.press(Input::Down);
        view.press(Input::A);
        assert_eq!(view.spec().rows[2].label, "Confirm delete");
        view.press(Input::A);
        assert_eq!(view.spec().rows.len(), 2);
        assert_eq!(view.spec().name, FILMS);
        assert!(view.go("web"));
        assert!(
            view.spec()
                .texts
                .iter()
                .any(|t| t.text.ends_with("/theater/game-1"))
        );
    }

    #[test]
    fn theater_overlay_hides_and_restores_while_paused() {
        let mut view = live(OVERLAY).unwrap();
        assert!(view.go("playing"));
        view.press(Input::A);
        assert!(
            view.spec()
                .texts
                .iter()
                .any(|t| t.text.starts_with("Paused"))
        );
        view.press(Input::Back);
        assert!(view.spec().texts.is_empty());
        view.press(Input::Back);
        assert!(
            view.spec()
                .texts
                .iter()
                .any(|t| t.text.starts_with("Paused"))
        );
        assert!(
            view.spec()
                .texts
                .iter()
                .any(|t| t.text.contains("after keyframes"))
        );
    }
}
