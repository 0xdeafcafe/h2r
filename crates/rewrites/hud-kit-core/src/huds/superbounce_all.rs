//! The whole superbounce HUD in one frame, for the preview only (`h2 hud
//! preview superbounce-all`): the superbounce HUD's elements and the
//! readout's together, at named moments of a run that pair a state of
//! each. The runtime compiles the two HUDs on their own ([`super::all`]);
//! this one is never compiled into a map.

use crate::model::{Describe, Hud, Values};
use crate::world::Pose;

use super::superbounce::{self, Scan, Superbounce};
use super::superbounce_readout::Readout;
use super::superbounce_segments as segs;

/// The combined HUD's name.
pub const NAME: &str = "superbounce-all";

/// The moments: a name, the superbounce HUD's state, the readout's.
pub const MOMENTS: [(&str, &str, &str); 27] = [
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
    ("15-targets-switched", "primed-full", "targets-switched"),
    ("16-guide-approach", "primed-full", "guide-approach"),
    ("17-guide-lined-up", "primed-full", "guide-lined-up"),
    ("18-guide-falling-caught", "primed-full", "guide-falling-caught"),
    ("19-guide-too-shallow", "primed-full", "guide-too-shallow"),
    ("20-guide-off-line", "primed-full", "guide-off-line"),
    ("21-priming-near", "near", "segments-near"),
    ("22-priming-held", "held", "segments-held"),
    ("23-primed-ending", "primed-ending", "segments-primed"),
    ("24-charge-warning", "primed-still-warning", "segments-warning"),
    ("25-charge-lost", "lost-stood-still", "segments-lost"),
    ("26-guide-on-the-cut", "primed-full", "guide-on-the-cut"),
    ("27-guide-scan-only", "primed-full", "guide-scan-only"),
];

/// The charge segments at a moment: the superbounce state's charge, as
/// the game's readout takes them from the mode
/// (`superbounce_segments::segments`); none while the map is scanned or
/// has no strips.
pub fn segments_of(state: &str) -> Option<segs::Segments> {
    let (_, view) = superbounce::views().into_iter().find(|(n, _)| *n == state)?;
    (view.scan == Scan::Ready).then(|| segs::segments(view.charge))
}

/// The moment that shows the wipe-back: the superbounce state "primed
/// ending" is the arm's ending at a full charge, so the segments take a
/// charge just over two thirds left, six of nine lit, between full and the warning.
pub const WIPE_MOMENT: &str = "23-primed-ending";
pub const WIPE_FILL: f32 = 0.67;

/// The segments of `moment`, which pairs the superbounce state `state`:
/// [`segments_of`] but for [`WIPE_MOMENT`].
pub fn moment_segments(moment: &str, state: &str) -> Option<segs::Segments> {
    segments_of(state)?;
    if moment == WIPE_MOMENT {
        return Some(segs::segments(Some(superbounce::Charge {
            phase: superbounce::Phase::Primed,
            fill: WIPE_FILL,
        })));
    }
    segments_of(state)
}

/// Both HUDs at once.
pub struct All;

impl Describe for All {
    /// Both HUDs' elements, fullscreen only: the readout has no half or
    /// quarter place, so a split frame of both is the superbounce HUD's
    /// own (`h2 hud preview superbounce --mode all`), and one HUD's
    /// widgets show in the same screen modes (the `modes` check).
    fn hud(&self) -> Hud {
        let mut elements = Superbounce.hud().elements;
        elements.extend(Readout.hud().elements);
        for e in &mut elements {
            e.at.half = None;
            e.at.quarter = None;
            e.at.split_origin = None;
        }
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
                // One charge: the superbounce state's, whatever ring the
                // readout's state has.
                segs::set_values(&mut v, moment_segments(name, a).as_ref());
                (*name, v)
            })
            .collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Superbounce.multiplayer_only()
    }

    /// A moment's world marks are its readout state's.
    fn world(&self, state: &str) -> Vec<(&'static str, Pose)> {
        MOMENTS
            .iter()
            .find(|(name, _, _)| *name == state)
            .map(|(_, _, readout)| Readout.world(readout))
            .unwrap_or_default()
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

    /// The approach guide's seven moments pose the marks they show; the
    /// others have none. Each has the guide's percentage on its box:
    /// alone before the jump, after the call in a fall.
    #[test]
    fn the_guides_moments_pose_their_marks() {
        use crate::huds::superbounce_guide as guide;
        let with: Vec<_> = MOMENTS
            .iter()
            .filter(|(name, _, _)| !All.world(name).is_empty())
            .map(|(name, _, _)| *name)
            .collect();
        assert_eq!(
            with,
            [
                "16-guide-approach",
                "17-guide-lined-up",
                "18-guide-falling-caught",
                "19-guide-too-shallow",
                "20-guide-off-line",
                "26-guide-on-the-cut",
                "27-guide-scan-only"
            ]
        );
        let states = All.states();
        let label = |moment: &str| {
            let (_, v) = states.iter().find(|(n, _)| *n == moment).unwrap();
            v.get(guide::BOX).unwrap().text.clone()
        };
        assert_eq!(label("16-guide-approach"), "13%");
        assert_eq!(label("17-guide-lined-up"), "62%");
        assert_eq!(label("18-guide-falling-caught"), "Will bounce 82%");
        assert_eq!(label("19-guide-too-shallow"), "Too shallow 30%");
        assert_eq!(label("20-guide-off-line"), "5%");
        assert_eq!(label("26-guide-on-the-cut"), "Will bounce 52%");
        assert_eq!(label("27-guide-scan-only"), "Will bounce 60%");
        for (name, v) in All.states() {
            let poses = All.world(name);
            for id in guide::MARKS {
                let shown = v.get(id).unwrap().shown;
                assert_eq!(shown, poses.iter().any(|(i, _)| *i == id), "{name} {id}");
            }
        }
    }

    /// Each moment's charge segments are its superbounce state's charge:
    /// the ring fills with the recipe, is full primed, wipes as the
    /// charge drains and is empty after a loss.
    #[test]
    fn the_segments_are_the_superbounce_states_charge() {
        use segs::Stage;
        let stage = |moment: &str| {
            let (_, a, _) = MOMENTS.iter().find(|(n, _, _)| *n == moment).unwrap();
            moment_segments(moment, a).map(|s| (s.stage, s.lit))
        };
        assert_eq!(stage("1-walking"), Some((Stage::Idle, 0.0)));
        assert_eq!(stage("21-priming-near"), Some((Stage::Near, 0.0)));
        assert_eq!(stage("2-arming"), Some((Stage::Under, 3.0)));
        assert_eq!(stage("22-priming-held"), Some((Stage::Held, 6.0)));
        // The wipe-back: partly lit, over the warning's count, under full.
        let (st, lit) = stage("23-primed-ending").unwrap();
        assert_eq!(st, Stage::Primed);
        assert!((6.0..6.1).contains(&lit), "{lit}");
        assert!(lit < 9.0 && lit > 9.0 * crate::huds::superbounce::LOW_CHARGE);
        assert_eq!(stage("3-primed-targets"), Some((Stage::Primed, 9.0)));
        assert_eq!(stage("5-charge-draining"), Some((Stage::Warning, 2.25)));
        assert_eq!(stage("24-charge-warning"), Some((Stage::Warning, 4.5)));
        assert_eq!(stage("25-charge-lost"), Some((Stage::Lost, 0.0)));
        assert_eq!(segments_of("scanning"), None);
        assert_eq!(segments_of("no-strips"), None);
        // The values carry them.
        let states = All.states();
        let lit = |moment: &str| {
            let (_, v) = states.iter().find(|(n, _)| *n == moment).unwrap();
            segs::IDS
                .iter()
                .filter(|id| v.get(**id).is_some_and(|x| x.shown && x.fill == 1.0))
                .count()
        };
        assert_eq!(
            [
                "1-walking",
                "2-arming",
                "22-priming-held",
                "23-primed-ending",
                "3-primed-targets",
                "25-charge-lost"
            ]
            .map(lit),
            [0, 3, 6, 6, 9, 0]
        );
    }

    /// The combined HUD is fullscreen only, as the readout is: no element
    /// has a half or quarter place, so none of the block is drawn without
    /// the rest in a split frame; fullscreen each is where its own HUD
    /// puts it.
    #[test]
    fn the_combined_hud_is_fullscreen_only() {
        let own: Vec<_> = Superbounce
            .hud()
            .elements
            .into_iter()
            .chain(Readout.hud().elements)
            .collect();
        let all = All.hud();
        assert_eq!(all.elements.len(), own.len());
        for (e, o) in all.elements.iter().zip(&own) {
            assert_eq!((e.id, e.at.full, e.at.anchor), (o.id, o.at.full, o.at.anchor));
            assert_eq!((e.at.half, e.at.quarter, e.at.split_origin), (None, None, None), "{}", e.id);
        }
        assert!(own.iter().any(|o| o.at.half.is_some()));
        assert!(!all.uses_grenade_row());
    }
}
