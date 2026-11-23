//! The superbounce approach guide (docs/game/hud-kit.md, "The approach
//! guide"; docs/game/superbounce.md, "The approach guide"): four world
//! marks of the readout HUD ([`super::superbounce_readout`]) on the seam
//! the player is going for, so the line-up is seen on the geometry and
//! not guessed.
//!
//! - `sbr.guide.box`: a bracket on the seam's landing point, turned along
//!   the seam's edge: the catch across it, [`BOX_ALONG`] along it. Dim
//!   while the predicted landing is off the seam, lit when it is lined
//!   up, and during a fall `Too shallow` or, green, `Will bounce`
//!   ([`Call`]). Its label ends with the bounce's predicted percentage,
//!   and before the jump is the percentage alone ([`box_text`]).
//! - `sbr.guide.pipper`: where the player's path touches down: on the
//!   ground where a jump now would land, in the air the landing itself.
//! - `sbr.guide.ticks`: closure cues, short dashes from the pipper toward
//!   the box that shorten as the two close and go when the landing is
//!   caught.
//! - `sbr.guide.drift`: an arrow beside the box for the way the seam
//!   wants the landing to drift, lit while the player moves that way.
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
/// The guide's marks, in draw order.
pub const MARKS: [&str; 4] = [BOX, PIPPER, TICKS, DRIFT];

/// The box's labels during a fall.
pub const WILL_BOUNCE: &str = "Will bounce";
pub const TOO_SHALLOW: &str = "Too shallow";
/// The widest label: the wider of the two with the widest percentage.
pub const BOX_LONGEST: &str = "Too shallow 100%";

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

/// What the box says of the predicted landing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Call {
    /// Not within the seam's catch.
    OffLine,
    /// Within the catch, with no call on the bounce: on the ground, or
    /// in a fall the depth rule has nothing to say about.
    LinedUp,
    /// In a fall: within the catch, and the last free tick ends too high
    /// over the seam.
    TooShallow,
    /// In a fall: primed, within the landing window, and the last free
    /// tick ends deep enough.
    WillBounce,
}

/// What the guide shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guide {
    pub call: Call,
    /// The predicted landing is known and by the seam: the pipper.
    pub pipper: bool,
    /// The closure cues (the landing is known and not caught).
    pub ticks: bool,
    /// The seam has a drift: `Some(true)` while the player moves its way.
    pub drift: Option<bool>,
    /// The bounce's predicted chance, a whole percentage ([`percent`]):
    /// before the jump the share of the next half second's jumps that
    /// bounce, in a fall how sure the call is. `None` where there is
    /// none to give (not primed, a seam with no drift).
    pub chance: Option<u8>,
}

/// A chance of 0 to 1 as a whole percentage, 0 to 100; one that is not
/// a number is 0.
pub fn percent(chance: f32) -> u8 {
    if chance.is_finite() {
        (chance.clamp(0.0, 1.0) * 100.0).round() as u8
    } else {
        0
    }
}

/// Where the guide lies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The seam's landing point: the box's centre.
    pub seam: Vec3,
    /// The seam's edge direction, flat.
    pub along: [f32; 2],
    /// The box's half length along the edge.
    pub half_along: f32,
    /// The seam's drift, flat; zero for none.
    pub drift: [f32; 2],
    /// The predicted landing, on the seam's floor.
    pub impact: Option<Vec3>,
    /// 0 to 1: the guide's fade.
    pub strength: f32,
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
    let strength = if l.strength.is_finite() { l.strength.clamp(0.0, 1.0) } else { 0.0 };
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
        let at = [l.seam[0] + u[0] * out_by, l.seam[1] + u[1] * out_by, l.seam[2]];
        out.push((DRIFT, pose(at, l.drift, ARROW_HALF)));
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
    ]
}

/// The box's label for `call` with the chance `chance`: a fall's call
/// and its percentage, `Will bounce 82%` or `Too shallow 30%`; with no
/// call (before the jump, or off the line) the percentage alone, `62%`;
/// with no chance the call alone, and with neither no label.
pub fn box_text(call: Call, chance: Option<u8>) -> Option<String> {
    let word = match call {
        Call::TooShallow => Some(TOO_SHALLOW),
        Call::WillBounce => Some(WILL_BOUNCE),
        Call::OffLine | Call::LinedUp => None,
    };
    let chance = chance.map(|c| c.min(100));
    match (word, chance) {
        (Some(w), Some(c)) => Some(format!("{w} {c}%")),
        (Some(w), None) => Some(w.to_owned()),
        (None, Some(c)) => Some(format!("{c}%")),
        (None, None) => None,
    }
}

/// The box's value for `call` with the chance `chance`: its label
/// ([`box_text`]), dim off the line and in the go green when the fall
/// will bounce.
pub fn box_value(call: Call, chance: Option<u8>) -> Value {
    let style = match call {
        Call::OffLine => Style::Dim,
        Call::LinedUp | Call::TooShallow => Style::Normal,
        Call::WillBounce => Style::Go,
    };
    box_text(call, chance)
        .map_or_else(Value::shown, Value::text)
        .styled(style)
}

/// Sets the four marks' values for `g` (`None`: all hidden).
pub fn set_values(out: &mut Values, g: Option<&Guide>) {
    let Some(g) = g else {
        for id in MARKS {
            out.set(id, Value::hidden());
        }
        return;
    };
    let shown = |on: bool, style: Style| {
        if on { Value::shown().styled(style) } else { Value::hidden() }
    };
    let will = g.call == Call::WillBounce;
    out.set(BOX, box_value(g.call, g.chance));
    out.set(PIPPER, shown(g.pipper, if will { Style::Go } else { Style::Normal }));
    out.set(TICKS, shown(g.ticks && g.pipper, Style::Normal));
    out.set(
        DRIFT,
        match g.drift {
            Some(true) => Value::shown(),
            Some(false) => Value::shown().styled(Style::Dim),
            None => Value::hidden(),
        },
    );
}

/// The preview's moments of the guide: a name, what it shows and where
/// it lies in a sample scene (the seam's landing point at the origin, its
/// edge along +X, its drift +Y, away from the viewer).
pub fn samples() -> Vec<(&'static str, Guide, Layout)> {
    let scene = |impact: [f32; 2]| Layout {
        seam: [0.0; 3],
        along: [1.0, 0.0],
        half_along: BOX_ALONG,
        drift: [0.0, 1.0],
        impact: Some([impact[0], impact[1], 0.0]),
        strength: 1.0,
    };
    let guide = |call, ticks, drift, chance| Guide {
        call,
        pipper: true,
        ticks,
        drift: Some(drift),
        chance: Some(chance),
    };
    vec![
        // Walking up: a jump now would come down short and to one side;
        // two of the next 16 ticks' jumps would bounce.
        ("guide-approach", guide(Call::OffLine, true, false, 13), scene([0.3, -0.75])),
        // On the ground with the jump's landing in the box: ten of 16.
        ("guide-lined-up", guide(Call::LinedUp, false, true, 62), scene([0.04, -0.03])),
        // Falling, caught, the last free tick deep enough.
        ("guide-falling-caught", guide(Call::WillBounce, false, true, 82), scene([-0.02, -0.02])),
        // Falling, caught, the last free tick too high.
        ("guide-too-shallow", guide(Call::TooShallow, false, true, 30), scene([0.02, -0.03])),
        // Falling wide of the seam, drifting the wrong way.
        ("guide-off-line", guide(Call::OffLine, true, false, 5), scene([-0.35, -0.3])),
        // Falling, caught, the last free tick just under the depth cut.
        ("guide-on-the-cut", guide(Call::WillBounce, false, true, 52), scene([0.05, -0.02])),
        // Falling deep and centred onto a seam no run has landed on: the
        // scan-only cap.
        ("guide-scan-only", guide(Call::WillBounce, false, true, 60), scene([-0.06, -0.03])),
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
            strength: 1.0,
        }
    }

    fn pose(l: &Layout, id: &str) -> Option<Pose> {
        poses(l).into_iter().find(|(i, _)| *i == id).map(|(_, p)| p)
    }

    /// The box is on the seam's landing point, turned along its edge, as
    /// wide across as the catch; the drift arrow is outside it on the
    /// drift's side, pointing that way.
    #[test]
    fn the_box_is_on_the_seam_and_the_arrow_beside_it() {
        let l = layout(None);
        let b = pose(&l, BOX).unwrap();
        assert_eq!((b.at, b.along, b.half), (l.seam, [1.0, 0.0], [BOX_ALONG, BOX_ACROSS]));
        assert!(pose(&l, PIPPER).is_none() && pose(&l, TICKS).is_none());
        let a = pose(&l, DRIFT).unwrap();
        assert_eq!(a.along, [0.0, 1.0]);
        // Its tail is ARROW_GAP outside the box's far side.
        let tail = a.at[1] - ARROW_HALF[0];
        assert!((tail - (4.0 + BOX_ACROSS + ARROW_GAP)).abs() < 1e-5);
        assert!(pose(&Layout { drift: [0.0; 2], ..l }, DRIFT).is_none());
        // A seam along another heading turns the box with it.
        let turned = Layout { along: [0.0, 1.0], ..l };
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
        for e in edges(Shape::Ticks, &pose(&layout(Some([10.0, 3.6])), TICKS).unwrap()) {
            assert!(e[1][1] < 4.0);
        }
        // The pipper is on the landing.
        assert_eq!(pose(&far, PIPPER).unwrap().at, [10.0, 2.0, -3.2]);
    }

    /// The fade is every mark's strength, kept inside 0 to 1.
    #[test]
    fn the_fade_is_every_marks_strength() {
        for (s, want) in [(0.4, 0.4), (7.0, 1.0), (-1.0, 0.0), (f32::NAN, 0.0)] {
            let l = Layout { strength: s, ..layout(Some([10.0, 3.0])) };
            let all = poses(&l);
            assert_eq!(all.len(), 4);
            for (id, p) in all {
                assert_eq!(p.strength, want, "{id}");
                assert!(p.fault().is_none(), "{id}");
            }
        }
    }

    /// The values: dim off the line, lit lined up, the two calls of a
    /// fall with their words (the bounce in the go green, the pipper
    /// with it), ticks only with a pipper, the arrow lit by the drift.
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
            chance: None,
        };
        let off = values(Some(g(Call::OffLine)));
        let b = off.get(BOX).unwrap();
        assert!(b.shown && b.style == Style::Dim && b.text.is_empty());
        assert!(off.get(TICKS).unwrap().shown);
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
        let no_pipper = values(Some(Guide { pipper: false, ..g(Call::OffLine) }));
        assert!(!no_pipper.get(PIPPER).unwrap().shown && !no_pipper.get(TICKS).unwrap().shown);
        let lit = values(Some(Guide { drift: Some(true), ..g(Call::LinedUp) }));
        assert_eq!(lit.get(DRIFT).unwrap().style, Style::Normal);
        assert!(!values(Some(Guide { drift: None, ..g(Call::LinedUp) })).get(DRIFT).unwrap().shown);
        let none = values(None);
        assert!(MARKS.iter().all(|id| !none.get(id).unwrap().shown));
        for l in [WILL_BOUNCE, TOO_SHALLOW] {
            assert!(l.chars().count() <= BOX_LONGEST.chars().count());
        }
    }

    /// The box's label: a fall's call with its percentage, before the
    /// jump the percentage alone, in the call's style; no label with
    /// neither; and none wider than the longest.
    #[test]
    fn the_label_ends_with_the_percentage() {
        let text = |call, chance| box_text(call, chance);
        assert_eq!(text(Call::WillBounce, Some(82)).as_deref(), Some("Will bounce 82%"));
        assert_eq!(text(Call::TooShallow, Some(30)).as_deref(), Some("Too shallow 30%"));
        assert_eq!(text(Call::LinedUp, Some(62)).as_deref(), Some("62%"));
        assert_eq!(text(Call::OffLine, Some(0)).as_deref(), Some("0%"));
        assert_eq!(text(Call::WillBounce, None).as_deref(), Some(WILL_BOUNCE));
        assert_eq!(text(Call::TooShallow, None).as_deref(), Some(TOO_SHALLOW));
        assert_eq!(text(Call::LinedUp, None), None);
        assert_eq!(text(Call::OffLine, None), None);
        // A percentage over 100 is 100, and nothing is wider than the
        // longest.
        assert_eq!(text(Call::TooShallow, Some(250)).as_deref(), Some(BOX_LONGEST));
        for call in [Call::OffLine, Call::LinedUp, Call::TooShallow, Call::WillBounce] {
            for c in [0, 5, 50, 100, 255] {
                let t = text(call, Some(c)).unwrap();
                assert!(t.chars().count() <= BOX_LONGEST.chars().count(), "{t}");
                assert!(t.ends_with('%'), "{t}");
            }
        }
        // The value: the label in the call's style.
        let v = box_value(Call::WillBounce, Some(82));
        assert_eq!((v.text.as_str(), v.style, v.shown), ("Will bounce 82%", Style::Go, true));
        let v = box_value(Call::TooShallow, Some(30));
        assert_eq!((v.text.as_str(), v.style), ("Too shallow 30%", Style::Normal));
        let v = box_value(Call::LinedUp, Some(62));
        assert_eq!((v.text.as_str(), v.style), ("62%", Style::Normal));
        let v = box_value(Call::OffLine, Some(13));
        assert_eq!((v.text.as_str(), v.style, v.shown), ("13%", Style::Dim, true));
        let v = box_value(Call::OffLine, None);
        assert!(v.shown && v.text.is_empty() && v.style == Style::Dim);
        // Through the values, with the guide's chance.
        let mut out = Values::default();
        let g = Guide {
            call: Call::WillBounce,
            pipper: true,
            ticks: false,
            drift: Some(true),
            chance: Some(60),
        };
        set_values(&mut out, Some(&g));
        assert_eq!(out.get(BOX).unwrap().text, "Will bounce 60%");
        assert_eq!(out.get(PIPPER).unwrap().style, Style::Go);
    }

    /// A chance is a whole percentage, rounded, kept inside 0 to 100.
    #[test]
    fn a_chance_is_a_whole_percentage() {
        for (c, want) in [
            (0.0, 0),
            (0.004, 0),
            (0.125, 13),
            (0.375, 38),
            (0.5, 50),
            (0.6, 60),
            (0.944, 94),
            (1.0, 100),
            (3.0, 100),
            (-1.0, 0),
            (f32::NAN, 0),
            (f32::INFINITY, 0),
        ] {
            assert_eq!(percent(c), want, "{c}");
        }
    }

    /// Every sample gives each mark its values show a pose, and the
    /// calls of a caught landing have their pipper inside the box.
    #[test]
    fn every_sample_poses_what_it_shows() {
        let all = samples();
        assert_eq!(all.len(), 7);
        for (name, g, l) in all {
            // Every sample is primed on a seam with a drift: it has a
            // chance, and a fall's call agrees with it.
            let c = g.chance.expect(name);
            assert!(g.call != Call::WillBounce || c >= 50, "{name}");
            assert!(g.call != Call::TooShallow || c <= 50, "{name}");
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
