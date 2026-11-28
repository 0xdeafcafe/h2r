//! Whole collision query (entry J). All marks and result bytes are planned in
//! private storage before any is written. The installed entry ([`query`])
//! and the agent's read-only trace use the same walk.
use super::{CollisionMemory, CollisionWorld, Read, consts, layout};
use runtime::CpuAbi;
use sim_collision::{Bail, Bsp, GuestRead, Hit, Outcome, Segment, bsp::NONE};
use sim_core::det::{Fp, X87};
use std::cmp::Ordering;
use std::sync::Arc;

mod batch;
pub use batch::{Trace, trace_batch};
mod clusters;
mod guards;
pub(super) use guards::Guard;
mod native;
mod relocate;
pub use native::{Prediction, query};

const SEGMENT_BSP: u32 = 0x004e_0340;
const BREAKABLES: u32 = 0x004e_d280;
const SEGMENT_LIMIT: u32 = 0x0044_aed4;
const DEFAULT_MATERIAL: u32 = 0x0047_d8e0;
const BSP_SIDES: u32 = 3 << 23;
const FIRST_HIT: u32 = 1 << 28;
const RELOCATE: u32 = 1 << 29;
const RECORD_SIZE: usize = 0x5c;

#[derive(Clone, Copy, Debug)]
pub struct Location {
    pub leaf: u32,
    pub cluster: i16,
    pub bsp: i16,
}

#[derive(Clone, Debug)]
pub struct QueryResult {
    pub hit_type: u32,
    pub fraction: u32,
    pub position: [u32; 3],
    pub locations: [Location; 2],
    pub plane: Option<[u32; 4]>,
    pub material: Option<u16>,
    pub cluster_epoch: Option<u32>,
    pub instance_epoch: Option<u32>,
    pub object_epoch: Option<u32>,
    pub cluster_marks: Vec<u32>,
    pub instance_marks: Vec<u32>,
    pub object_marks: Vec<u32>,
}

#[derive(Clone, Copy)]
struct Ray {
    start: [u32; 3],
    delta: [u32; 3],
    flags: u32,
    ignore: [u32; 2],
}

impl Ray {
    fn filter(self) -> u32 {
        (self.flags >> 23) & 63
    }

    fn position(self, env: &mut Env, fraction: u32) -> Result<[u32; 3], Bail> {
        let pos = std::array::from_fn(|i| {
            let d = env.fp.mul(self.delta[i], fraction);
            env.fp.add(d, self.start[i])
        });
        env.exact()?;
        Ok(pos)
    }
}

/// J's floating-point state: its SSE work on [`Fp::guest`] under the
/// caller's MXCSR, its x87 work on [`X87`] under the caller's control
/// word. Every operand runs as the guest's instructions do; the flags they
/// raise go to the CPU when the plan commits.
pub(super) struct Env {
    pub(super) fp: Fp,
    pub(super) x87: X87,
}

impl Env {
    fn new(cpu: &CpuAbi) -> Self {
        Self {
            fp: Fp::guest(cpu.mxcsr),
            x87: X87::new(cpu.x87.control),
        }
    }

    /// Only an unmasked exception, which [`super::vector_env`] excludes,
    /// leaves the model.
    pub(super) fn exact(&self) -> Result<(), Bail> {
        if self.fp.exact() { Ok(()) } else { Err(Bail::Env) }
    }

    /// The guest's MXCSR so far.
    pub(super) fn mxcsr(&self) -> u32 {
        self.fp.mxcsr().unwrap_or_default()
    }

    /// Fold in the flags a sub-entry raised: `mxcsr` its MXCSR after the
    /// call, `x87` its x87 exception flags.
    pub(super) fn absorb(&mut self, mxcsr: u32, x87: u16) {
        self.fp.raise(mxcsr & 0x3f);
        self.x87.raise_bits(x87);
    }

    /// Entry C's walk (`0x1de630`) under this state.
    pub(super) fn vector_test(
        &mut self,
        b: &Bsp,
        g: &impl GuestRead,
        q: &Segment,
        t1: u32,
    ) -> Result<Outcome, Bail> {
        let mxcsr = self.mxcsr();
        let out = b.vector_test_under(mxcsr, self.x87.control(), g, q, 0, 0, t1)?;
        self.absorb(out.mxcsr.unwrap_or(mxcsr), out.x87_flags);
        Ok(out)
    }
}

struct Record([u8; RECORD_SIZE]);

impl Record {
    fn store(&mut self, offset: u32, bytes: &[u8]) {
        self.0[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
    }

    fn word(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.0[offset..offset + 4].try_into().unwrap())
    }

    fn half(&self, offset: usize) -> u16 {
        u16::from_le_bytes(self.0[offset..offset + 2].try_into().unwrap())
    }

    fn put(&mut self, offset: u32, value: u32) {
        self.store(offset, &value.to_le_bytes());
    }

    fn put_location(&mut self, offset: u32, loc: Location) {
        self.put(offset, loc.leaf);
        self.store(offset + 4, &loc.cluster.to_le_bytes());
        self.store(offset + 6, &loc.bsp.to_le_bytes());
    }

    fn location(&self, offset: usize) -> Location {
        Location {
            leaf: self.word(offset),
            cluster: self.half(offset + 4) as i16,
            bsp: self.half(offset + 6) as i16,
        }
    }

    fn plane(&self) -> [u32; 4] {
        std::array::from_fn(|i| self.word(0x28 + 4 * i))
    }

    /// `0 - x` per word (`subss`) when flipped.
    fn put_plane(&mut self, env: &mut Env, p: [u32; 4], flip: bool) -> Result<(), Bail> {
        for (i, x) in p.into_iter().enumerate() {
            let x = if flip { env.fp.sub(0, x) } else { x };
            self.put(0x28 + 4 * i as u32, x);
        }
        env.exact()
    }

    fn structure_hit(
        &mut self,
        env: &mut Env,
        g: &impl GuestRead,
        structure: u32,
        h: Hit,
    ) -> Result<(), Bail> {
        let material = match h.material as i16 {
            -1 => DEFAULT_MATERIAL,
            n if n >= 0 => g
                .u32(structure.wrapping_add(0x10))?
                .wrapping_add(n as u32 * 20 + 8),
            _ => return Err(Bail::Data),
        };
        self.put(0, 1);
        self.put(4, h.fraction);
        self.store(0x24, &g.u16(material)?.to_le_bytes());
        self.put_plane(env, plane_at(g, h.plane_addr)?, h.plane & 0x8000 != 0)?;
        self.put(0x4c, h.leaf);
        self.put(0x50, h.surface);
        self.put(0x54, h.plane);
        self.store(0x58, &[h.flags, h.breakable]);
        self.store(0x5a, &h.material.to_le_bytes());
        Ok(())
    }

    fn set_position(&mut self, pos: [u32; 3]) {
        for (i, x) in pos.into_iter().enumerate() {
            self.put(8 + i as u32 * 4, x);
        }
    }
}

fn plane_at(g: &impl GuestRead, at: u32) -> Result<[u32; 4], Bail> {
    let bytes = g.bytes(at, 16)?;
    Ok(std::array::from_fn(|i| {
        u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap())
    }))
}

fn bsp(g: &mut impl CollisionMemory, world: &CollisionWorld, addr: u32) -> Result<Arc<Bsp>, Bail> {
    let rev = g.bsp_revision(addr)?;
    let m = Read(&*g, rev);
    world.bsp(&m, addr, || consts(&m))
}

fn location(g: &impl GuestRead, structure: u32, index: i16, leaf: u32) -> Result<Location, Bail> {
    let cluster = if leaf == NONE {
        -1
    } else {
        let table = g.u32(structure.wrapping_add(layout::STRUCTURE_LEAVES))?;
        g.u16(table.wrapping_add(leaf.wrapping_mul(8)))? as i16
    };
    Ok(Location {
        leaf,
        cluster,
        bsp: index,
    })
}

struct Plan {
    record: Record,
    flags: u32,
    marks: clusters::Marks,
    /// The guest's MXCSR afterwards and the x87 exception flags raised.
    mxcsr: u32,
    x87: u16,
}

fn plan(
    cpu: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    mut ray: Ray,
    mut record: Record,
) -> Result<Plan, Bail> {
    if g.trapped() {
        return Err(Bail::Trap);
    }
    super::vector_env(cpu)?;
    // Shape and model transforms can use forward string stores.
    if cpu.eflags & (1 << 10) != 0 {
        return Err(Bail::Env);
    }
    if ray.flags & BSP_SIDES == 0 {
        ray.flags |= BSP_SIDES;
    }

    let m = Read(&*g, (0, 0));
    let structure = m.u32(layout::ACTIVE_STRUCTURE)?;
    let index = m.u16(layout::BSP_INDEX)? as i16;
    let one = m.u32(layout::ONE)?;
    let limit = m.u32(SEGMENT_LIMIT)?;
    // The image's limit is `FLT_MAX`; the walk runs to `min(limit, 1)`.
    let mut env = Env::new(cpu);
    let addr = m.u32(SEGMENT_BSP)?;
    let mask = m
        .u32(BREAKABLES)?
        .wrapping_add((i32::from(index) as u32).wrapping_mul(32))
        .wrapping_add(1);
    // Ordered tests: a NaN limit is neither clamped.
    let fraction = if env.fp.comiss(limit, 0) == Some(Ordering::Less) {
        0
    } else {
        limit
    };
    let t1 = if env.fp.comiss(fraction, one) == Some(Ordering::Greater) {
        one
    } else {
        fraction
    };
    env.exact()?;
    let query = Segment {
        flags: ray.filter(),
        start: ray.start,
        delta: ray.delta,
        fraction,
        mask,
        breakable_count: 256,
        listed: 0,
        prev_leaf: NONE,
        prev_kind: 0,
        plane: NONE,
    };
    let b = bsp(g, world, addr)?;
    let m = Read(&*g, (0, 0));
    let outcome = env.vector_test(&b, &m, &query, t1)?;
    record.put(0, 0);
    record.put(4, one);
    let nowhere = location(&m, structure, index, NONE)?;
    record.put_location(0x14, nowhere);
    record.put_location(0x1c, nowhere);
    for offset in [0x38, 0x3c, 0x40] {
        record.put(offset, NONE);
    }
    if let Some(hit) = outcome.hit.filter(|_| ray.flags & 1 != 0) {
        record.structure_hit(&mut env, &m, structure, hit)?;
    }
    // R's overflow slot is outside the list J consumes. After 256 leaves
    // even the last location is the 256th listed leaf, not the overflow.
    let leaves = &outcome.leaves[..outcome
        .leaves
        .len()
        .min(sim_collision::bsp::segment::LEAF_LIST as usize)];
    let mut marks = clusters::Marks::default();
    let mut hit_bsp = None;
    if let (Some(&first), Some(&last)) = (leaves.first(), leaves.last()) {
        record.put_location(0x14, location(&m, structure, index, first)?);
        record.put_location(0x1c, location(&m, structure, index, last)?);
        if ray.flags & 14 != 0 && !(ray.flags & FIRST_HIT != 0 && record.word(0) != 0) {
            if ray.flags & 8 != 0 && ray.flags & 0x1fff0 == 0 {
                ray.flags |= 0x1fff0;
            }
            marks.walk(
                cpu,
                &mut env,
                g,
                world,
                structure,
                index,
                ray,
                leaves,
                &mut record,
                &mut hit_bsp,
            )?;
        }
    }
    record.set_position(ray.position(&mut env, record.word(4))?);
    if ray.flags & RELOCATE != 0 && record.word(0) != 0 {
        relocate::nudge(&mut env, g, world, ray, structure, index, hit_bsp, &mut record)?;
    }
    Ok(Plan {
        record,
        flags: ray.flags,
        marks,
        mxcsr: env.mxcsr(),
        x87: env.x87.flags(),
    })
}

/// Read-only gameplay query using nearest-even guest arithmetic. Hit fields
/// use binary32 bits and locations refer to original guest indices.
pub fn trace(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    start: [u32; 3],
    delta: [u32; 3],
    flags: u32,
    ignore: [u32; 2],
) -> Result<QueryResult, Bail> {
    let cpu = CpuAbi {
        mxcsr: 0x1fa0,
        ..CpuAbi::xbox_thread()
    };
    let p = plan(
        &cpu,
        g,
        world,
        Ray {
            start,
            delta,
            flags,
            ignore,
        },
        Record([0; RECORD_SIZE]),
    )?;
    Ok(p.into_result())
}

impl Plan {
    fn into_result(self) -> QueryResult {
        let r = self.record;
        let hit_type = r.word(0);
        QueryResult {
            hit_type,
            fraction: r.word(4),
            position: std::array::from_fn(|i| r.word(8 + 4 * i)),
            locations: [r.location(0x14), r.location(0x1c)],
            plane: (hit_type != 0).then(|| r.plane()),
            material: (hit_type != 0).then(|| r.half(0x24)),
            cluster_epoch: self.marks.cluster_epoch,
            instance_epoch: self.marks.instance_epoch,
            object_epoch: self.marks.object_epoch,
            cluster_marks: self.marks.clusters,
            instance_marks: self.marks.instances,
            object_marks: self.marks.objects,
        }
    }
}
