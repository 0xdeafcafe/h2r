//! The projectile level's import handler `fbe70` (slot `+4c`;
//! docs/engine/object-update.md, "Slice 3 bodies").
//!
//! `fbe70 (object, name, value out, active out) -> al` answers five
//! names from the projectile's body (`+12c` flags, `+158`, `+168`) and
//! definition (`+e0`, `+17c`..`+194`). The value is stored and active is
//! "value > 0"; `al` is `bl`, 1. An unknown name returns `al` 0 with
//! nothing stored. No calls.

use super::{
    Ss, above, byte, imm, lo, load, movaps, pop, push, ret, slot, ss, ucomiss_lahf_jp,
    unowned, zero, OBJECTS, ONE, TAGS,
};
use guest_layout::datum::layout;
use crate::sim_objects::{Cx, Next, Result};
use runtime::{CpuAbi, Guest};

const ENTRY: u32 = 0x000f_be70;

/// The imm32 of each `cmp eax, imm32` (the instruction address + 1).
const NAME_FBEA4: u32 = 0x000f_bea5;
const NAME_FBEB0: u32 = 0x000f_beb1;
const NAME_FBEB7: u32 = 0x000f_beb8;
const NAME_FBF04: u32 = 0x000f_bf05;
const NAME_FBF0B: u32 = 0x000f_bf0c;

/// Body `+12c`: the projectile's flags.
const FLAGS: u32 = 0x12c;

/// `fbe70 (object: stack 0, name: stack 1, value: stack 2, active:
/// stack 3) -> al`; `ret 10`.
pub(in crate::sim_objects) fn projectile(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != ENTRY {
        return Err(unowned(pc));
    }
    c.eax = slot(c, g, 4)?;
    c.ecx = g.u32(OBJECTS)?;
    c.edx = g.u32(c.ecx.wrapping_add(layout::DATA))?;
    zero(c, 1);
    c.eax = (c.eax & 0xffff) * 3;
    c.ecx = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
    c.eax = g.u32(c.ecx)?;
    c.edx = g.u32(TAGS)?;
    c.eax = (c.eax & 0xffff) << 4;
    c.edx = g.u32(c.eax.wrapping_add(c.edx).wrapping_add(8))?;
    c.eax = slot(c, g, 8)?;
    push(c, g, c.ebx)?;
    c.ebx = byte(c.ebx, 1);
    let name = c.eax as i32;
    let n = imm(g, NAME_FBEA4)? as i32;
    if name > n {
        // `fbf04`.
        if c.eax == imm(g, NAME_FBF04)? {
            // `fbf80`.
            c.ecx = g.u32(c.ecx.wrapping_add(FLAGS))? >> 3;
            return one_if(c, g, c.ecx & 1 != 0);
        }
        if c.eax != imm(g, NAME_FBF0B)? {
            // `fbf12`.
            c.eax = byte(c.eax, 0);
            c.ebx = pop(c, g)?;
            return ret(c, 0x10);
        }
        // `fbf18`.
        c.eax = g.u32(c.ecx.wrapping_add(FLAGS))? >> 8;
        if c.eax & 1 != 0 {
            return one(c, g);
        }
        load(c, 0, g.u32(c.edx.wrapping_add(0x17c))?);
        if !ucomiss_lahf_jp(c, lo(c, 0), lo(c, 1))? {
            return one(c, g);
        }
        load(c, 0, g.u32(c.edx.wrapping_add(0x180))?);
        if !ucomiss_lahf_jp(c, lo(c, 0), lo(c, 1))? {
            return one(c, g);
        }
        load(c, 0, g.u32(c.ecx.wrapping_add(0x168))?);
        ss(c, Ss::Sub, 0, g.u32(c.edx.wrapping_add(0x18c))?)?;
        ss(c, Ss::Mul, 0, g.u32(c.edx.wrapping_add(0x194))?)?;
        if above(c, lo(c, 1), lo(c, 0))? {
            movaps(c, 0, 1);
            return store(c, g);
        }
        load(c, 2, g.u32(ONE)?);
        if above(c, lo(c, 0), lo(c, 2))? {
            movaps(c, 0, 2);
        }
        return store(c, g);
    }
    if name == n {
        // `fbeda`.
        load(c, 0, g.u32(c.edx.wrapping_add(0xe0))?);
        if !ucomiss_lahf_jp(c, lo(c, 0), lo(c, 1))? {
            movaps(c, 0, 1);
            return store(c, g);
        }
        load(c, 0, g.u32(c.ecx.wrapping_add(0x168))?);
        ss(c, Ss::Div, 0, g.u32(c.edx.wrapping_add(0xe0))?)?;
        return store(c, g);
    }
    if c.eax == imm(g, NAME_FBEB0)? {
        // `fbecb`.
        c.eax = g.u32(c.ecx.wrapping_add(FLAGS))? >> 1;
        return one_if(c, g, c.eax & 1 != 0);
    }
    if c.eax != imm(g, NAME_FBEB7)? {
        c.eax = byte(c.eax, 0);
        c.ebx = pop(c, g)?;
        return ret(c, 0x10);
    }
    load(c, 0, g.u32(c.ecx.wrapping_add(0x158))?);
    store(c, g)
}

/// `fbf25`: one.
fn one(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    load(c, 0, g.u32(ONE)?);
    store(c, g)
}

/// `fbf8c`: one when the tested bit is set, else zero (`fbf96`).
fn one_if(c: &mut CpuAbi, g: &mut Guest, set: bool) -> Result<Next> {
    load(c, 0, g.u32(ONE)?);
    if !set {
        movaps(c, 0, 1);
    }
    store(c, g)
}

/// `fbf99`: store `xmm0`; active is "value > 0"; `al` is `bl`.
fn store(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let positive = above(c, lo(c, 0), lo(c, 1))?;
    c.edx = slot(c, g, 0x10)?;
    g.set_u32(c.edx, lo(c, 0))?;
    c.ecx = slot(c, g, 0x14)?;
    c.eax = u32::from(positive);
    g.set_u8(c.ecx, c.eax as u8)?;
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    ret(c, 0x10)
}
