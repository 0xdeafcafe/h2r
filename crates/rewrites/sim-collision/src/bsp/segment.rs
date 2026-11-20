//! The BSP vector test, `0x001de6d0` and its wrapper `0x001de630`, with
//! the reference matcher `0x001dea00`, the 2D walk `0x00296520` and the
//! ring test `0x001debd0` (docs/engine/collision-queries.md, entries C
//! and D).
//!
//! The guest recurses once per plane crossing and keeps its state in a
//! 40-byte context and the result record R. Here the walk runs over the
//! decoded arrays with the same tests in the same order, and the context
//! and R updates are returned in [`Outcome`] for the adapter to write.
use super::{
    BAD_CHILD, BAD_PLANE, Bsp, LEAF, MAX_DEPTH, NONE, Node, RingEdge, above, non_negative,
};
use crate::Bail;
use crate::mem::GuestRead;
use sim_core::det::{self, Fp};

/// Leaf states: an empty leaf, a leaf with flag bit 0 (it reaches a
/// two-sided surface), and solid.
pub const KIND_EMPTY: u8 = 1;
pub const KIND_TWO_SIDED: u8 = 2;
pub const KIND_SOLID: u8 = 3;

/// Leaf indices R holds before further leaves go to its overflow slot.
pub const LEAF_LIST: u32 = 0x100;

/// One vector test: the context record's inputs and the R words the walk
/// reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Query bits (context `+0`): 1 front-facing hits, 2 back-facing, 4
    /// no two-sided hits, 8 skip surfaces with flag bit 1, 0x10 skip
    /// surfaces with flag bit 3.
    pub flags: u32,
    /// Breakable surfaces with an index below this (as `i16`) are tested
    /// against `mask` (context `+8`).
    pub breakable_count: u16,
    /// Guest address of the breakable bit words, 0 for none (`+0xc`).
    pub mask: u32,
    pub start: [u32; 3],
    pub delta: [u32; 3],
    /// R `+0`: the fraction a split must be below for the far side to be
    /// walked.
    pub fraction: u32,
    /// R `+0x18`: leaves listed so far.
    pub listed: u32,
    /// Context `+0x1c`, `+0x20`, `+0x24`: the previous leaf, its state,
    /// and the last crossed plane (sign-extended index).
    pub prev_leaf: u32,
    pub prev_kind: u8,
    pub plane: u32,
}

/// The surface a hit accepted, as R `+0..+0x18` records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// `+0`: the entry fraction of the leaf segment.
    pub fraction: u32,
    /// `+4`: the guest address of the crossed plane.
    pub plane_addr: u32,
    /// `+8`: the leaf the surface was looked up in.
    pub leaf: u32,
    /// `+0xc`: the surface index.
    pub surface: u32,
    /// `+0x10`: the surface's plane word, sign-extended (bit 15 flips).
    pub plane: u32,
    /// `+0x14`, `+0x15`, `+0x16`.
    pub flags: u8,
    pub breakable: u8,
    pub material: u16,
}

/// What the walk leaves in R and the context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub hit: Option<Hit>,
    /// Leaves appended to R's list, in order, starting at the entry
    /// `listed`; once the list holds [`LEAF_LIST`] the rest go to the
    /// overflow slot `+0x418`, the last one staying.
    pub leaves: Vec<u32>,
    /// The context's final previous leaf, state and crossed plane.
    pub prev_leaf: u32,
    pub prev_kind: u8,
    pub plane: u32,
    /// Some split's x87 quotient was inexact: the guest set PE in the x87
    /// status word.
    pub inexact: bool,
    /// Guest addresses of the breakable words the walk read.
    pub mask_reads: Vec<u32>,
}

impl Outcome {
    /// R's leaf count after the walk, and where each appended leaf went:
    /// `(R offset, leaf)` in order.
    pub fn list_writes(&self, listed: u32) -> (u32, Vec<(u32, u32)>) {
        let mut count = listed;
        let mut out = Vec::with_capacity(self.leaves.len());
        for &leaf in &self.leaves {
            if (count as i32) < LEAF_LIST as i32 {
                out.push((0x1c + 4 * count, leaf));
                count += 1;
            } else {
                out.push((0x418, leaf));
            }
        }
        (count, out)
    }
}

struct Walk<'a, G> {
    bsp: &'a Bsp,
    q: &'a Segment,
    g: &'a G,
    fp: Fp,
    out: Outcome,
}

impl Bsp {
    /// Walk `q` from `node` over `[t0, t1]`, as `0x1de6d0` called with
    /// that context. `g` is read only for the breakable mask words.
    pub fn vector_test(
        &self,
        g: &impl GuestRead,
        q: &Segment,
        node: u32,
        t0: u32,
        t1: u32,
    ) -> Result<Outcome, Bail> {
        if (q.listed as i32) < 0 {
            return Err(Bail::Data);
        }
        let mut w = Walk {
            bsp: self,
            q,
            g,
            fp: Fp::new(),
            out: Outcome {
                prev_leaf: q.prev_leaf,
                prev_kind: q.prev_kind,
                plane: q.plane,
                ..Outcome::default()
            },
        };
        w.fp.check(q.fraction);
        w.visit(node, t0, t1, 0)?;
        if !w.fp.exact() {
            return Err(Bail::Float);
        }
        Ok(w.out)
    }
}

impl<G: GuestRead> Walk<'_, G> {
    fn node(&self, child: u32) -> Result<&Node, Bail> {
        let n = self
            .bsp
            .nodes
            .get((child & 0x7f_ffff) as usize)
            .ok_or(Bail::Data)?;
        if n.bad & BAD_PLANE != 0 {
            return Err(Bail::Data);
        }
        Ok(n)
    }

    fn child(n: &Node, side: bool) -> Result<u32, Bail> {
        if n.bad & BAD_CHILD[usize::from(side)] != 0 {
            return Err(Bail::Data);
        }
        Ok(n.child[usize::from(side)])
    }

    /// One frame of `0x1de6d0`: descend while both ends are on one side,
    /// recurse into the near child at a crossing and continue in the far
    /// child, then handle the leaf. True when a surface was accepted.
    fn visit(&mut self, node: u32, t0: u32, t1: u32, depth: u32) -> Result<bool, Bail> {
        if depth >= MAX_DEPTH {
            return Err(Bail::Depth);
        }
        let (p, d) = (self.q.start, self.q.delta);
        let mut t0 = t0;
        let mut child = node;
        while child & LEAF == 0 {
            let n = *self.node(child)?;
            let [nx, ny, nz, w] = n.plane;
            let fp = &mut self.fp;
            // dist = ((pz*nz + py*ny) + nx*px) - d
            let z = fp.mul(p[2], nz);
            let y = fp.mul(p[1], ny);
            let zy = fp.add(z, y);
            let x = fp.mul(nx, p[0]);
            let dot = fp.add(zy, x);
            let dist = fp.sub(dot, w);
            // delta = (dz*nz + dy*ny) + dx*nx
            let z = fp.mul(d[2], nz);
            let y = fp.mul(d[1], ny);
            let zy = fp.add(z, y);
            let x = fp.mul(d[0], nx);
            let delta = fp.add(zy, x);
            let end = fp.mul(delta, t1);
            let begin = fp.mul(delta, t0);
            let s0 = fp.add(begin, dist);
            let s1 = fp.add(end, dist);
            if !fp.exact() {
                return Err(Bail::Float);
            }
            let (a, b) = (non_negative(s0), non_negative(s1));
            if a == b {
                child = Self::child(&n, a)?;
                continue;
            }
            // Opposite signs at the ends: delta is nonzero.
            let increasing = above(delta, 0);
            let split = det::x87_neg_quotient(dist, delta);
            self.out.inexact |= split.inexact;
            let near = Self::child(&n, !increasing)?;
            if self.visit(near, t0, split.value, depth + 1)? {
                return Ok(true);
            }
            // `jae`: the far side only when the split is below R's fraction.
            if !above(self.q.fraction, split.value) {
                return Ok(false);
            }
            self.out.plane = n.plane_index as i16 as i32 as u32;
            child = Self::child(&n, increasing)?;
            t0 = split.value;
        }
        self.leaf(child, t0)
    }

    /// The leaf tail of `0x1de6d0` (`0x1de827`..): its state, the three hit
    /// patterns, the matcher, the surface filters, then the leaf list.
    fn leaf(&mut self, child: u32, t0: u32) -> Result<bool, Bail> {
        let (leaf, kind) = if child == NONE {
            (NONE, KIND_SOLID)
        } else {
            let i = child & 0x7f_ffff;
            let l = self.bsp.leaves.get(i as usize).ok_or(Bail::Data)?;
            (i, 1 + (l.flags & 1))
        };
        let bits = self.q.flags;
        let prev = self.out.prev_kind;
        let (lookup, two_sided) = if bits & 1 != 0 && matches!(prev, 1 | 2) && kind == KIND_SOLID {
            (Some(self.out.prev_leaf), false)
        } else if bits & 2 != 0 && prev == KIND_SOLID && matches!(kind, 1 | 2) {
            (Some(leaf), false)
        } else if bits & 4 == 0 && prev == KIND_TWO_SIDED && kind == KIND_TWO_SIDED {
            (
                Some(if bits & 1 != 0 {
                    self.out.prev_leaf
                } else {
                    leaf
                }),
                true,
            )
        } else {
            (None, false)
        };
        if let Some(lookup) = lookup.filter(|&l| l != NONE) {
            let s = self.matcher(lookup, t0, two_sided)?;
            if s != NONE {
                let surface = self.bsp.surfaces.get(s as usize).ok_or(Bail::Data)?;
                let skip = (surface.flags & 2 != 0 && bits & 8 != 0)
                    || (surface.flags & 8 != 0 && bits & 0x10 != 0);
                if !skip {
                    self.out.hit = Some(Hit {
                        fraction: t0,
                        plane_addr: self.bsp.plane_addr(self.out.plane),
                        leaf: lookup,
                        surface: s,
                        plane: surface.plane as i16 as i32 as u32,
                        flags: surface.flags,
                        breakable: surface.breakable,
                        material: surface.material,
                    });
                    return Ok(true);
                }
            }
        }
        if leaf != NONE {
            self.out.leaves.push(leaf);
        }
        self.out.prev_leaf = leaf;
        self.out.prev_kind = kind;
        Ok(false)
    }

    /// `0x1dea00`: the first reference of `leaf` on the crossed plane
    /// decides. The hit point `start + delta * t0` is projected by the
    /// reference's row and walked down its 2D tree. A one-sided lookup
    /// returns that surface (or none) as is; a two-sided one also needs
    /// the ring test, and otherwise goes on to the next reference.
    fn matcher(&mut self, leaf: u32, t0: u32, two_sided: bool) -> Result<u32, Bail> {
        let l = *self.bsp.leaves.get(leaf as usize).ok_or(Bail::Data)?;
        let first = u32::from(l.first_ref);
        for r in first..first + u32::from(l.ref_count) {
            let rf = *self.bsp.refs.get(r as usize).ok_or(Bail::Data)?;
            if (u32::from(rf.plane) & 0x7fff) != self.out.plane {
                continue;
            }
            let [u_axis, v_axis] = self.bsp.row_axes(rf.row)?;
            let (s, d) = (self.q.start, self.q.delta);
            let fp = &mut self.fp;
            let point: [u32; 3] = std::array::from_fn(|k| {
                let step = fp.mul(d[k], t0);
                fp.add(step, s[k])
            });
            let (u, v) = (point[u_axis], point[v_axis]);
            let surface = self.walk_2d(rf.node, u, v)?;
            if !two_sided {
                return Ok(surface);
            }
            // The ring test would read the record before the surfaces.
            if surface == NONE {
                return Err(Bail::Data);
            }
            if self.inside(surface, [u_axis, v_axis], u, v)? {
                return Ok(surface);
            }
        }
        Ok(NONE)
    }

    /// `0x296520`: front child when `(b*v + a*u) - d >= 0`; a child with
    /// bit 15 set ends the walk (`0xffff` none, else the surface).
    fn walk_2d(&mut self, root: u16, u: u32, v: u32) -> Result<u32, Bail> {
        let mut x = root as i16;
        let mut steps = 0;
        while x >= 0 {
            steps += 1;
            if steps > self.bsp.nodes_2d.len() {
                // A cycle: the guest would never return.
                return Err(Bail::Data);
            }
            let n = *self
                .bsp
                .nodes_2d
                .get((x & 0x7fff) as usize)
                .ok_or(Bail::Data)?;
            let fp = &mut self.fp;
            let bv = fp.mul(n.b, v);
            let au = fp.mul(n.a, u);
            let sum = fp.add(bv, au);
            let value = fp.sub(sum, n.d);
            if !fp.exact() {
                return Err(Bail::Float);
            }
            x = n.child[usize::from(non_negative(value))] as i16;
        }
        Ok(if x == -1 { NONE } else { (x & 0x7fff) as u32 })
    }

    /// `0x1debd0`: a breakable surface whose bit is clear is never inside;
    /// otherwise the point is inside when no edge has
    /// `(Bv - Av) * (pu - Au) - (pv - Av) * (Bu - Au) > 0`. The edges are
    /// tested in ring order and the first rejection ends the test, as in
    /// the guest.
    fn inside(&mut self, s: u32, [ua, va]: [usize; 2], u: u32, v: u32) -> Result<bool, Bail> {
        let surface = *self.bsp.surfaces.get(s as usize).ok_or(Bail::Data)?;
        let q = self.q;
        if q.mask != 0
            && surface.flags & 8 != 0
            && i16::from(surface.breakable) < q.breakable_count as i16
        {
            let b = u32::from(surface.breakable);
            let at = q.mask.wrapping_add(4 * (b >> 5));
            self.out.mask_reads.push(at);
            if self.g.u32(at)? & (1 << (b & 31)) == 0 {
                return Ok(false);
            }
        }
        let ring: &[RingEdge] = self.bsp.surface_ring(&surface)?;
        let fp = &mut self.fp;
        for e in ring {
            let (au, av) = (e.a[ua], e.a[va]);
            let (bu, bv) = (e.b[ua], e.b[va]);
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
            if above(cross, 0) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
