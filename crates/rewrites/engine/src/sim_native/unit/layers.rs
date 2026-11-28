//! Base and action layers, optional overlays, and the three aim channels.
//! Each evaluator sees the current guest frame, including the scratch
//! channel at frame + 0x48. Its result is consumed at its return site.

use super::*;
use guest_layout::points::{OBJECTS, TAG_ENTRIES};

const EVALUATE: u32 = 0x1c6f60;
const SELECT: u32 = 0x1c6470;
const RESOLVE: u32 = 0x1cb490;
const ZERO: u32 = 0x45dbd8;
const FINAL_LAYERS: u32 = 0x112c5e;

fn initialize(c: &mut CpuAbi, g: &mut impl Memory) -> std::result::Result<(), GuestFault> {
    c.esp = c.esp.wrapping_sub(0xd4);
    let slot = object_slot(g.u32(at(c.esp, 0xd8))?);
    let objects = g.u32(OBJECTS.base)?;
    let slots = g.u32(at(objects, 0x44))?;
    for saved in [c.ebx, c.ebp, c.esi, c.edi] {
        push(c, g, saved)?;
    }
    let sp = c.esp;
    c.edi = g.u32(at(slots, slot + 8))?;
    let definition = g.u32(c.edi)?;
    c.esi = at(c.edi, g.u16(at(c.edi, 0x12a))? as i16 as u32);
    let graph = g.u32(at(c.esi, 0x68))?;
    c.ebp = at(c.edi, g.u16(at(c.edi, 0x33e))? as i16 as u32);
    g.set_u32(at(sp, 0x6c), slot)?;
    let tags = g.u32(TAG_ENTRIES.base)?;
    let definition = tag_data(g, tags, definition)?;
    g.set_u32(at(sp, 0x24), definition)?;
    let model = g.u32(at(definition, 0x38))?;
    let graph = tag_data(g, tags, graph)?;
    g.set_u32(at(sp, 0x68), graph)?;
    let model = tag_data(g, tags, model)?;
    c.edx = tag_data(g, tags, g.u32(at(model, 4))?)?;
    c.ebx = u32::MAX;
    c.eax = at(c.ebp, 0x9c);
    c.ecx = g.u32(c.eax)?;
    sub_flags(c, c.ecx, c.ebx, 32);
    for offset in [0x58, 0x59] {
        g.set_u8(at(sp, offset), 0)?;
    }
    for offset in [0x5a, 0x5c, 0x5e] {
        g.set_u16(at(sp, offset), 0)?;
    }
    g.set_u32(at(sp, 0x64), 0)?;
    let one = g.u32(ONE)?;
    put_xmm(c, 0, scalar(one));
    for (offset, value) in [(0x2c, c.edi), (0x18, c.esi), (0x20, c.edx), (0x48, c.ebx)] {
        g.set_u32(at(sp, offset), value)?;
    }
    g.set_u16(at(sp, 0x4c), u16::MAX)?;
    g.set_u16(at(sp, 0x4e), u16::MAX)?;
    g.set_u32(at(sp, 0x60), one)?;
    g.set_u32(at(sp, 0x50), u32::MAX)?;
    g.set_u16(at(sp, 0x56), u16::MAX)?;
    g.set_u8(at(sp, 0x54), u8::MAX)?;
    g.set_u8(at(sp, 0x55), u8::MAX)
}

// The full-weight argument is an immediate operand at each callsite.
fn push_weight(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    operand: u32,
) -> std::result::Result<(), GuestFault> {
    let weight = g.u32(operand)?;
    push(c, g, weight)
}

fn index_present(
    c: &mut CpuAbi,
    g: &impl Memory,
    addr: u32,
) -> std::result::Result<bool, GuestFault> {
    let index = u32::from(g.u16(addr)?);
    sub_flags(c, index, c.ebx, 16);
    Ok(index != c.ebx & 0xffff)
}

fn base_layer(c: &mut CpuAbi, g: &mut impl Memory, first: bool) -> Segment {
    let skip = if first { 0x112197 } else { 0x1121d4 };
    if c.ecx == c.ebx || !index_present(c, g, at(c.eax, 6))? {
        return Ok(Step::Next(skip));
    }
    let target = g.u32(at(c.esp, 0xf4))?;
    let source = g.u32(at(c.esp, 0xf0))?;
    push(c, g, target)?;
    let nodes = g.u32(at(c.esp, 0xf0))?;
    push(c, g, source)?;
    push(c, g, nodes)?;
    let (operand, site) = if first {
        (c.ecx, c.edx) = (nodes, source);
        (0x112189, 0x112194)
    } else {
        (c.edx, c.ecx) = (nodes, source);
        (0x1121c6, 0x1121d1)
    };
    push_weight(c, g, operand)?;
    c.ebx = c.eax;
    invoke(c, g, EVALUATE, site)
}

fn full_weight_args(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    operand: u32,
) -> std::result::Result<(), GuestFault> {
    c.ecx = g.u32(at(c.esp, 0xf4))?;
    c.edx = g.u32(at(c.esp, 0xf0))?;
    c.eax = g.u32(at(c.esp, 0xec))?;
    for arg in [c.ecx, c.edx, c.eax] {
        push(c, g, arg)?;
    }
    push_weight(c, g, operand)
}

fn action_layer(c: &mut CpuAbi, g: &mut impl Memory) -> Segment {
    let active = c.eax as u8;
    test_byte(c, active);
    let mut weighted = false;
    if active != 0 {
        logic_flags(c, c.ebx);
        if c.ebx != 0 {
            let weight = g.u32(at(c.ebx, 0x1c))?;
            put_xmm(c, 0, scalar(weight));
            weighted = compare(c, weight, g.u32(ZERO)?) == 0;
        }
    }
    if weighted {
        c.eax = g.u32(at(c.esp, 0xf4))?;
        c.ecx = g.u32(at(c.esp, 0xf0))?;
        c.edx = g.u32(at(c.esp, 0xec))?;
        push(c, g, c.eax)?;
        c.eax = g.u32(at(c.ebx, 0x1c))?;
        for arg in [c.ecx, c.edx, c.eax] {
            push(c, g, arg)?;
        }
    } else {
        full_weight_args(c, g, 0x112250)?;
    }
    c.ebx = at(c.ebp, 0xdc);
    invoke(c, g, EVALUATE, 0x11225f)
}

fn overlay(c: &mut CpuAbi, g: &mut impl Memory, first: bool) -> Segment {
    let (offset, mask, resume, skip) = if first {
        (0x104, 1 << 10, 0x11228b, 0x1122b8)
    } else {
        (0x108, 1 << 11, 0x1122e1, 0x11230e)
    };
    if !index_present(c, g, at(c.ebp, offset + 2))? {
        return Ok(Step::Next(skip));
    }
    c.ecx = g.u32(at(c.esi, 0x68))?;
    sub_flags(c, c.ecx, c.ebx, 32);
    c.eax = g.u32(at(c.ebp, offset))?;
    if c.ecx == c.ebx {
        return Ok(Step::Next(skip));
    }
    for arg in [c.ebx, c.ebx, c.ebx, c.ebx, c.eax] {
        push(c, g, arg)?;
    }
    c.edx = mask;
    c.eax = at(c.esp, 0x5c);
    invoke(c, g, SELECT, resume)
}

fn aim_lookup(c: &mut CpuAbi, g: &mut impl Memory) -> Segment {
    g.set_u8(c.ebp, g.u8(c.ebp)? & !(3 << 5))?;
    c.ecx = g.u32(at(c.esp, 0x24))?;
    c.edx = g.u32(at(c.ecx, 0xbc))? >> 11;
    test_byte(c, (c.edx & 1) as u8);
    if c.edx & 1 != 0 {
        return Ok(Step::Next(FINAL_LAYERS));
    }
    c.eax = g.u32(at(c.esp, 0x18))?;
    let graph = g.u32(at(c.eax, 0x68))?;
    sub_flags(c, graph, c.ebx, 32);
    if graph == c.ebx {
        return Ok(Step::Next(FINAL_LAYERS));
    }
    let weight = g.u32(at(c.edi, 0x208))?;
    put_xmm(c, 0, scalar(weight));
    if compare(c, weight, g.u32(ZERO)?) == 0 {
        c.ecx = g.u32(OBJECTS.base)?;
        c.edx = g.u32(at(c.ecx, 0x44))?;
        let slot = g.u32(at(c.esp, 0x6c))?;
        c.eax = g.u32(at(c.edx, at(slot, 8)))?;
        c.ecx = g.u16(at(c.eax, 0x33e))? as i16 as u32;
        if !index_present(c, g, at(at(c.ecx, c.eax), 0x36))? {
            c.eax = g.u32(at(c.esp, 0x18))?;
            c.esi = at(c.esp, 0x1c);
            return invoke(c, g, 0x1cc150, 0x112372);
        }
    }
    Ok(Step::Next(0x1123bd))
}

fn set_none(c: &mut CpuAbi) {
    c.ebx = u32::MAX;
    logic_flags(c, c.ebx);
}

pub(super) fn run(c: &mut CpuAbi, g: &mut impl Memory, mut pc: u32) -> Segment {
    loop {
        let step = match pc {
            0x112070 => {
                initialize(c, g)?;
                base_layer(c, g, true)?
            }
            0x112194 | 0x112197 => {
                if pc == 0x112194 {
                    set_none(c);
                }
                c.eax = at(c.ebp, 0xbc);
                c.ecx = g.u32(c.eax)?;
                sub_flags(c, c.ecx, c.ebx, 32);
                base_layer(c, g, false)?
            }
            0x1121d1 | 0x1121d4 => {
                if pc == 0x1121d1 {
                    set_none(c);
                }
                let action = g.u32(at(c.ebp, 0xdc))?;
                sub_flags(c, action, c.ebx, 32);
                if action != c.ebx && index_present(c, g, at(c.ebp, 0xe2))? {
                    c.ecx = g.u32(at(c.esp, 0xe8))?;
                    return invoke(c, g, 0xc8fc0, 0x1121f5);
                }
                Step::Next(0x112262)
            }
            0x1121f5 => {
                c.ebx = c.eax;
                c.eax = g.u32(at(c.esp, 0xe8))?;
                return invoke(c, g, 0x10f890, 0x112203);
            }
            0x112203 => return action_layer(c, g),
            0x11225f | 0x112262 | 0x1122b5 | 0x1122b8 => {
                if matches!(pc, 0x11225f | 0x1122b5) {
                    set_none(c);
                }
                overlay(c, g, pc < 0x1122b5)?
            }
            0x11228b | 0x1122e1 => {
                let first = pc == 0x11228b;
                test_byte(c, c.eax as u8);
                if c.eax as u8 != 0 {
                    let (operand, site) = if first {
                        (0x1122a8, 0x1122b5)
                    } else {
                        (0x1122fe, 0x11230b)
                    };
                    full_weight_args(c, g, operand)?;
                    c.ebx = at(c.esp, 0x58);
                    return invoke(c, g, EVALUATE, site);
                }
                Step::Next(if first { 0x1122b8 } else { 0x11230e })
            }
            0x11230b | 0x11230e => {
                if pc == 0x11230b {
                    set_none(c);
                }
                aim_lookup(c, g)?
            }
            0x112372 => {
                if index_present(c, g, at(c.esp, 0x1e))? {
                    c.edx = g.u32(at(c.esp, 0x1c))?;
                    push(c, g, c.edx)?;
                    c.eax = at(c.esp, 0x4c);
                    push(c, g, c.eax)?;
                    c.eax = g.u32(at(c.esp, 0x20))?;
                    c.ebx = 0;
                    logic_flags(c, 0);
                    return invoke(c, g, RESOLVE, 0x11238e);
                }
                Step::Next(0x1123bd)
            }
            0x11238e => {
                test_byte(c, c.eax as u8);
                if c.eax as u8 != 0 {
                    c.ecx = g.u32(at(c.esp, 0xf4))?;
                    c.edx = g.u32(at(c.esp, 0xf0))?;
                    c.eax = g.u32(at(c.esp, 0xec))?;
                    push(c, g, c.ecx)?;
                    c.ecx = g.u32(at(c.edi, 0x208))?;
                    for arg in [c.edx, c.eax, c.ecx] {
                        push(c, g, arg)?;
                    }
                    c.ebx = at(c.esp, 0x58);
                    return invoke(c, g, EVALUATE, 0x1123ba);
                }
                Step::Next(0x1123ba)
            }
            0x1123ba | 0x1123bd => {
                if pc == 0x1123ba {
                    set_none(c);
                }
                c.edx = g.u32(at(c.esp, 0x18))?;
                let name = g.u32(at(c.edx, 0x7c))?;
                let excluded = g.u32(0x1123c4)?;
                sub_flags(c, name, excluded, 32);
                if name == excluded {
                    return Ok(Step::Next(FINAL_LAYERS));
                }
                c.eax = low(c.eax, g.u8(c.ebp)? >> 1);
                test_byte(c, (c.eax & 1) as u8);
                if c.eax & 1 != 0 {
                    c.esi = at(c.edi, 0x288);
                    c.edi = at(c.ebp, 0x118);
                    g.set_u32(at(c.esp, 0x1c), 3)?;
                    Step::Next(0x1123f0)
                } else {
                    Step::Next(0x1124a8)
                }
            }
            0x1123f0 => {
                c.eax = g.u32(c.edi)?;
                c.ecx = c.eax >> 16;
                sub_flags(c, c.ecx, c.ebx, 16);
                g.set_u32(at(c.esp, 0x20), c.eax)?;
                if c.ecx != c.ebx & 0xffff && aim_weight(c, g)? {
                    push(c, g, c.eax)?;
                    c.eax = g.u32(at(c.esp, 0x1c))?;
                    c.ecx = at(c.esp, 0x4c);
                    push(c, g, c.ecx)?;
                    c.ebx = 0;
                    logic_flags(c, 0);
                    return invoke(c, g, RESOLVE, 0x112462);
                }
                Step::Next(0x112493)
            }
            0x112462 => {
                test_byte(c, c.eax as u8);
                if c.eax as u8 != 0 {
                    c.edx = g.u32(at(c.esp, 0xf4))?;
                    c.eax = g.u32(at(c.esp, 0xf0))?;
                    c.ecx = g.u32(at(c.esp, 0x14))?;
                    push(c, g, c.edx)?;
                    c.edx = g.u32(at(c.esp, 0xf0))?;
                    push(c, g, c.eax)?;
                    push_weight(c, g, 0x112482)?;
                    push(c, g, c.ecx)?;
                    c.ebx = at(c.esp, 0x58);
                    return invoke(c, g, 0x1c7200, 0x112490);
                }
                Step::Next(0x112490)
            }
            0x112490 | 0x112493 => {
                if pc == 0x112490 {
                    set_none(c);
                }
                c.eax = g.u32(at(c.esp, 0x1c))?.wrapping_sub(1);
                c.edi = at(c.edi, 4);
                c.esi = at(c.esi, 4);
                g.set_u32(at(c.esp, 0x1c), c.eax)?;
                Step::Next(if c.eax != 0 { 0x1123f0 } else { 0x1124a8 })
            }
            0x1124a8 => {
                c.edx = g.u32(at(c.esp, 0x24))?;
                c.eax = g.u32(at(c.edx, 0xbc))? >> 10;
                test_byte(c, (c.eax & 1) as u8);
                if c.eax & 1 != 0 {
                    return Ok(Step::Next(FINAL_LAYERS));
                }
                let basis = g.u32(0x4687c4)?;
                c.ecx = g.u32(basis)?;
                c.edx = g.u32(at(basis, 4))?;
                c.eax = g.u32(at(c.esp, 0xe8))?;
                c.edi = at(c.esp, 0x8c);
                c.esi = at(c.esp, 0x74);
                g.set_u32(at(c.esp, 0x34), c.ecx)?;
                g.set_u32(at(c.esp, 0x38), c.edx)?;
                return invoke(c, g, 0xb9fc0, 0x1124e6);
            }
            _ => unreachable!("unit pose layers entered at {pc:#x}"),
        };
        match step {
            Step::Next(next) if next != FINAL_LAYERS => pc = next,
            _ => return Ok(step),
        }
    }
}
