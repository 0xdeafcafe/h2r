//! A walk to a weapon on the ground (docs/game/bots.md §3.6, "Trips that
//! fail"): each trip has a time budget from its length, and a bot that
//! stands on the weapon without the weapons decision letting it go has not
//! got it. Either way the weapon is refused for a while, so the bot goes
//! back to fighting and roaming instead of walking the same trip again.
//!
//! Pure, time in game ticks (30 a second).

/// A trip may take this long before the walk itself...
pub const TRIP_SLACK: u32 = 90;
/// ...and this long per world unit of the straight line at its start
/// (a player walks a unit in about 13 ticks; routes bend).
pub const TRIP_PER_UNIT: u32 = 20;
/// Within this of the weapon (world units, XY) the bot is on it...
pub const ON_IT: f32 = 1.0;
/// ...and still wanting it after this long, it will not come (a weapon of
/// the same kind that gives nothing on a walk-over, one on a ledge or in a
/// wall). Longer than the action button's hold (`mind::TAKE_TICKS`).
pub const ON_IT_TICKS: u32 = 45;
/// A refused weapon is not sought again for this long.
pub const REFUSE_TICKS: u32 = 900;
/// At most this many refusals are kept (the oldest goes).
pub const REFUSE_MAX: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Trip {
    id: u32,
    since: u32,
    budget: u32,
    on_it: Option<u32>,
}

/// The trip under way and the weapons refused.
#[derive(Clone, Debug, Default)]
pub(crate) struct Errands {
    trip: Option<Trip>,
    refused: Vec<(u32, u32)>,
}

impl Errands {
    /// Whether the weapon object `id` is refused at `tick`.
    pub fn refused(&self, id: u32, tick: u32) -> bool {
        self.refused
            .iter()
            .any(|&(r, at)| r == id && tick.wrapping_sub(at) < REFUSE_TICKS)
    }

    /// Refuses `id` from `tick` and ends a trip to it.
    pub fn refuse(&mut self, id: u32, tick: u32) {
        self.refused
            .retain(|&(r, at)| r != id && tick.wrapping_sub(at) < REFUSE_TICKS);
        if self.refused.len() >= REFUSE_MAX {
            self.refused.remove(0);
        }
        self.refused.push((id, tick));
        if self.trip.is_some_and(|t| t.id == id) {
            self.trip = None;
        }
    }

    /// One tick of the trip to `id`, `dist` away (XY). `false`: it is over
    /// budget or the bot has stood on it too long; it is refused.
    pub fn step(&mut self, id: u32, tick: u32, dist: f32) -> bool {
        let mut trip = match self.trip {
            Some(t) if t.id == id => t,
            _ => Trip {
                id,
                since: tick,
                budget: TRIP_SLACK + (TRIP_PER_UNIT as f32 * dist.max(0.0)) as u32,
                on_it: None,
            },
        };
        trip.on_it = if dist < ON_IT {
            Some(trip.on_it.unwrap_or(tick))
        } else {
            None
        };
        let late = tick.wrapping_sub(trip.since) > trip.budget;
        let stood = trip
            .on_it
            .is_some_and(|since| tick.wrapping_sub(since) >= ON_IT_TICKS);
        if late || stood {
            self.refuse(id, tick);
            return false;
        }
        self.trip = Some(trip);
        true
    }

    /// No trip this tick: the next one starts afresh.
    pub fn idle(&mut self) {
        self.trip = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trip_over_its_budget_is_refused_for_a_while() {
        let mut e = Errands::default();
        // 10 units: 90 + 200 ticks.
        for t in 0..=290 {
            assert!(e.step(7, t, 10.0), "tick {t}");
        }
        assert!(!e.step(7, 291, 9.0));
        assert!(e.refused(7, 291));
        assert!(e.refused(7, 291 + REFUSE_TICKS - 1));
        assert!(!e.refused(7, 291 + REFUSE_TICKS));
        assert!(!e.refused(8, 291), "only that one");
    }

    #[test]
    fn standing_on_a_weapon_that_will_not_come_gives_it_up() {
        let mut e = Errands::default();
        assert!(e.step(3, 0, 5.0));
        // Arrives at tick 20 and stays on it.
        for t in 20..20 + ON_IT_TICKS {
            assert!(e.step(3, t, 0.3), "tick {t}");
        }
        assert!(!e.step(3, 20 + ON_IT_TICKS, 0.3));
        assert!(e.refused(3, 20 + ON_IT_TICKS));
        // Stepping off resets the clock on it.
        let mut e = Errands::default();
        for t in 0..ON_IT_TICKS {
            assert!(e.step(4, t, 0.3));
        }
        assert!(e.step(4, ON_IT_TICKS, 2.0));
        assert!(e.step(4, ON_IT_TICKS + 1, 0.3));
    }

    #[test]
    fn a_new_weapon_or_a_pause_starts_a_fresh_trip() {
        let mut e = Errands::default();
        for t in 0..200 {
            assert!(e.step(1, t, 10.0));
        }
        // Another weapon: its own budget from now.
        assert!(e.step(2, 200, 2.0));
        e.idle();
        for t in 300..300 + TRIP_SLACK + 40 {
            assert!(e.step(2, t, 2.0), "tick {t}");
        }
    }

    #[test]
    fn refusals_are_bounded() {
        let mut e = Errands::default();
        for id in 0..(REFUSE_MAX as u32 + 4) {
            e.refuse(id, 10);
        }
        assert!(!e.refused(0, 10), "the oldest went");
        assert!(e.refused(REFUSE_MAX as u32 + 3, 10));
    }
}
