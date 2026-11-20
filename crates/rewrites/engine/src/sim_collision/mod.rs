//! The guest side of `sim_collision` (docs/engine/collision-queries.md):
//! the collision entries taken over under `impl.sim.native.collision`.
//!
//! Each adapter reads the entry's arguments and the live guest words its
//! query needs, runs the kernel against the decoded BSP, and only then
//! writes what the original leaves in guest memory. It returns `Ok(None)`
//! with nothing written when the kernel bails; the installer then runs the
//! original from the untouched entry state.
//!
//! Registers follow the contract's clobber rule: the result register is
//! set as the original sets it, callee-saved registers, `esp` and the
//! return are exact, and the rest of each entry's clobber set (scratch
//! GPRs, XMM registers, EFLAGS, the x87 condition codes and last-
//! instruction words, the dead stack below `esp`) is left as it was.
//! The x87 precision flag the original's split divisions raise is set.
pub mod layout;

use crate::sim_native::Memory;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::bsp::NONE;
use sim_collision::mem::overlaps;
pub use sim_collision::{Bail, CollisionWorld, Entry, Stats};
use sim_collision::{Bsp, Consts, Fault, GuestRead, Outcome, Segment};
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
}

pub fn stats() -> Stats {
    WORLD.stats()
}

/// Guest memory as the kernels read it.
struct Read<'a, M>(&'a M);

impl<M: Memory> GuestRead for Read<'_, M> {
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

fn finish(c: &mut CpuAbi, g: &impl Memory, pop: u32) -> Result<TransferAbi, GuestFault> {
    let target = g.u32(c.esp)?;
    c.esp = c.esp.wrapping_add(4 + pop);
    Ok(TransferAbi::ret(target))
}

fn xmm0(c: &CpuAbi) -> u32 {
    u32::from_le_bytes(c.xmm[0][..4].try_into().unwrap())
}

/// Entry A, `0x14a280`: `eax` the start node, `ecx` the BSP, `edx` the
/// point; returns the leaf (or `-1`) in `eax`.
pub fn point_to_leaf(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let r = (|| {
        if !det::fast_env(c.mxcsr) {
            return Err(Bail::Env);
        }
        let m = Read(&*g);
        let bsp = world.bsp(&m, c.ecx, || consts(&m))?;
        bsp.point_to_leaf(c.eax, m.vec3(c.edx)?)
    })();
    world.record(Entry::PointToLeaf, &r);
    let Ok(leaf) = r else { return Ok(None) };
    c.eax = leaf;
    finish(c, g, 0).map(Some)
}

/// Entry B, `0x11bed0`: `edx` the point, `esi` the 8-byte location
/// `{leaf u32, cluster i16, bsp index i16}`. Writes `-1` forms when no
/// structure is loaded or the point is in solid.
pub fn location(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let out = c.esi;
    let r = (|| {
        let m = Read(&*g);
        let index = m.u16(layout::BSP_INDEX)?;
        if index == 0xffff {
            return Ok((NONE, 0xffff, index));
        }
        if !det::fast_env(c.mxcsr) {
            return Err(Bail::Env);
        }
        let bsp = world.bsp(&m, m.u32(layout::STRUCTURE_BSP)?, || consts(&m))?;
        let leaf = bsp.point_to_leaf(0, m.vec3(c.edx)?)?;
        if leaf == NONE {
            return Ok((NONE, 0xffff, index));
        }
        // The original stores the leaf before reading the cluster.
        let table_at = m
            .u32(layout::ACTIVE_STRUCTURE)?
            .wrapping_add(layout::STRUCTURE_LEAVES);
        let at = m.u32(table_at)?.wrapping_add(leaf.wrapping_mul(8));
        if overlaps(out, 4, layout::ACTIVE_STRUCTURE, 4)
            || overlaps(out, 4, table_at, 4)
            || overlaps(out, 4, at, 2)
        {
            return Err(Bail::Alias);
        }
        Ok((leaf, m.u16(at)?, index))
    })();
    world.record(Entry::Location, &r);
    let Ok((leaf, cluster, index)) = r else {
        return Ok(None);
    };
    g.set_u32(out, leaf)?;
    g.set_u16(out.wrapping_add(6), index)?;
    g.set_u16(out.wrapping_add(4), cluster)?;
    finish(c, g, 0).map(Some)
}

/// What a vector test writes, decided before anything is written.
struct Plan {
    record: u32,
    /// Entry C's record initialisation: the clamped fraction, count 0.
    init: Option<u32>,
    listed: u32,
    /// Entry D's context, whose leaf, state and plane words it updates.
    context: Option<u32>,
    outcome: Outcome,
}

fn vector_env(c: &CpuAbi, world: &CollisionWorld) -> Result<(), Bail> {
    world.record_precision(c.x87.control);
    // Two x87 registers for the matcher's loads; the split needs one.
    if !det::fast_env(c.mxcsr) || !det::x87_env(c.x87.control, c.x87.status, c.x87.tag, 2) {
        return Err(Bail::Env);
    }
    Ok(())
}

/// Output records may not overlap what the walk reads, nor the stack the
/// original's frames would write.
fn check_outputs(
    outputs: &[(u32, u32)],
    inputs: &[(u32, u32)],
    bsp: &Bsp,
    sp: u32,
) -> Result<(), Bail> {
    let dead = sp.wrapping_sub(layout::VECTOR_DEAD_BELOW);
    let dead_len = layout::VECTOR_DEAD_BELOW + 4 + layout::VECTOR_ARGS;
    for (i, &(a, n)) in outputs.iter().enumerate() {
        let clash = bsp.overlaps(a, n)
            || overlaps(a, n, dead, dead_len)
            || inputs.iter().any(|&(b, m)| overlaps(a, n, b, m))
            || outputs[i + 1..].iter().any(|&(b, m)| overlaps(a, n, b, m));
        if clash {
            return Err(Bail::Alias);
        }
    }
    Ok(())
}

fn check_mask_reads(outcome: &Outcome, outputs: &[(u32, u32)]) -> Result<(), Bail> {
    let clash = outcome
        .mask_reads
        .iter()
        .any(|&a| outputs.iter().any(|&(b, n)| overlaps(a, 4, b, n)));
    if clash { Err(Bail::Alias) } else { Ok(()) }
}

/// Entry C, `0x1de630`: `eax` R, `ecx` the query bits, `edx` the BSP,
/// `xmm0` the limit; stack: breakable count (`u16`), breakable mask,
/// start, delta. `ret 0x10`, the hit in `al`.
fn plan_vector(c: &CpuAbi, g: &impl Memory, world: &CollisionWorld) -> Result<Plan, Bail> {
    vector_env(c, world)?;
    let m = Read(g);
    let sp = c.esp;
    let (record, flags, bsp_addr, limit) = (c.eax, c.ecx, c.edx, xmm0(c));
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
    let outputs = [(record, layout::record::SIZE)];
    check_outputs(
        &outputs,
        &[(start_at, 12), (delta_at, 12), (layout::ONE, 4)],
        &bsp,
        sp,
    )?;
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
    check_mask_reads(&outcome, &outputs)?;
    Ok(Plan {
        record,
        init: Some(fraction),
        listed: 0,
        context: None,
        outcome,
    })
}

/// Entry D, `0x1de6d0`: stack: the context, the node, `t0`, `t1`.
/// `ret 0x10`, the hit in `al`.
fn plan_context(c: &CpuAbi, g: &impl Memory, world: &CollisionWorld) -> Result<Plan, Bail> {
    use layout::context as ctx;
    vector_env(c, world)?;
    let m = Read(g);
    let sp = c.esp;
    let arg = |k: u32| m.u32(sp.wrapping_add(4 * k));
    let (context, node, t0, t1) = (arg(1)?, arg(2)?, arg(3)?, arg(4)?);
    let b = m.bytes(context, ctx::SIZE as usize)?;
    let w = |at: u32| u32::from_le_bytes(b[at as usize..at as usize + 4].try_into().unwrap());
    let (start_at, delta_at, record) = (w(ctx::START), w(ctx::DELTA), w(ctx::RECORD));
    let (start, delta) = (m.vec3(start_at)?, m.vec3(delta_at)?);
    let fraction = m.u32(record.wrapping_add(layout::record::FRACTION))?;
    let listed = m.u32(record.wrapping_add(layout::record::COUNT))?;
    let bsp = world.bsp(&m, w(ctx::BSP), || consts(&m))?;
    let outputs = [(record, layout::record::SIZE), (context, ctx::SIZE)];
    check_outputs(&outputs, &[(start_at, 12), (delta_at, 12)], &bsp, sp)?;
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
    check_mask_reads(&outcome, &outputs)?;
    Ok(Plan {
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
    finish(c, g, layout::VECTOR_ARGS)
}

/// Entry C, `0x1de630`, natively.
pub fn vector_test(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = plan_vector(c, &*g, world);
    world.record(Entry::VectorTest, &plan);
    match plan {
        Ok(plan) => commit(c, g, plan).map(Some),
        Err(_) => Ok(None),
    }
}

/// Entry D, `0x1de6d0`, natively.
pub fn vector_context(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = plan_context(c, &*g, world);
    world.record(Entry::VectorContext, &plan);
    match plan {
        Ok(plan) => commit(c, g, plan).map(Some),
        Err(_) => Ok(None),
    }
}
