//! The object spatial visit, `0x000bb050` (docs/engine/collision-queries.md,
//! entry H), with the list heads `0x000b8940` and `0x000b8a10`.
//!
//! The visit floods the clusters a sphere reaches ([`crate::flood`]), then
//! walks two per-cluster lists of object references and returns the
//! handles whose bounding spheres overlap. The reference lists are live
//! object state, so they are read from the guest on every call; only the
//! portal graph is decoded data. Nothing is written: [`Visit`] carries the
//! epochs, marks and handles in the order the guest would store them.
use crate::flood::{Flood, FloodQuery, MAX_CLUSTERS, Topology};
use crate::{Bail, GuestRead};
use sim_core::det::{Checked, checked, checked_inputs};
use std::collections::HashSet;
use std::sync::Arc;

/// A reference list, by the two globals that point at it: the per-cluster
/// head table (a datum word per cluster, `-1` for none) and the datum array
/// its records live in (`+0x24` record size, `+0x44` records). Both are
/// read on every walk, as the guest does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct List {
    pub heads: u32,
    pub array: u32,
}

/// Datum array header offsets.
pub mod layout {
    pub const ELEMENT_SIZE: u32 = 0x24;
    pub const DATA: u32 = 0x44;
    /// In a reference record: the object handle, the next reference's
    /// datum (`-1` ends the list), then the type byte and the bounding
    /// sphere (centre at `+4`, radius at `+0x10` from the type byte).
    pub const HANDLE: u32 = 4;
    pub const NEXT: u32 = 8;
    pub const BOUNDS: u32 = 0xc;
    /// Bytes of a record the visit reads, from [`HANDLE`].
    pub const READ: u32 = 0x1c;
}

/// One visit, as the guest's arguments and live words give it.
#[derive(Clone, Copy, Debug)]
pub struct VisitQuery {
    /// The list bits (1: the first list, 2: the second) and the object
    /// type mask; zero means all.
    pub lists: u32,
    pub types: u32,
    /// The location's cluster word (`0xffff`: nowhere).
    pub cluster: u16,
    pub centre: [u32; 3],
    pub radius: u32,
    /// The output capacity (the argument's low half, compared signed).
    pub capacity: u16,
    /// The zero the radius is compared with.
    pub zero: u32,
    /// The x87 control word, and whether the x87 state admits the flood.
    pub control: u16,
    pub x87: bool,
    /// The cluster mark array and the flood epoch before this call bumps it.
    pub cluster_marks: u32,
    pub flood_epoch: u32,
    /// The object mark array, the visit generation before this call bumps
    /// it, and the number of mark slots the adapter can write without
    /// reaching other globals.
    pub object_marks: u32,
    pub generation: u32,
    pub slots: u32,
    /// The collideable list, then the second list.
    pub lists_at: [List; 2],
}

/// What a visit leaves behind, decided before any of it is written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visit {
    /// The flood, when the radius is positive and the location has a
    /// cluster: the flood epoch is bumped and its clusters marked.
    pub flood: Option<Flood>,
    /// Object mark slots, in the order they were marked.
    pub marks: Vec<u16>,
    /// The output handles, in order.
    pub handles: Vec<u32>,
    /// When the output filled: the handle that found it full (the guest
    /// returns its high half with the count).
    pub full: Option<u32>,
    /// The guest spans the walk read besides the topology, for the
    /// adapter's alias check against what it writes.
    pub reads: Vec<(u32, u32)>,
}

impl Visit {
    /// `eax` as the guest returns it.
    pub fn eax(&self) -> u32 {
        let count = self.handles.len() as u32;
        self.full.map_or(count, |h| (h & 0xffff_0000) | count)
    }
}

enum Walk {
    Done,
    Full,
}

struct State<'q, G> {
    g: &'q G,
    r: &'q Checked,
    q: &'q VisitQuery,
    types: u32,
    generation: u32,
    marked: HashSet<u16>,
    out: Visit,
}

impl<G: GuestRead> State<'_, G> {
    fn read(&mut self, addr: u32, len: u32) -> Result<Vec<u8>, Bail> {
        let b = self.g.bytes(addr, len as usize)?;
        self.out.reads.push((addr, len));
        Ok(b)
    }

    fn u32(&mut self, addr: u32) -> Result<u32, Bail> {
        let b = self.read(addr, 4)?;
        Ok(u32::from_le_bytes(b[..4].try_into().unwrap()))
    }

    /// One cluster's list `k` (`0x000b8940` for the first list,
    /// `0x000b8a10` for the second, then the walk inline in the visit).
    fn walk(&mut self, k: usize, cluster: u16) -> Result<Walk, Bail> {
        let list = self.q.lists_at[k];
        let heads = self.u32(list.heads)?;
        let head_at = heads.wrapping_add((cluster as i16 as i32 as u32).wrapping_mul(4));
        let head = self.u32(head_at)?;
        if head == u32::MAX {
            return Ok(Walk::Done);
        }
        let array = self.u32(list.array)?;
        let size = self.u32(array.wrapping_add(layout::ELEMENT_SIZE))?;
        let data = self.u32(array.wrapping_add(layout::DATA))?;
        let record = |datum: u32| data.wrapping_add((datum & 0xffff).wrapping_mul(size));
        let mut at = record(head);
        let mut records = HashSet::new();
        loop {
            if !records.insert(at) {
                return Err(Bail::Data);
            }
            let fields = self.read(at.wrapping_add(layout::HANDLE), layout::READ)?;
            let w = |o: u32| {
                let o = (o - layout::HANDLE) as usize;
                u32::from_le_bytes(fields[o..o + 4].try_into().unwrap())
            };
            let (handle, next) = (w(layout::HANDLE), w(layout::NEXT));
            if handle == u32::MAX {
                return Ok(Walk::Done);
            }
            let kind = fields[(layout::BOUNDS - layout::HANDLE) as usize];
            if self.types & 1u32.wrapping_shl(u32::from(kind)) != 0 {
                let slot = handle as u16;
                if u32::from(slot) >= self.q.slots {
                    return Err(Bail::Data);
                }
                let mark = self
                    .q
                    .object_marks
                    .wrapping_add(4 * u32::from(slot));
                if !self.marked.contains(&slot) && self.g.u32(mark)? != self.generation {
                    // Marked before the bounds test: a later reference to
                    // the same object is skipped even when this one missed.
                    self.marked.insert(slot);
                    self.out.marks.push(slot);
                    let b = |i: u32| f32::from_bits(w(layout::BOUNDS + 4 + 4 * i));
                    if self.overlaps(k, [b(0), b(1), b(2)], b(3))? {
                        let count = self.out.handles.len() as u16 as i16;
                        if count >= self.q.capacity as i16 {
                            self.out.full = Some(handle);
                            return Ok(Walk::Full);
                        }
                        self.out.handles.push(handle);
                    }
                }
            }
            if next == u32::MAX {
                return Ok(Walk::Done);
            }
            at = record(next);
        }
    }

    /// `(r_ref + r)^2 >= |c_ref - c|^2`, SSE. The two lists sum the
    /// squares in different orders: z, y, x for the first and z, x, y for
    /// the second.
    fn overlaps(&self, k: usize, centre: [f32; 3], radius: f32) -> Result<bool, Bail> {
        let r = self.r;
        let c = self.q.centre.map(f32::from_bits);
        let z = r.sub(centre[2], c[2]);
        let y = r.sub(centre[1], c[1]);
        let x = r.sub(centre[0], c[0]);
        let (zz, yy, xx) = (r.mul(z, z), r.mul(y, y), r.mul(x, x));
        let reach = r.add(radius, f32::from_bits(self.q.radius));
        let apart = if k == 0 {
            r.add(r.add(zz, yy), xx)
        } else {
            r.add(r.add(zz, xx), yy)
        };
        let rr = r.mul(reach, reach);
        Ok(rr >= apart)
    }
}

/// Entry H. `topology` is asked for only when the visit floods.
pub fn object_visit(
    g: &impl GuestRead,
    q: &VisitQuery,
    topology: impl FnOnce() -> Result<Arc<Topology>, Bail>,
) -> Result<Visit, Bail> {
    checked(|r| Some(visit(g, r, q, topology))).unwrap_or(Err(Bail::Float))
}

fn visit(
    g: &impl GuestRead,
    r: &Checked,
    q: &VisitQuery,
    topology: impl FnOnce() -> Result<Arc<Topology>, Bail>,
) -> Result<Visit, Bail> {
    let lists = if q.lists == 0 { u32::MAX } else { q.lists };
    let types = if q.types == 0 { u32::MAX } else { q.types };
    let mut s = State {
        g,
        r,
        q,
        types,
        generation: q.generation.wrapping_add(1),
        marked: HashSet::new(),
        out: Visit::default(),
    };
    let clusters: Vec<u16> = if q.cluster == u16::MAX {
        Vec::new()
    } else {
        if !checked_inputs(&[q.radius, q.zero]) {
            return Err(Bail::Float);
        }
        if f32::from_bits(q.radius) > f32::from_bits(q.zero) {
            if !q.x87 {
                return Err(Bail::Env);
            }
            let t = topology()?;
            let flood = t.flood(
                g,
                r,
                &FloodQuery {
                    cluster: q.cluster,
                    centre: q.centre,
                    radius: q.radius,
                    capacity: MAX_CLUSTERS,
                    control: q.control,
                    marks: q.cluster_marks,
                    epoch: q.flood_epoch.wrapping_add(1),
                },
            )?;
            let clusters = flood.clusters[..flood.written].to_vec();
            s.out.flood = Some(flood);
            clusters
        } else {
            vec![q.cluster]
        }
    };
    for c in clusters {
        for k in 0..2 {
            if lists & (1 << k) != 0 {
                if let Walk::Full = s.walk(k, c)? {
                    return Ok(s.out);
                }
            }
        }
    }
    Ok(s.out)
}

#[cfg(test)]
mod tests;
