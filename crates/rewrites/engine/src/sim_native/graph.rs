use super::*;
use guest_layout::points::TAG_ENTRIES;

pub const ENABLED: u32 = 0x0051_0c20;
const AFTER_GRAPH: u32 = 0x001d_da11;
const AFTER_FIRST: u32 = 0x001d_da4e;
const AFTER_LINK: u32 = 0x001d_da8d;
const PREFETCH: u32 = 0x0012_36f0;

fn clip(g: &impl Memory, graph: u32, index: u32) -> std::result::Result<u32, GuestFault> {
    Ok(if index == u32::MAX {
        0
    } else {
        at(g.u32(at(graph, 48))?, index.wrapping_mul(108))
    })
}

/// Resolve an inherited graph, follow clip aliases and preload chained
/// resources. A cache request can call back into the scheduler; return to
/// its real guest continuation with the same caller frame.
pub fn resolve(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    for addr in [ENABLED, ENABLED + 1] {
        let on = g.u8(addr)?;
        c.eax = low(c.eax, on);
        test_byte(c, on);
        if on == 0 {
            return ret(g, c, 4);
        }
    }
    let saved = c.esi;
    push(c, g, saved)?;
    let handle = g.u32(at(c.esp, 8))?;
    c.esi = handle >> 16;
    sub_flags(c, c.esi, 0xffff, 16);
    if c.esi == 0xffff {
        c.esi = pop(c, g)?;
        return ret(g, c, 4);
    }
    c.eax = handle;
    let saved = c.edi;
    push(c, g, saved)?;
    c.edi = c.edx;
    let parent = handle as i16 as i32;
    if parent != -1 {
        push(c, g, AFTER_GRAPH)?;
        let saved = c.esi;
        push(c, g, saved)?;
        let count = g.u32(at(c.edx, 0x4c))? as i32;
        c.eax = 0;
        if parent < count {
            let parents = g.u32(at(c.edx, 0x50))?;
            let h = g.u32(at(parents, (parent as u32).wrapping_shl(5).wrapping_add(4)))?;
            if h != u32::MAX {
                c.eax = tag_data(g, g.u32(TAG_ENTRIES.base)?, h)?;
            }
        }
        c.esi = pop(c, g)?;
        pop(c, g)?;
        c.edi = c.eax;
    }
    c.eax = c.esi as i16 as u32;
    let first = clip(g, c.edi, c.eax)?;
    let alias = g.u16(at(first, 48))? as i16;
    c.esi = if alias == -1 {
        first
    } else {
        clip(g, c.edi, alias as u32)?
    };
    c.edx = c.edi;
    // 1dd840 receives the original index, even when the chain starts
    // from its alias. A live first request tail-jumps to the cache.
    push(c, g, AFTER_FIRST)?;
    c.eax = first;
    c.ecx = first;
    if request(c, g)? {
        return Ok(TransferAbi::jump(PREFETCH));
    }
    pop(c, g)?;
    for depth in 0.. {
        if depth == 256 {
            return Ok(TransferAbi::resume(AFTER_FIRST));
        }
        let next = g.u16(at(c.esi, 50))?;
        sub_flags(c, u32::from(next), 0xffff, 16);
        if next == 0xffff {
            break;
        }
        c.eax = clip(g, c.edi, next as i16 as u32)?;
        c.ecx = c.eax;
        let link = c.ecx;
        if request(c, g)? {
            return call(c, g, PREFETCH, AFTER_LINK);
        }
        c.ecx = link;
        c.esi = link;
        // The original reloads the record and leaves its address in EAX.
        c.eax = link;
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    ret(g, c, 4)
}

fn request(c: &mut CpuAbi, g: &impl Memory) -> std::result::Result<bool, GuestFault> {
    let resident = u32::from(g.u8(at(c.ecx, 22))?) & 16;
    logic_flags(c, resident);
    if resident == 0 {
        return Ok(false);
    }
    let index = g.u32(at(c.ecx, 36))?;
    c.ecx = g.u32(at(c.edi, 176))?;
    c.eax = at(c.ecx, index.wrapping_mul(20));
    let active = g.u8(at(c.eax, 16))?;
    c.ecx = low(c.ecx, active);
    test_byte(c, active);
    if active != 0 {
        c.ecx = low(c.ecx, 1);
    }
    Ok(active != 0)
}
