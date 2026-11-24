//! Local-user stick response and magnetism, including the local-user view
//! a phantom's action builder supplies. The frame and input globals always
//! come from the calling executor.
use crate::light_constants::X87;
use crate::sim_native::{Memory, Result, SegmentMemory, plan::segment};
use crate::sim_objects::sse::{self, Ss, lane0, load, movaps, ss, ss_reg};
use crate::sim_physics::{arithmetic, logic, pop, push, spatial::{finish_fpu, fp_error, load_fpu}};
use runtime::{CpuAbi, GuestFault, TransferAbi};

pub const CURVE: u32 = 0x0018_5b30;
const ONE: u32 = 0x0045_dbc0;
const ZERO: u32 = 0x0045_dbd8;
const TINY: u32 = 0x0045_dbdc;

type Checked = std::result::Result<Option<TransferAbi>, GuestFault>;

/// Sample a signed axis against its profile's evenly spaced response curve.
pub fn curve(cpu: &mut CpuAbi, ram: &mut impl SegmentMemory, pc: u32) -> Checked {
    segment::<4, _>(cpu, ram, pc, CURVE..0x0018_5bd4, &[], sample_curve)
}

fn sample_curve(c: &mut CpuAbi, m: &mut impl Memory, _: u32) -> Result {
    let argument = c.esp.wrapping_add(4);
    push(c, m, c.ecx)?;
    sse::zero(c, 2);
    let negative = sse::comiss(c, 0, m.u32(argument)?) & 0x41 == 0;
    push(c, m, c.ebx)?;

    let count = c.eax as i16 as i32;
    let last = count - 1;
    // The argument slot doubles as the rounded sample position, and the
    // saved ECX slot holds the last index when the function returns.
    let mut x = load_fpu(c);
    x.fld(m.u32(argument)?).map_err(fp_error)?;
    x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
    m.set_u32(c.esp.wrapping_add(4), last as u32)?;
    X87::ok(x87::abi::arith(&mut x.0, x87::abi::arith_op::MUL, 0,
        x87::abi::src_kind::I32, last as u32 as u64)).map_err(fp_error)?;
    m.set_u32(argument, x.fst_m32().map_err(fp_error)?)?;
    x.fld(m.u32(ZERO)?).map_err(fp_error)?;
    let below_zero = x.fcomip(1).map_err(fp_error)? & 0x41 == 0;
    x.fstp_st(0).map_err(fp_error)?;
    finish_fpu(c, m, x, 0x0018_5b62, ZERO)?;
    if below_zero {
        movaps(c, 1, 2);
    } else {
        load(c, 1, m.u32(argument)?);
        sse::cvtsi2ss(c, 0, count as u32);
        let upper = ss(c, Ss::Sub, 0, m.u32(ONE)?);
        let position = lane0(c, 1);
        if sse::comiss(c, position, upper) & 0x41 == 0 {
            movaps(c, 1, 0);
        }
    }
    let position = lane0(c, 1);
    let trunc = sse::cvttss2si(c, position) as i16 as i32;
    let index = if trunc < 0 { 0 } else { trunc.min(last) } as i16 as i32;
    let next = (index + 1).min(last) as i16 as i32;
    let sample = |i: i32| c.esi.wrapping_add((i as u32).wrapping_mul(4));
    let (lo, hi) = (m.u32(sample(index))?, m.u32(sample(next))?);
    load(c, 0, hi);
    ss(c, Ss::Sub, 0, lo);
    let integer = sse::cvtsi2ss(c, 3, index as u32);
    ss(c, Ss::Sub, 1, integer);
    ss_reg(c, Ss::Mul, 0, 1);
    ss(c, Ss::Add, 0, lo);
    if negative {
        ss_reg(c, Ss::Sub, 2, 0);
        movaps(c, 0, 2);
    }
    c.eax = index as u32;
    c.edx = index.wrapping_add(1) as u32;
    logic(c, u32::from(negative), 0x80);
    c.ebx = pop(c, m)?;
    c.ecx = pop(c, m)?;
    m.ret(c, 4)
}
