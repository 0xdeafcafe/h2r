//! The superbounce readout (docs/game/hud-kit.md, "The superbounce
//! readout"): three rows right of the motion tracker, the charge
//! segments round it, and three navpoints, all drawn by Halo 2's own
//! HUD.
//!
//! - `sbr.seg.1` to `sbr.seg.9`: the charge segments round the motion
//!   tracker, the priming indicator ([`super::superbounce_segments`]).
//! - `sbr.speed`: an arrow for the way the player is moving against the
//!   aim and the horizontal speed, and in the air the fall speed after
//!   it. Hidden standing still.
//! - `sbr.fall` and `sbr.steer`, on a fall toward a seam: a bar that
//!   fills with the touchdown speed over the speed the seam needs, and
//!   beside it `On seam`, or an arrow for the way to push and how far the
//!   landing is off the seam, and after either the approach guide's
//!   predicted percentage ([`steer_line`]).
//! - `sbr.info`: why the last landing by a seam did not bounce, for a few
//!   seconds ([`Miss`]); in the flight after a bounce, the seams it can
//!   reach, the best first, each a rank, an arrow for where it is against
//!   the aim and its distance ([`Next`]).
//! - `sbr.tag`, `sbr.target.1` and `sbr.target.2`: the game's navpoint
//!   over the tagged seam and over the two nearest seams the player can
//!   reach.
//!
//! - `sbr.guide.*`: the approach guide, four world marks on the seam the
//!   player is going for ([`super::superbounce_guide`]).
//!
//! Nothing is in the middle of the screen: the rows are bottom left, and
//! a navpoint or a world mark is where its seam is. The readout is fullscreen only. A
//! half or quarter frame has no three rows free under the prompt line's
//! box, so it is a HUD of its own beside the superbounce HUD
//! ([`super::superbounce`]), whose widgets show in every screen mode.
//!
//! [`View`] is what it shows; the game's runtime
//! (`game::superbounce_readout`) fills it from `superbounce_mode`'s state
//! each tick, with the pieces a variant turned off left out.

use crate::model::{Anchor, Art, Describe, Element, Font, Hud, Justify, Kind, Place, Value, Values};

use super::superbounce::{Charge, DISTANCE_LONGEST, Phase, distance_text};
use super::superbounce_guide::{self as guide, Guide};
use super::superbounce_segments::{self as segs, Segments};
use crate::world::Pose;

pub const NAME: &str = "superbounce-readout";

pub const SPEED: &str = "sbr.speed";
pub const FALL: &str = "sbr.fall";
pub const STEER: &str = "sbr.steer";
pub const INFO: &str = "sbr.info";
pub const TAG: &str = "sbr.tag";
pub const TARGET_1: &str = "sbr.target.1";
pub const TARGET_2: &str = "sbr.target.2";
/// The target navpoints, nearest first.
pub const TARGETS: [&str; 2] = [TARGET_1, TARGET_2];

/// The arrow pointing up, the first of eight a HUD font carries, each 45
/// degrees clockwise of the last (`font::arrows`, U+E2B0..U+E2B7;
/// docs/game/hud-kit.md, "Direction arrows").
pub const ARROW_UP: char = '\u{e2b0}';
/// The arrow pointing down: the fall speed's.
pub const ARROW_DOWN: char = '\u{e2b4}';
/// How many arrows.
pub const ARROWS: u32 = 8;

/// The arrow nearest `angle`, radians clockwise from up (forward, for a
/// direction against the aim). An angle that is not a number is up.
pub fn arrow(angle: f32) -> char {
    let step = if angle.is_finite() {
        ((angle / std::f32::consts::FRAC_PI_4).round() as i64).rem_euclid(i64::from(ARROWS)) as u32
    } else {
        0
    };
    char::from_u32(ARROW_UP as u32 + step).unwrap_or(ARROW_UP)
}

/// Pixels between the charge segments' right side and the rows.
pub const SEGMENTS_GAP: i16 = 6;
/// The rows' left edge, from the frame's bottom-left corner (48, 444):
/// beside the motion tracker (`rects::TRACKER`, x 47 to 130),
/// [`SEGMENTS_GAP`] right of the charge segments' right side
/// (`superbounce_segments::RIGHT_EDGE`, x 143), at screen x 149.
pub const LEFT: i16 = segs::RIGHT_EDGE - segs::CORNER.0 + SEGMENTS_GAP;
/// The rows' tops from the frame's bottom: y 376, 397 and 418 fullscreen,
/// a line of font 6 (21) apart, the last ending 5 above the frame's foot.
pub const SPEED_UP: i16 = -68;
pub const FALL_UP: i16 = -47;
pub const INFO_UP: i16 = -26;
/// The bar: the battery strip's last 64 columns, 8 high, 6 under its
/// row's top so it sits on the text's middle.
pub const FALL_COLUMNS: u8 = 64;
pub const FALL_DOWN: i16 = 6;
/// The steer text's left edge: 6 right of the bar.
pub const STEER_LEFT: i16 = LEFT + FALL_COLUMNS as i16 + 6;

/// The widest texts: the most characters, the widest digits.
pub const SPEED_LONGEST: &str = "\u{e2b1} 88.8  \u{e2b4} 88.8 m/s";
pub const STEER_LONGEST: &str = "\u{e2b0} 88.88 m 100%";
pub const INFO_LONGEST: &str = "No bounce: on seam at 88.8 m/s";
/// The widest ranked line: [`NEXT`] seams, three digits each. Shorter
/// than [`INFO_LONGEST`], whose row it shares.
pub const NEXT_LONGEST: &str = "1 \u{e2b1} 888 m  2 \u{e2b1} 888 m";
/// The steer line when the coasting landing is caught.
pub const ON_SEAM: &str = "On seam";
/// The steer line's word after the arrow when a caught landing wants its
/// drift.
pub const DRIFT: &str = "Drift";

/// The largest speed and distance shown.
pub const SPEED_MAX: f32 = 99.9;
pub const STEER_MAX: f32 = 99.99;
/// Under this horizontal speed (metres a second) the player is not
/// moving: no arrow, no number.
pub const MOVING: f32 = 0.2;

/// The player's motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speed {
    /// The way of travel against the aim, radians clockwise from
    /// straight ahead.
    pub heading: f32,
    /// Horizontal speed, metres a second.
    pub across: f32,
    /// The fall speed, metres a second, while airborne and falling.
    pub down: Option<f32>,
}

/// Where the coasting landing comes down against the seam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Steer {
    /// Within the seam's catch.
    OnSeam,
    /// Off it: the way to push against the aim (radians clockwise from
    /// straight ahead) and how far, metres.
    Off { heading: f32, metres: f32 },
    /// Within the catch, coming down still or against the way a bounce
    /// on it drifts: the way to drift, against the aim.
    Drift { heading: f32 },
}

/// A fall toward a seam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fall {
    /// The touchdown speed the fall gives over the speed the seam needs:
    /// 1 and over is fast enough.
    pub speed: f32,
    pub steer: Steer,
}

/// Why a landing by a seam did not bounce, with its numbers: the
/// tracker's verdict by its first failed test (docs/game/superbounce.md,
/// "Landings"). Speeds in metres a second, distances in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Miss {
    NotPrimed,
    /// The origin was `by` from the seam's edge.
    Missed { by: f32 },
    /// The speed in was `had`; the seam needs `need`.
    TooSlow { had: f32, need: f32 },
    /// Primed, on the seam, at `had`, and no launch.
    NoBounce { had: f32 },
}

/// A seam the flight after a bounce can reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Next {
    /// Where it is against the aim, radians clockwise from straight
    /// ahead.
    pub heading: f32,
    /// Metres to it.
    pub metres: f32,
}

/// Which kinds of seam the target navpoints are on: the variant's
/// `targets` option, or the player's switch of it in the game
/// (docs/game/superbounce.md, "Targets").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kinds {
    Common,
    Likely,
    All,
}

impl Kinds {
    pub const EVERY: [Kinds; 3] = [Kinds::Common, Kinds::Likely, Kinds::All];
}

/// The line that says which kinds the player has just switched to.
pub fn kinds_text(k: Kinds) -> &'static str {
    match k {
        Kinds::Common => "Targets: common",
        Kinds::Likely => "Targets: likely",
        Kinds::All => "Targets: all",
    }
}

/// How many ranked seams the line lists: as many as the readout has
/// target navpoints ([`TARGETS`]).
pub const NEXT: usize = 2;

/// What the readout shows. `None` hides a piece.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct View {
    pub speed: Option<Speed>,
    pub fall: Option<Fall>,
    pub info: Option<Miss>,
    /// In the flight after a bounce, the seams it can reach, the best
    /// first. With any, the line is theirs and not the miss's, the fall
    /// row is the best one's, and the target navpoints are on them.
    pub next: [Option<Next>; NEXT],
    /// The kinds of seam the player has just switched the targets to:
    /// while set, the line says so ([`kinds_text`]) and nothing else.
    pub kinds: Option<Kinds>,
    /// Metres to the tagged seam.
    pub tag: Option<f32>,
    /// Metres to each reachable seam marked, nearest first.
    pub targets: [Option<f32>; 2],
    /// The approach guide on the seam the player is going for
    /// ([`super::superbounce_guide`]).
    pub guide: Option<Guide>,
    /// The charge segments round the motion tracker
    /// ([`super::superbounce_segments`]).
    pub segments: Option<Segments>,
}

fn one(v: f32, max: f32) -> String {
    let v = if v.is_finite() { v.clamp(0.0, max) } else { 0.0 };
    format!("{v:.1}")
}

/// The speed line: `(arrow) 7.2 m/s` on the ground, `(arrow) 2.1  (down)
/// 12.4 m/s` falling, the fall speed alone when the player is not moving
/// across; `None` standing still.
pub fn speed_text(s: &Speed) -> Option<String> {
    let moving = s.across.is_finite() && s.across >= MOVING;
    let across = moving.then(|| format!("{} {}", arrow(s.heading), one(s.across, SPEED_MAX)));
    let down = s
        .down
        .filter(|d| d.is_finite() && *d >= MOVING)
        .map(|d| format!("{ARROW_DOWN} {}", one(d, SPEED_MAX)));
    match (across, down) {
        (Some(a), Some(d)) => Some(format!("{a}  {d} m/s")),
        (Some(a), None) => Some(format!("{a} m/s")),
        (None, Some(d)) => Some(format!("{d} m/s")),
        (None, None) => None,
    }
}

/// The steer line: [`ON_SEAM`], or the arrow and the distance, `(arrow)
/// 0.40 m`.
pub fn steer_text(s: &Steer) -> String {
    match s {
        Steer::OnSeam => ON_SEAM.into(),
        Steer::Off { heading, metres } => {
            let m = if metres.is_finite() { metres.clamp(0.0, STEER_MAX) } else { 0.0 };
            format!("{} {m:.2} m", arrow(*heading))
        }
        Steer::Drift { heading } => format!("{} {DRIFT}", arrow(*heading)),
    }
}

/// The fall row's text: the steer line ([`steer_text`]) and, with the
/// approach guide's chance, its percentage after it, `On seam 82%`.
pub fn steer_line(s: &Steer, chance: Option<u8>) -> String {
    match chance {
        Some(c) => format!("{} {}%", steer_text(s), c.min(100)),
        None => steer_text(s),
    }
}

/// The chance the fall row carries for `v`: the approach guide's, on a
/// fall toward the seam the guide is on. In the flight after a bounce
/// the row is the best ranked seam's, which the guide may not be on: no
/// percentage.
pub fn fall_chance(v: &View) -> Option<u8> {
    let ranked = v.next.iter().any(|n| n.is_some());
    v.guide.filter(|_| !ranked).and_then(|g| g.chance)
}

/// The miss line.
pub fn miss_text(m: &Miss) -> String {
    match m {
        Miss::NotPrimed => "Not primed".into(),
        Miss::Missed { by } => {
            let by = if by.is_finite() { by.clamp(0.0, STEER_MAX) } else { 0.0 };
            format!("Missed seam by {by:.2} m")
        }
        Miss::TooSlow { had, need } => {
            format!("Too slow: {} of {} m/s", one(*had, SPEED_MAX), one(*need, SPEED_MAX))
        }
        Miss::NoBounce { had } => format!("No bounce: on seam at {} m/s", one(*had, SPEED_MAX)),
    }
}

/// The ranked line: each seam's rank, the arrow toward it and its
/// distance in whole metres, `1 (arrow) 12 m  2 (arrow) 30 m`; `None`
/// with no seam.
pub fn next_text(next: &[Option<Next>; NEXT]) -> Option<String> {
    let parts: Vec<String> = next
        .iter()
        .flatten()
        .enumerate()
        .map(|(i, n)| format!("{} {} {} m", i + 1, arrow(n.heading), distance_text(n.metres)))
        .collect();
    (!parts.is_empty()).then(|| parts.join("  "))
}

/// The elements, in draw order: the bar, the charge segments, the rows'
/// texts, the navpoints, then the approach guide's world marks.
pub fn hud() -> Hud {
    let row = |up: i16| Place::new(Anchor::BottomLeft, (LEFT, up));
    let navpoint = |id| Element::channel(id, Kind::Navpoint { longest: DISTANCE_LONGEST });
    let mut elements = vec![Element::meter(
        FALL,
        Art::StripMeterEnd(FALL_COLUMNS),
        Place::new(Anchor::BottomLeft, (LEFT, FALL_UP + FALL_DOWN)),
    )];
    elements.extend(segs::elements());
    let mut hud = Hud {
        name: NAME,
        elements,
    };
    hud.elements.extend([
            Element::text(SPEED, Font::Hud, Justify::Start, SPEED_LONGEST, row(SPEED_UP)),
            Element::text(
                STEER,
                Font::Hud,
                Justify::Start,
                STEER_LONGEST,
                Place::new(Anchor::BottomLeft, (STEER_LEFT, FALL_UP)),
            ),
            Element::text(INFO, Font::Hud, Justify::Start, INFO_LONGEST, row(INFO_UP)),
            navpoint(TAG),
            navpoint(TARGET_1),
            navpoint(TARGET_2),
    ]);
    hud.elements.extend(guide::elements());
    hud
}

/// The values for `v`: every element of [`hud`], once.
pub fn values(v: &View) -> Values {
    let mut out = Values::default();
    let text = |t: Option<String>| t.map_or_else(Value::hidden, Value::text);
    let nav = |m: Option<f32>| text(m.map(distance_text));
    out.set(
        FALL,
        match v.fall {
            Some(f) if f.speed.is_finite() => Value::fill(f.speed.clamp(0.0, 1.0)),
            Some(_) => Value::fill(0.0),
            None => Value::hidden(),
        },
    );
    out.set(SPEED, text(v.speed.as_ref().and_then(speed_text)));
    out.set(STEER, text(v.fall.map(|f| steer_line(&f.steer, fall_chance(v)))));
    out.set(
        INFO,
        text(
            v.kinds
                .map(|k| kinds_text(k).to_owned())
                .or_else(|| next_text(&v.next))
                .or_else(|| v.info.as_ref().map(miss_text)),
        ),
    );
    out.set(TAG, nav(v.tag));
    out.set(TARGET_1, nav(v.targets[0]));
    out.set(TARGET_2, nav(v.targets[1]));
    guide::set_values(&mut out, v.guide.as_ref());
    segs::set_values(&mut out, v.segments.as_ref());
    out
}

/// The preview's states, by name, as [`View`]s.
pub fn views() -> Vec<(&'static str, View)> {
    use std::f32::consts::FRAC_PI_4;
    let running = Speed {
        heading: FRAC_PI_4,
        across: 7.2,
        down: None,
    };
    let falling = Speed {
        heading: 0.0,
        across: 2.1,
        down: Some(12.4),
    };
    let fall = |speed, steer| View {
        speed: Some(falling),
        fall: Some(Fall { speed, steer }),
        ..View::default()
    };
    let miss = |m| View {
        info: Some(m),
        ..View::default()
    };
    let mut out = vec![
        ("idle", View::default()),
        (
            "moving",
            View {
                speed: Some(running),
                ..View::default()
            },
        ),
        (
            "falling-off-seam",
            fall(
                0.6,
                Steer::Off {
                    heading: -2.0 * FRAC_PI_4,
                    metres: 0.4,
                },
            ),
        ),
        ("falling-on-seam", fall(1.0, Steer::OnSeam)),
        (
            "falling-wants-drift",
            fall(
                1.0,
                Steer::Drift {
                    heading: 2.0 * FRAC_PI_4,
                },
            ),
        ),
        ("miss-not-primed", miss(Miss::NotPrimed)),
        ("miss-missed", miss(Miss::Missed { by: 0.31 })),
        ("miss-too-slow", miss(Miss::TooSlow { had: 12.4, need: 16.5 })),
        ("miss-no-bounce", miss(Miss::NoBounce { had: 17.2 })),
        (
            "targets",
            View {
                speed: Some(running),
                targets: [Some(9.0), Some(21.0)],
                ..View::default()
            },
        ),
        (
            "second-bounce",
            View {
                speed: Some(Speed {
                    heading: 0.0,
                    across: 3.4,
                    down: None,
                }),
                fall: Some(Fall {
                    speed: 1.0,
                    steer: Steer::Off {
                        heading: -FRAC_PI_4,
                        metres: 1.2,
                    },
                }),
                next: [
                    Some(Next {
                        heading: -FRAC_PI_4,
                        metres: 12.0,
                    }),
                    Some(Next {
                        heading: 2.0 * FRAC_PI_4,
                        metres: 30.0,
                    }),
                ],
                targets: [Some(12.0), Some(30.0)],
                ..View::default()
            },
        ),
        (
            "second-bounce-lined-up",
            View {
                speed: Some(falling),
                fall: Some(Fall {
                    speed: 1.0,
                    steer: Steer::OnSeam,
                }),
                next: [
                    Some(Next {
                        heading: 0.0,
                        metres: 4.0,
                    }),
                    None,
                ],
                targets: [Some(4.0), None],
                ..View::default()
            },
        ),
        (
            "targets-switched",
            View {
                speed: Some(running),
                kinds: Some(Kinds::All),
                targets: [Some(6.0), Some(9.0)],
                ..View::default()
            },
        ),
        (
            "tagged",
            View {
                tag: Some(34.0),
                targets: [Some(9.0), None],
                ..View::default()
            },
        ),
    ];
    // The approach guide's moments: the rows as they are then, with the
    // guide's marks (`superbounce_guide::samples`).
    let rows = |name: &str| match name {
        "guide-approach" | "guide-lined-up" => View {
            speed: Some(running),
            ..View::default()
        },
        "guide-off-line" => fall(
            0.97,
            Steer::Off {
                heading: 3.0 * FRAC_PI_4,
                metres: 1.4,
            },
        ),
        _ => fall(0.97, Steer::OnSeam),
    };
    for (name, g, _) in guide::samples() {
        out.push((
            name,
            View {
                guide: Some(g),
                // The guide shows on a seam while primed.
                segments: Some(segs::segments(Some(Charge {
                    phase: Phase::Primed,
                    fill: 1.0,
                }))),
                ..rows(name)
            },
        ));
    }
    // The charge segments' moments (`superbounce_segments::samples`).
    for (name, charge) in segs::samples() {
        out.push((
            name,
            View {
                segments: Some(segs::segments(charge)),
                ..View::default()
            },
        ));
    }
    // In every other state but `idle` (a map with no strips: nothing of
    // the readout shows) the ring is there, empty: not primed.
    for (name, v) in &mut out {
        if *name != "idle" {
            v.segments.get_or_insert(segs::segments(None));
        }
    }
    out
}

/// The readout's description.
pub struct Readout;

impl Describe for Readout {
    fn hud(&self) -> Hud {
        hud()
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        views().into_iter().map(|(n, v)| (n, values(&v))).collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("superbounce is a multiplayer game type")
    }

    /// The approach guide's sample scene in its moments.
    fn world(&self, state: &str) -> Vec<(&'static str, Pose)> {
        guide::samples()
            .into_iter()
            .find(|(name, _, _)| *name == state)
            .map(|(_, _, l)| guide::poses(&l))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    fn state(name: &str) -> Values {
        Readout.states().into_iter().find(|(n, _)| *n == name).unwrap().1
    }

    fn shown(s: &str, id: &str) -> Option<String> {
        let v = state(s);
        let x = v.get(id).unwrap();
        x.shown.then(|| x.text.clone())
    }

    /// One layout: every state values every element once and nothing
    /// else, and the model has no faults.
    #[test]
    fn every_state_has_the_one_layout() {
        let h = hud();
        assert!(h.problems().is_empty(), "{:?}", h.problems());
        for (name, v) in Readout.states() {
            assert!(v.layout_problems(&h).is_empty(), "{name}: {:?}", v.layout_problems(&h));
        }
        assert!(Readout.multiplayer_only().is_some());
    }

    /// Fullscreen only, bottom left, right of the tracker and of the
    /// charge segments' right side, clear of the middle of the screen;
    /// the rows a line apart.
    #[test]
    fn the_rows_are_bottom_left_and_fullscreen_only() {
        let h = hud();
        for e in h.elements.iter().filter(|e| !e.kind.is_channel()) {
            assert_eq!(e.at.anchor, Anchor::BottomLeft, "{}", e.id);
            assert_eq!((e.at.half, e.at.quarter), (None, None), "{}", e.id);
            if segs::IDS.contains(&e.id) {
                continue;
            }
            let (x, y) = (48 + e.at.full.0, 444 + e.at.full.1);
            assert!(x >= 149, "{}: right of the segments' right side, x 134 to 143", e.id);
            assert!((376..444).contains(&y), "{}: under the tracker's top, in the frame", e.id);
            assert_eq!(e.outside, 0, "{}: inside the frame", e.id);
        }
        assert_eq!(48 + LEFT, 149);
        let top = |id: &str| 444 + h.element(id).unwrap().at.full.1;
        assert_eq!((top(SPEED), top(STEER), top(INFO)), (376, 397, 418));
        // The bar is in the steer row, left of its text.
        let bar = h.element(FALL).unwrap().at.full;
        assert_eq!((48 + bar.0, 444 + bar.1), (149, 403));
        assert_eq!(48 + h.element(STEER).unwrap().at.full.0, 149 + 64 + 6);
        assert_eq!(
            h.arts().len(),
            3,
            "the strip's end at the bar's width and at a segment's, and the navpoints' arrow"
        );
    }

    #[test]
    fn an_angle_picks_the_nearest_of_eight_arrows() {
        assert_eq!(arrow(0.0), '\u{e2b0}');
        assert_eq!(arrow(0.3), '\u{e2b0}');
        assert_eq!(arrow(FRAC_PI_2), '\u{e2b2}');
        assert_eq!(arrow(PI), ARROW_DOWN);
        assert_eq!(arrow(-FRAC_PI_2), '\u{e2b6}');
        assert_eq!(arrow(2.0 * PI - 0.1), ARROW_UP);
        assert_eq!(arrow(f32::NAN), ARROW_UP);
    }

    #[test]
    fn the_speed_line_is_hidden_still_and_adds_the_fall_in_the_air() {
        let s = |across, down| Speed {
            heading: FRAC_PI_2,
            across,
            down,
        };
        assert_eq!(speed_text(&s(0.0, None)), None);
        assert_eq!(speed_text(&s(0.19, None)), None);
        assert_eq!(speed_text(&s(7.24, None)).unwrap(), "\u{e2b2} 7.2 m/s");
        assert_eq!(
            speed_text(&s(2.1, Some(12.44))).unwrap(),
            "\u{e2b2} 2.1  \u{e2b4} 12.4 m/s"
        );
        assert_eq!(speed_text(&s(0.0, Some(12.4))).unwrap(), "\u{e2b4} 12.4 m/s");
        assert_eq!(speed_text(&s(500.0, None)).unwrap(), "\u{e2b2} 99.9 m/s");
        assert_eq!(speed_text(&s(f32::NAN, None)), None);
        assert_eq!(shown("idle", SPEED), None);
        assert_eq!(shown("moving", SPEED).unwrap(), "\u{e2b1} 7.2 m/s");
    }

    #[test]
    fn the_fall_row_is_the_bar_and_the_way_to_steer() {
        let v = state("falling-off-seam");
        let bar = v.get(FALL).unwrap();
        assert!(bar.shown);
        assert_eq!(bar.fill, 0.6);
        assert_eq!(shown("falling-off-seam", STEER).unwrap(), "\u{e2b6} 0.40 m");
        assert_eq!(state("falling-on-seam").get(FALL).unwrap().fill, 1.0);
        assert_eq!(shown("falling-on-seam", STEER).unwrap(), ON_SEAM);
        // Faster than needed is a full bar, not past it.
        let over = values(&View {
            fall: Some(Fall {
                speed: 1.8,
                steer: Steer::OnSeam,
            }),
            ..View::default()
        });
        assert_eq!(over.get(FALL).unwrap().fill, 1.0);
        assert!(!state("moving").get(FALL).unwrap().shown);
        assert_eq!(shown("moving", STEER), None);
    }

    #[test]
    fn a_miss_says_why_with_its_numbers() {
        assert_eq!(shown("miss-not-primed", INFO).unwrap(), "Not primed");
        assert_eq!(shown("miss-missed", INFO).unwrap(), "Missed seam by 0.31 m");
        assert_eq!(shown("miss-too-slow", INFO).unwrap(), "Too slow: 12.4 of 16.5 m/s");
        assert_eq!(
            shown("miss-no-bounce", INFO).unwrap(),
            "No bounce: on seam at 17.2 m/s"
        );
        assert_eq!(shown("moving", INFO), None);
    }

    /// In the flight after a bounce the line is the ranked seams', over
    /// a miss's, and fits the row it shares.
    #[test]
    fn the_ranked_line_lists_the_seams_best_first() {
        assert_eq!(next_text(&[None, None]), None);
        let n = |heading, metres| Some(Next { heading, metres });
        assert_eq!(
            next_text(&[n(FRAC_PI_2, 12.4), n(PI, 30.0)]).unwrap(),
            "1 \u{e2b2} 12 m  2 \u{e2b4} 30 m"
        );
        assert_eq!(next_text(&[n(0.0, 4.0), None]).unwrap(), "1 \u{e2b0} 4 m");
        assert_eq!(
            next_text(&[n(0.0, 1e9), n(0.0, f32::NAN)]).unwrap().chars().count(),
            "1 \u{e2b0} 999 m  2 \u{e2b0} 0 m".chars().count()
        );
        assert!(NEXT_LONGEST.chars().count() <= INFO_LONGEST.chars().count());
        assert_eq!(NEXT, TARGETS.len());
        let v = View {
            info: Some(Miss::NotPrimed),
            next: [n(0.0, 4.0), None],
            ..View::default()
        };
        assert_eq!(values(&v).get(INFO).unwrap().text, "1 \u{e2b0} 4 m");
        assert_eq!(
            shown("second-bounce", INFO).unwrap(),
            "1 \u{e2b7} 12 m  2 \u{e2b2} 30 m"
        );
        assert_eq!(shown("second-bounce", STEER).unwrap(), "\u{e2b7} 1.20 m");
        assert_eq!(shown("second-bounce", TARGET_2).unwrap(), "30");
        assert_eq!(shown("second-bounce-lined-up", STEER).unwrap(), ON_SEAM);
        assert_eq!(shown("second-bounce-lined-up", TARGET_2), None);
    }

    /// The switch's line names the kinds, takes the row from the ranked
    /// line and the miss line while it is up, and is no longer than the
    /// row's longest.
    #[test]
    fn the_switch_line_names_the_kinds() {
        assert_eq!(
            Kinds::EVERY.map(kinds_text),
            ["Targets: common", "Targets: likely", "Targets: all"]
        );
        for k in Kinds::EVERY {
            assert!(kinds_text(k).chars().count() <= INFO_LONGEST.chars().count());
        }
        let v = View {
            info: Some(Miss::NotPrimed),
            next: [Some(Next { heading: 0.0, metres: 4.0 }), None],
            kinds: Some(Kinds::Common),
            ..View::default()
        };
        assert_eq!(values(&v).get(INFO).unwrap().text, "Targets: common");
        assert_eq!(shown("targets-switched", INFO).unwrap(), "Targets: all");
        assert_eq!(shown("targets-switched", TARGET_2).unwrap(), "9");
        assert_eq!(shown("targets", INFO), None);
    }

    #[test]
    fn the_navpoints_show_with_their_distance() {
        assert_eq!(shown("targets", TARGET_1).unwrap(), "9");
        assert_eq!(shown("targets", TARGET_2).unwrap(), "21");
        assert_eq!(shown("targets", TAG), None);
        assert_eq!(shown("tagged", TAG).unwrap(), "34");
        assert_eq!(shown("tagged", TARGET_2), None);
        assert_eq!(shown("idle", TARGET_1), None);
    }

    /// The approach guide's marks are the readout's last four elements,
    /// hidden in every state but the guide's moments, which pose every
    /// mark they show.
    #[test]
    fn the_guide_is_in_its_moments_alone() {
        let h = hud();
        let ids: Vec<_> = h.elements[h.elements.len() - 4..].iter().map(|e| e.id).collect();
        assert_eq!(ids, guide::MARKS);
        for id in guide::MARKS {
            let e = h.element(id).unwrap();
            assert!(e.kind.world().is_some() && e.kind.is_channel(), "{id}");
            assert_eq!((e.at.half, e.at.quarter), (None, None), "{id}: fullscreen only");
        }
        let mut moments = 0;
        for (name, v) in Readout.states() {
            let poses = Readout.world(name);
            let is_guide = name.starts_with("guide-");
            moments += usize::from(is_guide);
            for id in guide::MARKS {
                let shown = v.get(id).unwrap().shown;
                assert_eq!(shown, poses.iter().any(|(i, _)| *i == id), "{name} {id}");
                assert!(is_guide || !shown, "{name} {id}");
            }
        }
        assert_eq!(moments, guide::samples().len());
        assert_eq!(shown("guide-falling-caught", guide::BOX).unwrap(), "Will bounce 82%");
        assert_eq!(shown("guide-too-shallow", guide::BOX).unwrap(), "Too shallow 30%");
        // Before the jump the box has the percentage alone.
        assert_eq!(shown("guide-lined-up", guide::BOX).unwrap(), "62%");
        assert_eq!(shown("guide-approach", guide::BOX).unwrap(), "13%");
        assert_eq!(shown("guide-scan-only", guide::BOX).unwrap(), "Will bounce 60%");
    }

    /// The fall row ends with the guide's percentage on a fall toward
    /// its seam: after `On seam`, the drift's word or the distance; not
    /// with no guide, not on the ground (no row), and not in the flight
    /// after a bounce, where the row is the best ranked seam's.
    #[test]
    fn the_fall_row_ends_with_the_guides_percentage() {
        assert_eq!(shown("guide-falling-caught", STEER).unwrap(), "On seam 82%");
        assert_eq!(shown("guide-too-shallow", STEER).unwrap(), "On seam 30%");
        assert_eq!(shown("guide-on-the-cut", STEER).unwrap(), "On seam 52%");
        assert_eq!(shown("guide-off-line", STEER).unwrap(), "\u{e2b3} 1.40 m 5%");
        assert_eq!(shown("guide-lined-up", STEER), None);
        assert_eq!(shown("falling-on-seam", STEER).unwrap(), ON_SEAM);
        let on = Steer::OnSeam;
        assert_eq!(steer_line(&on, None), ON_SEAM);
        assert_eq!(steer_line(&on, Some(95)), "On seam 95%");
        assert_eq!(steer_line(&on, Some(200)), "On seam 100%");
        let drift = Steer::Drift { heading: FRAC_PI_2 };
        assert_eq!(steer_line(&drift, Some(40)), "\u{e2b2} Drift 40%");
        // The widest: the furthest landing at the widest percentage.
        let off = Steer::Off { heading: 1.0, metres: 1e9 };
        let h = hud();
        let longest = h.element(STEER).unwrap().kind.longest().unwrap().chars().count();
        assert_eq!(steer_line(&off, Some(100)).chars().count(), longest);
        // After a bounce the row is the ranked seam's: the guide's
        // percentage is left off it.
        let (_, falling) = views().into_iter().find(|(n, _)| *n == "guide-falling-caught").unwrap();
        assert_eq!(fall_chance(&falling), Some(82));
        let ranked = View {
            next: [Some(Next { heading: 0.0, metres: 4.0 }), None],
            ..falling
        };
        assert_eq!(fall_chance(&ranked), None);
        assert_eq!(values(&ranked).get(STEER).unwrap().text, ON_SEAM);
        assert_eq!(fall_chance(&View { guide: None, ..falling }), None);
    }

    /// The charge segments are nine meters after the bar: the empty ring
    /// in every state but `idle`, the guide's moments primed and full,
    /// and their own moments one for each stage.
    #[test]
    fn the_segments_show_the_ring_in_every_state_but_idle() {
        let h = hud();
        let ids: Vec<_> = h.elements[1..=segs::SEGMENTS].iter().map(|e| e.id).collect();
        assert_eq!(ids, segs::IDS);
        let fills = |name: &str| -> Vec<Option<f32>> {
            let v = state(name);
            segs::IDS
                .iter()
                .map(|id| v.get(id).filter(|x| x.shown).map(|x| x.fill))
                .collect()
        };
        assert_eq!(fills("idle"), [None; 9]);
        assert_eq!(fills("moving"), [Some(0.0); 9]);
        assert_eq!(fills("miss-too-slow"), [Some(0.0); 9]);
        assert_eq!(fills("guide-lined-up"), [Some(1.0); 9]);
        assert_eq!(fills("segments-idle"), [Some(0.0); 9]);
        assert_eq!(fills("segments-near"), [Some(0.0); 9]);
        let lit = |name: &str| fills(name).iter().filter(|f| **f == Some(1.0)).count();
        assert_eq!(
            ["segments-under", "segments-held", "segments-primed"].map(lit),
            [3, 6, 9]
        );
        // Three quarters of the charge: 6.75 segments.
        assert_eq!(fills("segments-draining")[5..], [Some(1.0), Some(0.75), Some(0.0), Some(0.0)]);
        assert_eq!(lit("segments-warning"), 4);
        assert_eq!(fills("segments-warning")[4], Some(0.5));
        assert_eq!(lit("segments-lost"), 0);
        let n = Readout.states().iter().filter(|(n, _)| n.starts_with("segments-")).count();
        assert_eq!(n, segs::samples().len());
    }

    /// No text any state or any number gives is longer than its
    /// element's `longest`, by its characters (the checks measure the
    /// pixels on the disc's font).
    #[test]
    fn no_text_is_longer_than_its_longest() {
        let h = hud();
        let longest = |id: &str| h.element(id).unwrap().kind.longest().unwrap().chars().count();
        for (name, v) in Readout.states() {
            for e in &h.elements {
                let x = v.get(e.id).unwrap();
                if let (true, Some(l)) = (x.shown, e.kind.longest()) {
                    assert!(x.text.chars().count() <= l.chars().count(), "{name} {}", e.id);
                }
            }
        }
        let far = Speed {
            heading: 1.0,
            across: 1e9,
            down: Some(1e9),
        };
        assert_eq!(speed_text(&far).unwrap().chars().count(), longest(SPEED));
        let off = Steer::Off {
            heading: 1.0,
            metres: 1e9,
        };
        assert_eq!(steer_line(&off, Some(100)).chars().count(), longest(STEER));
        assert!(ON_SEAM.chars().count() <= longest(STEER));
        let drift = steer_text(&Steer::Drift { heading: FRAC_PI_2 });
        assert_eq!(drift, "\u{e2b2} Drift");
        assert!(drift.chars().count() <= longest(STEER));
        assert_eq!(shown("falling-wants-drift", STEER).unwrap(), drift);
        for m in [
            Miss::NotPrimed,
            Miss::Missed { by: 1e9 },
            Miss::TooSlow { had: 1e9, need: 1e9 },
            Miss::NoBounce { had: 1e9 },
        ] {
            assert!(miss_text(&m).chars().count() <= longest(INFO), "{m:?}");
        }
        assert_eq!(miss_text(&Miss::NoBounce { had: 1e9 }).chars().count(), longest(INFO));
    }
}
