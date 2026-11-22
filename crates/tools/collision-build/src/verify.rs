//! The properties every retail structure BSP has (collision.md, "What the
//! engine does with a BSP", V), checked on any BSP: the compiler's
//! acceptance, and `geometry.check` on an edited one.
//!
//! - `Bsp::validate`: indices, unit planes, acyclic trees, closed rings on
//!   their planes.
//! - Rings have at most eight vertices.
//! - Every surface a reference's 2D tree reaches walks back to itself from
//!   its centroid, projected by the reference's plane and flip.
//! - Every reference's plane is on its leaf's path, on the side its flip
//!   bit says (set: the back).
//! - Leaf flag bit 0 is set exactly where a reference reaches a two-sided
//!   surface.

use tags::collision::{Bsp, Child, Child2d};

use crate::{MAX_RING, TWO_SIDED, query};

/// Every problem found; empty means the BSP passes.
pub fn check(bsp: &Bsp, materials: usize) -> Vec<String> {
    let mut out = bsp.validate(materials);
    if !out.is_empty() {
        return out;
    }
    for s in 0..bsp.surfaces.len() {
        let n = bsp.surface_ring(s).len();
        if n > MAX_RING {
            out.push(format!("surface {s}: ring of {n} vertices"));
        }
    }
    let paths = leaf_paths(bsp);
    for (leaf, l) in bsp.leaves.iter().enumerate() {
        let first = usize::from(l.first_ref);
        let mut two_sided = false;
        for r in first..first + usize::from(l.ref_count) {
            let rf = bsp.refs[r];
            let plane = usize::from(rf.plane & 0x7fff);
            let flip = rf.plane & 0x8000 != 0;
            for path in &paths[leaf] {
                let on = path
                    .iter()
                    .any(|&(node, front)| usize::from(bsp.nodes[node].plane) == plane && front != flip);
                if !on {
                    out.push(format!(
                        "leaf {leaf}: reference {r} (plane {plane}, flip {flip}) is not on its path's side"
                    ));
                }
            }
            let pl = bsp.planes[plane].0;
            for s in reached(bsp, rf.target()) {
                if bsp.surfaces[s].flags & TWO_SIDED != 0 {
                    two_sided = true;
                }
                let ring = query::ring(bsp, s);
                let k = ring.len().max(1) as f32;
                let c: [f32; 3] = std::array::from_fn(|a| ring.iter().map(|p| p[a]).sum::<f32>() / k);
                let got = query::walk_2d(bsp, rf.target(), query::project(pl, flip, c));
                if got != Some(s) {
                    out.push(format!(
                        "leaf {leaf}: reference {r}: surface {s}'s centroid walks to {got:?}"
                    ));
                }
            }
        }
        if (l.flags & 1 != 0) != two_sided {
            out.push(format!(
                "leaf {leaf}: flag bit 0 is {} but its references {} a two-sided surface",
                l.flags & 1,
                if two_sided { "reach" } else { "do not reach" }
            ));
        }
    }
    out
}

/// Surfaces a 2D tree reaches, in order, once each.
pub fn reached(bsp: &Bsp, root: Child2d) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut seen = vec![false; bsp.nodes_2d.len()];
    while let Some(c) = stack.pop() {
        match c {
            Child2d::Surface(0x7fff) => {}
            Child2d::Surface(s) => {
                if !out.contains(&usize::from(s)) {
                    out.push(usize::from(s));
                }
            }
            Child2d::Node(n) => {
                let n = usize::from(n);
                if n < seen.len() && !seen[n] {
                    seen[n] = true;
                    stack.push(bsp.nodes_2d[n].front);
                    stack.push(bsp.nodes_2d[n].back);
                }
            }
        }
    }
    out.sort_unstable();
    out
}

/// Every path from the root to each leaf: (node, front side) steps.
pub fn leaf_paths(bsp: &Bsp) -> Vec<Vec<Vec<(usize, bool)>>> {
    let mut out = vec![Vec::new(); bsp.leaves.len()];
    if bsp.nodes.is_empty() {
        return out;
    }
    let mut stack: Vec<(usize, Vec<(usize, bool)>)> = vec![(0, Vec::new())];
    let mut budget = 64 * (bsp.nodes.len() + bsp.leaves.len() + 1);
    while let Some((node, path)) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        let n = bsp.nodes[node];
        for (front, c) in [(false, n.back), (true, n.front)] {
            let mut p = path.clone();
            p.push((node, front));
            match c {
                Child::None => {}
                Child::Leaf(l) => {
                    if let Some(slot) = out.get_mut(l as usize) {
                        slot.push(p);
                    }
                }
                Child::Node(k) => stack.push((k as usize, p)),
            }
        }
    }
    out
}
