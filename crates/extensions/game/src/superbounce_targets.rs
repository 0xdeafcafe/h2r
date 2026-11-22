//! Seam targets (docs/game/superbounce.md, "Targets"): which seams of the
//! scan get one of the game's navpoints, and the one the player tagged.
//!
//! - Every scanned bounce spot has one [`Class`], from what is known
//!   about it: proven by a game run, closed on by the sim's training,
//!   the scan's calibrated shape, or a shape nothing has tried.
//! - The script's `targets` option
//!   ([`crate::superbounce_options::TargetMode`]) picks the classes shown
//!   ([`in_mode`]).
//! - Of those, the two nearest the player can reach from their height
//!   ([`reachable`]) are marked, and the tagged one wherever it is
//!   ([`choose`]).
//! - Holding the flashlight while aiming at a target tags it; holding on
//!   the tagged one clears it ([`Tagger`], [`aimed`]).
//! - Two taps of the flashlight switch the classes shown, for this game:
//!   common, likely, all and round again, from the option's value
//!   ([`Switch`]).
//!
//! The marks are drawn by Halo 2's own HUD: the HUD kit's navpoints of
//! the superbounce readout ([`crate::superbounce_readout`]). This module
//! only says where they go ([`Marks`]).

use std::sync::{Arc, Mutex};

use superbounce::learned::{Entry, State as Line};
use superbounce::{Kind, Spot, Status, Vec3};

use crate::superbounce_charge::{NavKind, NavTarget};
use crate::superbounce_options::TargetMode;
use crate::superbounce_view::{Button, EYE, Press};

/// What is known about a seam, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// Proven: a game run bounced on it (the map's verified file), or it
    /// is a measured place (`superbounce::Status::Proven`).
    Known,
    /// The sim's training closed on it: a `learned` line with a hit.
    Learned,
    /// A `partial` line with a hit: it bounced, and the budget ran out.
    Partial,
    /// The scan's two-joint slot, the shape measured on Ascension.
    Slot,
    /// Scan only: a single crease, a V's arm or tip, a busy point.
    Candidate,
}

impl Class {
    /// The class's name, as `superbounce.state` writes it.
    pub fn name(self) -> &'static str {
        match self {
            Class::Known => "known",
            Class::Learned => "learned",
            Class::Partial => "partial",
            Class::Slot => "slot",
            Class::Candidate => "candidate",
        }
    }
}

/// A learned line's aim is its spot's stand within this (world units):
/// `superbounce::learned::apply` moves the stand onto the aim exactly, and
/// the file keeps three decimals.
pub const AIM_NEAR: f32 = 0.01;

/// The learned line that is about `spot`: a `learned` or `partial` line
/// with a hit, never a control line, whose aim is the spot's stand (the
/// stand `learned::apply` gave it) or, for a line with no aim, whose
/// site is.
fn line_of<'a>(spot: &Spot, lines: &'a [Entry]) -> Option<&'a Entry> {
    lines
        .iter()
        .filter(|e| !e.control && e.hits > 0 && matches!(e.state, Line::Learned | Line::Partial))
        .find(|e| match e.knobs.aim {
            Some(aim) => {
                (aim[0] - spot.stand[0]).hypot(aim[1] - spot.stand[1]) <= AIM_NEAR
                    && (e.at[2] - spot.stand[2]).abs() <= 1.0
            }
            None => e.is_of(spot.stand),
        })
}

/// `spot`'s class with the map's learned `lines`; `None` for a crouch
/// spot and for a seam a game run failed (never a target).
pub fn class(spot: &Spot, lines: &[Entry]) -> Option<Class> {
    if spot.kind != Kind::Bounce || spot.status == Status::Failed {
        return None;
    }
    if spot.status == Status::Proven {
        return Some(Class::Known);
    }
    match line_of(spot, lines).map(|e| e.state) {
        Some(Line::Learned) => Some(Class::Learned),
        Some(_) => Some(Class::Partial),
        None if spot.candidate() => Some(Class::Candidate),
        None => Some(Class::Slot),
    }
}

/// Whether `mode` shows a seam of `class`: `common` the known ones,
/// `likely` those and the learned, the partial and the slots, `all` every
/// class.
pub fn in_mode(mode: TargetMode, class: Class) -> bool {
    match mode {
        TargetMode::Common => class == Class::Known,
        TargetMode::Likely => class != Class::Candidate,
        TargetMode::All => true,
    }
}

/// A seam a mode shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub spot: Spot,
    pub class: Class,
}

impl Target {
    /// Where its navpoint points: the spot's aim, on the seam.
    pub fn at(&self) -> Vec3 {
        self.spot.stand
    }
}

/// The seams of `all` (the whole scan, the map's files applied) that
/// `mode` shows, in the scan's order.
pub fn targets(all: &[Spot], lines: &[Entry], mode: TargetMode) -> Vec<Target> {
    all.iter()
        .filter_map(|s| Some((s, class(s, lines)?)))
        .filter(|(_, c)| in_mode(mode, *c))
        .map(|(s, c)| Target { spot: *s, class: c })
        .collect()
}

/// Whether `t` is one a player at `at` is given: a V's arm only from
/// across its bisector, as the mode's own marks
/// (`superbounce_mode::for_player`).
pub fn faces(t: &Target, at: Vec3) -> bool {
    t.spot.arm.is_none_or(|a| a.approached_from([at[0], at[1]]))
}

/// Feet this far (world units) under the height a seam's fall needs still
/// reach it, as `superbounce_charge::HIGH_SLACK`.
pub const REACH_SLACK: f32 = crate::superbounce_charge::HIGH_SLACK;

/// Whether feet at height `feet` can fall onto `spot` fast enough: they
/// are at least its `fall` above its stand, less [`REACH_SLACK`]. A seam
/// that needs no fall is reached from its own height or above. Only
/// height is judged, not the way there.
pub fn reachable(feet: f32, spot: &Spot) -> bool {
    let fall = if spot.fall.is_finite() {
        spot.fall.max(0.0)
    } else {
        0.0
    };
    feet - spot.stand[2] >= fall - REACH_SLACK
}

/// Two points are the same seam's within this (world units).
pub const SAME: f32 = 0.02;

fn same(a: Vec3, b: Vec3) -> bool {
    distance(a, b) <= SAME
}

fn distance(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// A navpoint: where, and how far from the player (world units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub at: Vec3,
    pub distance: f32,
}

/// How many reachable seams are marked.
pub const NEAREST: usize = 2;

/// The navpoints of one tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Marks {
    /// The tagged seam, reachable or not; `None` with no tag, or while
    /// the mode's own navpoint is on it.
    pub tag: Option<Mark>,
    /// The nearest reachable seams, nearest first.
    pub targets: [Option<Mark>; NEAREST],
    /// The tag itself, marked or not.
    pub tagged: Option<Vec3>,
}

impl Marks {
    pub const NONE: Marks = Marks {
        tag: None,
        targets: [None; NEAREST],
        tagged: None,
    };
}

/// The marks for a player whose feet are at `at`: the `tag`, and the
/// [`NEAREST`] reachable seams of `targets` nearest them, the tagged one
/// and the one at `own` (where the mode's own navpoint is drawn) left
/// out. A V's arm counts only from across its bisector ([`faces`]).
pub fn choose(targets: &[Target], at: Vec3, tag: Option<Vec3>, own: Option<Vec3>) -> Marks {
    let taken = |p: Vec3| tag.is_some_and(|t| same(t, p)) || own.is_some_and(|o| same(o, p));
    let mut near: Vec<Mark> = targets
        .iter()
        .filter(|t| faces(t, at) && reachable(at[2], &t.spot) && !taken(t.at()))
        .map(|t| Mark {
            at: t.at(),
            distance: distance(t.at(), at),
        })
        .collect();
    near.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    let mut out = Marks {
        tag: tag
            .filter(|t| !own.is_some_and(|o| same(o, *t)))
            .map(|t| Mark {
                at: t,
                distance: distance(t, at),
            }),
        tagged: tag,
        ..Marks::NONE
    };
    for (slot, m) in out.targets.iter_mut().zip(near) {
        *slot = Some(m);
    }
    out
}

/// A target is aimed at within this angle of the aim (6 degrees), or
/// within the angle [`AIM_SPAN`] spans at its distance when that is
/// wider (a near seam).
pub const AIM_CONE: f32 = 6.0 * std::f32::consts::PI / 180.0;
/// World units either side of a target that count as on it.
pub const AIM_SPAN: f32 = 0.4;
/// A target nearer the feet than this (world units) is underfoot, not
/// aimed at.
pub const AIM_MIN: f32 = 0.5;

/// The target of `targets` the player aims at: from the eye [`EYE`] over
/// `at`, along the unit vector `aim`, the one nearest the aim's line
/// among those within the cone ([`AIM_CONE`], [`AIM_SPAN`]). A V's arm
/// counts only from across its bisector. The index in `targets`.
pub fn aimed(at: Vec3, aim: Vec3, targets: &[Target]) -> Option<usize> {
    let len = (aim[0] * aim[0] + aim[1] * aim[1] + aim[2] * aim[2]).sqrt();
    if !len.is_finite() || len < 0.5 {
        return None;
    }
    let eye = [at[0], at[1], at[2] + EYE];
    targets
        .iter()
        .enumerate()
        .filter(|(_, t)| faces(t, at))
        .filter_map(|(i, t)| {
            let p = t.at();
            let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
            if distance(p, at) < AIM_MIN {
                return None;
            }
            let far = distance(p, eye);
            let cos = (d[0] * aim[0] + d[1] * aim[1] + d[2] * aim[2]) / (far * len);
            let off = cos.clamp(-1.0, 1.0).acos();
            (off <= AIM_CONE.max((AIM_SPAN / far).atan())).then_some((i, off))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The tag, and the flashlight's button that sets it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tagger {
    button: Button,
    tag: Option<Vec3>,
}

impl Tagger {
    pub const fn new() -> Tagger {
        Tagger {
            button: Button::UP,
            tag: None,
        }
    }

    /// The tagged seam's point.
    pub fn tag(&self) -> Option<Vec3> {
        self.tag
    }

    /// One tick: `down` is the flashlight action. As the press becomes a
    /// hold (`superbounce_view::TAP_TICKS`), the target aimed at
    /// ([`aimed`]) is tagged, or untagged when it is the tagged one; a
    /// hold aimed at none changes nothing. The tag after the tick.
    pub fn step(
        &mut self,
        down: bool,
        at: Option<Vec3>,
        aim: Option<Vec3>,
        targets: &[Target],
    ) -> Option<Vec3> {
        if self.button.step(down) == Some(Press::HoldStart)
            && let (Some(at), Some(aim)) = (at, aim)
            && let Some(i) = aimed(at, aim, targets)
        {
            let p = targets[i].at();
            self.tag = if self.tag.is_some_and(|t| same(t, p)) {
                None
            } else {
                Some(p)
            };
        }
        self.tag
    }
}

/// Two taps of the flashlight let go within this many ticks of each
/// other are a double tap: 12 ticks is 0.4 s.
pub const DOUBLE_TICKS: u32 = 12;
/// Ticks the readout's line says which kinds the player switched to: two
/// seconds.
pub const SWITCHED_TICKS: u32 = 60;

/// The player's switch of the kinds of seam shown, and the flashlight's
/// double tap that makes it. The flashlight is the mode's one action: the
/// game binds it on every input set and layout, a multiplayer biped's
/// light is back as it was after two taps, a tap is the bounce view's
/// pulse and a hold its marks and the tag, so a double tap takes nothing
/// from play. The switch lasts the game: a new one starts from the
/// variant's `targets` option.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Switch {
    button: Button,
    /// Ticks since a tap that a second can pair with.
    since: Option<u32>,
    pick: Option<TargetMode>,
    shown: u32,
}

impl Switch {
    pub const fn new() -> Switch {
        Switch {
            button: Button::UP,
            since: None,
            pick: None,
            shown: 0,
        }
    }

    /// One tick: `down` is the flashlight action, `from` the variant's
    /// mode. A double tap moves the mode on ([`TargetMode::next`]), from
    /// the player's last pick or from `from`. The mode in effect after
    /// the tick.
    pub fn step(&mut self, down: bool, from: TargetMode) -> TargetMode {
        self.shown = self.shown.saturating_sub(1);
        if let Some(n) = self.since {
            self.since = (n < DOUBLE_TICKS).then_some(n + 1);
        }
        match self.button.step(down) {
            Some(Press::Tap) => {
                if self.since.take().is_some() {
                    self.pick = Some(self.pick.unwrap_or(from).next());
                    self.shown = SWITCHED_TICKS;
                } else {
                    self.since = Some(0);
                }
            }
            // A hold is the tag's and the bounce view's.
            Some(Press::HoldStart | Press::HoldEnd) => self.since = None,
            None => {}
        }
        self.pick.unwrap_or(from)
    }

    /// The mode the player switched to, for [`SWITCHED_TICKS`] after the
    /// double tap: the readout's line.
    pub fn switched(&self) -> Option<TargetMode> {
        self.pick.filter(|_| self.shown > 0)
    }
}

// ---- the running game ----

/// The chosen strip with the tag: while the mode's navpoint is on a strip
/// (`nav`, primed) and the player is not falling onto one (`falling`), a
/// tagged seam that is one of the mode's marked `spots` is the chosen
/// strip, so the navpoint, the take-off and the assist follow the tag.
/// Otherwise `nav` as it is.
pub fn chosen(
    nav: Option<NavTarget>,
    falling: bool,
    tag: Option<Vec3>,
    spots: &[Spot],
    at: Option<Vec3>,
) -> Option<NavTarget> {
    let strip = nav.is_some_and(|n| n.kind == NavKind::Strip);
    let tagged = tag.filter(|_| strip && !falling).and_then(|t| {
        spots
            .iter()
            .find(|s| s.kind == Kind::Bounce && same(s.stand, t))
    });
    match (tagged, at) {
        (Some(s), Some(at)) => Some(NavTarget {
            at: s.stand,
            kind: NavKind::Strip,
            distance: distance(s.stand, at),
            fall: s.fall,
        }),
        _ => nav,
    }
}

/// What one tick gives [`step`].
pub struct Input<'a> {
    pub map: Option<&'a str>,
    /// The whole scan, the map's files applied
    /// (`superbounce_mode::scanned_spots`).
    pub scanned: &'a Arc<Vec<Spot>>,
    /// The variant's `targets` option: the mode until the player
    /// switches it ([`Switch`]).
    pub mode: TargetMode,
    pub position: Option<Vec3>,
    pub aim: Option<Vec3>,
    /// The flashlight action.
    pub down: bool,
}

/// The classed list, kept while the scan, the map's files and the mode
/// stay the same.
struct Kept {
    scanned: Arc<Vec<Spot>>,
    generation: u64,
    mode: TargetMode,
    targets: Arc<Vec<Target>>,
}

struct Live {
    tagger: Tagger,
    switch: Switch,
    kept: Option<Kept>,
}

static LIVE: Mutex<Live> = Mutex::new(Live {
    tagger: Tagger::new(),
    switch: Switch::new(),
    kept: None,
});

fn live() -> std::sync::MutexGuard<'static, Live> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The mode's tick: steps the player's switch of the kinds shown, classes
/// the scan (once per scan, file generation and mode) and steps the tag.
/// The tag after the tick.
pub fn step(i: &Input<'_>) -> Option<Vec3> {
    let mut l = live();
    let mode = l.switch.step(i.down, i.mode);
    let generation = bots::spots::generation();
    let fresh = l.kept.as_ref().is_some_and(|k| {
        Arc::ptr_eq(&k.scanned, i.scanned) && k.generation == generation && k.mode == mode
    });
    if !fresh {
        let lines = i.map.map(bots::spots::learned_for).unwrap_or_default();
        l.kept = Some(Kept {
            scanned: Arc::clone(i.scanned),
            generation,
            mode,
            targets: Arc::new(targets(i.scanned, &lines, mode)),
        });
    }
    let list = l
        .kept
        .as_ref()
        .map(|k| Arc::clone(&k.targets))
        .unwrap_or_default();
    l.tagger.step(i.down, i.position, i.aim, &list)
}

/// The tick's marks for a player at `position` with the tag `tag`, the
/// mode's own navpoint drawn at `own` ([`choose`], over the list [`step`]
/// classed). Without a position there are none; the tag is kept.
pub fn marks(position: Option<Vec3>, tag: Option<Vec3>, own: Option<Vec3>) -> Marks {
    match position {
        Some(at) => choose(&current(), at, tag, own),
        None => Marks {
            tagged: tag,
            ..Marks::NONE
        },
    }
}

/// The targets the mode shows now, as the last tick classed them.
pub fn current() -> Arc<Vec<Target>> {
    live()
        .kept
        .as_ref()
        .map(|k| Arc::clone(&k.targets))
        .unwrap_or_default()
}

/// The kinds of seam shown now: the variant's option, or the player's
/// switch of it. `None` before the mode's first tick.
pub fn mode() -> Option<TargetMode> {
    live().kept.as_ref().map(|k| k.mode)
}

/// The mode the player has just switched to ([`Switch::switched`]).
pub fn switched() -> Option<TargetMode> {
    live().switch.switched()
}

/// Forgets the tag, the switch and the list: a new game, or the mode off.
pub fn reset() {
    let mut l = live();
    l.tagger = Tagger::new();
    l.switch = Switch::new();
    l.kept = None;
}

#[cfg(test)]
mod tests {
    use superbounce::learned::Knobs;
    use superbounce::{Arm, Shape};

    use super::*;
    use crate::superbounce_view::TAP_TICKS;

    fn seam(stand: Vec3) -> Spot {
        Spot {
            kind: Kind::Bounce,
            edge: [
                [stand[0] - 0.5, stand[1], stand[2]],
                [stand[0] + 0.5, stand[1], stand[2]],
            ],
            stand,
            fall: 2.43,
            ..Spot::default()
        }
    }

    fn line(at: Vec3, state: Line, hits: u32, aim: Option<[f32; 2]>) -> Entry {
        Entry {
            state,
            at,
            hits,
            tries: 10,
            knobs: Knobs {
                aim,
                ..Knobs::default()
            },
            ..Entry::default()
        }
    }

    fn target(stand: Vec3) -> Target {
        Target {
            spot: seam(stand),
            class: Class::Slot,
        }
    }

    #[test]
    fn a_seam_takes_the_best_class_its_data_gives() {
        let at = [4.0, 5.0, 1.0];
        let slot = seam(at);
        assert_eq!(class(&slot, &[]), Some(Class::Slot));
        let crease = Spot {
            shape: Shape::Crease,
            ..slot
        };
        assert_eq!(class(&crease, &[]), Some(Class::Candidate));
        // A proven crease is known, whatever its shape or its line.
        let proven = Spot {
            status: Status::Proven,
            ..crease
        };
        let learned = [line(at, Line::Learned, 6, None)];
        assert_eq!(class(&proven, &learned), Some(Class::Known));
        assert_eq!(class(&crease, &learned), Some(Class::Learned));
        assert_eq!(
            class(&slot, &[line(at, Line::Partial, 1, None)]),
            Some(Class::Partial)
        );
        // No hit, an open or dead line, or a control line says nothing.
        for l in [
            line(at, Line::Learned, 0, None),
            line(at, Line::Open, 3, None),
            line(at, Line::Dead, 0, None),
            Entry {
                control: true,
                ..line(at, Line::Learned, 6, None)
            },
        ] {
            let what = format!("{l:?}");
            assert_eq!(class(&crease, &[l]), Some(Class::Candidate), "{what}");
        }
        // A failed seam and a crouch spot are never targets.
        let failed = Spot {
            status: Status::Failed,
            ..slot
        };
        assert_eq!(class(&failed, &learned), None);
        let crouch = Spot {
            kind: Kind::Crouch,
            ..slot
        };
        assert_eq!(class(&crouch, &learned), None);
    }

    /// `learned::apply` moves a spot's stand onto its line's aim: the
    /// line is still found, by the aim, and a neighbour within the
    /// site's reach that did not take the aim is not.
    #[test]
    fn a_line_with_an_aim_is_its_retargeted_spots_only() {
        let site = [0.0, 3.88, 1.0];
        let aim = [0.0, 3.985];
        let l = [line(site, Line::Learned, 5, Some(aim))];
        let mut spots = [
            seam([0.0, 4.0, 1.0]),
            Spot {
                shape: Shape::Crease,
                ..seam([0.0, 3.8, 1.0])
            },
        ];
        superbounce::learned::apply(&mut spots, &l);
        let classes: Vec<_> = spots.iter().map(|s| class(s, &l)).collect();
        assert!(classes.contains(&Some(Class::Learned)), "{classes:?}");
        assert_eq!(
            classes
                .iter()
                .filter(|c| **c == Some(Class::Learned))
                .count(),
            1,
            "{spots:?}"
        );
    }

    #[test]
    fn each_mode_holds_the_one_before_it() {
        let classes = [
            Class::Known,
            Class::Learned,
            Class::Partial,
            Class::Slot,
            Class::Candidate,
        ];
        let set = |m| classes.iter().filter(|c| in_mode(m, **c)).count();
        assert_eq!(set(TargetMode::Common), 1);
        assert_eq!(set(TargetMode::Likely), 4);
        assert_eq!(set(TargetMode::All), 5);
        for c in classes {
            assert!(!in_mode(TargetMode::Common, c) || in_mode(TargetMode::Likely, c));
            assert!(!in_mode(TargetMode::Likely, c) || in_mode(TargetMode::All, c));
        }
        assert!(in_mode(TargetMode::Common, Class::Known));
        assert!(!in_mode(TargetMode::Likely, Class::Candidate));
    }

    #[test]
    fn targets_are_the_modes_seams_never_a_failed_one() {
        let known = Spot {
            status: Status::Proven,
            ..seam([0.0, 0.0, 0.0])
        };
        let slot = seam([5.0, 0.0, 0.0]);
        let crease = Spot {
            shape: Shape::Crease,
            ..seam([10.0, 0.0, 0.0])
        };
        let failed = Spot {
            status: Status::Failed,
            ..seam([15.0, 0.0, 0.0])
        };
        let crouch = Spot {
            kind: Kind::Crouch,
            ..seam([20.0, 0.0, 0.0])
        };
        let all = [known, slot, crease, failed, crouch];
        let at = |m| -> Vec<f32> { targets(&all, &[], m).iter().map(|t| t.at()[0]).collect() };
        assert_eq!(at(TargetMode::Common), [0.0]);
        assert_eq!(at(TargetMode::Likely), [0.0, 5.0]);
        assert_eq!(at(TargetMode::All), [0.0, 5.0, 10.0]);
    }

    #[test]
    fn a_seam_is_reached_from_its_fall_above_it() {
        let s = seam([0.0, 0.0, 1.0]);
        assert!(reachable(1.0 + 2.43, &s));
        assert!(reachable(1.0 + 2.43 - 0.04, &s));
        assert!(!reachable(1.0 + 2.43 - 0.06, &s));
        assert!(!reachable(1.0, &s));
        assert!(reachable(9.0, &s));
        // No fall: from its own height, not from under it.
        let flat = Spot { fall: 0.0, ..s };
        assert!(reachable(1.0, &flat));
        assert!(!reachable(0.5, &flat));
        let unknown = Spot {
            fall: f32::NAN,
            ..s
        };
        assert!(reachable(1.0, &unknown));
    }

    #[test]
    fn the_two_nearest_reachable_seams_are_marked() {
        let list = [
            target([30.0, 0.0, 0.0]),
            target([10.0, 0.0, 0.0]),
            target([20.0, 0.0, 0.0]),
            // Nearest of all, and too high to fall onto.
            target([1.0, 0.0, 4.0]),
        ];
        let at = [0.0, 0.0, 3.0];
        let m = choose(&list, at, None, None);
        let xs = |m: &Marks| m.targets.map(|t| t.map(|t| t.at[0]));
        assert_eq!(xs(&m), [Some(10.0), Some(20.0)]);
        assert_eq!(m.tag, None);
        assert!((m.targets[0].unwrap().distance - (100.0f32 + 9.0).sqrt()).abs() < 1e-4);
        // From the floor none is reachable: no mark.
        assert_eq!(
            xs(&choose(&list, [0.0, 0.0, 0.0], None, None)),
            [None, None]
        );
        // One reachable seam leaves the second slot empty.
        assert_eq!(xs(&choose(&list[..1], at, None, None)), [Some(30.0), None]);
    }

    #[test]
    fn the_tag_is_marked_wherever_it_is_and_not_twice() {
        let list = [
            target([10.0, 0.0, 0.0]),
            target([20.0, 0.0, 0.0]),
            target([30.0, 0.0, 0.0]),
        ];
        let xs = |m: &Marks| m.targets.map(|t| t.map(|t| t.at[0]));
        // Tagged and reachable: the tag's navpoint, and the next two.
        let m = choose(&list, [0.0, 0.0, 3.0], Some([10.0, 0.0, 0.0]), None);
        assert_eq!(m.tag.map(|t| t.at[0]), Some(10.0));
        assert_eq!(xs(&m), [Some(20.0), Some(30.0)]);
        // Tagged and out of reach: still marked.
        let m = choose(&list, [0.0, 0.0, 0.0], Some([10.0, 0.0, 0.0]), None);
        assert_eq!(m.tag.map(|t| t.distance), Some(10.0));
        assert_eq!(xs(&m), [None, None]);
        // The mode's own navpoint on a seam: no second one there, tag or
        // target; the tag is still the tag.
        let own = Some([10.0, 0.0, 0.0]);
        let m = choose(&list, [0.0, 0.0, 3.0], own, own);
        assert_eq!(m.tag, None);
        assert_eq!(m.tagged, own);
        assert_eq!(xs(&m), [Some(20.0), Some(30.0)]);
    }

    #[test]
    fn a_vs_arm_is_a_target_only_from_across_its_bisector() {
        let arm = Target {
            spot: Spot {
                arm: Some(Arm {
                    pivot: [10.0, 0.0, 0.0],
                    bisector: [1.0, 0.0],
                    from: [0.0, -1.0],
                }),
                ..seam([10.0, 0.5, 0.0])
            },
            class: Class::Known,
        };
        let across = [8.0, -3.0, 3.0];
        let own_side = [8.0, 3.0, 3.0];
        assert!(choose(&[arm], across, None, None).targets[0].is_some());
        assert!(choose(&[arm], own_side, None, None).targets[0].is_none());
        let toward = |from: Vec3| {
            let p = arm.at();
            let d = [p[0] - from[0], p[1] - from[1], p[2] - from[2] - EYE];
            let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            d.map(|v| v / n)
        };
        assert_eq!(aimed(across, toward(across), &[arm]), Some(0));
        assert_eq!(aimed(own_side, toward(own_side), &[arm]), None);
    }

    #[test]
    fn the_target_nearest_the_aim_within_the_cone_is_aimed_at() {
        let at = [0.0, 0.0, -EYE];
        // Two targets 20 away, 3 and 5 degrees off the aim along +X, and
        // one 10 degrees off.
        let off = |deg: f32| {
            let a = deg.to_radians();
            target([20.0 * a.cos(), 20.0 * a.sin(), 0.0])
        };
        let list = [off(5.0), off(-3.0), off(10.0)];
        assert_eq!(aimed(at, [1.0, 0.0, 0.0], &list), Some(1));
        assert_eq!(aimed(at, [1.0, 0.0, 0.0], &list[2..]), None);
        // Behind the player: none.
        assert_eq!(aimed(at, [-1.0, 0.0, 0.0], &list), None);
        // A near seam is wider than the cone: 0.4 units at 2 is 11
        // degrees.
        let near = [target([2.0, 0.35, 0.0])];
        assert_eq!(aimed(at, [1.0, 0.0, 0.0], &near), Some(0));
        let wide = [target([2.0, 0.6, 0.0])];
        assert_eq!(aimed(at, [1.0, 0.0, 0.0], &wide), None);
        // Underfoot, or with no aim: none.
        assert_eq!(
            aimed(at, [0.0, 0.0, -1.0], &[target([0.0, 0.0, -EYE - 0.2])]),
            None
        );
        assert_eq!(aimed(at, [0.0, 0.0, 0.0], &list), None);
        assert_eq!(aimed(at, [f32::NAN, 0.0, 0.0], &list), None);
    }

    /// Two taps of the flashlight within [`DOUBLE_TICKS`] move the mode
    /// on from the variant's, round the cycle; one tap, two far apart and
    /// a hold do not. The line is up [`SWITCHED_TICKS`].
    #[test]
    fn a_double_tap_switches_the_kinds_shown() {
        let from = TargetMode::Likely;
        let tap = |s: &mut Switch, gap: u32| {
            for _ in 0..gap {
                s.step(false, from);
            }
            s.step(true, from);
            s.step(true, from);
            s.step(false, from)
        };
        let mut s = Switch::new();
        assert_eq!(s.step(false, from), TargetMode::Likely);
        assert_eq!(tap(&mut s, 0), TargetMode::Likely, "one tap");
        assert_eq!(s.switched(), None);
        assert_eq!(tap(&mut s, 3), TargetMode::All, "the second, soon after");
        assert_eq!(s.switched(), Some(TargetMode::All));
        // The pair is spent: a third tap starts a new one.
        assert_eq!(tap(&mut s, 3), TargetMode::All);
        assert_eq!(tap(&mut s, 3), TargetMode::Common);
        assert_eq!(tap(&mut s, 20), TargetMode::Common);
        assert_eq!(tap(&mut s, 3), TargetMode::Likely);
        // The pick stays whatever the variant says; the line goes.
        assert_eq!(s.step(false, TargetMode::All), TargetMode::Likely);
        for _ in 0..SWITCHED_TICKS {
            s.step(false, from);
        }
        assert_eq!(s.switched(), None);
        // Two taps too far apart are two single taps.
        let mut s = Switch::new();
        tap(&mut s, 0);
        assert_eq!(tap(&mut s, DOUBLE_TICKS), TargetMode::Likely);
        // A hold between two taps breaks the pair, and is no tap itself.
        let mut s = Switch::new();
        tap(&mut s, 0);
        for _ in 0..TAP_TICKS {
            s.step(true, from);
        }
        s.step(false, from);
        assert_eq!(tap(&mut s, 0), TargetMode::Likely);
        // Unswitched, the mode is the variant's.
        assert_eq!(Switch::new().step(false, TargetMode::Common), TargetMode::Common);
        assert_eq!(
            [TargetMode::Common, TargetMode::Likely, TargetMode::All].map(TargetMode::next),
            [TargetMode::Likely, TargetMode::All, TargetMode::Common]
        );
    }

    #[test]
    fn a_hold_on_a_target_tags_it_and_a_hold_on_the_tag_clears_it() {
        let at = Some([0.0, 0.0, -EYE]);
        let ahead = Some([1.0, 0.0, 0.0]);
        let left = Some([0.0, 1.0, 0.0]);
        let list = [target([20.0, 0.0, 0.0]), target([0.0, 20.0, 0.0])];
        let hold = |t: &mut Tagger, aim| {
            let mut tag = None;
            for _ in 0..TAP_TICKS {
                tag = t.step(true, at, aim, &list);
            }
            t.step(false, at, aim, &list);
            tag
        };
        let mut t = Tagger::new();
        // A tap tags nothing.
        for _ in 0..TAP_TICKS - 1 {
            assert_eq!(t.step(true, at, ahead, &list), None);
        }
        assert_eq!(t.step(false, at, ahead, &list), None);
        assert_eq!(hold(&mut t, ahead), Some([20.0, 0.0, 0.0]));
        // Kept while the button is up, and through a hold aimed at none.
        assert_eq!(t.step(false, at, ahead, &list), Some([20.0, 0.0, 0.0]));
        assert_eq!(hold(&mut t, Some([0.0, 0.0, 1.0])), Some([20.0, 0.0, 0.0]));
        // A hold on another moves it; on the tagged one, clears it.
        assert_eq!(hold(&mut t, left), Some([0.0, 20.0, 0.0]));
        assert_eq!(hold(&mut t, left), None);
        assert_eq!(t.tag(), None);
        // One tag a hold, however long it lasts.
        let mut t = Tagger::new();
        for _ in 0..TAP_TICKS * 4 {
            t.step(true, at, ahead, &list);
        }
        assert_eq!(t.tag(), Some([20.0, 0.0, 0.0]));
    }

    #[test]
    fn a_tagged_marked_seam_is_the_chosen_strip_while_primed_on_the_ground() {
        let near = seam([5.0, 0.0, 0.0]);
        let far = seam([40.0, 0.0, 0.0]);
        let spots = [near, far];
        let at = Some([0.0, 0.0, 3.0]);
        let strip = Some(NavTarget {
            at: near.stand,
            kind: NavKind::Strip,
            distance: 5.0,
            fall: near.fall,
        });
        let tag = Some(far.stand);
        let got = chosen(strip, false, tag, &spots, at).unwrap();
        assert_eq!(
            (got.at, got.kind, got.fall),
            (far.stand, NavKind::Strip, far.fall)
        );
        assert!((got.distance - (1600.0f32 + 9.0).sqrt()).abs() < 1e-3);
        // Falling onto a strip, that strip stays the target.
        assert_eq!(chosen(strip, true, tag, &spots, at), strip);
        // Not primed, the navpoint is the crouch spot's.
        let crouch = Some(NavTarget {
            at: [1.0, 1.0, 0.0],
            kind: NavKind::Crouch,
            distance: 1.0,
            fall: 0.0,
        });
        assert_eq!(chosen(crouch, false, tag, &spots, at), crouch);
        assert_eq!(chosen(None, false, tag, &spots, at), None);
        // A tag on a seam the mode does not mark, or no tag: as it was.
        assert_eq!(
            chosen(strip, false, Some([9.0, 9.0, 9.0]), &spots, at),
            strip
        );
        assert_eq!(chosen(strip, false, None, &spots, at), strip);
    }
}
