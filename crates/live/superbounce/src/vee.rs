//! A V's three targets (docs/game/superbounce.md, "Vs"): each arm's edge,
//! and the tip or the bottom line.
//!
//! Played at Zanzibar, the V's two converging edges met at the tip, and
//! the landing that counts mirrors the approach: coming in from one side
//! of the V, the player lands on the arm across the bisector, not on the
//! tip. So a V is three candidates for the sim's training, each on its
//! own collision edge, and each arm records the side it is approached
//! from ([`Arm`]), the far side of the bisector. The training flies an
//! arm from that side and once from its own as a control (`bots::learn`);
//! play marks, once an arm is proven, the arm across from the player
//! ([`facing`]).
//!
//! Two shapes are Vs:
//!
//! - **Converging creases**: two concave creases that share an end
//!   vertex, the tip, at an acute angle ([`OPEN_DOT`]). The arms are the
//!   two creases' edges, the tip is the vertex (its `edge` the vertex
//!   twice), and the bisector runs from the tip into the opening.
//! - **A groove**: two ramps meeting at the bottom (`bounce.rs`, `crease`,
//!   [`Shape::Vee`]). The bottom line is the target and the bisector; each
//!   arm is a ramp, its edge the bottom line's own two vertices, its aim
//!   [`ARM_STAND`] across the line onto that ramp.
use crate::bounce::JOIN_TOL;
use crate::geom::{V2, dot2, len2, sub};
use crate::scan::ORIGIN_LIFT;
use crate::{Arm, Kind, Shape, Spot, Vec3};

/// Two creases converge on a tip when their horizontal directions out of
/// it have a dot over this: an opening under 90°. A wider fan is a grid's
/// crossing, not a V.
pub(crate) const OPEN_DOT: f32 = 0.0;
/// ...and under this: not one line carried on (cos about 2.6°).
const COLLINEAR: f32 = 0.999;
/// A groove's arm aims this far (world units, horizontally) across the
/// bottom line onto its ramp: inside the catch ([`crate::CATCH`]), so a
/// landing at the aim is on the edge, and apart from the bottom line's
/// aim, so the training keeps the three apart.
pub(crate) const ARM_STAND: f32 = 0.05;
/// A converging V's parts score 0.5, plus this × the two creases' mean
/// fit (a crease's score over its 0.3 floor, out of its 0.3 range): 0.5 to
/// 0.95, the groove's range.
const PAIR_FIT: f32 = 0.45;

fn xy(p: Vec3) -> V2 {
    [p[0], p[1]]
}

fn unit2(d: V2) -> Option<V2> {
    let l = len2(d);
    (l > 1e-6 && l.is_finite()).then(|| [d[0] / l, d[1] / l])
}

fn close(a: Vec3, b: Vec3) -> bool {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() <= JOIN_TOL
}

/// A line spot that can be a V's arm: a single crease or a groove with
/// no arm yet, on an edge, not a vertex.
fn line(s: &Spot) -> bool {
    s.kind == Kind::Bounce
        && matches!(s.shape, Shape::Crease | Shape::Vee)
        && s.arm.is_none()
        && unit2([s.edge[1][0] - s.edge[0][0], s.edge[1][1] - s.edge[0][1]]).is_some()
}

/// A crease's fit for a V's score, 0 to 1.
fn fit(s: &Spot) -> f32 {
    ((s.score - 0.3) / 0.3).clamp(0.0, 1.0)
}

/// Where two line spots meet at an end, as a V: the tip vertex (the map's
/// own, `a`'s) and each one's horizontal direction out of it.
fn tip(a: &Spot, b: &Spot) -> Option<(Vec3, V2, V2)> {
    for (ia, ib) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        if !close(a.edge[ia], b.edge[ib]) {
            continue;
        }
        let t = a.edge[ia];
        let out = |s: &Spot, i: usize| {
            let far = s.edge[1 - i];
            unit2([far[0] - t[0], far[1] - t[1]])
        };
        let (ua, ub) = (out(a, ia)?, out(b, ib)?);
        let c = dot2(ua, ub);
        return (c > OPEN_DOT && c < COLLINEAR).then_some((t, ua, ub));
    }
    None
}

/// The arm along `u` out of a V whose bisector is `b`, through `pivot`:
/// approached from the far side of the bisector, drifting across it onto
/// the arm.
fn arm_of(pivot: Vec3, b: V2, u: V2) -> Arm {
    let n = [-b[1], b[0]];
    let toward = if dot2(n, u) >= 0.0 { n } else { [-n[0], -n[1]] };
    Arm {
        pivot,
        bisector: b,
        from: [-toward[0], -toward[1]],
    }
}

/// `spots` with every V made three ([`crate::bounce`] calls it after the
/// cover test, before the ranking): two line spots converging on a tip
/// become its two arms and a tip spot; each groove left keeps its bottom
/// line and gains its two arms. A line spot is an arm of one V at most,
/// the most acute pair first.
pub(crate) fn parts(spots: Vec<Spot>) -> Vec<Spot> {
    let lines: Vec<usize> = (0..spots.len()).filter(|&i| line(&spots[i])).collect();
    let mut pairs: Vec<(f32, usize, usize, Vec3, V2, V2)> = Vec::new();
    for (n, &i) in lines.iter().enumerate() {
        for &k in &lines[n + 1..] {
            if let Some((t, ua, ub)) = tip(&spots[i], &spots[k]) {
                pairs.push((dot2(ua, ub), i, k, t, ua, ub));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut out = spots;
    let mut used = vec![false; out.len()];
    let mut tips: Vec<Spot> = Vec::new();
    for (_, i, k, t, ua, ub) in pairs {
        if used[i] || used[k] {
            continue;
        }
        let Some(b) = unit2([ua[0] + ub[0], ua[1] + ub[1]]) else {
            continue;
        };
        used[i] = true;
        used[k] = true;
        let score = (0.5 + PAIR_FIT * (fit(&out[i]) + fit(&out[k])) / 2.0)
            .max(out[i].score)
            .max(out[k].score);
        let launch = out[i].launch.max(out[k].launch);
        for (j, u) in [(i, ua), (k, ub)] {
            let a = arm_of(t, b, u);
            let s = &mut out[j];
            s.shape = Shape::Vee;
            s.arm = Some(a);
            // Across the bisector onto the arm, the mirror of the way in.
            s.drift = [-a.from[0], -a.from[1]];
            s.score = score;
        }
        if !tips.iter().any(|s| close(s.edge[0], t)) {
            tips.push(Spot {
                kind: Kind::Bounce,
                edge: [t, t],
                stand: [t[0], t[1], t[2] + ORIGIN_LIFT],
                launch,
                // In from the opening, toward the tip.
                drift: [-b[0], -b[1]],
                score,
                shape: Shape::Vee,
                ..Spot::default()
            });
        }
    }
    // Grooves left: the bottom line stays, and gains its two arms.
    let mut arms = Vec::new();
    for (j, s) in out.iter().enumerate() {
        if used[j] || s.shape != Shape::Vee || !line(s) {
            continue;
        }
        let Some(u) = unit2([s.edge[1][0] - s.edge[0][0], s.edge[1][1] - s.edge[0][1]]) else {
            continue;
        };
        let m = [
            (s.edge[0][0] + s.edge[1][0]) / 2.0,
            (s.edge[0][1] + s.edge[1][1]) / 2.0,
            (s.edge[0][2] + s.edge[1][2]) / 2.0,
        ];
        for side in [1.0f32, -1.0] {
            let p = [-u[1] * side, u[0] * side];
            let a = arm_of(m, u, p);
            arms.push(Spot {
                stand: [m[0] + p[0] * ARM_STAND, m[1] + p[1] * ARM_STAND, s.stand[2]],
                drift: p,
                arm: Some(a),
                ..*s
            });
        }
    }
    out.extend(tips);
    out.extend(arms);
    out
}

/// The spots a player standing at `at` is given: every spot but a V's arm
/// approached from the other side. The landing arm mirrors the approach,
/// so of a V's two arms the player is shown the one across the bisector
/// from them. With no position, every spot (`superbounce_mode`'s marks
/// and the assist go through it; docs/game/superbounce.md, "Vs").
pub fn facing(spots: &[Spot], at: Option<Vec3>) -> Vec<Spot> {
    spots
        .iter()
        .filter(|s| match (s.arm, at) {
            (Some(a), Some(p)) => a.approached_from(xy(p)),
            _ => true,
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crease(a: Vec3, b: Vec3, score: f32) -> Spot {
        Spot {
            kind: Kind::Bounce,
            edge: [a, b],
            stand: [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, 0.01],
            launch: 6.0,
            drift: [0.0, 1.0],
            score,
            shape: Shape::Crease,
            ..Spot::default()
        }
    }

    #[test]
    fn two_creases_converging_on_a_tip_are_two_arms_and_the_tip() {
        let t = [0.0, 0.0, 0.0];
        let (a, b) = ([4.0, 1.0, 0.0], [4.0, -1.0, 0.0]);
        let out = parts(vec![crease(t, a, 0.5), crease(b, t, 0.45)]);
        assert_eq!(out.len(), 3, "{out:?}");
        // Each arm keeps its crease's exact edge.
        assert_eq!((out[0].edge, out[1].edge), ([t, a], [b, t]));
        assert_eq!(out[2].edge, [t, t]);
        assert!(out.iter().all(|s| s.shape == Shape::Vee && s.candidate()));
        let (aa, ab) = (out[0].arm.unwrap(), out[1].arm.unwrap());
        assert!(out[2].arm.is_none());
        // The bisector runs from the tip into the opening, +x.
        assert!((aa.bisector[0] - 1.0).abs() < 1e-5, "{aa:?}");
        // The +y arm is approached from -y, the -y arm from +y: mirrors.
        assert!(aa.from[1] < -0.99 && ab.from[1] > 0.99, "{aa:?} {ab:?}");
        assert_eq!(aa.from, [-ab.from[0], -ab.from[1]]);
        assert!(aa.approached_from([2.0, -0.3]) && !aa.approached_from([2.0, 0.3]));
        // Each arm drifts across the bisector onto itself.
        assert!(out[0].drift[1] > 0.99 && out[1].drift[1] < -0.99);
    }

    #[test]
    fn a_wide_fan_and_a_line_carried_on_are_no_v() {
        let t = [0.0, 0.0, 0.0];
        let fan = parts(vec![
            crease(t, [1.0, 0.0, 0.0], 0.5),
            crease(t, [0.0, 1.0, 0.0], 0.5),
        ]);
        assert!(fan.iter().all(|s| s.arm.is_none()), "{fan:?}");
        let on = parts(vec![
            crease([-1.0, 0.0, 0.0], t, 0.5),
            crease(t, [1.0, 0.0, 0.0], 0.5),
        ]);
        assert_eq!(on.len(), 2);
        assert!(on.iter().all(|s| s.arm.is_none() && s.shape == Shape::Crease));
    }

    #[test]
    fn a_player_on_one_side_is_given_the_arm_across_the_bisector() {
        let t = [0.0, 0.0, 0.0];
        let out = parts(vec![
            crease(t, [4.0, 1.0, 0.0], 0.5),
            crease(t, [4.0, -1.0, 0.0], 0.5),
        ]);
        let up = out[0].edge;
        let down = out[1].edge;
        // On the -y side: the +y arm, and the tip.
        let seen = facing(&out, Some([-1.0, -0.5, 0.0]));
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen.iter().any(|s| s.edge == up));
        assert!(seen.iter().all(|s| s.edge != down));
        // On the +y side: the -y arm.
        let seen = facing(&out, Some([-1.0, 0.5, 0.0]));
        assert!(seen.iter().any(|s| s.edge == down) && seen.iter().all(|s| s.edge != up));
        assert_eq!(facing(&out, None).len(), 3);
    }
}
