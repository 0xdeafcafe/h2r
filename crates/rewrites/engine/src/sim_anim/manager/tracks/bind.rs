//! Resolve a channel handle from a graph, then apply the requested mode.
use super::*;

pub(super) fn remap(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1cb3b0 => {
            c.ecx = frame(c, g, 4)?;
            c.eax = c.ecx >> 16;
            if !eq_narrow(c, c.eax, NONE, 16) {
                c.eax = g.u32(at(c.edi, 0x68))?;
                c.edx = g.u32(TAG_ENTRIES.base)?;
                let offset = tag_offset(c, c.eax);
                c.eax = tag(g, c.edx, offset)?;
                logic(c, c.eax, 1 << 31);
                if c.eax != 0 {
                    c.edx = u32::from(g.u8(at(c.edi, 0x6c))? >> 1) & 1;
                    crate::render_census::selector_call(c.edx != 0);
                    logic(c, c.edx, 1 << 31);
                    push(c, g, c.edx)?;
                    push(c, g, c.ecx)?;
                    c.ecx = at(c.esp, 12);
                    push(c, g, c.ecx)?;
                    push(c, g, c.eax)?;
                    return g.call(c, 0x1dd630, 0x1cb3f2);
                }
            }
            g.set_u32(c.esi, c.ecx)?;
        }
        0x1cb3f2 => {
            c.eax = g.u32(c.eax)?;
            g.set_u32(c.esi, c.eax)?;
        }
        _ => return Err(bad(pc)),
    }
    c.eax = c.esi;
    g.ret(c, 4)
}

pub(super) fn select(c: &mut CpuAbi, g: &mut Bus<'_>, mut pc: u32) -> Transfer {
    loop {
        pc = match pc {
            0x1cb410 => {
                push(c, g, c.ebp)?;
                c.ebp = frame(c, g, 8)?;
                push(c, g, c.edi)?;
                c.edi = c.eax;
                c.ecx = g.u32(at(c.edi, 0x68))?;
                c.eax = byte(c.eax, 0);
                if eq32(c, c.ecx, NONE) {
                    0x1cb487
                } else {
                    let remap = test(c, c.ebx as u8 & 0x10);
                    c.eax = frame(c, g, 0x10)?;
                    if remap {
                        push(c, g, c.esi)?;
                        push(c, g, c.eax)?;
                        c.esi = at(c.esp, 0x18);
                        return g.call(c, 0x1cb3b0, 0x1cb436);
                    }
                    0x1cb439
                }
            }
            0x1cb436 => {
                c.eax = g.u32(c.eax)?;
                c.esi = pop(c, g)?;
                0x1cb439
            }
            0x1cb439 => {
                c.edx = frame(c, g, 0x20)?;
                c.ecx = g.u32(at(c.edi, 0x68))?;
                push(c, g, c.edx)?;
                for _ in 0..3 {
                    c.edx = frame(c, g, 0x20)?;
                    push(c, g, c.edx)?;
                }
                push(c, g, c.eax)?;
                c.edx = c.ebx;
                c.eax = c.ebp;
                return g.call(c, 0x1c6470, 0x1cb45a);
            }
            0x1cb45a => {
                if !test(c, c.eax as u8) {
                    c.eax = byte(c.eax, 0);
                    test(c, 0);
                    0x1cb487
                } else if test(c, c.ebx as u8 & 4) {
                    push(c, g, 0)?;
                    c.eax = c.ebp;
                    return g.call(c, 0x1c6560, 0x1cb46c);
                } else {
                    0x1cb46c
                }
            }
            0x1cb46c => {
                if test(c, c.ebx as u8 & 8) {
                    let one = g.u32(ONE)?;
                    load_ss(c, 0, one);
                    g.set_u32(at(c.ebp, 0x18), one)?;
                }
                c.edi = pop(c, g)?;
                c.eax = byte(c.eax, 1);
                c.ebp = pop(c, g)?;
                return g.ret(c, 24);
            }
            0x1cb487 => {
                c.edi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                return g.ret(c, 24);
            }
            _ => return Err(bad(pc)),
        }
    }
}

pub(super) fn by_handle(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    if pc == 0x1cb490 {
        c.edx = g.u32(at(c.eax, 0x68))?;
        c.ecx = byte(c.ecx, 0);
        if eq32(c, c.edx, NONE) {
            c.eax = byte(c.eax, c.ecx as u8);
        } else {
            c.ecx = frame(c, g, 8)?;
            c.edx = frame(c, g, 4)?;
            for arg in [NONE, NONE, NONE, NONE, c.ecx, c.edx] {
                push(c, g, arg)?;
            }
            return g.call(c, 0x1cb410, 0x1cb4b1);
        }
    } else if pc != 0x1cb4b1 {
        return Err(bad(pc));
    }
    g.ret(c, 8)
}

// Both lookup forms leave the resolved handle in a local slot. The low
// and high halves are consumed by the selector as a single dword.
fn lookup_result(c: &mut CpuAbi, g: &mut Bus<'_>, kind: u32, site: u32) -> Transfer {
    if !eq_narrow(c, g.u16(at(c.esp, 6))?.into(), NONE, 16) {
        c.eax = frame(c, g, 4)?;
        c.ecx = frame(c, g, 12)?;
        c.ebx = frame(c, g, 16)?;
        for arg in [kind, NONE, NONE, c.edi, c.eax, c.ecx] {
            push(c, g, arg)?;
        }
        c.eax = c.esi;
        return g.call(c, 0x1cb410, site);
    }
    c.eax = byte(c.eax, c.ebx as u8);
    lookup_return(c, g)
}

fn lookup_return(c: &mut CpuAbi, g: &mut Bus<'_>) -> Transfer {
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 8)
}

pub(super) fn by_name(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1cb4c0 => {
            push(c, g, c.ecx)?;
            c.eax = g.u32(at(c.esi, 0x68))?;
            push(c, g, c.ebx)?;
            c.ebx = byte(c.ebx, 0);
            if eq32(c, c.eax, NONE) {
                c.eax = byte(c.eax, 0);
                return lookup_return(c, g);
            }
            c.edx = g.u32(TAG_ENTRIES.base)?;
            c.eax &= 0xffff;
            push(c, g, c.edi)?;
            c.ecx = at(c.esp, 8);
            c.eax = tag_offset(c, c.eax);
            c.edx = tag(g, c.edx, c.eax)?;
            push(c, g, c.ecx)?;
            g.call(c, 0x1dd0b0, 0x1cb4e9)
        }
        0x1cb4e9 => lookup_result(c, g, 0, 0x1cb50d),
        0x1cb50d => lookup_return(c, g),
        _ => Err(bad(pc)),
    }
}

pub(super) fn by_state(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1cb520 => {
            push(c, g, c.ecx)?;
            push(c, g, c.ebx)?;
            c.ebx = byte(c.ebx, 0);
            if eq32(c, c.ecx, g.u32(0x1cb526)?) || eq32(c, c.ecx, NONE) {
                c.ecx = g.u32(at(c.esi, 0x74))?;
            }
            if eq32(c, c.edx, g.u32(0x1cb536)?) || eq32(c, c.edx, NONE) {
                c.edx = g.u32(at(c.esi, 0x78))?;
            }
            c.eax = g.u32(at(c.esi, 0x68))?;
            if eq32(c, c.eax, NONE) {
                c.eax = byte(c.eax, 0);
                return lookup_return(c, g);
            }
            for arg in [0, 0, 0, c.edx, c.ecx] {
                push(c, g, arg)?;
            }
            c.ecx = g.u32(at(c.esi, 0x70))?;
            push(c, g, c.ecx)?;
            c.ecx = g.u32(TAG_ENTRIES.base)?;
            c.edx = at(c.esp, 0x1c);
            c.eax = tag_offset(c, c.eax);
            push(c, g, c.edx)?;
            c.edx = tag(g, c.ecx, c.eax)?;
            push(c, g, c.edx)?;
            c.eax = c.edi;
            g.call(c, 0x1dbc80, 0x1cb577)
        }
        0x1cb577 => lookup_result(c, g, 1, 0x1cb59b),
        0x1cb59b => lookup_return(c, g),
        _ => Err(bad(pc)),
    }
}
