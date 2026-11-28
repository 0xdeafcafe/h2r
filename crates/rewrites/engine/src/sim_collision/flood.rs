//! Entries G (`0x0014a5b0`, the sphere cluster flood) and H (`0x000bb050`,
//! the object spatial visit). See docs/engine/collision-queries.md.
//!
//! Both read their arguments and the live words they need, run the kernel
//! over the structure's decoded portal graph, and only then write what the
//! original leaves: the output array, the epochs, the active bytes and the
//! marks. The runtime dispatch in [`spatial`] replays the full
//! scratch state of the no-portal-test flood and the no-cluster visit.
//! Every other admitted call commits the kernel's plan. The x87 and SSE
//! PE flags are set as the region's operations set them.
use super::preflight::{CollisionMemory, Outputs};
use super::{Read, consts, finish, layout};
use crate::sim_native::{Memory, add_flags, logic_flags};
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::flood::{Flood, FloodQuery, MAX_CLUSTERS, Topology, TopologyKey};
use sim_collision::objects::{List, Visit, VisitQuery, object_visit_inexact};
use sim_collision::{Bail, CollisionWorld, Entry, GuestRead, Inexact};
use sim_core::det::{self, checked_inputs};
use std::sync::Arc;

pub mod spatial;

/// The x87 state the flood's sequences reproduce: masked, nearest, single
/// or double precision and two free registers. PE may be clear: the
/// commit sets it when a sequence rounds ([`x87_after`]).
fn x87_ready(c: &CpuAbi) -> bool {
    det::x87_env(c.x87.control, c.x87.status, c.x87.tag, 2) && c.x87.control & 0x300 != 0x300
}

/// `mxcsr` after a region whose SSE operations were `inexact`.
fn after(mxcsr: u32, inexact: Inexact) -> u32 {
    if inexact.sse { mxcsr | 0x20 } else { mxcsr }
}

/// The x87 status word after a region whose x87 sequences were `inexact`.
fn x87_after(status: u16, inexact: Inexact) -> u16 {
    if inexact.x87 { status | 0x20 } else { status }
}

fn sse_ready(c: &CpuAbi) -> Result<(), Bail> {
    if !det::checked_guest_env(c.mxcsr) || !det::host_fast_env() {
        return Err(Bail::Env);
    }
    Ok(())
}

/// The active structure's portal graph, decoded once per version its
/// provider vouches for.
fn topology(g: &mut impl CollisionMemory, world: &CollisionWorld) -> Result<Arc<Topology>, Bail> {
    let (key, projection) = {
        let m = Read(&*g, (0, 0));
        let key = TopologyKey {
            structure: m.u32(layout::ACTIVE_STRUCTURE)?,
            bsp: m.u32(layout::STRUCTURE_BSP)?,
        };
        (key, consts(&m)?.projection)
    };
    if let Some(version) = g.topology_version(key, None)
        && let Some(t) = world.cached_topology(key, version)
    {
        return Ok(t);
    }
    let t = Topology::decode(&Read(&*g, (0, 0)), key, &projection)?;
    let version = g.topology_version(key, Some(t.sources()));
    Ok(world.keep_topology(version, t))
}

/// The words the topology names besides its own sources.
const TOPOLOGY_GLOBALS: [(u32, u32); 3] = [
    (layout::ACTIVE_STRUCTURE, 4),
    (layout::STRUCTURE_BSP, 4),
    (layout::PROJECTION, 36),
];

fn separate_topology<const N: usize>(
    outputs: &Outputs<N>,
    g: &impl CollisionMemory,
    t: &Topology,
) -> Result<(), Bail> {
    for &(at, len) in t.sources().iter().chain(&TOPOLOGY_GLOBALS) {
        outputs.separate(g, at, len)?;
    }
    Ok(())
}

/// An output span, or an empty one at the return slot when nothing is
/// written (the original does not touch the pointer then).
fn span(sp: u32, at: u32, len: u32) -> (u32, u32) {
    if len == 0 { (sp, 0) } else { (at, len) }
}

/// What a flood writes, decided before anything is written.
struct FloodPlan {
    target: u32,
    out: u32,
    epoch: u32,
    structure: u32,
    flood: Flood,
    /// The MXCSR after the flood: PE set when an SSE operation was inexact.
    mxcsr: u32,
    /// The x87 status after the flood: PE set when a sequence rounded.
    x87_status: u16,
}

/// Entry G, `0x14a5b0`: stack: cluster (`i16`), centre, radius, capacity,
/// out. `ret 0x14`, the count in `ax`.
fn plan_flood(
    c: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<FloodPlan, Bail> {
    sse_ready(c)?;
    if !x87_ready(c) {
        return Err(Bail::Env);
    }
    let sp = c.esp;
    let [cluster, centre_at, radius, capacity, out] = {
        let m = Read(&*g, (0, 0));
        let mut args = [0; 5];
        for (k, a) in args.iter_mut().enumerate() {
            *a = m.u32(sp.wrapping_add(4 + 4 * k as u32))?;
        }
        args
    };
    let t = topology(g, world)?;
    let m = Read(&*g, (0, 0));
    let epoch = m.u32(layout::FLOOD_EPOCH)?;
    let q = FloodQuery {
        cluster: cluster as u16,
        centre: m.vec3(centre_at)?,
        radius,
        capacity,
        control: c.x87.control,
        marks: layout::CLUSTER_MARKS,
        epoch,
    };
    let (flood, inexact) = t.cluster_flood_inexact(&m, &q)?;
    let outputs = Outputs::new(
        g,
        sp,
        layout::flood_dead_below(flood.deepest),
        [
            span(sp, out, 2 * flood.written as u32),
            (layout::CLUSTER_MARKS, 4 * MAX_CLUSTERS),
            (sp.wrapping_add(4), layout::FLOOD_ARGS),
        ],
    )?;
    outputs.separate(g, centre_at, 12)?;
    outputs.separate(g, layout::FLOOD_EPOCH, 4)?;
    separate_topology(&outputs, g, &t)?;
    Ok(FloodPlan {
        target: outputs.target,
        out,
        epoch,
        structure: t.key().structure,
        flood,
        mxcsr: after(c.mxcsr, inexact),
        x87_status: x87_after(c.x87.status, inexact),
    })
}

fn commit_flood(c: &mut CpuAbi, g: &mut impl Memory, p: FloodPlan) -> Result<TransferAbi, GuestFault> {
    c.mxcsr = p.mxcsr;
    c.x87.status = p.x87_status;
    let f = &p.flood;
    for (k, &cluster) in f.clusters.iter().enumerate() {
        if k < f.written {
            g.set_u16(p.out.wrapping_add(2 * k as u32), cluster)?;
        }
        g.set_u32(layout::CLUSTER_MARKS + 4 * u32::from(cluster), p.epoch)?;
    }
    c.eax = u32::from(f.count());
    // The slot saved by PUSH ECX is reused for the active structure;
    // POP ECX loads that pointer after the last comparison.
    c.ecx = p.structure;
    if f.portal_count as i32 <= 0 {
        // `test ecx, ecx` before the empty loop.
        logic_flags(c, f.portal_count);
    } else {
        // The loop's last `cmp eax, ecx` with both at the count.
        c.eflags = (c.eflags & !0x8d5) | 0x44;
        c.eflags_defined |= 0x8d5;
    }
    Ok(finish(c, p.target, layout::FLOOD_ARGS))
}

/// Entry G, `0x14a5b0`, natively.
pub fn cluster_flood(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = plan_flood(c, g, world);
    world.record(Entry::ClusterFlood, &plan);
    match plan {
        Ok(plan) => commit_flood(c, g, plan).map(Some),
        Err(_) => Ok(None),
    }
}

/// What a visit writes, decided before anything is written.
pub struct VisitPlan {
    target: u32,
    out: u32,
    flood_epoch: u32,
    generation: u32,
    empty_location: bool,
    lists: u32,
    types: u32,
    radius: u32,
    pub visit: Visit,
    /// The MXCSR after the visit: PE set when an SSE operation was inexact.
    pub mxcsr: u32,
    /// The x87 status after the visit: PE set when a flood sequence rounded.
    pub x87_status: u16,
}

/// Entry H, `0x0bb050`: stack: lists, types, location, centre, radius,
/// out, capacity. `ret 0x1c`, the count in `ax`.
fn plan_visit(
    c: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<VisitPlan, Bail> {
    let sp = c.esp;
    let args = {
        let m = Read(&*g, (0, 0));
        let mut args = [0; 7];
        for (k, a) in args.iter_mut().enumerate() {
            *a = m.u32(sp.wrapping_add(4 + 4 * k as u32))?;
        }
        args
    };
    plan_visit_at(c, g, world, sp, args)
}

/// The visit `0x0bb050` would make as called with `args` (lists, types,
/// location, centre, radius, out, capacity) and the stack pointer `sp` at
/// its entry (the return address's slot), without the arguments being on
/// the stack. A caller that is itself a Rust body plans the visit it would
/// have called this way ([`visit_writes`] is what the visit leaves).
pub fn plan_visit_at(
    c: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    sp: u32,
    args: [u32; 7],
) -> Result<VisitPlan, Bail> {
    sse_ready(c)?;
    let (cluster, zero) = {
        let m = Read(&*g, (0, 0));
        (m.u16(args[2].wrapping_add(4))?, m.u32(layout::ZERO)?)
    };
    let [lists, types, location, centre_at, radius, out, capacity] = args;
    // The graph is needed only when the visit floods.
    let floods = cluster != u16::MAX
        && checked_inputs(&[radius, zero])
        && f32::from_bits(radius) > f32::from_bits(zero);
    let t = if floods && x87_ready(c) {
        Some(topology(g, world)?)
    } else {
        None
    };
    let m = Read(&*g, (0, 0));
    let flood_epoch = m.u32(layout::FLOOD_EPOCH)?;
    let generation = m.u32(layout::VISIT_GENERATION)?;
    let q = VisitQuery {
        lists,
        types,
        cluster,
        centre: m.vec3(centre_at)?,
        radius,
        capacity: capacity as u16,
        zero,
        control: c.x87.control,
        x87: x87_ready(c),
        cluster_marks: layout::CLUSTER_MARKS,
        flood_epoch,
        object_marks: layout::OBJECT_MARKS,
        generation,
        slots: layout::OBJECT_MARK_SLOTS,
        lists_at: [
            List {
                heads: layout::COLLIDEABLE_HEADS,
                array: layout::COLLIDEABLE_ARRAY,
            },
            List {
                heads: layout::OTHER_HEADS,
                array: layout::OTHER_ARRAY,
            },
        ],
    };
    let (visit, inexact) = object_visit_inexact(&m, &q, || t.clone().ok_or(Bail::Data))?;
    let outputs = Outputs::new(
        g,
        sp,
        layout::visit_dead_below(visit.flood.as_ref().map(|f| f.deepest)),
        [
            span(sp, out, 4 * visit.handles.len() as u32),
            (layout::VISIT_ACTIVE, 8),
            (layout::OBJECT_MARKS, 4 * layout::OBJECT_MARK_SLOTS),
            (layout::FLOOD_ACTIVE, 7),
            (layout::CLUSTER_MARKS, 4 * MAX_CLUSTERS),
            (sp.wrapping_add(4), layout::VISIT_ARGS),
        ],
    )?;
    outputs.separate(g, location.wrapping_add(4), 2)?;
    outputs.separate(g, centre_at, 12)?;
    outputs.separate(g, layout::ZERO, 4)?;
    for &(at, len) in &visit.reads {
        outputs.separate(g, at, len)?;
    }
    if let Some(t) = &t
        && visit.flood.is_some()
    {
        separate_topology(&outputs, g, t)?;
    }
    Ok(VisitPlan {
        target: outputs.target,
        out,
        flood_epoch,
        generation,
        empty_location: cluster == u16::MAX,
        lists,
        types,
        radius,
        visit,
        mxcsr: after(c.mxcsr, inexact),
        x87_status: x87_after(c.x87.status, inexact),
    })
}

/// The words a visit leaves, as (address, value, width in bytes): the
/// flood's epoch, its clusters' marks and its active byte, the visit's
/// generation, the objects' marks and its active byte.
pub fn visit_writes(p: &VisitPlan) -> Vec<(u32, u32, u8)> {
    let v = &p.visit;
    let mut writes = Vec::new();
    if let Some(f) = &v.flood {
        let epoch = p.flood_epoch.wrapping_add(1);
        writes.push((layout::FLOOD_EPOCH, epoch, 4));
        for &cluster in &f.clusters {
            writes.push((layout::CLUSTER_MARKS + 4 * u32::from(cluster), epoch, 4));
        }
        writes.push((layout::FLOOD_ACTIVE, 0, 1));
    }
    let generation = p.generation.wrapping_add(1);
    writes.push((layout::VISIT_GENERATION, generation, 4));
    for &slot in &v.marks {
        writes.push((layout::OBJECT_MARKS + 4 * u32::from(slot), generation, 4));
    }
    writes.push((layout::VISIT_ACTIVE, 0, 1));
    writes
}

fn commit_visit(c: &mut CpuAbi, g: &mut impl Memory, p: VisitPlan) -> Result<TransferAbi, GuestFault> {
    c.mxcsr = p.mxcsr;
    c.x87.status = p.x87_status;
    let v = &p.visit;
    for (at, value, width) in visit_writes(&p) {
        if width == 1 {
            g.set_u8(at, value as u8)?;
        } else {
            g.set_u32(at, value)?;
        }
    }
    for (i, &handle) in v.handles.iter().enumerate() {
        g.set_u32(p.out.wrapping_add(4 * i as u32), handle)?;
    }
    c.eax = v.eax();
    // Both returns end with `add esp, 0x420`.
    add_flags(c, c.esp.wrapping_sub(0x420), 0x420);
    Ok(finish(c, p.target, layout::VISIT_ARGS))
}

/// Entry H, `0x0bb050`, natively.
pub fn object_spatial_visit(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = plan_visit(c, g, world);
    world.record(Entry::ObjectVisit, &plan);
    match plan {
        Ok(plan) => commit_visit(c, g, plan).map(Some),
        Err(_) => Ok(None),
    }
}
