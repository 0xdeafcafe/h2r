//! Blend the motion of the manager's primary and transition channels.
use super::*;
use crate::function_eval::constant::{ONE, SMALL};
use crate::sim_native::sub_flags;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, byte, logic};

pub(super) const BODIES: &[Body] = &[
    body!(
        0x1cdb00,
        0x1cdcc3,
        &[
            0x1cdb6b, 0x1cdb88, 0x1cdb97, 0x1cdba6, 0x1cdbcf, 0x1cdbe0, 0x1cdc9b
        ],
        update_movement
    ),
    body!(
        0x1cdcd0,
        0x1cdef1,
        &[0x1cdd01, 0x1cdd4c, 0x1cdd94, 0x1cdda8, 0x1cddba, 0x1cdeb4],
        sample_movement
    ),
    body!(0x1cdf00, 0x1cdf4b, &[], blend_weight),
    body!(0x1cdf50, 0x1ce006, &[0x1cdfa9, 0x1cdfbe], reset_channels),
];

const TRANSITION_ENABLED: u32 = 0x46fbf4;
const OVERRIDE_ENABLED: u32 = 0x46fbf5;
const NEAR_ONE: u32 = 0x45dc78;
const CLIP: u32 = 0x1c6440;
const SAMPLE: u32 = 0x1c6c00;

fn different(c: &mut CpuAbi, value: u32, other: u32, width: u32) -> bool {
    sub_flags(c, value, other, width);
    if width == 16 {
        value as u16 != other as u16
    } else {
        value != other
    }
}

fn valid_channel(c: &mut CpuAbi, g: &Bus<'_>, channel: u32, none: u32) -> Result<bool, GuestFault> {
    Ok(different(c, g.u32(channel)?, none, 32)
        && different(c, u32::from(g.u16(at(channel, 6))?), none, 16))
}

fn motion_return(c: &mut CpuAbi, g: &mut Bus<'_>, saved_edi: bool) -> sim_native::Result {
    if saved_edi {
        c.edi = pop(c, g)?;
    }
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 24, false);
    g.ret(c, 8)
}

fn stop_movement(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    sse::zero(c, 2);
    g.set_u32(at(c.esi, 128), 0)?;
    motion_return(c, g, true)
}

fn update_movement(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cdb00 => {
            c.esp = c.esp.wrapping_sub(24);
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = g.u32(at(c.esp, 36))?;
            c.ebx = byte(c.ebx, 0);
            push(c, g, c.esi)?;
            c.esi = c.eax;
            if c.ebp != u32::MAX {
                c.ecx = g.u32(guest_layout::points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, 68))?;
                let slot = (c.ebp & 0xffff) * 12;
                c.eax = g.u32(at(c.edx, slot + 8))?;
                c.ecx = byte(c.ecx, g.u8(at(c.eax, 179))?);
                if c.ecx as u8 != 0 {
                    return motion_return(c, g, false);
                }
            }
            if g.u8(at(c.esi, 110))? & 1 != 0 {
                return motion_return(c, g, false);
            }
            c.eax = byte(c.eax, g.u8(TRANSITION_ENABLED)?);
            push(c, g, c.edi)?;
            if c.eax as u8 == 0 || !valid_channel(c, g, c.esi, u32::MAX)? {
                return stop_movement(c, g);
            }
            c.ecx = c.esi;
            g.call(c, CLIP, 0x1cdb6b)
        }
        0x1cdb6b => {
            if c.eax == 0 {
                return stop_movement(c, g);
            }
            c.ecx = byte(c.ecx, g.u8(at(c.eax, 17))?);
            logic(c, c.ecx & 0xff, 128);
            if c.ecx as u8 == 0 {
                return stop_movement(c, g);
            }
            c.edi = at(c.esi, 32);
            c.eax = c.edi;
            g.call(c, 0xb6760, 0x1cdb88)
        }
        0x1cdb88 => {
            logic(c, c.eax & 0xff, 128);
            if c.eax as u8 == 0 {
                return stop_movement(c, g);
            }
            c.ecx = c.edi;
            g.call(c, CLIP, 0x1cdb97)
        }
        0x1cdb97 => {
            logic(c, c.eax, 1 << 31);
            if c.eax == 0 {
                return stop_movement(c, g);
            }
            c.ecx = c.edi;
            g.call(c, CLIP, 0x1cdba6)
        }
        0x1cdba6 => {
            c.ecx = c.eax;
            c.eax = 0;
            let frames = g.u16(at(c.ecx, 20))?;
            sub_flags(c, u32::from(frames), 0, 16);
            if frames as i16 <= 0 {
                return stop_movement(c, g);
            }
            g.set_u32(at(c.esp, 16), 0)?;
            g.set_u32(at(c.esp, 20), 0)?;
            g.set_u8(at(c.esp, 24), 0)?;
            g.set_u8(at(c.esp, 25), 0)?;
            c.edi = at(c.esp, 16);
            c.eax = c.esi;
            g.call(c, 0x1c7380, 0x1cdbcf)
        }
        0x1cdbcf => {
            c.edi = at(c.esp, 44);
            c.eax = at(c.esp, 28);
            c.edx = at(c.esp, 16);
            g.call(c, 0x20ab60, 0x1cdbe0)
        }
        0x1cdbe0 => scale_movement(c, g),
        0x1cdc9b => {
            c.ebx = byte(c.ebx, c.eax as u8);
            if c.ebx as u8 == 0 {
                stop_movement(c, g)
            } else {
                motion_return(c, g, true)
            }
        }
        _ => Err(bad(pc)),
    }
}

fn scale_movement(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.eax = g.u32(at(c.esp, 48))?;
    sse::load(c, 2, g.u32(at(c.eax, 4))?);
    sse::load(c, 1, g.u32(at(c.eax, 8))?);
    sse::load(c, 0, g.u32(c.eax)?);
    sse::movaps(c, 3, 2);
    sse::ss_reg(c, Ss::Mul, 3, 2);
    sse::ss_reg(c, Ss::Mul, 0, 0);
    sse::movaps(c, 2, 1);
    sse::ss_reg(c, Ss::Mul, 2, 1);
    sse::ss_reg(c, Ss::Add, 0, 3);
    sse::ss_reg(c, Ss::Add, 0, 2);
    sse::zero(c, 2);
    if sse::comiss(c, sse::lane0(c, 0), 0) & 0x41 != 0 {
        g.set_u32(at(c.esi, 128), 0)?;
        return motion_return(c, g, true);
    }
    // Magnitude uses the exponent-halving estimate, including its stack spill.
    let temp = at(c.esp, 44);
    g.set_u32(temp, sse::lane0(c, 0))?;
    c.edx = ((g.u32(temp)? as i32 >> 1) as u32).wrapping_add(g.u32(0x1cdc26)?);
    g.set_u32(temp, c.edx)?;
    sse::load(c, 1, g.u32(temp)?);
    if sse::comiss(c, 0, sse::lane0(c, 1)) & 0x41 == 0 {
        sse::movaps(c, 1, 2);
    } else {
        sse::load(c, 0, g.u32(ONE)?);
        if sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0)) & 0x41 == 0 {
            sse::movaps(c, 1, 0);
        }
    }
    c.eax = g.u32(0x510c54)?;
    c.ecx = g.u16(at(c.eax, 2))? as i16 as u32;
    sse::cvtsi2ss(c, 0, c.ecx);
    sse::ss_reg(c, Ss::Mul, 0, 1);
    for offset in [0, 4, 8] {
        sse::load(c, 1, g.u32(at(c.esp, 28 + offset))?);
        sse::ss_reg(c, Ss::Mul, 1, 0);
        if offset == 8 {
            c.edx = at(c.esp, 16);
            push(c, g, c.edx)?;
            c.eax = c.ebp;
            g.set_u32(at(c.esp, 28), sse::lane0(c, 1))?;
        } else {
            g.set_u32(at(c.esp, 16 + offset), sse::lane0(c, 1))?;
        }
    }
    g.call(c, 0x1cd8a0, 0x1cdc9b)
}

fn sample_return(c: &mut CpuAbi, g: &mut Bus<'_>, saved_ebx: bool) -> sim_native::Result {
    if saved_ebx {
        c.ebx = pop(c, g)?;
    }
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 48, false);
    g.ret(c, 8)
}

fn sample_movement(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cdcd0 => {
            c.eax = byte(c.eax, g.u8(OVERRIDE_ENABLED)?);
            c.esp = c.esp.wrapping_sub(48);
            push(c, g, c.ebp)?;
            c.ebp = g.u32(at(c.esp, 60))?;
            c.edx = u32::MAX;
            push(c, g, c.esi)?;
            if c.eax as u8 != 0 {
                c.eax = g.u32(at(c.edi, 64))?;
                c.ecx = at(c.edi, 64);
                if different(c, c.eax, c.edx, 32)
                    && different(c, u32::from(g.u16(at(c.ecx, 6))?), c.edx, 16)
                {
                    c.eax = g.u32(at(c.esp, 60))?;
                    push(c, g, c.eax)?;
                    c.esi = c.ebp;
                    return g.call(c, SAMPLE, 0x1cdd01);
                }
            }
            c.eax = byte(c.eax, g.u8(TRANSITION_ENABLED)?);
            push(c, g, c.ebx)?;
            if c.eax as u8 != 0 {
                c.eax = g.u32(at(c.edi, 32))?;
                let has_graph = different(c, c.eax, c.edx, 32);
                sse::load(c, 0, g.u32(NEAR_ONE)?);
                c.ecx = at(c.edi, 32);
                if has_graph {
                    if different(c, u32::from(g.u16(at(c.ecx, 6))?), c.edx, 16) {
                        sse::load(c, 1, g.u32(at(c.edi, 128))?);
                        if sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0)) & 1 == 0 {
                            c.edx = g.u32(at(c.esp, 64))?;
                            push(c, g, c.edx)?;
                            c.esi = c.ebp;
                            return g.call(c, SAMPLE, 0x1cdd4c);
                        }
                    }
                    if valid_channel(c, g, c.ecx, c.edx)? {
                        sse::load(c, 1, g.u32(SMALL)?);
                        let weight = g.u32(at(c.edi, 128))?;
                        if sse::comiss(c, sse::lane0(c, 1), weight) & 1 != 0 {
                            sse::load(c, 1, g.u32(at(c.edi, 128))?);
                            if sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0)) & 1 != 0 {
                                c.eax = c.edi;
                                return g.call(c, 0x1cdf00, 0x1cdd94);
                            }
                        }
                    }
                }
            }
            sample_primary(c, g)
        }
        0x1cdd01 => sample_return(c, g, false),
        0x1cdd4c => sample_return(c, g, true),
        0x1cdd94 => {
            c.eax = at(c.esp, 12);
            push(c, g, c.eax)?;
            c.esi = at(c.esp, 40);
            g.set_u32(at(c.esp, 72), sse::lane0(c, 0))?;
            g.call(c, SAMPLE, 0x1cdda8)
        }
        0x1cdda8 => {
            c.ecx = at(c.esp, 24);
            push(c, g, c.ecx)?;
            c.esi = at(c.esp, 52);
            c.ecx = c.edi;
            c.ebx = byte(c.ebx, c.eax as u8);
            g.call(c, SAMPLE, 0x1cddba)
        }
        0x1cddba => blend_movement(c, g),
        0x1cdeb4 => {
            sse::load(c, 0, g.u32(at(c.esp, 68))?);
            sse::load(c, 1, g.u32(at(c.esp, 48))?);
            sse::ss_reg(c, Ss::Mul, 1, 0);
            g.set_u32(c.ebp, sse::lane0(c, 1))?;
            sse::movaps(c, 1, 0);
            sse::ss(c, Ss::Mul, 1, g.u32(at(c.esp, 52))?);
            g.set_u32(at(c.ebp, 4), sse::lane0(c, 1))?;
            sse::load(c, 1, g.u32(at(c.esp, 56))?);
            sse::ss_reg(c, Ss::Mul, 1, 0);
            g.set_u32(at(c.ebp, 8), sse::lane0(c, 1))?;
            c.eax = byte(c.eax, 1);
            sample_return(c, g, true)
        }
        _ => Err(bad(pc)),
    }
}

fn sample_primary(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.ecx = g.u32(c.edi)?;
    c.eax = byte(c.eax, 0);
    if !different(c, c.ecx, c.edx, 32) || !different(c, u32::from(g.u16(at(c.edi, 6))?), c.edx, 16)
    {
        return sample_return(c, g, true);
    }
    sse::load(c, 0, g.u32(at(c.edi, 24))?);
    sse::ss(c, Ss::Mul, 0, g.u32(0x45dcfc)?);
    c.edx = g.u32(at(c.esp, 64))?;
    g.set_u32(at(c.esp, 68), sse::lane0(c, 0))?;
    sse::load(c, 0, g.u32(at(c.edi, 28))?);
    push(c, g, c.edx)?;
    c.ebx = at(c.esp, 52);
    c.ecx = c.edi;
    g.call(c, 0x1c6c80, 0x1cdeb4)
}

fn blend_movement(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    let sp = c.esp;
    sse::load(c, 0, g.u32(at(sp, 68))?);
    sse::load(c, 1, g.u32(at(sp, 24))?);
    // Two vectors, each component B + (A - B) * weight. The next B
    // component is already live before the preceding output is written.
    for (i, source) in [12, 16, 20, 36, 40, 44].into_iter().enumerate() {
        sse::load(c, 2, g.u32(at(sp, source))?);
        sse::ss_reg(c, Ss::Sub, 2, 1);
        if i == 5 {
            c.eax = byte(c.eax, c.ebx as u8);
            c.ebx = pop(c, g)?;
        }
        sse::ss_reg(c, Ss::Mul, 2, 0);
        sse::ss_reg(c, Ss::Add, 2, 1);
        if let Some(next) = [28, 32, 48, 52, 56].get(i) {
            sse::load(c, 1, g.u32(at(sp, *next))?);
        }
        if i == 0 {
            c.ebx = byte(c.ebx, (c.ebx & c.eax) as u8);
            logic(c, c.ebx & 0xff, 128);
            c.eax = g.u32(at(sp, 64))?;
        }
        if i == 5 {
            c.esi = pop(c, g)?;
        }
        let base = if i < 3 { c.eax } else { c.ebp };
        g.set_u32(at(base, (i as u32 % 3) * 4), sse::lane0(c, 2))?;
    }
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 48, false);
    g.ret(c, 8)
}

fn blend_weight(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> sim_native::Result {
    sse::load(c, 0, g.u32(at(c.eax, 128))?);
    sse::ss(c, Ss::Add, 0, g.u32(SMALL)?);
    sse::zero(c, 1);
    if sse::comiss(c, 0, sse::lane0(c, 0)) & 0x41 == 0 {
        sse::movaps(c, 0, 1);
    } else {
        sse::load(c, 2, g.u32(NEAR_ONE)?);
        if sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2)) & 0x41 == 0 {
            sse::movaps(c, 0, 2);
        }
    }
    sse::ss(c, Ss::Mul, 0, g.u32(0x45dc74)?);
    if sse::comiss(c, 0, sse::lane0(c, 0)) & 0x41 == 0 {
        sse::movaps(c, 0, 1);
    } else {
        sse::load(c, 1, g.u32(ONE)?);
        if sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 1)) & 0x41 == 0 {
            sse::movaps(c, 0, 1);
        }
    }
    g.ret(c, 0)
}

fn mark_channel(c: &mut CpuAbi, g: &mut Bus<'_>, channel: u32) -> Result<(), GuestFault> {
    if valid_channel(c, g, channel, c.ebx)? {
        let flags = g.u8(at(channel, 18))? & c.eax as u8;
        logic(c, u32::from(flags), 128);
        if flags != 0 {
            let status = g.u8(at(channel, 17))? | c.eax as u8;
            g.set_u8(at(channel, 17), status)?;
            logic(c, u32::from(status), 128);
        }
    }
    Ok(())
}

fn reset_channels(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cdf50 {
        sse::zero(c, 0);
        push(c, g, c.ebx)?;
        c.ebx = u32::MAX;
        g.set_u32(at(c.edi, 64), c.ebx)?;
        g.set_u16(at(c.edi, 68), c.ebx as u16)?;
        g.set_u16(at(c.edi, 70), c.ebx as u16)?;
        c.eax = 0;
        g.set_u32(at(c.edi, 92), 0)?;
        sse::load(c, 0, g.u32(ONE)?);
        g.set_u8(at(c.edi, 80), 0)?;
        g.set_u8(at(c.edi, 81), 0)?;
        g.set_u16(at(c.edi, 82), 0)?;
        g.set_u32(at(c.edi, 88), sse::lane0(c, 0))?;
        g.set_u16(at(c.edi, 84), 0)?;
        g.set_u16(at(c.edi, 86), 0)?;
        g.set_u32(at(c.edi, 72), c.ebx)?;
        g.set_u8(at(c.edi, 76), c.ebx as u8)?;
        g.set_u8(at(c.edi, 77), c.ebx as u8)?;
        g.set_u16(at(c.edi, 78), c.ebx as u16)?;
        let first = different(c, g.u32(c.edi)?, c.ebx, 32);
        push(c, g, c.esi)?;
        if first && different(c, u32::from(g.u16(at(c.edi, 6))?), c.ebx, 16) {
            c.esi = c.edi;
            return g.call(c, 0x1c6500, 0x1cdfa9);
        }
    }
    if pc != 0x1cdfbe {
        c.eax = g.u32(at(c.edi, 32))?;
        c.esi = at(c.edi, 32);
        if different(c, c.eax, c.ebx, 32)
            && different(c, u32::from(g.u16(at(c.esi, 6))?), c.ebx, 16)
        {
            return g.call(c, 0x1c6500, 0x1cdfbe);
        }
    }
    g.set_u8(at(c.edi, 101), 0)?;
    g.set_u8(at(c.edi, 100), 0)?;
    g.set_u8(at(c.edi, 103), 0)?;
    c.eax = byte(c.eax, 1);
    for channel in [at(c.edi, 64), c.edi, c.esi] {
        mark_channel(c, g, channel)?;
    }
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 0)
}
