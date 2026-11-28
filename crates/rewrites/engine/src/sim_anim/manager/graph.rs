//! Graph elements, resource preloads, sampling views and named graph blocks.
use super::*;
use ::sim_anim::graph_resolve::{CLIP_BYTES, Preload};
use guest_layout::points::TAG_ENTRIES;
use crate::sim_native::{graph::ENABLED, sub_flags};
use crate::sim_physics::{arithmetic, byte, logic};

mod lookup;

pub(super) const BODIES: &[Body] = &[
    body!(0x1daea0, 0x1daf26, &[0x1daf1f], element),
    body!(0x1dd560, 0x1dd5c3, &[], find_record),
    body!(0x1dd9d0, 0x1ddab0, &[0x1dda11, 0x1dda4e, 0x1dda8d], preload),
    body!(0x1ddb40, 0x1ddb8e, &[0x1ddb60, 0x1ddb6a], sample_view),
];

/// Clip entries contain the resource and its alias/chain indices.
const CLIPS: u32 = 0x30;
const NO_INDEX: u16 = u16::MAX;

fn signed_word(value: u32) -> u32 {
    value as i16 as i32 as u32
}

fn clip_addr(c: &mut CpuAbi, base: u32, index: u32) -> u32 {
    arithmetic(c, index.wrapping_mul(CLIP_BYTES as u32), base, false)
}

fn element(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1daea0 {
        c.ecx = g.u32(at(c.esp, 4))? >> 16;
        c.eax = 0;
        sub_flags(c, c.ecx, u32::from(NO_INDEX), 16);
        if c.ecx as u16 == NO_INDEX {
            return g.ret(c, 4);
        }
        push(c, g, c.ebx)?;
        c.ebx = g.u32(at(c.esp, 8))?;
        push(c, g, c.esi)?;
        let inherited = c.ebx as u16 != NO_INDEX;
        let graph = if inherited {
            c.esi = g.u32(at(c.edx, 0x4c))?;
            c.eax = signed_word(c.ebx);
            push(c, g, c.edi)?;
            c.edi = 0;
            if (c.eax as i32) < (c.esi as i32) {
                let parents = g.u32(at(c.edx, 0x50))?;
                let handle = g.u32(at(at(parents, c.eax << 5), 4))?;
                if handle != u32::MAX {
                    let tags = g.u32(TAG_ENTRIES.base)?;
                    c.edi = g.u32(at(at(tags, (handle & 0xffff) << 4), 8))?;
                }
            }
            c.edi
        } else {
            c.edx
        };
        c.eax = signed_word(c.ecx);
        c.ecx = g.u32(at(graph, CLIPS))?;
        c.eax = clip_addr(c, c.ecx, c.eax);
        c.esi = c.eax;
        if inherited {
            c.edi = pop(c, g)?;
        }
        logic(c, c.esi, 1 << 31);
        if c.esi != 0 {
            push(c, g, c.ebx)?;
            return g.call(c, 0x1dd9d0, 0x1daf1f);
        }
    }
    c.eax = c.esi;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 4)
}

/// Walk aliases and preload a clip's dependent resources. After a cache
/// call the guest reads the link again, since that call can change it.
fn preload(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    if pc == 0x1dd9d0 {
        for addr in [ENABLED, ENABLED + 1] {
            let gate = g.u8(addr)?;
            c.eax = byte(c.eax, gate);
            logic(c, u32::from(gate), 0x80);
            if gate == 0 {
                return g.ret(c, 4);
            }
        }
        push(c, g, c.esi)?;
        c.esi = g.u32(at(c.esp, 8))? >> 16;
        sub_flags(c, c.esi, u32::from(NO_INDEX), 16);
        if c.esi as u16 == NO_INDEX {
            c.esi = pop(c, g)?;
            return g.ret(c, 4);
        }
        c.eax = g.u32(at(c.esp, 8))?;
        sub_flags(c, c.eax, u32::from(NO_INDEX), 16);
        push(c, g, c.edi)?;
        c.edi = c.edx;
        if c.eax as u16 != NO_INDEX {
            c.ecx = signed_word(c.eax);
            return g.call(c, 0x1dafc0, 0x1dda11);
        }
    }
    if matches!(pc, 0x1dd9d0 | 0x1dda11) {
        if pc == 0x1dda11 {
            c.edi = c.eax;
        }
        c.eax = signed_word(c.esi);
        c.ecx = 0;
        if c.eax != u32::MAX {
            c.edx = g.u32(at(c.edi, CLIPS))?;
            c.ecx = clip_addr(c, c.edx, c.eax);
        }
        c.esi = c.ecx;
        let alias = g.u16(at(c.ecx, 48))?;
        c.ecx = (c.ecx & 0xffff_0000) | u32::from(alias);
        sub_flags(c, c.ecx, u32::from(NO_INDEX), 16);
        if alias != NO_INDEX {
            c.edx = g.u32(at(c.edi, CLIPS))?;
            c.ecx = clip_addr(c, c.edx, signed_word(c.ecx));
            c.esi = c.ecx;
        }
        // The first request receives the original index in EAX, even if
        // subsequent links start from the alias in ESI.
        c.edx = c.edi;
        return g.call(c, 0x1dd840, 0x1dda4e);
    }

    let mut reload_link = pc == 0x1dda8d;
    if let Some(boundary) = preload_table(c, g, reload_link) {
        return boundary;
    }
    // A malformed uncached cycle belongs to the lifted loop.
    for _ in 0..=u16::MAX {
        if reload_link {
            let next = g.u16(at(c.esi, 50))?;
            c.eax = signed_word(u32::from(next));
            c.ecx = 0;
            if next != NO_INDEX {
                c.ecx = g.u32(at(c.edi, CLIPS))?;
                c.eax = clip_addr(c, c.ecx, c.eax);
                c.ecx = c.eax;
            }
            c.esi = c.ecx;
        }
        let next = g.u16(at(c.esi, 50))?;
        sub_flags(c, u32::from(next), u32::from(NO_INDEX), 16);
        if next == NO_INDEX {
            return preload_return(c, g);
        }
        c.ecx = g.u32(at(c.edi, CLIPS))?;
        c.eax = clip_addr(c, c.ecx, signed_word(u32::from(next)));
        c.ecx = c.eax;
        if g.u8(at(c.ecx, 22))? & 16 != 0 {
            let index = g.u32(at(c.ecx, 36))?;
            c.ecx = g.u32(at(c.edi, 176))?;
            c.eax = at(c.ecx, index.wrapping_mul(20));
            let active = g.u8(at(c.eax, 16))?;
            c.ecx = byte(c.ecx, active);
            logic(c, u32::from(active), 0x80);
            if active != 0 {
                c.ecx = byte(c.ecx, 1);
                return g.call(c, 0x1236f0, 0x1dda8d);
            }
        }
        reload_link = true;
    }
    Err(bad(pc))
}

/// Compute one span between guest callbacks from the graph's decoded links.
/// Populate the crossing registers and flags only after the walk succeeds,
/// so an ineligible block can take the ordered path from the same frame.
fn preload_table(
    c: &mut CpuAbi, g: &mut Bus<'_>, reload: bool,
) -> Option<sim_native::Result> {
    let (base, start, table) = lookup::dependencies(g, c.edi, c.esi)?;
    let clip_address = |index: usize| base + (index * CLIP_BYTES) as u32;
    let mut resources = 0;
    let boundary = table.preload(start, reload, |index| {
        resources = g.u32(at(c.edi, 176)).ok()?;
        g.u8(at(at(resources, index.wrapping_mul(20)), 16)).ok()
    })?;
    Some(match boundary {
        Preload::End { clip } => {
            if reload || clip != start {
                c.eax = clip_address(clip);
                c.ecx = c.eax;
            }
            sub_flags(c, u32::from(NO_INDEX), u32::from(NO_INDEX), 16);
            preload_return(c, g)
        }
        Preload::Request { from, resource, active } => {
            c.esi = clip_address(from);
            c.eax = at(resources, resource.wrapping_mul(20));
            c.ecx = byte(resources, 1);
            logic(c, u32::from(active), 0x80);
            g.call(c, 0x1236f0, 0x1dda8d)
        }
    })
}

fn preload_return(c: &mut CpuAbi, g: &mut Bus<'_>) -> sim_native::Result {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    g.ret(c, 4)
}

fn sample_view(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    match pc {
        0x1ddb40 => {
            c.eax = g.u32(at(c.esp, 8))? >> 16;
            sub_flags(c, c.eax, u32::from(NO_INDEX), 16);
            push(c, g, c.ebp)?;
            c.ebp = g.u32(at(c.esp, 8))?;
            if c.eax as u16 != NO_INDEX {
                push(c, g, c.ebx)?;
                c.ebx = g.u32(at(c.esp, 16))?;
                push(c, g, c.esi)?;
                push(c, g, c.ebx)?;
                c.edx = c.ebp;
                return g.call(c, 0x1daea0, 0x1ddb60);
            }
        }
        0x1ddb60 => {
            push(c, g, c.ebx)?;
            c.ecx = c.ebp;
            c.esi = c.eax;
            return g.call(c, 0x1dd7c0, 0x1ddb6a);
        }
        0x1ddb6a => {
            c.ebx = u32::from(g.u8(at(c.esi, 19))?);
            c.ecx = signed_word(u32::from(g.u16(at(c.esi, 20))?));
            c.edx = g.u8(at(c.esi, 17))? as i8 as i32 as u32;
            c.esi = arithmetic(c, c.esi, 60, false);
            g.set_u32(at(c.edi, 4), c.esi)?;
            c.esi = pop(c, g)?;
            g.set_u8(at(c.edi, 8), c.ebx as u8)?;
            g.set_u32(c.edi, c.eax)?;
            g.set_u8(at(c.edi, 9), c.edx as u8)?;
            g.set_u16(at(c.edi, 10), c.ecx as u16)?;
            c.ebx = pop(c, g)?;
        }
        _ => return Err(bad(pc)),
    }
    c.ebp = pop(c, g)?;
    g.ret(c, 8)
}

/// Graph blocks are ordered by signed string ID. On a hit a uniquely
/// named record fixes the last midpoint, live registers and exit flags.
fn find_record(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    push(c, g, c.ebx)?;
    push(c, g, c.ebp)?;
    c.ebp = g.u32(at(c.esp, 12))?;
    push(c, g, c.esi)?;
    let count = g.u32(c.ebp)?;
    c.ebx = 0;
    c.esi = count.wrapping_sub(1);
    push(c, g, c.edi)?;
    let result_slot = at(c.esp, 20);
    g.set_u32(result_slot, 0)?;
    c.eax = byte(c.eax, 0);
    let mut found = false;
    if count != 0 && count <= i32::MAX as u32 {
        // The prologue can overwrite aliased arguments. Gather the block,
        // name and stride after those writes, exactly as the loop does.
        let records = g.u32(at(c.ebp, 4))?;
        let wanted = g.u32(at(c.esp, 24))?;
        let stride = g.u32(at(c.esp, 28))?;
        if let Some(index) = lookup::find(g, records, count, stride, wanted) {
            c.ecx = at(records, index.wrapping_mul(stride));
            c.edx = index;
            g.set_u32(result_slot, c.ecx)?;
            logic(c, c.ecx, 1 << 31);
            found = true;
        }
    }

    if !found {
        // The lifted midpoint adds two i32 bounds before shifting. Very
        // large corrupt bounds can wrap and stop making progress.
        let mut probes = 0;
        loop {
            logic(c, c.eax & 0xff, 0x80);
            if c.eax as u8 != 0 {
                break;
            }
            arithmetic(c, c.ebx, c.esi, true);
            if (c.ebx as i32) > (c.esi as i32) {
                break;
            }
            probes += 1;
            if probes > 32 {
                return Err(bad(pc));
            }
            c.edi = g.u32(at(c.ebp, 4))?;
            c.eax = byte(c.eax, u8::from(c.ebx == c.esi));
            c.edx = ((c.esi.wrapping_add(c.ebx) as i32) >> 1) as u32;
            let stride = g.u32(at(c.esp, 28))?;
            c.ecx = at(c.edi, c.edx.wrapping_mul(stride));
            c.edi = g.u32(c.ecx)?;
            let wanted = g.u32(at(c.esp, 24))?;
            if c.edi == wanted {
                g.set_u32(result_slot, c.ecx)?;
            } else if (c.edi as i32) > (wanted as i32) {
                c.esi = c.edx.wrapping_sub(1);
            } else {
                c.ebx = c.edx.wrapping_add(1);
            }
            c.ecx = g.u32(result_slot)?;
            logic(c, c.ecx, 1 << 31);
            if c.ecx != 0 {
                break;
            }
        }
    }
    c.eax = g.u32(result_slot)?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 12)
}

#[cfg(test)]
mod preload_tests;
