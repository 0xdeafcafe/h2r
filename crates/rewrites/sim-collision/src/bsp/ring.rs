//! Edge tests for a two-sided BSP surface. Short cached rings also hold
//! four-edge groups of vertex A and the rounded difference B - A.
use super::{Bsp, Column, RingEdge, Surface, above, cached_ring};
use crate::{Bail, GuestRead};
use sim_core::{
    det::{self, Fp},
    lanes::{self, Op, Vec4},
};
use std::mem::size_of;

const LANES: usize = 4;
const MAX_PACKED_EDGES: usize = 8;

pub(super) struct Rings {
    pub(super) edges: Column<RingEdge>,
    pub(super) packed: Column<EdgeGroup>,
    pub(super) complete: bool,
    pub(super) mapped_padding: usize,
}

impl Rings {
    pub(super) fn new(edges: Vec<RingEdge>, surfaces: &mut [Surface], budget: u64) -> Self {
        let mut packed = Vec::new();
        // Pack only after the BSP's raw rings fit in the available space.
        let room = budget.saturating_sub((edges.capacity() * size_of::<RingEdge>()) as u64);
        let max_groups = (room / size_of::<EdgeGroup>() as u64) as usize;
        let mut complete = true;
        if det::host_fast_env() {
            for surface in surfaces {
                let len = surface.ring_len as usize;
                if !(1..=MAX_PACKED_EDGES).contains(&len) {
                    continue;
                }
                let count = len.div_ceil(LANES);
                let needed = packed.len() + count;
                if needed > max_groups {
                    complete = false;
                    continue;
                }
                let Ok(ring) = cached_ring(&edges, surface) else {
                    continue;
                };
                let Some(first) = EdgeGroup::new(&ring[..len.min(LANES)]) else {
                    continue;
                };
                let second = if len > LANES {
                    let Some(group) = EdgeGroup::new(&ring[LANES..]) else {
                        continue;
                    };
                    Some(group)
                } else {
                    None
                };
                if needed > packed.capacity() {
                    let capacity = needed
                        .max(packed.capacity().saturating_mul(2))
                        .min(max_groups);
                    packed.reserve_exact(capacity - packed.len());
                }
                surface.packed_ring = packed.len() as u32;
                packed.push(first);
                packed.extend(second);
            }
        }
        Self {
            edges: edges.into(),
            packed: packed.into(),
            complete,
            mapped_padding: 0,
        }
    }

    pub(super) fn footprint(&self) -> usize {
        self.mapped_padding
            + self.edges.capacity() * size_of::<RingEdge>()
            + self.packed.capacity() * size_of::<EdgeGroup>()
    }

    pub(super) fn groups(&self, surface: &Surface) -> &[EdgeGroup] {
        let start = surface.packed_ring as usize;
        let count = (surface.ring_len as usize).div_ceil(LANES);
        self.packed
            .get(start..)
            .and_then(|tail| tail.get(..count))
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(super) struct EdgeGroup {
    // Keep all three axes so references with different projection rows
    // share one cache. The query loads only its projected pair.
    origin: [Vec4; 3],
    delta: [Vec4; 3],
}

impl EdgeGroup {
    fn new(edges: &[RingEdge]) -> Option<Self> {
        let mut group = Self {
            origin: [[0; LANES]; 3],
            delta: [[0; LANES]; 3],
        };
        for lane in 0..LANES {
            // Repeat the first edge in unused lanes. It has no extra
            // effect on the any-edge test or the range checks.
            let edge = edges.get(lane).unwrap_or(&edges[0]);
            for axis in 0..3 {
                let (a, b) = (edge.a[axis], edge.b[axis]);
                if !lanes::ordinary1(a) || !lanes::ordinary1(b) {
                    return None;
                }
                let delta = lanes::binary1(Op::Sub, b, a);
                if !lanes::ordinary1(delta) {
                    return None;
                }
                group.origin[axis][lane] = a;
                group.delta[axis][lane] = delta;
            }
        }
        Some(group)
    }

    fn crosses(&self, fp: &mut Fp, [ua, va]: [usize; 2], [u, v]: [u32; 2]) -> Vec4 {
        let pu = fp.sub4([u; LANES], self.origin[ua]);
        let pv = fp.sub4([v; LANES], self.origin[va]);
        let left = fp.mul4(self.delta[va], pu);
        let right = fp.mul4(pv, self.delta[ua]);
        fp.sub4(left, right)
    }
}

impl Bsp {
    pub(super) fn ring_contains(
        &self,
        g: &impl GuestRead,
        index: u32,
        surface: &Surface,
        axes: [usize; 2],
        point: [u32; 2],
        fp: &mut Fp,
    ) -> Result<bool, Bail> {
        self.with_surface_ring(g, index, surface, |edges, groups| {
            // The packed test runs every lane, past the edge where the
            // guest's loop stops. Its answer stands only when it raised no
            // flag the walk had not already raised, so the guest's shorter
            // run raises none either.
            if !groups.is_empty()
                && let Some((inside, after)) = contains_packed(groups, axes, point, *fp)
                && after.mxcsr() == fp.mxcsr()
            {
                return Ok(inside);
            }
            contains_scalar(edges, axes, point, fp)
        })
    }
}

fn contains_packed(
    groups: &[EdgeGroup],
    axes: [usize; 2],
    point: [u32; 2],
    mut fp: Fp,
) -> Option<(bool, Fp)> {
    for group in groups {
        let crosses = group.crosses(&mut fp, axes, point);
        // A later lane can fail a range check after an earlier lane would
        // have rejected the point. Replay the ordered test in that case.
        if !fp.exact() {
            return None;
        }
        if crosses.into_iter().any(|cross| above(&mut fp, cross, 0)) {
            return fp.exact().then_some((false, fp));
        }
    }
    fp.exact().then_some((true, fp))
}

fn contains_scalar(
    edges: &[RingEdge],
    [ua, va]: [usize; 2],
    [u, v]: [u32; 2],
    fp: &mut Fp,
) -> Result<bool, Bail> {
    for edge in edges {
        let (au, av) = (edge.a[ua], edge.a[va]);
        let (bu, bv) = (edge.b[ua], edge.b[va]);
        let pu = fp.sub(u, au);
        let pv = fp.sub(v, av);
        let ev = fp.sub(bv, av);
        let eu = fp.sub(bu, au);
        let left = fp.mul(ev, pu);
        let right = fp.mul(pv, eu);
        let cross = fp.sub(left, right);
        if !fp.exact() {
            return Err(Bail::Float);
        }
        // `comiss cross, 0; ja`.
        if above(fp, cross, 0) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
