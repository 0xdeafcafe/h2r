//! The small leaves `bd090` calls (docs/engine/object-update.md, "The
//! walks in Rust"): the node matrix address `b8bd0`, the animation and
//! attachment tests `bf5a0`, `bfc90` and `b6780`, and the five node-mask
//! operations `bff10`, `bfe80`, `bfed0` and `bfe20`. None calls guest
//! code and none makes a float instruction, so each runs as one segment
//! with the registers and the arithmetic flags retail returns with (from
//! the last instruction that sets them), and memory written in retail's
//! order. Pushes of `ebx` and `edi` that `bfed0` pops again leave the
//! stack below `esp` only.
//!
//! | Entry | In | Out |
//! |---|---|---|
//! | `b8bd0` | `eax` object, `[esp+4]` node (i16); `ret 4` | `eax` the node's matrix; `ecx` the body, `edx` its matrix base |
//! | `bf5a0` | `eax` object | `al` 1 when body `+12a` (word) is not `ffff`; `ecx` the same, `edx` the datum array |
//! | `bfc90` | `ecx` object | `al` 1 when the object is not none and its body `+b3` is not 0 |
//! | `bff10` | `ecx` mask | `al` 1 when all eight words are 0 |
//! | `bfe80` | `eax` mask | the eight words inverted; the last keeps its low byte |
//! | `bfed0` | `eax` bit count, `esi` mask | words past the count cleared, the partial word masked |
//! | `bfe20` | `eax` word, `cl` bit, `dl` set | that bit set or cleared |
//! | `b6780` | `eax` record | `eax` 1 when `+68`, `+0` and word `+6` are all not none |

use super::alu::cmp16;
use super::{Cx, Next, Result, unowned};
use crate::datum::layout::DATA;
use crate::points;
use crate::sim_physics::{arithmetic, byte, logic};
use runtime::{CpuAbi, Guest};

const NONE: u32 = u32::MAX;

fn at(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

fn ld(g: &Guest, a: u32, off: u32) -> Result<u32> {
    g.u32(at(a, off))
}

fn word(g: &Guest, a: u32) -> Result<u32> {
    Ok(g.u16(a)? as i16 as i32 as u32)
}

/// `mov ecx,[4e0300]; mov edx,[ecx+44]; and eax,ffff; lea eax,[eax*3];
/// mov <r>,[edx+eax*4+8]`: `eax` the object (handle) in, the data array
/// in `edx` and the body returned; `eax` is left `index * 3` and the flags
/// those of the `and`.
fn body(c: &mut CpuAbi, g: &Guest, handle: u32) -> Result<u32> {
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = ld(g, c.ecx, DATA)?;
    let index = handle & 0xffff;
    logic(c, index, 1 << 31);
    c.eax = index.wrapping_mul(3);
    ld(g, c.edx, c.eax.wrapping_mul(4).wrapping_add(8))
}

/// `b8bd0 (object: eax, node: stack 0) -> eax`; `ret 4`.
pub(super) fn node_matrix(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_NODE_MATRIX.base {
        return Err(unowned(pc));
    }
    c.ecx = body(c, g, c.eax)?;
    let node = word(g, at(c.esp, 4))?;
    c.edx = word(g, at(c.ecx, 0x116))?;
    // `imul eax, 34h` (flags overwritten by the add that follows).
    let offset = node.wrapping_mul(0x34);
    c.edx = at(c.edx, c.ecx);
    c.eax = arithmetic(c, offset, c.edx, false);
    Ok(Next::Ret { pop: 4 })
}

/// `bf5a0 (object: eax) -> al`; `ret`.
pub(super) fn has_animation(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_HAS_ANIMATION.base {
        return Err(unowned(pc));
    }
    c.eax = body(c, g, c.eax)?;
    let v = u32::from(g.u16(at(c.eax, 0x12a))?);
    // `xor ecx,ecx` then `cmp word [eax+12a],ffff`.
    cmp16(c, v, 0xffff);
    let set = u32::from(v != 0xffff);
    c.ecx = set;
    c.eax = byte(c.eax, set as u8);
    Ok(Next::Ret { pop: 0 })
}

/// `bfc90 (object: ecx) -> al`; `ret`.
pub(super) fn attached(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_ATTACHED.base {
        return Err(unowned(pc));
    }
    c.eax = byte(c.eax, 0);
    arithmetic(c, c.ecx, NONE, true);
    if c.ecx == NONE {
        return Ok(Next::Ret { pop: 0 });
    }
    let handle = c.ecx;
    c.edx = g.u32(points::OBJECTS.base)?;
    c.edx = ld(g, c.edx, DATA)?;
    logic(c, handle & 0xffff, 1 << 31);
    c.ecx = (handle & 0xffff).wrapping_mul(3);
    c.ecx = ld(g, c.edx, c.ecx.wrapping_mul(4).wrapping_add(8))?;
    let dl = g.u8(at(c.ecx, 0xb3))?;
    c.edx = byte(c.edx, dl);
    logic(c, u32::from(dl), 0x80);
    if dl != 0 {
        c.eax = byte(c.eax, 1);
    }
    Ok(Next::Ret { pop: 0 })
}

/// `bff10 (mask: ecx) -> al`; `ret`. The words are read from the last
/// (`+1c`) down; each `sete` replaces the low byte of the register that
/// held the word.
pub(super) fn mask_empty(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::NODE_MASK_EMPTY.base {
        return Err(unowned(pc));
    }
    let mask = c.ecx;
    c.eax = ld(g, mask, 0x1c)?;
    c.edx = ld(g, mask, 0x18)?;
    let mut all = u8::from(c.eax == 0);
    c.eax = byte(c.eax, all);
    c.edx = byte(c.edx, u8::from(c.edx == 0));
    all &= c.edx as u8;
    for off in [0x14, 0x10, 0x0c, 0x08, 0x04] {
        c.edx = ld(g, mask, off)?;
        c.edx = byte(c.edx, u8::from(c.edx == 0));
        all &= c.edx as u8;
    }
    let first = g.u32(mask)?;
    c.ecx = byte(c.ecx, u8::from(first == 0));
    all &= c.ecx as u8;
    all &= 1;
    logic(c, u32::from(all), 0x80);
    c.eax = byte(c.eax, all);
    Ok(Next::Ret { pop: 0 })
}

/// `bfe80 (mask: eax)`; `ret`. Inverts the eight words in place, then
/// stores the last word's complement masked to its low byte (the code
/// writes the full word first, then the masked one).
pub(super) fn mask_invert(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::NODE_MASK_INVERT.base {
        return Err(unowned(pc));
    }
    let mask = c.eax;
    // Retail alternates `ecx` and `edx`: it loads two words ahead of the
    // store of the first.
    c.ecx = ld(g, mask, 0)?;
    c.edx = ld(g, mask, 4)?;
    c.ecx = !c.ecx;
    g.set_u32(mask, c.ecx)?;
    c.ecx = ld(g, mask, 8)?;
    c.edx = !c.edx;
    g.set_u32(at(mask, 4), c.edx)?;
    c.edx = ld(g, mask, 0xc)?;
    c.ecx = !c.ecx;
    g.set_u32(at(mask, 8), c.ecx)?;
    c.ecx = ld(g, mask, 0x10)?;
    c.edx = !c.edx;
    g.set_u32(at(mask, 0xc), c.edx)?;
    c.edx = ld(g, mask, 0x14)?;
    c.ecx = !c.ecx;
    g.set_u32(at(mask, 0x10), c.ecx)?;
    c.ecx = ld(g, mask, 0x18)?;
    c.edx = !c.edx;
    g.set_u32(at(mask, 0x14), c.edx)?;
    c.edx = ld(g, mask, 0x1c)?;
    c.ecx = !c.ecx;
    g.set_u32(at(mask, 0x18), c.ecx)?;
    c.edx = !c.edx;
    c.ecx = c.edx & 0xff;
    logic(c, c.ecx, 1 << 31);
    g.set_u32(at(mask, 0x1c), c.edx)?;
    g.set_u32(at(mask, 0x1c), c.ecx)?;
    Ok(Next::Ret { pop: 0 })
}

/// `REP STOSD` of `eax` at `edi`, honouring the direction flag.
fn rep_stosd(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let step = if c.eflags & 0x400 == 0 { 4u32 } else { 4u32.wrapping_neg() };
    while c.ecx != 0 {
        g.set_u32(c.edi, c.eax)?;
        c.edi = c.edi.wrapping_add(step);
        c.ecx -= 1;
    }
    Ok(())
}

/// `bfed0 (count: eax, mask: esi)`; `ret`. `ebx` and `edi` are pushed and
/// popped, so they return unchanged.
pub(super) fn mask_trim(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::NODE_MASK_TRIM.base {
        return Err(unowned(pc));
    }
    let count = c.eax;
    let last = ((count.wrapping_add(0x1f)) as i32 >> 5).wrapping_sub(1);
    c.edx = last as u32;
    let next = c.edx.wrapping_add(1);
    c.eax = next;
    if (next as i32) < 8 {
        let edi = c.edi;
        c.ecx = 8u32.wrapping_sub(next);
        c.edi = at(c.esi, next.wrapping_mul(4));
        c.eax = 0;
        rep_stosd(c, g)?;
        c.edi = edi;
    }
    let low = count & 0x1f;
    c.eax = NONE;
    // `jle` after `and ebx, 1f`: taken when the result is zero.
    if low != 0 {
        c.ecx = 0x20 - low;
        // `or eax,eax` then `shr eax,cl`; the flags are overwritten below.
        c.eax >>= c.ecx & 31;
    }
    let addr = at(c.esi, c.edx.wrapping_mul(4));
    let v = g.u32(addr)? & c.eax;
    g.set_u32(addr, v)?;
    logic(c, v, 1 << 31);
    Ok(Next::Ret { pop: 0 })
}

/// `bfe20 (word: eax, bit: cl, set: dl)`; `ret`.
pub(super) fn mask_set(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::NODE_MASK_SET.base {
        return Err(unowned(pc));
    }
    let bit = 1u32.wrapping_shl(c.ecx);
    if c.edx & 0xff != 0 {
        c.edx = bit;
        let v = g.u32(c.eax)? | bit;
        g.set_u32(c.eax, v)?;
        logic(c, v, 1 << 31);
    } else {
        c.edx = !bit;
        c.ecx = g.u32(c.eax)? & c.edx;
        logic(c, c.ecx, 1 << 31);
        g.set_u32(c.eax, c.ecx)?;
    }
    Ok(Next::Ret { pop: 0 })
}

/// `b6780 (record: eax) -> eax`; `ret`.
pub(super) fn animation_valid(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::ANIMATION_VALID.base {
        return Err(unowned(pc));
    }
    c.edx = ld(g, c.eax, 0x68)?;
    c.ecx = NONE;
    let set = 'valid: {
        if arithmetic(c, c.edx, NONE, true) == 0 {
            break 'valid false;
        }
        let v = g.u32(c.eax)?;
        arithmetic(c, v, NONE, true);
        if v == NONE {
            break 'valid false;
        }
        let w = u32::from(g.u16(at(c.eax, 6))?);
        cmp16(c, w, 0xffff);
        w != 0xffff
    };
    if set {
        c.eax = 1;
    } else {
        c.eax = 0;
        logic(c, 0, 1 << 31);
    }
    Ok(Next::Ret { pop: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const STACK: u32 = 0x0057_0800;
    const MASK: u32 = 0x0057_0100;

    fn guest() -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0057_0000, 0x1000, Permissions::RW).unwrap();
        g.set_u32(STACK, 0x1234).unwrap();
        (CpuAbi { esp: STACK, ..Default::default() }, g)
    }

    fn words(g: &Guest) -> Vec<u32> {
        (0..8).map(|i| g.u32(MASK + 4 * i).unwrap()).collect()
    }

    #[test]
    fn a_trim_clears_the_words_past_the_count_and_masks_the_partial_word() {
        let (mut cpu, mut g) = guest();
        for i in 0..8 {
            g.set_u32(MASK + 4 * i, u32::MAX).unwrap();
        }
        (cpu.eax, cpu.esi) = (36, MASK);
        let next = mask_trim(&mut Cx { cpu: &mut cpu, g: &mut g }, points::NODE_MASK_TRIM.base).unwrap();
        assert_eq!(next, Next::Ret { pop: 0 });
        assert_eq!(words(&g), [u32::MAX, 0xf, 0, 0, 0, 0, 0, 0]);
        assert_eq!((cpu.edx, cpu.ecx), (1, 28));
    }

    #[test]
    fn a_set_sets_or_clears_one_bit_and_an_empty_test_reads_the_words() {
        let (mut cpu, mut g) = guest();
        (cpu.eax, cpu.ecx, cpu.edx) = (MASK, 0x4000_0003, 1);
        mask_set(&mut Cx { cpu: &mut cpu, g: &mut g }, points::NODE_MASK_SET.base).unwrap();
        assert_eq!(g.u32(MASK).unwrap(), 8);
        (cpu.eax, cpu.ecx) = (MASK, MASK);
        mask_empty(&mut Cx { cpu: &mut cpu, g: &mut g }, points::NODE_MASK_EMPTY.base).unwrap();
        assert_eq!(cpu.eax & 0xff, 0);
        (cpu.eax, cpu.ecx, cpu.edx) = (MASK, 3, 0);
        mask_set(&mut Cx { cpu: &mut cpu, g: &mut g }, points::NODE_MASK_SET.base).unwrap();
        assert_eq!(g.u32(MASK).unwrap(), 0);
        cpu.ecx = MASK;
        mask_empty(&mut Cx { cpu: &mut cpu, g: &mut g }, points::NODE_MASK_EMPTY.base).unwrap();
        assert_eq!(cpu.eax & 0xff, 1);
    }

    #[test]
    fn an_invert_flips_the_words_and_keeps_the_last_word_a_byte() {
        let (mut cpu, mut g) = guest();
        cpu.eax = MASK;
        mask_invert(&mut Cx { cpu: &mut cpu, g: &mut g }, points::NODE_MASK_INVERT.base).unwrap();
        assert_eq!(words(&g), [u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0xff]);
    }

    #[test]
    fn an_animation_is_valid_when_none_of_its_three_fields_is_none() {
        let (mut cpu, mut g) = guest();
        g.set_u32(MASK + 0x68, 7).unwrap();
        g.set_u32(MASK, 9).unwrap();
        g.set_u16(MASK + 6, 4).unwrap();
        cpu.eax = MASK;
        animation_valid(&mut Cx { cpu: &mut cpu, g: &mut g }, points::ANIMATION_VALID.base).unwrap();
        assert_eq!((cpu.eax, cpu.ecx), (1, u32::MAX));
        g.set_u16(MASK + 6, 0xffff).unwrap();
        cpu.eax = MASK;
        animation_valid(&mut Cx { cpu: &mut cpu, g: &mut g }, points::ANIMATION_VALID.base).unwrap();
        assert_eq!(cpu.eax, 0);
    }
}
