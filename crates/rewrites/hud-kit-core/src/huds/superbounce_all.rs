//! The whole superbounce HUD in one frame, for the preview only (`h2 hud
//! preview superbounce-all`): the superbounce HUD's elements and the
//! readout's together, at named moments of a run that pair a state of
//! each. The runtime compiles the two HUDs on their own ([`super::all`]);
//! this one is never compiled into a map.

use crate::model::{Describe, Hud, Values};

use super::superbounce::Superbounce;
use super::superbounce_readout::Readout;

/// The combined HUD's name.
pub const NAME: &str = "superbounce-all";

/// The moments: a name, the superbounce HUD's state, the readout's.
pub const MOMENTS: [(&str, &str, &str); 14] = [
    ("1-walking", "not-primed", "moving"),
    ("2-arming", "under", "idle"),
    ("3-primed-targets", "primed-full", "targets"),
    ("4-primed-tagged", "primed-full", "tagged"),
    ("5-charge-draining", "primed-draining", "moving"),
    ("6-falling-on-seam", "primed-full", "falling-on-seam"),
    ("7-falling-off-seam", "primed-full", "falling-off-seam"),
    ("8-bounced", "bounced", "idle"),
    ("9-medal-chain", "medal-chain", "idle"),
    ("10-miss-too-slow", "lost", "miss-too-slow"),
    ("11-miss-off-seam", "not-primed", "miss-missed"),
    ("12-miss-not-primed", "not-primed", "miss-not-primed"),
    ("13-second-bounce-targets", "bounced", "second-bounce"),
    ("14-second-bounce-lined-up", "bounced", "second-bounce-lined-up"),
];

/// Both HUDs at once.
pub struct All;

impl Describe for All {
    fn hud(&self) -> Hud {
        let mut elements = Superbounce.hud().elements;
        elements.extend(Readout.hud().elements);
        Hud { name: NAME, elements }
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        let sb = Superbounce.states();
        let rd = Readout.states();
        let pick = |list: &[(&'static str, Values)], n: &str| {
            list.iter()
                .find(|(name, _)| *name == n)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("no state {n}"))
        };
        MOMENTS
            .iter()
            .map(|(name, a, b)| {
                let mut v = pick(&sb, a);
                v.0.extend(pick(&rd, b).0);
                (*name, v)
            })
            .collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Superbounce.multiplayer_only()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every moment names states the two HUDs have, and gives each
    /// element of the combined HUD exactly one value.
    #[test]
    fn every_moment_is_a_whole_layout_of_both_huds() {
        let hud = All.hud();
        let states = All.states();
        assert_eq!(states.len(), MOMENTS.len());
        for (name, v) in states {
            assert!(v.layout_problems(&hud).is_empty(), "{name}: {:?}", v.layout_problems(&hud));
        }
    }
}
