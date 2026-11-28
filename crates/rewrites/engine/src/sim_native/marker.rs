use super::*;
use crate::function_eval::constant::ONE;
use guest_layout::points::{OBJECTS, TAG_ENTRIES};

const PARENT_OBJECT: u32 = 0x000b_8ca0;
const AFTER_PARENT: u32 = 0x000b_8d48;
const AFTER_FIND: u32 = 0x000b_8dd4;

// The object may inherit markers from its parent. The parent resolver
// tail-calls itself after replacing the argument in its caller's frame.
fn parent(
    c: &mut CpuAbi,
    g: &mut impl Memory,
) -> std::result::Result<Option<TransferAbi>, GuestFault> {
    push(c, g, c.esi)?;
    push(c, g, AFTER_PARENT)?;
    for _ in 0..256 {
        c.ecx = g.u32(OBJECTS.base)?;
        c.edx = g.u32(at(c.ecx, 0x44))?;
        for saved in [c.esi, c.edi] {
            push(c, g, saved)?;
        }
        c.edi = g.u32(at(c.esp, 12))?;
        c.esi = g.u32(at(c.edx, object_slot(c.edi) + 8))?;
        c.eax = g.u32(at(c.esi, 20))?;
        if c.eax != u32::MAX {
            c.edx = g.u32(TAG_ENTRIES.base)?;
            c.ecx = tag_data(g, c.edx, g.u32(c.esi)?)?;
            let flags = g.u8(at(c.ecx, 2))? >> 4;
            c.edx = low(c.edx, flags);
            if flags & 1 != 0 {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                g.set_u32(at(c.esp, 4), c.eax)?;
                continue;
            }
        }
        if g.u8(at(c.esi, 4))? & 1 != 0 {
            c.ecx = low(c.ecx, g.u8(at(c.esi, 0xaa))?);
            c.eax = 1u32.wrapping_shl(c.ecx);
            let bits = (c.eax as u8) & 0x1c;
            if bits != 0 {
                test_byte(c, bits);
                c.ecx = c.edi;
                const ATTACHED_OBJECT: u32 = 0x0010_cf50;
                const AFTER_ATTACHED: u32 = 0x000b_8d0e;
                return call(c, g, ATTACHED_OBJECT, AFTER_ATTACHED).map(Some);
            }
        }
        c.eax = c.edi;
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        pop(c, g)?;
        c.esp = at(c.esp, 4);
        return Ok(None);
    }
    Ok(Some(TransferAbi::jump(PARENT_OBJECT)))
}

// The search returns the first matching group index (or the sentinel).
// Its caller passes that index and the node matrices to the model.
fn lookup(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    push(c, g, AFTER_FIND)?;
    c.eax = u32::MAX;
    sub_flags(c, c.ecx, u32::MAX, 32);
    if c.ecx != u32::MAX {
        sub_flags(c, c.edi, u32::MAX, 32);
        if c.edi != u32::MAX {
            logic_flags(c, c.edi);
            if c.edi != 0 {
                c.edx = g.u32(TAG_ENTRIES.base)?;
                c.ecx = tag_data(g, c.edx, c.ecx)?;
                push(c, g, c.esi)?;
                c.esi = g.u32(at(c.ecx, 88))?;
                c.edx = 0;
                logic_flags(c, c.esi);
                if (c.esi as i32) > 0 {
                    c.ecx = g.u32(at(c.ecx, 92))?;
                    for _ in 0.. {
                        if c.edx == 256 {
                            const NEXT_GROUP: u32 = 0x001d_8f31;
                            return Ok(TransferAbi::resume(NEXT_GROUP));
                        }
                        let name = g.u32(c.ecx)?;
                        sub_flags(c, name, c.edi, 32);
                        if name == c.edi {
                            c.eax = c.edx;
                            break;
                        }
                        c.edx = at(c.edx, 1);
                        c.ecx = at(c.ecx, 12);
                        sub_flags(c, c.edx, c.esi, 32);
                        if c.edx == c.esi {
                            break;
                        }
                    }
                }
                c.esi = pop(c, g)?;
            }
        }
    }
    pop(c, g)?;
    c.edx = g.u16(at(c.esp, 32))? as i16 as u32;
    c.ecx = g.u32(at(c.esp, 12))?;
    for arg in [c.edx, c.ebx, c.ecx] {
        push(c, g, arg)?;
    }
    c.ecx = g.u32(at(c.esp, 28))?;
    for arg in [c.ebp, 0, c.esi] {
        push(c, g, arg)?;
    }
    const MODEL_MARKERS: u32 = 0x001d_8f50;
    const AFTER_MARKERS: u32 = 0x000b_8ded;
    call(c, g, MODEL_MARKERS, AFTER_MARKERS)
}

/// Select the model and its node matrices for a marker lookup. When the
/// model is absent, produce the object's root marker, including mirroring.
pub fn transforms(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.esp = c.esp.wrapping_sub(8);
    for saved in [c.ebp, c.esi] {
        push(c, g, saved)?;
    }
    c.esi = g.u32(at(c.esp, 20))?;
    c.ebp = 0;
    test_byte(c, c.ecx as u8);
    let saved = c.edi;
    push(c, g, saved)?;
    c.eax = c.esi;
    if c.ecx as u8 == 0
        && let Some(transfer) = parent(c, g)?
    {
        return Ok(transfer);
    }
    let sp = c.esp;
    if c.eax != u32::MAX {
        let objects = g.u32(OBJECTS.base)?;
        let slots = g.u32(at(objects, 0x44))?;
        let obj = g.u32(at(slots, object_slot(c.eax) + 8))?;
        let tags = g.u32(TAG_ENTRIES.base)?;
        let def = tag_data(g, tags, g.u32(obj)?)?;
        let model = g.u32(at(def, 0x38))?;
        if model != u32::MAX {
            c.esi = at(obj, g.u16(at(obj, 0x11a))? as i16 as u32);
            c.ebp = at(obj, g.u16(at(obj, 0x116))? as i16 as u32);
            let model = tag_data(g, tags, model)?;
            let bytes = g.u16(at(obj, 0x114))? as i16 as u32;
            g.set_u8(at(sp, 12), ((g.u32(at(obj, 4))? >> 10) & 1) as u8)?;
            // The compiler divides the byte count by sizeof(matrix).
            // EAX retains the low half of the widening multiply on entry
            // to the lookup. Recover its reciprocal from the instruction.
            const DIVISOR_RECIPROCAL: u32 = 0x000b_8db7;
            let wide = u64::from(bytes) * u64::from(g.u32(DIVISOR_RECIPROCAL)?);
            c.eax = wide as u32;
            c.ecx = g.u32(at(model, 4))?;
            c.edi = g.u32(at(sp, 28))?;
            let high = (wide >> 32) as u32;
            c.edx = high >> 4;
            let carry = (high >> 3) & 1;
            flags(c, szp(c.edx) | carry, STATUS & !(0x800 | 0x10));
            g.set_u32(at(sp, 16), c.edx)?;
            g.set_u32(at(sp, 16), c.ecx)?;
            // Phase 1c: the scan and the transform fill as one Rust call
            // (engine::sim_anim::markers). The port's own path, with the
            // guest `1d8f50`, runs only when the kernel is off; a decline
            // panics.
            if let Some(transfer) = crate::sim_anim::markers::run(c, g)? {
                return Ok(transfer);
            }
            return lookup(c, g);
        }
    }
    let objects = g.u32(OBJECTS.base)?;
    let slots = g.u32(at(objects, 0x44))?;
    let one = g.u32(ONE)?;
    let slot = object_slot(c.esi);
    let obj = g.u32(at(slots, slot + 8))?;
    put_xmm(c, 0, [0; 4]);
    put_xmm(c, 1, scalar(one));
    g.set_u16(c.ebx, 0)?;
    for i in 0..13 {
        g.set_u32(
            at(c.ebx, 4 + 4 * i),
            if matches!(i, 0 | 1 | 5 | 9) { one } else { 0 },
        )?;
    }
    let slots = g.u32(at(objects, 0x44))?;
    let current = g.u32(at(slots, slot + 8))?;
    let mut src = at(current, g.u16(at(current, 0x116))? as i16 as u32);
    let mut dst = at(c.ebx, 56);
    let step = if c.eflags & (1 << 10) == 0 {
        4
    } else {
        u32::MAX - 3
    };
    for _ in 0..13 {
        g.set_u32(dst, g.u32(src)?)?;
        src = at(src, step);
        dst = at(dst, step);
    }
    g.set_u32(at(c.ebx, 108), 0)?;
    c.edx = g.u32(at(obj, 4))? >> 10;
    if c.edx & 1 != 0 {
        for (i, offset) in [(1, 72), (1, 76), (0, 80)] {
            let v = op(c, Sub, 0, g.u32(at(c.ebx, offset))?);
            put_xmm(c, i, scalar(v));
            g.set_u32(at(c.ebx, offset), v)?;
        }
    }
    c.eax = g.u32(at(sp, 28))?;
    c.eax = if c.eax == 0 { 1 } else { c.eax & 0xffff_0000 };
    c.ecx = 0;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    add_flags(c, c.esp, 8);
    c.esp = at(c.esp, 8);
    ret(g, c, 12)
}
