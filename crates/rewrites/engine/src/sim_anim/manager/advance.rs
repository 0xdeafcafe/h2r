//! Channel priority, outgoing motion, and the translation/rate blend.
//! Every call uses the live guest stack; a callback can replace a channel.
use super::*;
use ::sim_anim::manager_channels::{ChannelMix, Mix};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::spatial::{finish_fpu, fp_error, load_fpu};
use crate::sim_physics::{ARITH, arithmetic, byte, logic};

const SECONDARY_ENABLED: u32 = 0x46fbf4;
const OVERRIDE_ENABLED: u32 = 0x46fbf5;
const SMALL: u32 = crate::function_eval::constant::SMALL;
const ONE: u32 = crate::function_eval::constant::ONE;
const FULL_WEIGHT: u32 = 0x45dc78;
const ZERO_VECTOR: u32 = 0x4687a4;
const LOOKUP: u32 = 0x1c6440;
const SAMPLE: u32 = 0x1c6f60;
const MOTION: u32 = 0x1c6d00;

pub(super) const BODIES: &[Body] = &[
    body!(
        0x1cd4e0,
        0x1cd507,
        &[0x1cd4e9, 0x1cd503],
        tick_interpolators
    ),
    body!(0x1cd510, 0x1cd585, &[], select_channel),
    body!(
        0x1cd590,
        0x1cd6b2,
        &[0x1cd5c8, 0x1cd5ec, 0x1cd65a, 0x1cd673, 0x1cd693, 0x1cd6ac],
        sample_channels
    ),
    body!(
        0x1cd6c0,
        0x1cd89d,
        &[0x1cd714, 0x1cd791, 0x1cd7a5, 0x1cd7b5, 0x1cd84a],
        channel_motion
    ),
    body!(
        0x1cd8a0,
        0x1cdafe,
        &[
            0x1cd8fe, 0x1cd919, 0x1cd928, 0x1cd937, 0x1cd95d, 0x1cd966, 0x1cd97b, 0x1cd980,
            0x1cd9a3, 0x1cd9b4, 0x1cd9b9, 0x1cda13, 0x1cda28
        ],
        blend_rate
    ),
];

fn cmp(c: &mut CpuAbi, a: u32, b: u32, width: u32) -> bool {
    sim_native::sub_flags(c, a, b, width);
    c.eflags & 0x40 != 0
}
fn test_byte(c: &mut CpuAbi, b: u8) -> bool {
    logic(c, u32::from(b), 0x80);
    b != 0
}
fn graph_present(
    c: &mut CpuAbi,
    g: &Bus<'_>,
    channel: u32,
    sentinel: u32,
) -> Result<bool, GuestFault> {
    Ok(!cmp(c, g.u32(channel)?, sentinel, 32))
}
fn clip_present(
    c: &mut CpuAbi,
    g: &Bus<'_>,
    channel: u32,
    sentinel: u32,
) -> Result<bool, GuestFault> {
    Ok(!cmp(c, u32::from(g.u16(at(channel, 6))?), sentinel, 16))
}
fn valid(c: &mut CpuAbi, g: &Bus<'_>, channel: u32, sentinel: u32) -> Result<bool, GuestFault> {
    Ok(graph_present(c, g, channel, sentinel)? && clip_present(c, g, channel, sentinel)?)
}

fn tick_interpolators(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cd4e0 => {
            push(c, g, c.ebx)?;
            c.ecx = at(c.esi, 0x64);
            g.call(c, 0x1d9320, 0x1cd4e9)
        }
        0x1cd4e9 => {
            c.ebx = byte(c.ebx, c.eax as u8);
            if test_byte(c, g.u8(at(c.esi, 0x67))? & 2) {
                c.eax = byte(c.eax, 0);
                logic(c, 0, 0x80);
                for offset in [1, 0, 3] {
                    g.set_u8(at(c.ecx, offset), 0)?;
                }
            }
            c.ecx = at(c.esi, 0x60);
            g.call(c, 0x1d9320, 0x1cd503)
        }
        0x1cd503 => {
            c.eax = byte(c.eax, (c.eax | c.ebx) as u8);
            test_byte(c, c.eax as u8);
            c.ebx = pop(c, g)?;
            g.ret(c, 0)
        }
        _ => Err(bad(pc)),
    }
}

fn select_channel(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> sim_native::Result {
    push(c, g, c.ecx)?;
    push(c, g, c.ebx)?;
    c.ebx = byte(c.ebx, g.u8(OVERRIDE_ENABLED)?);
    c.edx = u32::MAX;
    let has_override = test_byte(c, c.ebx as u8);
    for offset in [4, 6] {
        g.set_u16(at(c.esp, offset), c.edx as u16)?;
    }
    let pair = if has_override && valid(c, g, at(c.ecx, 0x40), c.edx)? {
        at(c.ecx, 0x44)
    } else {
        c.ebx = byte(c.ebx, g.u8(SECONDARY_ENABLED)?);
        if test_byte(c, c.ebx as u8) && valid(c, g, at(c.ecx, 0x20), c.edx)? && {
            sse::load(c, 0, g.u32(at(c.ecx, 0x80))?);
            sse::comiss(c, sse::lane0(c, 0), g.u32(FULL_WEIGHT)?) & 1 == 0
        } {
            at(c.ecx, 0x24)
        } else if valid(c, g, c.ecx, c.edx)? {
            at(c.ecx, 4)
        } else {
            at(c.esp, 4)
        }
    };
    c.ecx = g.u32(pair)?;
    g.set_u32(c.eax, c.ecx)?;
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 0)
}

// Both consumers have already loaded the full-weight threshold and checked
// the graph handle. There are no writes or callbacks between the weight
// comparisons, so one gathered secondary weight serves the whole decision.
fn secondary_mix(c: &mut CpuAbi, g: &Bus<'_>, channel: u32, manager: u32, none: u32)
    -> Result<Mix, GuestFault>
{
    if !clip_present(c, g, channel, none)? {
        return Ok(Mix::Primary);
    }
    let decision = ChannelMix::select(
        g.u32(at(manager, 0x80))?, sse::lane0(c, 0), || g.u32(SMALL), &mut c.mxcsr,
    )?;
    sse::load(c, 1, decision.compared);
    c.eflags = (c.eflags & !ARITH) | decision.flags;
    c.eflags_defined |= ARITH;
    Ok(decision.channels)
}
fn sample_args(c: &mut CpuAbi, g: &mut Bus<'_>) -> Result<(), GuestFault> {
    c.ecx = g.u32(at(c.esp, 0x20))?;
    c.edx = g.u32(at(c.esp, 0x18))?;
    for v in [c.ecx, c.esi, c.ebp, c.edx] {
        push(c, g, v)?;
    }
    Ok(())
}
fn sample_channels(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cd590 => {
            c.eax = byte(c.eax, g.u8(OVERRIDE_ENABLED)?);
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = g.u32(at(c.esp, 0x18))?;
            c.ecx = u32::MAX;
            let has_override = test_byte(c, c.eax as u8);
            c.eax = g.u32(at(c.esp, 0xc))?;
            push(c, g, c.edi)?;
            if has_override {
                c.edx = g.u32(at(c.eax, 0x40))?;
                let present = !cmp(c, c.edx, c.ecx, 32);
                c.ebx = at(c.eax, 0x40);
                if present && clip_present(c, g, c.ebx, c.ecx)? {
                    c.eax = g.u32(at(c.esp, 0x20))?;
                    c.edi = g.u32(at(c.esp, 0x18))?;
                    for v in [c.eax, c.esi, c.ebp, c.edi] {
                        push(c, g, v)?;
                    }
                    return g.call(c, SAMPLE, 0x1cd5c8);
                }
            }
            c.edx = byte(c.edx, g.u8(SECONDARY_ENABLED)?);
            let mut chosen = c.eax;
            if test_byte(c, c.edx as u8) {
                c.edx = g.u32(at(c.eax, 0x20))?;
                let present = !cmp(c, c.edx, c.ecx, 32);
                sse::load(c, 0, g.u32(FULL_WEIGHT)?);
                c.edi = at(c.eax, 0x20);
                if present {
                    match secondary_mix(c, g, c.edi, c.eax, c.ecx)? {
                        Mix::Secondary => chosen = c.edi,
                        Mix::Both => return g.call(c, 0x1cdf00, 0x1cd65a),
                        Mix::Primary => {}
                    }
                }
            }
            c.ebx = chosen;
            sample_args(c, g)?;
            return g.call(c, SAMPLE, 0x1cd6ac);
        }
        0x1cd5c8 => {
            logic(c, c.esi, 1 << 31);
            if c.esi as i32 > 0 {
                logic(c, c.ebp, 1 << 31);
                if c.ebp == 0 || test_byte(c, g.u8(c.ebp)? & 1) {
                    c.ebx = g.u32(at(c.esp, 0x10))?;
                    push(c, g, c.edi)?;
                    c.edi = g.u32(at(c.esp, 0x24))?;
                    return g.call(c, 0x1ce110, 0x1cd5ec);
                }
            }
        }
        0x1cd65a => {
            sample_args(c, g)?;
            c.ebx = c.eax;
            g.set_u32(at(c.esp, 0x20), sse::lane0(c, 0))?;
            return g.call(c, SAMPLE, 0x1cd673);
        }
        0x1cd673 => {
            c.eax = g.u32(at(c.esp, 0x20))?;
            sse::load(c, 0, g.u32(at(c.esp, 0x10))?);
            sse::ss(c, Ss::Mul, 0, g.u32(at(c.esp, 0x18))?);
            for v in [c.eax, c.esi, c.ebp, c.ecx] {
                push(c, g, v)?;
            }
            c.ebx = c.edi;
            g.set_u32(c.esp, sse::lane0(c, 0))?;
            return g.call(c, SAMPLE, 0x1cd693);
        }
        0x1cd5ec | 0x1cd693 | 0x1cd6ac => {}
        _ => return Err(bad(pc)),
    }
    c.edi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 0x1c)
}

fn motion_call(c: &mut CpuAbi, g: &mut Bus<'_>, site: u32) -> sim_native::Result {
    push(c, g, c.ebp)?;
    c.edi = c.esi;
    g.call(c, MOTION, site)
}

fn default_motion(
    c: &mut CpuAbi,
    g: &mut Bus<'_>,
    object_suppressed: bool,
) -> Result<(), GuestFault> {
    // The two exits use different source registers; both reload the vector
    // one word at a time so an overlapping output is observable.
    if object_suppressed {
        c.edx = g.u32(ZERO_VECTOR)?;
        c.eax = g.u32(c.edx)?;
        sse::zero(c, 0);
        g.set_u32(c.esi, c.eax)?;
        c.ecx = g.u32(at(c.edx, 4))?;
        g.set_u32(at(c.esi, 4), c.ecx)?;
        c.edx = g.u32(at(c.edx, 8))?;
        g.set_u32(at(c.esi, 8), c.edx)?;
    } else {
        c.ecx = g.u32(ZERO_VECTOR)?;
        c.edx = g.u32(c.ecx)?;
        sse::zero(c, 0);
        g.set_u32(c.esi, c.edx)?;
        c.eax = g.u32(at(c.ecx, 4))?;
        g.set_u32(at(c.esi, 4), c.eax)?;
        c.ecx = g.u32(at(c.ecx, 8))?;
        g.set_u32(at(c.esi, 8), c.ecx)?;
        c.edi = pop(c, g)?;
    }
    g.set_u32(c.ebp, sse::lane0(c, 0))?;
    Ok(())
}

fn channel_motion(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cd6c0 => {
            c.esp = arithmetic(c, c.esp, 0x20, true);
            c.edx = u32::MAX;
            let missing = cmp(c, c.eax, c.edx, 32);
            push(c, g, c.ebp)?;
            c.ebp = g.u32(at(c.esp, 0x28))?;
            let suppressed = if missing {
                false
            } else {
                c.ecx = g.u32(guest_layout::points::OBJECTS.base)?;
                c.ecx = g.u32(at(c.ecx, 0x44))?;
                c.eax &= 0xffff;
                logic(c, c.eax, 1 << 31);
                c.eax = c.eax.wrapping_mul(3);
                c.eax = g.u32(at(c.ecx, c.eax.wrapping_mul(4).wrapping_add(8)))?;
                c.ecx = byte(c.ecx, g.u8(at(c.eax, 0xb3))?);
                test_byte(c, c.ecx as u8)
            };
            if !suppressed {
                c.eax = byte(c.eax, g.u8(OVERRIDE_ENABLED)?);
                let has_override = test_byte(c, c.eax as u8);
                push(c, g, c.edi)?;
                if has_override {
                    c.eax = g.u32(at(c.ebx, 0x40))?;
                    let present = !cmp(c, c.eax, c.edx, 32);
                    c.ecx = at(c.ebx, 0x40);
                    if present && clip_present(c, g, c.ecx, c.edx)? {
                        return motion_call(c, g, 0x1cd714);
                    }
                }
                c.eax = byte(c.eax, g.u8(SECONDARY_ENABLED)?);
                if test_byte(c, c.eax as u8) {
                    c.eax = g.u32(at(c.ebx, 0x20))?;
                    let present = !cmp(c, c.eax, c.edx, 32);
                    sse::load(c, 0, g.u32(FULL_WEIGHT)?);
                    c.ecx = at(c.ebx, 0x20);
                    if present {
                        match secondary_mix(c, g, c.ecx, c.ebx, c.edx)? {
                            Mix::Secondary => return motion_call(c, g, 0x1cd714),
                            Mix::Both => {
                                c.eax = c.ebx;
                                return g.call(c, 0x1cdf00, 0x1cd791);
                            }
                            Mix::Primary => {}
                        }
                    }
                }
                if valid(c, g, c.ebx, c.edx)? {
                    c.ecx = c.ebx;
                    return motion_call(c, g, 0x1cd84a);
                }
            }
            default_motion(c, g, suppressed)?;
        }
        0x1cd791 => {
            c.edx = at(c.esp, 8);
            push(c, g, c.edx)?;
            c.edi = at(c.esp, 0x14);
            g.set_u32(at(c.esp, 0x30), sse::lane0(c, 0))?;
            return g.call(c, MOTION, 0x1cd7a5);
        }
        0x1cd7a5 => {
            c.eax = at(c.esp, 0xc);
            push(c, g, c.eax)?;
            c.edi = at(c.esp, 0x20);
            c.ecx = c.ebx;
            return g.call(c, MOTION, 0x1cd7b5);
        }
        0x1cd7b5 => {
            sse::load(c, 1, g.u32(at(c.esp, 0x1c))?);
            sse::load(c, 0, g.u32(at(c.esp, 0x2c))?);
            for (base, next, dest) in [
                (0x10, 0x20, c.esi),
                (0x14, 0x24, at(c.esi, 4)),
                (0x18, 0xc, at(c.esi, 8)),
            ] {
                sse::load(c, 2, g.u32(at(c.esp, base))?);
                sse::ss_reg(c, Ss::Sub, 2, 1);
                sse::ss_reg(c, Ss::Mul, 2, 0);
                sse::ss_reg(c, Ss::Add, 2, 1);
                sse::load(c, 1, g.u32(at(c.esp, next))?);
                g.set_u32(dest, sse::lane0(c, 2))?;
            }
            sse::load(c, 2, g.u32(at(c.esp, 8))?);
            sse::ss_reg(c, Ss::Sub, 2, 1);
            sse::ss_reg(c, Ss::Mul, 2, 0);
            sse::ss_reg(c, Ss::Add, 2, 1);
            c.edi = pop(c, g)?;
            g.set_u32(c.ebp, sse::lane0(c, 2))?;
        }
        0x1cd714 | 0x1cd84a => {
            c.edi = pop(c, g)?;
        }
        _ => return Err(bad(pc)),
    }
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x20, false);
    g.ret(c, 4)
}

fn spill_rate(
    c: &mut CpuAbi,
    g: &mut Bus<'_>,
    offset: u32,
    store_pc: u32,
    scale: bool,
) -> Result<(), GuestFault> {
    let dest = at(c.esp, offset);
    let mut x = load_fpu(c);
    if scale {
        x.fmul_m(g.u32(0x45dcfc)?).map_err(fp_error)?;
    }
    g.set_u32(dest, x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c, g, x, store_pc, dest)
}
fn measure_motion(c: &mut CpuAbi, g: &mut Bus<'_>, site: u32) -> sim_native::Result {
    c.edi = at(c.esp, 0xc);
    c.eax = at(c.esp, 0x24);
    c.edx = at(c.esp, 0x18);
    g.call(c, 0x20ab60, site)
}
fn rate_exit(
    c: &mut CpuAbi,
    g: &mut Bus<'_>,
    clear_weight: bool,
    pushed_bp: bool,
) -> sim_native::Result {
    if clear_weight {
        sse::zero(c, 0);
    }
    if pushed_bp {
        c.ebp = pop(c, g)?;
    }
    c.eax = byte(c.eax, c.ebx as u8);
    if clear_weight {
        g.set_u32(at(c.esi, 0x80), sse::lane0(c, 0))?;
    }
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x24, false);
    g.ret(c, 4)
}
fn apply_rate(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    sse::load(c, 4, g.u32(at(c.esi, 0x80))?);
    sse::load(c, 3, g.u32(0x45dc84)?);
    let above = sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 3)) & 0x41 == 0;
    sse::ss(c, Ss::Mul, 0, g.u32(0x45dc68)?);
    sse::ss_reg(c, Ss::Mul, 4, 3);
    sse::ss_reg(c, Ss::Add, 4, 0);
    g.set_u32(at(c.esi, 0x80), sse::lane0(c, 4))?;
    if above {
        g.set_u32(at(c.esp, 0x30), sse::lane0(c, 2))?;
        sse::movaps(c, 1, 2);
    }
    c.ebp = pop(c, g)?;
    g.set_u32(at(c.esi, 0x18), sse::lane0(c, 1))?;
    c.eax = byte(c.eax, 1);
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x24, false);
    g.ret(c, 4)
}
fn blend_rate(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cd8a0 => {
            c.esp = arithmetic(c, c.esp, 0x24, true);
            push(c, g, c.ebx)?;
            c.ebx = 0;
            if !cmp(c, c.eax, u32::MAX, 32) {
                c.ecx = g.u32(guest_layout::points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, 0x44))?;
                c.eax &= 0xffff;
                logic(c, c.eax, 1 << 31);
                c.eax = c.eax.wrapping_mul(3);
                c.eax = g.u32(at(c.edx, c.eax.wrapping_mul(4).wrapping_add(8)))?;
                if !cmp(c, u32::from(g.u8(at(c.eax, 0xb3))?), c.ebx, 8) {
                    return rate_exit(c, g, false, false);
                }
            }
            if test_byte(c, g.u8(at(c.esi, 0x6e))? & 1) {
                return rate_exit(c, g, false, false);
            }
            let disabled = cmp(c, u32::from(g.u8(SECONDARY_ENABLED)?), c.ebx, 8);
            push(c, g, c.ebp)?;
            if disabled || !valid(c, g, c.esi, u32::MAX)? {
                return rate_exit(c, g, true, true);
            }
            c.ecx = c.esi;
            g.call(c, LOOKUP, 0x1cd8fe)
        }
        0x1cd8fe => {
            if cmp(c, c.eax, c.ebx, 32) || cmp(c, u32::from(g.u8(at(c.eax, 0x11))?), c.ebx, 8) {
                return rate_exit(c, g, true, true);
            }
            c.ebp = at(c.esi, 0x20);
            c.eax = c.ebp;
            g.call(c, 0xb6760, 0x1cd919)
        }
        0x1cd919 => {
            if !test_byte(c, c.eax as u8) {
                return rate_exit(c, g, true, true);
            }
            c.ecx = c.ebp;
            g.call(c, LOOKUP, 0x1cd928)
        }
        0x1cd928 => {
            logic(c, c.eax, 1 << 31);
            if c.eax == 0 {
                return rate_exit(c, g, true, true);
            }
            c.ecx = c.ebp;
            g.call(c, LOOKUP, 0x1cd937)
        }
        0x1cd937 => {
            let frames = g.u16(at(c.eax, 0x14))?;
            cmp(c, u32::from(frames), c.ebx, 16);
            if (frames as i16) <= c.ebx as i16 {
                return rate_exit(c, g, true, true);
            }
            push(c, g, c.edi)?;
            c.edi = at(c.esp, 0x18);
            c.eax = c.esi;
            g.set_u32(at(c.esp, 0x18), c.ebx)?;
            g.set_u32(at(c.esp, 0x1c), c.ebx)?;
            g.set_u8(at(c.esp, 0x20), c.ebx as u8)?;
            g.set_u8(at(c.esp, 0x21), c.ebx as u8)?;
            g.call(c, 0x1c7380, 0x1cd95d)
        }
        0x1cd95d => {
            c.eax = g.u32(at(c.esp, 0x34))?;
            g.call(c, 0x24590, 0x1cd966)
        }
        0x1cd966 => {
            spill_rate(c, g, 0x14, 0x1cd966, false)?;
            measure_motion(c, g, 0x1cd97b)
        }
        0x1cd97b => g.call(c, 0x24590, 0x1cd980),
        0x1cd980 => {
            sse::load(c, 0, g.u32(ONE)?);
            c.edi = at(c.esp, 0x18);
            c.eax = c.ebp;
            spill_rate(c, g, 0x10, 0x1cd994, true)?;
            g.set_u32(at(c.esp, 0x34), sse::lane0(c, 0))?;
            g.call(c, 0x1c7380, 0x1cd9a3)
        }
        0x1cd9a3 => measure_motion(c, g, 0x1cd9b4),
        0x1cd9b4 => g.call(c, 0x24590, 0x1cd9b9),
        0x1cd9b9 => {
            sse::load(c, 1, g.u32(at(c.esp, 0x10))?);
            sse::zero(c, 0);
            spill_rate(c, g, 0xc, 0x1cd9c8, true)?;
            sse::load(c, 3, g.u32(at(c.esp, 0xc))?);
            sse::ss_reg(c, Ss::Sub, 1, 3);
            let above = sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0)) & 0x41 == 0;
            c.edi = pop(c, g)?;
            if !above {
                sse::load(c, 2, g.u32(ONE)?);
                sse::load(c, 1, g.u32(at(c.esp, 0x30))?);
                return apply_rate(c, g);
            }
            sse::load(c, 2, g.u32(at(c.esp, 0x10))?);
            sse::ss_reg(c, Ss::Sub, 2, 3);
            sse::ss_reg(c, Ss::Div, 2, 1);
            let below_zero = sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2)) & 0x41 == 0;
            g.set_u32(at(c.esp, 8), sse::lane0(c, 2))?;
            if below_zero || {
                sse::load(c, 0, g.u32(ONE)?);
                sse::comiss(c, sse::lane0(c, 2), sse::lane0(c, 0)) & 0x41 == 0
            } {
                g.set_u32(at(c.esp, 8), sse::lane0(c, 0))?;
            }
            c.ecx = c.esi;
            g.call(c, LOOKUP, 0x1cda13)
        }
        0x1cda13 => {
            c.ecx = g.u16(at(c.eax, 0x14))? as i16 as u32;
            sse::cvtsi2ss(c, 0, c.ecx);
            c.ecx = c.ebp;
            g.set_u32(at(c.esp, 0x10), sse::lane0(c, 0))?;
            g.call(c, LOOKUP, 0x1cda28)
        }
        0x1cda28 => {
            c.edx = g.u16(at(c.eax, 0x14))? as i16 as u32;
            sse::load(c, 0, g.u32(at(c.esp, 0x10))?);
            sse::load(c, 2, g.u32(ONE)?);
            sse::load(c, 3, g.u32(at(c.esp, 8))?);
            sse::cvtsi2ss(c, 1, c.edx);
            sse::ss_reg(c, Ss::Div, 0, 1);
            sse::movaps(c, 1, 2);
            sse::ss_reg(c, Ss::Sub, 1, 0);
            sse::ss_reg(c, Ss::Mul, 1, 3);
            sse::ss_reg(c, Ss::Add, 1, 0);
            sse::movaps(c, 0, 2);
            sse::ss_reg(c, Ss::Sub, 0, 3);
            sse::zero(c, 3);
            if sse::comiss(c, sse::lane0(c, 3), sse::lane0(c, 0)) & 0x41 == 0 {
                sse::movaps(c, 0, 3);
            } else if sse::comiss(c, sse::lane0(c, 0), sse::lane0(c, 2)) & 0x41 == 0 {
                sse::movaps(c, 0, 2);
            }
            if sse::comiss(c, sse::lane0(c, 3), sse::lane0(c, 1)) & 0x41 == 0 {
                g.set_u32(at(c.esp, 0x30), sse::lane0(c, 3))?;
                sse::load(c, 1, g.u32(at(c.esp, 0x30))?);
            } else if sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 2)) & 0x41 == 0 {
                g.set_u32(at(c.esp, 0x30), sse::lane0(c, 2))?;
                sse::load(c, 1, g.u32(at(c.esp, 0x30))?);
            }
            apply_rate(c, g)
        }
        _ => Err(bad(pc)),
    }
}
