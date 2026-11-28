//! The BSP vector test, `0x001de6d0` and its wrapper `0x001de630`, with
//! the reference matcher `0x001dea00`, the 2D walk `0x00296520` and the
//! ring test `0x001debd0` (docs/engine/collision-queries.md, entries C
//! and D).
//!
//! The guest recurses once per plane crossing and keeps its state in a
//! 40-byte context and the result record R. Here the walk runs over the
//! decoded arrays with the same tests in the same order, and the context
//! and R updates are returned in [`Outcome`] for the adapter to write.
use super::{BAD_CHILD, BAD_PLANE, Bsp, LEAF, MAX_DEPTH, NONE, Node, above, non_negative};
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

/// Last plane visited before an unsplit segment reaches its leaf. These
/// four binary32 results also survive in registers or the D caller's frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneDistances {
    pub distance: u32,
    pub delta: u32,
    pub start: u32,
    pub end: u32,
}

/// An unsplit walk that does not enter a surface lookup. A direct leaf
/// starts without a plane, leaving the incoming XMM registers intact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StraightWalk {
    pub last: Option<PlaneDistances>,
}

/// One step of the guest walk, in execution order. The adapters replay
/// these to rebuild the registers and frames `0x1de6d0` leaves behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// A node's plane test (`0x1de6f0..0x1de79d`): the node index, its
    /// sign-extended plane index, the four values and `s0 >= 0`, `s1 >= 0`.
    /// `next` is the child an unsplit test takes.
    Plane {
        node: u32,
        plane: u32,
        d: PlaneDistances,
        front: [bool; 2],
        next: u32,
    },
    /// The ends differ: the near child is called over `[t0, split]`.
    Cross { split: u32, increasing: bool, near: u32 },
    /// The near child missed and the split was not below R's fraction:
    /// the frame returns (`0x1de895`).
    Stop,
    /// The walk goes on in the far child from the split (`0x1de7f7`).
    Far { far: u32 },
    /// The frame's leaf tail for the child word `child`, the leaf state it
    /// sets, and whether it ran the reference matcher.
    Leaf { child: u32, kind: u8, lookup: bool },
}

/// What the walk leaves in R and the context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub hit: Option<Hit>,
    pub straight: Option<StraightWalk>,
    /// Every step the guest takes, in order.
    pub steps: Vec<Step>,
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
    /// [`Bsp::vector_test_under`]: the guest's MXCSR after the walk's SSE
    /// arithmetic, PE set when an operation was inexact.
    pub mxcsr: Option<u32>,
}

pub mod record {
    /// `f32`: the hit fraction; `max(limit, 0)` before the walk.
    pub const FRACTION: u32 = 0x00;
    /// The hit plane's guest address.
    pub const PLANE: u32 = 0x04;
    /// The leaf the surface was found in, then the surface index.
    pub const LEAF: u32 = 0x08;
    pub const SURFACE: u32 = 0x0c;
    /// The surface's plane word, sign-extended (bit 15: flipped).
    pub const SURFACE_PLANE: u32 = 0x10;
    /// Surface flags `u8`, breakable index `u8`, material `i16`.
    pub const FLAGS: u32 = 0x14;
    pub const BREAKABLE: u32 = 0x15;
    pub const MATERIAL: u32 = 0x16;
    /// `u32` leaves listed, then up to 256 leaf indices.
    pub const COUNT: u32 = 0x18;
    pub const LEAVES: u32 = 0x1c;
    /// The last leaf visited once the list is full.
    pub const OVERFLOW: u32 = 0x418;
    pub const SIZE: u32 = 0x41c;
}

/// The vector test's 40-byte context (entry D's first argument; entry C
/// builds one in its own frame).
pub mod context {
    pub const FLAGS: u32 = 0x00;
    pub const BSP: u32 = 0x04;
    /// `u16`.
    pub const BREAKABLE_COUNT: u32 = 0x08;
    pub const MASK: u32 = 0x0c;
    pub const START: u32 = 0x10;
    pub const DELTA: u32 = 0x14;
    pub const RECORD: u32 = 0x18;
    pub const PREV_LEAF: u32 = 0x1c;
    /// `u8`.
    pub const PREV_KIND: u32 = 0x20;
    pub const PLANE: u32 = 0x24;
    pub const SIZE: u32 = 0x28;
}

impl Outcome {
    /// The context fields the caller reads after the walk.
    pub fn write_context<E>(
        &self,
        mut store: impl FnMut(u32, &[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        store(context::PREV_LEAF, &self.prev_leaf.to_le_bytes())?;
        store(context::PREV_KIND, &[self.prev_kind])?;
        store(context::PLANE, &self.plane.to_le_bytes())
    }

    /// Emit result-record stores in execution order. Unvisited list entries and
    /// the hit fields on a miss retain their caller-provided bytes.
    pub fn write_record<E>(
        &self,
        init: Option<u32>,
        listed: u32,
        mut store: impl FnMut(u32, &[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        if let Some(fraction) = init {
            store(record::FRACTION, &fraction.to_le_bytes())?;
            store(record::COUNT, &0u32.to_le_bytes())?;
        }
        let (count, writes) = self.list_writes(listed);
        for (offset, leaf) in writes {
            store(offset, &leaf.to_le_bytes())?;
        }
        if count != listed {
            store(record::COUNT, &count.to_le_bytes())?;
        }
        if let Some(h) = self.hit {
            store(record::FRACTION, &h.fraction.to_le_bytes())?;
            store(record::PLANE, &h.plane_addr.to_le_bytes())?;
            store(record::LEAF, &h.leaf.to_le_bytes())?;
            store(record::SURFACE, &h.surface.to_le_bytes())?;
            store(record::SURFACE_PLANE, &h.plane.to_le_bytes())?;
            store(record::FLAGS, &[h.flags])?;
            store(record::BREAKABLE, &[h.breakable])?;
            store(record::MATERIAL, &h.material.to_le_bytes())?;
        }
        Ok(())
    }

    /// R's leaf count after the walk, and where each appended leaf went:
    /// `(R offset, leaf)` in order.
    pub fn list_writes(&self, listed: u32) -> (u32, Vec<(u32, u32)>) {
        let mut count = listed;
        let mut out = Vec::with_capacity(self.leaves.len());
        for &leaf in &self.leaves {
            if (count as i32) < LEAF_LIST as i32 {
                out.push((record::LEAVES + 4 * count, leaf));
                count += 1;
            } else {
                out.push((record::OVERFLOW, leaf));
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
    // A tree reaches each internal node at most once, including tail descents.
    nodes_left: usize,
    out: Outcome,
}

impl Bsp {
    /// Walk `q` from `node` over `[t0, t1]`, as `0x1de6d0` called with
    /// that context. The live reads are breakable-mask words and, in a
    /// budget-limited world, the surface ring’s edges and vertices.
    pub fn vector_test(
        &self,
        g: &impl GuestRead,
        q: &Segment,
        node: u32,
        t0: u32,
        t1: u32,
    ) -> Result<Outcome, Bail> {
        self.walk_with(Fp::new(), g, q, node, t0, t1)
    }

    /// [`Bsp::vector_test`] under the guest's `mxcsr` ([`Fp::tracked`]; it
    /// must pass `sim_core::det::guest_env`): [`Outcome::mxcsr`] holds the
    /// MXCSR the guest's walk leaves.
    pub fn vector_test_under(
        &self,
        mxcsr: u32,
        g: &impl GuestRead,
        q: &Segment,
        node: u32,
        t0: u32,
        t1: u32,
    ) -> Result<Outcome, Bail> {
        let mut out = self.walk_with(Fp::tracked(mxcsr), g, q, node, t0, t1)?;
        out.mxcsr.get_or_insert(mxcsr);
        Ok(out)
    }

    fn walk_with(
        &self,
        fp: Fp,
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
            fp,
            nodes_left: self.nodes.len(),
            out: Outcome {
                straight: Some(StraightWalk::default()),
                prev_leaf: q.prev_leaf,
                prev_kind: q.prev_kind,
                plane: q.plane,
                ..Outcome::default()
            },
        };
        // R's fraction is only compared with splits: J passes `FLT_MAX`.
        w.fp.check_compared(q.fraction);
        w.visit(node, t0, t1, 0)?;
        if !w.fp.exact() {
            return Err(Bail::Float);
        }
        w.out.mxcsr = w.fp.mxcsr();
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
            self.nodes_left = self.nodes_left.checked_sub(1).ok_or(Bail::Data)?;
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
            let d = PlaneDistances { distance: dist, delta, start: s0, end: s1 };
            let plane = n.plane_index as i16 as i32 as u32;
            let node = child & 0x7f_ffff;
            if a == b {
                if let Some(straight) = &mut self.out.straight {
                    straight.last = Some(d);
                }
                child = Self::child(&n, a)?;
                self.out.steps.push(Step::Plane { node, plane, d, front: [a, b], next: child });
                continue;
            }
            self.out.steps.push(Step::Plane { node, plane, d, front: [a, b], next: NONE });
            // Opposite signs at the ends: delta is nonzero.
            self.out.straight = None;
            let increasing = above(delta, 0);
            let split = det::x87_neg_quotient(dist, delta);
            self.out.inexact |= split.inexact;
            let near = Self::child(&n, !increasing)?;
            self.out.steps.push(Step::Cross { split: split.value, increasing, near });
            if self.visit(near, t0, split.value, depth + 1)? {
                return Ok(true);
            }
            // `jae`: the far side only when the split is below R's fraction.
            if !above(self.q.fraction, split.value) {
                self.out.steps.push(Step::Stop);
                return Ok(false);
            }
            self.out.plane = plane;
            child = Self::child(&n, increasing)?;
            self.out.steps.push(Step::Far { far: child });
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
        // Even a table lookup of NONE uses additional stack slots.
        if lookup.is_some() {
            self.out.straight = None;
        }
        self.out.steps.push(Step::Leaf { child, kind, lookup: lookup.is_some() });
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
    /// `(Bv - Av) * (pu - Au) - (pv - Av) * (Bu - Au) > 0`.
    fn inside(&mut self, s: u32, axes: [usize; 2], u: u32, v: u32) -> Result<bool, Bail> {
        let surface = self.bsp.surfaces.get(s as usize).ok_or(Bail::Data)?;
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
        self.bsp.ring_contains(self.g, s, surface, axes, [u, v], &mut self.fp)
    }
}
