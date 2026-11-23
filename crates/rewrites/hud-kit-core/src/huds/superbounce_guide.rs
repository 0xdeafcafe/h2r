//! The superbounce approach guide (docs/game/hud-kit.md, "The approach
//! guide"; docs/game/superbounce.md, "The approach guide"): five world
//! marks of the readout HUD ([`super::superbounce_readout`]) on the seam
//! the player is going for, so the line-up is seen on the geometry and
//! not guessed.
//!
//! - `sbr.guide.box`: a bracket on the seam's edge by its landing point, turned along
//!   the seam's edge: the catch across it, [`BOX_ALONG`] along it. Dim
//!   while the predicted landing is off the seam, lit when it is lined
//!   up, and during a fall the bounce rule's verdict (`superbounce::rule`):
//!   green `Will bounce`, or why not: `Too shallow`, `Too high`, `Off
//!   edge`, `No drift`, `Don't crouch` ([`Call`]). Its label is the one
//!   verdict, with no percentage: in a fall the call; before the jump the
//!   jump cue ([`Cue`]): `Walk to
//!   line`, `Jump in 0.7 s` or, green, `Jump now` ([`box_text`]).
//! - `sbr.guide.pipper`: where the player's path touches down: on the
//!   ground where a jump now would land, in the air the landing itself.
//! - `sbr.guide.ticks`: closure cues, short dashes from the pipper toward
//!   the box that shorten as the two close and go when the landing is
//!   caught.
//! - `sbr.guide.drift`: an arrow beside the box for the way the seam
//!   wants the landing to drift, lit while the player moves that way.
//! - `sbr.guide.takeoff`: on the ground, the line to press jump on: a
//!   bar across the run with the band of presses that bounce, green
//!   while the cue is `Jump now`.
//!
//! [`Guide`] is what the marks show and [`Layout`] where they lie;
//! [`poses`] turns a layout into each mark's pose, for the game's runtime
//! (`game::superbounce_readout`) and for the preview's samples
//! ([`samples`]) alike.

use crate::model::{Element, Kind, Style, Value, Values};
use crate::world::{Pose, Shape, Vec3, unit};

pub const BOX: &str = "sbr.guide.box";
pub const PIPPER: &str = "sbr.guide.pipper";
pub const TICKS: &str = "sbr.guide.ticks";
pub const DRIFT: &str = "sbr.guide.drift";
pub const TAKEOFF: &str = "sbr.guide.takeoff";
/// The guide's marks, in draw order.
pub const MARKS: [&str; 5] = [BOX, PIPPER, TICKS, DRIFT, TAKEOFF];

/// The box's labels during a fall.
pub const WILL_BOUNCE: &str = "Will bounce";
pub const TOO_SHALLOW: &str = "Too shallow";
pub const TOO_HIGH: &str = "Too high";
pub const OFF_EDGE: &str = "Off edge";
pub const NO_DRIFT: &str = "No drift";
pub const DONT_CROUCH: &str = "Don't crouch";
/// Every call's label, [`WILL_BOUNCE`] first.
pub const CALLS: [&str; 6] = [
    WILL_BOUNCE,
    TOO_SHALLOW,
    TOO_HIGH,
    OFF_EDGE,
    NO_DRIFT,
    DONT_CROUCH,
];
/// The box's labels before the jump ([`Cue`]).
pub const WALK_TO_LINE: &str = "Walk to line";
pub const JUMP_NOW: &str = "Jump now";
/// The widest label: the widest cue, `Jump in 9.9 s`, wider than any
/// call.
pub const BOX_LONGEST: &str = "Jump in 9.9 s";
/// The most the count says, in tenths of a second: `Jump in 9.9 s`.
pub const CUE_MAX: u8 = 99;

/// The box's half length along the seam's edge (at most half the edge).
pub const BOX_ALONG: f32 = 0.3;
/// The box's half width across the seam: the catch
/// (`superbounce::CATCH`), so a pipper inside the box is a caught
/// landing.
pub const BOX_ACROSS: f32 = 0.1;
/// The pipper's radius.
pub const PIPPER_RADIUS: f32 = 0.06;
/// The drift arrow: half its length, and how far out its head's wings
/// are.
pub const ARROW_HALF: [f32; 2] = [0.09, 0.04];
/// The gap between the box and the drift arrow's tail.
pub const ARROW_GAP: f32 = 0.06;
/// The gap between the pipper's ring and the first tick.
pub const TICKS_GAP: f32 = 0.03;
/// The longest run the ticks take, however far the box is.
pub const TICKS_RUN: f32 = 0.6;
/// Under this run the ticks are not drawn.
pub const TICKS_MIN: f32 = 0.02;
/// The takeoff line's half length across the run.
pub const TAKEOFF_ACROSS: f32 = 0.25;

/// What the box says of the predicted landing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Call {
    /// Not within the seam's catch.
    OffLine,
    /// Within the catch, with no call on the bounce: on the ground, or
    /// in a fall the bounce rule has no floor for.
    LinedUp,
    /// In a fall: the sweep does not come on before the floor (the bounce
    /// rule's `stopped` or `landing`): the drop from the apex is too
    /// short.
    TooShallow,
    /// In a fall: the sweep comes on too high and the capsule is caught
    /// over the floor (the rule's `caught`): the drop is too long.
    TooHigh,
    /// In a fall: through a floor polygon too far from its edges (the
    /// rule's `pop`).
    OffEdge,
    /// In a fall: on an edge with too little drift across it (the rule's
    /// `level_pinch`).
    NoDrift,
    /// In a fall, primed, with crouch held: a crouch in the air ends the
    /// prime.
    Crouching,
    /// In a fall: primed, and the bounce rule says it bounces.
    WillBounce,
}

/// The jump cue on the box before the jump: when to press jump for the
/// landing to bounce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cue {
    /// Standing, or not moving toward the takeoff line: walk to it.
    Walk,
    /// Moving toward the line: this many tenths of a second to it,
    /// `Jump in 0.7 s`.
    In(u8),
    /// On the line: press jump now.
    Now,
    /// Any other cue as the game words it (`Climb up`, `Jump in`,
    /// `Walk off`): shown as it is, never green. At most
    /// [`BOX_LONGEST`] wide.
    Label(&'static str),
}

/// The count's tenths of a second for `ticks` game ticks (30 a
/// second): rounded up, so the last count before [`Cue::Now`] is `0.1`,
/// and kept to [`CUE_MAX`].
pub fn tenths(ticks: u32) -> u8 {
    ticks.div_ceil(3).min(u32::from(CUE_MAX)) as u8
}

/// The cue's words: `Walk to line`, `Jump in 0.7 s`, `Jump now`.
pub fn cue_text(c: Cue) -> String {
    match c {
        Cue::Walk => WALK_TO_LINE.to_owned(),
        Cue::In(t) => {
            let t = t.min(CUE_MAX);
            format!("Jump in {}.{} s", t / 10, t % 10)
        }
        Cue::Now => JUMP_NOW.to_owned(),
        Cue::Label(l) => l.to_owned(),
    }
}

/// What the guide shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guide {
    pub call: Call,
    /// The predicted landing is known and by the seam: the pipper.
    pub pipper: bool,
    /// The closure cues (the landing is known and not caught).
    pub ticks: bool,
    /// The seam has a drift: `Some(true)` while the player moves its way
    /// fast enough for a full launch (`superbounce::rule::DRIFT_FULL`).
    pub drift: Option<bool>,
    /// Before the jump, when to press it; `None` in the air and where no
    /// takeoff bounces.
    pub cue: Option<Cue>,
    /// The takeoff line is drawn.
    pub takeoff: bool,
}

/// Where the guide lies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The box's centre: the point of the seam's edge beside its landing
    /// point.
    pub seam: Vec3,
    /// The seam's edge direction, flat.
    pub along: [f32; 2],
    /// The box's half length along the edge.
    pub half_along: f32,
    /// The seam's drift, flat; zero for none.
    pub drift: [f32; 2],
    /// The predicted landing, on the seam's floor.
    pub impact: Option<Vec3>,
    /// The takeoff line, on the ground.
    pub takeoff: Option<Line>,
    /// 0 to 1: the guide's fade.
    pub strength: f32,
}

/// Where the takeoff line lies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// The line's centre, on the ground.
    pub at: Vec3,
    /// The run's direction, flat: the line lies across it.
    pub heading: [f32; 2],
    /// Half the band of presses that bounce, along the run.
    pub half: f32,
}

/// How far the ticks run from a pipper `distance` from the box's centre:
/// from the ring's edge and [`TICKS_GAP`] toward the box, at most
/// [`TICKS_RUN`]; `None` under [`TICKS_MIN`].
pub fn ticks_run(distance: f32) -> Option<f32> {
    // `min` drops a NaN, so the distance is checked before it.
    let run = (distance - PIPPER_RADIUS - TICKS_GAP).min(TICKS_RUN);
    (distance.is_finite() && run >= TICKS_MIN).then_some(run)
}

/// Each mark's pose for `l`; a mark with nothing to lie on (no landing,
/// no drift) has none.
pub fn poses(l: &Layout) -> Vec<(&'static str, Pose)> {
    let strength = if l.strength.is_finite() {
        l.strength.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let pose = |at: Vec3, along: [f32; 2], half: [f32; 2]| Pose {
        at,
        along,
        half,
        strength,
    };
    let mut out = vec![(BOX, pose(l.seam, l.along, [l.half_along, BOX_ACROSS]))];
    if let Some(p) = l.impact {
        out.push((PIPPER, pose(p, l.along, [PIPPER_RADIUS, 0.0])));
        let d = [l.seam[0] - p[0], l.seam[1] - p[1]];
        if let Some(run) = ticks_run(d[0].hypot(d[1])) {
            let u = unit(d);
            let from = PIPPER_RADIUS + TICKS_GAP;
            let at = [p[0] + u[0] * from, p[1] + u[1] * from, p[2]];
            out.push((TICKS, pose(at, d, [run, 0.0])));
        }
    }
    if l.drift[0].hypot(l.drift[1]) > 1e-6 {
        let u = unit(l.drift);
        let out_by = BOX_ACROSS + ARROW_GAP + ARROW_HALF[0];
        let at = [
            l.seam[0] + u[0] * out_by,
            l.seam[1] + u[1] * out_by,
            l.seam[2],
        ];
        out.push((DRIFT, pose(at, l.drift, ARROW_HALF)));
    }
    if let Some(t) = l.takeoff {
        out.push((
            TAKEOFF,
            pose(t.at, t.heading, [t.half.max(0.0), TAKEOFF_ACROSS]),
        ));
    }
    out
}

/// The guide's elements: world marks, fullscreen only as the readout is.
pub fn elements() -> Vec<Element> {
    let mark = |id, shape, longest| Element::channel(id, Kind::WorldMark { shape, longest });
    vec![
        mark(BOX, Shape::Bracket, BOX_LONGEST),
        mark(PIPPER, Shape::Pipper, ""),
        mark(TICKS, Shape::Ticks, ""),
        mark(DRIFT, Shape::Arrow, ""),
        mark(TAKEOFF, Shape::Bar, ""),
    ]
}

/// The box's label, the one verdict: before the jump the cue (`cue`,
/// [`cue_text`]), `Jump in 0.7 s`; in a fall the call, `Will bounce` or
/// why not ([`CALLS`]); with neither no label. No percentage: the bounce
/// rule's verdict is the one call.
pub fn box_text(call: Call, cue: Option<Cue>) -> Option<String> {
    if let Some(c) = cue {
        return Some(cue_text(c));
    }
    let label = match call {
        Call::TooShallow => TOO_SHALLOW,
        Call::TooHigh => TOO_HIGH,
        Call::OffEdge => OFF_EDGE,
        Call::NoDrift => NO_DRIFT,
        Call::Crouching => DONT_CROUCH,
        Call::WillBounce => WILL_BOUNCE,
        Call::OffLine | Call::LinedUp => return None,
    };
    Some(label.to_owned())
}

/// The guide says go: the fall will bounce, or the cue is `Jump now`.
pub fn go(call: Call, cue: Option<Cue>) -> bool {
    call == Call::WillBounce || cue == Some(Cue::Now)
}

/// The box's value: its label ([`box_text`]), in the go green when the
/// guide says go ([`go`]), else dim off the line.
pub fn box_value(call: Call, cue: Option<Cue>) -> Value {
    let style = if go(call, cue) {
        Style::Go
    } else if call == Call::OffLine {
        Style::Dim
    } else {
        Style::Normal
    };
    box_text(call, cue)
        .map_or_else(Value::shown, Value::text)
        .styled(style)
}

/// Sets the marks' values for `g` (`None`: all hidden).
pub fn set_values(out: &mut Values, g: Option<&Guide>) {
    let Some(g) = g else {
        for id in MARKS {
            out.set(id, Value::hidden());
        }
        return;
    };
    let shown = |on: bool, style: Style| {
        if on {
            Value::shown().styled(style)
        } else {
            Value::hidden()
        }
    };
    let lit = if go(g.call, g.cue) {
        Style::Go
    } else {
        Style::Normal
    };
    out.set(BOX, box_value(g.call, g.cue));
    out.set(PIPPER, shown(g.pipper, lit));
    out.set(TICKS, shown(g.ticks && g.pipper, Style::Normal));
    out.set(
        DRIFT,
        match g.drift {
            Some(true) => Value::shown(),
            Some(false) => Value::shown().styled(Style::Dim),
            None => Value::hidden(),
        },
    );
    out.set(TAKEOFF, shown(g.takeoff, lit));
}

/// The preview's moments of the guide: a name, what it shows and where
/// it lies in a sample scene (the seam's landing point at the origin, its
/// edge along +X, its drift +Y, away from the viewer).
pub fn samples() -> Vec<(&'static str, Guide, Layout)> {
    let scene = |impact: [f32; 2], takeoff: Option<[f32; 2]>| Layout {
        seam: [0.0; 3],
        along: [1.0, 0.0],
        half_along: BOX_ALONG,
        drift: [0.0, 1.0],
        impact: Some([impact[0], impact[1], 0.0]),
        takeoff: takeoff.map(|t| Line {
            at: [t[0], t[1], 0.0],
            heading: [0.0, 1.0],
            half: 0.03,
        }),
        strength: 1.0,
    };
    let fall = |call, ticks, drift| Guide {
        call,
        pipper: true,
        ticks,
        drift: Some(drift),
        cue: None,
        takeoff: false,
    };
    let ground = |call, ticks, drift, cue| Guide {
        call,
        pipper: true,
        ticks,
        drift: Some(drift),
        cue: Some(cue),
        takeoff: true,
    };
    vec![
        // Walking up the drift: a jump now would come down short; the
        // takeoff line is 0.35 ahead, 0.7 s at this pace.
        (
            "guide-approach",
            ground(Call::OffLine, true, true, Cue::In(7)),
            scene([0.1, -0.75], Some([0.1, -1.1])),
        ),
        // On the ground with the jump's landing in the box, a tick short
        // of the line's band.
        (
            "guide-lined-up",
            ground(Call::LinedUp, false, true, Cue::In(1)),
            scene([0.04, -0.03], Some([0.04, -1.1])),
        ),
        // Falling, caught, and the bounce rule says it bounces.
        (
            "guide-falling-caught",
            fall(Call::WillBounce, false, true),
            scene([-0.02, -0.02], None),
        ),
        // Falling, caught, and the sweep does not come on before the floor.
        (
            "guide-too-shallow",
            fall(Call::TooShallow, false, true),
            scene([0.02, -0.03], None),
        ),
        // Falling wide of the seam, drifting the wrong way.
        (
            "guide-off-line",
            fall(Call::OffLine, true, false),
            scene([-0.35, -0.3], None),
        ),
        // Falling, caught, and the sweep comes on a tick too high: caught
        // over the floor.
        (
            "guide-too-high",
            fall(Call::TooHigh, false, true),
            scene([0.05, -0.02], None),
        ),
        // Falling onto the edge with too little drift across it.
        (
            "guide-no-drift",
            fall(Call::NoDrift, false, false),
            scene([-0.06, -0.03], None),
        ),
        // On the takeoff line, walking the drift: press jump now; the
        // jump comes down in the box.
        (
            "guide-jump-now",
            ground(Call::LinedUp, false, true, Cue::Now),
            scene([0.0, -0.02], Some([0.0, -1.1])),
        ),
        // Standing to one side of the run-up: a jump here comes down
        // where it went up; the line is where a walk up the drift
        // should press it.
        (
            "guide-walk-to-line",
            ground(Call::OffLine, true, false, Cue::Walk),
            scene([-0.5, -0.9], Some([0.0, -1.1])),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::edges;

    fn layout(impact: Option<[f32; 2]>) -> Layout {
        Layout {
            seam: [10.0, 4.0, -3.2],
            along: [1.0, 0.0],
            half_along: BOX_ALONG,
            drift: [0.0, 1.0],
            impact: impact.map(|p| [p[0], p[1], -3.2]),
            takeoff: None,
            strength: 1.0,
        }
    }

    fn pose(l: &Layout, id: &str) -> Option<Pose> {
        poses(l).into_iter().find(|(i, _)| *i == id).map(|(_, p)| p)
    }

    /// The takeoff line lies across the run at its centre, as long across
    /// as `TAKEOFF_ACROSS` either side, its band along the run; with no
    /// line it has no pose.
    #[test]
    fn the_takeoff_line_lies_across_the_run() {
        assert!(pose(&layout(None), TAKEOFF).is_none());
        let line = Line {
            at: [10.0, 2.9, -0.2],
            heading: [0.0, 1.0],
            half: 0.025,
        };
        let l = Layout {
            takeoff: Some(line),
            ..layout(None)
        };
        let t = pose(&l, TAKEOFF).unwrap();
        assert_eq!(
            (t.at, t.along, t.half),
            (line.at, [0.0, 1.0], [0.025, TAKEOFF_ACROSS])
        );
        let e = edges(Shape::Bar, &t);
        // The line itself runs across the run (+Y), in X.
        assert!((e[0][0][1] - 2.9).abs() < 1e-5 && (e[0][1][1] - 2.9).abs() < 1e-5);
        assert!(((e[0][1][0] - e[0][0][0]).abs() - 2.0 * TAKEOFF_ACROSS).abs() < 1e-5);
        // A negative band is none.
        let l = Layout {
            takeoff: Some(Line { half: -1.0, ..line }),
            ..layout(None)
        };
        assert_eq!(pose(&l, TAKEOFF).unwrap().half[0], 0.0);
    }

    /// The box is on the seam's landing point, turned along its edge, as
    /// wide across as the catch; the drift arrow is outside it on the
    /// drift's side, pointing that way.
    #[test]
    fn the_box_is_on_the_seam_and_the_arrow_beside_it() {
        let l = layout(None);
        let b = pose(&l, BOX).unwrap();
        assert_eq!(
            (b.at, b.along, b.half),
            (l.seam, [1.0, 0.0], [BOX_ALONG, BOX_ACROSS])
        );
        assert!(pose(&l, PIPPER).is_none() && pose(&l, TICKS).is_none());
        let a = pose(&l, DRIFT).unwrap();
        assert_eq!(a.along, [0.0, 1.0]);
        // Its tail is ARROW_GAP outside the box's far side.
        let tail = a.at[1] - ARROW_HALF[0];
        assert!((tail - (4.0 + BOX_ACROSS + ARROW_GAP)).abs() < 1e-5);
        assert!(
            pose(
                &Layout {
                    drift: [0.0; 2],
                    ..l
                },
                DRIFT
            )
            .is_none()
        );
        // A seam along another heading turns the box with it.
        let turned = Layout {
            along: [0.0, 1.0],
            ..l
        };
        assert_eq!(pose(&turned, BOX).unwrap().along, [0.0, 1.0]);
    }

    /// The ticks start outside the pipper's ring, point at the box and
    /// shorten as the pipper closes on it, to nothing inside the catch.
    #[test]
    fn the_ticks_shorten_as_the_pipper_closes() {
        let far = layout(Some([10.0, 2.0]));
        let t = pose(&far, TICKS).unwrap();
        assert_eq!(t.half[0], TICKS_RUN, "capped far out");
        assert!((t.at[1] - (2.0 + PIPPER_RADIUS + TICKS_GAP)).abs() < 1e-5);
        assert_eq!(unit(t.along), [0.0, 1.0]);
        let run = |y: f32| pose(&layout(Some([10.0, y])), TICKS).map(|p| p.half[0]);
        let (a, b) = (run(3.6).unwrap(), run(3.8).unwrap());
        assert!(b < a && a < TICKS_RUN, "{a} {b}");
        assert!((b - (0.2 - PIPPER_RADIUS - TICKS_GAP)).abs() < 1e-5);
        // Inside the catch the run is under the least drawn.
        assert_eq!(run(4.0 - BOX_ACROSS), None);
        assert_eq!(ticks_run(f32::NAN), None);
        // Every tick ends before the box's centre.
        for e in edges(
            Shape::Ticks,
            &pose(&layout(Some([10.0, 3.6])), TICKS).unwrap(),
        ) {
            assert!(e[1][1] < 4.0);
        }
        // The pipper is on the landing.
        assert_eq!(pose(&far, PIPPER).unwrap().at, [10.0, 2.0, -3.2]);
    }

    /// The fade is every mark's strength, kept inside 0 to 1.
    #[test]
    fn the_fade_is_every_marks_strength() {
        let line = Line {
            at: [10.0, 2.9, -0.2],
            heading: [0.0, 1.0],
            half: 0.03,
        };
        for (s, want) in [(0.4, 0.4), (7.0, 1.0), (-1.0, 0.0), (f32::NAN, 0.0)] {
            let l = Layout {
                strength: s,
                takeoff: Some(line),
                ..layout(Some([10.0, 3.0]))
            };
            let all = poses(&l);
            assert_eq!(all.len(), MARKS.len());
            for (id, p) in all {
                assert_eq!(p.strength, want, "{id}");
                assert!(p.fault().is_none(), "{id}");
            }
        }
    }

    /// The values: dim off the line, lit lined up, the two calls of a
    /// fall with their words (the bounce in the go green, the pipper
    /// with it), ticks only with a pipper, the arrow lit by the drift,
    /// the takeoff line with its flag and green with `Jump now`.
    #[test]
    fn the_values_say_the_call() {
        let values = |g: Option<Guide>| {
            let mut v = Values::default();
            set_values(&mut v, g.as_ref());
            v
        };
        let g = |call| Guide {
            call,
            pipper: true,
            ticks: true,
            drift: Some(false),
            cue: None,
            takeoff: false,
        };
        let off = values(Some(g(Call::OffLine)));
        let b = off.get(BOX).unwrap();
        assert!(b.shown && b.style == Style::Dim && b.text.is_empty());
        assert!(off.get(TICKS).unwrap().shown);
        assert!(!off.get(TAKEOFF).unwrap().shown);
        assert_eq!(off.get(DRIFT).unwrap().style, Style::Dim);
        let lined = values(Some(g(Call::LinedUp)));
        assert_eq!(lined.get(BOX).unwrap().style, Style::Normal);
        assert!(lined.get(BOX).unwrap().text.is_empty());
        let shallow = values(Some(g(Call::TooShallow)));
        assert_eq!(shallow.get(BOX).unwrap().text, TOO_SHALLOW);
        assert_eq!(shallow.get(BOX).unwrap().style, Style::Normal);
        assert_eq!(shallow.get(PIPPER).unwrap().style, Style::Normal);
        let will = values(Some(g(Call::WillBounce)));
        assert_eq!(will.get(BOX).unwrap().text, WILL_BOUNCE);
        assert_eq!(will.get(BOX).unwrap().style, Style::Go);
        assert_eq!(will.get(PIPPER).unwrap().style, Style::Go);
        let no_pipper = values(Some(Guide {
            pipper: false,
            ..g(Call::OffLine)
        }));
        assert!(!no_pipper.get(PIPPER).unwrap().shown && !no_pipper.get(TICKS).unwrap().shown);
        let lit = values(Some(Guide {
            drift: Some(true),
            ..g(Call::LinedUp)
        }));
        assert_eq!(lit.get(DRIFT).unwrap().style, Style::Normal);
        assert!(
            !values(Some(Guide {
                drift: None,
                ..g(Call::LinedUp)
            }))
            .get(DRIFT)
            .unwrap()
            .shown
        );
        // The takeoff line: lit while counting, green on it with the box
        // and the pipper.
        let counting = values(Some(Guide {
            cue: Some(Cue::In(7)),
            takeoff: true,
            ..g(Call::OffLine)
        }));
        let t = counting.get(TAKEOFF).unwrap();
        assert!(t.shown && t.style == Style::Normal);
        assert_eq!(counting.get(BOX).unwrap().style, Style::Dim);
        let now = values(Some(Guide {
            cue: Some(Cue::Now),
            takeoff: true,
            ..g(Call::OffLine)
        }));
        for id in [BOX, PIPPER, TAKEOFF] {
            assert_eq!(now.get(id).unwrap().style, Style::Go, "{id}");
        }
        let none = values(None);
        assert!(MARKS.iter().all(|id| !none.get(id).unwrap().shown));
        for l in CALLS.iter().chain(&[WALK_TO_LINE, JUMP_NOW]) {
            assert!(l.chars().count() <= BOX_LONGEST.chars().count());
        }
    }

    /// The box's label is the one verdict: in a fall the call alone, with
    /// no percentage; before the jump the cue in its place, whatever the
    /// call; no label with neither; and none wider than the longest.
    #[test]
    fn the_label_is_the_call_or_the_cue() {
        let text = |call| box_text(call, None);
        assert_eq!(text(Call::WillBounce).as_deref(), Some(WILL_BOUNCE));
        assert_eq!(text(Call::TooShallow).as_deref(), Some(TOO_SHALLOW));
        assert_eq!(text(Call::TooHigh).as_deref(), Some(TOO_HIGH));
        assert_eq!(text(Call::OffEdge).as_deref(), Some(OFF_EDGE));
        assert_eq!(text(Call::NoDrift).as_deref(), Some(NO_DRIFT));
        assert_eq!(text(Call::Crouching).as_deref(), Some(DONT_CROUCH));
        // Off the line in a fall: no label, not a bare percentage.
        assert_eq!(text(Call::OffLine), None);
        assert_eq!(text(Call::LinedUp), None);
        for call in [
            Call::OffLine,
            Call::LinedUp,
            Call::TooShallow,
            Call::TooHigh,
            Call::OffEdge,
            Call::NoDrift,
            Call::Crouching,
            Call::WillBounce,
        ] {
            if let Some(t) = text(call) {
                assert!(!t.contains('%'), "{t}");
                assert!(t.chars().count() <= BOX_LONGEST.chars().count(), "{t}");
            }
        }
        // Before the jump the cue is the label, whatever the call.
        for call in [Call::OffLine, Call::LinedUp] {
            let cue = |c| box_text(call, Some(c));
            assert_eq!(cue(Cue::Walk).as_deref(), Some("Walk to line"));
            assert_eq!(cue(Cue::In(7)).as_deref(), Some("Jump in 0.7 s"));
            assert_eq!(cue(Cue::In(30)).as_deref(), Some("Jump in 3.0 s"));
            assert_eq!(cue(Cue::Now).as_deref(), Some("Jump now"));
            assert_eq!(cue(Cue::Label("Climb up")).as_deref(), Some("Climb up"));
        }
        for t in [0, 1, 9, 10, 30, 99, 200] {
            let s = cue_text(Cue::In(t));
            assert!(s.chars().count() <= BOX_LONGEST.chars().count(), "{s}");
        }
        assert_eq!(cue_text(Cue::In(200)), "Jump in 9.9 s");
        // The value: the label in the call's style, green for `Jump now`.
        let v = box_value(Call::WillBounce, None);
        assert_eq!(
            (v.text.as_str(), v.style, v.shown),
            ("Will bounce", Style::Go, true)
        );
        let v = box_value(Call::TooShallow, None);
        assert_eq!((v.text.as_str(), v.style), ("Too shallow", Style::Normal));
        let v = box_value(Call::LinedUp, Some(Cue::In(4)));
        assert_eq!((v.text.as_str(), v.style), ("Jump in 0.4 s", Style::Normal));
        let v = box_value(Call::OffLine, Some(Cue::Walk));
        assert_eq!(
            (v.text.as_str(), v.style, v.shown),
            ("Walk to line", Style::Dim, true)
        );
        let v = box_value(Call::OffLine, Some(Cue::Now));
        assert_eq!((v.text.as_str(), v.style), ("Jump now", Style::Go));
        let v = box_value(Call::OffLine, Some(Cue::Label("Walk off")));
        assert_eq!((v.text.as_str(), v.style), ("Walk off", Style::Dim));
        let v = box_value(Call::OffLine, None);
        assert!(v.shown && v.text.is_empty() && v.style == Style::Dim);
        // Through the values: the call is the label.
        let mut out = Values::default();
        let g = Guide {
            call: Call::WillBounce,
            pipper: true,
            ticks: false,
            drift: Some(true),
            cue: None,
            takeoff: false,
        };
        set_values(&mut out, Some(&g));
        assert_eq!(out.get(BOX).unwrap().text, "Will bounce");
        assert_eq!(out.get(PIPPER).unwrap().style, Style::Go);
    }

    /// The count is in tenths of a second, rounded up: 1 to 3 ticks are
    /// 0.1, 4 ticks 0.2, a second 1.0, and it stops at 9.9.
    #[test]
    fn the_count_is_tenths_rounded_up() {
        for (ticks, want) in [
            (0, 0),
            (1, 1),
            (3, 1),
            (4, 2),
            (21, 7),
            (30, 10),
            (90, 30),
            (5000, 99),
        ] {
            assert_eq!(tenths(ticks), want, "{ticks}");
        }
    }

    /// Every sample gives each mark its values show a pose, and the
    /// calls of a caught landing have their pipper inside the box. A
    /// fall has a call and no takeoff; on the ground the cue and the
    /// takeoff line are there instead.
    #[test]
    fn every_sample_poses_what_it_shows() {
        let all = samples();
        assert_eq!(all.len(), 9);
        for (name, g, l) in all {
            match g.cue {
                None => {
                    assert!(g.call != Call::LinedUp, "{name}");
                    assert!(!g.takeoff, "{name}");
                }
                Some(_) => {
                    assert!(g.takeoff, "{name}");
                    assert!(matches!(g.call, Call::OffLine | Call::LinedUp), "{name}");
                }
            }
            let mut v = Values::default();
            set_values(&mut v, Some(&g));
            let p = poses(&l);
            for id in MARKS {
                let has = p.iter().any(|(i, _)| *i == id);
                assert_eq!(v.get(id).unwrap().shown, has, "{name} {id}");
            }
            let at = l.impact.unwrap();
            let inside = at[0].abs() <= BOX_ALONG && at[1].abs() <= BOX_ACROSS;
            assert_eq!(inside, g.call != Call::OffLine, "{name}");
        }
    }
}
