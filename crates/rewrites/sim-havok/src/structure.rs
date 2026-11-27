//! The polygon that a structure child builds from a BSP surface's edge ring
//! (docs/engine/havok-structure.md). Keep vertex order and all four SSE
//! lanes, including the feature ID in w.
//!
//! [`build`] is the builder `0x001edd00` on a [`Journal`]: every word it
//! stores in the shape, in its order, from the collision BSP's surface,
//! edge and vertex blocks. Every child-shape getter of the structure's
//! containers reaches the polygon through it (`0x001ee4d0`, the
//! collection's `vt[+0x34]`; `0x001ee500`; `0x001ef99b`).
use crate::mem::Journal;
use simd::{Xm, addss, comiss, mulss, ucomiss};
use std::sync::atomic::{AtomicU64, Ordering};

pub type Vertex = [u32; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transform {
    pub scale: u32,
    pub basis: [[u32; 3]; 3],
    pub translation: [u32; 3],
}

fn mul(mxcsr: &mut u32, left: u32, right: u32) -> Result<u32, Xm> {
    mulss(mxcsr, left.into(), right).map(|v| v as u32)
}

fn add(mxcsr: &mut u32, left: u32, right: u32) -> Result<u32, Xm> {
    addss(mxcsr, left.into(), right).map(|v| v as u32)
}

impl Transform {
    fn point(&self, mut point: [u32; 3], one: u32, mxcsr: &mut u32) -> Result<[u32; 3], Xm> {
        // Skip scaling only on an ordered equal comparison.
        if ucomiss(mxcsr, self.scale, one)? != 0x40 {
            for p in &mut point {
                *p = mul(mxcsr, self.scale, *p)?;
            }
        }
        let mut out = [0; 3];
        for (i, coord) in out.iter_mut().enumerate() {
            let z = mul(mxcsr, self.basis[2][i], point[2])?;
            let y = mul(mxcsr, self.basis[1][i], point[1])?;
            let zy = add(mxcsr, z, y)?;
            let x = mul(mxcsr, self.basis[0][i], point[0])?;
            let zyx = add(mxcsr, zy, x)?;
            *coord = add(mxcsr, zyx, self.translation[i])?;
        }
        Ok(out)
    }
}

/// The convex polygon shape's vtable (type `0x18`).
pub const POLYGON_VTABLE: u32 = 0x0045_60b8;
/// The word the builder copies into the shape's radius.
pub const POLYGON_RADIUS: u32 = 0x0047_f068;
/// The `1.0` the scale is compared with.
pub const ONE: u32 = 0x0045_dbc0;
/// The high bits of every vertex's feature ID (`w`).
pub const FEATURE_BITS: u32 = 0x3f00_0000;
/// The collision BSP's surface, edge and vertex blocks (`count`, `address`
/// pairs) and their strides.
pub const BSP_SURFACES: u32 = 0x2c;
pub const BSP_EDGES: u32 = 0x34;
pub const BSP_VERTICES: u32 = 0x3c;
/// A ring longer than this declines: retail's shape buffers hold a few
/// dozen vertices, and a broken ring would not end.
pub const MAX_RING: u32 = 256;

/// The shape's fields, by byte offset.
pub mod field {
    pub const VTABLE: u32 = 0x00;
    /// A half-word, set to 1.
    pub const REFERENCES: u32 = 0x06;
    pub const TAIL: u32 = 0x08;
    pub const RADIUS: u32 = 0x0c;
    pub const SURFACE: u32 = 0x10;
    pub const COUNT: u32 = 0x14;
    /// The surface's half-word `+6`, its byte `+4`, then the flag argument.
    pub const MATERIAL: u32 = 0x18;
    pub const SURFACE_FLAGS: u32 = 0x1a;
    pub const FLAG: u32 = 0x1b;
    pub const OWNER: u32 = 0x1c;
    pub const VERTICES: u32 = 0x20;
}

/// The builder's arguments: the shape in ecx, the transform in edi (zero
/// for none), then five stack words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args {
    pub shape: u32,
    pub transform: u32,
    pub bsp: u32,
    pub flag: u8,
    pub owner: u32,
    pub surface: u32,
    pub tail: u32,
}

/// Why a build runs the guest. Each is counted ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the builder reads is unmapped, a half-word straddles a word,
    /// a store is unaligned or not plain writable memory.
    Memory,
    /// The ring has more than [`MAX_RING`] vertices.
    Ring,
    /// A transform under an MXCSR that does not mask every exception.
    Float,
    /// The guest answered an audited call whose return could not be
    /// observed, so it went uncompared.
    Unobserved,
}

impl Fallback {
    pub const ALL: [Fallback; 4] = [Self::Memory, Self::Ring, Self::Float, Self::Unobserved];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Ring => "ring",
            Self::Float => "float",
            Self::Unobserved => "unobserved",
        }
    }
}

static FALLBACKS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Count a call sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// The builder's declines since start, by reason, in [`Fallback::ALL`] order.
pub fn fallbacks() -> [(&'static str, u64); 4] {
    Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed)))
}

fn word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    j.get(at).ok_or(Fallback::Memory)
}

fn half(j: &Journal, at: u32) -> Result<u32, Fallback> {
    if at & 3 == 3 {
        return Err(Fallback::Memory);
    }
    Ok(word(j, at & !3)? >> (8 * (at & 3)) & 0xffff)
}

fn put(j: &mut Journal, at: u32, v: u32) -> Result<(), Fallback> {
    j.put(at, v).ok_or(Fallback::Memory)
}

/// Store the low `bytes` bytes of `v` at `at`, inside one word.
fn put_part(j: &mut Journal, at: u32, v: u32, bytes: u32) -> Result<(), Fallback> {
    let shift = 8 * (at & 3);
    if shift + 8 * bytes > 32 {
        return Err(Fallback::Memory);
    }
    let mask = (u32::MAX >> (32 - 8 * bytes)) << shift;
    let old = word(j, at & !3)?;
    put(j, at & !3, (old & !mask) | (v << shift & mask))
}

impl Transform {
    /// The 13 words at `at`: the scale, the basis by input axis, the
    /// translation.
    pub fn read(j: &Journal, at: u32) -> Option<Self> {
        let w = |off: u32| j.get(at.wrapping_add(off));
        let mut basis = [[0; 3]; 3];
        for (r, row) in basis.iter_mut().enumerate() {
            for (i, v) in row.iter_mut().enumerate() {
                *v = w(4 + 0xc * r as u32 + 4 * i as u32)?;
            }
        }
        Some(Self { scale: w(0)?, basis, translation: [w(0x28)?, w(0x2c)?, w(0x30)?] })
    }
}

/// The whole builder: the shape's header, then each ring vertex from the
/// surface's first edge, taking each edge's start or end and next edge by
/// which side names the surface, until the ring is back at its first edge;
/// then the surface's material and flag bytes and the tail word. Returns
/// the vertex count.
pub fn build(j: &mut Journal, a: &Args, mxcsr: u32) -> Result<u32, Fallback> {
    use field::*;
    if a.transform != 0 && mxcsr & 0x1f80 != 0x1f80 {
        return Err(Fallback::Float);
    }
    let shape = |off: u32| a.shape.wrapping_add(off);
    let radius = word(j, POLYGON_RADIUS)?;
    put(j, shape(TAIL), 0)?;
    put(j, shape(COUNT), 0)?;
    put_part(j, shape(FLAG), u32::from(a.flag), 1)?;
    put(j, shape(OWNER), a.owner)?;
    put_part(j, shape(REFERENCES), 1, 2)?;
    put(j, shape(RADIUS), radius)?;
    put(j, shape(VTABLE), POLYGON_VTABLE)?;
    put(j, shape(SURFACE), a.surface)?;
    let record = word(j, a.bsp.wrapping_add(BSP_SURFACES))?.wrapping_add(a.surface.wrapping_mul(8));
    let first = half(j, record.wrapping_add(2))?;
    let mut edge = first;
    let mut vertices = 0;
    loop {
        if vertices == MAX_RING {
            return Err(Fallback::Ring);
        }
        let e = word(j, a.bsp.wrapping_add(BSP_EDGES))?.wrapping_add(edge.wrapping_mul(12));
        // 0x001edd6c: the edge's right surface, sign-extended, against the
        // surface argument.
        let right = half(j, e.wrapping_add(0xa))? as u16 as i16 as i32 as u32;
        let side = u32::from(right == a.surface);
        let vertex = half(j, e.wrapping_add(2 * side))?;
        let at = word(j, a.bsp.wrapping_add(BSP_VERTICES))?.wrapping_add(vertex << 4);
        let p = [word(j, at)?, word(j, at.wrapping_add(4))?, word(j, at.wrapping_add(8))?];
        let p = if a.transform == 0 {
            p
        } else {
            let t = Transform::read(j, a.transform).ok_or(Fallback::Memory)?;
            let one = word(j, ONE)?;
            let mut env = mxcsr;
            t.point(p, one, &mut env).map_err(|Xm| Fallback::Float)?
        };
        let count = word(j, shape(COUNT))?;
        let slot = shape(VERTICES).wrapping_add(count.wrapping_mul(16));
        for (i, v) in p.into_iter().chain([0]).enumerate() {
            put(j, slot.wrapping_add(4 * i as u32), v)?;
        }
        let count = word(j, shape(COUNT))?;
        put(j, shape(0x2c).wrapping_add(count << 4), count | FEATURE_BITS)?;
        let count = word(j, shape(COUNT))?;
        put(j, shape(COUNT), count.wrapping_add(1))?;
        vertices += 1;
        let next = half(j, e.wrapping_add(2 * side + 4))?;
        if next == first {
            break;
        }
        edge = next;
    }
    // 0x001edea5: the record's half-word `+6` and byte `+4`, then the tail.
    let material = half(j, record.wrapping_add(6))?;
    put_part(j, shape(MATERIAL), material, 2)?;
    let flags = word(j, record.wrapping_add(4) & !3)? >> (8 * (record.wrapping_add(4) & 3)) & 0xff;
    put_part(j, shape(SURFACE_FLAGS), flags, 1)?;
    put(j, shape(TAIL), a.tail)?;
    Ok(vertices)
}

/// Source ring in the surface's winding, with the selected endpoint first.
pub fn polygon(
    ring: impl Iterator<Item = [u32; 3]>,
    transform: Option<&Transform>,
    one: u32,
    feature_bits: u32,
    mxcsr: &mut u32,
) -> Result<Vec<Vertex>, Xm> {
    ring.enumerate().map(|(index, p)| {
        let [x, y, z] = match transform {
            Some(t) => t.point(p, one, mxcsr)?,
            None => p,
        };
        Ok([x, y, z, feature_bits | index as u32])
    }).collect()
}

/// The fourth product is discarded by the reduction, but it still raises
/// exceptions. Strictly greater keeps the earliest vertex on ties.
pub fn support(
    vertices: &[Vertex],
    direction: Vertex,
    mut best: u32,
    mxcsr: &mut u32,
) -> Result<usize, Xm> {
    let mut selected = 0;
    for (i, vertex) in vertices.iter().enumerate() {
        let mut product = [0; 4];
        for axis in 0..4 {
            product[axis] = mul(mxcsr, vertex[axis], direction[axis])?;
        }
        let yx = add(mxcsr, product[1], product[0])?;
        let dot = add(mxcsr, product[2], yx)?;
        if comiss(mxcsr, dot, best)? == 0 {
            best = dot;
            selected = i;
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::Words;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Heap(HashMap<u32, u32>);

    impl Words for Heap {
        fn get(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied().or(Some(0))
        }
    }

    const BSP: u32 = 0x1000;
    const SHAPE: u32 = 0x8000;

    /// A triangle: surface 1 on the left of edges 0 and 2 and on the right
    /// of edge 1, whose start and end the builder swaps.
    fn triangle() -> Heap {
        let mut h = Heap::default();
        let (surfaces, edges, vertices) = (0x2000, 0x3000, 0x4000);
        h.0.insert(BSP + BSP_SURFACES, surfaces);
        h.0.insert(BSP + BSP_EDGES, edges);
        h.0.insert(BSP + BSP_VERTICES, vertices);
        // Surface 1: first edge 0, flags byte 0x55, material 0x1234.
        h.0.insert(surfaces + 8, 0);
        h.0.insert(surfaces + 12, 0x1234_0055);
        // Edge: start, end, forward, reverse, left, right.
        let edge = |h: &mut Heap, i: u32, w: [u32; 6]| {
            let at = edges + 12 * i;
            h.0.insert(at, w[0] | w[1] << 16);
            h.0.insert(at + 4, w[2] | w[3] << 16);
            h.0.insert(at + 8, w[4] | w[5] << 16);
        };
        edge(&mut h, 0, [0, 1, 1, 9, 1, 7]);
        edge(&mut h, 1, [0, 2, 9, 2, 7, 1]);
        edge(&mut h, 2, [1, 2, 0, 9, 1, 7]);
        for (v, p) in [[1.0f32, 0., 0.], [0., 1., 0.], [0., 0., 1.]].into_iter().enumerate() {
            for (k, x) in p.into_iter().enumerate() {
                h.0.insert(vertices + 16 * v as u32 + 4 * k as u32, x.to_bits());
            }
        }
        h.0.insert(POLYGON_RADIUS, 0x3c23_d70a);
        h
    }

    #[test]
    fn a_ring_is_laid_out_as_the_builder_lays_it() {
        let h = triangle();
        let mut j = Journal::new(&h);
        let a = Args { shape: SHAPE, transform: 0, bsp: BSP, flag: 0xab, owner: 0xffff_ffff, surface: 1, tail: 0x77 };
        assert_eq!(build(&mut j, &a, simd::MXCSR_DEFAULT), Ok(3));
        let w = |off: u32| j.get(SHAPE + off).unwrap();
        assert_eq!((w(0), w(4) >> 16, w(8), w(0xc), w(0x10), w(0x14)), (POLYGON_VTABLE, 1, 0x77, 0x3c23_d70a, 1, 3));
        assert_eq!((w(0x18), w(0x1c)), (0xab55_1234, 0xffff_ffff));
        // Edge 0 from its start, edge 1 from its end (vertex 2), edge 2.
        let xs: Vec<u32> = (0..3).map(|i| [w(0x20 + 16 * i), w(0x24 + 16 * i), w(0x28 + 16 * i)].iter().position(|&b| b == 1f32.to_bits()).unwrap() as u32).collect();
        assert_eq!(xs, [0, 2, 1]);
        assert_eq!((0..3).map(|i| w(0x2c + 16 * i)).collect::<Vec<_>>(), [FEATURE_BITS, FEATURE_BITS | 1, FEATURE_BITS | 2]);
    }

    #[test]
    fn a_ring_that_never_closes_and_an_unmasked_transform_decline() {
        let mut h = triangle();
        // Edge 2's forward edge goes back to edge 1, never to 0.
        h.0.insert(0x3000 + 24 + 4, 1 | 9 << 16);
        let a = Args { shape: SHAPE, transform: 0, bsp: BSP, flag: 0, owner: 0, surface: 1, tail: 0 };
        assert_eq!(build(&mut Journal::new(&h), &a, simd::MXCSR_DEFAULT), Err(Fallback::Ring));
        let h = triangle();
        let a = Args { transform: 0x9000, ..a };
        assert_eq!(build(&mut Journal::new(&h), &a, 0x1f00), Err(Fallback::Float));
        let a = Args { shape: SHAPE + 2, ..a };
        assert_eq!(build(&mut Journal::new(&h), &a, simd::MXCSR_DEFAULT), Err(Fallback::Memory));
    }

    #[test]
    fn declines_are_counted_by_reason_in_order() {
        assert_eq!(fallbacks().map(|(n, _)| n), ["memory", "ring", "float", "unobserved"]);
        let before = fallbacks()[1].1;
        count(Fallback::Ring);
        assert!(fallbacks()[1].1 > before);
    }

    #[test]
    fn transform_uses_guest_axis_order() {
        let large = 1e20f32;
        let transform = Transform {
            scale: 1f32.to_bits(),
            basis: [[1., 1., 1.], [-large, -large, -large], [large; 3]].map(|v| v.map(f32::to_bits)),
            translation: [(-0.5f32).to_bits(); 3],
        };
        let mut env = simd::MXCSR_DEFAULT;
        let result = polygon([[1f32.to_bits(); 3]; 2].into_iter(), Some(&transform),
            1f32.to_bits(), 0x100, &mut env).unwrap();
        assert_eq!(result[0], [0.5f32.to_bits(), 0.5f32.to_bits(), 0.5f32.to_bits(), 0x100]);
        assert_eq!(result[1][3], 0x101);
    }

    #[test]
    fn support_preserves_first_tie_and_unused_lane_exceptions() {
        let vertices = [[1., 0., 0., 0.], [2., 2., 0., 3.], [2., -2., 0., 5.]]
            .map(|v| v.map(f32::to_bits));
        let mut env = simd::MXCSR_DEFAULT;
        let direction = [1., 0., 0., f32::INFINITY].map(f32::to_bits);
        assert_eq!(support(&vertices, direction, (-10f32).to_bits(), &mut env).unwrap(), 1);
        assert_ne!(env & simd::IE, 0);
    }
}
