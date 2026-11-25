//! Gain from source distance and emission angle. Minimum and maximum
//! distances can come from the instance, sound class, or playback tag.
use super::*;
use crate::points::{Point, Loc, Sig};
use crate::sim_physics::ARITH;
use sim_sound::spatial::Ramp;
use x87::abi::{arith_op, src_kind, unary_op};

const DOC: &str = "docs/audio/dsound.md";
const UNITY: u32 = 0x45dbc0;
const EPSILON: u32 = 0x45dbdc;
const PLAYBACK_GLOBALS: u32 = 0x51ebd4;
const MIN_DISTANCE: u32 = 0x45dc70;
const MIN_DIRECTION_SQUARED: u32 = 0x45df1c;
const ZERO: u32 = 0x45dbd8;

const RAMP: Point = Point::new("SOUND_GAIN_RAMP", 0x12aff0, Sig::Fn {
    args: &[("mode", Loc::Ecx), ("start", Loc::Stack(0)), ("end", Loc::Stack(1)), ("input", Loc::Stack(2))],
    ret: Some(Loc::Xmm0), pop: 12,
}, DOC);

const DISTANCE: Point = Point::new("SOUND_DISTANCE_GAIN", 0x12ac20, Sig::Fn {
    args: &[("tag", Loc::Eax), ("source", Loc::Edi), ("distance", Loc::Stack(0))],
    ret: Some(Loc::Xmm0), pop: 4,
}, DOC);

const CONE: Point = Point::new("SOUND_DISTANCE_AND_CONE", 0x12ad30, Sig::Fn {
    args: &[("position", Loc::Eax), ("source", Loc::Ecx), ("tag", Loc::Stack(0)), ("direction", Loc::Stack(1))],
    ret: Some(Loc::Xmm0), pop: 8,
}, DOC);

pub(super) const RAMP_BODY: Body = Body { point: &RAMP, sites: &[], run: ramp };
pub(super) const DISTANCE_BODY: Body = Body { point: &DISTANCE, sites: &[0x12ac64, 0x12ac90, 0x12accd, 0x12ad00], run: distance };
pub(super) const CONE_BODY: Body = Body { point: &CONE, sites: &[0x12ad81, 0x12ae64, 0x12ae76, 0x12ae85, 0x12ae94, 0x12aea3, 0x12aeb0, 0x12aec4], run: cone };

// Both tests compare |ST(0)| to the engine epsilon, consume both operands
// and leave the x87 last-data pointer at epsilon.
fn near_zero(c: &mut CpuAbi, g: &Guest, mut stack: fp::X87Abi, pc: u32) -> Result<bool> {
    fp::unary(&mut stack, unary_op::ABS);
    fp::ld_f32(&mut stack, g.u32(EPSILON)?);
    let flags = fp::cmp(&mut stack, 0, src_kind::ST, 1, 1, true) & 0x45;
    fp::st_st(&mut stack, 0, true);
    finish_fpu(c, g, &stack, pc, EPSILON)?;
    c.eflags = (c.eflags & !ARITH) | flags;
    c.eflags_defined |= ARITH;
    Ok(flags & 0x41 == 0)
}

fn ramp(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let range = Ramp { start: g.u32(c.esp.wrapping_add(4))?, end: g.u32(c.esp.wrapping_add(8))? };
    let mut stack = load_fpu(c);
    fp::ld_f32(&mut stack, range.start);
    fp::arith(&mut stack, arith_op::SUB, 0, src_kind::F32, u64::from(range.end));
    let step = near_zero(c, g, stack, 0x12b002)?;
    let sample = g.u32(c.esp.wrapping_add(12))?;
    if step {
        let flags = sse::comiss(c, range.start, sample);
        c.eax = u32::from(flags & 0x41 == 0);
        c.ecx &= 0xff;
        arithmetic(c, c.eax, c.ecx, true);
        let gain = if c.eax == c.ecx { 0 } else { g.u32(UNITY)? };
        sse::load(c, 0, gain);
    } else {
        let result = range.fraction(sample, g.u32(UNITY)?, &mut c.mxcsr);
        sse::load(c, 0, result.value);
        sse::load(c, 1, result.bound);
        sse::load(c, 2, result.span);
        c.eflags = (c.eflags & !ARITH) | result.flags;
        c.eflags_defined |= ARITH;
    }
    Ok(Next::Ret { pop: 12 })
}

fn distance(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    loop {
        let sp = c.esp;
        pc = match pc {
            0x12ac20 => {
                c.esp = arithmetic(c, sp, 8, true);
                push(c, g, c.ebx)?;
                c.ebx = g.u32(TAGS)?;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(PLAYBACK_GLOBALS)?;
                push(c, g, c.esi)?;
                c.esi = c.eax;
                c.eax = byte(c.eax, g.u8(c.edi)? >> 2);
                if c.eax & 1 != 0 {
                    sse::load(c, 0, g.u32(c.edi.wrapping_add(0x34))?);
                } else {
                    c.ecx = (c.esi & 0xffff) << 4;
                    c.eax = g.u32(c.ecx.wrapping_add(c.ebx).wrapping_add(8))?;
                    let flag = g.u8(c.eax.wrapping_add(1))? & 4;
                    logic(c, flag.into(), 0x80);
                    if flag != 0 {
                        let class = g.u8(c.eax.wrapping_add(2))? as i8 as i16 as u16;
                        c.edx = (c.edx & 0xffff_0000) | u32::from(class);
                        push(c, g, c.edx)?;
                        return call(0x221810, 0x12ac64);
                    }
                    c.eax = g.u16(c.eax.wrapping_add(6))? as i16 as i32 as u32;
                    c.ecx = g.u32(c.ebp.wrapping_add(4))?;
                    c.eax = c.eax.wrapping_mul(0x38);
                    sse::load(c, 0, g.u32(c.eax.wrapping_add(c.ecx))?);
                }
                0x12ac7a
            }
            0x12ac64 => {
                sse::load(c, 0, g.u32(c.eax.wrapping_add(0x18))?);
                0x12ac7a
            }
            0x12ac7a => {
                let floor = g.u32(MIN_DISTANCE)?;
                sse::load(c, 1, floor);
                if sse::comiss(c, sse::lane0(c, 0), floor) & 0x41 == 0 {
                    c.eax = c.esi;
                    c.ecx = c.edi;
                    return call(0x124fc0, 0x12ac90);
                }
                g.set_u32(sp.wrapping_add(0x10), floor)?;
                0x12ac9e
            }
            0x12ac90 => {
                g.set_u32(sp.wrapping_add(0x10), sse::lane0(c, 0))?;
                0x12ac9e
            }
            0x12ac9e => {
                c.edx = byte(c.edx, g.u8(c.edi)? >> 3);
                if c.edx & 1 != 0 {
                    sse::load(c, 0, g.u32(c.edi.wrapping_add(0x38))?);
                } else {
                    c.esi = (c.esi & 0xffff) << 4;
                    c.esi = g.u32(c.esi.wrapping_add(c.ebx).wrapping_add(8))?;
                    let flag = g.u8(c.esi.wrapping_add(1))? & 8;
                    logic(c, flag.into(), 0x80);
                    if flag != 0 {
                        let class = g.u8(c.esi.wrapping_add(2))? as i8 as i16 as u16;
                        c.eax = (c.eax & 0xffff_0000) | u32::from(class);
                        push(c, g, c.eax)?;
                        return call(0x221810, 0x12accd);
                    }
                    c.ecx = g.u16(c.esi.wrapping_add(6))? as i16 as i32 as u32;
                    c.edx = g.u32(c.ebp.wrapping_add(4))?;
                    c.ecx = c.ecx.wrapping_mul(0x38);
                    sse::load(c, 0, g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(4))?);
                }
                0x12ace4
            }
            0x12accd => {
                sse::load(c, 0, g.u32(c.eax.wrapping_add(0x1c))?);
                0x12ace4
            }
            0x12ace4 => {
                c.eax = g.u32(sp.wrapping_add(0x18))?;
                c.ecx = g.u32(sp.wrapping_add(0x10))?;
                push(c, g, c.eax)?;
                g.set_u32(c.esp.wrapping_add(0x10), sse::lane0(c, 0))?;
                c.edx = g.u32(c.esp.wrapping_add(0x10))?;
                push(c, g, c.ecx)?;
                push(c, g, c.edx)?;
                c.ecx = byte(c.ecx, 0);
                logic(c, 0, 0x80);
                return call(RAMP.base, 0x12ad00);
            }
            0x12ad00 => {
                let distance = g.u32(sp.wrapping_add(0x18))?;
                sse::load(c, 1, distance);
                c.esi = pop(c, g)?;
                sse::movaps(c, 2, 0);
                let near = g.u32(c.esp.wrapping_add(0xc))?;
                sse::load(c, 0, near);
                let flags = sse::comiss(c, near, distance);
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                if flags & 0x41 == 0 { sse::movaps(c, 1, 0); }
                sse::ss_reg(c, Ss::Div, 0, 1);
                sse::ss_reg(c, Ss::Mul, 0, 2);
                c.esp = arithmetic(c, c.esp, 8, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => unreachable!("distance continuation {pc:#x}"),
        };
    }
}

// Square and accumulate XYZ in the order used before the x87 square
// root. The later angle test accumulates YZX and uses a separate cutoff.
fn cone(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    loop {
        let sp = c.esp;
        pc = match pc {
            0x12ad30 => {
                c.esp = arithmetic(c, sp, 0x10, true);
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(c.esp.wrapping_add(0x1c))?;
                push(c, g, c.esi)?;
                c.esi = c.eax;
                sse::load(c, 2, g.u32(c.esi)?);
                sse::load(c, 1, g.u32(c.esi.wrapping_add(4))?);
                sse::load(c, 0, g.u32(c.esi.wrapping_add(8))?);
                sse::movaps(c, 3, 2);
                sse::ss_reg(c, Ss::Mul, 3, 2);
                sse::movaps(c, 2, 1);
                sse::ss_reg(c, Ss::Mul, 2, 1);
                sse::movaps(c, 1, 0);
                sse::ss_reg(c, Ss::Add, 3, 2);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                let squared = sse::ss_reg(c, Ss::Add, 3, 1);
                let scratch = c.esp.wrapping_add(0x20);
                g.set_u32(scratch, squared)?;
                let mut stack = load_fpu(c);
                fp::ld_f32(&mut stack, g.u32(scratch)?);
                push(c, g, c.edi)?;
                fp::unary(&mut stack, unary_op::SQRT);
                push(c, g, c.ecx)?;
                c.edi = c.ecx;
                c.eax = c.ebp;
                let (_, word) = fp::st_f32(&stack, 0);
                g.set_u32(c.esp, word.value)?;
                fp::store_commit(&mut stack, word.status, true);
                finish_fpu(c, g, &stack, 0x12ad79, c.esp)?;
                return call(DISTANCE.base, 0x12ad81);
            }
            0x12ad81 => {
                let flags = g.u16(c.edi)?;
                c.ebx = (c.ebx & 0xffff_0000) | u32::from(flags);
                sse::load(c, 1, g.u32(UNITY)?);
                c.eax = byte(c.eax, (flags as u8) >> 4);
                logic(c, c.eax & 1, 0x80);
                g.set_u32(sp.wrapping_add(0x10), sse::lane0(c, 0))?;
                sse::movaps(c, 0, 1);
                g.set_u32(sp.wrapping_add(0x24), sse::lane0(c, 0))?;
                let mut skip = false;
                if c.eax & 1 == 0 {
                    c.ecx = byte(c.ecx, (flags as u8) >> 5);
                    logic(c, c.ecx & 1, 0x80);
                    if c.ecx & 1 == 0 {
                        c.eax = g.u32(TAGS)?;
                        c.edx = (c.ebp & 0xffff) << 4;
                        c.ecx = g.u32(c.edx.wrapping_add(c.eax).wrapping_add(8))?;
                        let tag_flag = g.u8(c.ecx.wrapping_add(1))? & 0x10;
                        logic(c, tag_flag.into(), 0x80);
                        skip = tag_flag != 0;
                    }
                }
                if skip { 0x12aee6 } else { 0x12adcc }
            }
            0x12adcc => {
                let mut stack = load_fpu(c);
                fp::ld_f32(&mut stack, g.u32(sp.wrapping_add(0x10))?);
                if near_zero(c, g, stack, 0x12adda)? { 0x12aee6 } else {
                    let y = g.u32(c.esi.wrapping_add(4))?;
                    let z = g.u32(c.esi.wrapping_add(8))?;
                    let x = g.u32(c.esi)?;
                    sse::load(c, 3, y);
                    sse::load(c, 2, z);
                    sse::load(c, 4, x);
                    sse::movaps(c, 5, 3);
                    sse::ss_reg(c, Ss::Mul, 5, 3);
                    sse::movaps(c, 3, 2);
                    sse::ss_reg(c, Ss::Mul, 3, 2);
                    sse::movaps(c, 2, 4);
                    sse::ss_reg(c, Ss::Add, 5, 3);
                    sse::ss_reg(c, Ss::Mul, 2, 4);
                    let squared = sse::ss_reg(c, Ss::Add, 5, 2);
                    if sse::comiss(c, squared, g.u32(MIN_DIRECTION_SQUARED)?) & 1 != 0 {
                        0x12aee6
                    } else {
                        c.edx = g.u32(c.esi)?;
                        sse::zero(c, 0);
                        c.eax = g.u32(c.esi.wrapping_add(4))?;
                        c.ecx = g.u32(c.esi.wrapping_add(8))?;
                        g.set_u32(sp.wrapping_add(0x14), c.edx)?;
                        sse::movaps(c, 1, 0);
                        sse::ss(c, Ss::Sub, 1, g.u32(sp.wrapping_add(0x14))?);
                        g.set_u32(sp.wrapping_add(0x18), c.eax)?;
                        g.set_u32(sp.wrapping_add(0x14), sse::lane0(c, 1))?;
                        g.set_u32(sp.wrapping_add(0x1c), c.ecx)?;
                        sse::movaps(c, 1, 0);
                        sse::ss(c, Ss::Sub, 1, g.u32(sp.wrapping_add(0x18))?);
                        sse::ss(c, Ss::Sub, 0, g.u32(sp.wrapping_add(0x1c))?);
                        c.eax = sp.wrapping_add(0x14);
                        g.set_u32(sp.wrapping_add(0x18), sse::lane0(c, 1))?;
                        g.set_u32(sp.wrapping_add(0x1c), sse::lane0(c, 0))?;
                        return call(0x30bf0, 0x12ae64);
                    }
                }
            }
            0x12ae64 => {
                if sse::comiss(c, sse::lane0(c, 0), g.u32(ZERO)?) & 0x41 == 0 {
                    c.eax = c.ebp;
                    c.ecx = c.edi;
                    return call(0x1251a0, 0x12ae76);
                }
                0x12aed8
            }
            0x12ae76 => {
                c.ecx = g.u32(sp.wrapping_add(0x28))?;
                c.esi = c.eax;
                c.eax = sp.wrapping_add(0x14);
                return call(0x11cf50, 0x12ae85);
            }
            0x12ae85 | 0x12ae94 => {
                push(c, g, c.ecx)?;
                c.eax = c.ebp;
                c.ecx = c.edi;
                g.set_u32(c.esp, sse::lane0(c, 0))?;
                return if pc == 0x12ae85 { call(0x125120, 0x12ae94) } else { call(0x1250a0, 0x12aea3) };
            }
            0x12aea3 => {
                push(c, g, c.ecx)?;
                c.ecx = byte(c.ecx, 1);
                g.set_u32(c.esp, sse::lane0(c, 0))?;
                return call(RAMP.base, 0x12aeb0);
            }
            0x12aeb0 => {
                let at = sp.wrapping_add(0x24);
                g.set_u32(at, sse::lane0(c, 0))?;
                let mut stack = load_fpu(c);
                fp::ld_f32(&mut stack, g.u32(at)?);
                push(c, g, c.esi)?;
                let (_, word) = fp::st_f32(&stack, 0);
                g.set_u32(at, word.value)?;
                fp::store_commit(&mut stack, word.status, true);
                finish_fpu(c, g, &stack, 0x12aebb, at)?;
                return call(0x2195f0, 0x12aec4);
            }
            0x12aec4 => {
                // Interpolate unity towards the cone's outside gain with
                // extended-precision intermediates, then round to binary32.
                let at = sp.wrapping_add(0x24);
                let mut stack = load_fpu(c);
                for (op, address) in [(arith_op::SUB, UNITY), (arith_op::MUL, at), (arith_op::ADD, UNITY)] {
                    fp::arith(&mut stack, op, 0, src_kind::F32, u64::from(g.u32(address)?));
                }
                let (_, result) = fp::st_f32(&stack, 0);
                g.set_u32(at, result.value)?;
                fp::store_commit(&mut stack, result.status, true);
                finish_fpu(c, g, &stack, 0x12aed4, at)?;
                0x12aed8
            }
            0x12aed8 => {
                sse::load(c, 1, g.u32(UNITY)?);
                sse::load(c, 0, g.u32(sp.wrapping_add(0x24))?);
                0x12aee6
            }
            0x12aee6 => {
                sse::ss(c, Ss::Mul, 0, g.u32(sp.wrapping_add(0x10))?);
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                let invert = c.ebx & (1 << 9) != 0;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                if invert {
                    sse::ss_reg(c, Ss::Sub, 1, 0);
                    sse::movaps(c, 0, 1);
                }
                c.esp = arithmetic(c, c.esp, 0x10, false);
                return Ok(Next::Ret { pop: 8 });
            }
            _ => unreachable!("cone continuation {pc:#x}"),
        };
    }
}
