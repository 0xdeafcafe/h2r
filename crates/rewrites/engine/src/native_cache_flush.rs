//! The resource caches' flushes and an owner's unload, with no push-buffer
//! kick (docs/graphics/d3d-nv2a.md, "NV2A register window").
//!
//! Retail opens each of these with `CDevice_KickOff` (`0x3fac30`) and a
//! peek at the GPU's FIFO through device `+0x934`, so the GPU is done with
//! what the cache is about to free. There is no GPU queue in the guest:
//! every resource is idle, and the kick and the peek are gone. The rest
//! runs as retail does, its guest calls made from their retail return
//! sites.
//!
//! | Entry | What it does |
//! |---|---|
//! | `0x12d0a0` ([`points::CACHE_FLUSH`]) | runs the pending completions at `[0x4e645c]` (`[+0x18](+8, +0x10)`, stdcall), empties the allocator `[0x4e6464]` (`0x13d2b0`), and deletes every entry of `[0x4e6458]` (`0x16ba40`), clearing bit 2 of its resource's `+0xf` |
//! | `0x12e0c0` ([`points::CACHE_FLUSH_SECOND`]) | empties the allocator `[0x4e649c]` and deletes every entry of `[0x4e6490]`, clearing bit 2 of its resource's `+0x1f` |
//! | `0x12e150` ([`points::CACHE_UNLOAD_OWNER`]) | for the owner in EBX: unloads each `[0x4e648c]` entry's resource with `+0x18 == owner` (`0x12ddd0`), then deletes each `[0x4e6490]` entry whose resource has `+0x18 == owner` |

use crate::points;
use crate::sim_objects::frame::{pop, push};
use crate::sim_objects::{Body, Cx, Next, Result, Table};
use runtime::{CpuAbi, Guest};
use std::sync::LazyLock;

/// The pending completions' list head.
const PENDING: u32 = 0x004e_645c;
/// The first cache's allocator and entry array.
const ALLOCATOR: u32 = 0x004e_6464;
const ENTRIES: u32 = 0x004e_6458;
/// The second cache's allocator and entry array, and the owners' array.
const SECOND_ALLOCATOR: u32 = 0x004e_649c;
const SECOND_ENTRIES: u32 = 0x004e_6490;
const OWNED: u32 = 0x004e_648c;
/// The allocator's reset.
const ALLOCATOR_RESET: u32 = 0x0013_d2b0;
/// The data array's next used index (`ecx` array, `edx` first index, `eax`
/// the index or -1).
const DATUM_NEXT: u32 = 0x0016_bc00;
/// The data array's delete (`edx` array, `eax` handle).
const DATUM_DELETE: u32 = 0x0016_ba40;
/// A resource's unload (stdcall, `ret 4`).
const RESOURCE_UNLOAD: u32 = 0x0012_ddd0;

pub const BODIES: &[Body] = &[
    Body { point: &points::CACHE_FLUSH, sites: &[0x12d0bb, 0x12d0f3, 0x12d158], run: flush },
    Body { point: &points::CACHE_FLUSH_SECOND, sites: &[0x12e0f7, 0x12e11a, 0x12e146], run: flush_second },
    Body { point: &points::CACHE_UNLOAD_OWNER, sites: &[0x12e19d, 0x12e1bf, 0x12e1da, 0x12e205], run: unload_owner },
];

/// The bodies. None touches float state.
pub static TABLE: LazyLock<Table> = LazyLock::new(|| {
    Table::new(BODIES.iter())
        .expect("cache flush entries and sites are unique")
        .float_free(&[points::CACHE_FLUSH.base, points::CACHE_FLUSH_SECOND.base, points::CACHE_UNLOAD_OWNER.base])
});

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

/// `[array + 0x44] + [array + 0x24] * index`: the datum's address.
fn datum(g: &Guest, array: u32, index: u32) -> Result<u32> {
    let size = g.u32(array.wrapping_add(0x24))?;
    let data = g.u32(array.wrapping_add(0x44))?;
    Ok(size.wrapping_mul(index).wrapping_add(data))
}

/// The handle of the datum at `at`, index `index`: its salt (`movsx word`)
/// in the high half.
fn handle(g: &Guest, at: u32, index: u32) -> Result<u32> {
    let salt = g.u16(at)? as i16 as i32 as u32;
    Ok(salt << 16 | index)
}

/// `and byte [at], 0xfb`.
fn clear_cached(g: &mut Guest, at: u32) -> Result<()> {
    let b = g.u8(at)?;
    g.set_u8(at, b & 0xfb)
}

fn flush(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let c = &mut *cx.cpu;
    let g = &mut *cx.g;
    match pc {
        0x12d0a0 | 0x12d0bb => {
            c.eax = g.u32(PENDING)?;
            if c.eax != 0 {
                c.ecx = g.u32(c.eax.wrapping_add(0x10))?;
                c.edx = g.u32(c.eax.wrapping_add(8))?;
                push(c, g, c.ecx)?;
                push(c, g, c.edx)?;
                let target = g.u32(c.eax.wrapping_add(0x18))?;
                return call(target, 0x12d0bb);
            }
            c.eax = g.u32(ALLOCATOR)?;
            call(ALLOCATOR_RESET, 0x12d0f3)
        }
        0x12d0f3 => {
            c.edx = g.u32(ENTRIES)?;
            let used = g.u8(c.edx.wrapping_add(0x29))?;
            c.eax = (c.eax & !0xff) | u32::from(used);
            if used == 0 {
                return Ok(Next::Ret { pop: 0 });
            }
            push(c, g, c.esi)?;
            push(c, g, c.ebx)?;
            c.esi = u32::MAX;
            push(c, g, c.ebp)?;
            flush_next(c, g)
        }
        0x12d158 => flush_next(c, g),
        _ => unreachable!("cache flush: no segment at {pc:#x}"),
    }
}

/// `0x12d106`: the next used entry of `[0x4e6458]` after ESI, by its bit
/// in `[edx + 0x48]`, deleted; none left, the frame popped.
fn flush_next(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esi = c.esi.wrapping_add(1);
    if (c.esi as i32) >= 0 {
        loop {
            c.ecx = g.u32(c.edx.wrapping_add(0x38))?;
            if (c.esi as i32) >= (c.ecx as i32) {
                break;
            }
            c.eax = g.u32(c.edx.wrapping_add(0x48))?;
            c.ebx = 1 << (c.esi & 0x1f);
            let word = g.u32(c.eax.wrapping_add(((c.esi as i32 >> 5) as u32).wrapping_mul(4)))?;
            if word & c.ebx != 0 {
                c.ebp = g.u32(c.edx.wrapping_add(0x44))?;
                let at = datum(g, c.edx, c.esi)?;
                c.eax = handle(g, at, c.esi)?;
                c.ecx = g.u32(at.wrapping_add(4))?;
                clear_cached(g, c.ecx.wrapping_add(0xf))?;
                return call(DATUM_DELETE, 0x12d158);
            }
            c.esi = c.esi.wrapping_add(1);
        }
    }
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

fn flush_second(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let c = &mut *cx.cpu;
    let g = &mut *cx.g;
    match pc {
        0x12e0c0 => {
            push(c, g, c.esi)?;
            c.esi = g.u32(SECOND_ALLOCATOR)?;
            if c.esi == 0 {
                c.esi = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
            c.eax = c.esi;
            call(ALLOCATOR_RESET, 0x12e0f7)
        }
        0x12e0f7 => {
            c.esi = g.u32(SECOND_ENTRIES)?;
            let used = g.u8(c.esi.wrapping_add(0x29))?;
            c.eax = (c.eax & !0xff) | u32::from(used);
            if used == 0 {
                c.esi = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
            push(c, g, c.edi)?;
            c.edi = u32::MAX;
            second_next(c)
        }
        0x12e11a => {
            c.edx = c.eax;
            if c.edx == u32::MAX {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
            let at = datum(g, c.esi, c.edx)?;
            c.eax = handle(g, at, c.edx)?;
            c.ecx = g.u32(at.wrapping_add(4))?.wrapping_add(0x1f);
            c.edi = c.edx;
            clear_cached(g, c.ecx)?;
            c.edx = c.esi;
            call(DATUM_DELETE, 0x12e146)
        }
        0x12e146 => second_next(c),
        _ => unreachable!("second cache flush: no segment at {pc:#x}"),
    }
}

/// `0x12e110`: the next used entry of the array in ESI after EDI.
fn second_next(c: &mut CpuAbi) -> Result<Next> {
    c.edx = c.edi.wrapping_add(1);
    c.ecx = c.esi;
    call(DATUM_NEXT, 0x12e11a)
}

fn unload_owner(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let c = &mut *cx.cpu;
    let g = &mut *cx.g;
    match pc {
        0x12e150 => {
            push(c, g, c.esi)?;
            c.esi = g.u32(OWNED)?;
            if c.esi == 0 {
                c.esi = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
            let used = g.u8(c.esi.wrapping_add(0x29))?;
            c.eax = (c.eax & !0xff) | u32::from(used);
            if used == 0 {
                c.esi = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
            push(c, g, c.edi)?;
            c.edi = u32::MAX;
            owned_next(c)
        }
        // A used entry of `[0x4e648c]`: its resource (`+8`) unloaded when
        // the owner's.
        0x12e19d => {
            if c.eax == u32::MAX {
                return entries_first(c, g);
            }
            let at = datum(g, c.esi, c.eax)?;
            c.edx = g.u32(c.esi.wrapping_add(0x44))?;
            c.ecx = at;
            c.edi = c.eax;
            if at == 0 {
                return entries_first(c, g);
            }
            c.ecx = g.u32(at.wrapping_add(8))?;
            if g.u32(c.ecx.wrapping_add(0x18))? != c.ebx {
                return owned_next(c);
            }
            push(c, g, c.ecx)?;
            call(RESOURCE_UNLOAD, 0x12e1bf)
        }
        0x12e1bf => owned_next(c),
        // A used entry of `[0x4e6490]`: deleted when its resource (`+4`)
        // is the owner's.
        0x12e1da => {
            c.edx = c.eax;
            if c.edx == u32::MAX {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                return Ok(Next::Ret { pop: 0 });
            }
            let at = datum(g, c.esi, c.edx)?;
            c.eax = handle(g, at, c.edx)?;
            c.ecx = g.u32(at.wrapping_add(4))?;
            c.edi = c.edx;
            if g.u32(c.ecx.wrapping_add(0x18))? != c.ebx {
                return entries_next(c);
            }
            c.edx = c.esi;
            call(DATUM_DELETE, 0x12e205)
        }
        0x12e205 => entries_next(c),
        _ => unreachable!("owner unload: no segment at {pc:#x}"),
    }
}

/// `0x12e193`: the next used entry of `[0x4e648c]` after EDI.
fn owned_next(c: &mut CpuAbi) -> Result<Next> {
    c.edx = c.edi.wrapping_add(1);
    c.ecx = c.esi;
    call(DATUM_NEXT, 0x12e19d)
}

/// `0x12e1c1`: the walk of `[0x4e6490]` from its start.
fn entries_first(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = g.u32(SECOND_ENTRIES)?;
    c.edi = u32::MAX;
    entries_next(c)
}

/// `0x12e1d0`: the next used entry of the array in ESI after EDI.
fn entries_next(c: &mut CpuAbi) -> Result<Next> {
    c.edx = c.edi.wrapping_add(1);
    c.ecx = c.esi;
    call(DATUM_NEXT, 0x12e1da)
}

#[cfg(test)]
mod tests;
