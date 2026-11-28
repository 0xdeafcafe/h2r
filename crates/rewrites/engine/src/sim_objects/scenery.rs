//! The scenery update `10a2f0` (slice 11 of docs/engine/sim-rewrite-objects.md;
//! contract in docs/engine/object-update.md, "Slice 11 bodies"): the `scen`
//! descriptor's slot `+40`.
//!
//! **`10a2f0 (object: stack 0) -> al`; `ret 4`.** Saves `ebx` and `edi`
//! (`edi` the object across the calls), and `esi` (the animation block,
//! body `+ [body+12a]`) once the scenery has one. Without an animation
//! block it returns 0. Otherwise it advances the animation
//! (`1cb710(0xbf600, object, 0, 0, 0)`, `ret 14`; `0xbf600` is a function
//! pointer it passes, O) and, when the block's `+68` is not none, its
//! `+0` tag is not none and its `+6` word is not none, looks the block up
//! (`1daea0(edx: the tag's definition, stack: block +4)`, `ret 4`); when
//! the answer's `+14` word is above 1 it wakes the scenery for the post
//! walk (`bba20`, a slice 1 body) and returns 1. Every other path returns
//! 0 (`bl`).
//!
//! Exit: `al` the result over the last callee's `eax` (or the body
//! pointer, on the early path); `ecx` and `edx` as the last call or load
//! left them; flags from the last compare.

use super::alu::{cmp16, shl};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, pop, push};

const NONE: u32 = u32::MAX;
/// The animation advance `10a2f0` calls (`ret 14`).
const ANIMATION_ADVANCE: u32 = 0x001c_b710;
/// The function pointer it passes as the advance's first argument.
const ADVANCE_CALLBACK: u32 = 0x000b_f600;
/// The animation lookup (`edx` the definition, one stack argument).
const ANIMATION_LOOKUP: u32 = 0x001d_aea0;
/// Animation block field read after the advance.
const BLOCK_STATE: u32 = 0x68;

/// After the advance, the lookup and the wake.
pub(super) const SCENERY_SITES: [u32; 3] = [0x10a334, 0x10a365, 0x10a373];

/// `10a37b`: no animation block.
fn early_ret(c: &mut runtime::CpuAbi, g: &runtime::Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

/// `10a382`: `bl` with `esi` saved.
fn ret_bl(c: &mut runtime::CpuAbi, g: &runtime::Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    early_ret(c, g)
}

/// `10a2f0 (object: stack 0) -> al`; `ret 4`.
pub(super) fn scenery_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::SCENERY_UPDATE.base {
        c.ecx = g.u32(points::OBJECTS.base)?;
        c.edx = g.u32(c.ecx.wrapping_add(DATA))?;
        push(c, g, c.ebx)?;
        push(c, g, c.edi)?;
        c.edi = g.u32(c.esp.wrapping_add(0xc))?;
        c.eax = (c.edi & 0xffff) * 3;
        c.eax = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
        c.ebx = byte(c.ebx, 0);
        let block = u32::from(g.u16(c.eax.wrapping_add(0x12a))?);
        cmp16(c, block, 0xffff);
        if block == 0xffff {
            return early_ret(c, g);
        }
        push(c, g, c.esi)?;
        c.esi = block as u16 as i16 as i32 as u32;
        push(c, g, 0)?;
        push(c, g, 0)?;
        push(c, g, 0)?;
        push(c, g, c.edi)?;
        c.esi = arithmetic(c, c.esi, c.eax, false);
        push(c, g, ADVANCE_CALLBACK)?;
        return Ok(Next::Call { target: ANIMATION_ADVANCE, site: SCENERY_SITES[0] });
    }
    match pc {
        p if p == SCENERY_SITES[0] => {
            let state = g.u32(c.esi.wrapping_add(BLOCK_STATE))?;
            arithmetic(c, state, NONE, true);
            if state == NONE {
                return ret_bl(c, g);
            }
            c.eax = g.u32(c.esi)?;
            arithmetic(c, c.eax, NONE, true);
            if c.eax == NONE {
                return ret_bl(c, g);
            }
            let word = u32::from(g.u16(c.esi.wrapping_add(6))?);
            cmp16(c, word, 0xffff);
            if word == 0xffff {
                return ret_bl(c, g);
            }
            c.ecx = g.u32(points::TAG_ENTRIES.base)?;
            c.edx = g.u32(c.esi.wrapping_add(4))?;
            c.eax &= 0xffff;
            c.eax = shl(c, c.eax, 4, 32);
            c.eax = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(8))?;
            push(c, g, c.edx)?;
            c.edx = c.eax;
            Ok(Next::Call { target: ANIMATION_LOOKUP, site: SCENERY_SITES[1] })
        }
        p if p == SCENERY_SITES[1] => {
            let count = u32::from(g.u16(c.eax.wrapping_add(0x14))?);
            cmp16(c, count, 1);
            if (count as u16 as i16) <= 1 {
                return ret_bl(c, g);
            }
            c.eax = c.edi;
            Ok(Next::Call { target: points::OBJECT_WAKE_FOR_POST.base, site: SCENERY_SITES[2] })
        }
        p if p == SCENERY_SITES[2] => {
            c.esi = pop(c, g)?;
            c.edi = pop(c, g)?;
            c.eax = byte(c.eax, 1);
            c.ebx = pop(c, g)?;
            Ok(Next::Ret { pop: 4 })
        }
        _ => Err(unowned(pc)),
    }
}
