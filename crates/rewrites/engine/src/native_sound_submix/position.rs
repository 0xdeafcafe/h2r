//! The mix-in record's position cache and its two acoustic ramps.
//! The position setter can deliver callbacks; the cached coordinates come
//! from the source after that setter returns.

use super::{SOUND_GLOBALS, call};
use crate::points;
use crate::sim_native::{add_flags, sub_flags};
use crate::sim_objects::frame::{pop, push, set_slot, slot};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_objects::{Cx, Next, Result};
use crate::sim_physics::spatial::{finish_fpu, load_fpu};
use crate::sim_physics::{ARITH, logic};
use runtime::{CpuAbi, Guest};

pub(super) const POSITION_SENT: u32 = 0x220ea6;
pub(super) const EFFECTS_SENT: u32 = 0x220fc2;
const SET_POSITION: u32 = 0x37c668;
const POSITION_EPSILON: u32 = 0x45dc1c;
const ACOUSTIC_EPSILON: u32 = 0x45dc70;

fn low_byte(reg: u32, value: u8) -> u32 {
    reg & !0xff | u32::from(value)
}

// Each comparison short-circuits once one coordinate reaches the threshold.
// Preserve the FPU state from the last comparison actually performed.
fn within(
    c: &mut CpuAbi,
    g: &Guest,
    target: u32,
    old: u32,
    epsilon: u32,
    pop_pc: u32,
) -> Result<bool> {
    let mut fpu = load_fpu(c);
    let flags =
        sim_sound::submix::compare_delta(&mut fpu.0, g.u32(target)?, g.u32(old)?, g.u32(epsilon)?);
    finish_fpu(c, g, fpu, pop_pc, epsilon)?;
    c.eflags = (c.eflags & !ARITH) | flags;
    c.eflags_defined |= ARITH;
    Ok(flags & 0x41 == 0)
}

fn submit_position(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = g.u32(c.edi.wrapping_add(8))?;
    c.eax = g.u32(c.edi.wrapping_add(12))?;
    c.ecx = g.u32(c.edi.wrapping_add(4))?;
    push(c, g, 1)?;
    push(c, g, c.edx)?;
    c.edx = g.u32(c.esi.wrapping_add(0x34))?;
    // DirectSound takes the world axes in x, z, y order.
    push(c, g, c.eax)?;
    push(c, g, c.ecx)?;
    push(c, g, c.edx)?;
    Ok(call(SET_POSITION, POSITION_SENT))
}

fn finish(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let flags = c.esi.wrapping_add(4);
    g.set_u8(flags, g.u8(flags)? | 2)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    add_flags(c, c.esp, 8);
    c.esp = c.esp.wrapping_add(8);
    Ok(Next::Ret { pop: 4 })
}

pub(super) fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let c = &mut *cx.cpu;
    let g = &mut *cx.g;
    if pc == EFFECTS_SENT {
        return finish(c, g);
    }
    let fresh = if pc == POSITION_SENT {
        c.ecx = g.u32(c.edi.wrapping_add(4))?;
        sse::load(c, 2, slot(c, g, 0x10)?);
        sse::load(c, 0, slot(c, g, 0x0c)?);
        c.eax = c.esi.wrapping_add(8);
        g.set_u32(c.eax, c.ecx)?;
        c.edx = g.u32(c.edi.wrapping_add(8))?;
        g.set_u32(c.eax.wrapping_add(4), c.edx)?;
        c.ecx = g.u32(c.edi.wrapping_add(12))?;
        g.set_u32(c.eax.wrapping_add(8), c.ecx)?;
        let fresh = g.u8(c.esp.wrapping_add(0x18))?;
        c.eax = low_byte(c.eax, fresh);
        logic(c, u32::from(fresh), 0x80);
        fresh != 0
    } else {
        c.eax = g.u32(SOUND_GLOBALS)?;
        c.esp = c.esp.wrapping_sub(8);
        push(c, g, c.ebx)?;
        push(c, g, c.ebp)?;
        c.ebp = slot(c, g, 0x14)?;
        c.ecx = c.ebp.wrapping_mul(0x3c);
        push(c, g, c.esi)?;
        c.esi = c.eax.wrapping_add(0x1a18).wrapping_add(c.ecx);
        let fresh = g.u8(c.esi.wrapping_add(4))? & 2 == 0 || g.u8(c.eax)? == 0;
        g.set_u8(c.esp.wrapping_add(0x18), u8::from(fresh))?;
        c.ebx = low_byte(c.ebx, g.u8(c.edi)? & 1);
        c.eax = low_byte(c.eax, u8::from(fresh));
        sse::load(c, 2, g.u32(c.edi.wrapping_add(0x10))?);
        sse::load(c, 0, g.u32(c.edi.wrapping_add(0x14))?);
        logic(c, u32::from(fresh), 0x80);
        set_slot(c, g, 0x10, sse::lane0(c, 2))?;
        set_slot(c, g, 0x0c, sse::lane0(c, 0))?;

        if fresh {
            return submit_position(c, g);
        }
        for (offset, pop_pc) in [(4, 0x220e63), (8, 0x220e77), (12, 0x220e8b)] {
            if !within(
                c,
                g,
                c.edi.wrapping_add(offset),
                c.esi.wrapping_add(offset + 4),
                POSITION_EPSILON,
                pop_pc,
            )? {
                return submit_position(c, g);
            }
        }
        false
    };

    if !fresh {
        let same_occlusion = within(
            c,
            g,
            c.esp.wrapping_add(0x10),
            c.esi.wrapping_add(0x2c),
            ACOUSTIC_EPSILON,
            0x220edf,
        )?;
        if same_occlusion
            && within(
                c,
                g,
                c.esp.wrapping_add(0x0c),
                c.esi.wrapping_add(0x30),
                ACOUSTIC_EPSILON,
                0x220ef4,
            )?
        {
            c.edx = low_byte(c.edx, g.u8(c.esi.wrapping_add(4))? & 1);
            sub_flags(c, c.edx, c.ebx, 8);
            if c.edx as u8 == c.ebx as u8 {
                return finish(c, g);
            }
        }
    }

    smooth(c, g, fresh)?;
    let flags_at = c.esi.wrapping_add(4);
    let flags = g.u8(flags_at)?;
    c.edx = low_byte(c.edx, (flags ^ c.ebx as u8) & 1);
    c.ebx = low_byte(c.ebx, flags ^ c.edx as u8);
    logic(c, u32::from(c.ebx as u8), 0x80);
    g.set_u8(flags_at, c.ebx as u8)?;
    c.eax = g.u32(c.edi.wrapping_add(0x18))?;
    push(c, g, c.eax)?;
    c.eax = c.ebp;
    Ok(call(points::SOUND_SUBMIX_FX.base, EFFECTS_SENT))
}

fn smooth(c: &mut CpuAbi, g: &mut Guest, fresh: bool) -> Result<()> {
    let old_obstruction = c.esi.wrapping_add(0x30);
    let old_occlusion = c.esi.wrapping_add(0x2c);
    sse::zero(c, 1);
    if fresh {
        g.set_u32(old_obstruction, sse::lane0(c, 0))?;
        g.set_u32(old_occlusion, sse::lane0(c, 2))?;
        return Ok(());
    }

    let mode = g.u8(c.esi.wrapping_add(4))? & 1;
    c.eax = low_byte(c.eax, mode);
    let rate_at = c.edi.wrapping_add(0x20);
    let same_mode = mode == c.ebx as u8;
    let ramp = same_mode && sse::comiss(c, 0, g.u32(rate_at)?) & 1 != 0;
    if ramp {
        sse::ss(c, Ss::Sub, 0, g.u32(old_obstruction)?);
        sse::load(c, 3, g.u32(rate_at)?);
        sse::movaps(c, 4, 0);
        sse::movaps(c, 0, 1);
        let negative = sse::ss_reg(c, Ss::Sub, 0, 3);
        let delta = sse::lane0(c, 4);
        sse::comiss(c, negative, delta);
        if sse::below_or_equal(c) {
            sse::comiss(c, delta, sse::lane0(c, 3));
            sse::movaps(c, 0, 3);
            if sse::below_or_equal(c) {
                sse::movaps(c, 0, 4);
            }
        }
        sse::load(c, 3, g.u32(old_obstruction)?);
        g.set_u32(old_obstruction, sse::ss_reg(c, Ss::Add, 3, 0))?;
    } else {
        c.eax = low_byte(c.eax, g.u8(c.esp.wrapping_add(0x18))?);
        g.set_u32(old_obstruction, sse::lane0(c, 0))?;
    }

    let mode = g.u8(c.esi.wrapping_add(4))? & 1;
    c.ecx = low_byte(c.ecx, mode);
    let rate_at = c.edi.wrapping_add(0x1c);
    let ramp = mode == c.ebx as u8 && sse::comiss(c, 0, g.u32(rate_at)?) & 1 != 0;
    if ramp {
        sse::load(c, 0, g.u32(rate_at)?);
        let delta = sse::ss(c, Ss::Sub, 2, g.u32(old_occlusion)?);
        let negative = sse::ss_reg(c, Ss::Sub, 1, 0);
        sse::comiss(c, negative, delta);
        if sse::below_or_equal(c) {
            sse::comiss(c, delta, sse::lane0(c, 0));
            if sse::below_or_equal(c) {
                sse::movaps(c, 0, 2);
            }
        } else {
            sse::movaps(c, 0, 1);
        }
        g.set_u32(old_occlusion, sse::ss(c, Ss::Add, 0, g.u32(old_occlusion)?))?;
    } else {
        g.set_u32(old_occlusion, sse::lane0(c, 2))?;
    }
    Ok(())
}
