//! The whole Forge editor in one frame, for the preview only (`h2 hud
//! preview forge-all`): the monitor's `forge` HUD and the readout's
//! elements together, at the moments of one edit, each pairing a state of
//! each HUD. The runtime compiles the two HUDs on their own
//! ([`super::all`]); this one is never compiled into a map.
//! docs/game/forge-map-variants-ui.md, "Previews".

use crate::model::{Describe, Hud, Value, Values};
use crate::world::Pose;

use super::forge::{self, Forge};
use super::forge_readout::Readout;

/// The combined HUD's name.
pub const NAME: &str = "forge-all";

/// The moments of an edit: a name, the `forge` HUD's state, the
/// readout's. From flying with nothing aimed at, through aim, pick up, a
/// snapped hold, a refused drop, a drop, a field change and a save.
pub const MOMENTS: [(&str, &str, &str); 8] = [
    ("1-flying", "idle", "idle"),
    ("2-aim", "aimed", "aim-spawner"),
    ("3-pick-up", "placing", "hold-allowed"),
    ("4-snapped-hold", "holding", "hold-snapped"),
    ("5-refused-drop", "refused", "refused"),
    ("6-drop", "aimed", "aim-object"),
    ("7-field-change", "turning", "hold-allowed"),
    ("8-save", "idle", "idle"),
];

/// The moment that shows a save: no state of the `forge` HUD is one, so
/// its notice is the store's "Saved <name>." for a sample name.
pub const SAVE_MOMENT: &str = "8-save";
pub const SAVE_NOTICE: &str = "Saved Arena Rumble.";

/// Both HUDs at once.
pub struct All;

impl Describe for All {
    /// Both HUDs' elements, fullscreen only: the readout has no half or
    /// quarter place, so a split frame of both is the `forge` HUD's own
    /// (`h2 hud preview forge --mode all`), and one HUD's widgets show in
    /// the same screen modes (the `modes` check).
    fn hud(&self) -> Hud {
        let mut elements = Forge.hud().elements;
        elements.extend(Readout.hud().elements);
        for e in &mut elements {
            e.at.half = None;
            e.at.quarter = None;
            e.at.split_origin = None;
        }
        Hud {
            name: NAME,
            elements,
        }
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        let monitor = Forge.states();
        let readout = Readout.states();
        let pick = |list: &[(&'static str, Values)], n: &str| {
            list.iter()
                .find(|(name, _)| *name == n)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("no state {n}"))
        };
        MOMENTS
            .iter()
            .map(|(name, a, b)| {
                let mut v = pick(&monitor, a);
                v.0.extend(pick(&readout, b).0);
                if *name == SAVE_MOMENT {
                    v.set(forge::NOTICE, Value::text(SAVE_NOTICE));
                }
                (*name, v)
            })
            .collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Forge.multiplayer_only()
    }

    fn never_with(&self) -> &'static [&'static str] {
        Forge.never_with()
    }

    fn monitor(&self) -> bool {
        Forge.monitor()
    }

    /// A moment's world marks are its two states': the gizmo's and the
    /// readout's.
    fn world(&self, state: &str) -> Vec<(&'static str, Pose)> {
        MOMENTS
            .iter()
            .find(|(name, _, _)| *name == state)
            .map(|(_, a, b)| {
                let mut marks = Forge.world(a);
                marks.extend(Readout.world(b));
                marks
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every moment names states both HUDs have, and gives each element of
    /// the combined HUD exactly one value.
    #[test]
    fn every_moment_is_a_whole_layout_of_both_huds() {
        let hud = All.hud();
        let states = All.states();
        assert_eq!(states.len(), MOMENTS.len());
        for (name, v) in states {
            assert!(
                v.layout_problems(&hud).is_empty(),
                "{name}: {:?}",
                v.layout_problems(&hud)
            );
        }
        let names = |d: &dyn Describe| -> Vec<&'static str> {
            d.states().into_iter().map(|(n, _)| n).collect()
        };
        for (moment, a, b) in MOMENTS {
            assert!(names(&Forge).contains(&a), "{moment}: {a}");
            assert!(names(&Readout).contains(&b), "{moment}: {b}");
        }
    }

    /// The two HUDs share no element id, so a moment's values are each
    /// HUD's own.
    #[test]
    fn the_two_huds_share_no_element() {
        let own = Forge.hud().elements.len() + Readout.hud().elements.len();
        assert_eq!(All.hud().elements.len(), own);
        let mut ids: Vec<_> = All.hud().elements.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), own);
    }

    /// The moments tell the edit: nothing aimed, then a name and its
    /// facts, a held thing's guide, a refusal in both HUDs, a field
    /// change's heading, and the save's notice.
    #[test]
    fn the_moments_tell_the_edit() {
        use super::super::forge_readout as rd;
        let states = All.states();
        let at = |m: &str| &states.iter().find(|(n, _)| *n == m).unwrap().1;
        let shown = |m: &str, id: &str| at(m).get(id).filter(|x| x.shown).map(|x| x.text.clone());
        assert_eq!(shown("1-flying", rd::NAME_ROW), None);
        assert!(shown("2-aim", rd::NAME_ROW).is_some());
        assert!(shown("2-aim", rd::FACTS).is_some());
        assert!(shown("4-snapped-hold", rd::GUIDE).is_some());
        assert_eq!(shown("5-refused-drop", forge::NOTICE).as_deref(), Some(forge::NOTICE_LONGEST));
        assert!(shown("5-refused-drop", rd::GUIDE).is_some());
        assert_eq!(shown("8-save", forge::NOTICE).as_deref(), Some(SAVE_NOTICE));
        for (name, _, _) in MOMENTS {
            if name != SAVE_MOMENT && name != "5-refused-drop" {
                assert_eq!(shown(name, forge::NOTICE), None, "{name}");
            }
        }
    }

    /// A moment's marks are its two states' together: the gizmo while
    /// holding, the readout's selection while aimed, none flying.
    #[test]
    fn a_moment_poses_both_huds_marks() {
        let ids = |m: &str| All.world(m).into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        assert!(ids("1-flying").iter().all(|id| id.starts_with("fgr.")));
        let holding = ids("4-snapped-hold");
        for id in forge::GIZMO {
            assert!(holding.contains(&id), "{id}");
        }
        assert!(holding.iter().any(|id| id.starts_with("fgr.")));
        assert_eq!(All.world("nope"), vec![]);
    }

    /// The combined HUD is fullscreen only and the monitor's, as the
    /// `forge` HUD is, and never with the superbounce readouts.
    #[test]
    fn the_combined_hud_is_the_monitors_and_fullscreen_only() {
        for e in All.hud().elements {
            assert_eq!((e.at.half, e.at.quarter, e.at.split_origin), (None, None, None), "{}", e.id);
        }
        assert!(All.monitor());
        assert_eq!(All.never_with(), Forge.never_with());
        assert_eq!(All.multiplayer_only(), Forge.multiplayer_only());
    }
}
