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
//! | `badc0` | `eax` object, `[esp+4]` type bits; `ret 4` | `eax` the body when the handle is live and its type (datum `+3`) has its bit set, else 0 |

use super::alu::cmp16;
use super::{Cx, Next, Result, unowned};
use crate::datum::layout::{DATA, ELEMENT_SIZE, HIGH_WATER};
use crate::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
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

/// `badc0 (object: eax, types: stack 0) -> eax`; `ret 4`. The datum is
/// found when the handle is not none, its index is below the high-water
/// mark (a signed compare), its salt word is not 0 and equals the handle's
/// high half; its body is answered when bit `[datum+3]` of `types` is set.
/// The flags are those of the last `test`: of the datum (none found) or of
/// the type bit.
pub(super) fn object_get_typed(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_GET_TYPED.base {
        return Err(unowned(pc));
    }
    push(c, g, c.esi)?;
    push(c, g, c.edi)?;
    let handle = c.eax;
    c.esi = handle;
    c.edi = 0;
    'found: {
        if handle == NONE {
            break 'found;
        }
        c.edx = g.u32(points::OBJECTS.base)?;
        let high = ld(g, c.edx, HIGH_WATER)?;
        c.eax = high;
        c.ecx = handle & 0xffff;
        if (c.ecx as i32) >= (high as i32) {
            break 'found;
        }
        let datum = ld(g, c.edx, ELEMENT_SIZE)?.wrapping_mul(c.ecx).wrapping_add(ld(g, c.edx, DATA)?);
        c.eax = datum;
        let salt = u32::from(g.u16(datum)?);
        c.ecx = (c.ecx & !0xffff) | salt;
        if salt == 0 {
            break 'found;
        }
        c.ecx = salt as u16 as i16 as i32 as u32;
        c.esi = ((handle as i32) >> 16) as u32;
        if c.ecx != c.esi {
            break 'found;
        }
        c.edi = datum;
    }
    c.eax = 0;
    logic(c, c.edi, 1 << 31);
    if c.edi != 0 {
        let kind = g.u8(at(c.edi, 3))?;
        c.ecx = byte(c.ecx, kind);
        c.edx = 1u32 << (kind & 31);
        // `test [esp+0c], edx`: the type bits past the two pushes.
        let types = ld(g, c.esp, 0xc)?;
        logic(c, types & c.edx, 1 << 31);
        if types & c.edx != 0 {
            c.eax = ld(g, c.edi, 8)?;
        }
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
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

    /// The object pool at `0x0057_0200`: header, two 12-byte datums at
    /// `+0x100` (slot 1 salt `0x8201`, type 3; slot 0 free), high-water 2.
    fn pool(g: &mut Guest) -> u32 {
        let header = 0x0057_0200;
        g.set_u32(points::OBJECTS.base, header).unwrap();
        g.set_u32(header + ELEMENT_SIZE, 12).unwrap();
        g.set_u32(header + HIGH_WATER, 2).unwrap();
        g.set_u32(header + DATA, header + 0x100).unwrap();
        let slot1 = header + 0x100 + 12;
        g.set_u16(slot1, 0x8201).unwrap();
        g.set_u8(slot1 + 3, 3).unwrap();
        g.set_u32(slot1 + 8, 0x0057_0400).unwrap();
        slot1
    }

    fn typed(g: &mut Guest, handle: u32, types: u32) -> CpuAbi {
        g.map_zeroed(points::OBJECTS.base & !0xfff, 0x1000, Permissions::RW).ok();
        pool(g);
        g.set_u32(STACK + 4, types).unwrap();
        let mut cpu = CpuAbi { esp: STACK, eax: handle, ecx: 0x1111, edx: 0x2222, esi: 0x3333, edi: 0x4444, ..Default::default() };
        let next = object_get_typed(&mut Cx { cpu: &mut cpu, g }, points::OBJECT_GET_TYPED.base).unwrap();
        assert_eq!(next, Next::Ret { pop: 4 });
        assert_eq!((cpu.esp, cpu.esi, cpu.edi), (STACK, 0x3333, 0x4444));
        cpu
    }

    #[test]
    fn a_typed_get_answers_the_body_of_a_live_handle_whose_type_bit_is_set() {
        let (_, mut g) = guest();
        let cpu = typed(&mut g, 0x8201_0001, 1 << 3);
        assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (0x0057_0400, 0xffff_8203, 8));
        assert_eq!(cpu.eflags & 0x40, 0, "the type bit tested set");
        let cpu = typed(&mut g, 0x8201_0001, u32::MAX);
        assert_eq!(cpu.eax, 0x0057_0400);
    }

    #[test]
    fn a_typed_get_answers_0_for_none_a_stale_salt_a_free_slot_a_slot_past_the_mark_or_another_type() {
        let (_, mut g) = guest();
        // None: nothing read, `ecx` and `edx` kept.
        let cpu = typed(&mut g, NONE, u32::MAX);
        assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (0, 0x1111, 0x2222));
        assert_eq!(cpu.eflags & 0x40, 0x40);
        // Past the high-water mark: `ecx` the index.
        let cpu = typed(&mut g, 0x8201_0002, u32::MAX);
        assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (0, 2, 0x0057_0200));
        // A free slot (salt 0) and a stale salt.
        let cpu = typed(&mut g, 0x8200_0000, u32::MAX);
        assert_eq!((cpu.eax, cpu.ecx), (0, 0));
        let cpu = typed(&mut g, 0x8202_0001, u32::MAX);
        assert_eq!((cpu.eax, cpu.ecx), (0, 0xffff_8201));
        // Another type: the datum found, the bit clear.
        let cpu = typed(&mut g, 0x8201_0001, !(1 << 3));
        assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (0, 0xffff_8203, 8));
        assert_eq!(cpu.eflags & 0x40, 0x40);
    }
}
