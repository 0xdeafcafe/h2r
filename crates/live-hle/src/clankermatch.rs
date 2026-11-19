//! Clankermatch's search (docs/game/bots.md §8.4): a clanker hopper's
//! OptiMatch search finds no session, after a plausible delay, so the game
//! hosts its own lobby, which the host's Clankers fill.
//!
//! The game widens its search window step by step and searches again after
//! each answer (docs/live/live-matchmaking.md §7.3), so one search run is
//! many `XOnlineMatchSearch` calls. The delay is the run's, not each
//! call's: the first call of a run draws it, and every call of the run
//! answers no sooner than the run's start plus that delay. A call more than
//! [`RUN_GAP`] after the last one starts a new run, as does a search of
//! another hopper.

use std::time::{Duration, Instant};

pub use live_proto::hopper::clankers::{ClankerHopper, ClankerHoppers};

/// The marks the server served last, shared with the Live worker that
/// fetches them.
pub(crate) type Marks = std::sync::Arc<std::sync::Mutex<ClankerHoppers>>;

/// The shortest and longest search (§8.1 item 2).
pub const MIN_DELAY: Duration = Duration::from_secs(4);
pub const MAX_DELAY: Duration = Duration::from_secs(20);
/// Calls further apart than this are separate search runs. The game's
/// widening searches follow each other within a frame or two of the
/// answer; a new search from the playlist screen is seconds later.
pub const RUN_GAP: Duration = Duration::from_secs(10);

/// The search delay for random word `r`: uniform over
/// [`MIN_DELAY`]..=[`MAX_DELAY`] in milliseconds.
pub fn search_delay(r: u32) -> Duration {
    let span = (MAX_DELAY - MIN_DELAY).as_millis() as u32 + 1;
    MIN_DELAY + Duration::from_millis(u64::from(r % span))
}

/// One search run of a clanker hopper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    hopper: u32,
    answer_at: Instant,
    last: Instant,
}

impl Run {
    /// Whether a call at `now` belongs to this run: no more than
    /// [`RUN_GAP`] after its last call or its answer, whichever is later
    /// (a held call is answered up to [`MAX_DELAY`] after it was made, and
    /// the next widening search follows the answer).
    fn within(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last.max(self.answer_at)) <= RUN_GAP
    }
}

/// The search runs' timing.
#[derive(Debug, Default)]
pub(crate) struct Hold {
    run: Option<Run>,
}

impl Hold {
    /// When a search of `hopper` made at `now` answers. `delay` is drawn
    /// only for a new run.
    pub fn answer_at(
        &mut self,
        hopper: u32,
        now: Instant,
        delay: impl FnOnce() -> Duration,
    ) -> Instant {
        let run = match self.run {
            Some(r) if r.hopper == hopper && r.within(now) => Run { last: now, ..r },
            _ => Run {
                hopper,
                answer_at: now + delay(),
                last: now,
            },
        };
        self.run = Some(run);
        run.answer_at
    }

    /// Whether a run is under way at `now` (its answer not given yet, or
    /// given less than [`RUN_GAP`] ago).
    #[cfg(test)]
    fn running(&self, now: Instant) -> bool {
        self.run.is_some_and(|r| r.within(now))
    }
}

/// The hopper an OptiMatch search names, when it is a clanker hopper.
pub fn clanker_search(marks: &ClankerHoppers, hopper: i64) -> Option<u32> {
    let h = u32::try_from(hopper).ok()?;
    marks.contains(h).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks() -> ClankerHoppers {
        ClankerHoppers {
            hoppers: vec![ClankerHopper {
                hopper_id: 8,
                playlist_id: 9,
                players: 8,
                teams: 0,
                ranks_as: None,
            }],
        }
    }

    #[test]
    fn the_delay_is_four_to_twenty_seconds() {
        assert_eq!(search_delay(0), MIN_DELAY);
        assert_eq!(search_delay(16_000), MAX_DELAY);
        assert_eq!(search_delay(16_001), MIN_DELAY);
        for r in [1, 999, 0x1234_5678, u32::MAX] {
            let d = search_delay(r);
            assert!((MIN_DELAY..=MAX_DELAY).contains(&d), "{r}: {d:?}");
        }
    }

    #[test]
    fn only_clanker_hoppers_are_held() {
        let m = marks();
        assert_eq!(clanker_search(&m, 8), Some(8));
        assert_eq!(clanker_search(&m, 0), None, "a human hopper");
        assert_eq!(clanker_search(&m, -1), None, "the custom games browser");
        assert_eq!(
            clanker_search(&ClankerHoppers::default(), 8),
            None,
            "no marks"
        );
    }

    #[test]
    fn a_run_answers_once_its_delay_is_over() {
        let t0 = Instant::now();
        let mut h = Hold::default();
        let at = h.answer_at(8, t0, || Duration::from_secs(7));
        assert_eq!(at, t0 + Duration::from_secs(7));
        // The widening searches of the run: the same answer time, no new
        // draw.
        let s = Duration::from_secs;
        for n in 1..=7 {
            let again = h.answer_at(8, t0 + s(n), || panic!("drawn again"));
            assert_eq!(again, at);
        }
        // The next widening search comes right after a long hold's answer:
        // the same run.
        let mut long = Hold::default();
        let held = long.answer_at(8, t0, || s(20));
        assert_eq!(
            long.answer_at(8, held + Duration::from_millis(50), || panic!()),
            held
        );
        // Past the delay: at once.
        assert!(h.answer_at(8, t0 + s(9), || panic!("drawn again")) < t0 + s(9));
        assert!(h.running(t0 + s(9)));
        // A pause past the gap: a new run, a new delay.
        let later = t0 + s(9) + RUN_GAP + s(1);
        assert!(!h.running(later));
        assert_eq!(h.answer_at(8, later, || s(4)), later + s(4));
        // Another hopper: a new run too.
        let other = later + s(1);
        assert_eq!(h.answer_at(9, other, || s(5)), other + s(5));
    }
}
