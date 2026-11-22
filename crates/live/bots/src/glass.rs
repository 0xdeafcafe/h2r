//! Breakable glass in the structure BSP (docs/game/bots.md, "Breaking the
//! glass"; docs/game/superbounce.md, "Zanzibar's window").
//!
//! A structure BSP surface with flag bit 3 (`0x08`) and a breakable index
//! (`+5`, `0xff` none) is breakable: Zanzibar's window panes, cut into
//! pieces, each piece its own surface. The game skips such a surface in
//! collision once its bit in the BSP's live breakable mask is clear
//! (docs/formats/collision.md, "Breakable mask"), so a broken piece is a
//! gap a body walks through and the pieces left are solid.
//!
//! The nav build keeps the breakable surfaces beside the walls
//! ([`crate::los::Walls::glass`], [`panes`]); [`in_the_way`] finds the ones
//! a crouched walk from a crouch spot's start to its stand passes through.
//! Whether a piece is still whole is the guest's mask
//! ([`crate::guest::breakable`]), read each tick while a bot practises
//! and tested here with [`intact`]. Pure, no guest memory.

use tags::collision::Bsp;

use crate::nav::V3;

/// Surface flag bit 3: tested against the breakable mask.
pub const BREAKABLE_FLAG: u8 = 0x08;
/// A surface's breakable index when it has none.
pub const NO_INDEX: u8 = 0xff;
/// One BSP's breakable mask: a bit an index (256), set while the piece is
/// whole.
pub type Mask = [u32; 8];

/// One breakable surface: its index in the BSP's mask and its outline.
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub index: u8,
    pub verts: Vec<V3>,
}

impl Pane {
    /// The outline's mean point: where a shot at the piece aims.
    pub fn center(&self) -> V3 {
        let n = self.verts.len().max(1) as f32;
        let s = self.verts.iter().fold([0.0; 3], |s, v| {
            [s[0] + v[0], s[1] + v[1], s[2] + v[2]]
        });
        [s[0] / n, s[1] / n, s[2] / n]
    }

    /// Whether the segment `a`-`b` passes through the piece (either side).
    pub fn crossed(&self, a: V3, b: V3) -> bool {
        let v = &self.verts;
        (1..v.len().saturating_sub(1)).any(|i| segment_hits(a, b, [v[0], v[i], v[i + 1]]))
    }
}

/// Every breakable surface of `bsp`, in surface order.
pub fn panes(bsp: &Bsp) -> Vec<Pane> {
    bsp.surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| s.flags & BREAKABLE_FLAG != 0 && s.breakable_surface != NO_INDEX)
        .filter_map(|(i, s)| {
            let verts: Vec<V3> = bsp
                .surface_ring(i)
                .into_iter()
                .filter_map(|v| bsp.vertices.get(v).map(|v| v.point))
                .collect();
            (verts.len() >= 3).then_some(Pane {
                index: s.breakable_surface,
                verts,
            })
        })
        .collect()
}

/// Whether breakable index `i` is whole in `mask`.
pub fn intact(mask: &Mask, i: u8) -> bool {
    mask[usize::from(i >> 5)] & (1 << (i & 31)) != 0
}

/// The heights over the floor (world units) the crouched walk in is
/// checked at: all under the crouched capsule's top (0.5), so a piece
/// above it (the lip the crouch is forced by) is never in the way.
pub const IN_HEIGHTS: [f32; 3] = [0.1, 0.25, 0.4];
/// The checked lines run this far either side of the walk's centre
/// (world units): about the capsule's radius (0.175).
pub const IN_SIDE: f32 = 0.15;
/// ...and on past the stand by this much: the body's front.
pub const IN_PAST: f32 = 0.175;

/// The panes (indices into `panes`) a crouched body walking from `from`
/// to `to` (unit origins, on the floor) goes through: the pieces to break
/// before a crouch spot's walk in, lowest first.
pub fn in_the_way(panes: &[Pane], from: V3, to: V3) -> Vec<usize> {
    let d = [to[0] - from[0], to[1] - from[1]];
    let l = d[0].hypot(d[1]);
    if l < 1e-4 {
        return Vec::new();
    }
    let (ux, uy) = (d[0] / l, d[1] / l);
    let end = [to[0] + ux * IN_PAST, to[1] + uy * IN_PAST, to[2]];
    let side = [-uy * IN_SIDE, ux * IN_SIDE];
    let mut hit: Vec<usize> = (0..panes.len())
        .filter(|&i| {
            IN_HEIGHTS.iter().any(|&h| {
                [-1.0f32, 0.0, 1.0].iter().any(|&k| {
                    let at = |p: V3| [p[0] + side[0] * k, p[1] + side[1] * k, p[2] + h];
                    panes[i].crossed(at(from), at(end))
                })
            })
        })
        .collect();
    hit.sort_by(|&a, &b| panes[a].center()[2].total_cmp(&panes[b].center()[2]));
    hit
}

/// Möller–Trumbore, two-sided, the segment's ends included.
fn segment_hits(a: V3, b: V3, t: [V3; 3]) -> bool {
    let sub = |p: V3, q: V3| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    let cross = |p: V3, q: V3| {
        [
            p[1] * q[2] - p[2] * q[1],
            p[2] * q[0] - p[0] * q[2],
            p[0] * q[1] - p[1] * q[0],
        ]
    };
    let dot = |p: V3, q: V3| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
    let (e1, e2, d) = (sub(t[1], t[0]), sub(t[2], t[0]), sub(b, a));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-9 {
        return false;
    }
    let inv = 1.0 / det;
    let s = sub(a, t[0]);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    (0.0..=1.0).contains(&(dot(e2, q) * inv))
}

#[cfg(test)]
mod tests;
