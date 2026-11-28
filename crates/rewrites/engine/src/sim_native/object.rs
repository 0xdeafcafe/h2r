//! Object function value and import ports. Never installed: the bodies in
//! `crate::sim_objects::functions` own these entries; these stay as test
//! oracles
//! (`tests/sim_native_differential.rs`).
use super::*;
use crate::function_eval::constant::ONE;
use guest_layout::points::OBJECTS;

pub const DESCRIPTORS: u32 = 0x0046_8630;
// The imm32 follows the opcode and ModR/M byte in both CMP EDI instructions.
pub const NAME_ONE: u32 = 0x000b_abab;
pub const NAME_ZERO: u32 = 0x000b_abb7;
const OVERRIDE_VALUE: u32 = 0x0010_aac0;
const AFTER_OVERRIDE: u32 = 0x000b_ab93;
const AFTER_IMPORT: u32 = 0x000b_abe2;
const AFTER_HANDLER: u32 = 0x0010_8de7;

/// Resolve the type ancestry, zero the outputs, and dispatch the first
/// live import handler. Its result re-enters the original descriptor loop.
pub fn import(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let objects = g.u32(OBJECTS.base)?;
    c.edx = g.u32(at(objects, 0x44))?;
    put_xmm(c, 0, [0; 4]);
    let saved = c.ebp;
    push(c, g, saved)?;
    c.ebp = g.u32(at(c.esp, 8))?;
    c.eax = g.u32(at(c.edx, object_slot(c.ebp) + 8))?;
    let ty = g.u8(at(c.eax, 0xaa))? as i8 as u32;
    c.ecx = g.u32(at(DESCRIPTORS, ty << 2))?;
    c.eax &= !255;
    let saved = c.esi;
    push(c, g, saved)?;
    g.set_u32(c.ebx, 0)?;
    g.set_u8(c.edi, 0)?;
    c.esi = at(c.ecx, 0x84);
    descriptors(c, g)
}

fn descriptors(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    for index in 0.. {
        if index == 256 {
            const NEXT_DESCRIPTOR: u32 = 0x0010_8dd0;
            test_byte(c, c.eax as u8);
            return Ok(TransferAbi::resume(NEXT_DESCRIPTOR));
        }
        c.ecx = g.u32(c.esi)?;
        logic_flags(c, c.ecx);
        if c.ecx == 0 {
            logic_flags(c, 0);
            put_xmm(c, 0, [0; 4]);
            g.set_u32(c.ebx, 0)?;
            g.set_u8(c.edi, 0)?;
            c.esi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            return ret(g, c, 8);
        }
        c.ecx = g.u32(at(c.ecx, 0x4c))?;
        logic_flags(c, c.ecx);
        if c.ecx != 0 {
            c.edx = g.u32(at(c.esp, 16))?;
            for arg in [c.edi, c.ebx, c.edx, c.ebp] {
                push(c, g, arg)?;
            }
            return call(c, g, c.ecx, AFTER_HANDLER);
        }
        c.esi = at(c.esi, 4);
    }
    unreachable!()
}

/// Object value lookup, including the built-in constants and scripted overrides.
pub fn value(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let saved = c.ecx;
    push(c, g, saved)?;
    put_xmm(c, 4, [0; 4]);
    let saved = c.ebx;
    push(c, g, saved)?;
    c.ebx = g.u32(at(c.esp, 12))?;
    let saved = c.ebp;
    push(c, g, saved)?;
    c.ebp = g.u32(at(c.esp, 24))?;
    let saved = c.esi;
    push(c, g, saved)?;
    c.esi = object_slot(c.ebx);
    let objects = g.u32(OBJECTS.base)?;
    g.set_u32(c.ebp, 0)?;
    c.ecx = g.u32(at(objects, 0x44))?;
    g.set_u8(at(c.esp, 15), 0)?;
    c.edx = g.u32(at(c.ecx, c.esi + 8))?;
    c.eax = u32::from(g.u16(at(c.edx, 0xc0))?) >> 9;
    let saved = c.edi;
    push(c, g, saved)?;
    c.edi = g.u32(at(c.esp, 28))?;
    let override_bit = c.eax & 1;
    logic_flags(c, override_bit);
    if override_bit != 0 {
        c.ecx = at(c.esp, 32);
        for arg in [c.ecx, c.ebp, c.edi] {
            push(c, g, arg)?;
        }
        return call(c, g, OVERRIDE_VALUE, AFTER_OVERRIDE);
    }
    let name = c.edi;
    logic_flags(c, name);
    let one = if name == 0 {
        true
    } else {
        let key = g.u32(NAME_ONE)?;
        sub_flags(c, name, key, 32);
        name == key
    };
    if one {
        let one = g.u32(ONE)?;
        put_xmm(c, 0, scalar(one));
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        g.set_u32(c.ebp, one)?;
        c.ebp = pop(c, g)?;
        c.eax = low(c.eax, 1);
    } else {
        let key = g.u32(NAME_ZERO)?;
        sub_flags(c, name, key, 32);
        if name == key {
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            g.set_u32(c.ebp, 0)?;
            c.ebp = pop(c, g)?;
            c.eax &= !255;
            logic_flags(c, 0);
        } else {
            let object = c.ebx;
            sub_flags(c, object, u32::MAX, 32);
            if object != u32::MAX {
                push(c, g, name)?;
                push(c, g, object)?;
                c.edi = at(c.esp, 27);
                c.ebx = c.ebp;
                return call(
                    c,
                    g,
                    guest_layout::points::OBJECT_FUNCTION_IMPORT.base,
                    AFTER_IMPORT,
                );
            }
            c.eax = low(c.eax, g.u8(at(c.esp, 19))?);
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            c.ebp = pop(c, g)?;
        }
    }
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    ret(g, c, 12)
}

/// A return from a native import can continue the calling native evaluator
/// in the same dispatch. Test the actual return address (after all guest
/// cleanup). A feature after-hook's return token still goes to its owner.
pub fn complete_return(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    mut transfer: TransferAbi,
    descriptor_on: bool,
    value_on: bool,
) -> Result {
    if descriptor_on && transfer == TransferAbi::ret(AFTER_HANDLER) {
        c.esi = at(c.esi, 4);
        test_byte(c, c.eax as u8);
        transfer = if c.eax as u8 == 0 {
            descriptors(c, g)?
        } else {
            c.esi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            ret(g, c, 8)?
        };
    }
    if value_on && transfer == TransferAbi::ret(AFTER_IMPORT) {
        return after_import(c, g);
    }
    Ok(transfer)
}

fn after_import(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    test_byte(c, c.eax as u8);
    if c.eax as u8 == 0 {
        let objects = g.u32(OBJECTS.base)?;
        let slots = g.u32(at(objects, 0x44))?;
        c.ecx = g.u32(at(slots, at(c.esi, 8)))?;
        let tag = g.u32(c.ecx)?;
        c.edi = tag_data(g, g.u32(guest_layout::points::TAG_ENTRIES.base)?, tag)?;
        c.edx = g.u32(at(c.edi, 0x64))?;
        c.esi = 0;
        g.set_u32(at(c.esp, 32), c.ecx)?;
        if (c.edx as i32) > 0 {
            c.ebx = 0;
            for scanned in 0.. {
                if scanned == 256 {
                    return Ok(TransferAbi::resume(0x000b_ac20));
                }
                let exports = g.u32(at(c.edi, 0x68))?;
                c.ecx = at(exports, c.ebx);
                c.edx = g.u32(at(c.ecx, 8))?;
                let name = g.u32(at(c.esp, 28))?;
                sub_flags(c, c.edx, name, 32);
                if c.edx == name {
                    c.eax = g.u32(at(c.esp, 24))?;
                    push(c, g, c.ebp)?;
                    push(c, g, c.eax)?;
                    c.eax = c.ecx;
                    const EVALUATE_EXPORT: u32 = 0x000b_a8c0;
                    return call(c, g, EVALUATE_EXPORT, 0x000b_ac3c);
                }
                c.ecx = g.u32(at(c.edi, 0x64))?;
                c.esi = at(c.esi, 1);
                c.ebx = at(c.ebx, 32);
                sub_flags(c, c.esi, c.ecx, 32);
                if (c.esi as i32) >= (c.ecx as i32) {
                    break;
                }
            }
            c.ecx = g.u32(at(c.esp, 32))?;
        }
        c.edx = g.u32(at(c.ecx, 4))? >> 26;
        if c.edx & 1 != 0 {
            c.eax = g.u32(at(c.ecx, 20))?;
            sub_flags(c, c.eax, u32::MAX, 32);
            if c.eax != u32::MAX {
                c.ecx = g.u32(at(c.esp, 28))?;
                for arg in [c.ebp, c.ecx, c.eax] {
                    push(c, g, arg)?;
                }
                return call(c, g, guest_layout::points::OBJECT_FUNCTION_VALUE.base, 0x000b_ac74);
            }
        }
        put_xmm(c, 0, scalar(0));
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        g.set_u32(c.ebp, 0)?;
        c.ebp = pop(c, g)?;
        c.eax &= !255;
        logic_flags(c, 0);
    } else {
        c.eax = low(c.eax, g.u8(at(c.esp, 19))?);
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        c.ebp = pop(c, g)?;
    }
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    ret(g, c, 12)
}
