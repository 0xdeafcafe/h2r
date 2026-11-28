//! Action selection and resource prediction (1cc3f0). A successful selection
//! walks mode/class/type matches in the graph and then each inherited graph.
//! The five fallback combinations use distinct, resumable cache requests.
use super::*;
use guest_layout::points::TAG_ENTRIES;
use crate::sim_native::sub_flags;
use crate::sim_physics::{arithmetic, byte, logic};

const ACTION: u32 = 0x1db170;
const MODE: u32 = 0x1db120;
const SEARCH: u32 = 0x1dd560;
const REQUEST: u32 = 0x1236f0;
const ENTRY: u32 = 0x1cc3f0;
const SITES: &[u32] = &[
    0x1cc431, 0x1cc471, 0x1cc4b3, 0x1cc505, 0x1cc528, 0x1cc552, 0x1cc57a, 0x1cc5a8,
    // For each choice: mode, class, type, cache request.
    0x1cc5f5, 0x1cc609, 0x1cc61d, 0x1cc650, 0x1cc665, 0x1cc679, 0x1cc68d, 0x1cc6c0, 0x1cc6dd,
    0x1cc6f1, 0x1cc705, 0x1cc734, 0x1cc74d, 0x1cc761, 0x1cc775, 0x1cc7a4, 0x1cc7bd, 0x1cc7d1,
    0x1cc7e5, 0x1cc814, 0x1cc87b, 0x1cc88f, 0x1cc8a3, 0x1cc8d2, 0x1cc8ed, 0x1cc901, 0x1cc915,
    0x1cc944, 0x1cc95d, 0x1cc971, 0x1cc985, 0x1cc9b4, 0x1cc9cd, 0x1cc9e1, 0x1cc9f5, 0x1cca24,
    0x1cca3d, 0x1cca51, 0x1cca65, 0x1cca94,
];
pub(super) const BODIES: &[Body] = &[body!(0x1cc3f0, 0x1ccad1, SITES, select_action)];

type Checked<T> = std::result::Result<T, GuestFault>;

fn stack(c: &CpuAbi, g: &Bus<'_>, off: u32) -> Checked<u32> {
    g.u32(at(c.esp, off))
}

// The handle's low word selects a tag entry. The SHL by four leaves OF
// undefined and shifts out a zero bit.
fn slot(c: &mut CpuAbi, handle: u32) -> u32 {
    let offset = (handle & 0xffff) << 4;
    logic(c, offset, 1 << 31);
    c.eflags_defined &= !(1 << 11);
    offset
}

fn begin(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.esp = c.esp.wrapping_sub(12);
    for saved in [c.ebx, c.ebp, c.esi, c.edi] {
        push(c, g, saved)?;
    }
    c.edi = c.ecx;
    c.ecx = g.u32(at(c.eax, 0x68))? & 0xffff;
    c.esi = c.edx;
    c.edx = g.u32(TAG_ENTRIES.base)?;
    c.ecx = slot(c, c.ecx);
    c.ecx = g.u32(at(at(c.ecx, c.edx), 8))?;
    c.edx = stack(c, g, 0x2c)?;
    push(c, g, c.edx)?;
    c.edx = stack(c, g, 0x2c)?;
    g.set_u32(at(c.esp, 0x18), c.ecx)?;
    c.ecx = stack(c, g, 0x28)?;
    push(c, g, c.eax)?;
    c.eax = stack(c, g, 0x28)?;
    g.set_u8(at(c.esp, 0x1b), 0)?;
    g.call(c, 0x1cc2f0, 0x1cc431)
}

fn first_action(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.eax = c.esi;
    c.ecx = c.edi;
    for off in [0, 4, 8] {
        c.edx = g.u32(at(c.eax, off))?;
        g.set_u32(at(c.ecx, off), c.edx)?;
    }
    c.eax = g.u32(at(c.eax, 12))?;
    g.set_u32(at(c.ecx, 12), c.eax)?;
    c.ecx = g.u32(at(c.esi, 8))?;
    c.edx = g.u32(at(c.esi, 4))?;
    c.eax = g.u32(c.esi)?;
    c.ebp = at(c.edi, 8);
    c.ebx = at(c.edi, 4);
    for v in [c.ebp, c.ebx, c.edi, c.ecx, c.edx] {
        push(c, g, v)?;
    }
    c.edx = stack(c, g, 0x28)?;
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.esi, 12))?;
    c.ecx = at(c.esp, 0x38);
    push(c, g, c.ecx)?;
    push(c, g, c.edx)?;
    g.call(c, ACTION, 0x1cc471)
}

fn default_action(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.edx = g.u32(at(c.esi, 8))?;
    c.eax = g.u32(at(c.esi, 4))?;
    c.ecx = g.u32(c.esi)?;
    for v in [c.ebp, c.ebx, c.edi, c.edx, c.eax] {
        push(c, g, v)?;
    }
    c.eax = stack(c, g, 0x28)?;
    push(c, g, c.ecx)?;
    c.edx = at(c.esp, 0x30);
    push(c, g, c.edx)?;
    push(c, g, c.eax)?;
    c.eax = g.u32(0x1cc4aa)?;
    g.call(c, ACTION, 0x1cc4b3)
}

fn finish(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.eax = byte(c.eax, g.u8(at(c.esp, 0x13))?);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 12, false);
    g.ret(c, 24)
}

fn selected(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.eax = g.u32(c.ebx)?;
    c.edi = g.u32(c.edi)?;
    c.ebx = stack(c, g, 0x14)?;
    c.edx = g.u32(c.ebp)?;
    push(c, g, 0x14)?;
    push(c, g, c.edi)?;
    c.ebx = arithmetic(c, c.ebx, 0x34, false);
    push(c, g, c.ebx)?;
    for (off, v) in [(0x38, c.edx), (0x3c, c.eax), (0x40, c.edi)] {
        g.set_u32(at(c.esp, off), v)?;
    }
    g.call(c, SEARCH, 0x1cc5f5)
}

fn check_selected(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    let yes = g.u8(at(c.esp, 0x13))?;
    c.eax = byte(c.eax, yes);
    logic(c, u32::from(yes), 0x80);
    if yes == 0 {
        finish(c, g)
    } else {
        selected(c, g)
    }
}

// 1db170 returns a pointer to a packed animation reference; the caller
// copies it and checks the signed animation index in the high word.
fn action_result(c: &mut CpuAbi, g: &mut Bus<'_>, first: bool) -> sim_native::Result {
    c.ecx = g.u32(c.eax)?;
    c.eax = stack(c, g, 0x34)?;
    g.set_u32(c.eax, c.ecx)?;
    let index = u32::from(g.u16(at(c.eax, 2))?);
    sub_flags(c, index, 0xffff, 16);
    if index != 0xffff {
        if !first {
            g.set_u32(at(c.edi, 12), g.u32(0x1cc4c5)?)?;
        }
        g.set_u8(at(c.esp, 0x13), 1)?;
        if first {
            return selected(c, g);
        }
    } else if first {
        let option = g.u8(at(c.esp, 0x30))? & 2;
        logic(c, u32::from(option), 0x80);
        if option != 0 {
            return default_action(c, g);
        }
    }

    c.edx = stack(c, g, 0x34)?;
    let index = u32::from(g.u16(at(c.edx, 2))?);
    sub_flags(c, index, 0xffff, 16);
    if index == 0xffff {
        let option = g.u8(at(c.esp, 0x30))? & 4;
        logic(c, u32::from(option), 0x80);
        if option != 0 {
            c.eax = g.u32(at(c.esi, 8))?;
            c.ecx = g.u32(at(c.esi, 4))?;
            c.esi = g.u32(c.esi)?;
            push(c, g, c.eax)?;
            g.set_u32(at(c.esp, 0x34), c.eax)?;
            g.set_u32(at(c.esp, 0x38), c.ecx)?;
            push(c, g, c.ecx)?;
            c.ecx = stack(c, g, 0x1c)?;
            c.eax = c.esi;
            return g.call(c, MODE, 0x1cc505);
        }
    }
    check_selected(c, g)
}

fn mode_result(c: &mut CpuAbi, g: &mut Bus<'_>, site: u32) -> sim_native::Result {
    logic(c, c.eax, 1 << 31);
    if c.eax != 0 {
        g.set_u8(at(c.esp, 0x13), 1)?;
        // Each fallback replaces the names it wildcarded in the output.
        // Read the operand at the store, including when two fields alias.
        match site {
            0x1cc505 => (),
            0x1cc528 => g.set_u32(c.ebp, g.u32(0x1cc534)?)?,
            0x1cc552 => g.set_u32(c.ebx, g.u32(0x1cc55d)?)?,
            0x1cc57a => {
                g.set_u32(c.ebx, g.u32(0x1cc585)?)?;
                g.set_u32(c.ebp, g.u32(0x1cc58c)?)?;
            }
            0x1cc5a8 => {
                g.set_u32(c.edi, g.u32(0x1cc5b3)?)?;
                g.set_u32(c.ebx, g.u32(0x1cc5b9)?)?;
                g.set_u32(c.ebp, g.u32(0x1cc5c0)?)?;
            }
            _ => return Err(bad(site)),
        }
        return selected(c, g);
    }

    let next = match site {
        0x1cc505 => {
            c.eax = stack(c, g, 0x34)?;
            c.ecx = stack(c, g, 0x14)?;
            push_operand(c, g, 0x1cc51c)?;
            push(c, g, c.eax)?;
            c.eax = c.esi;
            0x1cc528
        }
        0x1cc528 => {
            c.ecx = stack(c, g, 0x30)?;
            push(c, g, c.ecx)?;
            c.ecx = stack(c, g, 0x18)?;
            push_operand(c, g, 0x1cc547)?;
            c.eax = c.esi;
            0x1cc552
        }
        0x1cc552 => {
            c.eax = c.esi;
            c.esi = stack(c, g, 0x14)?;
            push_operand(c, g, 0x1cc56a)?;
            push_operand(c, g, 0x1cc56f)?;
            c.ecx = c.esi;
            0x1cc57a
        }
        0x1cc57a => {
            push_operand(c, g, 0x1cc593)?;
            push_operand(c, g, 0x1cc598)?;
            c.eax = g.u32(0x1cc59d)?;
            c.ecx = c.esi;
            0x1cc5a8
        }
        0x1cc5a8 => return check_selected(c, g),
        _ => return Err(bad(site)),
    };
    g.call(c, MODE, next)
}

fn site_for(choice: usize, step: usize) -> u32 {
    SITES[8 + 4 * choice + step]
}

// The primary and inherited graphs use the same five combinations. The
// three searches follow mode -> weapon class -> weapon type. Keys that
// aren't wildcarded live in the caller's frame across all three calls.
fn search_mode(c: &mut CpuAbi, g: &mut Bus<'_>, choice: usize) -> sim_native::Result {
    let site = site_for(choice, 0);
    let block = if choice < 5 { c.ebx } else { c.ebp };
    let form = choice % 5;
    if form == 4 {
        push(c, g, 0x14)?;
        push_operand(c, g, site - 10)?;
    } else {
        let mode = stack(c, g, 0x34)?;
        if form == 1 {
            c.eax = mode;
        } else {
            c.edx = mode;
        }
        push(c, g, 0x14)?;
        push(c, g, mode)?;
    }
    push(c, g, block)?;
    g.call(c, SEARCH, site)
}

fn search_child(c: &mut CpuAbi, g: &mut Bus<'_>, choice: usize, step: usize) -> sim_native::Result {
    let form = choice % 5;
    let exact = if step == 1 {
        form < 2
    } else {
        form == 0 || form == 2
    };
    let stride = if step == 1 { 0x14 } else { 0x34 };
    let site = site_for(choice, step);
    if exact {
        let offset = if step == 1 { 0x30 } else { 0x2c };
        let key = stack(c, g, offset)?;
        if step == 2 && form == 0 {
            c.edx = key;
        } else {
            c.ecx = key;
        }
        push(c, g, stride)?;
        push(c, g, key)?;
    } else {
        push(c, g, stride)?;
        push_operand(c, g, site - 13)?;
    }
    c.eax = arithmetic(c, c.eax, 4, false);
    push(c, g, c.eax)?;
    g.call(c, SEARCH, site)
}

fn push_operand(c: &mut CpuAbi, g: &mut Bus<'_>, addr: u32) -> Checked<()> {
    let name = g.u32(addr)?;
    push(c, g, name)
}

// edi indexes a list of resource groups. The cache call can change the
// list and its count. Reload both at the same places as the guest.
fn resources(
    c: &mut CpuAbi,
    g: &mut Bus<'_>,
    choice: usize,
    after_request: bool,
) -> sim_native::Result {
    let form = choice % 5;
    if !after_request {
        c.esi = c.eax;
        logic(c, c.esi, 1 << 31);
        if c.esi == 0 {
            return next_choice(c, g, choice, false);
        }
        c.eax = g.u32(at(c.esi, 0x24))?;
        c.edi = 0;
        logic(c, c.eax, 1 << 31);
        if c.eax as i32 <= 0 {
            return next_choice(c, g, choice, false);
        }
        if choice <= 1 {
            c.ebp = stack(c, g, 0x14)?;
        }
    }
    let mut advance = after_request;
    // Leave excessive lists to the original at this segment's selector.
    for _ in 0..4096 {
        if advance {
            c.eax = g.u32(at(c.esi, 0x24))?;
            c.edi = c.edi.wrapping_add(1);
            arithmetic(c, c.edi, c.eax, true);
            if c.edi as i32 >= c.eax as i32 {
                return next_choice(c, g, choice, true);
            }
        }
        let graph = if choice < 5 { c.ebp } else { c.ebx };
        let list = g.u32(at(c.esi, 0x28))?;
        let item = g.u32(at(list, c.edi.wrapping_mul(4)))?;
        let groups = g.u32(at(graph, 0xb0))?;
        let scale = item.wrapping_mul(5);
        if form == 1 || form == 2 {
            c.edx = list;
            c.ecx = groups;
        } else {
            c.edx = groups;
            c.ecx = scale;
        }
        c.eax = at(groups, scale.wrapping_mul(4));
        let active = g.u8(at(c.eax, 0x10))?;
        c.ecx = byte(c.ecx, active);
        logic(c, u32::from(active), 0x80);
        if active != 0 {
            c.ecx = byte(c.ecx, 1);
            return g.call(c, REQUEST, site_for(choice, 3));
        }
        advance = true;
    }
    Err(bad(c.eip))
}

fn next_choice(
    c: &mut CpuAbi,
    g: &mut Bus<'_>,
    choice: usize,
    scanned: bool,
) -> sim_native::Result {
    match choice {
        4 => {
            c.eax = g.u32(at(c.ebp, 0x4c))?;
            c.ecx = 0;
            arithmetic(c, c.eax, c.ecx, true);
            g.set_u32(at(c.esp, 0x28), 0)?;
            if c.eax as i32 <= 0 {
                return finish(c, g);
            }
            g.set_u32(at(c.esp, 0x24), 0)?;
            inherited(c, g)
        }
        9 => {
            c.ecx = stack(c, g, 0x28)?;
            c.esi = stack(c, g, 0x24)?;
            c.eax = stack(c, g, 0x14)?;
            c.eax = g.u32(at(c.eax, 0x4c))?;
            c.ecx = c.ecx.wrapping_add(1);
            c.esi = at(c.esi, 32);
            arithmetic(c, c.ecx, c.eax, true);
            g.set_u32(at(c.esp, 0x28), c.ecx)?;
            g.set_u32(at(c.esp, 0x24), c.esi)?;
            if (c.ecx as i32) < c.eax as i32 {
                c.ebp = stack(c, g, 0x14)?;
                inherited(c, g)
            } else {
                finish(c, g)
            }
        }
        _ => {
            if choice == 1 && !scanned {
                c.ebp = stack(c, g, 0x14)?;
            }
            search_mode(c, g, choice + 1)
        }
    }
}

fn inherited(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.ecx = stack(c, g, 0x28)?;
    c.ebx = 0;
    arithmetic(c, c.ecx, c.eax, true);
    if (c.ecx as i32) < c.eax as i32 {
        c.eax = g.u32(at(c.ebp, 0x50))?;
        let offset = stack(c, g, 0x24)?;
        let links = arithmetic(c, c.eax, offset, false);
        c.eax = g.u32(at(links, 4))?;
        arithmetic(c, c.eax, u32::MAX, true);
        if c.eax != u32::MAX {
            c.ecx = g.u32(TAG_ENTRIES.base)?;
            c.eax = slot(c, c.eax);
            c.ebx = g.u32(at(at(c.eax, c.ecx), 8))?;
        }
    }
    c.edx = stack(c, g, 0x34)?;
    push(c, g, 0x14)?;
    push(c, g, c.edx)?;
    c.ebp = at(c.ebx, 0x34);
    push(c, g, c.ebp)?;
    g.call(c, SEARCH, site_for(5, 0))
}

fn select_action(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        ENTRY => begin(c, g),
        0x1cc431 => first_action(c, g),
        0x1cc471 | 0x1cc4b3 => action_result(c, g, pc == 0x1cc471),
        0x1cc505 | 0x1cc528 | 0x1cc552 | 0x1cc57a | 0x1cc5a8 => mode_result(c, g, pc),
        _ => {
            let index = SITES[8..].binary_search(&pc).map_err(|_| bad(pc))?;
            let (choice, step) = (index / 4, index % 4);
            if step >= 2 {
                return resources(c, g, choice, step == 3);
            }
            logic(c, c.eax, 1 << 31);
            if c.eax == 0 {
                next_choice(c, g, choice, false)
            } else {
                search_child(c, g, choice, step + 1)
            }
        }
    }
}
