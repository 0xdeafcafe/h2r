//! A script's objective on the HUD kit (docs/game/hud-kit.md, "The scripted
//! objective HUD"): the objective, marks, meter and countdown a script sets
//! with the `hud.objective` capability (`objective`, `objective_mark`,
//! `meter`, `countdown`) are kept per team in the VM's state
//! (`rules::vm::RulesExt::objectives`), and the kit's
//! `script_objective` HUD draws the viewer's team's.
//!
//! Each tick [`publish`] makes a [`View`] for each local user from the
//! state, the blob's strings and the zones, and publishes its values; a
//! mark's navpoint is over its zone for user 0 (the kit's navpoints are
//! shared). A script that does not require `hud.objective` draws nothing:
//! [`publish`] returns false and the runtime hides the HUD once. Every
//! machine that runs the VM makes the same values from the same state.

use std::sync::{Arc, OnceLock};

use ::rules::ir::Ir;
use ::rules::vm::{MarkKind as Kind, Objective, RulesExt};
use guest_layout::GuestMemory;
use hud_kit_core::views::script_objective::{self as kit, Mark, MarkKind, ScriptObjective, View};

use crate::hud_kit::{self, Handle, USERS};
use crate::rules::world::hud::metres;
use crate::rules::world::show::roster;
use crate::rules::world::zones::{self, Zones};

/// The capability whose script feeds the HUD.
pub const CAPABILITY: &str = "hud.objective";

/// The kit's handle of the HUD, registered once.
fn handle() -> Handle {
    static HANDLE: OnceLock<Handle> = OnceLock::new();
    *HANDLE.get_or_init(|| hud_kit::register(Arc::new(ScriptObjective)))
}

/// Registers the HUD with the kit, hidden, so the game installs its
/// widgets at its activation. Called once when the rules feature installs.
pub fn register() {
    handle().publish(kit::hidden());
}

/// Hides the HUD for every local user.
pub fn clear() {
    let h = handle();
    h.publish(kit::hidden());
    for user in 1..USERS {
        h.publish_for(user, None);
    }
    for id in kit::NAVS {
        h.navpoint_at(id, None);
    }
}

/// Whether the script `ir` feeds the HUD: it requires `hud.objective`.
pub fn fed_by(ir: &Ir) -> bool {
    ir.header.requires.iter().any(|r| r == CAPABILITY)
}

/// What a viewer on a team with `o` sees at `tick`: the line from the
/// blob's strings, the clock in whole seconds, the gauge and each marked
/// zone with the distance `metres_to` gives (the zone object's, from the
/// viewer's unit; none without a unit or a zone).
pub fn view_of(o: &Objective, strings: &[String], tick: u32, metres_to: &dyn Fn(u32) -> Option<u16>) -> View {
    View {
        text: o.text.and_then(|t| strings.get(usize::from(t))).cloned(),
        seconds: o
            .left(tick)
            .map(|t| kit::seconds_of(t, ::rules::caps::TICKS_PER_SECOND as u32)),
        meter: o.meter,
        marks: o
            .marked()
            .map(|(zone, kind)| Mark {
                kind: match kind {
                    Kind::Attack => MarkKind::Attack,
                    Kind::Defend => MarkKind::Defend,
                },
                metres: metres_to(zone),
            })
            .collect(),
    }
}

/// Feeds the HUD from the VM's state for each local user of this machine,
/// and puts each mark's navpoint over its zone for user 0. Returns whether
/// the script feeds it, so the runtime hides it when none does.
pub fn publish(m: &dyn GuestMemory, ir: &Ir, ext: &RulesExt, zones: &Zones) -> bool {
    if !fed_by(ir) {
        return false;
    }
    let h = handle();
    let bodies = zones::bodies(m);
    let middle = |zone: u32| zones.get(zone).map(|z| z.shape.middle());
    for p in roster(m) {
        let Some(user) = p.user else {
            continue;
        };
        let me = bodies.iter().find(|b| b.slot == p.slot).map(|b| b.at);
        let o = p
            .team
            .and_then(|t| ext.objectives.get(usize::from(t)))
            .copied()
            .unwrap_or_default();
        let distance = |zone: u32| Some(metres(me?, middle(zone)?));
        let view = view_of(&o, &ir.strings, ext.tick, &distance);
        let values = kit::values(&view);
        if user == 0 {
            h.publish(values);
            for (id, mark) in kit::NAVS.into_iter().zip(o.marked().map(Some).chain(std::iter::repeat(None))) {
                h.navpoint_at(id, mark.and_then(|(zone, _)| middle(zone)));
            }
        } else {
            h.publish_for(user, Some(values));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objective() -> Objective {
        let mut o = Objective::default();
        o.text = Some(1);
        o.ends = Some(900);
        o.meter = Some((10, 40));
        o.marks[0] = Some((0x4001_0000, Kind::Attack));
        o.marks[1] = Some((0x4001_0001, Kind::Defend));
        o
    }

    fn strings() -> Vec<String> {
        vec!["unused".into(), "Phase 1 of 3: take a site".into()]
    }

    /// The team's state becomes the viewer's: the line from the strings,
    /// the clock to the second, the gauge and a mark each, with their
    /// distances.
    #[test]
    fn a_teams_state_is_the_viewers_view() {
        let d = |z: u32| (z == 0x4001_0000).then_some(42);
        let v = view_of(&objective(), &strings(), 300, &d);
        assert_eq!(v.text.as_deref(), Some("Phase 1 of 3: take a site"));
        assert_eq!(v.seconds, Some(20));
        assert_eq!(v.meter, Some((10, 40)));
        assert_eq!(
            v.marks,
            [
                Mark { kind: MarkKind::Attack, metres: Some(42) },
                Mark { kind: MarkKind::Defend, metres: None },
            ]
        );
        // Past the end the clock reads 0:00; a team with nothing set sees
        // an empty view, which draws nothing.
        assert_eq!(view_of(&objective(), &strings(), 5000, &d).seconds, Some(0));
        let none = view_of(&Objective::default(), &strings(), 0, &d);
        assert!(none.is_empty());
        assert_eq!(kit::values(&none), kit::hidden());
        // A line past the blob's strings is no line.
        let mut bad = objective();
        bad.text = Some(9);
        assert_eq!(view_of(&bad, &strings(), 0, &d).text, None);
    }

    /// The last 30 seconds flash and split screen keeps the clock: the
    /// values are the kit's, whatever the screen mode.
    #[test]
    fn the_last_30_seconds_reach_the_values_flashing() {
        let d = |_: u32| None;
        let at = |tick| {
            let v = kit::values(&view_of(&objective(), &strings(), tick, &d));
            v.get(kit::CLOCK).unwrap().clone()
        };
        assert_eq!(at(0).text, "0:30");
        assert_eq!(at(0).style, hud_kit_core::model::Style::Flash(hud_kit_core::model::Pulse::Help));
        assert_eq!(at(0).text, "0:30");
        assert_eq!(at(1000).text, "0:00");
    }

    /// A script that does not require the capability feeds nothing.
    #[test]
    fn a_script_without_the_capability_draws_no_hud() {
        let src = "rules \"Quiet\" 1\ntype slayer\non tick { }\n";
        let (ir, _) = ::rules::build(src).unwrap();
        assert!(!fed_by(&ir));
        let src = "rules \"Loud\" 1\ntype slayer\nrequires hud.objective\non tick { }\n";
        let (ir, _) = ::rules::build(src).unwrap();
        assert!(fed_by(&ir));
    }
}
