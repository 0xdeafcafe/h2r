//! Base object function imports. StringIDs, the PRNG parameters and the
//! normalized random scale belong to the loaded image. The constants here
//! identify operand locations, so the public source carries no retail IDs.
//! Never installed: `crate::sim_objects::functions::base` owns the entry;
//! this stays a test oracle.
use super::*;
use guest_layout::points::{OBJECTS, TAG_ENTRIES};
use crate::function_eval::constant::ONE;
use std::cmp::Ordering::{self, Equal, Greater, Less};
use x87::abi as fp;

const RANDOM: u32 = 0x000b_cc48;
const NAME_ONE: u32 = 0x000b_cc85;
const NAME_ZERO: u32 = 0x000b_cc90;
const OBJECT_FLAG: u32 = 0x000b_cc9b;
const FACING: u32 = 0x000b_cc78;
const MODEL_VARIANT: u32 = 0x000b_cd13;
const QUERY_A: u32 = 0x000b_cd1a;
const QUERY_B: u32 = 0x000b_cd21;
const QUERY_C: u32 = 0x000b_cc67;
const BODY: u32 = 0x000b_cdb3;
const EXTRA_ONE: u32 = 0x000b_cdbc;
const QUERY_DELTA: u32 = 0x000b_cdc7;
const COMPONENT: u32 = 0x000b_cdfc;
const SHIELD: u32 = 0x000b_ce07;
const OVERCHARGE: u32 = 0x000b_ce55;
const QUERY_D: u32 = 0x000b_ce66;
const SHIELD_EMPTY: u32 = 0x000b_ce77;
const UNIT_ONE: u32 = 0x000b_ce7e;
const SHIELD_RECENT: u32 = 0x000b_ce85;
const BODY_RECENT: u32 = 0x000b_cf04;
const COMPONENT_LO: u32 = 0x000b_cf0b;
const COMPONENT_HI: u32 = 0x000b_cf12;
const WORLD_C: u32 = 0x000b_cf46;
const COMPONENT_BASE: u32 = 0x000b_cf52;
const WORLD_DELTA: u32 = 0x000b_cf5a;
const WORLD_D: u32 = 0x000b_cf8a;
const LATE_COMPONENT_LO: u32 = 0x000b_cf91;
const LATE_COMPONENT_HI: u32 = 0x000b_cf98;

enum Source {
    Missing,
    Zero,
    One,
    ObjectFlag,
    Facing,
    ModelVariant,
    Random,
    Field(u32),
    Overcharge,
    ShieldEmpty,
    World(u32),
    Component,
    Callback(u32, u32),
}

fn selector(c: &mut CpuAbi, g: &impl Memory) -> std::result::Result<Source, GuestFault> {
    use Source::*;
    let name = c.eax;
    let cmp = |c: &mut CpuAbi, addr| -> std::result::Result<Ordering, GuestFault> {
        let key = g.u32(addr)?;
        sub_flags(c, name, key, 32);
        Ok((name as i32).cmp(&(key as i32)))
    };
    Ok(match cmp(c, RANDOM)? {
        Equal => Random,
        Less => match cmp(c, QUERY_C)? {
            Equal => Callback(0x000b_cad0, 0x000b_cdaa),
            Less => match cmp(c, FACING)? {
                Equal => Facing,
                Less if cmp(c, NAME_ONE)? == Equal => One,
                Less if cmp(c, NAME_ZERO)? == Equal => Zero,
                Less if cmp(c, OBJECT_FLAG)? == Equal => ObjectFlag,
                Greater if cmp(c, MODEL_VARIANT)? == Equal => ModelVariant,
                Greater if cmp(c, QUERY_A)? == Equal => Callback(0x000b_cbb0, 0x000b_cd3a),
                Greater if cmp(c, QUERY_B)? == Equal => Callback(0x000b_ca70, 0x000b_cd30),
                _ => Missing,
            },
            Greater => match cmp(c, BODY)? {
                Equal => Field(0xec),
                Greater if cmp(c, COMPONENT)? == Equal => Component,
                Greater if cmp(c, SHIELD)? == Equal => Field(0xf0),
                Greater => Missing,
                Less => {
                    let key = g.u32(EXTRA_ONE)?;
                    c.eax = name.wrapping_sub(key);
                    if c.eax == 0 {
                        One
                    } else {
                        let delta = g.u32(QUERY_DELTA)?;
                        sub_flags(c, c.eax, delta, 32);
                        c.eax = c.eax.wrapping_sub(delta);
                        match c.eax {
                            0 => Callback(0x000b_cb10, 0x000b_cde6),
                            1 => {
                                sub_flags(c, 1, 1, 32);
                                c.eax = 0;
                                Callback(0x000b_cb60, 0x000b_cdd9)
                            }
                            _ => Missing,
                        }
                    }
                }
            },
        },
        Greater => match cmp(c, OVERCHARGE)? {
            Equal => Overcharge,
            Less => match cmp(c, QUERY_D)? {
                Equal => Callback(0x000c_18e0, 0x000b_cefb),
                Less if cmp(c, SHIELD_EMPTY)? == Equal => ShieldEmpty,
                Less if cmp(c, UNIT_ONE)? == Equal => One,
                Less if cmp(c, SHIELD_RECENT)? == Equal => Field(0xf8),
                Greater if cmp(c, BODY_RECENT)? == Equal => Field(0xf4),
                Greater if cmp(c, COMPONENT_LO)? == Greater && cmp(c, COMPONENT_HI)? != Greater => {
                    Component
                }
                _ => Missing,
            },
            Greater => match cmp(c, WORLD_C)? {
                Equal => World(0x78),
                Less => {
                    let base = g.u32(COMPONENT_BASE)?;
                    sub_flags(c, name, base, 32);
                    c.edx = name.wrapping_sub(base);
                    if c.edx == 0 {
                        Component
                    } else {
                        c.edx = c.edx.wrapping_sub(g.u32(WORLD_DELTA)?);
                        if c.edx == 0 {
                            World(0x70)
                        } else {
                            c.edx = c.edx.wrapping_sub(1);
                            if c.edx == 0 { World(0x74) } else { Missing }
                        }
                    }
                }
                Greater if cmp(c, WORLD_D)? == Equal => World(0x7c),
                Greater
                    if cmp(c, LATE_COMPONENT_LO)? == Greater
                        && cmp(c, LATE_COMPONENT_HI)? != Greater =>
                {
                    Component
                }
                _ => Missing,
            },
        },
    })
}

fn random(c: &mut CpuAbi, g: &mut impl Memory) -> std::result::Result<u32, GuestFault> {
    const MULTIPLIER: u32 = 0x000b_ce20;
    const INCREMENT: u32 = 0x000b_ce26;
    const SCALE: u32 = 0x0045_dc50;
    const STORE: u32 = 0x000b_ce45;
    c.ecx = c
        .ecx
        .wrapping_mul(g.u32(MULTIPLIER)?)
        .wrapping_add(g.u32(INCREMENT)?)
        >> 16;
    g.set_u32(at(c.esp, 20), c.ecx)?;
    let mut x = ray::load_fpu(c);
    fp::ld_int(&mut x, c.ecx.into());
    fp::arith(
        &mut x,
        fp::arith_op::MUL,
        0,
        fp::src_kind::F32,
        g.u32(SCALE)?.into(),
    );
    let (_, s) = fp::st_f32(&x, 0);
    let addr = at(c.esp, 8);
    g.set_u32(addr, s.value)?;
    fp::store_commit(&mut x, s.status, true);
    ray::finish_fpu(c, g, &x, STORE, addr)?;
    g.u32(addr)
}

pub fn base_import(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    push(c, g, c.ecx)?;
    c.ecx = g.u32(at(c.esp, 8))?;
    c.edx = g.u32(at(g.u32(OBJECTS.base)?, 0x44))?;
    put_xmm(c, 2, scalar(0));
    for saved in [c.ebx, c.esi] {
        push(c, g, saved)?;
    }
    c.esi = g.u32(at(c.edx, object_slot(c.ecx) + 8))?;
    c.eax = g.u32(at(c.esp, 20))?;
    c.ebx &= !255;
    put_xmm(c, 0, scalar(0));
    g.set_u8(at(c.esp, 16), 1)?;
    g.set_u32(at(c.esp, 8), 0)?;
    let source = selector(c, g)?;
    let mut clamp = true;
    let mut value = match source {
        Source::Callback(target, resume) => return call(c, g, target, resume),
        Source::Facing => {
            push(c, g, 0)?;
            c.eax = c.ecx;
            return call(c, g, 0x000b_8bd0, 0x000b_ccc4);
        }
        Source::Component => {
            c.edx = at(c.esp, 8);
            push(c, g, c.edx)?;
            return call(c, g, 0x000b_c970, 0x000b_cfac);
        }
        Source::Missing => {
            g.set_u8(at(c.esp, 16), 0)?;
            clamp = false;
            0
        }
        Source::Zero => {
            clamp = false;
            0
        }
        Source::One => {
            clamp = false;
            g.u32(ONE)?
        }
        Source::ObjectFlag => {
            clamp = false;
            if g.u8(at(c.esi, 0x10a))? & 4 == 0 {
                g.u32(ONE)?
            } else {
                0
            }
        }
        Source::Overcharge => op(c, Sub, g.u32(at(c.esi, 0xf0))?, g.u32(ONE)?),
        Source::Field(offset) => g.u32(at(c.esi, offset))?,
        Source::World(offset) => {
            const WORLD: u32 = 0x004d_e2f4;
            let world = g.u32(WORLD)?;
            if offset == 0x70 {
                c.edx = world;
            }
            g.u32(at(world, offset))?
        }
        Source::ShieldEmpty => {
            const MODE: u32 = 0x004e_6948;
            c.edx = g.u32(MODE)?;
            if g.u32(at(c.edx, 8))? == 2 {
                c.ecx = low(c.ecx, g.u8(at(c.esi, 0xaa))?);
                c.eax = 1u32.wrapping_shl(c.ecx);
                if c.eax & 3 != 0 {
                    c.ecx = g.u32(at(c.esi, 0x13c))?;
                    sub_flags(c, c.ecx, u32::MAX, 32);
                    if c.ecx != u32::MAX {
                        push(c, g, 2)?;
                        return call(c, g, 0x0015_88b0, 0x000b_cecb);
                    }
                }
            }
            let shield = g.u32(at(c.esi, 0xf0))?;
            let f = simd::ucomiss(&mut c.mxcsr, shield, 0).expect("masked SSE exceptions");
            clamp = false;
            if f & 0x44 == 0x40 { g.u32(ONE)? } else { 0 }
        }
        Source::Random => random(c, g)?,
        Source::ModelVariant => {
            let tags = g.u32(TAG_ENTRIES.base)?;
            c.edx = tag_data(g, tags, g.u32(c.esi)?)?;
            let model = tag_data(g, tags, g.u32(at(c.edx, 0x38))?)?;
            let count = g.u32(at(model, 0x50))? as i32;
            if count <= 0 {
                clamp = false;
                0
            } else {
                let index = (i32::from(g.u8(at(c.esi, 0xb1))? as i8) + 1).clamp(0, count);
                let numerator =
                    simd::cvtsi2ss(&mut c.mxcsr, 0, index as u32).expect("masked SSE exceptions");
                let old = u128::from_le_bytes(c.xmm[1]);
                let denominator =
                    simd::cvtsi2ss(&mut c.mxcsr, old, count as u32).expect("masked SSE exceptions");
                c.xmm[1] = denominator.to_le_bytes();
                simd::divss(&mut c.mxcsr, numerator, denominator as u32)
                    .expect("masked SSE exceptions") as u32
            }
        }
    };
    if clamp {
        if compare(c, 0, value) == 0 {
            value = 0;
        } else {
            let one = g.u32(ONE)?;
            put_xmm(c, 1, scalar(one));
            if compare(c, value, one) == 0 {
                value = one;
            }
        }
    }
    put_xmm(c, 0, scalar(value));
    c.ecx = g.u32(at(c.esp, 24))?;
    c.esi = pop(c, g)?;
    g.set_u32(c.ecx, value)?;
    c.ebx = pop(c, g)?;
    let active = u8::from(compare(c, value, 0) == 0);
    c.ecx = g.u32(at(c.esp, 20))?;
    if active == 0 {
        logic_flags(c, 0);
    }
    g.set_u8(c.ecx, active)?;
    c.eax = u32::from(g.u8(at(c.esp, 8))?);
    c.ecx = pop(c, g)?;
    ret(g, c, 16)
}
