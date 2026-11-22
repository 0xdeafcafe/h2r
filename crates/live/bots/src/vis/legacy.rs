//! The scalar visibility proof used as a byte-equality reference.

use super::*;

/// Whether triangle `t` alone blocks every segment from box `a` to box
/// `b`, with the margins in the module docs.
pub(super) fn covers(t: &[V3; 3], a: &Box3, b: &Box3) -> bool {
    let v = t.map(d3);
    let n = dcross(dsub(v[1], v[0]), dsub(v[2], v[0]));
    let area2 = dlen(n);
    if area2 < 1e-6 {
        return false;
    }
    let nh = n.map(|c| c / area2);
    let side = |p: D3| ddot(nh, dsub(p, v[0]));
    let (ca, cb) = (a.corners(), b.corners());
    let mut fa = ca.map(side);
    let mut fb = cb.map(side);
    if fa[0] < 0.0 {
        fa = fa.map(|f| -f);
        fb = fb.map(|f| -f);
    }
    // `a` in front by PLANE_GAP, `b` behind by as much; `gb` its depth.
    let gb = fb.map(|f| -f);
    let (fa_min, fa_max) = fa
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), &f| (l.min(f), h.max(f)));
    let (gb_min, gb_max) = gb
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), &f| (l.min(f), h.max(f)));
    if fa_min < PLANE_GAP || gb_min < PLANE_GAP {
        return false;
    }
    // The crossing's place along a segment, `fa / (fa + gb)`, rises with
    // `fa` and falls with `gb`: its bounds are at the corners.
    if fa_min / (fa_min + gb_max) < T_END || fa_max / (fa_max + gb_min) > 1.0 - T_END {
        return false;
    }
    // Moller-Trumbore's determinant is `area2 * (fa + gb)`; `clear`
    // refuses below 1e-9.
    if area2 * (fa_min + gb_min) < 1e-5 {
        return false;
    }
    let longest = ca
        .iter()
        .flat_map(|p| cb.iter().map(move |q| dlen(dsub(*q, *p))))
        .fold(0.0, f64::max);
    if (fa_min + gb_min) < MIN_COS * longest {
        return false;
    }
    let edge = EDGE + EDGE_PER_LENGTH * longest;
    // Each edge's inward normal in the plane.
    let mut inward = [[0.0; 3]; 3];
    for k in 0..3 {
        let (p, q, r) = (v[k], v[(k + 1) % 3], v[(k + 2) % 3]);
        let mut m = dcross(nh, dsub(q, p));
        let l = dlen(m);
        if l < 1e-9 {
            return false;
        }
        m = m.map(|c| c / l);
        let alt = ddot(m, dsub(r, p));
        if alt < 0.0 {
            m = m.map(|c| -c);
        }
        if alt.abs() <= 2.0 * edge {
            return false;
        }
        inward[k] = m;
    }
    for (i, p) in ca.iter().enumerate() {
        for (j, q) in cb.iter().enumerate() {
            let (f, g) = (fa[i], gb[j]);
            let x: D3 = std::array::from_fn(|k| (g * p[k] + f * q[k]) / (f + g));
            for k in 0..3 {
                if ddot(inward[k], dsub(x, v[k])) < edge {
                    return false;
                }
            }
        }
    }
    true
}

/// Whether every segment from `a` to `b` is certainly blocked, within
/// `budget` pieces.
fn prove(walls: &Walls, a: Box3, b: Box3, inherited: &[u32], budget: &mut u32, depth: u32) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    let mut hit = Vec::new();
    walls.crossed(a.center(), b.center(), &mut hit);
    if hit.is_empty() {
        // A clear ray: they can see each other.
        return false;
    }
    if hit
        .iter()
        .chain(inherited)
        .any(|&t| covers(&walls.tris[t as usize], &a, &b))
    {
        return true;
    }
    if depth == 0 {
        // Before splitting: another clear ray settles it at once.
        let (ca, cb) = (a.corners(), b.corners());
        let f = |p: [f64; 3]| p.map(|c| c as f32);
        if (0..8).any(|c| walls.clear(f(ca[c]), f(cb[7 - c]))) {
            return false;
        }
    }
    if depth >= DEPTH {
        return false;
    }
    for t in inherited {
        if hit.len() >= CANDIDATES {
            break;
        }
        if !hit.contains(t) {
            hit.push(*t);
        }
    }
    let (la, a1, a2) = a.split();
    let (lb, b1, b2) = b.split();
    if la >= lb {
        prove(walls, a1, b, &hit, budget, depth + 1) && prove(walls, a2, b, &hit, budget, depth + 1)
    } else {
        prove(walls, a, b1, &hit, budget, depth + 1) && prove(walls, a, b2, &hit, budget, depth + 1)
    }
}

/// Whether the two boxes certainly never see each other.
pub(super) fn never_between(walls: &Walls, a: Box3, b: Box3) -> bool {
    let mut budget = LEAVES;
    prove(walls, a, b, &[], &mut budget, 0)
}
