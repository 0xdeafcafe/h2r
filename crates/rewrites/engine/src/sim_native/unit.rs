use super::*;
use crate::{
    function_eval::constant::ONE,
    points::{OBJECTS, TAG_ENTRIES},
};

const FIRST_LAYER: u32 = 0x0011_2194;
const SECOND_LAYER: u32 = 0x0011_21d1;
const EVALUATE_LAYER: u32 = 0x001c_6f60;

/// Set up the unit's animation manager, model and render-model views and
/// select the unit's base, action, overlay and aim layers. Evaluators
/// receive the same root/parent frame and per-layer weight as the guest.
pub fn pose(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.esp = c.esp.wrapping_sub(0xd4);
    let unit = g.u32(at(c.esp, 0xd8))?;
    let objects = g.u32(OBJECTS.base)?;
    let slots = g.u32(at(objects, 0x44))?;
    for saved in [c.ebx, c.ebp, c.esi, c.edi] {
        push(c, g, saved)?;
    }
    let slot = object_slot(unit);
    c.edi = g.u32(at(slots, slot + 8))?;
    let def = g.u32(c.edi)?;
    c.esi = at(c.edi, g.u16(at(c.edi, 0x12a))? as i16 as u32);
    let graph = g.u32(at(c.esi, 0x68))?;
    c.ebp = at(c.edi, g.u16(at(c.edi, 0x33e))? as i16 as u32);
    let sp = c.esp;
    g.set_u32(at(sp, 0x6c), slot)?;
    let tags = g.u32(TAG_ENTRIES.base)?;
    let def = tag_data(g, tags, def)?;
    g.set_u32(at(sp, 0x24), def)?;
    let model = g.u32(at(def, 0x38))?;
    g.set_u32(at(sp, 0x68), tag_data(g, tags, graph)?)?;
    let model = tag_data(g, tags, model)?;
    c.edx = tag_data(g, tags, g.u32(at(model, 4))?)?;
    c.ebx = u32::MAX;
    c.ecx = g.u32(at(c.ebp, 0x9c))?;
    for offset in [0x58, 0x59] {
        g.set_u8(at(sp, offset), 0)?;
    }
    for offset in [0x5a, 0x5c, 0x5e] {
        g.set_u16(at(sp, offset), 0)?;
    }
    g.set_u32(at(sp, 0x64), 0)?;
    let one = g.u32(ONE)?;
    put_xmm(c, 0, scalar(one));
    for (offset, value) in [
        (0x2c, c.edi),
        (0x18, c.esi),
        (0x20, c.edx),
        (0x48, u32::MAX),
    ] {
        g.set_u32(at(sp, offset), value)?;
    }
    g.set_u16(at(sp, 0x4c), u16::MAX)?;
    g.set_u16(at(sp, 0x4e), u16::MAX)?;
    g.set_u32(at(sp, 0x60), one)?;
    g.set_u32(at(sp, 0x50), u32::MAX)?;
    g.set_u16(at(sp, 0x56), u16::MAX)?;
    g.set_u8(at(sp, 0x54), 255)?;
    g.set_u8(at(sp, 0x55), 255)?;
    for (offset, resume) in [(0x9c, FIRST_LAYER), (0xbc, SECOND_LAYER)] {
        c.eax = at(c.ebp, offset);
        c.ecx = g.u32(c.eax)?;
        sub_flags(c, c.ecx, u32::MAX, 32);
        if c.ecx == u32::MAX {
            continue;
        }
        let index = g.u16(at(c.eax, 6))?;
        sub_flags(c, u32::from(index), 0xffff, 16);
        if index == u16::MAX {
            continue;
        }
        let end = g.u32(at(sp, 0xf4))?;
        let start = g.u32(at(sp, 0xf0))?;
        push(c, g, end)?;
        let count = g.u32(at(c.esp, 0xf0))?;
        push(c, g, start)?;
        push(c, g, count)?;
        // Immediate weight in the call (separate from the mutable global).
        let weight_operand = if offset == 0x9c {
            0x0011_2189
        } else {
            0x0011_21c6
        };
        let weight = g.u32(weight_operand)?;
        push(c, g, weight)?;
        c.ecx = if offset == 0x9c { count } else { start };
        c.edx = if offset == 0x9c { start } else { count };
        c.ebx = c.eax;
        return call(c, g, EVALUATE_LAYER, resume);
    }
    // Optional action layer. Its weight depends on two callbacks.
    logic_flags(c, u32::MAX);
    c.ebx = u32::MAX;
    let action = g.u32(at(c.ebp, 0xdc))?;
    sub_flags(c, action, c.ebx, 32);
    if action != c.ebx {
        let index = u32::from(g.u16(at(c.ebp, 0xe2))?);
        sub_flags(c, index, 0xffff, 16);
        if index != 0xffff {
            c.ecx = g.u32(at(sp, 0xe8))?;
            const UNIT_ACTION: u32 = 0x000c_8fc0;
            const AFTER_ACTION: u32 = 0x0011_21f5;
            return call(c, g, UNIT_ACTION, AFTER_ACTION);
        }
    }
    // The two overlay selectors use different animation flag bits.
    for (offset, mask, resume) in [(0x104, 1 << 10, 0x0011_228b), (0x108, 1 << 11, 0x0011_22e1)] {
        let index = u32::from(g.u16(at(c.ebp, offset + 2))?);
        sub_flags(c, index, 0xffff, 16);
        if index != 0xffff {
            c.ecx = g.u32(at(c.esi, 0x68))?;
            sub_flags(c, c.ecx, c.ebx, 32);
            c.eax = g.u32(at(c.ebp, offset))?;
            if c.ecx != c.ebx {
                for arg in [c.ebx, c.ebx, c.ebx, c.ebx, c.eax] {
                    push(c, g, arg)?;
                }
                c.edx = mask;
                c.eax = at(c.esp, 0x5c);
                const SELECT_OVERLAY: u32 = 0x001c_6470;
                return call(c, g, SELECT_OVERLAY, resume);
            }
        }
    }
    g.set_u8(c.ebp, g.u8(c.ebp)? & !(3 << 5))?;
    c.ecx = g.u32(at(sp, 0x24))?;
    c.edx = g.u32(at(c.ecx, 0xbc))? >> 11;
    if c.edx & 1 != 0 {
        return final_layers(c, g);
    }
    c.eax = g.u32(at(sp, 0x18))?;
    if g.u32(at(c.eax, 0x68))? == c.ebx {
        return final_layers(c, g);
    }
    const ZERO: u32 = 0x0045_dbd8;
    let weight = g.u32(at(c.edi, 0x208))?;
    put_xmm(c, 0, scalar(weight));
    if compare(c, weight, g.u32(ZERO)?) == 0 {
        c.ecx = g.u32(OBJECTS.base)?;
        c.edx = g.u32(at(c.ecx, 0x44))?;
        c.eax = g.u32(at(sp, 0x6c))?;
        c.eax = g.u32(at(c.edx, at(c.eax, 8)))?;
        c.ecx = g.u16(at(c.eax, 0x33e))? as i16 as u32;
        let index = g.u16(at(at(c.ecx, c.eax), 0x36))?;
        sub_flags(c, index.into(), 0xffff, 16);
        if index == u16::MAX {
            c.eax = g.u32(at(sp, 0x18))?;
            c.esi = at(sp, 0x1c);
            const LOOKUP: u32 = 0x001c_c150;
            return call(c, g, LOOKUP, 0x0011_2372);
        }
    }
    c.edx = g.u32(at(sp, 0x18))?;
    const SKIP_AIM_NAME: u32 = 0x0011_23c4;
    if g.u32(at(c.edx, 0x7c))? == g.u32(SKIP_AIM_NAME)? {
        return final_layers(c, g);
    }
    if g.u8(c.ebp)? & 2 != 0 {
        c.esi = at(c.edi, 0x288);
        c.edi = at(c.ebp, 0x118);
        g.set_u32(at(sp, 0x1c), 3)?;
        loop {
            c.eax = g.u32(c.edi)?;
            c.ecx = c.eax >> 16;
            g.set_u32(at(sp, 0x20), c.eax)?;
            if c.ecx != 0xffff && aim_weight(c, g)? {
                push(c, g, c.eax)?;
                c.eax = g.u32(at(c.esp, 0x1c))?;
                c.ecx = at(c.esp, 0x4c);
                push(c, g, c.ecx)?;
                c.ebx = 0;
                logic_flags(c, 0);
                const RESOLVE_AIM: u32 = 0x001c_b490;
                return call(c, g, RESOLVE_AIM, 0x0011_2462);
            }
            c.eax = g.u32(at(sp, 0x1c))?.wrapping_sub(1);
            c.edi = at(c.edi, 4);
            c.esi = at(c.esi, 4);
            g.set_u32(at(sp, 0x1c), c.eax)?;
            if c.eax == 0 {
                break;
            }
        }
    }
    c.edx = g.u32(at(sp, 0x24))?;
    c.eax = g.u32(at(c.edx, 0xbc))? >> 10;
    test_byte(c, (c.eax & 1) as u8);
    if c.eax & 1 != 0 {
        return final_layers(c, g);
    }
    const BASIS: u32 = 0x0046_87c4;
    c.eax = g.u32(BASIS)?;
    c.ecx = g.u32(c.eax)?;
    c.edx = g.u32(at(c.eax, 4))?;
    c.eax = g.u32(at(sp, 0xe8))?;
    c.edi = at(sp, 0x8c);
    c.esi = at(sp, 0x74);
    g.set_u32(at(sp, 0x34), c.ecx)?;
    g.set_u32(at(sp, 0x38), c.edx)?;
    const OBJECT_BASIS: u32 = 0x000b_9fc0;
    call(c, g, OBJECT_BASIS, 0x0011_24e6)
}

// Decide whether this aim channel differs enough from its neutral pose.
// The clamp is SSE. The threshold test retains extended precision.
fn aim_weight(c: &mut CpuAbi, g: &mut impl Memory) -> std::result::Result<bool, GuestFault> {
    use x87::abi as fp;
    const HALF: u32 = 0x0045_dbbc;
    let half = g.u32(HALF)?;
    let scaled = op(c, Mul, g.u32(c.esi)?, half);
    let weight = op(c, Add, scaled, half);
    put_xmm(c, 0, scalar(weight));
    put_xmm(c, 1, scalar(0));
    let local = at(c.esp, 20);
    let below = compare(c, 0, weight) == 0;
    g.set_u32(local, weight)?;
    if below {
        g.set_u32(local, 0)?;
    } else {
        let one = g.u32(ONE)?;
        put_xmm(c, 1, scalar(one));
        if compare(c, weight, one) == 0 {
            g.set_u32(local, one)?;
        }
    }
    let mut x = ray::load_fpu(c);
    fp::ld_f32(&mut x, g.u32(local)?);
    fp::arith(
        &mut x,
        fp::arith_op::SUB,
        0,
        fp::src_kind::F32,
        g.u32(HALF)?.into(),
    );
    fp::unary(&mut x, fp::unary_op::ABS);
    let small = crate::function_eval::constant::SMALL;
    fp::ld_f32(&mut x, g.u32(small)?);
    let f = fp::cmp(&mut x, 0, fp::src_kind::ST, 1, 1, true);
    fp::st_st(&mut x, 0, true);
    const POP_WEIGHT: u32 = 0x0011_244d;
    ray::finish_fpu(c, g, &x, POP_WEIGHT, small)?;
    flags(c, f, STATUS);
    Ok(f != 0)
}

fn final_layers(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let sp = c.esp;
    c.eax = g.u32(OBJECTS.base)?;
    c.ecx = g.u32(at(c.eax, 0x44))?;
    c.edx = g.u32(at(sp, 0x6c))?;
    c.eax = g.u32(at(c.ecx, at(c.edx, 8)))?;
    let paired = g.u8(at(c.eax, 0x212))? != 255 && g.u8(at(c.eax, 0x213))? != 255;
    if paired {
        for (offset, start, mask, resume) in [
            (0x10c, true, 1 << 10, 0x0011_2d2a),
            (0x110, false, 1 << 11, 0x0011_2d84),
        ] {
            if g.u16(at(c.ebp, offset + 2))? != u16::MAX {
                let animation = g.u32(at(sp, 0x18))?;
                c.ecx = g.u32(at(animation, 0x68))?;
                let handle = g.u32(at(c.ebp, offset))?;
                if start {
                    c.edx = animation;
                    c.eax = handle;
                } else {
                    c.eax = animation;
                    c.ebp = handle;
                }
                sub_flags(c, c.ecx, c.ebx, 32);
                if c.ecx != c.ebx {
                    for arg in [c.ebx, c.ebx, c.ebx, c.ebx, handle] {
                        push(c, g, arg)?;
                    }
                    c.edx = mask;
                    c.eax = at(c.esp, 0x5c);
                    return call(c, g, 0x001c_6470, resume);
                }
            }
        }
    } else if g.u16(at(c.ebp, 0x10e))? != u16::MAX {
        let active = g.u8(at(c.ebp, 0x81))?;
        c.eax = low(c.eax, active);
        c.esi = 0;
        test_byte(c, active);
        if active != 0 {
            c.eax = at(c.ebp, 0x80);
            return call(c, g, 0x001d_9370, 0x0011_2ca2);
        }
        c.eax = g.u32(at(sp, 0x18))?;
        let graph = g.u32(at(c.eax, 0x68))?;
        sub_flags(c, graph, c.ebx, 32);
        c.ebp = g.u32(at(c.ebp, 0x10c))?;
        if graph != c.ebx {
            for arg in [c.ebx, c.ebx, c.ebx, c.ebx, c.ebp] {
                push(c, g, arg)?;
            }
            c.ecx = at(c.esp, 0x5c);
            push(c, g, c.ecx)?;
            c.ebx = c.esi;
            return call(c, g, 0x001c_b410, 0x0011_2cd8);
        }
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    add_flags(c, c.esp, 0xd4);
    c.esp = at(c.esp, 0xd4);
    ret(g, c, 16)
}
