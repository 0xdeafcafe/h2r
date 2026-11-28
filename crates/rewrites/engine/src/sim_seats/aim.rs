//! Per-shot and per-user aim assist. The input search and barrel both ask
//! for the same cone. Zoom and weapon disposition are guest calls, so each
//! calculation reads the weapon and output again after those calls return.
use crate::function_eval::constant::ONE;
use guest_layout::points;
use crate::sim_native::{Memory, Result, sub_flags};
use crate::sim_objects::{alu::cmp16, sse::{self, Ss}};
use crate::sim_physics::{byte, logic, pop, push};
use runtime::{CpuAbi, GuestFault};

pub mod arm;
mod shot;
pub use shot::{SHOT_ENTRY, SHOT_SITES, shot};

const PRIMARY_ZOOM: u32 = 0x001a_5137;
const AFTER_TRACKING: u32 = 0x001a_51c6;
const AFTER_ACQUISITION: u32 = 0x001a_51ea;
const SECONDARY_ZOOM: u32 = 0x001a_527f;
pub const SITES: &[u32] = &[PRIMARY_ZOOM, AFTER_TRACKING, AFTER_ACQUISITION, SECONDARY_ZOOM];

const ZOOM: u32 = 0x0010_1090;
const TRACKING: u32 = 0x0010_11d0;
const ACQUISITION: u32 = 0x0010_1160;
const AUTO_ANGLE: u32 = 0x208;
const AUTO_RANGE: u32 = 0x20c;
const MAGNET_ANGLE: u32 = 0x210;
const MAGNET_RANGE: u32 = 0x214;
const BEND_ANGLE: u32 = 0x218;
const WEAPON_CLASS: u32 = 0x294;

fn at(base: u32, delta: u32) -> u32 { base.wrapping_add(delta) }

fn slot_index(datum: u32) -> u32 { (datum & 0xffff) * 12 }

fn low_word(reg: u32, value: u16) -> u32 { (reg & 0xffff_0000) | u32::from(value) }

fn signed_slot(g: &impl Memory, address: u32) -> std::result::Result<u16, GuestFault> {
    Ok(g.u8(address)? as i8 as i16 as u16)
}

fn weapon_slot(g: &impl Memory, unit: u32, index: u32) -> std::result::Result<u32, GuestFault> {
    g.u32(at(at(unit, 0x218), index.wrapping_mul(4)))
}

fn weapon_definition(c: &mut CpuAbi, g: &impl Memory, headers: u32, weapon: u32) -> std::result::Result<(), GuestFault> {
    let object = g.u32(at(headers, slot_index(weapon) + 8))?;
    c.edx = (g.u32(object)? & 0xffff) << 4;
    c.ecx = g.u32(points::TAG_ENTRIES.base)?;
    c.edi = g.u32(at(at(c.edx, c.ecx), 8))?;
    Ok(())
}

fn leave(c: &mut CpuAbi, g: &impl Memory) -> Result {
    c.ebx = pop(c, g)?;
    c.edi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 4)
}

/// Compose angle and range with the magnification that the weapon returned.
/// Keep the reciprocal as its own binary32 result for both angles.
struct Zoom { magnification: u32, inverse: u32 }

impl Zoom {
    fn read(c: &mut CpuAbi, g: &impl Memory) -> std::result::Result<Self, GuestFault> {
        let magnification = sse::lane0(c, 0);
        sse::load(c, 1, g.u32(ONE)?);
        let inverse = sse::ss(c, Ss::Div, 1, magnification);
        Ok(Self { magnification, inverse })
    }

    fn scaled(c: &mut CpuAbi, g: &impl Memory, field: u32, scale: u32, reg: usize) -> std::result::Result<u32, GuestFault> {
        sse::load(c, reg, g.u32(at(c.edi, field))?);
        Ok(sse::ss(c, Ss::Mul, reg, scale))
    }

    fn bend(&self, c: &mut CpuAbi, g: &impl Memory) -> std::result::Result<u32, GuestFault> {
        let bend = g.u32(at(c.edi, BEND_ANGLE))?;
        let cone = g.u32(at(c.edi, AUTO_ANGLE))?;
        let greater = sse::comiss(c, bend, cone) & 0x41 == 0;
        sse::load(c, 0, if greater { bend } else { cone });
        Ok(sse::ss(c, Ss::Mul, 0, self.inverse))
    }
}

fn primary(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let zoom = Zoom::read(c, g)?;
    for (field, scale, offset) in [
        (AUTO_ANGLE, zoom.inverse, 0), (AUTO_RANGE, zoom.magnification, 4),
        (MAGNET_ANGLE, zoom.inverse, 8), (MAGNET_RANGE, zoom.magnification, 12),
    ] {
        let word = Zoom::scaled(c, g, field, scale, 2)?;
        g.set_u32(at(c.esi, offset), word)?;
    }
    let bend = zoom.bend(c, g)?;
    g.set_u32(at(c.esi, 16), bend)?;
    g.set_u32(at(c.esp, 12), bend)?;
    sse::zero(c, 0);
    g.set_u32(at(c.esi, 20), 0)?;
    g.set_u32(at(c.esi, 32), 0)?;
    c.eax = c.ebx;
    g.call(c, TRACKING, AFTER_TRACKING)
}

fn next_hand(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.edx = g.u32(points::OBJECTS.base)?;
    c.eax = g.u32(at(c.edx, 0x44))?;
    c.ebp = g.u32(at(at(c.eax, c.ebp), 8))?;
    let index = signed_slot(g, at(c.ebp, 0x213))?;
    c.eax = low_word(c.eax, index);
    cmp16(c, u32::from(index), 0xffff);
    if index != u16::MAX {
        c.ecx = index as i16 as i32 as u32;
        c.eax = weapon_slot(g, c.ebp, c.ecx)?;
        sub_flags(c, c.eax, u32::MAX, 32);
        if c.eax != u32::MAX {
            let headers = g.u32(at(c.edx, 0x44))?;
            weapon_definition(c, g, headers, c.eax)?;
            c.ecx = g.u32(at(c.esp, 20))?;
            // SHL EDX,4 precedes the call. EDX holds the zero-extended
            // 16-bit tag index scaled by 16; neither carry nor overflow survives.
            let index = c.edx >> 4;
            crate::sim_objects::alu::shl(c, index, 4, 32);
            return g.call(c, ZOOM, SECONDARY_ZOOM);
        }
    }
    let empty = sse::lane0(c, 0);
    g.set_u32(at(c.esi, 24), empty)?;
    g.set_u32(at(c.esi, 28), empty)?;
    c.eax = byte(c.eax, 1);
    leave(c, g)
}

fn secondary(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.edx = g.u32(at(c.edi, AUTO_ANGLE))?;
    sse::load(c, 3, g.u32(at(c.esi, 8))?);
    g.set_u32(at(c.esi, 24), c.edx)?;
    c.eax = g.u32(at(c.edi, AUTO_RANGE))?;
    let zoom = Zoom::read(c, g)?;
    sse::movaps(c, 2, 1);
    g.set_u32(at(c.esi, 28), c.eax)?;
    let angle = Zoom::scaled(c, g, MAGNET_ANGLE, zoom.inverse, 1)?;
    let first = sse::lane0(c, 3);
    if sse::comiss(c, first, angle) & 0x41 == 0 { sse::movaps(c, 1, 3); }
    g.set_u32(at(c.esi, 8), sse::lane0(c, 1))?;
    let range = Zoom::scaled(c, g, MAGNET_RANGE, zoom.magnification, 1)?;
    let first = g.u32(at(c.esi, 12))?;
    sse::load(c, 0, first);
    if sse::comiss(c, first, range) & 0x41 == 0 { sse::movaps(c, 1, 0); }
    g.set_u32(at(c.esi, 12), sse::lane0(c, 1))?;
    let candidate = zoom.bend(c, g)?;
    let first = g.u32(at(c.esp, 12))?;
    sse::load(c, 1, first);
    if sse::comiss(c, first, candidate) & 0x41 == 0 {
        sse::movaps(c, 0, 1);
    } else {
        zoom.bend(c, g)?;
    }
    g.set_u32(at(c.esi, 16), sse::lane0(c, 0))?;
    c.eax = byte(c.eax, 1);
    leave(c, g)
}

/// Held-weapon cone at AIM_ASSIST_PARAMS or a return from its children.
pub fn params(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Result {
    match pc {
        PRIMARY_ZOOM => return primary(c, g),
        SECONDARY_ZOOM => return secondary(c, g),
        AFTER_TRACKING | AFTER_ACQUISITION => {
            logic(c, c.eax & 0xff, 0x80);
            if c.eax as u8 == 0 {
                if pc == AFTER_TRACKING {
                    c.eax = c.ebx;
                    return g.call(c, ACQUISITION, AFTER_ACQUISITION);
                }
            } else {
                let class = g.u16(at(c.edi, WEAPON_CLASS))?;
                cmp16(c, u32::from(class), 1);
                let flag = if pc == AFTER_TRACKING {
                    (class == 1).then_some(32)
                } else {
                    c.edi = low_word(c.edi, class);
                    if class == 1 { Some(8) } else {
                        cmp16(c, u32::from(class), 2);
                        (class == 2).then_some(16)
                    }
                };
                if let Some(flag) = flag {
                    let range = g.u32(at(c.esi, 4))?;
                    g.set_u32(at(c.esi, 20), flag)?;
                    g.set_u32(at(c.esi, 32), range)?;
                }
            }
            return next_hand(c, g);
        }
        _ => debug_assert_eq!(pc, points::AIM_ASSIST_PARAMS.base),
    }
    push(c, g, c.ecx)?;
    c.eax = byte(c.eax, 0);
    sub_flags(c, c.ecx, u32::MAX, 32);
    if c.ecx == u32::MAX {
        c.ecx = pop(c, g)?;
        return g.ret(c, 4);
    }
    push(c, g, c.ebp)?;
    c.ebp = slot_index(c.ecx);
    push(c, g, c.edi)?;
    c.edi = g.u32(points::OBJECTS.base)?;
    let headers = g.u32(at(c.edi, 0x44))?;
    c.ecx = g.u32(at(at(headers, c.ebp), 8))?;
    let index = signed_slot(g, at(c.ecx, 0x212))?;
    c.edx = low_word(c.edx, index);
    cmp16(c, c.edx, 0xffff);
    if index == u16::MAX {
        c.edi = pop(c, g)?;
        c.ebp = pop(c, g)?;
        c.ecx = pop(c, g)?;
        return g.ret(c, 4);
    }
    c.edx = index as i16 as i32 as u32;
    push(c, g, c.ebx)?;
    c.ebx = weapon_slot(g, c.ecx, c.edx)?;
    sub_flags(c, c.ebx, u32::MAX, 32);
    if c.ebx == u32::MAX { return leave(c, g); }
    let headers = g.u32(at(c.edi, 0x44))?;
    weapon_definition(c, g, headers, c.ebx)?;
    c.ecx = g.u32(at(c.esp, 20))?;
    cmp16(c, c.ecx, 0xffff);
    if c.ecx as u16 == u16::MAX {
        c.edx = g.u32(at(c.edi, 0x12c))? >> 5;
        logic(c, c.edx & 1, 0x80);
        if c.edx & 1 != 0 { return leave(c, g); }
    }
    c.eax = c.ebx;
    g.call(c, ZOOM, PRIMARY_ZOOM)
}
