//! Assemble one channel's spatial inputs. Source and listener transforms
//! feed the distance/cone evaluator. Routing puts the gain on the common
//! channel, the submix, or both during a fade. A game callback can edit
//! the complete descriptor and explicitly accept its changes.

use crate::points;
use crate::sim_native::ray::{finish_fpu, load_fpu};
use crate::sim_objects::{Body, Cx, Next, Result, alu, frame::{pop, push}, sse::{self, Ss}};
use crate::sim_physics::{arithmetic, byte, logic};
use runtime::{CpuAbi, Guest};
use sim_sound::spatial::{SourceSpace, transition_start};
use x87::abi as fp;

mod attenuation;

const ENTRY: u32 = points::SOUND_CHANNEL_SPATIAL.base;
const SITES: &[u32] = &[
    0x129b3e, 0x129b4c, 0x129b57, 0x129b6b, 0x129c02, 0x129c26,
    0x129c36, 0x129c6a, 0x129c8b, 0x129c99, 0x129d84, 0x129e0e,
    0x129e24, 0x129e35, 0x129e67, 0x129ebe, 0x129f05,
];

pub const BODIES: &[Body] = &[Body {
    point: &points::SOUND_CHANNEL_SPATIAL,
    sites: SITES,
    run: update,
}, attenuation::RAMP_BODY, attenuation::DISTANCE_BODY, attenuation::CONE_BODY];

const CHANNELS: u32 = points::SOUND_CHANNELS.base;
const INSTANCES: u32 = points::SOUND_INSTANCES.base;
const SOUND: u32 = points::SOUND_GLOBALS.base;
const TAGS: u32 = points::TAG_ENTRIES.base;
const DEFAULT_DIRECTION: u32 = 0x4687a4;
const DEFAULT_POSITION: u32 = 0x468788;
const DEFAULT_GAIN: u32 = 0x45dbc0;
const OPEN_ROUTE_GAIN: u32 = 0x440c48;
const CLOSED_ROUTE_GAIN: u32 = 0x440c4c;

// Image operands that parameterize the channel fade.
const FADE_MILLISECONDS: u32 = 0x129da8;
const FADE_FLOOR: u32 = 0x129dd1;
const FADE_FORWARD: u32 = 0x129df7;
const FADE_REVERSE: u32 = 0x129e11;

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

fn fstp(c: &mut CpuAbi, g: &mut Guest, pc: u32, out: u32) -> Result<()> {
    let mut fpu = load_fpu(c);
    let (_, word) = fp::st_f32(&fpu, 0);
    g.set_u32(out, word.value)?;
    fp::store_commit(&mut fpu, word.status, true);
    finish_fpu(c, g, &fpu, pc, out)
}

// Project direction in Z, Y, X multiply/add order. The third row
// consumes the last live copies of Y and Z in the guest registers.
fn project_direction(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    sse::load(c, 1, g.u32(c.eax.wrapping_add(4))?);
    sse::load(c, 2, g.u32(c.eax.wrapping_add(8))?);
    sse::load(c, 0, g.u32(c.eax)?);
    for row in 0..3 {
        let axis = c.ebx.wrapping_add(4 + row * 12);
        let (y, x) = if row == 2 { (2, 1) } else { (4, 4) };
        sse::load(c, 3, g.u32(axis.wrapping_add(8))?);
        sse::ss_reg(c, Ss::Mul, 3, 2);
        sse::load(c, y, g.u32(axis.wrapping_add(4))?);
        sse::ss_reg(c, Ss::Mul, y, 1);
        sse::ss_reg(c, Ss::Add, 3, y);
        sse::load(c, x, g.u32(axis)?);
        sse::ss_reg(c, Ss::Mul, x, 0);
        sse::ss_reg(c, Ss::Add, 3, x);
        g.set_u32(c.esp.wrapping_add(0x14 + row * 4), sse::lane0(c, 3))?;
    }
    Ok(())
}

// The callback receives a private descriptor copy. Preserve forward and
// backwards REP ordering, including overlap and the final pointer values.
fn copy_descriptor(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let step = if c.eflags & 0x400 == 0 { 4 } else { 0u32.wrapping_sub(4) };
    c.ecx = 14;
    while c.ecx != 0 {
        let word = g.u32(c.esi)?;
        g.set_u32(c.edi, word)?;
        c.edi = c.edi.wrapping_add(step);
        c.esi = c.esi.wrapping_add(step);
        c.ecx -= 1;
    }
    Ok(())
}

fn update(cx: &mut Cx<'_>, mut pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    loop {
        let sp = c.esp;
        pc = match pc {
            ENTRY => {
                c.ecx = g.u32(CHANNELS)?;
                c.edx = g.u32(INSTANCES)?;
                c.esp = arithmetic(c, sp, 0x48, true);
                push(c, g, c.ebx)?;
                c.ebx = g.u32(c.edx.wrapping_add(0x44))?;
                push(c, g, c.ebp)?;
                c.eax = (c.eax as i16 as i32 as u32).wrapping_mul(9);
                let channel = c.ecx.wrapping_add(c.eax.wrapping_mul(4));
                c.ebp = g.u32(channel)?;
                push(c, g, c.esi)?;
                push(c, g, c.edi)?;
                c.edi = channel;
                c.eax = byte(c.eax, g.u8(channel.wrapping_add(10))?);
                c.ebp = c.ebx.wrapping_add((c.ebp & 0xffff) * crate::sound::SoundInstance::SIZE);
                g.set_u8(channel.wrapping_add(11), c.eax as u8)?;
                g.set_u8(channel.wrapping_add(10), 0)?;
                let space = SourceSpace::decode(g.u8(c.ebp.wrapping_add(0x1b))?);
                c.eax = byte(c.eax, space.kind() as u8);
                match space {
                    SourceSpace::Plain => 0x129ce6,
                    SourceSpace::Other(_) => 0x129d2c,
                    SourceSpace::World | SourceSpace::Listener => {
                        arithmetic(c, space.kind() as u32, 1, true);
                        c.esi = g.u8(c.ebp.wrapping_add(0xa1))? as i8 as i32 as u32;
                        c.edx = g.u32(SOUND)?;
                        c.ecx = c.esi.wrapping_mul(9);
                        c.ebx = c.edx.wrapping_add(c.ecx.wrapping_mul(8)).wrapping_add(0x88);
                        c.ecx = g.u32(DEFAULT_DIRECTION)?;
                        c.edx = g.u32(c.ecx)?;
                        g.set_u32(c.esp.wrapping_add(0x14), c.edx)?;
                        c.edx = g.u32(c.ecx.wrapping_add(4))?;
                        c.ecx = g.u32(c.ecx.wrapping_add(8))?;
                        g.set_u32(c.esp.wrapping_add(0x18), c.edx)?;
                        g.set_u32(c.esp.wrapping_add(0x1c), c.ecx)?;
                        if space == SourceSpace::World {
                            c.ecx = c.ebp.wrapping_add(0x18);
                            c.eax = c.esp.wrapping_add(0x20);
                            return call(0x12abe0, 0x129b3e);
                        }
                        0x129c04
                    }
                }
            }
            0x129b3e => {
                c.ebx = arithmetic(c, c.ebx, 8, false);
                c.edx = c.eax;
                c.esi = c.eax;
                c.ecx = c.ebx;
                return call(0x142700, 0x129b4c);
            }
            0x129b4c => {
                c.ecx = g.u32(c.ebp.wrapping_add(0xc))?;
                c.eax = c.ebp.wrapping_add(0x18);
                return call(0x125060, 0x129b57);
            }
            0x129b57 | 0x129c26 => {
                let world = pc == 0x129b57;
                let has_direction = c.eax & 0xff;
                logic(c, has_direction, 0x80);
                if has_direction != 0 {
                    c.ecx = g.u32(c.ebp.wrapping_add(0x30))?;
                    c.esi = sp.wrapping_add(0x14);
                    return call(0x188ea0, if world { 0x129b6b } else { 0x129c36 });
                }
                if world { 0x129bf4 } else { 0x129c36 }
            }
            0x129b6b => {
                project_direction(c, g)?;
                0x129bf4
            }
            0x129bf4 => {
                c.eax = c.ebp.wrapping_add(0x34);
                c.edx = sp.wrapping_add(0x30);
                c.ecx = c.ebx;
                return call(0x1427f0, 0x129c02);
            }
            0x129c02 => 0x129c4d,
            0x129c04 => {
                c.edx = c.ebp.wrapping_add(0x24);
                c.eax = g.u32(c.edx)?;
                c.ecx = g.u32(c.edx.wrapping_add(4))?;
                c.edx = g.u32(c.edx.wrapping_add(8))?;
                g.set_u32(sp.wrapping_add(0x20), c.eax)?;
                g.set_u32(sp.wrapping_add(0x24), c.ecx)?;
                c.ecx = g.u32(c.ebp.wrapping_add(0xc))?;
                c.eax = c.ebp.wrapping_add(0x18);
                g.set_u32(sp.wrapping_add(0x28), c.edx)?;
                return call(0x125060, 0x129c26);
            }
            0x129c36 => {
                c.eax = c.ebp.wrapping_add(0x34);
                c.ecx = g.u32(c.eax)?;
                c.edx = g.u32(c.eax.wrapping_add(4))?;
                c.eax = g.u32(c.eax.wrapping_add(8))?;
                g.set_u32(sp.wrapping_add(0x30), c.ecx)?;
                g.set_u32(sp.wrapping_add(0x34), c.edx)?;
                g.set_u32(sp.wrapping_add(0x38), c.eax)?;
                0x129c4d
            }
            0x129c4d => {
                let submix = g.u8(c.ebp.wrapping_add(6))? & 2;
                logic(c, submix.into(), 0x80);
                c.eax = g.u32(c.ebp.wrapping_add(0xc))?;
                c.esi = c.ebp.wrapping_add(0x18);
                let direction = sp.wrapping_add(0x14);
                if submix != 0 { c.ecx = direction; } else { c.edx = direction; }
                push(c, g, direction)?;
                push(c, g, c.eax)?;
                c.eax = sp.wrapping_add(0x20);
                c.ecx = c.esi;
                return call(0x12ad30, if submix != 0 { 0x129c6a } else { 0x129c8b });
            }
            0x129c6a => {
                c.ebx = g.u32(sp.wrapping_add(0x60))?;
                g.set_u32(c.ebx.wrapping_add(0x18), sse::lane0(c, 0))?;
                c.eax = byte(c.eax, g.u8(c.edi.wrapping_add(10))? | 2);
                0x129cb8
            }
            0x129c8b => {
                c.eax = g.u32(sp.wrapping_add(0x60))?;
                g.set_u32(c.eax.wrapping_add(0x18), sse::lane0(c, 0))?;
                return call(0x2197f0, 0x129c99);
            }
            0x129c99 => {
                c.ebx = g.u32(sp.wrapping_add(0x60))?;
                g.set_u32(sp.wrapping_add(0x10), c.eax)?;
                c.eax = g.u32(sp.wrapping_add(0x5c))?;
                sse::load(c, 0, g.u32(c.eax)?);
                let bits = sse::ss(c, Ss::Add, 0, g.u32(sp.wrapping_add(0x10))?);
                g.set_u32(c.eax, bits)?;
                c.eax = byte(c.eax, g.u8(c.edi.wrapping_add(10))? | 1);
                0x129cb8
            }
            0x129cb8 => {
                g.set_u8(c.edi.wrapping_add(10), c.eax as u8)?;
                c.ecx = u32::from(g.u16(c.esi)?) >> 11;
                logic(c, c.ecx & 1, 0x80);
                if c.ecx & 1 != 0 {
                    c.eax = byte(c.eax, c.eax as u8 | 1);
                    g.set_u8(c.edi.wrapping_add(10), c.eax as u8)?;
                }
                c.eax = g.u32(sp.wrapping_add(0x20))?;
                c.ecx = g.u32(sp.wrapping_add(0x24))?;
                c.edx = c.ebx.wrapping_add(0x1c);
                g.set_u32(c.edx, c.eax)?;
                c.eax = g.u32(sp.wrapping_add(0x28))?;
                g.set_u32(c.edx.wrapping_add(4), c.ecx)?;
                g.set_u32(c.edx.wrapping_add(8), c.eax)?;
                0x129d30
            }
            0x129ce6 => {
                let route = g.u8(c.edi.wrapping_add(10))? | 1;
                g.set_u8(c.edi.wrapping_add(10), route)?;
                let out = g.u32(sp.wrapping_add(0x60))?;
                if g.u8(c.ebp.wrapping_add(0x19))? & 1 != 0 {
                    c.ecx = out;
                    sse::zero(c, 0);
                } else {
                    c.edx = out;
                    sse::load(c, 0, g.u32(DEFAULT_GAIN)?);
                }
                g.set_u32(out.wrapping_add(0x18), sse::lane0(c, 0))?;
                c.eax = g.u32(sp.wrapping_add(0x60))?.wrapping_add(0x1c);
                c.ecx = g.u32(DEFAULT_POSITION)?;
                c.edx = g.u32(c.ecx)?;
                g.set_u32(c.eax, c.edx)?;
                c.edx = g.u32(c.ecx.wrapping_add(4))?;
                g.set_u32(c.eax.wrapping_add(4), c.edx)?;
                c.ecx = g.u32(c.ecx.wrapping_add(8))?;
                g.set_u32(c.eax.wrapping_add(8), c.ecx)?;
                0x129d2c
            }
            0x129d2c => {
                c.ebx = g.u32(sp.wrapping_add(0x60))?;
                0x129d30
            }
            0x129d30 => {
                let route = g.u8(c.edi.wrapping_add(10))?;
                sse::zero(c, 0);
                if route & 1 == 0 {
                    sse::load(c, 0, g.u32(DEFAULT_GAIN)?);
                }
                c.edx = g.u32(SOUND)?;
                g.set_u32(c.ebx.wrapping_add(0x14), sse::lane0(c, 0))?;
                c.eax = byte(c.eax, g.u8(c.edi.wrapping_add(11))?);
                c.esi = g.u32(c.edx.wrapping_add(0x84))?;
                if c.eax as u8 != 0 { 0x129d9f } else {
                    let route = g.u8(c.edi.wrapping_add(10))?;
                    c.esi = 0;
                    g.set_u32(c.edi.wrapping_add(0x14), 0)?;
                    let baseline = if route & 1 != 0 { OPEN_ROUTE_GAIN } else { CLOSED_ROUTE_GAIN };
                    c.eax = g.u32(baseline)?;
                    g.set_u32(c.ebx.wrapping_add(8), c.eax)?;
                    let submix = g.u8(c.edi.wrapping_add(10))? & 2;
                    logic(c, submix.into(), 0x80);
                    if submix != 0 {
                        sse::load(c, 0, g.u32(c.ebx.wrapping_add(0x18))?);
                        return call(0x2197f0, 0x129d84);
                    }
                    c.eax = CLOSED_ROUTE_GAIN;
                    c.ecx = g.u32(c.eax)?;
                    0x129e81
                }
            }
            0x129d84 => {
                g.set_u32(sp.wrapping_add(0x60), c.eax)?;
                c.eax = sp.wrapping_add(0x60);
                c.ecx = g.u32(c.eax)?;
                0x129e81
            }
            0x129d9f => {
                if c.eax as u8 != g.u8(c.edi.wrapping_add(10))? {
                    let start = g.u32(c.edi.wrapping_add(0x14))?;
                    let interval = g.u32(FADE_MILLISECONDS)?;
                    let next = transition_start(start as i32, c.esi as i32, interval as i32);
                    g.set_u32(c.edi.wrapping_add(0x14), next as u32)?;
                }
                c.ecx = g.u32(c.edi.wrapping_add(0x14))?;
                c.edx = 0;
                c.eax = g.u32(FADE_FLOOR)?;
                g.set_u32(sp.wrapping_add(0x60), c.edx)?;
                g.set_u32(sp.wrapping_add(0x5c), c.eax)?;
                if c.ecx as i32 > 0 {
                    c.esi = c.esi.wrapping_sub(c.ecx);
                    let limit = g.u32(0x129de3)?;
                    arithmetic(c, c.esi, limit, true);
                    g.set_u32(sp.wrapping_add(0x10), c.esi)?;
                    if c.esi as i32 > limit as i32 {
                        g.set_u32(c.edi.wrapping_add(0x14), 0)?;
                    } else {
                        let age = g.u32(sp.wrapping_add(0x10))? as i32;
                        let mut fpu = load_fpu(c);
                        fp::ld_int(&mut fpu, i64::from(age));
                        finish_fpu(c, g, &fpu, 0x129df2, sp.wrapping_add(0x10))?;
                        push(c, g, g.u32(FADE_FORWARD)?)?;
                        c.eax = 1;
                        fstp(c, g, 0x129e00, sp.wrapping_add(0x60))?;
                        c.esi = g.u32(sp.wrapping_add(0x60))?;
                        push(c, g, c.esi)?;
                        return call(0x12a6d0, 0x129e0e);
                    }
                }
                0x129e28
            }
            0x129e0e => {
                c.edx = c.eax;
                push(c, g, g.u32(FADE_REVERSE)?)?;
                push(c, g, c.esi)?;
                c.eax = 1;
                g.set_u32(sp.wrapping_add(0x60), c.edx)?;
                return call(0x12a6d0, 0x129e24);
            }
            0x129e24 => {
                g.set_u32(sp.wrapping_add(0x5c), c.eax)?;
                0x129e28
            }
            0x129e28 => {
                let submix = g.u8(c.edi.wrapping_add(10))? & 2;
                logic(c, submix.into(), 0x80);
                if submix != 0 { c.eax = c.edx; }
                push(c, g, c.eax)?;
                return call(0x2195f0, 0x129e35);
            }
            0x129e35 => {
                fstp(c, g, 0x129e35, c.ebx.wrapping_add(0x14))?;
                let route = g.u8(c.edi.wrapping_add(10))?;
                c.eax = byte(c.eax, route);
                g.set_u8(c.edi.wrapping_add(11), route)?;
                c.eax = g.u32(sp.wrapping_add(if route & 1 != 0 { 0x60 } else { 0x5c }))?;
                g.set_u32(c.ebx.wrapping_add(8), c.eax)?;
                let submix = g.u8(c.edi.wrapping_add(10))? & 2;
                logic(c, submix.into(), 0x80);
                c.edx = sp.wrapping_add(if submix != 0 { 0x60 } else { 0x5c });
                sse::load(c, 0, g.u32(c.ebx.wrapping_add(0x18))?);
                return call(0x2197f0, 0x129e67);
            }
            0x129e67 => {
                sse::load(c, 0, g.u32(c.edx)?);
                g.set_u32(sp.wrapping_add(0x10), c.eax)?;
                let bits = sse::ss(c, Ss::Add, 0, g.u32(sp.wrapping_add(0x10))?);
                g.set_u32(sp.wrapping_add(0x10), bits)?;
                c.ecx = g.u32(sp.wrapping_add(0x10))?;
                c.esi = 0;
                0x129e81
            }
            0x129e81 => {
                g.set_u32(c.ebx.wrapping_add(4), c.ecx)?;
                c.edx = g.u8(c.edi.wrapping_add(10))?.into();
                g.set_u32(c.ebx, c.edx)?;
                g.set_u32(c.ebx.wrapping_add(0xc), c.esi)?;
                g.set_u32(c.ebx.wrapping_add(0x10), c.esi)?;
                c.eax = u32::from(g.u16(c.ebp.wrapping_add(0x18))?) >> 11;
                if c.eax & 1 != 0 {
                    c.ecx = g.u32(c.ebp.wrapping_add(0xc))? & 0xffff;
                    c.edx = g.u32(TAGS)?;
                    c.ecx = alu::shl(c, c.ecx, 4, 32);
                    c.eax = g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(8))?;
                    let class = g.u8(c.eax.wrapping_add(2))? as i8 as i16 as u16;
                    c.ecx = (c.ecx & 0xffff_0000) | u32::from(class);
                    push(c, g, c.ecx)?;
                    return call(0x221810, 0x129ebe);
                }
                0x129ecd
            }
            0x129ebe => {
                sse::load(c, 0, g.u32(c.eax.wrapping_add(0x14))?);
                let gain = sse::ss(c, Ss::Add, 0, g.u32(c.ebx.wrapping_add(8))?);
                g.set_u32(c.ebx.wrapping_add(8), gain)?;
                0x129ecd
            }
            0x129ecd => {
                c.edx = byte(c.edx, g.u8(c.ebp.wrapping_add(4))? >> 5);
                if c.edx & 1 == 0 {
                    c.eax = g.u32(c.ebp.wrapping_add(0x14))?;
                    if c.eax != c.esi {
                        let slot = c.eax.wrapping_add(0xc);
                        let target = g.u32(slot)?;
                        arithmetic(c, target, c.esi, true);
                        if target != c.esi {
                            c.esi = c.ebx;
                            c.edi = sp.wrapping_add(0x20);
                            copy_descriptor(c, g)?;
                            c.ecx = sp.wrapping_add(0x20);
                            push(c, g, c.ecx)?;
                            c.ecx = g.u32(c.ebp.wrapping_add(0xc))?;
                            c.edx = c.ebp.wrapping_add(0x5c);
                            push(c, g, c.edx)?;
                            c.edx = g.u32(c.ebp.wrapping_add(0x10))?;
                            push(c, g, c.ecx)?;
                            push(c, g, c.edx)?;
                            return call(g.u32(slot)?, 0x129f05);
                        }
                    }
                }
                0x129f16
            }
            0x129f05 => {
                if c.eax as u8 != 0 {
                    c.esi = sp.wrapping_add(0x20);
                    c.edi = c.ebx;
                    copy_descriptor(c, g)?;
                }
                0x129f16
            }
            0x129f16 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x48, false);
                return Ok(Next::Ret { pop: 8 });
            }
            _ => unreachable!("unregistered channel spatial continuation {pc:#x}"),
        };
    }
}
