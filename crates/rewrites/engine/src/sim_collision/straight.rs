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
            let count = g.u16(sp.wrapping_add(4))?;
            let mask = g.u32(sp.wrapping_add(8))?;
            let start = g.u32(sp.wrapping_add(12))?;
            let delta = g.u32(sp.wrapping_add(16))?;
            let context = sp - ctx::SIZE;
            let frame = context - WRAPPER_ARGUMENTS_AND_RETURN;
            g.set_u32(sp.wrapping_add(4), t1)?;
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
            g.set_u32(frame.wrapping_add(4), context)?;
            g.set_u32(frame.wrapping_add(8), 0)?;
            g.set_u32(frame.wrapping_add(12), 0)?;
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
        g.set_u32(frame.wrapping_add(4), last.distance)?;
        for (reg, value) in [0, last.delta, last.end, last.start]
            .into_iter()
            .enumerate()
        {
            c.xmm[reg] = u128::from(value).to_le_bytes();
        }
    }
    g.set_u8(frame.wrapping_add(8), kind)?;
    g.set_u32(frame.wrapping_add(16), leaf)?;
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
    // The walk's SSE arithmetic raised these under the guest's MXCSR.
    c.mxcsr |= plan.outcome.mxcsr.unwrap_or(0) & 0x3f;
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

#[cfg(test)]
mod tests {
    use super::super::{CollisionWorld, layout, vector_parity};
    use runtime::{CpuAbi, Guest, guest::Permissions};
    use sim_collision::{
        Bail, Entry,
        synth::{Builder, SOLID},
    };
    use sim_core::Epoch;

    const DATA: u32 = 0x10_000;
    const BSP: u32 = DATA;
    const RECORD: u32 = DATA + 0x1000;
    const CONTEXT: u32 = DATA + 0x2000;
    const START: u32 = DATA + 0x3000;
    const DELTA: u32 = START + 16;
    const SP: u32 = 0x30_000;
    const FRAME_BYTES: u32 = 0x4000;
    const FRAME_PHYS: u32 = 0x10_000;

    fn snapshot(g: &Guest) -> Vec<u8> {
        let mut bytes = Vec::new();
        // Read each mapping separately, including both virtual names of
        // any aliased bytes. Split mappings must not conceal partial writes.
        for m in g.mappings() {
            let start = bytes.len();
            bytes.resize(start + m.len as usize, 0);
            g.read_bytes(m.virtual_start, &mut bytes[start..], 1)
                .unwrap();
        }
        bytes
    }

    #[test]
    fn mapped_return_and_arguments_cannot_alias_the_discarded_frame() {
        for entry in [Entry::VectorTest, Entry::VectorContext] {
            for (ret, args, alias) in [
                (0x20_000, 0x21_000, false),
                (FRAME_PHYS + FRAME_BYTES - 8, 0x21_000, true),
                (0x20_000, FRAME_PHYS + FRAME_BYTES - 16, true),
            ] {
                let mut g = Guest::new();
                g.map(DATA, 0x4000, 0, Permissions::RW).unwrap();
                g.map(layout::PROJECTION, 36, 0x4000, Permissions::RW)
                    .unwrap();
                g.map(layout::ONE, 0x40, 0x5000, Permissions::RW).unwrap();
                g.map(SP - FRAME_BYTES, FRAME_BYTES, FRAME_PHYS, Permissions::RW)
                    .unwrap();
                g.map(SP, 4, ret, Permissions::RW).unwrap();
                g.map(SP + 4, 16, args, Permissions::RW).unwrap();
                g.set_u32(layout::ONE, 1.0f32.to_bits()).unwrap();
                g.set_u32(SP, 0xabcd_ef00).unwrap();
                let mut tree = Builder::default();
                tree.planes.push([1.0, 0.0, 0.0, 0.0]);
                tree.nodes.push((0, SOLID, SOLID));
                tree.write(BSP, |a, b| g.write_bytes(a, b, 1).unwrap());
                let mut c = CpuAbi {
                    eax: RECORD,
                    edx: BSP,
                    esp: SP,
                    mxcsr: 0x1fa0,
                    ..CpuAbi::default()
                };
                let dispatch = if entry == Entry::VectorTest {
                    g.set_u32(SP + 12, START).unwrap();
                    g.set_u32(SP + 16, DELTA).unwrap();
                    vector_parity::dispatch_vector
                } else {
                    use layout::context as ctx;
                    g.set_u32(SP + 4, CONTEXT).unwrap();
                    g.set_u32(CONTEXT + ctx::BSP, BSP).unwrap();
                    g.set_u32(CONTEXT + ctx::RECORD, RECORD).unwrap();
                    g.set_u32(CONTEXT + ctx::START, START).unwrap();
                    g.set_u32(CONTEXT + ctx::DELTA, DELTA).unwrap();
                    vector_parity::dispatch_context
                };
                static EPOCH: Epoch = Epoch::new();
                let world = CollisionWorld::new(&EPOCH);
                // D under C's wrapper (the lifted C's own call) skips the
                // frame replay, which is the wrapper's, but is answered.
                if entry == Entry::VectorContext && !alias {
                    g.set_u32(SP, super::WRAPPER_AFTER_WALK).unwrap();
                    let mut wrapped = c;
                    let ret = dispatch(&mut wrapped, &mut g, &world).unwrap();
                    assert_eq!(ret.unwrap().target_pc, super::WRAPPER_AFTER_WALK);
                    assert_eq!(wrapped.esp, SP + 4 + layout::VECTOR_ARGS);
                    g.set_u32(SP, 0xabcd_ef00).unwrap();
                }
                let before = (c, snapshot(&g));
                let result = dispatch(&mut c, &mut g, &world).unwrap();
                if alias {
                    assert!(result.is_none(), "{entry:?} ret {ret:#x} args {args:#x}");
                    assert_eq!(c, before.0);
                    assert_eq!(snapshot(&g), before.1);
                    assert_eq!(
                        world.stats().entries[entry as usize].bails[Bail::Alias as usize],
                        1
                    );
                } else {
                    assert_eq!(result.unwrap().target_pc, 0xabcd_ef00);
                }
            }
        }
    }
}
