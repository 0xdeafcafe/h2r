//! Floating-point MOPP frame operations. The clip fractions live in ST(0)
//! until the program spills them to binary32 for the packed SSE blend.
use super::*;
use x87::abi::{self as fp, arith_op, src_kind};

pub(crate) fn load_fpu(c: &CpuAbi) -> fp::X87Abi {
    fp::X87Abi {
        st: c.x87.st,
        control: c.x87.control,
        status: c.x87.status,
        tag: c.x87.tag,
        ..Default::default()
    }
}

pub(crate) fn finish_fpu(
    c: &mut CpuAbi,
    g: &impl Memory,
    x: &fp::X87Abi,
    pc: u32,
    data: u32,
) -> std::result::Result<(), GuestFault> {
    c.x87.st = x.st;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.instruction_pointer = pc;
    c.x87.opcode = g.u16(pc)?.swap_bytes() & 0x7ff;
    c.x87.data_pointer = data;
    c.x87.instruction_selector = 8;
    c.x87.data_selector = 16;
    Ok(())
}

const POP_ENDPOINT: u32 = 0x0030_0d70;
const REJECT_BOUND: u32 = 0x0030_161d;
const STORE_SCALE: u32 = 0x0030_138c;
const UNIT_VECTOR: u32 = 0x0041_2420;
const ZERO: u32 = 0x0045_dbd8;

fn spill(
    x: &mut fp::X87Abi,
    g: &mut impl Memory,
    addr: u32,
    pop: bool,
) -> std::result::Result<u32, GuestFault> {
    let (_, s) = fp::st_f32(x, 0);
    g.set_u32(addr, s.value)?;
    fp::store_commit(x, s.status, pop);
    Ok(s.value)
}

fn aligned(g: &impl Memory, addr: u32) -> std::result::Result<Vec4, GuestFault> {
    let mut bytes = [0; 16];
    g.read_bytes(addr, &mut bytes, 16)?;
    Ok(unpack(u128::from_le_bytes(bytes)))
}

fn store_vec(g: &mut impl Memory, addr: u32, v: Vec4) -> std::result::Result<(), GuestFault> {
    g.write_bytes(addr, &pack(v).to_le_bytes(), 16)
}

pub(super) fn state_key(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    op: u8,
    operand: u32,
) -> std::result::Result<(), GuestFault> {
    let local = at(c.esp, 64);
    let property = op >= 0x60;
    let width = 1u32 << if property { (op - 0x60) / 4 } else { op - 9 };
    if property {
        g.set_u32(at(local, 28), operand)?;
        c.eax = g.u32(at(local, 28))?;
        c.edx = local;
    } else {
        c.ecx = local;
        c.eax = operand;
        if width > 1 {
            c.edx = operand & 255;
        }
    }
    if c.ebx != local {
        let v = aligned(g, c.ebx)?;
        put_xmm(c, 0, v);
        store_vec(g, local, v)?;
        for offset in [16, 20, 24] {
            let value = g.u32(at(c.ebx, offset))?;
            g.set_u32(at(local, offset), value)?;
            if (offset == 20) == property {
                c.edx = value;
            } else {
                c.ecx = value;
            }
        }
        if !property {
            c.ecx = g.u32(at(c.ebx, 28))?;
            g.set_u32(at(local, 28), c.ecx)?;
        }
        c.ebx = local;
    }
    if property {
        g.set_u32(at(local, 28), c.eax)?;
    } else {
        let key = if width == 4 {
            operand
        } else {
            at(operand, g.u32(at(local, 24))?)
        };
        g.set_u32(at(local, 24), key)?;
    }
    c.esi = at(c.esi, 1 + width);
    Ok(())
}

pub(super) fn rescale(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    bits: u32,
) -> std::result::Result<(), GuestFault> {
    let sp = c.esp;
    let mut x = load_fpu(c);
    let start = aligned(g, c.edi)?;
    let a = u32::from(g.u8(at(c.esi, 1))?);
    let b = u32::from(g.u8(at(c.esi, 2))?);
    g.set_u32(at(sp, 20), a)?;
    let d = u32::from(g.u8(at(c.esi, 3))?);
    fp::ld_int(&mut x, a.into());
    g.set_u32(at(sp, 20), b)?;
    spill(&mut x, g, at(sp, 96), true)?;
    g.set_u32(at(sp, 108), 0)?;
    fp::ld_int(&mut x, b.into());
    g.set_u32(at(sp, 20), d)?;
    spill(&mut x, g, at(sp, 100), true)?;
    fp::ld_int(&mut x, d.into());
    spill(&mut x, g, at(sp, 104), true)?;
    let offset = aligned(g, at(sp, 96))?;
    let first = vector(c, Sub, start, offset);
    store_vec(g, c.edi, first)?;
    let second = aligned(g, at(c.edi, 16))?;
    let scale = 1u32 << bits;
    g.set_u32(at(sp, 20), scale)?;
    fp::ld_int(&mut x, scale.into());
    let second = vector(c, Sub, second, offset);
    store_vec(g, at(c.edi, 16), second)?;
    let first = aligned(g, c.edi)?;
    let scale = spill(&mut x, g, at(sp, 20), false)?;
    put_xmm(c, 0, scalar(scale));
    let first = vector(c, Mul, [scale; 4], first);
    store_vec(g, c.edi, first)?;
    let second = aligned(g, at(c.edi, 16))?;
    let second = vector(c, Mul, [scale; 4], second);
    put_xmm(c, 3, second);
    store_vec(g, at(c.edi, 16), second)?;
    let src = aligned(g, c.ebx)?;
    let offset = vector(c, Add, src, offset);
    put_xmm(c, 2, offset);
    let new_offset = vector(c, Mul, [scale; 4], offset);
    put_xmm(c, 1, new_offset);
    store_vec(g, at(sp, 64), new_offset)?;
    c.ecx = at(g.u32(at(c.ebx, 16))?, bits);
    g.set_u32(at(sp, 80), c.ecx)?;
    fp::arith(
        &mut x,
        arith_op::MUL,
        0,
        src_kind::F32,
        g.u32(at(c.ebx, 20))?.into(),
    );
    c.esi = at(c.esi, 4);
    spill(&mut x, g, at(sp, 84), true)?;
    finish_fpu(c, g, &x, STORE_SCALE, at(sp, 84))?;
    c.edx = g.u32(at(c.ebx, 28))?;
    g.set_u32(at(sp, 92), c.edx)?;
    c.eax = g.u32(at(c.ebx, 24))?;
    g.set_u32(at(sp, 88), c.eax)?;
    c.ebx = at(sp, 64);
    Ok(())
}

// FNSTSW AX followed by TEST AH. These compares intentionally use the
// x87 status bits even for an unordered or masked exceptional operand.
fn status(c: &mut CpuAbi, x: &fp::X87Abi, mask: u8) -> u8 {
    c.eax = (c.eax & 0xffff_0000) | u32::from(x.status);
    let result = (x.status >> 8) as u8 & mask;
    test_byte(c, result);
    result
}

pub(super) fn bounds(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    op: u8,
    lo: u32,
    hi: u32,
) -> std::result::Result<bool, GuestFault> {
    let mut x = load_fpu(c);
    let sp = c.esp;
    let wide = op >= 0x29;
    let axis = u32::from(if wide { op - 0x29 } else { op - 0x26 });
    let scratch = at(sp, 20);
    c.eax = axis;
    g.set_u32(scratch, lo)?;
    fp::ld_int(&mut x, lo.into());
    if wide {
        let query = g.u32(at(sp, 40))?;
        fp::arith(
            &mut x,
            arith_op::MUL,
            0,
            src_kind::F32,
            g.u32(at(query, 20))?.into(),
        );
        fp::arith(
            &mut x,
            arith_op::MUL,
            0,
            src_kind::F32,
            g.u32(at(c.ebx, 20))?.into(),
        );
        g.set_u32(scratch, hi)?;
        c.edx = g.u32(at(sp, 40))?;
        fp::arith(
            &mut x,
            arith_op::SUB,
            0,
            src_kind::F32,
            g.u32(at(c.ebx, 4 * axis))?.into(),
        );
        fp::ld_int(&mut x, hi.into());
        fp::arith(
            &mut x,
            arith_op::MUL,
            0,
            src_kind::F32,
            g.u32(at(c.edx, 20))?.into(),
        );
        fp::arith(
            &mut x,
            arith_op::MUL,
            0,
            src_kind::F32,
            g.u32(at(c.ebx, 20))?.into(),
        );
        fp::arith(
            &mut x,
            arith_op::SUB,
            0,
            src_kind::F32,
            g.u32(at(c.ebx, 4 * axis))?.into(),
        );
        c.ecx = hi & 255;
    } else {
        c.ecx = lo;
        c.edx = hi;
        g.set_u32(scratch, hi)?;
        fp::ld_int(&mut x, hi.into());
    }
    c.esi = at(c.esi, if wide { 7 } else { 3 });
    let a = at(sp, 36);
    let b = at(sp, 24);
    fp::ld_f32(&mut x, g.u32(at(c.edi, 4 * axis))?);
    spill(&mut x, g, a, true)?;
    fp::ld_f32(&mut x, g.u32(at(c.edi, 16 + 4 * axis))?);
    spill(&mut x, g, b, true)?;
    fp::ld_f32(&mut x, g.u32(a)?);
    fp::cmp(&mut x, 0, src_kind::F32, g.u32(b)?.into(), 1, false);
    let ascending = status(c, &x, 5).count_ones() % 2 == 1;
    let (max, min) = if ascending { (b, a) } else { (a, b) };
    fp::ld_f32(&mut x, g.u32(max)?);
    fp::cmp(&mut x, 0, src_kind::ST, 2, 1, false);
    let mut reject = status(c, &x, 5).count_ones() % 2 == 1;
    let mut data = max;
    if !reject {
        data = min;
        fp::ld_f32(&mut x, g.u32(min)?);
        fp::cmp(&mut x, 0, src_kind::ST, 1, 1, false);
        reject = status(c, &x, 0x41) == 0;
    }
    if reject {
        fp::st_st(&mut x, 0, true);
        fp::st_st(&mut x, 0, true);
        finish_fpu(c, g, &x, REJECT_BOUND, data)?;
        return Ok(false);
    }
    c.ecx = u32::from(!ascending);
    let start = aligned(g, c.edi)?;
    let end = aligned(g, at(c.edi, 16))?;
    put_xmm(c, 1, start);
    put_xmm(c, 2, end);
    for upper in [true, false] {
        // One boundary remains at the top of the stack on each pass.
        fp::ld_f32(&mut x, g.u32(a)?);
        fp::arith(&mut x, arith_op::SUB, 0, src_kind::ST, 1);
        spill(&mut x, g, scratch, true)?;
        fp::arith(&mut x, arith_op::SUBR, 0, src_kind::F32, g.u32(b)?.into());
        fp::ld_f32(&mut x, g.u32(scratch)?);
        fp::arith(&mut x, arith_op::MUL, 0, src_kind::ST, 1);
        fp::cmp(&mut x, 0, src_kind::F32, g.u32(ZERO)?.into(), 1, false);
        data = ZERO;
        if status(c, &x, 5).count_ones() % 2 == 1 {
            fp::ld_f32(&mut x, g.u32(scratch)?);
            fp::arith(&mut x, arith_op::SUB, 0, src_kind::ST, 1);
            fp::arith(
                &mut x,
                arith_op::DIVR,
                0,
                src_kind::F32,
                g.u32(scratch)?.into(),
            );
            let weight = spill(&mut x, g, scratch, true)?;
            data = scratch;
            put_xmm(c, 0, [weight; 4]);
            let complement = vector(c, Sub, aligned(g, UNIT_VECTOR)?, [weight; 4]);
            let weighted_end = vector(c, Mul, [weight; 4], end);
            put_xmm(c, 3, weighted_end);
            let left = vector(c, Mul, complement, start);
            let value = vector(c, Add, left, weighted_end);
            let dest = if upper {
                c.edx = c.edi.wrapping_sub(c.ecx << 4);
                put_xmm(c, 4, value);
                at(c.edx, 16)
            } else {
                c.ecx <<= 4;
                put_xmm(c, 2, value);
                at(c.edi, c.ecx)
            };
            store_vec(g, dest, value)?;
        }
        fp::st_st(&mut x, 0, true);
    }
    finish_fpu(c, g, &x, POP_ENDPOINT, data)?;
    Ok(true)
}

fn projection(
    x: &mut fp::X87Abi,
    g: &impl Memory,
    op: u8,
    base: u32,
) -> std::result::Result<(), GuestFault> {
    // Component order matters under x87 PC and for NaN propagation.
    let (first, rest): (u32, &[(u32, u32)]) = match op {
        0x13 => (8, &[(arith_op::ADD, 4)]),
        0x14 => (4, &[(arith_op::SUB, 8)]),
        0x15 => (8, &[(arith_op::ADD, 0)]),
        0x16 => (0, &[(arith_op::SUB, 8)]),
        0x17 => (4, &[(arith_op::ADD, 0)]),
        0x18 => (0, &[(arith_op::SUB, 4)]),
        0x19 => (8, &[(arith_op::ADD, 4), (arith_op::ADD, 0)]),
        0x1a => (4, &[(arith_op::ADD, 0), (arith_op::SUB, 8)]),
        0x1b => (0, &[(arith_op::SUB, 4), (arith_op::ADD, 8)]),
        0x1c => (0, &[(arith_op::SUB, 4), (arith_op::SUB, 8)]),
        _ => unreachable!(),
    };
    fp::ld_f32(x, g.u32(at(base, first))?);
    for &(operation, axis) in rest {
        fp::arith(
            x,
            operation,
            0,
            src_kind::F32,
            g.u32(at(base, axis))?.into(),
        );
    }
    Ok(())
}

fn compare_less(c: &mut CpuAbi, x: &mut fp::X87Abi, operand: u32, pop: bool) -> bool {
    fp::cmp(x, 0, src_kind::F32, operand.into(), u32::from(pop), false);
    status(c, x, 5).count_ones() % 2 == 1
}

fn compare_greater(c: &mut CpuAbi, x: &mut fp::X87Abi, operand: u32, pop: bool) -> bool {
    fp::cmp(x, 0, src_kind::F32, operand.into(), u32::from(pop), false);
    status(c, x, 0x41) == 0
}

/// Project the endpoints on a MOPP plane and select the child. When
/// both sides must be visited, clip a copy of the segment for the first
/// child. Its return site clips the second against the updated hit time.
pub(super) fn split(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    op: u8,
) -> std::result::Result<Option<TransferAbi>, GuestFault> {
    let mut x = load_fpu(c);
    let (sp, pc) = (c.esp, c.esi);
    let scratch = at(sp, 20);
    let low_at = at(sp, 32);
    let high_at = at(sp, 28);
    let end_at = at(sp, 24);
    let lo = i64::from(g.u8(at(pc, 1))?);
    let second = u32::from(g.u8(at(pc, 2))?);
    if (0x20..=0x22).contains(&op) {
        let axis = u32::from(op - 0x20);
        c.edx = second;
        c.ecx = 0;
        g.set_u32(scratch, lo as u32)?;
        fp::ld_int(&mut x, lo);
        g.set_u32(at(sp, 60), axis)?;
        g.set_u32(at(sp, 36), 0)?;
        spill(&mut x, g, high_at, false)?;
        g.set_u32(scratch, second)?;
        fp::arith(
            &mut x,
            arith_op::ADD,
            0,
            src_kind::F32,
            g.u32(crate::function_eval::constant::ONE)?.into(),
        );
        spill(&mut x, g, low_at, true)?;
        fp::ld_f32(&mut x, g.u32(at(c.edi, axis * 4))?);
        fp::ld_f32(&mut x, g.u32(at(c.edi, 16 + axis * 4))?);
        c.esi = at(pc, 3);
        c.eax = axis;
    } else {
        let diagonal = (0x13..=0x1c).contains(&op);
        let (scale, bias) = if diagonal {
            let (_, s, b) = mopp::boxvm::diagonal_plane(op);
            (s, b)
        } else {
            (1, 0)
        };
        let lower = lo * scale - bias;
        let upper = i64::from(second) * scale - bias;
        g.set_u32(scratch, lower as u32)?;
        let wide = op >= 0x23;
        let axis = if diagonal {
            0
        } else if wide {
            u32::from(op - 0x23)
        } else {
            u32::from(op - 0x10)
        };
        if !diagonal && !wide {
            g.set_u32(at(sp, 60), axis)?;
        }
        fp::ld_int(&mut x, lower);
        // One diagonal handler spills the threshold before storing its
        // second integer operand. The order is visible in guest memory.
        if op == 0x1b {
            spill(&mut x, g, low_at, true)?;
        }
        g.set_u32(scratch, upper as u32)?;
        if op != 0x1b {
            spill(&mut x, g, low_at, true)?;
        }
        fp::ld_int(&mut x, upper);
        if wide {
            g.set_u32(at(sp, 60), axis)?;
        }
        spill(&mut x, g, high_at, true)?;
        if diagonal {
            projection(&mut x, g, op, c.edi)?;
            projection(&mut x, g, op, at(c.edi, 16))?;
            if op != 0x1b {
                c.edx = if op == 0x1a { upper } else { lower } as u32;
            }
        } else {
            fp::ld_f32(&mut x, g.u32(at(c.edi, axis * 4))?);
            fp::ld_f32(&mut x, g.u32(at(c.edi, 16 + axis * 4))?);
            c.edx = if wide {
                g.u8(at(pc, 6))?.into()
            } else {
                second
            };
        }
        let (first, next) = if wide {
            (
                u32::from(u16::from_be_bytes(g.load(at(pc, 3))?)),
                u32::from(u16::from_be_bytes(g.load(at(pc, 5))?)),
            )
        } else {
            (0, g.u8(at(pc, 3))?.into())
        };
        c.esi = at(pc, if wide { 7 } else { 4 });
        g.set_u32(at(sp, 36), first)?;
        c.ecx = g.u32(at(sp, 36))?;
        g.set_u32(scratch, next)?;
        c.eax = next;
    }
    spill(&mut x, g, end_at, true)?;
    fp::ld_f32(&mut x, g.u32(end_at)?);
    if compare_less(c, &mut x, g.u32(high_at)?, true)
        && compare_less(c, &mut x, g.u32(high_at)?, false)
    {
        c.esi = at(c.esi, c.ecx);
        fp::st_st(&mut x, 0, true);
        finish_fpu(c, g, &x, POP_ENDPOINT, high_at)?;
        return Ok(None);
    }
    let beyond = compare_greater(c, &mut x, g.u32(low_at)?, false);
    c.edx = g.u32(scratch)?;
    c.esi = at(c.esi, c.edx);
    if beyond {
        fp::ld_f32(&mut x, g.u32(end_at)?);
        if compare_greater(c, &mut x, g.u32(low_at)?, true) {
            fp::st_st(&mut x, 0, true);
            finish_fpu(c, g, &x, POP_ENDPOINT, low_at)?;
            return Ok(None);
        }
    }

    // Start-low, start-high, end-low and end-high distances. A store
    // rounds to f32 at each boundary before the next x87 operation.
    fp::ld_f32(&mut x, g.u32(low_at)?);
    let start = aligned(g, c.edi)?;
    fp::arith(&mut x, arith_op::SUBR, 0, src_kind::ST, 1);
    let end = aligned(g, at(c.edi, 16))?;
    put_xmm(c, 1, start);
    put_xmm(c, 2, end);
    let segment = at(sp, 128);
    store_vec(g, segment, start)?;
    store_vec(g, at(segment, 16), end)?;
    let start_low = at(sp, 52);
    let start_high = at(sp, 44);
    let end_low = at(sp, 48);
    spill(&mut x, g, start_low, true)?;
    fp::arith(
        &mut x,
        arith_op::SUB,
        0,
        src_kind::F32,
        g.u32(high_at)?.into(),
    );
    spill(&mut x, g, start_high, true)?;
    for (bound, output) in [(low_at, end_low), (high_at, end_at)] {
        fp::ld_f32(&mut x, g.u32(end_at)?);
        fp::arith(
            &mut x,
            arith_op::SUB,
            0,
            src_kind::F32,
            g.u32(bound)?.into(),
        );
        spill(&mut x, g, output, true)?;
    }
    fp::ld_f32(&mut x, g.u32(start_low)?);
    let increasing = compare_less(c, &mut x, g.u32(end_low)?, true);
    let (left, right, weight_at) = if increasing {
        (start_low, end_low, scratch)
    } else {
        (start_high, end_at, start_high)
    };
    fp::ld_f32(&mut x, g.u32(right)?);
    fp::arith(&mut x, arith_op::MUL, 0, src_kind::F32, g.u32(left)?.into());
    let clip = compare_less(c, &mut x, g.u32(ZERO)?, true);
    let (mark, data) = if clip {
        fp::ld_f32(&mut x, g.u32(left)?);
        fp::arith(
            &mut x,
            arith_op::SUB,
            0,
            src_kind::F32,
            g.u32(right)?.into(),
        );
        fp::arith(
            &mut x,
            arith_op::DIVR,
            0,
            src_kind::F32,
            g.u32(left)?.into(),
        );
        let w = spill(&mut x, g, weight_at, true)?;
        put_xmm(c, 0, [w; 4]);
        let to = vector(c, Mul, [w; 4], end);
        put_xmm(c, 3, to);
        let complement = vector(c, Sub, aligned(g, UNIT_VECTOR)?, [w; 4]);
        let from = vector(c, Mul, complement, start);
        let clipped = vector(c, Add, from, to);
        put_xmm(c, 2, clipped);
        store_vec(g, at(segment, 16), clipped)?;
        (
            if increasing { 0x0030_0e5a } else { 0x0030_0fb3 },
            weight_at,
        )
    } else {
        (if increasing { 0x0030_0e41 } else { 0x0030_0f9a }, ZERO)
    };
    finish_fpu(c, g, &x, mark, data)?;
    let resume = if increasing {
        c.ecx = c.ecx.wrapping_sub(c.edx);
        c.eax = segment;
        push(c, g, c.eax)?;
        add_flags(c, c.ecx, c.esi);
        c.ecx = at(c.ecx, c.esi);
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.esp, 48))?;
        0x0030_0e9d
    } else {
        c.ecx = g.u32(at(sp, 40))?;
        c.edx = segment;
        push(c, g, c.edx)?;
        push(c, g, c.esi)?;
        0x0030_0ff2
    };
    push(c, g, c.ebx)?;
    call(c, g, guest_layout::points::SIM_MOPP_RAY.base, resume).map(Some)
}
