//! A bot's tuning: the aim table's numbers, awareness and the personality,
//! each optional over the built-in values for its skill (docs/runtime/sim.md,
//! "Bot parameters from a file").
//!
//! The aim table and awareness are read deep in the mind, through
//! [`Skill::aim`](crate::mind::Skill::aim) and `decisions::awareness`, from
//! a dozen call sites. Rather than thread a tuning through each, a brain
//! that has one sets it for the length of its think ([`with`]), on its own
//! thread, and those two functions consult [`current`]. A brain with none
//! (every brain outside sim mode) leaves it unset, and nothing changes.

use std::cell::Cell;

use crate::decisions::Personality;
use crate::mind::Aim;

/// The burst a tuning asks for when the skill's table holds the trigger
/// (normal's).
pub const DEFAULT_BURST: (u32, u32) = (24, 8);

/// What a bot is tuned by. `None` keeps the built-in value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tuning {
    /// Nominal reaction, ticks.
    pub reaction: Option<u32>,
    /// The tracking error's spread, radians.
    pub error: Option<f32>,
    pub lead: Option<f32>,
    pub gain: Option<f32>,
    /// Radians.
    pub fire_cone: Option<f32>,
    /// `Some(false)`: hold the trigger. `Some(true)`: fire in bursts.
    pub burst: Option<bool>,
    /// Burst on and off ticks; either one implies bursts.
    pub burst_on: Option<u32>,
    pub burst_off: Option<u32>,
    pub grenades_per_minute: Option<f32>,
    /// 0..1.
    pub awareness: Option<f32>,
    pub aggression: Option<f32>,
    pub caution: Option<f32>,
    pub teamwork: Option<f32>,
    pub objective_focus: Option<f32>,
}

impl Tuning {
    /// `base` (a skill's row) with this tuning's numbers over it.
    pub fn aim(&self, base: Aim) -> Aim {
        let burst = if self.burst == Some(false) {
            None
        } else if self.burst == Some(true) || self.burst_on.is_some() || self.burst_off.is_some() {
            let (on, off) = base.burst.unwrap_or(DEFAULT_BURST);
            Some((
                self.burst_on.unwrap_or(on).max(1),
                self.burst_off.unwrap_or(off),
            ))
        } else {
            base.burst
        };
        Aim {
            reaction: self.reaction.unwrap_or(base.reaction),
            error: self.error.unwrap_or(base.error).max(0.0),
            lead: self.lead.unwrap_or(base.lead),
            gain: self.gain.unwrap_or(base.gain).max(0.0),
            fire_cone: self.fire_cone.unwrap_or(base.fire_cone).max(0.0),
            burst,
            grenades_per_minute: self
                .grenades_per_minute
                .unwrap_or(base.grenades_per_minute)
                .max(0.0),
        }
    }

    /// The skill's awareness, or this tuning's.
    pub fn awareness(&self, base: f32) -> f32 {
        self.awareness.map_or(base, |a| a.clamp(0.0, 1.0))
    }

    /// `p` with this tuning's traits over it.
    pub fn personality(&self, p: Personality) -> Personality {
        let unit = |v: Option<f32>, b: f32| v.map_or(b, |v| v.clamp(0.0, 1.0));
        Personality {
            aggression: unit(self.aggression, p.aggression),
            caution: unit(self.caution, p.caution),
            teamwork: unit(self.teamwork, p.teamwork),
            objective_focus: unit(self.objective_focus, p.objective_focus),
        }
    }

    /// Whether it sets anything at all.
    pub fn is_empty(&self) -> bool {
        *self == Tuning::default()
    }
}

thread_local! {
    static CURRENT: Cell<Option<Tuning>> = const { Cell::new(None) };
}

/// The tuning of the brain thinking on this thread, if it has one.
pub fn current() -> Option<Tuning> {
    CURRENT.with(Cell::get)
}

/// Runs `f` with `t` as this thread's tuning, and puts the old one back
/// after (a panic included).
pub fn with<R>(t: Option<Tuning>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Tuning>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.replace(t)));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::Skill;

    #[test]
    fn unset_changes_nothing() {
        for s in Skill::ALL {
            assert_eq!(Tuning::default().aim(s.base_aim()), s.base_aim());
            assert_eq!(s.aim(), s.base_aim());
        }
        assert!(current().is_none());
        assert!(Tuning::default().is_empty());
    }

    #[test]
    fn numbers_override_the_row() {
        let t = Tuning {
            reaction: Some(3),
            fire_cone: Some(0.01),
            error: Some(-1.0),
            ..Tuning::default()
        };
        let a = t.aim(Skill::Easy.base_aim());
        assert_eq!(a.reaction, 3);
        assert_eq!(a.fire_cone, 0.01);
        assert_eq!(a.error, 0.0);
        assert_eq!(a.lead, Skill::Easy.base_aim().lead);
    }

    #[test]
    fn bursts() {
        let hold = Tuning {
            burst: Some(false),
            ..Tuning::default()
        };
        assert_eq!(hold.aim(Skill::Easy.base_aim()).burst, None);
        let on = Tuning {
            burst_on: Some(5),
            ..Tuning::default()
        };
        // Easy's off time kept; heroic holds, so normal's.
        assert_eq!(on.aim(Skill::Easy.base_aim()).burst, Some((5, 18)));
        assert_eq!(on.aim(Skill::Heroic.base_aim()).burst, Some((5, 8)));
        let yes = Tuning {
            burst: Some(true),
            ..Tuning::default()
        };
        assert_eq!(
            yes.aim(Skill::Legendary.base_aim()).burst,
            Some(DEFAULT_BURST)
        );
        let zero = Tuning {
            burst_on: Some(0),
            ..Tuning::default()
        };
        assert_eq!(zero.aim(Skill::Easy.base_aim()).burst, Some((1, 18)));
    }

    #[test]
    fn scoped_to_the_call() {
        let t = Tuning {
            reaction: Some(1),
            awareness: Some(0.2),
            ..Tuning::default()
        };
        let inside = with(Some(t), || {
            assert_eq!(current(), Some(t));
            // Nested: the inner one wins, then the outer is back.
            with(None, || assert!(current().is_none()));
            assert_eq!(current(), Some(t));
            (
                Skill::Legendary.aim().reaction,
                crate::decisions::awareness(Skill::Legendary),
            )
        });
        assert_eq!(inside, (1, 0.2));
        assert!(current().is_none());
        assert_eq!(
            Skill::Legendary.aim().reaction,
            Skill::Legendary.base_aim().reaction
        );
        assert_eq!(crate::decisions::awareness(Skill::Legendary), 1.0);
    }

    #[test]
    fn restored_after_a_panic() {
        let t = Tuning {
            gain: Some(9.0),
            ..Tuning::default()
        };
        let r = std::panic::catch_unwind(|| with(Some(t), || -> u32 { panic!("think") }));
        assert!(r.is_err());
        assert!(current().is_none());
    }

    #[test]
    fn personality_clamps() {
        let base = Personality {
            aggression: 0.5,
            caution: 0.5,
            teamwork: 0.5,
            objective_focus: 0.5,
        };
        let t = Tuning {
            aggression: Some(2.0),
            teamwork: Some(0.1),
            ..Tuning::default()
        };
        let p = t.personality(base);
        assert_eq!(
            (p.aggression, p.caution, p.teamwork, p.objective_focus),
            (1.0, 0.5, 0.1, 0.5)
        );
        assert_eq!(t.awareness(0.65), 0.65);
        assert_eq!(
            Tuning {
                awareness: Some(1.5),
                ..Tuning::default()
            }
            .awareness(0.3),
            1.0
        );
    }
}
