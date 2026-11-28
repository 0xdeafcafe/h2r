//! Unit pose entry and guest-call continuations. The real stack frame and
//! CPU are the only state carried between calls into the animation manager.
use super::*;
use crate::function_eval::constant::{ONE, SMALL};
use x87::abi as fp;

mod aim_overlays;
mod angles;
mod layers;
mod tail;

pub use super::plan::SegmentMemory as PoseMemory;

/// Return selectors of all the calls made by the base-image unit pose body.
pub const SITES: &[u32] = &[
    0x112194, 0x1121d1, 0x1121f5, 0x112203, 0x11225f, 0x11228b, 0x1122b5, 0x1122e1, 0x11230b,
    0x112372, 0x11238e, 0x1123ba, 0x112462, 0x112490, 0x1124e6, 0x11259f, 0x1125c8, 0x112777,
    0x112795, 0x1127e4, 0x1127f5, 0x112807, 0x112813, 0x11282b, 0x112841, 0x112901, 0x1129b5,
    0x1129e7, 0x112a32, 0x112a56, 0x112b15, 0x112ba0, 0x112bbb, 0x112c2b, 0x112c59, 0x112ca2,
    0x112cd8, 0x112d2a, 0x112d54, 0x112d84, 0x112dae,
];

enum Step {
    Next(u32),
    Transfer(TransferAbi),
}
type Segment = std::result::Result<Step, GuestFault>;

fn invoke(c: &mut CpuAbi, g: &mut impl Memory, target: u32, resume: u32) -> Segment {
    call(c, g, target, resume).map(Step::Transfer)
}

fn drive(c: &mut CpuAbi, g: &mut impl Memory, mut pc: u32) -> Result {
    loop {
        let step = match pc {
            ..0x1124e6 => layers::run(c, g, pc),
            0x1124e6..0x112769 => angles::run(c, g, pc),
            0x112769..0x1129c5 => aim_overlays::run(c, g, pc),
            _ => tail::run(c, g, pc),
        }?;
        match step {
            Step::Next(next) => pc = next,
            Step::Transfer(transfer) => return Ok(transfer),
        }
    }
}

/// Entry-only port used by the independent per-function shadow switch.
pub fn pose(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    drive(c, g, guest_layout::points::SIM_UNIT_POSE.base)
}

/// Plan one segment with a private CPU and ordered stores. A callee may
/// change the inputs for the next segment, even on a recursive pose call.
pub fn run(
    c: &mut CpuAbi,
    g: &mut impl PoseMemory,
    pc: u32,
) -> std::result::Result<Option<TransferAbi>, GuestFault> {
    super::plan::segment::<64, _>(
        c, g, pc, guest_layout::points::SIM_UNIT_POSE.base..0x112dbb, SITES,
        |cpu, plan, site| drive(cpu, plan, site),
    )
}

// Decide whether this aim channel differs enough from its neutral pose.
// The clamp is SSE. The threshold test retains extended precision.
fn aim_weight(c: &mut CpuAbi, g: &mut impl Memory) -> std::result::Result<bool, GuestFault> {
    const HALF: u32 = 0x0045_dbbc;
    let half = g.u32(HALF)?;
    let scaled = op(c, Mul, g.u32(c.esi)?, half);
    let weight = op(c, Add, scaled, half);
    put_xmm(c, 0, scalar(weight));
    put_xmm(c, 1, scalar(0));
    let local = at(c.esp, 20);
    let below = compare(c, 0, weight) == 0;
    g.set_u32(local, weight)?;
    if below {
        g.set_u32(local, 0)?;
    } else {
        let one = g.u32(ONE)?;
        put_xmm(c, 1, scalar(one));
        if compare(c, weight, one) == 0 {
            g.set_u32(local, one)?;
        }
    }
    let mut x = ray::load_fpu(c);
    fp::ld_f32(&mut x, g.u32(local)?);
    fp::arith(
        &mut x,
        fp::arith_op::SUB,
        0,
        fp::src_kind::F32,
        g.u32(HALF)?.into(),
    );
    Ok(!magnitude_is_small(c, g, &mut x, 0x11244d)?)
}

// The ordered comparison sets ZF/PF/CF. Only strict SMALL > |ST(0)|
// takes JA; an unordered angle follows the same path as a large one.
fn magnitude_is_small(
    c: &mut CpuAbi,
    g: &impl Memory,
    x: &mut fp::X87Abi,
    last_pop: u32,
) -> std::result::Result<bool, GuestFault> {
    fp::unary(x, fp::unary_op::ABS);
    fp::ld_f32(x, g.u32(SMALL)?);
    let f = fp::cmp(x, 0, fp::src_kind::ST, 1, 1, true);
    fp::st_st(x, 0, true);
    ray::finish_fpu(c, g, x, last_pop, SMALL)?;
    flags(c, f, STATUS);
    Ok(f == 0)
}
