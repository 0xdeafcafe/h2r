//! How hard a named bot pushes the stick, and when it hops on the way
//! (docs/game/bots.md, "Pace and hops"). Both come from the name's
//! [`Weights`]: a bot without a profile keeps its thumb's own pace and
//! never hops on a run.

use super::feel::{Rng, reached};
use crate::name_profile::Weights;
use crate::nav::CRUISE_PACE;

/// Between rolls for a hop on an open straight run (ticks, rolled).
pub(crate) const HOP_EVERY: (u32, u32) = (30, 90);
/// The chance a roll hops, times the name's jump weight.
pub(crate) const HOP_CHANCE: f32 = 0.6;

/// How it pushes the stick this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Push {
    /// Closes the thumb's shortfall to full stick (`pad::Feet::step`).
    pub push: f32,
    /// Scales a route's walk: lifts its cruise to full stick.
    pub lift: f32,
}

/// Travelling with nothing in view, a named bot runs at full stick. In a
/// fight it aims with care, and only a strafe-heavy name keeps the stick
/// pinned while it does.
pub(crate) fn push(weights: Option<Weights>, fighting: bool, routed: bool) -> Push {
    match weights {
        None => Push {
            push: 0.0,
            lift: 1.0,
        },
        Some(w) if fighting => Push {
            push: w.strafe,
            lift: 1.0,
        },
        Some(_) => Push {
            push: 1.0,
            lift: if routed { 1.0 / CRUISE_PACE } else { 1.0 },
        },
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Stride {
    rng: Rng,
    next: Option<u32>,
}

impl Stride {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed ^ 0x0073_7472_6964),
            next: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.next = None;
    }

    /// On an open straight run (`open`), a roll every [`HOP_EVERY`] hops
    /// with the name's jump weight times [`HOP_CHANCE`].
    pub(crate) fn hop(&mut self, weights: Option<Weights>, open: bool, tick: u32) -> bool {
        let Some(w) = weights else {
            return false;
        };
        if !open {
            return false;
        }
        let due = match self.next {
            Some(due) => due,
            None => {
                let due = tick.wrapping_add(self.rng.ticks(HOP_EVERY.0, HOP_EVERY.1));
                self.next = Some(due);
                due
            }
        };
        if !reached(tick, due) {
            return false;
        }
        self.next = Some(tick.wrapping_add(self.rng.ticks(HOP_EVERY.0, HOP_EVERY.1)));
        self.rng.unit() < w.jump * HOP_CHANCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weights(strafe: f32, jump: f32) -> Weights {
        let mut w = Weights::from([0.5; 12]);
        w.strafe = strafe;
        w.jump = jump;
        w
    }

    #[test]
    fn a_named_bot_travels_at_full_stick_and_fights_by_its_strafe() {
        let w = Some(weights(0.3, 0.5));
        let travel = push(w, false, true);
        assert_eq!(travel.push, 1.0);
        assert!((CRUISE_PACE * travel.lift - 1.0).abs() < 1e-6);
        assert_eq!(push(w, false, false).lift, 1.0);
        assert_eq!(push(w, true, true), Push { push: 0.3, lift: 1.0 });
        assert_eq!(push(None, false, true), Push { push: 0.0, lift: 1.0 });
    }

    fn hops(jump: f32, open: bool) -> usize {
        let mut s = Stride::new(9);
        (0..3000)
            .filter(|&t| s.hop(Some(weights(0.5, jump)), open, t))
            .count()
    }

    #[test]
    fn hops_on_a_run_follow_the_names_jump_weight() {
        assert_eq!(hops(0.9, false), 0);
        assert_eq!(hops(0.0, true), 0);
        let (low, high) = (hops(0.2, true), hops(0.9, true));
        assert!(low < high, "{low} {high}");
        // A roll every 30 to 90 ticks: at most 100 in 3000 ticks.
        assert!((15..=100).contains(&high), "{high}");
        let mut s = Stride::new(9);
        assert!((0..3000).all(|t| !s.hop(None, true, t)));
    }
}
