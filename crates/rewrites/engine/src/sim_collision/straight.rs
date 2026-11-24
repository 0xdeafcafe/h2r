//! Exact exits for C and D when the segment stays on one side at every
//! node and the leaf transition skips the matcher. The deepest guest
//! frame in this case is D's four saved registers and two locals.
use super::{CollisionMemory, Plan, finish, layout};
#[cfg(feature = "differential")]
use super::{CollisionWorld, plan_context, plan_vector};
use crate::sim_native::add_flags;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::bsp::{NONE, segment::StraightWalk};
use sim_core::det;
use std::cmp::Ordering;

pub(super) const WRAPPER_AFTER_WALK: u32 = 0x001d_e6bd;
const WRAPPER_ARGUMENTS_AND_RETURN: u32 = 20;

/// All these ranges were validated by the vector/context planner. Outputs
/// and input spans (including physical aliases) are disjoint.
pub(super) fn commit(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    plan: &Plan,
    walk: StraightWalk,
) -> Result<TransferAbi, GuestFault> {
    use layout::{context as ctx, record};
    let sp = c.esp;
    let leaf = plan.outcome.prev_leaf;
    let kind = plan.outcome.prev_kind;
    let (frame, context, flags) = match plan.context {
        Some(context) => (sp, context, g.u32(context + ctx::FLAGS)?),
        None => {
            // C builds its 40-byte context and four D arguments. D sees
            // the clamped interval [0, t1]; R keeps the full limit.
            let fraction = plan.init.expect("wrapper initializes R");
            let one = g.u32(layout::ONE)?;
            let t1 = if det::compare(0, super::xmm0(c)) == Ordering::Greater {
                0
            } else if det::compare(fraction, one) == Ordering::Greater {
                one
            } else {
                fraction
            };
            let count = g.u16(sp + 4)?;
            let mask = g.u32(sp + 8)?;
            let start = g.u32(sp + 12)?;
            let delta = g.u32(sp + 16)?;
            let context = sp - ctx::SIZE;
            let frame = context - WRAPPER_ARGUMENTS_AND_RETURN;
            g.set_u32(sp + 4, t1)?;
            g.set_u32(context + ctx::FLAGS, c.ecx)?;
            g.set_u32(context + ctx::BSP, c.edx)?;
            g.set_u16(context + ctx::BREAKABLE_COUNT, count)?;
            g.set_u32(context + ctx::MASK, mask)?;
            g.set_u32(context + ctx::START, start)?;
            g.set_u32(context + ctx::DELTA, delta)?;
            g.set_u32(context + ctx::RECORD, plan.record)?;
            g.set_u32(context + ctx::PLANE, NONE)?;
            // The leaf tail replaces t1 entirely and only the low byte of
            // the starting node. The other argument slots may survive.
            g.set_u32(frame, WRAPPER_AFTER_WALK)?;
            g.set_u32(frame + 4, context)?;
            g.set_u32(frame + 8, 0)?;
            g.set_u32(frame + 12, 0)?;
            g.set_u32(plan.record + record::FRACTION, fraction)?;
            g.set_u32(plan.record + record::COUNT, 0)?;
            (frame, context, c.ecx)
        }
    };
    g.set_u32(frame - 12, c.ebx)?;
    g.set_u32(frame - 16, c.ebp)?;
    g.set_u32(frame - 20, c.esi)?;
    g.set_u32(frame - 24, c.edi)?;
    g.set_u32(frame - 4, flags)?;
    g.set_u8(frame - 8, 0)?;
    if let Some(last) = walk.last {
        g.set_u32(frame + 4, last.distance)?;
        for (reg, value) in [0, last.delta, last.end, last.start]
            .into_iter()
            .enumerate()
        {
            c.xmm[reg] = u128::from(value).to_le_bytes();
        }
    }
    g.set_u8(frame + 8, kind)?;
    g.set_u32(frame + 16, leaf)?;
    g.set_u32(context + ctx::PREV_LEAF, leaf)?;
    g.set_u8(context + ctx::PREV_KIND, kind)?;
    c.edx = flags & 1;
    if leaf == NONE {
        c.eax = NONE & !0xff;
        c.ecx = flags;
    } else {
        let count = plan.listed;
        if count < sim_collision::bsp::segment::LEAF_LIST {
            g.set_u32(plan.record + record::LEAVES + count * 4, leaf)?;
            g.set_u32(plan.record + record::COUNT, count + 1)?;
        } else {
            g.set_u32(plan.record + record::OVERFLOW, leaf)?;
        }
        c.eax = plan.record & !0xff;
        c.ecx = count;
    }
    let locals = if plan.context.is_some() { 8 } else { ctx::SIZE };
    add_flags(c, sp - locals, locals);
    Ok(finish(c, plan.target, layout::VECTOR_ARGS))
}

/// Exact path used by the synthetic differential and the disc sweep.
#[cfg(feature = "differential")]
pub fn vector_test(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let Ok(plan) = plan_vector(c, g, world) else {
        return Ok(None);
    };
    plan.outcome
        .straight
        .map(|walk| commit(c, g, &plan, walk))
        .transpose()
}

#[cfg(feature = "differential")]
pub fn vector_context(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let Ok(plan) = plan_context(c, g, world) else {
        return Ok(None);
    };
    plan.outcome
        .straight
        .map(|walk| commit(c, g, &plan, walk))
        .transpose()
}
