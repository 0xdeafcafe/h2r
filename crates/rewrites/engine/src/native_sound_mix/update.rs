//! The stream-parameter update at 21fa80. Its stack holds both the cached
//! voice properties and the pending descriptors across the driver's callbacks.

use super::{Step, levels, route};
use crate::light_constants::X87;
use crate::sim_objects::{Cx, Next, Result, alu, frame, sse};
use crate::sim_physics::{ARITH, arithmetic, byte, logic};
use runtime::{CpuAbi, Guest};

const DRIVER: u32 = 0x51ebe4;
const GAIN_FLOOR: u32 = 0x44a0b4;
const GAIN_DEFAULT: u32 = 0x44a23c;
const GAIN_THRESHOLD: u32 = 0x45dc68;
const MILLIBEL_SCALE: u32 = 0x45e008;

pub(super) const SITES: &[u32] = &[
    0x21fb3d, 0x21fb83, 0x21fb9e, 0x21fbba, 0x21fbca, 0x21fbf0, 0x21fc0e, 0x21ff56, 0x21ff67,
    0x21ff87, 0x21ffb0, 0x21fff0, 0x220005, 0x220013, 0x22002d, 0x220040, 0x220057, 0x220099,
    0x2200c8, 0x2200f9, 0x220117, 0x22013b, 0x2201c9, 0x2201e1,
];

pub(super) fn update(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    loop {
        let step = match pc {
            0x21fa80 => begin(cx)?,
            0x21fb1e..=0x21fc0e => gain(cx, pc)?,
            0x21fc21 => {
                if routing_current(cx)? {
                    Step::To(0x22013b)
                } else {
                    Step::To(0x21fcca)
                }
            }
            0x21fcca => {
                levels::prepare(cx)?;
                Step::To(0x21fef5)
            }
            0x21fef5..=0x220117 => route::dispatch(cx, pc)?,
            0x22013b | 0x2201c9 | 0x2201cc => pitch(cx, pc)?,
            0x2201e1 => {
                let (c, g) = (&mut *cx.cpu, &*cx.g);
                c.esi = frame::pop(c, g)?;
                c.ebp = frame::pop(c, g)?;
                c.ebx = frame::pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x224, false);
                return Ok(Next::Ret { pop: 12 });
            }
            _ => unreachable!("unregistered stream update site"),
        };
        match step {
            Step::To(next) => pc = next,
            Step::Call(next) => return Ok(next),
        }
    }
}

fn begin(cx: &mut Cx<'_>) -> Result<Step> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.eax = frame::slot(c, g, 4)?.wrapping_mul(0x34);
    c.ecx = g.u32(DRIVER)?;
    c.esp = c.esp.wrapping_sub(0x224);
    frame::push(c, g, c.ebx)?;
    c.ebx = c.ecx.wrapping_add(c.eax).wrapping_add(0xc);
    let initialized = g.u8(c.ebx.wrapping_add(3))? & 1 != 0;
    frame::push(c, g, c.ebp)?;
    frame::push(c, g, c.esi)?;
    let refresh = !initialized || g.u8(c.ecx)? == 0;
    g.set_u8(c.esp.wrapping_add(0xf), u8::from(refresh))?;
    let level = g.u32(c.edi.wrapping_add(0xc))?;
    sse::load(c, 0, level);
    c.eax = if sse::comiss(c, level, g.u32(GAIN_FLOOR)?) & 0x41 == 0 {
        c.edi.wrapping_add(0xc)
    } else {
        GAIN_DEFAULT
    };
    c.ebp = g.u32(c.eax)?;
    let layout = g.u8(c.ebx.wrapping_add(1))?;
    c.esi = i32::from((layout.wrapping_shl(2) as i8) >> 5) as u32;
    c.eax = byte(c.eax, u8::from(refresh));
    let plain = g.u8(c.edi.wrapping_add(0x30))? & 1;
    c.edx = byte(c.edx, plain);
    logic(c, u32::from(refresh), 0x80);
    frame::set_slot(c, g, 0x18, c.ebp)?;
    g.set_u8(c.esp.wrapping_add(0xe), plain)?;
    frame::set_slot(c, g, 0x10, c.esi)?;
    if !refresh {
        sse::load(c, 1, c.ebp);
        sse::load(c, 0, g.u32(c.ebx.wrapping_add(0xc))?);
        let delta = sse::ss_reg(c, sse::Ss::Sub, 1, 0);
        frame::set_slot(c, g, 0x18, delta)?;
        if below_threshold(c, g, delta, None, 0x21fb10)? {
            return Ok(Step::To(0x21fc21));
        }
        c.ecx = g.u32(DRIVER)?;
    }
    Ok(Step::To(0x21fb1e))
}

pub(super) fn load_x87(c: &CpuAbi) -> X87 {
    let f = &c.x87;
    X87(x87::abi::X87Abi {
        st: f.st,
        control: f.control,
        status: f.status,
        tag: f.tag,
        opcode: f.opcode,
        instruction_pointer: f.instruction_pointer,
        data_pointer: f.data_pointer,
        instruction_selector: f.instruction_selector,
        data_selector: f.data_selector,
    })
}

pub(super) fn store_x87(c: &mut CpuAbi, g: &Guest, x: X87, pc: u32, data: u32) -> Result<()> {
    c.x87.st = x.0.st;
    c.x87.status = x.0.status;
    c.x87.tag = x.0.tag;
    c.x87.instruction_pointer = pc;
    c.x87.opcode = g.u16(pc)?.swap_bytes() & 0x7ff;
    c.x87.instruction_selector = 8;
    c.x87.data_pointer = data;
    c.x87.data_selector = 16;
    Ok(())
}

/// Ordered epsilon comparison. A cached delta is subtracted in extended
/// precision before the absolute value; a single value has already rounded
/// to binary32 on the caller's stack.
fn below_threshold(
    c: &mut CpuAbi,
    g: &Guest,
    value: u32,
    previous: Option<u32>,
    last_pc: u32,
) -> Result<bool> {
    let mut fp = load_x87(c);
    if let Some(old) = previous {
        fp.fld(old).expect("masked x87");
    }
    fp.fld(value).expect("masked x87");
    if previous.is_some() {
        fp.arith_st(x87::abi::arith_op::SUB, 1).expect("masked x87");
    }
    fp.unary(x87::abi::unary_op::ABS).expect("masked x87");
    if previous.is_some() {
        fp.fstp_st(1).expect("masked x87");
    }
    fp.fld(g.u32(GAIN_THRESHOLD)?).expect("masked x87");
    let flags = fp.fcomip(1).expect("masked x87");
    fp.fstp_st(0).expect("masked x87");
    store_x87(c, g, fp, last_pc, GAIN_THRESHOLD)?;
    c.eflags = (c.eflags & !ARITH) | flags;
    c.eflags_defined |= ARITH;
    Ok(flags & 0x41 == 0)
}

fn routing_current(cx: &mut Cx<'_>) -> Result<bool> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let prev = (g.u8(c.ebx.wrapping_add(3))? >> 5) & 1;
    c.eax = byte(c.eax, prev);
    if c.edx as u8 != prev {
        return Ok(false);
    }
    if c.edx as u8 == 0 {
        for (source, cached, pc) in [(0x18, 0x10, 0x21fc49), (0x14, 0x18, 0x21fc61)] {
            if !below_threshold(
                c,
                g,
                g.u32(c.edi.wrapping_add(source))?,
                Some(g.u32(c.ebx.wrapping_add(cached))?),
                pc,
            )? {
                return Ok(false);
            }
        }
        for (source, pc) in [(0x1c, 0x21fc7e), (0x20, 0x21fc9b)] {
            let v = g.u32(c.edi.wrapping_add(source))?;
            sse::load(c, 0, v);
            frame::set_slot(c, g, 0x18, v)?;
            if !below_threshold(c, g, v, None, pc)? {
                return Ok(false);
            }
        }
    }
    if g.u16(c.edi.wrapping_add(0x24))? == u16::MAX {
        return Ok(true);
    }
    if below_threshold(
        c,
        g,
        g.u32(c.edi.wrapping_add(0x28))?,
        Some(g.u32(c.ebx.wrapping_add(0x14))?),
        0x21fcbe,
    )? {
        return Ok(true);
    }
    c.esi = frame::slot(c, g, 0x10)?;
    Ok(false)
}

fn gain(cx: &mut Cx<'_>, pc: u32) -> Result<Step> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let call = |target, site| Ok(Step::Call(Next::Call { target, site }));
    match pc {
        0x21fb1e => {
            let slot = g.u16(c.edi.wrapping_add(0x24))?;
            c.edx = (c.edx & 0xffff0000) | u32::from(slot);
            let static_count = g.u16(c.ecx.wrapping_add(8))?;
            alu::cmp16(c, slot.into(), static_count.into());
            if (slot as i16) < static_count as i16 {
                frame::push(c, g, c.ebp)?;
                frame::push(c, g, 0)?;
                let floor = g.u32(0x21fbe5)?;
                frame::push(c, g, floor)?;
                c.ecx = byte(c.ecx, 1);
                return call(0x12aff0, 0x21fbf0);
            }
            c.eax = g.u32(c.ebx.wrapping_add(0x24))?;
            c.ecx = c.esp.wrapping_add(0x11c);
            frame::push(c, g, c.ecx)?;
            frame::push(c, g, c.eax)?;
            call(0x37b83f, 0x21fb3d)
        }
        0x21fb3d => {
            c.eax = g.u32(DRIVER)?;
            let idx = i32::from(g.u16(c.edi.wrapping_add(0x24))? as i16) as u32;
            c.ecx = c
                .eax
                .wrapping_add(idx.wrapping_mul(0x3c))
                .wrapping_add(0x1a18);
            c.eax = arithmetic(c, c.esi, 0, true);
            c.edx = c.esp.wrapping_add(0x8c);
            frame::set_slot(c, g, 0x18, c.ecx)?;
            frame::set_slot(c, g, 0xcc, 0)?;
            frame::set_slot(c, g, 0xd0, c.edx)?;
            if c.eax == 0 {
                c.edx = frame::slot(c, g, 0x18)?;
                c.ecx = g.u32(c.edx)?;
                c.esi = c.esp.wrapping_add(0x8c);
                frame::push(c, g, c.ebp)?;
                return call(0x21e230, 0x21fb9e);
            }
            let cf = c.eflags & 1;
            c.eax = arithmetic(c, c.eax, 1, true);
            c.eflags = (c.eflags & !1) | cf;
            if c.eax == 0 {
                c.eax = c.ecx;
                c.ecx = g.u32(c.eax)?;
                frame::push(c, g, c.ebp)?;
                c.esi = c.edx;
                return call(0x21e230, 0x21fb83);
            }
            Ok(Step::To(0x21fba2))
        }
        0x21fb83 => {
            c.ecx = frame::slot(c, g, 0x18)?;
            c.ecx = g.u32(c.ecx)?;
            c.esi = c.esp.wrapping_add(0x8c);
            frame::push(c, g, c.ebp)?;
            call(0x21e230, 0x21fb9e)
        }
        0x21fb9e => {
            c.esi = frame::slot(c, g, 0x10)?;
            Ok(Step::To(0x21fba2))
        }
        0x21fba2 => {
            c.eax = g.u32(c.ebx.wrapping_add(0x24))?;
            frame::push(c, g, c.eax)?;
            c.ecx = c.esp.wrapping_add(0x90);
            frame::push(c, g, c.ecx)?;
            c.eax = c.esp.wrapping_add(0x124);
            call(0x2215d0, 0x21fbba)
        }
        0x21fbba => {
            c.eax = 0;
            logic(c, 0, 1 << 31);
            frame::push(c, g, 0)?;
            frame::push(c, g, 0)?;
            let floor = g.u32(0x21fbbf)?;
            frame::push(c, g, floor)?;
            c.ecx = byte(c.ecx, 1);
            call(0x12aff0, 0x21fbca)
        }
        0x21fbca | 0x21fbf0 => {
            sse::load(c, 1, g.u32(MILLIBEL_SCALE)?);
            sse::ss_reg(c, sse::Ss::Mul, 0, 1);
            let result = sse::ss_reg(c, sse::Ss::Sub, 0, 1);
            let volume = sse::cvttss2si(c, result);
            if pc == 0x21fbca {
                c.edx = volume;
            } else {
                c.ecx = volume;
            }
            frame::push(c, g, volume)?;
            c.eax = g.u32(c.ebx.wrapping_add(0x24))?;
            frame::push(c, g, c.eax)?;
            call(0x37b7ff, 0x21fc0e)
        }
        0x21fc0e => {
            let refresh = g.u8(c.esp.wrapping_add(0xf))?;
            c.eax = byte(c.eax, refresh);
            logic(c, u32::from(refresh), 0x80);
            c.edx = byte(c.edx, g.u8(c.esp.wrapping_add(0xe))?);
            g.set_u32(c.ebx.wrapping_add(0xc), c.ebp)?;
            Ok(Step::To(if refresh != 0 { 0x21fcca } else { 0x21fc21 }))
        }
        _ => unreachable!(),
    }
}

fn pitch(cx: &mut Cx<'_>, pc: u32) -> Result<Step> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == 0x2201c9 {
        g.set_u32(c.ebx.wrapping_add(8), c.esi)?;
    }
    if pc == 0x22013b {
        let skip = g.u8(c.esp.wrapping_add(0x23c))?;
        c.eax = byte(c.eax, skip);
        logic(c, u32::from(skip), 0x80);
        if skip == 0 {
            // The pitch offsets are instruction immediates in the owner's image.
            let base_at = match g.u32(c.edi.wrapping_add(4))? {
                0 => 0x220164,
                1 => 0x22015d,
                _ => 0x220156,
            };
            c.eax = g.u32(base_at)?;
            sse::load(c, 0, g.u32(c.edi.wrapping_add(8))?);
            sse::ss(c, sse::Ss::Mul, 0, g.u32(0x45dc4c)?);
            sse::cvtsi2ss(c, 1, c.eax);
            let requested = sse::ss_reg(c, sse::Ss::Add, 0, 1);
            let low = g.u32(0x45dc48)?;
            sse::load(c, 1, low);
            if sse::comiss(c, low, requested) & 0x41 == 0 {
                sse::movaps(c, 0, 1);
            } else {
                let high = g.u32(0x45dc44)?;
                sse::load(c, 1, high);
                if sse::comiss(c, requested, high) & 0x41 == 0 {
                    sse::movaps(c, 0, 1);
                }
            }
            let refresh = g.u8(c.esp.wrapping_add(0xf))?;
            c.eax = byte(c.eax, refresh);
            logic(c, refresh.into(), 0x80);
            c.esi = sse::cvttss2si(c, sse::lane0(c, 0));
            let submit = if refresh != 0 {
                true
            } else {
                c.ebp = g.u32(c.ebx.wrapping_add(8))?;
                c.eax = c.esi.wrapping_sub(c.ebp);
                c.edx = ((c.eax as i32) >> 31) as u32;
                c.eax = (c.eax ^ c.edx).wrapping_sub(c.edx);
                let delta = sse::cvtsi2ss(c, 0, c.eax);
                sse::comiss(c, delta, g.u32(0x43ff08)?) & 1 == 0
            };
            if submit {
                c.eax = g.u32(c.ebx.wrapping_add(0x24))?;
                frame::push(c, g, c.esi)?;
                frame::push(c, g, c.eax)?;
                return Ok(Step::Call(Next::Call {
                    target: 0x37b804,
                    site: 0x2201c9,
                }));
            }
        }
    }
    c.ebx = frame::slot(c, g, 0x238)?;
    c.eax = frame::slot(c, g, 0x234)?;
    frame::push(c, g, 0)?;
    Ok(Step::Call(Next::Call {
        target: 0x2201f0,
        site: 0x2201e1,
    }))
}
