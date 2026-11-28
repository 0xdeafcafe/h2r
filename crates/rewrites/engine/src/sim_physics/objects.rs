//! Object physics phases. The first pass uses the explicit update list.
//! The second enumerates the live headers in slot order, reloading the
//! high-water mark and bitmap after every visited slot. Callbacks can add,
//! delete and relink objects during either pass.

use super::*;
use guest_layout::datum::layout;

const LIST_OWNER: u32 = 0x0051_07f0;
const PHASE_STATE: u32 = 0x004d_e2f4;
const LIST: u32 = 0x1d00;
const LIST_COUNT: u32 = 0x1d80;
const HEADER_FLAGS: u32 = 2;
const OBJECT_PTR: u32 = 8;
const PHYSICS_FLAGS: u32 = 0xc0;
const HEADER_BYTES: u32 = 12;

const PRE_OBJECT: u32 = 0x000b_c470;
const PRE_LIST_VISIT: u32 = 0x0010_9580;
const POST_OBJECT: u32 = 0x000b_c820;
const POST_LIST_VISIT: u32 = 0x0010_9660;
const POST_FINISH: u32 = 0x000f_d910;

// Return PCs, in call order, of the list update, its follow-up, and the
// occupancy-walk update (plus the post phase's final callback).
pub(super) const PRE_SITES: [u32; 3] = [0xb6f6e, 0xb6f7f, 0xb7036];
pub(super) const POST_SITES: [u32; 4] = [0xb70ae, 0xb70b7, 0xb7142, 0xb70f6];

fn set_phase(g: &mut Guest, state: u32, active: bool) -> Result<()> {
    g.set_u8(state.wrapping_add(3), u8::from(active))
}

fn current_objects(g: &Guest) -> Result<u32> {
    g.u32(points::OBJECTS.base)
}

fn header_offset(handle: u32) -> u32 {
    (handle & 0xffff) * HEADER_BYTES
}

#[derive(PartialEq, Eq)]
enum Scan {
    Hit,
    Negative,
    AtLimit,
    Exhausted,
}

// Skip zero runs a word at a time. Never look beyond the current word
// before proving that the previous one contained no candidate. Recreate
// the final slot's scratch registers even when no callback follows.
fn next_live(c: &mut CpuAbi, g: &Guest) -> Result<Scan> {
    c.eax = c.edi.wrapping_add(1);
    if (c.eax as i32) < 0 {
        logic(c, c.eax, 1 << 31);
        return Ok(Scan::Negative);
    }
    c.edi = g.u32(c.esi.wrapping_add(layout::HIGH_WATER))?;
    if c.eax as i32 >= c.edi as i32 {
        arithmetic(c, c.eax, c.edi, true);
        return Ok(Scan::AtLimit);
    }
    c.edx = g.u32(c.esi.wrapping_add(layout::BITMAP))?;
    loop {
        c.ecx = c.eax >> 5;
        let word = g.u32(c.edx.wrapping_add(4 * c.ecx))?;
        let live = word & (u32::MAX << (c.eax & 31));
        let base = c.eax & !31;
        let candidate = base + live.trailing_zeros();
        if live != 0 && candidate < c.edi {
            c.eax = candidate;
            c.ebx = 1 << (candidate & 31);
            // Both originals compare the selected slot to the NONE sentinel.
            arithmetic(c, candidate, u32::MAX, true);
            return Ok(Scan::Hit);
        }
        if base + 32 >= c.edi {
            c.ebx = 1 << ((c.edi - 1) & 31);
            c.eax = c.edi;
            arithmetic(c, c.eax, c.edi, true);
            return Ok(Scan::Exhausted);
        }
        c.eax = base + 32;
    }
}

fn restore(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    Ok(())
}

pub(super) fn pre(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    if pc == PRE_SITES[2] {
        return pre_slots(c, g);
    }
    if pc == points::OBJECTS_PRE_PHYSICS.base {
        push(c, g, c.ecx)?;
        c.eax = g.u32(LIST_OWNER)?;
        c.ecx = g.u32(PHASE_STATE)?;
        push(c, g, c.ebx)?;
        push(c, g, c.ebp)?;
        c.ebx = c.eax.wrapping_add(LIST);
        c.eax = g.u32(c.eax.wrapping_add(LIST_COUNT))?;
        push(c, g, c.esi)?;
        c.ebp = 0;
        push(c, g, c.edi)?;
        g.set_u32(c.esp.wrapping_add(16), c.eax)?;
        set_phase(g, c.ecx, true)?;
    } else {
        if pc == PRE_SITES[0] {
            let flags = g.u8(c.edi.wrapping_add(HEADER_FLAGS))?;
            c.eax = byte(c.eax, flags);
            if flags & 12 == 4 {
                logic(c, 0, 0x80);
                return call_arg(c, g, c.esi, PRE_LIST_VISIT, PRE_SITES[1]);
            }
        }
        c.ebp = c.ebp.wrapping_add(1);
        c.eax = g.u32(c.esp.wrapping_add(16))?;
    }

    while (c.ebp as i32) < c.eax as i32 {
        c.esi = g.u32(c.ebx.wrapping_add(c.ebp.wrapping_mul(4)))?;
        c.edx = (c.esi & 0xffff) * 3;
        c.eax = current_objects(g)?;
        c.ecx = g.u32(c.eax.wrapping_add(layout::DATA))?;
        c.edi = c.ecx.wrapping_add(4 * c.edx);
        let flags = g.u8(c.edi.wrapping_add(HEADER_FLAGS))?;
        c.eax = byte(c.eax, flags);
        if flags & 11 == 3 {
            logic(c, 0, 0x80);
            return call_arg(c, g, c.esi, PRE_OBJECT, PRE_SITES[0]);
        }
        c.eax = g.u32(c.esp.wrapping_add(16))?;
        c.ebp = c.ebp.wrapping_add(1);
    }
    c.esi = current_objects(g)?;
    c.edi = u32::MAX;
    pre_slots(c, g)
}

fn pre_slots(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    loop {
        match next_live(c, g)? {
            Scan::Hit => {}
            exhausted => {
                let state = g.u32(PHASE_STATE)?;
                match exhausted {
                    Scan::Negative => c.eax = state,
                    Scan::AtLimit => c.ecx = state,
                    _ => c.edx = state,
                }
                restore(c, g)?;
                set_phase(g, state, false)?;
                c.ebx = pop(c, g)?;
                c.ecx = pop(c, g)?;
                return g.ret(c, 0);
            }
        }
        let stride = g.u32(c.esi.wrapping_add(layout::ELEMENT_SIZE))?;
        let data = g.u32(c.esi.wrapping_add(layout::DATA))?;
        c.ecx = data.wrapping_add(c.eax.wrapping_mul(stride));
        c.edx = (u32::from(g.u16(c.ecx)?) << 16) | c.eax;
        let flags = g.u8(c.ecx.wrapping_add(HEADER_FLAGS))?;
        c.ecx = byte(c.ecx, flags);
        c.edi = c.eax;
        if flags & 11 == 3 {
            c.ecx = g.u32(current_objects(g)?.wrapping_add(layout::DATA))?;
            c.eax = g.u32(c.ecx.wrapping_add(header_offset(c.edx) + OBJECT_PTR))?;
            let attachment = g.u8(c.eax.wrapping_add(PHYSICS_FLAGS))? >> 3;
            c.ecx = byte(c.ecx, attachment);
            if attachment & 1 == 0 {
                logic(c, 0, 0x80);
                return call_arg(c, g, c.edx, PRE_OBJECT, PRE_SITES[2]);
            }
        }
    }
}

pub(super) fn post(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    if pc == POST_SITES[3] {
        c.edx = g.u32(PHASE_STATE)?;
        restore(c, g)?;
        set_phase(g, c.edx, false)?;
        c.ebx = pop(c, g)?;
        return g.ret(c, 0);
    }
    if pc == POST_SITES[0] {
        c.ecx = c.edi;
        c.eax = c.esi;
        return g.call(c, POST_LIST_VISIT, POST_SITES[1]);
    }
    if pc != POST_SITES[2] {
        if pc == points::OBJECTS_POST_PHYSICS.base {
            c.eax = g.u32(LIST_OWNER)?;
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = g.u32(c.eax.wrapping_add(LIST_COUNT))?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.ebx = c.eax.wrapping_add(LIST);
            c.eax = g.u32(PHASE_STATE)?;
            c.edi = 0;
            set_phase(g, c.eax, true)?;
        } else {
            c.edi = c.edi.wrapping_add(1);
        }

        while (c.edi as i32) < c.ebp as i32 {
            c.esi = g.u32(c.ebx.wrapping_add(c.edi.wrapping_mul(4)))?;
            c.edx = current_objects(g)?;
            c.ecx = (c.esi & 0xffff) * 3;
            c.eax = g.u32(c.edx.wrapping_add(layout::DATA))?.wrapping_add(4 * c.ecx);
            let flags = g.u8(c.eax.wrapping_add(HEADER_FLAGS))?;
            c.eax = byte(c.eax, flags);
            if flags & 12 == 4 {
                logic(c, 0, 0x80);
                return call_arg(c, g, c.esi, POST_OBJECT, POST_SITES[0]);
            }
            c.edi = c.edi.wrapping_add(1);
        }
        c.esi = current_objects(g)?;
        c.edi = u32::MAX;
    }

    loop {
        if next_live(c, g)? != Scan::Hit {
            return g.call(c, POST_FINISH, POST_SITES[3]);
        }
        let stride = g.u32(c.esi.wrapping_add(layout::ELEMENT_SIZE))?;
        c.ebp = g.u32(c.esi.wrapping_add(layout::DATA))?;
        c.ecx = c.ebp.wrapping_add(c.eax.wrapping_mul(stride));
        c.edx = (u32::from(g.u16(c.ecx)?) << 16) | c.eax;
        c.edi = c.eax;
        let flags = g.u8(c.ecx.wrapping_add(HEADER_FLAGS))?;
        c.eax = byte(c.eax, flags);
        if flags & 12 == 4 {
            c.eax = g.u32(c.ecx.wrapping_add(OBJECT_PTR))?;
            let attachment = g.u8(c.eax.wrapping_add(PHYSICS_FLAGS))? >> 3;
            c.ecx = byte(c.ecx, attachment);
            if attachment & 1 == 0 {
                logic(c, 0, 0x80);
                return call_arg(c, g, c.edx, POST_OBJECT, POST_SITES[2]);
            }
        }
    }
}

// Unit type descriptor update. The first five callbacks contribute their
// entire low byte. The following four are called for their side effects.
pub(super) const UNIT_SITES: [u32; 9] = [
    0xc6b3c, 0xc6b44, 0xc6b4c, 0xc6b55, 0xc6b5e, 0xc6b67, 0xc6b6e, 0xc6b75, 0xc6b7c,
];

pub(super) fn unit(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    if pc == points::UNIT_UPDATE.base {
        push(c, g, c.ebx)?;
        push(c, g, c.esi)?;
        c.esi = g.u32(c.esp.wrapping_add(12))?;
        return call_arg(c, g, c.esi, 0xc49b0, UNIT_SITES[0]);
    }
    let stage = UNIT_SITES.iter().position(|&site| site == pc).expect("unit resume");
    match stage {
        0 => c.ebx = byte(c.ebx, c.eax as u8),
        1..=4 => {
            c.ebx = byte(c.ebx, (c.ebx | c.eax) as u8);
            logic(c, c.ebx & 0xff, 0x80);
        }
        _ => {}
    }
    match stage {
        0 => call_arg(c, g, c.esi, 0xc58f0, UNIT_SITES[1]),
        1 => call_arg(c, g, c.esi, 0xc60c0, UNIT_SITES[2]),
        8 => {
            c.esi = pop(c, g)?;
            c.eax = byte(c.eax, c.ebx as u8);
            c.ebx = pop(c, g)?;
            g.ret(c, 4)
        }
        _ => {
            let target = match stage {
                2 => 0xc5eb0,
                3 => 0xc6740,
                4 => 0x114c60,
                5 => 0xc6990,
                6 => 0xc6810,
                _ => 0xc50a0,
            };
            c.eax = c.esi;
            g.call(c, target, UNIT_SITES[stage + 1])
        }
    }
}
