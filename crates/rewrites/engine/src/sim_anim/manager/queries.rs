//! Manager queries on the graph handle at +0x68, the current action names,
//! and graph nodes. Results and temporary packed references can alias the
//! guest frame, including across the callbacks that populate them.
use super::*;
use crate::sim_native::{add_flags, logic_flags, sub_flags};
use crate::sim_physics::{
    arithmetic, byte, logic,
    spatial::{finish_fpu, fp_error, load_fpu},
};

pub(super) const BODIES: &[Body] = &[
    body!(0x1cb920, 0x1cb96b, &[0x1cb94c, 0x1cb966], select_mode),
    body!(0x1cb970, 0x1cb9ca, &[0x1cb9ab], lookup_action),
    body!(0x1cb9d0, 0x1cba7d, &[0x1cba4c], lookup_track),
    body!(0x1cba80, 0x1cbac8, &[0x1cba97, 0x1cbabe], resolve_action),
    body!(0x1cbad0, 0x1cbb45, &[0x1cbb12, 0x1cbb35], current_action),
    body!(0x1cbb50, 0x1cbca1, &[0x1cbbf1, 0x1cbc77], choose_transition),
    body!(0x1cbcb0, 0x1cbcdd, &[0x1cbcc9, 0x1cbcd6], begin_transition),
    body!(0x1cbce0, 0x1cbd0e, &[0x1cbd0b], query_action_state),
    body!(0x1cbd10, 0x1cbd2f, &[0x1cbd2c], query_graph_record),
    body!(0x1cbd30, 0x1cbd4b, &[], node_count),
    body!(0x1cbd50, 0x1cbd76, &[0x1cbd73], build_matrices),
    body!(0x1cbd80, 0x1cbdaf, &[0x1cbdac], transform_graph),
    body!(0x1cbdb0, 0x1cbddd, &[0x1cbdda], query_transform),
    body!(0x1cbde0, 0x1cbe12, &[], find_node),
    body!(0x1cbe20, 0x1cbe48, &[], graph_entry),
    body!(0x1cbe50, 0x1cbe8f, &[0x1cbe8b], reference_data),
    body!(0x1cbe90, 0x1cbeb1, &[], signed_node_count),
    body!(
        0x1cbec0,
        0x1cbf43,
        &[0x1cbf03, 0x1cbf28, 0x1cbf3d],
        sample_at_time
    ),
];

fn tag_index(c: &mut CpuAbi, handle: u32) -> u32 {
    let i = (handle & 0xffff) << 4;
    logic_flags(c, i);
    // SHL after zero-extending the datum index: CF is zero; OF undefined.
    c.eflags_defined &= !0x800;
    i
}

fn graph(g: &Bus<'_>, table: u32, index: u32) -> Result<u32, GuestFault> {
    g.u32(at(at(table, index), 8))
}

fn test_al(c: &mut CpuAbi) -> bool {
    logic(c, c.eax & 255, 0x80);
    c.eax as u8 != 0
}

fn release(c: &mut CpuAbi, n: u32) {
    c.esp = arithmetic(c, c.esp, n, false);
}

fn select_mode(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cb920 => {
            push(c, g, c.edi)?;
            c.edi = c.eax;
            c.ecx = g.u32(at(c.edi, 0x68))?;
            c.eax &= !255;
            sub_flags(c, c.ecx, u32::MAX, 32);
            if c.ecx == u32::MAX {
                c.edi = pop(c, g)?;
                return g.ret(c, 0);
            }
            c.eax = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
            push(c, g, c.ebx)?;
            c.ecx = tag_index(c, c.ecx);
            c.ecx = graph(g, c.eax, c.ecx)?;
            push(c, g, 0x14)?;
            push(c, g, c.esi)?;
            c.ecx = arithmetic(c, c.ecx, 0x34, false);
            push(c, g, c.ecx)?;
            return g.call(c, 0x1dd560, 0x1cb94c);
        }
        0x1cb94c => {
            c.ebx = byte(c.ebx, u8::from(c.eax != 0));
            logic(c, c.ebx & 255, 0x80);
            if c.ebx as u8 != 0 {
                push(c, g, 0)?;
                push(c, g, 1)?;
                c.edx = g.u32(0x1cb95a)?;
                push(c, g, c.esi)?;
                c.ecx = c.edx;
                return g.call(c, 0x1ce180, 0x1cb966);
            }
        }
        0x1cb966 => {}
        _ => return Err(bad(pc)),
    }
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    c.edi = pop(c, g)?;
    g.ret(c, 0)
}

fn lookup_action(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cb970 {
        c.esp = c.esp.wrapping_sub(0x28);
        c.ecx = u32::MAX;
        logic_flags(c, c.ecx);
        for offset in [4, 6, 0, 2] {
            g.set_u16(at(c.esp, offset), u16::MAX)?;
        }
        c.ecx = c.esp;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.esp, 0x34))?;
        push(c, g, 0)?;
        push(c, g, c.edx)?;
        c.edx = g.u32(at(c.esp, 0x38))?;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.eax, 0x70))?;
        push(c, g, c.edx)?;
        push(c, g, c.ecx)?;
        c.ecx = at(c.esp, 0x20);
        c.edx = at(c.esp, 0x30);
        return g.call(c, 0x1cc3f0, 0x1cb9ab);
    }
    if test_al(c) {
        c.eax = g.u32(c.esp)?;
        g.set_u32(c.esi, c.eax)?;
    } else {
        c.edx = g.u32(at(c.esp, 4))?;
        g.set_u32(c.esi, c.edx)?;
    }
    c.eax = c.esi;
    release(c, 0x28);
    g.ret(c, 8)
}

fn lookup_track(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    let value = if pc == 0x1cb9d0 {
        c.esp = c.esp.wrapping_sub(0x24);
        push(c, g, c.esi)?;
        c.esi = c.eax;
        c.ecx = g.u32(at(c.esi, 0x68))?;
        c.eax = u32::MAX;
        sub_flags(c, c.ecx, c.eax, 32);
        g.set_u16(at(c.esp, 4), u16::MAX)?;
        g.set_u16(at(c.esp, 6), u16::MAX)?;
        if c.ecx != u32::MAX {
            c.xmm[0] = [0; 16];
            c.ecx = 0;
            logic_flags(c, 0);
            g.set_u32(at(c.esp, 8), c.eax)?;
            g.set_u16(at(c.esp, 12), u16::MAX)?;
            g.set_u16(at(c.esp, 14), u16::MAX)?;
            g.set_u32(at(c.esp, 16), c.eax)?;
            g.set_u16(at(c.esp, 22), u16::MAX)?;
            g.set_u8(at(c.esp, 20), 255)?;
            g.set_u8(at(c.esp, 21), 255)?;
            push(c, g, 0)?;
            c.eax = at(c.esp, 12);
            g.set_u32(at(c.esp, 40), 0)?;
            c.xmm[0] = u128::from(g.u32(0x45dbc0)?).to_le_bytes();
            g.set_u8(at(c.esp, 28), 0)?;
            g.set_u8(at(c.esp, 29), 0)?;
            for offset in [30, 32, 34] {
                g.set_u16(at(c.esp, offset), 0)?;
            }
            c.ecx = g.u32(at(c.esp, 48))?;
            push(c, g, c.eax)?;
            g.set_u32(at(c.esp, 40), u128::from_le_bytes(c.xmm[0]) as u32)?;
            return g.call(c, 0x1cb520, 0x1cba4c);
        }
        c.edx = g.u32(at(c.esp, 4))?;
        c.edx
    } else if test_al(c) {
        c.eax = g.u32(at(c.esp, 12))?;
        c.eax
    } else {
        c.ecx = g.u32(at(c.esp, 4))?;
        c.ecx
    };
    g.set_u32(c.ebx, value)?;
    c.eax = c.ebx;
    c.esi = pop(c, g)?;
    release(c, 0x24);
    g.ret(c, 4)
}

fn resolve_action(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cba80 {
        push(c, g, c.edi)?;
        push(c, g, c.eax)?;
        c.eax = g.u32(at(c.esp, 16))?;
        c.ecx = at(c.esp, 16);
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.esp, 16))?;
        push(c, g, c.esi)?;
        c.edi = 0;
        logic_flags(c, 0);
        return g.call(c, 0x1cbad0, 0x1cba97);
    }
    if pc == 0x1cba97 {
        let index = u32::from(g.u16(at(c.esp, 14))?);
        sub_flags(c, index, 0xffff, 16);
        if index != 0xffff {
            c.edx = g.u32(at(c.esi, 0x68))?;
            c.eax = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
            c.ecx = g.u32(at(c.esp, 12))?;
            c.edx = tag_index(c, c.edx);
            c.edx = graph(g, c.eax, c.edx)?;
            push(c, g, c.ecx)?;
            return g.call(c, 0x1daea0, 0x1cbabe);
        }
        c.eax = c.edi;
    }
    c.edi = pop(c, g)?;
    g.ret(c, 8)
}

fn current_action(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1cbad0 => {
            push(c, g, c.ebp)?;
            c.ebp = g.u32(at(c.esp, 16))?;
            push(c, g, c.esi)?;
            c.esi = c.ecx;
            push(c, g, c.edi)?;
            c.edi = c.eax;
            c.eax = g.u32(at(c.esp, 16))?;
            if c.esi == g.u32(0x1cbada)? || c.esi == u32::MAX {
                c.esi = g.u32(at(c.eax, 0x74))?;
            }
            let default = g.u32(0x1cbaf7)?;
            sub_flags(c, c.ebp, default, 32);
            if c.ebp != default {
                sub_flags(c, c.ebp, u32::MAX, 32);
            }
            if c.ebp == default || c.ebp == u32::MAX {
                c.ebp = g.u32(at(c.eax, 0x78))?;
            }
            push(c, g, c.ebx)?;
            push(c, g, c.esi)?;
            c.edx = c.ebp;
            c.ebx = at(c.esp, 0x20);
            return g.call(c, 0x1cb9d0, 0x1cbb12);
        }
        0x1cbb12 => {
            c.eax = g.u32(c.eax)?;
            c.ecx = c.eax >> 16;
            sub_flags(c, c.ecx, 0xffff, 16);
            g.set_u32(at(c.esp, 28), c.eax)?;
            c.ebx = pop(c, g)?;
            if c.ecx == 0xffff {
                c.eax = g.u32(at(c.esp, 16))?;
                push(c, g, c.ebp)?;
                push(c, g, c.esi)?;
                c.edx = c.edi;
                c.esi = at(c.esp, 24);
                return g.call(c, 0x1cb970, 0x1cbb35);
            }
        }
        0x1cbb35 => c.eax = g.u32(c.eax)?,
        _ => return Err(bad(pc)),
    }
    c.ecx = g.u32(at(c.esp, 20))?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    g.set_u32(c.ecx, c.eax)?;
    c.eax = c.ecx;
    c.ebp = pop(c, g)?;
    g.ret(c, 12)
}

fn inc_index(c: &mut CpuAbi, n: u32) -> u32 {
    let carry = c.eflags & 1;
    let defined = c.eflags_defined & 1;
    add_flags(c, n, 1);
    c.eflags = (c.eflags & !1) | carry;
    c.eflags_defined = (c.eflags_defined & !1) | defined;
    n.wrapping_add(1)
}

fn lookup_transition(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.eax = at(c.esp, 0x1c);
    push(c, g, c.eax)?;
    c.eax = g.u16(at(c.esp, 0x44))? as i16 as u32;
    c.ecx = at(c.esp, 0x24);
    push(c, g, c.ecx)?;
    c.ecx = g.u32(at(c.esp, 0x44))?;
    c.edx = at(c.esp, 0x2c);
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esi, 0x78))?;
    c.edi = c.ebp as i16 as u32;
    push(c, g, c.edi)?;
    push(c, g, c.eax)?;
    push(c, g, c.ecx)?;
    c.ecx = g.u32(at(c.esp, 0x2c))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esi, 0x70))?;
    c.eax = at(c.esp, 0x44);
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.esi, 0x74))?;
    push(c, g, c.ecx)?;
    c.ebx = inc_index(c, c.ebx);
    g.call(c, 0x1dc790, 0x1cbbf1)
}

fn finish_transition(c: &mut CpuAbi, g: &mut Bus<'_>, first_popped: bool) -> sim_native::Result {
    if !first_popped {
        c.edi = pop(c, g)?;
    }
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    release(c, 40);
    g.ret(c, 24)
}

fn choose_transition(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbc77 {
        c.ecx = g.u32(at(c.esp, 0x48))?;
        if c.ecx != 0 {
            g.set_u16(c.ecx, c.edi as u16)?;
        }
        c.ecx = g.u32(at(c.esp, 0x4c))?;
        logic_flags(c, c.ecx);
        if c.ecx != 0 {
            c.edi = pop(c, g)?;
            g.set_u16(c.ecx, c.ebp as u16)?;
            return finish_transition(c, g, true);
        }
        return finish_transition(c, g, false);
    }

    if pc == 0x1cbb50 {
        c.esp = c.esp.wrapping_sub(40);
        c.ecx = g.u32(at(c.esi, 0x68))?;
        c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        push(c, g, c.ebx)?;
        c.ecx = tag_index(c, c.ecx);
        c.ecx = graph(g, c.edx, c.ecx)?;
        c.edx = g.u32(at(c.esp, 0x38))?;
        push(c, g, c.ebp)?;
        c.ebp = g.u32(at(c.esp, 0x40))?;
        c.eax |= 0xffff;
        g.set_u32(at(c.esp, 0x10), c.ecx)?;
        push(c, g, c.edi)?;
        g.set_u16(at(c.esp, 0x2c), 0xffff)?;
        g.set_u16(at(c.esp, 0x2e), 0xffff)?;
        g.set_u32(at(c.esp, 0x18), c.edx)?;
        g.set_u32(at(c.esp, 0x0c), c.ebp)?;
        g.set_u8(at(c.esp, 0x44), 1)?;
        c.ecx = 0;
    } else {
        c.edx = g.u32(c.eax)?;
        c.eax = c.edx >> 16;
        g.set_u32(at(c.esp, 0x2c), c.edx)?;
        if c.eax as u16 == u16::MAX {
            let next = g.u16(at(0x46fc18, c.edi.wrapping_mul(2)))?;
            c.ebp = (c.ebp & !0xffff) | u32::from(next);
            if next != g.u16(at(c.esp, 0x0c))? {
                sub_flags(c, c.ebx, 11, 32);
                if (c.ebx as i32) < 11 {
                    return lookup_transition(c, g);
                }
            }
            c.ecx = g.u16(at(c.esp, 0x40))? as i16 as u32;
            c.ecx = g.u16(at(0x4454a0, c.ecx.wrapping_mul(2)))? as i16 as u32;
            let same = c.ecx as u16 == g.u16(at(c.esp, 0x18))?;
            g.set_u32(at(c.esp, 0x40), c.ecx)?;
            if same {
                c.eax &= !255;
                return finish_transition(c, g, false);
            }
        } else {
            g.set_u8(at(c.esp, 0x44), 0)?;
        }
        c.ecx = byte(c.ecx, g.u8(at(c.esp, 0x44))?);
        if c.ecx as u8 == 0 {
            return apply_transition(c, g);
        }
        c.ecx = g.u32(at(c.esp, 0x10))?;
    }

    if (c.ecx as i32) < 4 {
        c.ebp = g.u32(at(c.esp, 0x0c))?;
        c.ecx = c.ecx.wrapping_add(1);
        g.set_u32(at(c.esp, 0x10), c.ecx)?;
        c.ebx = 0;
        sub_flags(c, c.ebx, 11, 32);
        return lookup_transition(c, g);
    }
    apply_transition(c, g)
}

fn apply_transition(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    if c.eax as u16 == u16::MAX {
        c.eax &= !255;
        return finish_transition(c, g, false);
    }
    c.eax = g.u32(at(c.esp, 0x3c))?;
    let alternate = g.u32(0x1cbc4d)?;
    sub_flags(c, c.eax, alternate, 32);
    c.ebx = g.u32(if c.eax == alternate {
        0x1cbc59
    } else {
        0x1cbc52
    })?;
    c.edi = g.u32(at(c.esp, 0x40))?;
    c.edx = g.u32(at(c.esp, 0x2c))?;
    for v in [2, c.ebp, c.edi, c.eax] {
        push(c, g, v)?;
    }
    c.eax = g.u32(at(c.esp, 0x48))?;
    push(c, g, c.edx)?;
    push(c, g, c.eax)?;
    c.eax = c.esi;
    g.call(c, 0x1cb410, 0x1cbc77)
}

fn begin_transition(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbcb0 {
        push(c, g, c.ebx)?;
        push(c, g, c.esi)?;
        c.esi = g.u32(at(c.esp, 12))?;
        push(c, g, c.eax)?;
        c.eax = g.u32(at(c.esp, 24))?;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.esp, 24))?;
        for v in [c.edx, c.eax, c.ecx, c.esi] {
            push(c, g, v)?;
        }
        return g.call(c, 0x1cbb50, 0x1cbcc9);
    }
    if pc == 0x1cbcc9 {
        c.ebx = byte(c.ebx, c.eax as u8);
        logic(c, c.ebx & 255, 0x80);
        if c.ebx as u8 != 0 {
            c.eax = c.esi;
            return g.call(c, 0x1ccc50, 0x1cbcd6);
        }
    }
    c.esi = pop(c, g)?;
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    g.ret(c, 12)
}

fn query_action_state(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbce0 {
        c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        push(c, g, c.ebx)?;
        push(c, g, c.esi)?;
        c.esi = c.ecx;
        let h = g.u32(at(c.eax, 0x68))?;
        c.ecx = tag_index(c, h);
        c.ebx = graph(g, c.edx, c.ecx)?;
        c.ecx = g.u32(at(c.eax, 0x70))?;
        g.set_u32(at(c.esi, 12), c.ecx)?;
        c.edx = g.u32(at(c.eax, 0x74))?;
        g.set_u32(at(c.esi, 16), c.edx)?;
        return g.call(c, 0x1dcfa0, 0x1cbd0b);
    }
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 0)
}

fn query_graph_record(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbd10 {
        c.edx = g.u32(at(c.ecx, 0x68))?;
        push(c, g, c.eax)?;
        c.eax = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        c.edx = tag_index(c, c.edx);
        c.edx = graph(g, c.eax, c.edx)?;
        push(c, g, c.esi)?;
        return g.call(c, 0x1dd0b0, 0x1cbd2c);
    }
    c.eax = c.esi;
    g.ret(c, 0)
}

fn node_count(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> sim_native::Result {
    c.ecx = g.u32(at(c.eax, 0x68))?;
    c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
    c.ecx = tag_index(c, c.ecx);
    c.eax = graph(g, c.edx, c.ecx)?;
    c.eax = (c.eax & !0xffff) | u32::from(g.u16(at(c.eax, 12))?);
    g.ret(c, 0)
}

fn build_matrices(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbd50 {
        push(c, g, c.eax)?;
        c.eax = g.u32(at(c.esp, 8))?;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.eax, 0x68))?;
        push(c, g, c.edx)?;
        c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        c.ecx = tag_index(c, c.ecx);
        c.eax = graph(g, c.edx, c.ecx)?;
        push(c, g, c.eax)?;
        return g.call(c, 0x1dd1c0, 0x1cbd73);
    }
    g.ret(c, 4)
}

fn transform_graph(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbd80 {
        push(c, g, c.eax)?;
        c.eax = g.u32(at(c.esp, 16))?;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.esp, 16))?;
        push(c, g, c.edx)?;
        c.edx = g.u32(at(c.esp, 16))?;
        push(c, g, c.eax)?;
        c.eax = g.u32(at(c.edx, 0x68))?;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        c.eax = tag_index(c, c.eax);
        c.edx = graph(g, c.ecx, c.eax)?;
        push(c, g, c.edx)?;
        return g.call(c, 0x1dd290, 0x1cbdac);
    }
    g.ret(c, 12)
}

fn query_transform(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbdb0 {
        c.ecx = g.u32(at(c.eax, 0x68))?;
        c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        c.ecx = tag_index(c, c.ecx);
        c.eax = graph(g, c.edx, c.ecx)?;
        c.ecx = g.u32(at(c.esp, 12))?;
        c.edx = g.u32(at(c.esp, 8))?;
        for v in [c.ecx, c.edx, c.eax] {
            push(c, g, v)?;
        }
        c.eax = g.u32(at(c.esp, 16))?;
        return g.call(c, 0x1dd4c0, 0x1cbdda);
    }
    g.ret(c, 12)
}

fn find_node(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> sim_native::Result {
    c.ecx = g.u32(at(c.eax, 0x68))?;
    c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
    c.ecx = tag_index(c, c.ecx);
    c.edx = graph(g, c.edx, c.ecx)?;
    c.ecx = g.u32(at(c.edx, 12))?;
    c.eax = 0;
    if (c.ecx as i32) > 0 {
        c.edx = g.u32(at(c.edx, 16))?;
        while c.eax < c.ecx {
            let name = g.u32(c.edx)?;
            sub_flags(c, name, c.esi, 32);
            if name == c.esi {
                return g.ret(c, 0);
            }
            c.eax += 1;
            c.edx = at(c.edx, 32);
            // A malformed count must not stall an entire native segment.
            if c.eax == 0x10000 {
                return Err(bad(0x1cbde0));
            }
        }
    }
    c.eax = u32::MAX;
    logic_flags(c, c.eax);
    g.ret(c, 0)
}

fn graph_entry(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> sim_native::Result {
    if c.eax == u32::MAX {
        c.eax = 0;
        logic_flags(c, 0);
    } else {
        c.edx = g.u32(at(c.ecx, 0x68))?;
        c.ecx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        c.edx = tag_index(c, c.edx);
        c.edx = graph(g, c.ecx, c.edx)?;
        c.ecx = g.u32(at(c.edx, 24))?;
        c.eax = at(c.ecx, c.eax.wrapping_mul(12));
    }
    g.ret(c, 0)
}

fn reference_data(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbe50 {
        c.ecx = g.u32(at(c.esp, 8))? >> 16;
        c.eax = 0;
        sub_flags(c, c.ecx, 0xffff, 16);
        if c.ecx == 0xffff {
            return g.ret(c, 8);
        }
        c.ecx = g.u32(at(c.esp, 8))?;
        sub_flags(c, c.ecx, 0xffff, 16);
        if c.ecx as u16 == u16::MAX {
            return g.ret(c, 8);
        }
        c.eax = g.u32(at(c.esp, 4))?;
        c.eax = g.u32(at(c.eax, 0x68))?;
        c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        c.eax = tag_index(c, c.eax);
        c.eax = graph(g, c.edx, c.eax)?;
        push(c, g, c.edi)?;
        push(c, g, c.ecx)?;
        c.edi = c.eax;
        return g.call(c, 0x1daff0, 0x1cbe8b);
    }
    c.edi = pop(c, g)?;
    g.ret(c, 8)
}

fn signed_node_count(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> sim_native::Result {
    c.edx = g.u32(at(c.ecx, 0x68))?;
    c.ecx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
    c.edx = tag_index(c, c.edx);
    c.ecx = graph(g, c.ecx, c.edx)?;
    c.eax = 0;
    logic_flags(c, c.ecx);
    if c.ecx != 0 {
        c.eax = g.u16(at(c.ecx, 12))? as i16 as u32;
    }
    g.ret(c, 0)
}

fn sample_at_time(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1cbec0 {
        let mut x = load_fpu(c);
        x.fld(g.u32(at(c.esp, 8))?).map_err(fp_error)?;
        push(c, g, c.ebx)?;
        x.fmul_m(g.u32(0x45dcfc)?).map_err(fp_error)?;
        push(c, g, c.ebp)?;
        c.ebp = g.u32(at(c.esp, 20))?;
        push(c, g, c.edi)?;
        c.edi = g.u32(at(c.esp, 16))?;
        g.set_u32(at(c.esp, 24), x.fst_m32().map_err(fp_error)?)?;
        x.unary(x87::abi::unary_op::ABS).map_err(fp_error)?;
        x.fld(g.u32(0x45dbdc)?).map_err(fp_error)?;
        let flags = x.fcomip(1).map_err(fp_error)?;
        x.fstp_st(0).map_err(fp_error)?;
        finish_fpu(c, g, x, 0x1cbee3, 0x45dbdc)?;
        c.eflags = (c.eflags & !0x8d5) | flags;
        c.eflags_defined |= 0x8d5;
        if flags & 0x41 == 0 {
            c.eax = g.u32(at(c.esi, 0x68))?;
            c.ecx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
            c.eax = tag_index(c, c.eax);
            c.ebx = graph(g, c.ecx, c.eax)?;
            push(c, g, c.ebp)?;
            push(c, g, c.edi)?;
            return g.call(c, 0x1dd8f0, 0x1cbf03);
        }
    }
    if pc == 0x1cbec0 || (pc == 0x1cbf03 && !test_al(c)) {
        c.edx = g.u32(at(c.esi, 0x68))?;
        c.eax = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
        push(c, g, 1)?;
        push(c, g, c.ebp)?;
        push(c, g, 0)?;
        c.edx &= 0xffff;
        push(c, g, c.edi)?;
        c.edx = tag_index(c, c.edx);
        c.ebx = graph(g, c.eax, c.edx)?;
        push(c, g, c.esi)?;
        return g.call(c, 0x1cbe50, 0x1cbf28);
    }
    if pc == 0x1cbf28 {
        c.ecx = g.u32(at(c.esp, 36))?;
        push(c, g, c.eax)?;
        let weight = g.u32(0x1cbf2e)?;
        for v in [weight, c.ecx, 1, c.edi] {
            push(c, g, v)?;
        }
        c.eax = c.ebx;
        return g.call(c, 0x279d80, 0x1cbf3d);
    }
    c.edi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 12)
}
