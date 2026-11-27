//! The Theater's playback HUD (docs/game/theater.md, "The playback HUD"):
//! the film's transport as kit elements, in Halo 2's HUD font and with its
//! battery strip, centred at the top of the frame (anchored to its centre).
//!
//! - `theater.status`: the transport's state, the time shown over the
//!   film's length and the speed when it is not 1x;
//! - `theater.rail.0`, `theater.rail.1`: two strip meters end to end
//!   (`Art::StripMeter`, 92 by 8), the first half of the film and the
//!   second, filled to the tick shown;
//! - `theater.view`: the camera and its subject, or what the film is doing
//!   (a jump's target, the range being marked, a divergence);
//! - `theater.hints`: the button hints, on the game's prompt line;
//! - `theater.notice`: a notice ("Film added to Saved"), one line of the
//!   game's notification list, sent as an event.
//!
//! One layout: a state never adds, removes or moves an element. The kit
//! has no element placed by a value, so a kill or a bookmark is not a mark
//! on the rail; the view line says the range being marked.
//!
//! The data is `content/core/huds/theater.hud.yaml`. [`State`] is what the
//! HUD shows; the game (`game::theater_ui::hud`) makes it from the
//! transport's track.

use crate::model::{Describe, Hud, Pulse, Style, Value, Values};

pub const NAME: &str = "theater";

pub const HINTS: &str = "theater.hints";
pub const NOTICE: &str = "theater.notice";
pub const STATUS: &str = "theater.status";
pub const RAIL: [&str; 2] = ["theater.rail.0", "theater.rail.1"];
pub const VIEW: &str = "theater.view";

/// The elements' ids, in draw order.
pub const IDS: [&str; 6] = [HINTS, NOTICE, STATUS, RAIL[0], RAIL[1], VIEW];

/// The rail in the HUD frame, from its top left corner: left, top, width
/// and height (two strips of 92 and the gap of 4 between them). The
/// YAML's places, which are from the frame's centre ([`FRAME_CENTRE`]);
/// `the_rail_is_where_the_yaml_puts_it` holds them to it.
pub const RAIL_RECT: [i16; 4] = [178, 25, 188, 8];
/// The fullscreen frame's centre from its top left corner: the frame is
/// 544 by 408 (the 640 by 480 screen inset 7.5%).
pub const FRAME_CENTRE: (i16, i16) = (272, 204);
/// The fullscreen frame's top left corner on the 640 by 480 screen.
pub const FRAME_ORIGIN: (i16, i16) = (48, 36);
/// A strip's width and the gap to the next.
pub const STRIP: i16 = 92;
pub const STRIP_GAP: i16 = 4;

/// The most characters the view line shows: its `longest`'s count.
pub const VIEW_CHARS: usize = 30;

/// What the transport is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Playing,
    Paused,
    /// A jump runs forward to its target.
    Jumping,
    /// The film's game has not answered yet.
    Opening,
    /// The replay diverged from the film.
    Stopped,
    /// The film cannot play on this installation.
    Unavailable,
}

impl Status {
    pub const ALL: [Status; 6] = [
        Status::Playing,
        Status::Paused,
        Status::Jumping,
        Status::Opening,
        Status::Stopped,
        Status::Unavailable,
    ];

    /// The word the status line starts with, in capitals as the HUD's
    /// own lines are.
    pub const fn word(self) -> &'static str {
        match self {
            Status::Playing => "PLAYING",
            Status::Paused => "PAUSED",
            Status::Jumping => "JUMPING",
            Status::Opening => "OPENING",
            Status::Stopped => "STOPPED",
            Status::Unavailable => "UNAVAILABLE",
        }
    }
}

/// Which buttons the prompt line names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hints {
    /// Play or pause, the menu, hide.
    #[default]
    Transport,
    /// A jump runs: B cancels it.
    Jump,
    /// The keyboard drives the film: its keys, as words.
    Keys,
    /// None.
    Off,
}

/// What the HUD shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The bar is up; hidden, no element shows.
    pub shown: bool,
    pub status: Status,
    /// The tick shown and the film's length, both from the start of the
    /// range watched, and the film's ticks a second.
    pub tick: u64,
    pub length: u64,
    pub tick_rate: u64,
    /// The speed asked for; 1 is not written.
    pub speed: f64,
    /// The view line: the camera and its subject, or what the film is
    /// doing. Empty hides the line.
    pub view: String,
    /// The view line is a fault (a divergence): it flashes.
    pub alert: bool,
    pub hints: Hints,
}

/// `ticks` as `m:ss`, the film library's clock.
pub fn clock(ticks: u64, rate: u64) -> String {
    let secs = ticks / rate.max(1);
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// A speed as the status line writes it: `2x`, `0.5x`, `0.25x`.
pub fn speed(speed: f64) -> String {
    format!("{speed}x")
}

/// The status line: the state's word, the time over the length, and the
/// speed when it is not 1x. A film that cannot play has no time.
pub fn status_line(s: &State) -> String {
    if s.status == Status::Unavailable {
        return s.status.word().into();
    }
    let mut line = format!(
        "{} {} / {}",
        s.status.word(),
        clock(s.tick.min(s.length), s.tick_rate),
        clock(s.length, s.tick_rate)
    );
    if (s.speed - 1.0).abs() > f64::EPSILON && s.speed > 0.0 {
        line.push(' ');
        line.push_str(&speed(s.speed));
    }
    line
}

/// The two strips' fills for the tick shown: the first fills over the
/// film's first half, the second over its second.
pub fn rail_fills(tick: u64, length: u64) -> [f32; 2] {
    let at = if length == 0 {
        0.0
    } else {
        (tick.min(length) as f64 / length as f64) as f32
    };
    [(at * 2.0).clamp(0.0, 1.0), (at * 2.0 - 1.0).clamp(0.0, 1.0)]
}

/// The fraction of the film at `x` pixels along the rail, for a click on
/// it: the gap between the strips is the film's middle.
pub fn rail_fraction(x: i16) -> f32 {
    let x = x.clamp(0, RAIL_RECT[2] - 1);
    let half = if x < STRIP {
        f32::from(x) / f32::from(STRIP - 1) * 0.5
    } else if x < STRIP + STRIP_GAP {
        0.5
    } else {
        0.5 + f32::from(x - STRIP - STRIP_GAP) / f32::from(STRIP - 1) * 0.5
    };
    half.clamp(0.0, 1.0)
}

/// `text` cut to the view line's room, the cut marked with three dots.
pub fn view_line(text: &str) -> String {
    if text.chars().count() <= VIEW_CHARS {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(VIEW_CHARS - 3).collect();
    cut.truncate(cut.trim_end().len());
    cut.push_str("...");
    cut
}

/// The prompt line's words.
pub fn hints_line(hints: Hints) -> Option<&'static str> {
    match hints {
        Hints::Transport => Some("\u{e100} Play / Pause  \u{e10c} Menu  \u{e10d} Hide"),
        Hints::Jump => Some("\u{e101} Cancel Jump"),
        Hints::Keys => Some("Space Play / Pause  Esc Menu  H Hide"),
        Hints::Off => None,
    }
}

/// Every element hidden: no film plays, or its bar is hidden.
pub fn hidden() -> Values {
    values(&State::default())
}

/// The values for `state`: one for each element. The notice is an event
/// (`Handle::notify`), so it is hidden in every state.
pub fn values(state: &State) -> Values {
    let mut out = Values::default();
    let hide = !state.shown;
    out.set(
        HINTS,
        match hints_line(state.hints).filter(|_| !hide) {
            Some(line) => Value::text(line),
            None => Value::hidden(),
        },
    );
    out.set(NOTICE, Value::hidden());
    out.set(
        STATUS,
        if hide {
            Value::hidden()
        } else {
            Value::text(status_line(state))
        },
    );
    let rail = !hide && !matches!(state.status, Status::Opening | Status::Unavailable);
    for (id, fill) in RAIL.into_iter().zip(rail_fills(state.tick, state.length)) {
        out.set(
            id,
            if rail {
                Value::fill(fill)
            } else {
                Value::hidden()
            },
        );
    }
    out.set(
        VIEW,
        if hide || state.view.is_empty() {
            Value::hidden()
        } else {
            let line = Value::text(view_line(&state.view));
            if state.alert {
                line.styled(Style::Flash(Pulse::TimeWarning))
            } else {
                line
            }
        },
    );
    out
}

/// The elements, in draw order ([`IDS`]).
pub fn hud() -> Hud {
    crate::data::builtin(NAME)
}

/// The named states: the transport as a film goes, a jump, a marked
/// range, a divergence, a film that cannot play, and the widest words.
pub fn states() -> Vec<(&'static str, State)> {
    let playing = State {
        shown: true,
        status: Status::Playing,
        tick: 146 * 30,
        length: 506 * 30,
        tick_rate: 30,
        speed: 1.0,
        view: "Follow / Chief".into(),
        alert: false,
        hints: Hints::Transport,
    };
    vec![
        ("playing", playing.clone()),
        (
            "paused",
            State {
                status: Status::Paused,
                view: "Fly cam".into(),
                ..playing.clone()
            },
        ),
        (
            "fast",
            State {
                speed: 4.0,
                tick: 400 * 30,
                ..playing.clone()
            },
        ),
        (
            "jumping",
            State {
                status: Status::Jumping,
                view: "Target 6:40".into(),
                hints: Hints::Jump,
                ..playing.clone()
            },
        ),
        (
            "marking",
            State {
                view: "Clip 2:26 - --".into(),
                ..playing.clone()
            },
        ),
        (
            "keys",
            State {
                hints: Hints::Keys,
                ..playing.clone()
            },
        ),
        (
            "opening",
            State {
                status: Status::Opening,
                tick: 0,
                view: String::new(),
                hints: Hints::Off,
                ..playing.clone()
            },
        ),
        (
            "divergence",
            State {
                status: Status::Stopped,
                view: "Replay diverged at 2:26".into(),
                alert: true,
                ..playing.clone()
            },
        ),
        (
            "unavailable",
            State {
                status: Status::Unavailable,
                view: "Playback is unavailable".into(),
                hints: Hints::Off,
                ..playing.clone()
            },
        ),
        (
            "widest",
            State {
                status: Status::Jumping,
                tick: 3599 * 30,
                length: 3599 * 30,
                speed: 0.25,
                view: "First person / Marathon Man 15".into(),
                hints: Hints::Transport,
                ..playing.clone()
            },
        ),
        ("hidden", State::default()),
    ]
}

pub struct Theater;

impl Describe for Theater {
    fn hud(&self) -> Hud {
        hud()
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        states().into_iter().map(|(n, s)| (n, values(&s))).collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("the rail is the energy sword's battery strip, which a campaign player HUD may lack; a campaign film keeps the host's bar")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Anchor, Kind};

    fn text(v: &Values, id: &str) -> Option<String> {
        v.get(id).filter(|x| x.shown).map(|x| x.text.clone())
    }

    #[test]
    fn every_state_has_a_value_for_every_element_and_no_other() {
        let hud = hud();
        let ids: Vec<&str> = hud.elements.iter().map(|e| e.id).collect();
        assert_eq!(ids, IDS);
        for (name, state) in states() {
            let v = values(&state);
            let got: Vec<&str> = v.0.iter().map(|(id, _)| *id).collect();
            assert_eq!(got, IDS, "{name}");
        }
    }

    #[test]
    fn the_status_line_says_the_state_the_time_and_a_speed_that_is_not_one() {
        let mut s = states().remove(0).1;
        assert_eq!(status_line(&s), "PLAYING 2:26 / 8:26");
        s.status = Status::Paused;
        s.speed = 0.25;
        assert_eq!(status_line(&s), "PAUSED 2:26 / 8:26 0.25x");
        s.speed = 2.0;
        assert_eq!(status_line(&s), "PAUSED 2:26 / 8:26 2x");
        s.status = Status::Unavailable;
        assert_eq!(status_line(&s), "UNAVAILABLE");
        // A tick past the end reads as the end.
        s.status = Status::Playing;
        s.speed = 1.0;
        s.tick = s.length + 90;
        assert_eq!(status_line(&s), "PLAYING 8:26 / 8:26");
    }

    #[test]
    fn no_state_text_is_longer_than_its_longest() {
        let hud = hud();
        let longest = |id: &str| {
            hud.elements
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| match &e.kind {
                    Kind::Text { longest, .. } | Kind::Prompt { longest } | Kind::Message { longest } => Some(*longest),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{id} has no longest"))
                .chars()
                .count()
        };
        for status in Status::ALL {
            for speed in [0.25, 0.5, 1.0, 2.0, 4.0] {
                let s = State {
                    shown: true,
                    status,
                    tick: 3599 * 30,
                    length: 3599 * 30,
                    tick_rate: 30,
                    speed,
                    ..State::default()
                };
                let line = status_line(&s);
                assert!(line.chars().count() <= longest(STATUS), "{line:?}");
            }
        }
        for hints in [Hints::Transport, Hints::Jump, Hints::Keys] {
            let line = hints_line(hints).unwrap();
            assert!(line.chars().count() <= longest(HINTS), "{line:?}");
        }
        assert_eq!(longest(VIEW), VIEW_CHARS);
        let cut = view_line("Replay diverged at 12:34 (the object pool's digest)");
        assert_eq!(cut.chars().count(), VIEW_CHARS);
        assert!(cut.ends_with("..."));
        assert_eq!(view_line("Fly cam"), "Fly cam");
    }

    #[test]
    fn the_rail_fills_one_strip_and_then_the_other() {
        assert_eq!(rail_fills(0, 100), [0.0, 0.0]);
        assert_eq!(rail_fills(25, 100), [0.5, 0.0]);
        assert_eq!(rail_fills(50, 100), [1.0, 0.0]);
        assert_eq!(rail_fills(75, 100), [1.0, 0.5]);
        assert_eq!(rail_fills(100, 100), [1.0, 1.0]);
        assert_eq!(rail_fills(900, 100), [1.0, 1.0], "past the end");
        assert_eq!(rail_fills(5, 0), [0.0, 0.0], "a film with no length");
    }

    #[test]
    fn a_click_on_the_rail_is_a_fraction_of_the_film() {
        assert_eq!(rail_fraction(0), 0.0);
        assert_eq!(rail_fraction(STRIP - 1), 0.5);
        assert_eq!(rail_fraction(STRIP + 1), 0.5, "the gap is the middle");
        assert_eq!(rail_fraction(STRIP + STRIP_GAP), 0.5);
        assert_eq!(rail_fraction(RAIL_RECT[2] - 1), 1.0);
        assert_eq!(rail_fraction(-4), 0.0);
        assert_eq!(rail_fraction(999), 1.0);
        let quarter = rail_fraction(STRIP / 2);
        assert!((quarter - 0.25).abs() < 0.01, "{quarter}");
    }

    #[test]
    fn the_rail_is_where_the_yaml_puts_it() {
        let hud = hud();
        let place = |id: &str| hud.elements.iter().find(|e| e.id == id).unwrap().at;
        let [left, top, width, _] = RAIL_RECT;
        let (cx, cy) = FRAME_CENTRE;
        assert_eq!(place(RAIL[0]).anchor, Anchor::Centre);
        assert_eq!(place(RAIL[0]).full, (left - cx, top - cy));
        assert_eq!(place(RAIL[1]).full, (left + STRIP + STRIP_GAP - cx, top - cy));
        assert_eq!(width, 2 * STRIP + STRIP_GAP);
        for id in [STATUS, RAIL[0], RAIL[1], VIEW] {
            let at = place(id);
            assert!(at.half.is_none() && at.quarter.is_none(), "{id}: fullscreen only");
        }
    }

    #[test]
    fn a_hidden_bar_shows_nothing_and_an_opening_film_no_rail() {
        let v = hidden();
        assert!(v.0.iter().all(|(_, x)| !x.shown));
        let opening = states().into_iter().find(|(n, _)| *n == "opening").unwrap().1;
        let v = values(&opening);
        assert_eq!(text(&v, STATUS).as_deref(), Some("OPENING 0:00 / 8:26"));
        assert!(RAIL.iter().all(|id| !v.get(id).unwrap().shown));
        assert!(text(&v, VIEW).is_none() && text(&v, HINTS).is_none());
    }

    #[test]
    fn a_divergence_flashes_the_view_line() {
        let s = states().into_iter().find(|(n, _)| *n == "divergence").unwrap().1;
        let v = values(&s);
        assert_eq!(v.get(VIEW).unwrap().style, Style::Flash(Pulse::TimeWarning));
        assert_eq!(text(&v, STATUS).as_deref(), Some("STOPPED 2:26 / 8:26"));
        let playing = values(&states().remove(0).1);
        assert_eq!(playing.get(VIEW).unwrap().style, Style::Normal);
    }

    #[test]
    fn the_notice_is_an_event_and_never_a_state() {
        for (name, s) in states() {
            assert!(!values(&s).get(NOTICE).unwrap().shown, "{name}");
        }
    }
}
