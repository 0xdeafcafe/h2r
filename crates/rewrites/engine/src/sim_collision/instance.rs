//! Instanced-geometry traces for one structure cluster (entry E).
mod parity;
pub use parity::ReturnPrediction;

use super::{
    CollisionMemory, Outputs, Read, consts, finish, layout, layout::mopp_ray as ray_layout,
};
use crate::sim_native::sub_flags;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::instance::PlacementMatrix;
use sim_collision::{Bail, CollisionWorld, Entry, GuestRead};
use sim_collision::{
    Hit, Segment,
    bsp::NONE,
    mopp,
    mopp::ray::{Constants, Event, Precision, Ray, RayWalk},
    sphere::Sphere,
};
use sim_core::det::{self, Fp};
use std::{cmp::Ordering, collections::BTreeSet};

const CLUSTERS: u32 = 0xa0;
const CLUSTER_SIZE: u32 = 0xb0;
const CODE_COUNT: u32 = 0xa8;

fn cluster_at(m: &impl GuestRead, structure: u32, index: u32) -> Result<u32, Bail> {
    let base = m.u32(structure.wrapping_add(CLUSTERS))?;
    Ok(base.wrapping_add(index.wrapping_mul(CLUSTER_SIZE)))
}

/// The count test precedes all floating point work and all reads of the
/// three arguments. Replay its complete frame and volatile registers.
/// A populated cluster continues into the guest with its original CPU.
pub fn empty_cluster(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = (|| -> Result<_, Bail> {
        let saved_bp = c.esp.checked_sub(4).ok_or(Bail::Fault)?;
        let frame = (saved_bp & !15).checked_sub(0x110).ok_or(Bail::Fault)?;
        let output = Outputs::new(g, c.esp, c.esp - frame, [])?;
        let m = Read(&*g, (0, 0));
        output.separate(g, layout::ACTIVE_STRUCTURE, 4)?;
        let structure = m.u32(layout::ACTIVE_STRUCTURE)?;
        output.separate(g, structure.wrapping_add(CLUSTERS), 4)?;
        let cluster = cluster_at(&m, structure, c.eax)?;
        output.separate(g, cluster.wrapping_add(CODE_COUNT), 4)?;
        let count = m.u32(cluster.wrapping_add(CODE_COUNT))?;
        Ok((saved_bp, frame, output.target, structure, cluster, count))
    })();
    if matches!(&plan, Ok((_, _, _, _, _, count)) if (*count as i32) > 0) {
        // A passive observer compares this call when its guest frame returns.
        return Ok(None);
    }
    world.record(Entry::InstanceTrace, &plan);
    let Ok((saved_bp, frame, target, structure, cluster, count)) = plan else {
        return Ok(None);
    };
    g.set_u32(saved_bp, c.ebp)?;
    g.set_u32(frame, c.ebx)?;
    g.write_bytes(frame + 0x13, &[0], 1)?;
    c.ecx = structure;
    c.edx &= !0xff;
    c.eax = cluster & !0xff;
    sub_flags(c, count, 0, 32);
    Ok(Some(finish(c, target, 12)))
}

pub const MARK_BASE: u32 = 0x004e_7c20;
/// The next global bounds the instance mark array, independently of the placement block.
pub const MARK_SLOTS: u32 = (crate::points::PLAYER_GLOBALS.base - MARK_BASE) / 4;
const BREAKABLE_BASE: u32 = 0x004e_d280;
const DEFAULT_MATERIAL: u32 = 0x0047_d8e0;

pub struct Query {
    pub cluster: u32,
    pub start: [u32; 3],
    pub delta: [u32; 3],
    pub flags: u32,
    pub filter: u32,
    pub limit: u32,
}

#[derive(Default)]
pub struct Prediction {
    /// First encounter of each unmarked key, even when its placement misses.
    pub marks: Vec<(u32, u32)>,
    pub hit: Option<InstanceHit>,
}

pub struct InstanceHit {
    key: u32,
    surface: Hit,
    material: u16,
    plane: [u32; 4],
}

impl InstanceHit {
    pub fn fraction(&self) -> u32 {
        self.surface.fraction
    }

    /// Writes relative to the caller's result. Padding and hit position
    /// keep the caller's bytes.
    pub fn writes(&self, mut store: impl FnMut(u32, &[u8])) {
        store(0, &3u32.to_le_bytes());
        store(0x24, &self.material.to_le_bytes());
        store(0x38, &u32::MAX.to_le_bytes());
        store(0x40, &u32::MAX.to_le_bytes());
        store(0x3c, &self.key.to_le_bytes());
        store(4, &self.surface.fraction.to_le_bytes());
        for (i, value) in self.plane.iter().enumerate() {
            store(0x28 + 4 * i as u32, &value.to_le_bytes());
        }
        store(0x4c, &self.surface.leaf.to_le_bytes());
        store(0x50, &self.surface.surface.to_le_bytes());
        store(0x54, &self.surface.plane.to_le_bytes());
        store(0x58, &[self.surface.flags]);
        store(0x59, &[self.surface.breakable]);
        store(0x5a, &self.surface.material.to_le_bytes());
    }
}

fn record(m: &impl GuestRead, block: u32, index: u32, stride: u32) -> Result<u32, Bail> {
    let count = m.u32(block)? as i32;
    if count <= 0 || index >= count as u32 {
        return Err(Bail::Data);
    }
    let offset = index.checked_mul(stride).ok_or(Bail::Data)?;
    m.u32(block.wrapping_add(4))?
        .checked_add(offset)
        .ok_or(Bail::Data)
}

/// Definition surface filter at 1689b0. The optional structure
/// exclusion table is indexed by placement, independently of the surface.
fn accepts(
    m: &impl GuestRead,
    structure: u32,
    def: u32,
    key: u32,
    flags: u32,
) -> Result<bool, Bail> {
    let count = m.u32(def.wrapping_add(0x98))? as i32;
    if count <= 0 {
        return Ok(false);
    }
    let last = record(m, def.wrapping_add(0x98), count as u32 - 1, 8)?;
    if m.u8(last.wrapping_add(4))? & 0x10 != 0 {
        return Ok(false);
    }
    if flags & 0x40000 == 0 || (m.u32(structure.wrapping_add(0xc4))? as i32) <= 0 {
        return Ok(true);
    }
    let table = m.u32(structure.wrapping_add(0xc8))?;
    if table == 0 {
        return Ok(true);
    }
    let count = m.u32(table.wrapping_add(0x40))? as i32;
    if (key as i32) < 0 || key as i32 >= count {
        return Ok(false);
    }
    let rows = m.u32(table.wrapping_add(0x44))?;
    Ok(m.u16(rows.wrapping_add(key.wrapping_mul(4)))? != u16::MAX)
}

fn four(m: &impl GuestRead, addr: u32) -> Result<[u32; 4], Bail> {
    let [x, y, z] = m.vec3(addr)?;
    Ok([x, y, z, m.u32(addr.wrapping_add(12))?])
}

fn placement(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    q: &Query,
    structure: u32,
    key: u32,
    limit: u32,
    one: u32,
) -> Result<Option<InstanceHit>, Bail> {
    let m = Read(&*g, (0, 0));
    let at = record(&m, structure.wrapping_add(0x140), key, 0x58)?;
    let def_index = m.u16(at.wrapping_add(0x34))? as i16;
    if def_index < 0 {
        return Err(Bail::Data);
    }
    let def = record(&m, structure.wrapping_add(0x138), def_index as u32, 0xc8)?;
    if !accepts(&m, structure, def, key, q.flags)? {
        return Ok(None);
    }
    let s = Sphere {
        center: m.vec3(at.wrapping_add(0x3c))?,
        radius: m.u32(at.wrapping_add(0x48))?,
    };
    let offset = s.offset(q.start)?;
    if !offset.contains_start() && !offset.sweep(q.delta)?.hit() {
        return Ok(None);
    }
    let matrix = PlacementMatrix::read(&m, at)?;
    let (start, delta) = matrix.local_ray(one, q.start, q.delta)?;
    let index = m.u16(layout::BSP_INDEX)? as i16;
    let mask = m
        .u32(BREAKABLE_BASE)?
        .wrapping_add((index as u32) << 10)
        .wrapping_add(key)
        .wrapping_add(0x201);
    let addr = def.wrapping_add(0x70);
    let rev = g.bsp_revision(addr)?;
    let m = Read(&*g, rev);
    let bsp = world.bsp(&m, addr, || consts(&m))?;
    let fraction = if det::compare(limit, 0) == Ordering::Less {
        0
    } else {
        limit
    };
    let end = if det::compare(fraction, one) == Ordering::Greater {
        one
    } else {
        fraction
    };
    let query = Segment {
        start,
        delta,
        flags: q.filter,
        breakable_count: 8,
        mask,
        fraction,
        listed: 0,
        prev_leaf: NONE,
        prev_kind: 0,
        plane: NONE,
    };
    let Some(surface) = bsp.vector_test(&m, &query, 0, 0, end)?.hit else {
        return Ok(None);
    };
    let material = if surface.material == u16::MAX {
        m.u16(DEFAULT_MATERIAL)?
    } else {
        let row = record(&m, structure.wrapping_add(0xc), surface.material.into(), 20)?;
        m.u16(row.wrapping_add(8))?
    };
    let plane = matrix.plane(four(&m, surface.plane_addr)?, surface.plane & 0x8000 != 0)?;
    Ok(Some(InstanceHit {
        key,
        surface,
        material,
        plane,
    }))
}

/// Predict the cluster's externally visible result and mark writes. The
/// owning whole query supplies its incremented epoch and the mark indices
/// of earlier clusters. This is a read-only arm: no callback runs against
/// guest memory and no field changes if a terminal declines.
pub fn predict_with_marks(
    c: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    q: Query,
    epoch: u32,
    already_marked: &[u32],
) -> Result<Prediction, Bail> {
    predict(c, g, world, q, epoch, already_marked, |g, key| {
        Ok(Read(g, (0, 0)).u32(MARK_BASE + 4 * key)? == epoch)
    })
}

fn predict<G: CollisionMemory>(
    c: &CpuAbi,
    g: &mut G,
    world: &CollisionWorld,
    q: Query,
    epoch: u32,
    already_marked: &[u32],
    marked: impl Fn(&G, u32) -> Result<bool, Bail>,
) -> Result<Prediction, Bail> {
    let m = Read(&*g, (0, 0));
    let structure = m.u32(layout::ACTIVE_STRUCTURE)?;
    let cluster = record(
        &m,
        structure.wrapping_add(CLUSTERS - 4),
        q.cluster,
        CLUSTER_SIZE,
    )?;
    let mut prediction = Prediction::default();
    if (m.u32(cluster.wrapping_add(CODE_COUNT))? as i32) <= 0 {
        return Ok(prediction);
    }
    if c.eflags & (1 << 10) != 0
        || !det::guest_env(c.mxcsr)
        || !det::host_fast_env()
        || !det::x87_env(c.x87.control, c.x87.status, c.x87.tag, 3)
    {
        return Err(Bail::Env);
    }
    let precision = Precision::from_control(c.x87.control)?;
    let code = m.u32(cluster.wrapping_add(0xac))?;
    if m.u32(code.wrapping_add(mopp::REFCOUNT))? == 0 || code & 15 != 0 {
        return Err(Bail::Data);
    }
    let rev = g.mopp_revision(code)?;
    let m = Read(&*g, (0, 0));
    let tree = world.mopp(&m, code, rev)?;
    let source = tree.source();
    let constants = Constants {
        zero: m.u32(layout::ZERO)?,
        one: m.u32(layout::ONE)?,
        unit: four(&m, ray_layout::UNIT)?,
        root_scale: m.u32(ray_layout::ROOT_SCALE)?,
    };
    let mut fp = Fp::new();
    fp.check(q.limit);
    let end: [u32; 3] = std::array::from_fn(|i| fp.add(q.start[i], q.delta[i]));
    if !fp.exact() {
        return Err(Bail::Float);
    }
    let ray = Ray {
        origin: [
            source.origin[0],
            source.origin[1],
            source.origin[2],
            source.scale,
        ],
        from: [q.start[0], q.start[1], q.start[2], 0],
        to: [end[0], end[1], end[2], 0],
        constants,
        precision,
    };
    let mut walk = RayWalk::new(source.bytecode.as_slice(), ray, &ray.root()?);
    let mut seen: BTreeSet<u32> = already_marked.iter().copied().collect();
    let mut fraction = q.limit;
    let mut limit = q.limit;
    while let Some(event) = walk.next(fraction)? {
        let Event::Key(key) = event else {
            continue;
        };
        // The mark uses the signed low half, while the placement uses the
        // entire key. Admit only keys naming both the same valid slot and
        // an existing placement.
        if key >= MARK_SLOTS {
            return Err(Bail::Data);
        }
        let mark = MARK_BASE + key * 4;
        if !seen.insert(key) || marked(g, key)? {
            continue;
        }
        prediction.marks.push((mark, epoch));
        if let Some(hit) = placement(g, world, &q, structure, key, limit, constants.one)? {
            limit = hit.fraction();
            prediction.hit = Some(hit);
            if q.flags & 0x1000_0000 != 0 {
                fraction = 0;
            } else if det::compare(limit, fraction) == Ordering::Less {
                fraction = limit;
            }
        }
    }
    Ok(prediction)
}
