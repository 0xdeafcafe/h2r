//! The playback HUD's feed (docs/game/theater.md, "The playback HUD"): the
//! transport's track as the HUD kit's `theater` HUD
//! (`hud_kit_core::views::theater`, `content/core/huds/theater.hud.yaml`),
//! which the game's own HUD code draws with the disc's font and its battery
//! strip.
//!
//! The kit's widgets are drawn inside a local user's HUD, so the feed is
//! for films whose picture is the guest's (a film recorded on a machine
//! with a player) on a map where the HUD joined the player's
//! ([`joined`]). Any other film keeps the host's bar
//! (`playback_page::bar`). The feed publishes from the host's pump, not
//! from a tick: a paused film runs none and its HUD still follows the
//! viewer.

use std::sync::{Arc, Mutex, OnceLock};

use agent_proto::theater::CompatibilityState;
use hud_kit_core::model::MESSAGE_UNITS;
use hud_kit_core::views::theater::{self as kit, Hints, State, Status};

use super::overlay::Track;
use super::selector::clock;
use crate::hud_kit::{self, Handle};

/// The local user the film's notices are for: the first.
const USER: u32 = 0;

static HANDLE: OnceLock<Handle> = OnceLock::new();

/// Registers the HUD with the kit, hidden. Called while the features are
/// registered, before a map's HUDs are compiled.
pub fn register() -> Handle {
    *HANDLE.get_or_init(|| {
        let handle = hud_kit::register(Arc::new(kit::Theater));
        handle.publish(kit::hidden());
        handle
    })
}

/// The loaded map's player HUD has the Theater's widgets: the game draws
/// what the feed publishes.
pub(super) fn joined() -> bool {
    HANDLE.get().is_some_and(Handle::joined)
}

/// The rail's hit box on the kit's 640 by 480 canvas, for a click on the
/// timeline: left, top, width and height, four pixels taller than the
/// strip each way, as the host bar's is.
pub(super) const TIMELINE: [u32; 4] = [
    (kit::FRAME_ORIGIN.0 + kit::RAIL_RECT[0]) as u32,
    (kit::FRAME_ORIGIN.1 + kit::RAIL_RECT[1] - 4) as u32,
    kit::RAIL_RECT[2] as u32,
    (kit::RAIL_RECT[3] + 8) as u32,
];

/// The fraction of the film a click `along` the rail's hit box asks for
/// (0 its left edge, 1 its right): the strips are the film's halves and
/// the gap between them its middle.
pub(super) fn rail_fraction(along: f32) -> f32 {
    let width = f32::from(kit::RAIL_RECT[2] - 1);
    kit::rail_fraction((along.clamp(0.0, 1.0) * width).round() as i16)
}

/// What the HUD shows for `track`: `ready` once the transport answers,
/// `keyboard` when the viewer last drove the film from the keys, `menu`
/// while the pause menu is up (the bar gives way to it).
pub(super) fn state(track: &Track, ready: bool, keyboard: bool, menu: bool) -> State {
    let unavailable = track.compatibility == CompatibilityState::Unavailable;
    let status = if unavailable {
        Status::Unavailable
    } else if !ready {
        if track.seeking { Status::Jumping } else { Status::Opening }
    } else if track.divergence.is_some() {
        Status::Stopped
    } else if track.seeking {
        Status::Jumping
    } else if track.paused {
        Status::Paused
    } else {
        Status::Playing
    };
    let (start, end) = track.bounds();
    let rate = track.tick_rate.max(1);
    let camera = match track.camera_target() {
        Some(player) => format!("{} / {}", track.camera_label(), player.name),
        None => track.camera_label().to_owned(),
    };
    let (view, alert) = if unavailable {
        let why = track.compatibility_reason.as_deref().unwrap_or("Playback is unavailable");
        (why.to_owned(), false)
    } else if let Some(d) = &track.divergence {
        (format!("Replay diverged at {}", clock(d.tick, rate)), true)
    } else if let Some(jump) = track.jump.filter(|_| track.seeking) {
        (format!("Target {}", clock(jump.target, rate)), false)
    } else if !ready {
        (String::new(), false)
    } else if let Some(from) = track.marks.0 {
        let to = track.marks.1.map_or_else(|| "--".into(), |tick| clock(tick, rate));
        (format!("Clip {} - {to}", clock(from, rate)), false)
    } else if let Some(notice) = track.monitor_notice() {
        (notice.to_owned(), false)
    } else {
        (camera, false)
    };
    let hints = if !ready || unavailable {
        Hints::Off
    } else if track.seeking {
        Hints::Jump
    } else if keyboard {
        Hints::Keys
    } else {
        Hints::Transport
    };
    State {
        shown: track.visible && !menu,
        status,
        tick: track.tick.clamp(start, end) - start,
        length: end - start,
        tick_rate: rate,
        speed: track.speed_requested,
        view,
        alert,
        hints,
    }
}

/// A notice as one line of the game's notification list: its line breaks
/// spaces, cut to the list's 62 units.
pub(super) fn notice_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut units = 0;
    line.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= MESSAGE_UNITS
        })
        .collect()
}

/// What the feed last gave the kit, so an unchanged film publishes nothing.
#[derive(Default)]
struct Last {
    state: Option<State>,
    notice: Option<String>,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

/// Gives the kit `state` (`None`: no film's HUD is the guest's, every
/// element hidden) and sends `notice` as a notification line when it is a
/// new one. Returns whether anything was published.
pub(super) fn publish(state: Option<State>, notice: Option<&str>) -> bool {
    let Some(handle) = HANDLE.get() else {
        return false;
    };
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let last = last.get_or_insert_with(Last::default);
    let mut published = false;
    if last.state != state {
        handle.publish(state.as_ref().map_or_else(kit::hidden, kit::values));
        last.state = state.clone();
        published = true;
    }
    let notice = notice.filter(|_| state.is_some()).map(notice_line).filter(|n| !n.is_empty());
    if last.notice != notice {
        if let Some(line) = &notice {
            handle.notify(USER, line);
            published = true;
        }
        last.notice = notice;
    }
    published
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theater_ui::playback_page::{Divergence, Jump};

    fn track() -> Track {
        use crate::theater_ui::adapter::{FilmMeta, Source};
        let mut track = Track::from_entry(&crate::theater_ui::Entry {
            film: FilmMeta {
                id: "test".into(),
                map: "lockout".into(),
                variant_name: "Slayer".into(),
                length_ticks: 506 * 30,
                source: Source::Local,
                compatibility: CompatibilityState::Playable,
                ..FilmMeta::default()
            },
            clip: None,
            archive_clip: None,
        });
        track.tick = 146 * 30;
        track.length = 506 * 30;
        track.tick_rate = 30;
        track.visible = true;
        track
    }

    #[test]
    fn the_hud_says_what_the_host_bar_says() {
        let mut t = track();
        let s = state(&t, true, false, false);
        assert!(s.shown);
        assert_eq!(kit::status_line(&s), "PLAYING 2:26 / 8:26");
        assert_eq!(s.hints, Hints::Transport);
        assert!(!s.view.is_empty(), "the camera and its subject");
        assert!(!s.alert);

        t.paused = true;
        t.speed_requested = 0.5;
        assert_eq!(kit::status_line(&state(&t, true, false, false)), "PAUSED 2:26 / 8:26 0.5x");

        // The keyboard's keys are words; the pad's are the font's glyphs.
        assert_eq!(state(&t, true, true, false).hints, Hints::Keys);
    }

    #[test]
    fn a_jump_a_marked_range_and_a_divergence_take_the_view_line() {
        let mut t = track();
        t.marks = (Some(146 * 30), None);
        assert_eq!(state(&t, true, false, false).view, "Clip 2:26 - --");
        t.marks.1 = Some(154 * 30);
        assert_eq!(state(&t, true, false, false).view, "Clip 2:26 - 2:34");

        t.seeking = true;
        t.jump = Some(Jump { target: 400 * 30, from_tick: 146 * 30, remaining_ticks: 90 });
        let s = state(&t, true, false, false);
        assert_eq!((s.status, s.view.as_str(), s.hints), (Status::Jumping, "Target 6:40", Hints::Jump));

        t.seeking = false;
        t.divergence = Some(Divergence { tick: 146 * 30, kind: "objects".into() });
        let s = state(&t, true, false, false);
        assert_eq!((s.status, s.view.as_str(), s.alert), (Status::Stopped, "Replay diverged at 2:26", true));
    }

    #[test]
    fn a_film_that_is_opening_or_cannot_play_has_no_hints() {
        let mut t = track();
        let s = state(&t, false, false, false);
        assert_eq!((s.status, s.hints), (Status::Opening, Hints::Off));
        assert!(s.view.is_empty());
        t.compatibility = CompatibilityState::Unavailable;
        t.compatibility_reason = Some("Film predates a breaking simulation change.".into());
        let s = state(&t, true, false, false);
        assert_eq!((s.status, s.hints), (Status::Unavailable, Hints::Off));
        assert_eq!(s.view, "Film predates a breaking simulation change.");
    }

    #[test]
    fn the_bar_hides_with_back_and_under_the_pause_menu() {
        let mut t = track();
        assert!(!state(&t, true, false, true).shown, "the pause menu is up");
        t.visible = false;
        assert!(!state(&t, true, false, false).shown, "Back hid the bar");
        assert!(kit::values(&state(&t, true, false, false)).0.iter().all(|(_, v)| !v.shown));
    }

    #[test]
    fn a_clip_s_rail_runs_over_its_range() {
        let mut t = track();
        t.tick = 150 * 30;
        let whole = state(&t, true, false, false);
        assert_eq!((whole.tick, whole.length), (150 * 30, 506 * 30));
        let (start, end) = t.bounds();
        assert_eq!((start, end), (0, 506 * 30));
    }

    #[test]
    fn a_notice_is_one_line_of_at_most_62_units() {
        assert_eq!(notice_line("Film added to Saved"), "Film added to Saved");
        assert_eq!(notice_line("Couldn't save film:\nno space"), "Couldn't save film: no space");
        let long = "x".repeat(200);
        assert_eq!(notice_line(&long).encode_utf16().count(), MESSAGE_UNITS);
        assert!(hud_kit_core::model::message_fault(&notice_line(&long)).is_none());
    }

    #[test]
    fn a_click_on_the_rail_maps_through_its_gap() {
        assert_eq!(rail_fraction(0.0), 0.0);
        assert_eq!(rail_fraction(1.0), 1.0);
        assert_eq!(rail_fraction(0.5), 0.5, "the gap between the strips");
        let quarter = rail_fraction(0.25);
        assert!((quarter - 0.25).abs() < 0.02, "{quarter}");
        assert_eq!(TIMELINE, [226, 57, 188, 16]);
    }
}
