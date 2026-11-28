//! The light record update `c0c80` (`LIGHT_RECORD_UPDATE`): the
//! per-function port (`sim_physics::light`, kept as a test oracle in
//! `sim_physics::ORACLES`) taken into the body table, with each transfer
//! returned to `drive` instead of the runtime. Its callees `c0b00`,
//! `baf80` (slice 1), `bab40` (slice 3), `c2d00` and `c3220` are bodies,
//! so `drive` fuses them; `bad50`, `131d60`, `17ca10`, `131c20`, `b9ce0`
//! and `b9d20` are callouts.
//!
//! **`c0c80 (light: stack 0)`; `ret 4`.** `sub esp, 58`, then `ebx`,
//! `ebp`, `esi`, `edi`; `ebp` holds the element, `ebx` the definition
//! until the colours are copied. It evaluates the light's animation or
//! fade (a function of the owner object through `bab40`, `bad50` and
//! `131d60`; a timed fade through `17ca10` and `131c20`; or the element's
//! own intensity), runs the modulation `c0b00`, attenuates by the owner's
//! damage (`baf80` to the root, body `+2b0`), clamps the two RGB triples
//! and the radius, writes the intensity (`+c4`), and reconciles the active
//! flag (bit 1): a light that turns on is linked (`c2d00`, only while the
//! object globals' first byte `[[4de2f4]]` is set) when it has no owner, or when the
//! owner tests answer `b9ce0` no and `b9d20` yes; one that turns off is
//! unlinked (`c3220`) when it was linked. The register-level record is the port's, compared with
//! the lifted original by `tests/sim_physics_differential.rs`.
//!
//! This body subsumes `impl.sim.static-light`'s skip: an unchanged placed
//! light takes the same arithmetic, with no runtime dispatch between it
//! and the walk `c1400`.

use ::sim_objects::light::{self, LightBlend};
use crate::light_constants::{Error, Sse};
use guest_layout::points;
use crate::sim_objects::{Cx, Next, Result, unowned};
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest, GuestFault};

/// After each of `c0c80`'s calls.
pub(in crate::sim_objects) const RECORD_SITES: [u32; 13] = [
    0xc0dc9, 0xc0dda, 0xc0dfc, 0xc0e1c, 0xc0e8c, 0xc0ebf, 0xc0edb, 0xc0f75, 0xc0f95, 0xc1275, 0xc1280, 0xc129c,
    0xc12b9,
];
const LIGHTS: u32 = super::LIGHTS;
const ONE: u32 = 0x0045_dbc0;
const RADIUS_SCALE: u32 = 0x0045_dc0c;
const RADIUS_MAX: u32 = 0x0045_dc10;
const COLOR_ORIGIN: u32 = 0x0046_8710;
/// The object globals; `c0c80` tests their first byte (not the walks'
/// phase byte at `+3`).
const PHASE: u32 = 0x004d_e2f4;

const FUNCTION_SOURCE: u32 = 0x000b_ad50;
const FUNCTION_COLOR: u32 = 0x0013_1d60;
const FADE: u32 = 0x0017_ca10;
const INTERPOLATE: u32 = 0x0013_1c20;
/// The two owner tests before a link (`ecx`, then `eax`, the owner; `al`).
const OWNER_TEST: u32 = 0x000b_9ce0;
const OWNER_TEST_2: u32 = 0x000b_9d20;
const LINK: u32 = 0x000c_2d00;
const UNLINK: u32 = 0x000c_3220;

/// Bodies run with every SSE exception masked, so the model cannot fail;
/// a failure would be a bug, reported at the entry.
fn fp(_: Error) -> GuestFault {
    unowned(points::LIGHT_RECORD_UPDATE.base)
}

fn scalar(c: &mut CpuAbi, index: usize, value: u32) {
    c.xmm[index] = u128::from(value).to_le_bytes();
}

fn low(c: &CpuAbi, r: usize) -> u32 {
    u128::from_le_bytes(c.xmm[r]) as u32
}

fn set_low(c: &mut CpuAbi, r: usize, v: u32) {
    c.xmm[r][..4].copy_from_slice(&v.to_le_bytes());
}

fn diff(c: &mut CpuAbi, lhs: u32, rhs: u32) -> Result<u32> {
    let f = simd::comiss(&mut c.mxcsr, lhs, rhs).map_err(|_| fp(Error::Simd))?;
    c.eflags = c.eflags & !ARITH | f;
    c.eflags_defined |= ARITH;
    Ok(f)
}

/// Run the ordinary blend in Rust. The SSE model covers other operand
/// ranges and sets the guest's sticky flags when they are observable.
fn blend(s: &mut Sse, start: u32, end: u32, weight: u32, one: u32) -> Result<LightBlend> {
    if let Some(result) = light::blend(start, end, weight, one, s.mxcsr) {
        return Ok(result);
    }
    let complement = s.sub(one, weight).map_err(fp)?;
    let weighted_start = s.mul(complement, start).map_err(fp)?;
    let weighted_end = s.mul(end, weight).map_err(fp)?;
    let value = s.add(weighted_start, weighted_end).map_err(fp)?;
    Ok(LightBlend { weighted_end, value })
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

fn finish(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 88, false);
    Ok(Next::Ret { pop: 4 })
}

fn adjust(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = g.u32(c.ebp + 208)?;
    c.eax = g.u32(c.esp + 108)?;
    c.ecx = c.ebp + 240;
    for v in [c.ecx, c.edi, c.edx, c.eax] {
        push(c, g, v)?;
    }
    c.esi = c.ebp + 212;
    c.ecx = c.ebx + 192;
    c.eax = c.esi;
    call(points::LIGHT_RECORD_PHASE.base, 0xc0f75)
}

/// `c0c80`; `ret 4`.
pub(in crate::sim_objects) fn record(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::LIGHT_RECORD_UPDATE.base {
        return begin(c, g);
    }
    let sp = c.esp;
    match pc {
        0xc0dc9 => {
            c.edi = g.u16(c.ebp + 86)? as i16 as i32 as u32;
            c.eax = g.u32(sp + 16)?;
            c.esi = sp + 28;
            call(FUNCTION_SOURCE, 0xc0dda)
        }
        0xc0dda | 0xc0dfc => {
            let first = pc == 0xc0dda;
            scalar(c, 7, g.u32(if first { c.ebx } else { c.ebp + 208 })?);
            c.eax = sp + if first { 40 } else { 72 };
            push(c, g, c.eax)?;
            if first {
                c.ebx = c.esi;
                c.esi = g.u32(sp + 20)?;
            }
            c.eax = g.u32(c.esi + 52)?;
            c.edx = sp + if first { 56 } else { 88 };
            c.ecx = c.ebp + if first { 212 } else { 224 };
            push(c, g, c.edx)?;
            call(FUNCTION_COLOR, if first { 0xc0dfc } else { 0xc0e1c })
        }
        0xc0e1c | 0xc0edb => {
            let animated = pc == 0xc0e1c;
            let def = if animated { c.esi } else { c.ebx };
            let t = g.u32(c.ebp + 208)?;
            let mut s = Sse { mxcsr: c.mxcsr };
            let one = if animated { g.u32(ONE)? } else { low(c, 7) };
            let radius = blend(&mut s, g.u32(def + 112)?, g.u32(def + 116)?, t, one)?;
            scalar(c, 0, t);
            if animated {
                scalar(c, 1, radius.value);
                scalar(c, 2, radius.weighted_end);
                c.ebx = c.esi;
            } else {
                scalar(c, 1, radius.weighted_end);
                set_low(c, 7, radius.value);
            }
            c.mxcsr = s.mxcsr;
            c.edi = c.ebp + 236;
            g.set_u32(c.edi, radius.value)?;
            adjust(c, g)
        }
        0xc0e8c | 0xc0ebf => {
            let first = pc == 0xc0e8c;
            if first {
                let one = g.u32(ONE)?;
                scalar(c, 7, one);
                let mut s = Sse { mxcsr: c.mxcsr };
                let t = s.sub(one, low(c, 0)).map_err(fp)?;
                let fade = s.mul(t, g.u32(c.ebp + 204)?).map_err(fp)?;
                c.mxcsr = s.mxcsr;
                scalar(c, 5, fade);
                g.set_u32(c.ebp + 208, fade)?;
            } else {
                scalar(c, 5, g.u32(c.ebp + 208)?);
            }
            c.eax = g.u32(c.ebx + 52)?;
            c.edi = c.ebp + if first { 212 } else { 224 };
            c.esi = c.ebx + if first { 100 } else { 76 };
            c.ecx = c.ebx + if first { 88 } else { 64 };
            call(INTERPOLATE, if first { 0xc0ebf } else { 0xc0edb })
        }
        0xc0f75 => {
            c.eax = g.u32(sp + 24)?;
            logic(c, c.eax, 1 << 31);
            scalar(c, 1, 0);
            g.set_u32(c.ebp + 200, 0)?;
            if c.eax != 0 {
                c.ecx = g.u32(sp + 16)?;
                return call(points::OBJECT_ROOT.base, 0xc0f95);
            }
            clamp(c, g)
        }
        0xc0f95 => {
            attenuate(c, g)?;
            clamp(c, g)
        }
        0xc1275 | 0xc1280 => {
            logic(c, c.eax & 255, 128);
            let yes = c.eax as u8 != 0;
            if pc == 0xc1275 && !yes {
                c.eax = c.esi;
                return call(OWNER_TEST_2, 0xc1280);
            }
            if pc == 0xc1280 && yes {
                return add_light(c, g);
            }
            finish(c, g)
        }
        0xc129c => finish(c, g),
        0xc12b9 => {
            let v = g.u8(c.ebp + 2)? & !2;
            g.set_u8(c.ebp + 2, v)?;
            finish(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

fn begin(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esp = c.esp.wrapping_sub(88);
    c.eax = g.u32(LIGHTS)?;
    c.edx = g.u32(points::TAG_ENTRIES.base)?;
    push(c, g, c.ebx)?;
    c.ebx = g.u32(c.eax + 68)?;
    push(c, g, c.ebp)?;
    c.ebp = c.ebx.wrapping_add((g.u32(c.esp + 100)? & 0xffff) * 272);
    c.ecx = (g.u32(c.ebp + 4)? & 0xffff) * 16;
    c.eax = g.u32(c.ebp + 76)?;
    c.ebx = g.u32(c.edx + c.ecx + 8)?;
    c.ecx = u32::MAX;
    push(c, g, c.esi)?;
    push(c, g, c.edi)?;
    let sp = c.esp;
    g.set_u32(sp + 20, c.ebx)?;
    g.set_u32(sp + 16, c.ecx)?;
    g.set_u32(sp + 24, 0)?;
    if c.eax != u32::MAX {
        c.edx = g.u32(g.u32(points::OBJECTS.base)? + 68)?;
        g.set_u32(sp + 16, c.eax)?;
        c.eax = g.u32(c.edx + (c.eax & 0xffff) * 12 + 8)?;
        g.set_u32(sp + 24, c.eax)?;
    } else {
        c.eax = g.u32(c.ebp + 88)?;
        if c.eax != u32::MAX {
            g.set_u32(sp + 16, c.eax)?;
            c.edx = (c.eax & 0xffff) * 3;
            c.eax = g.u32(g.u32(points::OBJECTS.base)? + 68)?;
            c.edx = g.u32(c.eax + c.edx * 4 + 8)?;
            g.set_u32(sp + 24, c.edx)?;
        }
    }
    if g.u16(c.ebp + 84)? != u16::MAX {
        // Four RGB endpoints with a weight after each triple.
        let origin = g.u32(COLOR_ORIGIN)?;
        let one = g.u32(ONE)?;
        scalar(c, 0, one);
        for (from, to) in [(origin, 28), (c.ebx + 88, 60), (c.ebx + 100, 44), (c.ebx + 64, 92), (c.ebx + 76, 76)] {
            for off in [0, 4, 8] {
                let v = g.u32(from + off)?;
                g.set_u32(sp + to + off, v)?;
            }
        }
        c.eax = g.u32(c.ebx + 84)?;
        let old = c.ebx;
        arithmetic(c, old, 76, false);
        c.ecx = g.u32(c.ebp + 80)?;
        c.ebx = c.ebp + 208;
        push(c, g, c.ebx)?;
        c.edx = g.u32(sp + 16)?;
        push(c, g, c.ecx)?;
        push(c, g, c.edx)?;
        for off in [40, 56, 72, 88] {
            g.set_u32(sp + off, one)?;
        }
        return call(points::OBJECT_FUNCTION_VALUE.base, 0xc0dc9);
    }
    let flags = g.u8(c.ebp + 2)? & 16;
    logic(c, u32::from(flags), 128);
    if flags != 0 {
        c.eax = g.u32(points::GAME_TIME.base)?;
        c.ecx = arithmetic(c, g.u32(c.eax + 8)?, g.u32(c.ebp + 20)?, true);
        c.edx = g.u16(c.eax + 2)? as i16 as i32 as u32;
        c.eax = (c.eax & !65535) | u32::from(g.u16(c.ebx + 182)?);
        let mut s = Sse { mxcsr: c.mxcsr };
        let rate = s.int(c.edx as i32).map_err(fp)?;
        let denom = s.mul(rate, g.u32(c.ebx + 176)?).map_err(fp)?;
        let ticks = s.int(c.ecx as i32).map_err(fp)?;
        let t = s.div(ticks, denom).map_err(fp)?;
        set_low(c, 0, t);
        set_low(c, 1, denom);
        c.mxcsr = s.mxcsr;
        return call(FADE, 0xc0e8c);
    }
    let t = g.u32(c.ebp + 204)?;
    g.set_u32(c.ebp + 208, t)?;
    for (src, dst) in [(100, 212), (76, 224)] {
        for i in [0, 4, 8] {
            let v = g.u32(c.ebx + src + i)?;
            g.set_u32(c.ebp + dst + i, v)?;
        }
    }
    c.edi = c.ebp + 236;
    let v = g.u32(c.ebx + 116)?;
    g.set_u32(c.edi, v)?;
    adjust(c, g)
}

fn attenuate(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.edx = g.u32(points::OBJECTS.base)?;
    c.ecx = (c.eax & 65535) * 3;
    c.eax = g.u32(g.u32(c.edx + 68)? + 4 * c.ecx + 8)?;
    c.ecx = byte(c.ecx, g.u8(c.eax + 170)?);
    c.edx = 1u32 << (c.ecx & 31);
    if c.edx & 3 == 0 {
        return Ok(());
    }
    let amount = g.u32(c.eax + 688)?;
    scalar(c, 0, amount);
    let zero = low(c, 1);
    if diff(c, amount, zero)? & 0x41 != 0 {
        return Ok(());
    }
    c.edx = g.u32(points::TAG_ENTRIES.base)?;
    c.ecx = g.u32(c.edx + 16 * (g.u32(c.ebp + 4)? & 65535) + 8)?;
    if (g.u8(c.ecx)? as i8) < 0 {
        return Ok(());
    }
    let one = g.u32(ONE)?;
    let mut s = Sse { mxcsr: c.mxcsr };
    let atten = s.clamp(amount, zero, one).map_err(fp)?;
    g.set_u32(c.ebp + 200, atten)?;
    let inv = s.sub(one, atten).map_err(fp)?;
    for (off, p) in [c.esi, c.ebp + 216, c.ebp + 220, c.ebp + 224, c.ebp + 228, c.ebp + 232].into_iter().enumerate() {
        let v = g.u32(p)?;
        let result = if off < 4 { s.mul(v, inv) } else { s.mul(inv, v) }.map_err(fp)?;
        g.set_u32(p, result)?;
        scalar(c, if off == 5 { 0 } else { 3 }, result);
    }
    c.mxcsr = s.mxcsr;
    Ok(())
}

fn clamp(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let sp = c.esp;
    let zero = low(c, 1);
    let one = g.u32(ONE)?;
    scalar(c, 2, one);
    let mut s = Sse { mxcsr: c.mxcsr };
    let mut rgb = [0u32; 6];
    for (i, p) in [c.esi, c.ebp + 216, c.ebp + 220, c.ebp + 224, c.ebp + 228, c.ebp + 232].into_iter().enumerate() {
        let v = g.u32(p)?;
        let below = s.above(zero, v).map_err(fp)?;
        rgb[i] = if below { zero } else if s.above(v, one).map_err(fp)? { one } else { v };
        if i < 2 {
            g.set_u32(sp + 20 + i as u32 * 4, rgb[i])?;
        } else if below {
            // The low clamp copies the whole register (movaps). At the
            // root-lookup continuation XMM1 belongs to the returning callee.
            c.xmm[9 - i] = c.xmm[1];
        } else {
            scalar(c, 9 - i, rgb[i]);
        }
        g.set_u32(p, rgb[i])?;
    }
    let v = s.mul(g.u32(c.edi)?, g.u32(RADIUS_SCALE)?).map_err(fp)?;
    let radius = if s.above(zero, v).map_err(fp)? {
        zero
    } else {
        let max = g.u32(RADIUS_MAX)?;
        scalar(c, 3, max);
        if s.above(v, max).map_err(fp)? { max } else { v }
    };
    g.set_u32(c.edi, radius)?;
    let t = g.u32(c.ebp + 208)?;
    c.ecx = (c.ecx & !65535) | u32::from(g.u16(c.ebp + 2)?);
    let intensity = blend(&mut s, g.u32(c.ebx + 8)?, g.u32(c.ebx + 12)?, t, one)?;
    scalar(c, 0, intensity.weighted_end);
    scalar(c, 2, intensity.value);
    g.set_u32(c.ebp + 196, intensity.value)?;
    let mut active = c.ecx & 1 != 0 && s.above(intensity.value, zero).map_err(fp)?;
    if active {
        active = false;
        for (i, value) in rgb.into_iter().enumerate() {
            if i < 2 {
                scalar(c, 0, value);
            }
            let flags = simd::ucomiss(&mut s.mxcsr, value, zero).map_err(|_| fp(Error::Simd))?;
            let ah = (flags | 2) & 255;
            c.eax = (c.eax & !0xff00) | (ah << 8);
            if flags & 0x44 != 0x40 {
                active = true;
                break;
            }
        }
    }
    c.mxcsr = s.mxcsr;
    if active {
        if c.ecx & 2 != 0 {
            return finish(c, g);
        }
        c.esi = g.u32(sp + 16)?;
        c.ecx |= 2;
        g.set_u16(c.ebp + 2, c.ecx as u16)?;
        arithmetic(c, c.esi, u32::MAX, true);
        if c.esi == u32::MAX {
            return add_light(c, g);
        }
        c.ecx = c.esi;
        return call(OWNER_TEST, 0xc1275);
    }
    if c.ecx & 2 != 0 {
        let notify = c.ecx & 8;
        logic(c, notify, 128);
        if notify != 0 {
            c.eax = g.u32(sp + 108)?;
            return call(UNLINK, 0xc12b9);
        }
        let new = g.u8(c.ebp + 2)? & !2;
        g.set_u8(c.ebp + 2, new)?;
    }
    finish(c, g)
}

fn add_light(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(PHASE)?;
    if c.eax != 0 {
        let active = g.u8(c.eax)?;
        if active != 0 {
            // CMP byte ptr [eax],0.
            let f = status(u32::from(active), 128);
            c.eflags = (c.eflags & !ARITH) | f;
            c.eflags_defined |= ARITH;
            c.edx = g.u32(c.esp + 108)?;
            push(c, g, c.edx)?;
            return call(LINK, 0xc129c);
        }
    }
    finish(c, g)
}
