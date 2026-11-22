//! Guest adapters for native point and location queries and the differential
//! BSP segment kernels. See docs/engine/collision-queries.md.
//!
//! Each adapter reads the entry's arguments and the live guest words its
//! query needs, runs the kernel against the decoded BSP, and only then
//! writes what the original leaves in guest memory. It returns `Ok(None)`
//! with nothing written when the kernel bails; the installer then runs the
//! original from the untouched entry state.
//!
//! Point and location also replay the saved stack words, scratch registers
//! and flags. The vector entries set the result as the original does.
//! Callee-saved registers, EFLAGS, `esp` and the return are exact. The
//! remainder of the vector clobber set (scratch GPRs and XMM registers,
//! x87 condition codes, last-instruction words and the dead stack below
//! `esp`) is excluded only in the vector differential harness.
//! The x87 precision flag the original's split divisions raise is set.
mod cluster;
pub mod layout;
mod point;
mod preflight;
mod sources;
pub use point::{location, point_to_leaf};
pub use preflight::CollisionMemory;
use preflight::Outputs;

use crate::sim_native::Memory;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::bsp::NONE;
pub use sim_collision::{Bail, CollisionWorld, Entry, Stats};
use sim_collision::{Consts, Fault, GuestRead, Outcome, Segment};
use sim_core::Epoch;
use sim_core::det::{self, Fp};
use std::cmp::Ordering;
use std::sync::LazyLock;

/// Bumped when the collision BSPs in guest memory may have changed: map
/// load, structure switch, snapshot load.
pub static EPOCH: Epoch = Epoch::new();

/// The process-wide collision world.
pub static WORLD: LazyLock<CollisionWorld> = LazyLock::new(|| CollisionWorld::new(&EPOCH));

/// Drop every decoded BSP.
pub fn invalidate() {
    EPOCH.bump();
    WORLD.clear();
}

pub fn stats() -> Stats {
    WORLD.stats()
}

/// Guest memory as the kernels read it.
struct Read<'a, M>(&'a M, (u64, u64));

impl<M: CollisionMemory> GuestRead for Read<'_, M> {
    fn memory_revision(&self) -> (u64, u64) {
        self.1
    }

    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), Fault> {
        self.0.read_bytes(addr, dst, 1).map_err(|_| Fault { addr })
    }
}

fn consts(g: &impl GuestRead) -> Result<Consts, Bail> {
    let zero = g.u32(layout::ZERO)?;
    let b = g.bytes(layout::PROJECTION, 36)?;
    let projection = std::array::from_fn(|row| {
        std::array::from_fn(|k| i16::from_le_bytes([b[6 * row + 2 * k], b[6 * row + 2 * k + 1]]))
    });
    Ok(Consts { zero, projection })
}

fn finish(c: &mut CpuAbi, target: u32, pop: u32) -> TransferAbi {
    c.esp = c.esp.wrapping_add(4 + pop);
    TransferAbi::ret(target)
}

fn xmm0(c: &CpuAbi) -> u32 {
    u32::from_le_bytes(c.xmm[0][..4].try_into().unwrap())
}

/// What a vector test writes, decided before anything is written.
struct Plan {
    target: u32,
    record: u32,
    /// Entry C's record initialisation: the clamped fraction, count 0.
    init: Option<u32>,
    listed: u32,
    /// Entry D's context, whose leaf, state and plane words it updates.
    context: Option<u32>,
    outcome: Outcome,
}

fn vector_env(c: &CpuAbi) -> Result<(), Bail> {
    // Two x87 registers for the matcher's loads; the split needs one.
    if !det::fast_env(c.mxcsr)
        || !det::host_fast_env()
        || !det::x87_env(c.x87.control, c.x87.status, c.x87.tag, 2)
    {
        return Err(Bail::Env);
    }
    Ok(())
}

/// Entry C, `0x1de630`: `eax` R, `ecx` the query bits, `edx` the BSP,
/// `xmm0` the limit; stack: breakable count (`u16`), breakable mask,
/// start, delta. `ret 0x10`, the hit in `al`.
fn plan_vector(
    c: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Plan, Bail> {
    vector_env(c)?;
    let sp = c.esp;
    let (record, flags, bsp_addr, limit) = (c.eax, c.ecx, c.edx, xmm0(c));
    let revision = g.bsp_revision(bsp_addr)?;
    let m = Read(&*g, revision);
    let count = m.u16(sp.wrapping_add(4))?;
    let mask = m.u32(sp.wrapping_add(8))?;
    let (start_at, delta_at) = (m.u32(sp.wrapping_add(12))?, m.u32(sp.wrapping_add(16))?);
    let (start, delta) = (m.vec3(start_at)?, m.vec3(delta_at)?);
    let one = m.u32(layout::ONE)?;
    let mut fp = Fp::new();
    fp.check(limit);
    fp.check(one);
    if !fp.exact() {
        return Err(Bail::Float);
    }
    // `0 > limit` clamps both to zero; otherwise the walk runs to
    // `min(limit, 1)` with R's fraction at the limit.
    let negative = det::compare(0, limit) == Ordering::Greater;
    let fraction = if negative { 0 } else { limit };
    let t1 = if negative {
        0
    } else if det::compare(limit, one) == Ordering::Greater {
        one
    } else {
        limit
    };
    let bsp = world.bsp(&m, bsp_addr, || consts(&m))?;
    let outputs = Outputs::new(
        g,
        sp,
        layout::VECTOR_DEAD_BELOW,
        [
            (sp.wrapping_add(4), layout::VECTOR_ARGS),
            (record, layout::record::SIZE),
        ],
    )?;
    outputs.check_bsp(g, &bsp)?;
    outputs.separate(g, start_at, 12)?;
    outputs.separate(g, delta_at, 12)?;
    outputs.separate(g, layout::ONE, 4)?;
    let q = Segment {
        flags,
        breakable_count: count,
        mask,
        start,
        delta,
        fraction,
        listed: 0,
        prev_leaf: NONE,
        prev_kind: 0,
        plane: NONE,
    };
    let outcome = bsp.vector_test(&m, &q, 0, 0, t1)?;
    for &at in &outcome.mask_reads {
        outputs.separate(g, at, 4)?;
    }
    Ok(Plan {
        target: outputs.target,
        record,
        init: Some(fraction),
        listed: 0,
        context: None,
        outcome,
    })
}

/// Entry D, `0x1de6d0`: stack: the context, the node, `t0`, `t1`.
/// `ret 0x10`, the hit in `al`.
fn plan_context(
    c: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Plan, Bail> {
    use layout::context as ctx;
    vector_env(c)?;
    let m = Read(&*g, (0, 0));
    let sp = c.esp;
    let arg = |k: u32| m.u32(sp.wrapping_add(4 * k));
    let (context, node, t0, t1) = (arg(1)?, arg(2)?, arg(3)?, arg(4)?);
    let b = m.bytes(context, ctx::SIZE as usize)?;
    let w = |at: u32| u32::from_le_bytes(b[at as usize..at as usize + 4].try_into().unwrap());
    let (start_at, delta_at, record) = (w(ctx::START), w(ctx::DELTA), w(ctx::RECORD));
    let (start, delta) = (m.vec3(start_at)?, m.vec3(delta_at)?);
    let fraction = m.u32(record.wrapping_add(layout::record::FRACTION))?;
    let listed = m.u32(record.wrapping_add(layout::record::COUNT))?;
    let bsp_addr = w(ctx::BSP);
    let revision = g.bsp_revision(bsp_addr)?;
    let m = Read(&*g, revision);
    let bsp = world.bsp(&m, bsp_addr, || consts(&m))?;
    let outputs = Outputs::new(
        g,
        sp,
        layout::CONTEXT_DEAD_BELOW,
        [
            (sp.wrapping_add(4), layout::VECTOR_ARGS),
            (record, layout::record::SIZE),
            (context, ctx::SIZE),
        ],
    )?;
    outputs.check_bsp(g, &bsp)?;
    outputs.separate(g, start_at, 12)?;
    outputs.separate(g, delta_at, 12)?;
    let q = Segment {
        flags: w(ctx::FLAGS),
        breakable_count: w(ctx::BREAKABLE_COUNT) as u16,
        mask: w(ctx::MASK),
        start,
        delta,
        fraction,
        listed,
        prev_leaf: w(ctx::PREV_LEAF),
        prev_kind: b[ctx::PREV_KIND as usize],
        plane: w(ctx::PLANE),
    };
    let outcome = bsp.vector_test(&m, &q, node, t0, t1)?;
    for &at in &outcome.mask_reads {
        outputs.separate(g, at, 4)?;
    }
    Ok(Plan {
        target: outputs.target,
        record,
        init: None,
        listed,
        context: Some(context),
        outcome,
    })
}

fn commit(c: &mut CpuAbi, g: &mut impl Memory, plan: Plan) -> Result<TransferAbi, GuestFault> {
    use layout::record as r;
    let at = |offset: u32| plan.record.wrapping_add(offset);
    let o = &plan.outcome;
    if let Some(fraction) = plan.init {
        g.set_u32(at(r::FRACTION), fraction)?;
        g.set_u32(at(r::COUNT), 0)?;
    }
    let (count, writes) = o.list_writes(plan.listed);
    for (offset, leaf) in writes {
        g.set_u32(at(offset), leaf)?;
    }
    if count != plan.listed {
        g.set_u32(at(r::COUNT), count)?;
    }
    if let Some(h) = o.hit {
        g.set_u32(at(r::FRACTION), h.fraction)?;
        g.set_u32(at(r::PLANE), h.plane_addr)?;
        g.set_u32(at(r::LEAF), h.leaf)?;
        g.set_u32(at(r::SURFACE), h.surface)?;
        g.set_u32(at(r::SURFACE_PLANE), h.plane)?;
        g.set_u8(at(r::FLAGS), h.flags)?;
        g.set_u8(at(r::BREAKABLE), h.breakable)?;
        g.set_u16(at(r::MATERIAL), h.material)?;
    }
    if let Some(context) = plan.context {
        use layout::context as ctx;
        g.set_u32(context.wrapping_add(ctx::PREV_LEAF), o.prev_leaf)?;
        g.set_u8(context.wrapping_add(ctx::PREV_KIND), o.prev_kind)?;
        g.set_u32(context.wrapping_add(ctx::PLANE), o.plane)?;
    }
    if o.inexact {
        c.x87.status |= 0x20;
    }
    c.eax = (c.eax & !0xff) | u32::from(o.hit.is_some());
    // The last flag producer on every return is the epilogue's add to ESP.
    let frame = if plan.context.is_some() { 8 } else { 0x28 };
    let before = c.esp.wrapping_sub(frame);
    crate::sim_native::add_flags(c, before, frame);
    Ok(finish(c, plan.target, layout::VECTOR_ARGS))
}

/// Entry C, `0x1de630`, natively.
pub fn vector_test(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = plan_vector(c, g, world);
    world.record(Entry::VectorTest, &plan);
    match plan {
        Ok(plan) => commit(c, g, plan).map(Some),
        Err(_) => Ok(None),
    }
}

/// Entry D, `0x1de6d0`, natively.
pub fn vector_context(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = plan_context(c, g, world);
    world.record(Entry::VectorContext, &plan);
    match plan {
        Ok(plan) => commit(c, g, plan).map(Some),
        Err(_) => Ok(None),
    }
}
