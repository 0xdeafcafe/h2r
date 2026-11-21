//! The superbounce figures drawn on the map (docs/game/superbounce.md,
//! "Crouch spots in the world" and "Landings"): for each crouch spot a
//! footprint ring, the lip it gets under and floor chevrons the way in and
//! out; for each bounce strip the slot as a closed ribbon with a tick at
//! the stand point and a post to the height to fall from; while primed and
//! falling, the predicted landing and the speed it touches down at. All of
//! it in the HUD's colour, labels in the HUD's font ([`Look`]).
//!
//! The builders are pure: [`ring`], [`chevrons`], [`slot_marks`],
//! [`post`], [`cross`] and [`speed_fill`] make geometry, [`figures`] puts a
//! frame's together from the nearest spots ([`Item`]) and the player's
//! [`Scene`], and [`frame`] reads both from the mode and adds the prompts
//! (`crate::superbounce_prompts`). What the scanner and the mode's state
//! give is read in one place, [`contract`].

use std::sync::OnceLock;
use std::time::Instant;

use engine::GuestMemory;
use gfx::world_lines::{Chevrons, Fade, Glow, LineStyle, Pulse, Ribbon};
use superbounce::{Kind, Spot, Vec3};

use crate::superbounce_font::{self as hud, Look};
use crate::superbounce_mode::{BOUNCE_LABELS, DIM, FADE_FAR, FADE_NEAR, MAX_LINES, State};
use crate::superbounce_prompts::{self as prompts, Callout, FLAT_FALL, whole_metres};

/// The biped's capsule radius: the footprint and the landing ring.
pub const CAPSULE_RADIUS: f32 = 0.175;
/// Floor figures sit this far above the floor.
pub const LIFT: f32 = 0.02;
/// Segments of a full ring.
pub const RING_SEGMENTS: usize = 24;
/// Crouch spots that get the lip and the way: the nearest few.
pub const WAYS: usize = 3;
/// The nearest crouch spot within this distance carries `Crouch`.
pub const CROUCH_LABEL_RANGE: f32 = 3.0;
/// The charge's phase is shown on the nearest crouch spot within this
/// horizontal distance of the player.
pub const PHASE_RANGE: f32 = 1.0;
/// A `likely` crouch spot's strength; a `proven` one's is 1.
pub const LIKELY: f32 = 0.5;
/// Crouch figures' strength while primed: a crouch would lose the charge.
pub const PRIMED_CROUCH: f32 = 0.2;
/// A strip's strength when the player's floor is too low to reach it.
pub const CLIMB: f32 = 0.5;
/// The way's chevrons run between these distances out from the stand.
pub const WAY_FAR: f32 = 1.2;
pub const WAY_NEAR: f32 = CAPSULE_RADIUS + 0.1;
/// World units between the way's chevrons, and their size.
pub const CHEVRON_SPACING: f32 = 0.25;
pub const CHEVRON_SIZE: f32 = 0.12;
/// World units a second the way's chevrons travel.
pub const CHEVRON_SPEED: f32 = 0.4;
/// Half the length of each arm of the cross at a post's top.
pub const POST_TICK: f32 = 0.08;
/// The speed bar: its height, and how far beside the landing it stands.
pub const BAR_HEIGHT: f32 = 0.5;
pub const BAR_SIDE: f32 = 0.35;
/// `Crouch` floats this far above the lip (or [`CROUCH_LABEL_HEIGHT`]
/// above the stand point without one).
pub const CROUCH_LABEL_RAISE: f32 = 0.15;
pub const CROUCH_LABEL_HEIGHT: f32 = 0.8;
/// The world labels' size: the HUD's own (`LabelFace::Hud` at 2).
pub const LABEL_SCALE: f32 = 2.0;

/// How far through the charge the player is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Near,
    Under,
    Held,
    Primed,
    Lost,
}

/// Whether the predicted landing is in the slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    OnSlot,
    /// The assist can still bring it in.
    Fixable,
    Miss,
}

/// Where a primed, falling player will land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    pub at: Vec3,
    pub verdict: Verdict,
    /// The touchdown speed, units/s.
    pub speed: f32,
    /// The touchdown speed the strip needs.
    pub need: f32,
}

/// A bounce strip as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strip {
    pub edge: [Vec3; 2],
    pub stand: Vec3,
    pub fall: f32,
    /// The slot's width.
    pub width: f32,
    pub drift: [f32; 2],
}

/// A crouch spot as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Crouch {
    pub stand: Vec3,
    /// Unit, flat: from the stand point toward the standing room.
    pub out: [f32; 2],
    /// The edges the standing capsule hits, at their own height.
    pub lip: Vec<[Vec3; 2]>,
    pub proven: bool,
}

/// One spot near the player, nearest first.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Strip(Strip),
    Crouch(Crouch),
}

/// The player, for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    /// The unit's origin, at its feet.
    pub at: Vec3,
    pub airborne: bool,
    /// Moving down.
    pub falling: bool,
    pub primed: bool,
    pub phase: Option<Phase>,
    pub landing: Option<Landing>,
    /// The edge of the strip the assist would steer onto.
    pub target: Option<[Vec3; 2]>,
    /// Seconds on a steady clock, for the chevrons' flow.
    pub seconds: f32,
}

/// What the scanner's spots and the mode's state carry, read in one
/// place (the fields other lanes add: docs/game/superbounce.md, "Crouch
/// spots: the stricter rule", "Charge", "Landings").
pub mod contract {
    use super::*;
    use crate::superbounce_mode as mode;

    pub fn strip(s: &Spot) -> Strip {
        Strip {
            edge: s.edge,
            stand: s.stand,
            fall: s.fall,
            width: s.width,
            drift: s.drift,
        }
    }

    pub fn crouch(s: &Spot) -> Crouch {
        Crouch {
            stand: s.stand,
            out: [s.out[0], s.out[1]],
            lip: s
                .lip
                .iter()
                .copied()
                .filter(|e| *e != [[0.0; 3]; 2])
                .collect(),
            proven: matches!(s.status, superbounce::Status::Proven),
        }
    }

    pub fn phase(s: &State) -> Option<Phase> {
        let c = s.charge.as_ref()?;
        Some(match c.phase {
            mode::ChargePhase::Near => Phase::Near,
            mode::ChargePhase::Under => Phase::Under,
            mode::ChargePhase::Held => Phase::Held,
            mode::ChargePhase::Primed => Phase::Primed,
            mode::ChargePhase::Lost => Phase::Lost,
        })
    }

    pub fn landing(s: &State) -> Option<Landing> {
        let p = s.landing.as_ref()?;
        Some(Landing {
            at: p.at,
            verdict: match p.verdict {
                mode::Course::OnSlot => Verdict::OnSlot,
                mode::Course::Fixable => Verdict::Fixable,
                mode::Course::Miss => Verdict::Miss,
            },
            speed: p.speed,
            need: p.need,
        })
    }
}

// ---- geometry ----

fn flat_unit(v: [f32; 2]) -> Option<[f32; 2]> {
    let len = v[0].hypot(v[1]);
    (len > 1e-6 && len.is_finite()).then(|| [v[0] / len, v[1] / len])
}

fn horizontal(a: Vec3, b: Vec3) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn distance3(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn offset(p: Vec3, d: [f32; 2], by: f32) -> Vec3 {
    [p[0] + d[0] * by, p[1] + d[1] * by, p[2]]
}

fn up(p: Vec3, by: f32) -> Vec3 {
    [p[0], p[1], p[2] + by]
}

fn middle([a, b]: [Vec3; 2]) -> Vec3 {
    [
        (a[0] + b[0]) / 2.0,
        (a[1] + b[1]) / 2.0,
        (a[2] + b[2]) / 2.0,
    ]
}

/// A flat ring of `radius` round `centre`, [`LIFT`] above it, as
/// [`RING_SEGMENTS`] edges end to end. `open` keeps every other one: a
/// broken ring.
pub fn ring(centre: Vec3, radius: f32, open: bool) -> Vec<[Vec3; 2]> {
    let point = |i: usize| {
        let a = std::f32::consts::TAU * (i % RING_SEGMENTS) as f32 / RING_SEGMENTS as f32;
        [
            centre[0] + radius * a.cos(),
            centre[1] + radius * a.sin(),
            centre[2] + LIFT,
        ]
    };
    (0..RING_SEGMENTS)
        .filter(|i| !open || i % 2 == 0)
        .map(|i| [point(i), point(i + 1)])
        .collect()
}

/// A flat cross of arms `half` long each way at `centre`, [`LIFT`] up,
/// on the diagonals.
pub fn cross(centre: Vec3, half: f32) -> Vec<[Vec3; 2]> {
    let c = up(centre, LIFT);
    let d = half * std::f32::consts::FRAC_1_SQRT_2;
    vec![
        [[c[0] - d, c[1] - d, c[2]], [c[0] + d, c[1] + d, c[2]]],
        [[c[0] - d, c[1] + d, c[2]], [c[0] + d, c[1] - d, c[2]]],
    ]
}

/// Arrowheads lying flat from `from` toward `to`, pointing that way,
/// `spacing` apart and `size` long, [`LIFT`] up. `phase` (0..1, of a
/// spacing) moves them along, wrapping at the end: flowing as it grows.
/// Each is two edges, wing to tip. None along a run with no length.
pub fn chevrons(from: Vec3, to: Vec3, spacing: f32, size: f32, phase: f32) -> Vec<[Vec3; 2]> {
    let length = horizontal(from, to);
    let Some(u) = flat_unit([to[0] - from[0], to[1] - from[1]]) else {
        return Vec::new();
    };
    if spacing.is_nan() || spacing <= 0.0 {
        return Vec::new();
    }
    let count = ((length / spacing) as usize).max(1);
    let span = count as f32 * spacing;
    let phase = if phase.is_finite() {
        phase.rem_euclid(1.0)
    } else {
        0.0
    };
    let n = [-u[1], u[0]];
    let mut out = Vec::with_capacity(count * 2);
    for i in 0..count {
        let t = ((i as f32 + 0.5 + phase) * spacing).rem_euclid(span) * length / span;
        let z = from[2] + (to[2] - from[2]) * t / length.max(1e-6) + LIFT;
        let c = [from[0] + u[0] * t, from[1] + u[1] * t, z];
        let tip = offset(c, u, size * 0.5);
        let back = offset(c, u, -size * 0.5);
        out.push([offset(back, n, size * 0.7), tip]);
        out.push([offset(back, n, -size * 0.7), tip]);
    }
    out
}

/// The slot's outline over its ribbon: a cap across each end of `edge`,
/// `width` long, and a tick across at `stand`. Without an edge length,
/// only the tick, along X.
pub fn slot_marks(edge: [Vec3; 2], stand: Vec3, width: f32) -> Vec<[Vec3; 2]> {
    let [a, b] = edge;
    let half = width * 0.5;
    let along = flat_unit([b[0] - a[0], b[1] - a[1]]);
    let n = along.map_or([1.0, 0.0], |u| [-u[1], u[0]]);
    let across = |p: Vec3| {
        let p = up(p, LIFT);
        [offset(p, n, half), offset(p, n, -half)]
    };
    let mut out = Vec::with_capacity(3);
    if along.is_some() {
        out.push(across(a));
        out.push(across(b));
    }
    out.push(across(stand));
    out
}

/// The post at a strip's stand point to the height to fall from, with a
/// cross at its top along and across `edge`: the edges and the top. None
/// for a strip a standing jump reaches ([`FLAT_FALL`]).
pub fn post(stand: Vec3, fall: f32, edge: [Vec3; 2]) -> Option<(Vec<[Vec3; 2]>, Vec3)> {
    if !(fall > FLAT_FALL && fall.is_finite()) {
        return None;
    }
    let top = up(stand, fall);
    let [a, b] = edge;
    let u = flat_unit([b[0] - a[0], b[1] - a[1]]).unwrap_or([1.0, 0.0]);
    let n = [-u[1], u[0]];
    Some((
        vec![
            [stand, top],
            [offset(top, u, -POST_TICK), offset(top, u, POST_TICK)],
            [offset(top, n, -POST_TICK), offset(top, n, POST_TICK)],
        ],
        top,
    ))
}

/// How full the speed bar is: the touchdown speed over the strip's need,
/// 0..1. Full when the strip needs nothing; empty for no number.
pub fn speed_fill(speed: f32, need: f32) -> f32 {
    if !speed.is_finite() {
        0.0
    } else if need.is_nan() || need <= 0.0 {
        1.0
    } else {
        (speed / need).clamp(0.0, 1.0)
    }
}

/// Whether feet at `feet` are high enough to fall onto `strip`: it needs
/// no fall, or they are at least its fall above its stand point.
pub fn high_enough(feet: f32, strip: &Strip) -> bool {
    strip.fall <= FLAT_FALL || feet - strip.stand[2] >= strip.fall
}

/// A strip's strength: dimmed unless primed, halved when the player must
/// climb to fall onto it.
pub fn strip_strength(primed: bool, high_enough: bool) -> f32 {
    let p = if primed { 1.0 } else { DIM };
    if high_enough { p } else { p * CLIMB }
}

/// A crouch figure's strength: half for a `likely` spot, a fifth of that
/// while primed.
pub fn crouch_strength(proven: bool, primed: bool) -> f32 {
    let k = if proven { 1.0 } else { LIKELY };
    if primed { k * PRIMED_CROUCH } else { k }
}

// ---- figures ----

fn style(look: &Look, width: f32, alpha: f32) -> LineStyle {
    LineStyle {
        width,
        alpha,
        fade: Some(Fade {
            start: FADE_NEAR,
            end: FADE_FAR,
        }),
        label_scale: LABEL_SCALE,
        label_centered: true,
        label_face: look.face,
        ..LineStyle::default()
    }
}

fn callout(edges: Vec<[Vec3; 2]>, anchor: Vec3, look: &Look, style: LineStyle) -> Callout {
    Callout {
        edges,
        anchor,
        color: look.colour,
        label: None,
        style,
    }
}

fn strip_figure(out: &mut Vec<Callout>, s: &Strip, scene: &Scene, look: &Look, label: bool) {
    let targeted = scene.target == Some(s.edge);
    let alpha = if targeted {
        1.0
    } else {
        strip_strength(scene.primed, high_enough(scene.at[2], s))
    };
    let mut line = style(look, 3.0, alpha);
    line.glow = Some(Glow {
        width: 4.0,
        alpha: 0.35,
    });
    line.ribbon = (s.width > 0.0).then_some(Ribbon {
        width: s.width,
        alpha: 0.18,
    });
    line.chevrons = flat_unit(s.drift).map(|d| Chevrons {
        direction: [d[0], d[1], 0.0],
        spacing: 0.5,
        size: 0.18,
        speed: if scene.primed { 0.4 } else { 0.0 },
    });
    line.pulse = scene.primed.then_some(Pulse {
        period: 1.2,
        depth: 0.35,
    });
    if targeted {
        line.width = 4.5;
        line.glow = Some(Glow {
            width: 7.0,
            alpha: 0.6,
        });
        line.pulse = Some(Pulse {
            period: 0.6,
            depth: 0.4,
        });
    }
    out.push(callout(vec![s.edge], middle(s.edge), look, line));
    out.push(callout(
        slot_marks(s.edge, s.stand, s.width),
        s.stand,
        look,
        style(look, 1.5, alpha),
    ));
    if let Some((edges, top)) = post(s.stand, s.fall, s.edge) {
        let mut c = callout(
            edges,
            up(top, CROUCH_LABEL_RAISE),
            look,
            style(look, 1.5, alpha),
        );
        c.label = label.then(|| format!("{} m", whole_metres(s.fall)));
        out.push(c);
    }
}

fn crouch_label_at(c: &Crouch) -> Vec3 {
    let lip = c
        .lip
        .iter()
        .flat_map(|e| e.iter().map(|p| p[2]))
        .fold(f32::NEG_INFINITY, f32::max);
    if lip.is_finite() {
        [c.stand[0], c.stand[1], lip + CROUCH_LABEL_RAISE]
    } else {
        up(c.stand, CROUCH_LABEL_HEIGHT)
    }
}

fn crouch_figure(
    out: &mut Vec<Callout>,
    c: &Crouch,
    phase: Option<Phase>,
    way: bool,
    label: bool,
    scene: &Scene,
    look: &Look,
) {
    let alpha = crouch_strength(c.proven, scene.primed);
    let under = matches!(phase, Some(Phase::Under | Phase::Held));
    let mut foot = style(look, if under { 3.0 } else { 1.5 }, alpha);
    foot.pulse = under.then_some(Pulse {
        period: 0.8,
        depth: 0.5,
    });
    let mut ring_callout = callout(
        ring(c.stand, CAPSULE_RADIUS, false),
        crouch_label_at(c),
        look,
        foot,
    );
    ring_callout.label = label.then(|| "Crouch".into());
    out.push(ring_callout);
    if !way {
        return;
    }
    if !c.lip.is_empty() {
        out.push(callout(
            c.lip.clone(),
            c.stand,
            look,
            style(look, 1.0, alpha),
        ));
    }
    if let Some(o) = flat_unit(c.out) {
        let (near, far) = (offset(c.stand, o, WAY_NEAR), offset(c.stand, o, WAY_FAR));
        // Walk in, and once the button is up, walk out.
        let leaving = matches!(phase, Some(Phase::Held | Phase::Primed));
        let (from, to) = if leaving { (near, far) } else { (far, near) };
        let flow = (scene.seconds * CHEVRON_SPEED / CHEVRON_SPACING).fract();
        out.push(callout(
            chevrons(from, to, CHEVRON_SPACING, CHEVRON_SIZE, flow),
            c.stand,
            look,
            style(look, 1.5, alpha),
        ));
    }
}

fn landing_figure(out: &mut Vec<Callout>, l: &Landing, at: Vec3, look: &Look) {
    let (edges, width) = match l.verdict {
        Verdict::OnSlot => (ring(l.at, CAPSULE_RADIUS, false), 3.0),
        Verdict::Fixable => (ring(l.at, CAPSULE_RADIUS, true), 2.0),
        Verdict::Miss => (cross(l.at, CAPSULE_RADIUS), 3.0),
    };
    out.push(callout(edges, l.at, look, style(look, width, 1.0)));
    // The bar stands to the side of the landing as the player sees it.
    let side = flat_unit([l.at[0] - at[0], l.at[1] - at[1]]).map_or([1.0, 0.0], |d| [-d[1], d[0]]);
    let base = offset(l.at, side, BAR_SIDE);
    let top = up(base, BAR_HEIGHT);
    out.push(callout(
        vec![
            [base, top],
            [offset(top, side, -0.05), offset(top, side, 0.05)],
        ],
        base,
        look,
        style(look, 1.0, 1.0),
    ));
    let fill = speed_fill(l.speed, l.need);
    if fill > 0.0 {
        out.push(callout(
            vec![[base, up(base, BAR_HEIGHT * fill)]],
            base,
            look,
            style(look, 4.0, 1.0),
        ));
    }
}

/// The figures for one frame. `items` are the spots to draw, nearest
/// first: every strip gets its ribbon, outline and post (the nearest
/// [`BOUNCE_LABELS`] posts their height); every crouch spot its ring, the
/// nearest [`WAYS`] the lip and the way, and the nearest within
/// [`CROUCH_LABEL_RANGE`] the word `Crouch`. The charge's phase shows on
/// the nearest crouch spot within [`PHASE_RANGE`]. While primed and
/// falling, the predicted landing and the speed bar.
pub fn figures(items: &[Item], scene: &Scene, look: &Look) -> Vec<Callout> {
    let nearest_crouch = |range: f32, d: fn(Vec3, Vec3) -> f32| {
        items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| match it {
                Item::Crouch(c) => Some((i, d(scene.at, c.stand))),
                Item::Strip(_) => None,
            })
            .filter(|(_, d)| *d <= range)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let phase_at = nearest_crouch(PHASE_RANGE, horizontal);
    let label_at = nearest_crouch(CROUCH_LABEL_RANGE, distance3);
    let mut out = Vec::new();
    let (mut strips, mut crouches) = (0, 0);
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Strip(s) => {
                strip_figure(&mut out, s, scene, look, strips < BOUNCE_LABELS);
                strips += 1;
            }
            Item::Crouch(c) => {
                let phase = if phase_at == Some(i) {
                    scene.phase
                } else {
                    None
                };
                crouch_figure(
                    &mut out,
                    c,
                    phase,
                    crouches < WAYS,
                    label_at == Some(i),
                    scene,
                    look,
                );
                crouches += 1;
            }
        }
    }
    if scene.primed
        && scene.airborne
        && scene.falling
        && let Some(l) = &scene.landing
    {
        landing_figure(&mut out, l, scene.at, look);
    }
    out
}

/// The `max` spots nearest `at` (by their edges' middle), nearest first.
pub fn nearest(spots: &[Spot], at: Vec3, max: usize) -> Vec<&Spot> {
    let mut near: Vec<(f32, &Spot)> = spots
        .iter()
        .map(|s| (distance3(middle(s.edge), at), s))
        .collect();
    let by_distance = |a: &(f32, &Spot), b: &(f32, &Spot)| a.0.total_cmp(&b.0);
    if near.len() > max {
        near.select_nth_unstable_by(max, by_distance);
        near.truncate(max);
    }
    near.sort_by(by_distance);
    near.into_iter().map(|(_, s)| s).collect()
}

fn seconds() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Everything the mode draws on the map this frame: the figures for the
/// [`MAX_LINES`] nearest spots and, while primed, the prompts. Nothing
/// without a position.
pub fn frame(guest: &dyn GuestMemory, s: &State, spots: &[Spot]) -> Vec<Callout> {
    use crate::superbounce_assist as assist;
    let Some(at) = s.position else {
        return Vec::new();
    };
    let look = hud::look(guest);
    let primed = s.primed();
    let body = if primed {
        assist::read_body(guest, assist::PORT)
    } else {
        None
    };
    let target = body.as_ref().and_then(|b| assist::pick(spots, b));
    let items: Vec<Item> = nearest(spots, at, MAX_LINES)
        .into_iter()
        .map(|sp| match sp.kind {
            Kind::Bounce => Item::Strip(contract::strip(sp)),
            Kind::Crouch => Item::Crouch(contract::crouch(sp)),
        })
        .collect();
    let scene = Scene {
        at,
        airborne: s.airborne,
        falling: s.vz.is_some_and(|v| v < 0.0),
        primed,
        phase: contract::phase(s),
        landing: contract::landing(s),
        target: target.as_ref().map(|(sp, _)| sp.edge),
        seconds: seconds(),
    };
    let mut out = figures(&items, &scene, &look);
    if let Some(body) = &body {
        out.extend(prompts::callouts(
            spots,
            body,
            true,
            target.as_ref().map(|(sp, reach)| (*sp, reach)),
            assist::assisting(),
            &look,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::world_lines::LabelFace;

    const LOOK: Look = Look {
        colour: [0.3, 0.6, 0.9],
        face: LabelFace::Hud,
        glyphs: true,
    };

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn scene() -> Scene {
        Scene {
            at: [0.0; 3],
            airborne: false,
            falling: false,
            primed: false,
            phase: None,
            landing: None,
            target: None,
            seconds: 0.0,
        }
    }

    fn strip(x: f32, fall: f32) -> Strip {
        Strip {
            edge: [[x, -0.5, 0.0], [x, 0.5, 0.0]],
            stand: [x, 0.0, 0.0],
            fall,
            width: 0.12,
            drift: [1.0, 0.0],
        }
    }

    fn crouch(x: f32, proven: bool) -> Crouch {
        Crouch {
            stand: [x, 0.0, 0.0],
            out: [1.0, 0.0],
            lip: vec![[[x - 0.2, 0.2, 0.6], [x + 0.2, 0.2, 0.7]]],
            proven,
        }
    }

    #[test]
    fn a_ring_is_closed_at_its_radius_just_above_the_floor() {
        let c = [1.0, 2.0, 3.0];
        let r = ring(c, CAPSULE_RADIUS, false);
        assert_eq!(r.len(), RING_SEGMENTS);
        for [a, b] in &r {
            for p in [a, b] {
                assert!(close((p[0] - c[0]).hypot(p[1] - c[1]), CAPSULE_RADIUS));
                assert!(close(p[2], c[2] + LIFT));
            }
        }
        // End to end, and round to the start.
        for w in r.windows(2) {
            assert_eq!(w[0][1], w[1][0]);
        }
        let (first, last) = (r[0][0], r[RING_SEGMENTS - 1][1]);
        assert!(close(first[0], last[0]) && close(first[1], last[1]));
        // Open: every other segment, with gaps between.
        let open = ring(c, CAPSULE_RADIUS, true);
        assert_eq!(open.len(), RING_SEGMENTS / 2);
        assert_ne!(open[0][1], open[1][0]);
    }

    #[test]
    fn a_cross_has_two_arms_through_its_centre() {
        let x = cross([0.0, 0.0, 1.0], 0.2);
        assert_eq!(x.len(), 2);
        for [a, b] in &x {
            assert!(close((a[0] + b[0]) / 2.0, 0.0) && close((a[1] + b[1]) / 2.0, 0.0));
            assert!(close(distance3(*a, *b), 0.4));
            assert!(close(a[2], 1.0 + LIFT));
        }
    }

    #[test]
    fn chevrons_point_toward_the_end_and_flow_without_leaving_the_run() {
        let (from, to) = ([1.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
        for phase in [0.0, 0.3, 0.99, 1.7, f32::NAN] {
            let c = chevrons(from, to, 0.25, 0.1, phase);
            assert_eq!(c.len(), 8, "four arrowheads of two edges");
            for pair in c.chunks(2) {
                let tip = pair[0][1];
                assert_eq!(tip, pair[1][1]);
                // Pointing toward `to` (down X): the tip is nearer it.
                assert!(tip[0] < pair[0][0][0] && tip[0] < pair[1][0][0]);
                // The wings are either side.
                assert!(pair[0][0][1] * pair[1][0][1] < 0.0);
                let centre = tip[0] + 0.05;
                assert!((0.0..=1.0).contains(&centre), "{centre} at phase {phase}");
            }
        }
        // The flow moves them along.
        let a = chevrons(from, to, 0.25, 0.1, 0.0);
        let b = chevrons(from, to, 0.25, 0.1, 0.5);
        assert!(close(a[0][1][0] - b[0][1][0], 0.125));
        // No run, no arrows; a run shorter than the spacing still has one.
        assert!(chevrons(from, from, 0.25, 0.1, 0.0).is_empty());
        assert_eq!(chevrons(from, [0.9, 0.0, 0.0], 0.25, 0.1, 0.0).len(), 2);
    }

    #[test]
    fn the_slot_is_closed_at_both_ends_and_ticked_at_the_stand() {
        let s = strip(2.0, 0.0);
        let m = slot_marks(s.edge, [2.0, 0.1, 0.0], 0.12);
        assert_eq!(m.len(), 3);
        for [a, b] in &m {
            // Across the strip (it runs along Y), the slot's width long.
            assert!(close(a[1], b[1]));
            assert!(close((a[0] - b[0]).abs(), 0.12));
            assert!(close(a[2], LIFT));
        }
        assert!(close(m[0][0][1], -0.5) && close(m[1][0][1], 0.5));
        assert!(close(m[2][0][1], 0.1));
        // A strip with no length: only the tick.
        assert_eq!(slot_marks([[0.0; 3]; 2], [0.0; 3], 0.12).len(), 1);
    }

    #[test]
    fn a_post_rises_to_the_fall_with_a_cross_at_its_top_and_none_for_a_standing_jump() {
        let s = strip(0.0, 1.0);
        let (edges, top) = post(s.stand, s.fall, s.edge).unwrap();
        assert_eq!(top, [0.0, 0.0, 1.0]);
        assert_eq!(edges[0], [s.stand, top]);
        assert_eq!(edges.len(), 3);
        for [a, b] in &edges[1..] {
            assert!(close(a[2], 1.0) && close(b[2], 1.0));
            assert!(close(distance3(*a, *b), 2.0 * POST_TICK));
        }
        assert!(post(s.stand, FLAT_FALL, s.edge).is_none());
        assert!(post(s.stand, f32::NAN, s.edge).is_none());
    }

    #[test]
    fn the_speed_bar_fills_toward_the_need() {
        assert_eq!(speed_fill(4.0, 8.0), 0.5);
        assert_eq!(speed_fill(9.0, 8.0), 1.0);
        assert_eq!(speed_fill(3.0, 0.0), 1.0);
        assert_eq!(speed_fill(f32::NAN, 8.0), 0.0);
        assert_eq!(speed_fill(-1.0, 8.0), 0.0);
    }

    #[test]
    fn a_strip_is_bright_when_primed_and_the_floor_is_high_enough() {
        let s = strip(0.0, 1.0);
        assert!(high_enough(1.0, &s) && !high_enough(0.9, &s));
        // A standing jump needs no height.
        assert!(high_enough(-5.0, &strip(0.0, 0.0)));
        assert_eq!(strip_strength(true, true), 1.0);
        assert_eq!(strip_strength(true, false), CLIMB);
        assert_eq!(strip_strength(false, true), DIM);
        assert_eq!(strip_strength(false, false), DIM * CLIMB);
    }

    #[test]
    fn a_likely_crouch_spot_is_half_strength_and_all_drop_to_a_fifth_while_primed() {
        assert_eq!(crouch_strength(true, false), 1.0);
        assert_eq!(crouch_strength(false, false), LIKELY);
        assert_eq!(crouch_strength(true, true), PRIMED_CROUCH);
        assert_eq!(crouch_strength(false, true), LIKELY * PRIMED_CROUCH);
    }

    #[test]
    fn only_the_nearest_crouch_within_range_says_crouch_and_the_nearest_three_have_a_way() {
        let items: Vec<Item> = (0..5)
            .map(|i| Item::Crouch(crouch(1.0 + i as f32, i % 2 == 0)))
            .collect();
        let out = figures(&items, &scene(), &LOOK);
        let labels: Vec<_> = out.iter().filter_map(|c| c.label.as_deref()).collect();
        assert_eq!(labels, ["Crouch"]);
        let first = out.iter().find(|c| c.label.is_some()).unwrap();
        // Over the lip, at the spot nearest the player; full strength, proven.
        assert!(close(first.anchor[0], 1.0) && close(first.anchor[2], 0.7 + CROUCH_LABEL_RAISE));
        assert_eq!(first.style.alpha, 1.0);
        assert_eq!(first.color, LOOK.colour);
        // Ring, lip, way for the nearest three; the ring alone after.
        assert_eq!(out.len(), 3 * 3 + 2);
        // The second is likely: half strength.
        assert_eq!(out[3].style.alpha, LIKELY);
        // Out of range, no label.
        let far = [Item::Crouch(crouch(3.5, true))];
        assert!(
            figures(&far, &scene(), &LOOK)
                .iter()
                .all(|c| c.label.is_none())
        );
    }

    #[test]
    fn the_way_flows_in_until_the_button_is_up_then_out() {
        let items = [Item::Crouch(crouch(0.5, true))];
        let way = |phase| {
            let s = Scene { phase, ..scene() };
            let out = figures(&items, &s, &LOOK);
            // Ring, lip, way.
            let tip = out[2].edges[0][1];
            let wing = out[2].edges[0][0];
            (tip[0] - wing[0]).signum()
        };
        // `out` is +X: walking in points -X, walking out +X.
        assert_eq!(way(Some(Phase::Near)), -1.0);
        assert_eq!(way(Some(Phase::Under)), -1.0);
        assert_eq!(way(Some(Phase::Held)), 1.0);
        assert_eq!(way(None), -1.0);
        // Under the lip the footprint is bold and pulses.
        let under = figures(
            &items,
            &Scene {
                phase: Some(Phase::Under),
                ..scene()
            },
            &LOOK,
        );
        assert!(under[0].style.pulse.is_some() && under[0].style.width > 1.5);
        assert!(figures(&items, &scene(), &LOOK)[0].style.pulse.is_none());
        // A spot out of the phase's range does not take it.
        let far = [Item::Crouch(crouch(2.0, true))];
        let s = Scene {
            phase: Some(Phase::Under),
            ..scene()
        };
        assert!(figures(&far, &s, &LOOK)[0].style.pulse.is_none());
    }

    #[test]
    fn primed_crouch_figures_drop_to_a_fifth() {
        let items = [Item::Crouch(crouch(0.5, true))];
        let s = Scene {
            primed: true,
            ..scene()
        };
        assert!(
            figures(&items, &s, &LOOK)
                .iter()
                .all(|c| c.style.alpha == PRIMED_CROUCH)
        );
    }

    #[test]
    fn a_strip_is_its_slot_ribbon_an_outline_and_a_post_labelled_for_the_nearest_three() {
        let items: Vec<Item> = (0..4).map(|i| Item::Strip(strip(i as f32, 1.0))).collect();
        let s = Scene {
            primed: true,
            at: [0.0, 0.0, 2.0],
            ..scene()
        };
        let out = figures(&items, &s, &LOOK);
        assert_eq!(out.len(), 4 * 3);
        let ribbon = out[0].style.ribbon.unwrap();
        assert_eq!(ribbon.width, 0.12);
        assert_eq!(out[0].style.alpha, 1.0);
        let labels: Vec<_> = out.iter().filter_map(|c| c.label.as_deref()).collect();
        assert_eq!(labels, ["3 m", "3 m", "3 m"]);
        // From a floor too low, half strength; unprimed, dimmed.
        let low = Scene {
            at: [0.0, 0.0, 0.5],
            ..s
        };
        assert_eq!(figures(&items, &low, &LOOK)[0].style.alpha, CLIMB);
        let unprimed = Scene { primed: false, ..s };
        assert_eq!(figures(&items, &unprimed, &LOOK)[0].style.alpha, DIM);
        // The assist's target is bright and bold whatever the height.
        let targeted = Scene {
            target: Some(strip(0.0, 1.0).edge),
            ..low
        };
        let t = &figures(&items, &targeted, &LOOK)[0];
        assert_eq!(t.style.alpha, 1.0);
        assert!(t.style.width > 3.0);
        // A standing-jump strip has no post.
        assert_eq!(figures(&[Item::Strip(strip(0.0, 0.0))], &s, &LOOK).len(), 2);
    }

    #[test]
    fn the_landing_shows_only_while_primed_and_falling_and_says_its_course() {
        let landing = |verdict| Landing {
            at: [1.0, 0.0, 0.0],
            verdict,
            speed: 4.0,
            need: 8.0,
        };
        let falling = Scene {
            primed: true,
            airborne: true,
            falling: true,
            at: [0.0, 0.0, 3.0],
            landing: Some(landing(Verdict::OnSlot)),
            ..scene()
        };
        let on = figures(&[], &falling, &LOOK);
        // The ring, the bar's frame and its fill.
        assert_eq!(on.len(), 3);
        assert_eq!(on[0].edges.len(), RING_SEGMENTS);
        let fix = figures(
            &[],
            &Scene {
                landing: Some(landing(Verdict::Fixable)),
                ..falling
            },
            &LOOK,
        );
        assert_eq!(fix[0].edges.len(), RING_SEGMENTS / 2);
        let miss = figures(
            &[],
            &Scene {
                landing: Some(landing(Verdict::Miss)),
                ..falling
            },
            &LOOK,
        );
        assert_eq!(miss[0].edges.len(), 2);
        // The bar stands beside the landing, across the way the player looks,
        // and its fill is half its height.
        let [base, top] = on[1].edges[0];
        assert!(close(base[1].abs(), BAR_SIDE) && close(base[0], 1.0));
        assert!(close(top[2] - base[2], BAR_HEIGHT));
        let fill = on[2].edges[0];
        assert!(close(fill[1][2] - fill[0][2], BAR_HEIGHT * 0.5));
        // Not primed, on the ground, or rising: none.
        for s in [
            Scene {
                primed: false,
                ..falling
            },
            Scene {
                airborne: false,
                ..falling
            },
            Scene {
                falling: false,
                ..falling
            },
        ] {
            assert!(figures(&[], &s, &LOOK).is_empty());
        }
    }
}
