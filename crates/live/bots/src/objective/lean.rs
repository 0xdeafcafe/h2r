//! How a name plays the flag and the bomb (docs/game/bots.md, section 3.7,
//! "Roles"): the continuous leanings the CTF and Assault plans read from
//! the bot's personality, itself a stable draw from its name within the
//! difficulty's band (`name_profile`). The same name leans the same way in
//! every game; two names on one team lean differently.

use crate::decisions::Personality;

use super::{DEFEND_RADIUS, RETURN_RANGE};

/// The personality axes the flag and bomb plans read, each in 0..1. The
/// default (all 0.5) is a neutral player: the plans then do what the rank
/// alone asks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lean {
    pub aggression: f32,
    pub caution: f32,
    pub teamwork: f32,
    pub focus: f32,
}

impl Default for Lean {
    fn default() -> Self {
        Lean {
            aggression: 0.5,
            caution: 0.5,
            teamwork: 0.5,
            focus: 0.5,
        }
    }
}

/// The narrowest and widest a defender patrols round our flag or plate.
pub const GUARD_MIN: f32 = 3.0;
pub const GUARD_MAX: f32 = 8.0;

impl Lean {
    pub fn of(p: &Personality) -> Lean {
        Lean {
            aggression: p.aggression.clamp(0.0, 1.0),
            caution: p.caution.clamp(0.0, 1.0),
            teamwork: p.teamwork.clamp(0.0, 1.0),
            focus: p.objective_focus.clamp(0.0, 1.0),
        }
    }

    /// An attacker who would rather cover a teammate nearer the flag or
    /// bomb than race them to it: teamwork over drive.
    pub fn escorts(&self) -> bool {
        self.teamwork > 0.5 * (self.aggression + self.focus)
    }

    /// How far a defender strays from our flag or plate to fight: a bold
    /// one patrols wide, a careful one sits on it.
    pub fn guard_radius(&self) -> f32 {
        (DEFEND_RADIUS * (1.0 + self.aggression - self.caution)).clamp(GUARD_MIN, GUARD_MAX)
    }

    /// How far an attacker turns back to return our dropped flag.
    pub fn return_range(&self) -> f32 {
        RETURN_RANGE * (0.5 + self.teamwork)
    }

    /// An attacker hunts our flag's carrier when the carrier is nearer
    /// than this share of the way to its own target: a focused one keeps
    /// on for the flag.
    pub fn carrier_pull(&self) -> f32 {
        1.5 - self.focus
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_neutral_lean_keeps_the_ranked_plan() {
        let l = Lean::default();
        assert!(!l.escorts());
        assert_eq!(l.guard_radius(), DEFEND_RADIUS);
        assert_eq!(l.return_range(), RETURN_RANGE);
        assert_eq!(l.carrier_pull(), 1.0);
    }

    #[test]
    fn leanings_are_continuous_and_bounded() {
        let bold = Lean {
            aggression: 0.9,
            caution: 0.2,
            teamwork: 0.3,
            focus: 0.9,
        };
        let careful = Lean {
            aggression: 0.2,
            caution: 0.9,
            teamwork: 0.9,
            focus: 0.4,
        };
        assert!(!bold.escorts() && careful.escorts());
        assert!(bold.guard_radius() > careful.guard_radius());
        assert!((GUARD_MIN..=GUARD_MAX).contains(&bold.guard_radius()));
        assert!((GUARD_MIN..=GUARD_MAX).contains(&careful.guard_radius()));
        assert!(careful.return_range() > bold.return_range());
        assert!(bold.carrier_pull() < careful.carrier_pull());
    }

    #[test]
    fn the_same_name_leans_the_same_way_and_names_differ() {
        use crate::mind::Skill;
        use crate::name_profile::NameProfile;
        let lean =
            |name: &str| Lean::of(&NameProfile::for_name(name).personality(Skill::Legendary));
        assert_eq!(lean("Kestrel"), lean("Kestrel"));
        let names = [
            "Kestrel", "Saber", "Wren", "Thorn", "Moth", "Pike", "Gully", "Ember",
        ];
        let escorts = names.iter().filter(|n| lean(n).escorts()).count();
        assert!(names.iter().any(|n| lean(n) != lean("Kestrel")));
        // Over eight names both kinds of attacker turn up.
        assert!(
            escorts > 0 && escorts < names.len(),
            "{escorts} of 8 escort"
        );
    }
}
