//! `object_new` `b7b40 (placement: stack 0) -> eax`, `ret 4`: make an
//! object from a placement record (docs/engine/object-update.md, "Walk
//! callouts").
//!
//! The placement's definition tag (`+0`, none fails at once) names the
//! definition, whose word `+0` is the type and `+38` the model tag. The
//! body allocates the datum through `bc280`, fills the header and the body
//! from the placement (`+4..+b2`), sizes the body's blocks through `bc380`,
//! runs the type new dispatch (`108a90`), and on success the colour,
//! attachment, node and wake setup; a failed allocation or block sizing
//! deletes the datum (`108bf0`, `bc300`) and returns none.
//!
//! A segment body: every callee is a guest call, so at each call `esp`,
//! the frame, all eight registers and the flags the last instruction
//! defined hold what retail holds. It touches `xmm0` to `xmm3` (`b81e1`,
//! `b82f7`) and MXCSR through `comiss`, so it is not float free.
//!
//! Frame, with `F` the `esp` after `sub esp, b8`: the result handle at
//! `F+4`, the byte `F+b` (set when a block sizing fails, passed to
//! `108a90`), the definition at `F+10`, the model definition (later the
//! body) at `F+c`, the header offset at `F+14`, the header at `F+24`, the
//! model's `+78`, `+70` and region values at `F+1c`, `F+18` and `F+20`,
//! the byte `F+3` (`1cb0d0` found a node, later body `+4` bit 17) and
//! `1caed0`'s record at `F+28`. The four pushes (`ebx`, `ebp`, then
//! `esi`, `edi` from `b7b9e` and `b7c0a`) sit below `F`.

use super::alu::{cmp16, rmw8, shl};
use super::frame::{pop, push, set_slot, slot};
use super::sse::{self, below_or_equal};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, status};
use runtime::{CpuAbi, Guest};

const ENTRY: u32 = 0x000b_7b40;
const FRAME: u32 = 0xb8;
const NONE: u32 = u32::MAX;
const SIGN: u32 = 1 << 31;
/// The type descriptors, `[468630 + 4 * type]`.
const TYPES: u32 = 0x0046_8630;
/// The object globals: `+18` the creation counter, byte `+0` its active latch.
const OBJECT_GLOBALS: u32 = 0x004d_e2f4;
/// The loaded BSP index.
const BSP_INDEX: u32 = 0x0046_86c4;
/// f32 1.0.
const ONE: u32 = 0x0045_dbc0;

// Guest callees. `bc280` takes the descriptor's datum size and answers
// the handle; `bc380` sizes one body block (`object: eax`, then the
// field offset, size and alignment on the stack; `ret c`, `al`).
const C_108A40: u32 = 0x0010_8a40;
const C_146BF0: u32 = 0x0014_6bf0;
const C_BC280: u32 = 0x000b_c280;
const C_BF9A0: u32 = 0x000b_f9a0;
const C_B9D20: u32 = 0x000b_9d20;
const C_B8B70: u32 = 0x000b_8b70;
const C_1CAED0: u32 = 0x001c_aed0;
const C_1CB0D0: u32 = 0x001c_b0d0;
const C_1CB0A0: u32 = 0x001c_b0a0;
const C_BC380: u32 = 0x000b_c380;
const C_1C3930: u32 = 0x001c_3930;
const C_1CAFC0: u32 = 0x001c_afc0;
const C_BA540: u32 = 0x000b_a540;
const C_BA590: u32 = 0x000b_a590;
const C_BE240: u32 = 0x000b_e240;
const C_D5B60: u32 = 0x000d_5b60;
const C_BA3D0: u32 = 0x000b_a3d0;
const C_BD020: u32 = 0x000b_d020;
const C_BF510: u32 = 0x000b_f510;
const C_B8600: u32 = 0x000b_8600;
const C_B8890: u32 = 0x000b_8890;
const C_BE690: u32 = 0x000b_e690;
const C_108B80: u32 = 0x0010_8b80;
const C_176780: u32 = 0x0017_6780;
const C_2E1E0: u32 = 0x0002_e1e0;
const C_109390: u32 = 0x0010_9390;
const C_B8820: u32 = 0x000b_8820;
const C_108BF0: u32 = 0x0010_8bf0;
const C_BC300: u32 = 0x000b_c300;

/// The return address of every guest call, in code order.
pub(super) const NEW_SITES: [u32; 43] = [
    0xb7b73, 0xb7bd4, 0xb7be0, 0xb7d65, 0xb7dfe, 0xb7e10, 0xb7e19, 0xb7f2c, 0xb7f45, 0xb7f99, //
    0xb7fbd, 0xb7fdf, 0xb8003, 0xb8023, 0xb8043, 0xb8063, 0xb8083, 0xb80a4, 0xb80b0, 0xb80ee, //
    0xb8105, 0xb816b, 0xb81b1, 0xb81be, 0xb81cc, 0xb81df, 0xb823a, 0xb8264, 0xb826a, 0xb828e, //
    0xb82ad, 0xb82b4, 0xb82bb, 0xb82df, 0xb82e4, 0xb82eb, 0xb830b, 0xb8312, 0xb8337, 0xb8351, //
    0xb836d, 0xb8383, 0xb838d,
];

fn at(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

fn rd(g: &Guest, a: u32) -> Result<u32> {
    g.u32(a)
}

fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

fn lo16(r: u32, v: u16) -> u32 {
    (r & !0xffff) | u32::from(v)
}

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

/// `test al, al`; true when `al` is not zero.
fn test_al(c: &mut CpuAbi) -> bool {
    let al = c.eax & 0xff;
    logic(c, al, 0x80);
    al != 0
}

/// `imul`: CF and OF the overflow; SF, ZF, AF and PF undefined.
fn imul_flags(c: &mut CpuAbi, overflow: bool) {
    c.eflags = (c.eflags & !ARITH) | if overflow { 0x801 } else { 0 };
    c.eflags_defined = (c.eflags_defined | ARITH) & !0xd4;
}

/// `cmp byte, 0`.
fn cmp8_zero(c: &mut CpuAbi, b: u8) {
    c.eflags = (c.eflags & !ARITH) | status(b.into(), 0x80);
    c.eflags_defined |= ARITH;
}

/// `or r32, bits` (`on`) or `and r32, ~bits`, with the flags of the
/// operation.
fn set_bits(c: &mut CpuAbi, v: u32, bits: u32, on: bool) -> u32 {
    let r = if on { v | bits } else { v & !bits };
    logic(c, r, SIGN);
    r
}

/// `b7b40 (placement: stack 0) -> eax`; `ret 4`.
pub(super) fn object_new(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        ENTRY => entry(c, g),
        0xb7b73 => b7b73(c, g),
        0xb7bd4 => b7bd4(c, g),
        0xb7be0 => b7be0(c, g),
        0xb7d65 => b7d65(c, g),
        0xb7dfe => b7dfe(c, g),
        0xb7e10 => b7e10(c, g),
        0xb7e19 => {
            c.esi = slot(c, g, 0x34)?;
            b7e1d(c, g)
        }
        0xb7f2c => b7f2c(c, g),
        0xb7f45 => b7f45(c, g),
        0xb7f99 => b7f99(c, g),
        0xb7fbd | 0xb7fdf | 0xb8003 | 0xb8023 | 0xb8043 | 0xb8063 | 0xb8083 | 0xb80a4 => blocks(c, g, pc),
        0xb80b0 => {
            if !test_al(c) {
                return b80b8(c, g);
            }
            c.eax = byte(c.eax, 1);
            b80bf(c, g)
        }
        0xb80ee => b80ee(c, g),
        0xb8105 => b8105(c, g),
        0xb816b => b816b(c, g),
        0xb8383 => {
            c.esi = slot(c, g, 0x14)?;
            push(c, g, c.esi)?;
            call(C_BC300, 0xb838d)
        }
        0xb838d => {
            set_slot(c, g, 0x14, NONE)?;
            finish(c, g, 2)
        }
        0xb81b1 => {
            c.eax = rd(g, at(c.ebp, 0xac))?;
            push(c, g, c.eax)?;
            push(c, g, c.ebx)?;
            call(C_BA590, 0xb81be)
        }
        0xb81be => {
            c.edx = rd(g, at(c.ebp, 0x74))?;
            c.ecx = at(c.ebp, 0x78);
            push(c, g, c.ecx)?;
            push(c, g, c.edx)?;
            push(c, g, c.ebx)?;
            call(C_BE240, 0xb81cc)
        }
        0xb81cc => {
            c.eax = rd(g, points::OBJECTS.base)?;
            c.ecx = rd(g, at(c.eax, DATA))?;
            c.edi = rd(g, at(at(c.ecx, c.esi), 8))?;
            c.ecx = c.ebx;
            call(C_D5B60, 0xb81df)
        }
        0xb81df => b81df(c, g),
        0xb823a => b823a(c, g),
        0xb8264 => b8264(c, g),
        0xb826a => b826a(c, g),
        0xb828e => {
            let al = c.eax as u8;
            logic(c, al.into(), 0x80);
            c.edi = slot(c, g, 0x1c)?;
            g.set_u8(at(c.ebp, 0xb8), al)?;
            if al == 0 {
                c.eax = 0;
                logic(c, 0, SIGN);
                return b82a6(c, g);
            }
            c.eax = at(c.ebp, 0xbc);
            b82a6(c, g)
        }
        0xb82ad => b82ad(c),
        0xb82b4 => {
            c.eax = c.ebx;
            call(points::OBJECT_WAKE.base, 0xb82bb)
        }
        0xb82bb => {
            c.eax = rd(g, at(c.ebp, 0x18))? >> 5;
            let on = c.eax & 1 != 0;
            rmw8(c, g, at(c.edi, 0xc0), |b| if on { b | 2 } else { b & 0xfd })?;
            c.esi = slot(c, g, 0x14)?;
            push(c, g, c.esi)?;
            call(C_B8890, 0xb82df)
        }
        0xb82df => call(C_BE690, 0xb82e4),
        0xb82e4 => {
            c.ebx = c.esi;
            call(C_108B80, 0xb82eb)
        }
        0xb82eb => {
            c.ecx = slot(c, g, 0x20)?;
            c.eax = rd(g, at(c.ecx, 0x50))?;
            arithmetic(c, c.eax, NONE, true);
            if c.eax == NONE {
                return b830b(c);
            }
            sse::zero(c, 0);
            for _ in 0..3 {
                push(c, g, 0)?;
            }
            push(c, g, c.eax)?;
            c.edx = c.esi;
            c.ebx = at(c.ebp, 0x68);
            call(C_176780, 0xb830b)
        }
        0xb830b => b830b(c),
        0xb8312 => {
            let al = g.u8(at(c.esp, 0x13))?;
            c.eax = byte(c.eax, al);
            c.edi = slot(c, g, 0x1c)?;
            let v = rd(g, at(c.edi, 4))?;
            c.eax = set_bits(c, v, 0x20000, al != 0);
            c.edx = c.esi;
            g.set_u32(at(c.edi, 4), c.eax)?;
            call(C_109390, 0xb8337)
        }
        0xb8337 => {
            c.edx = rd(g, at(c.edi, 4))? >> 17;
            logic(c, c.edx & 1, 0x80);
            if c.edx & 1 == 0 {
                return finish(c, g, 2);
            }
            c.eax = slot(c, g, 0x34)?;
            let t = g.u8(at(c.eax, 2))? & 1;
            logic(c, t.into(), 0x80);
            if t != 0 {
                return finish(c, g, 2);
            }
            call(C_B8820, 0xb8351)
        }
        0xb8351 => {
            if !test_al(c) {
                return finish(c, g, 2);
            }
            c.eax = rd(g, at(c.ebp, 0x18))?;
            logic(c, c.eax & 2, 0x80);
            if c.eax & 2 != 0 {
                return finish(c, g, 2);
            }
            logic(c, c.eax & 4, 0x80);
            if c.eax & 4 != 0 {
                let w = g.u16(at(c.edi, 0x2c))?;
                cmp16(c, w.into(), 0xffff);
                if w == 0xffff {
                    return finish(c, g, 2);
                }
            }
            push(c, g, c.esi)?;
            call(points::OBJECT_DELETE.base, 0xb836d)
        }
        0xb836d => {
            c.eax = slot(c, g, 0x14)?;
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            c.ebx = pop(c, g)?;
            c.esp = arithmetic(c, c.esp, FRAME, false);
            Ok(Next::Ret { pop: 4 })
        }
        _ => Err(unowned(pc)),
    }
}

/// `b8397` (`pushed` 0), `b8396` (1: `pop esi`) or `b8395` (2: `pop edi;
/// pop esi`): the result at `F+4`, restore, `ret 4`.
fn finish(c: &mut CpuAbi, g: &Guest, pushed: u32) -> Result<Next> {
    if pushed >= 2 {
        c.edi = pop(c, g)?;
    }
    if pushed >= 1 {
        c.esi = pop(c, g)?;
    }
    c.eax = slot(c, g, 0xc)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 4 })
}

fn entry(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esp = arithmetic(c, c.esp, FRAME, true);
    push(c, g, c.ebx)?;
    push(c, g, c.ebp)?;
    c.ebp = slot(c, g, 0xc4)?;
    let t = g.u8(at(c.ebp, 0x18))? & 0x10;
    logic(c, t.into(), 0x80);
    set_slot(c, g, 0xc, NONE)?;
    g.set_u8(at(c.esp, 0x13), 0)?;
    if t != 0 {
        return b7b73(c, g);
    }
    let tag = rd(g, c.ebp)?;
    arithmetic(c, tag, NONE, true);
    if tag == NONE {
        return finish(c, g, 0);
    }
    c.ebx = c.ebp;
    call(C_108A40, 0xb7b73)
}

/// The definition and its type descriptor; `146bf0` for the types in
/// `1883`.
fn b7b73(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = rd(g, c.ebp)?;
    arithmetic(c, c.eax, NONE, true);
    if c.eax == NONE {
        return finish(c, g, 0);
    }
    c.edx = rd(g, points::TAG_ENTRIES.base)?;
    c.eax = (c.eax & 0xffff) << 4;
    c.eax = rd(g, at(at(c.eax, c.edx), 8))?;
    c.ecx = sx16(g.u16(c.eax)?);
    set_slot(c, g, 0x18, c.eax)?;
    c.eax = rd(g, at(c.eax, 0x38))?;
    let none = c.eax == NONE;
    arithmetic(c, c.eax, NONE, true);
    push(c, g, c.esi)?;
    c.esi = rd(g, at(TYPES, c.ecx.wrapping_mul(4)))?;
    set_slot(c, g, 0x18, 0)?;
    if !none {
        c.eax = rd(g, at(at((c.eax & 0xffff) << 4, c.edx), 8))?;
        set_slot(c, g, 0x18, c.eax)?;
    }
    c.edx = 1u32 << (c.ecx & 31);
    let t = c.edx & 0x1883;
    logic(c, t, SIGN);
    if t != 0 {
        return call(C_146BF0, 0xb7bd4);
    }
    b7bd4(c, g)
}

fn b7bd4(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    logic(c, 0, SIGN);
    c.eax = g.u16(at(c.esi, 8))?.into();
    push(c, g, c.eax)?;
    call(C_BC280, 0xb7be0)
}

/// The new datum: header, body `+0..+a0` from the placement, the flags of
/// `+4`, then `bf9a0`.
fn b7be0(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    arithmetic(c, c.eax, NONE, true);
    set_slot(c, g, 0x10, c.eax)?;
    if c.eax == NONE {
        return finish(c, g, 1);
    }
    c.ecx = rd(g, points::OBJECTS.base)?;
    c.edx = rd(g, at(c.ecx, DATA))?;
    c.eax = shl(c, (c.eax & 0xffff).wrapping_mul(3), 2, 32);
    c.esi = at(c.eax, c.edx);
    let dl = g.u8(at(c.esi, 2))? | 8;
    c.edx = byte(c.edx, dl);
    logic(c, dl.into(), 0x80);
    push(c, g, c.edi)?;
    c.edi = rd(g, at(c.esi, 8))?;
    g.set_u8(at(c.esi, 2), dl)?;
    set_slot(c, g, 0x24, c.eax)?;
    c.eax = slot(c, g, 0x20)?;
    let cl = g.u8(c.eax)?;
    c.ecx = byte(c.ecx, cl);
    g.set_u8(at(c.esi, 3), cl)?;
    c.edx = rd(g, c.ebp)?;
    g.set_u32(c.edi, c.edx)?;
    let b = g.u8(at(c.ebp, 0xb))?;
    arithmetic(c, b.into(), 0xff, true);
    set_slot(c, g, 0x34, c.esi)?;
    g.set_u8(at(c.esp, 0x13), 0)?;
    if b == 0xff {
        c.ecx = sx16(g.u16(c.eax)?);
        c.eax = rd(g, OBJECT_GLOBALS)?;
        let n = rd(g, at(c.eax, 0x18))?.wrapping_add(1);
        g.set_u32(at(c.eax, 0x18), n)?;
        g.set_u8(at(c.edi, 0xaa), c.ecx as u8)?;
        let cl = g.u8(BSP_INDEX)?;
        c.ecx = byte(c.ecx, cl);
        c.ebx = NONE;
        logic(c, NONE, SIGN);
        g.set_u8(at(c.edi, 0xab), 2)?;
        g.set_u16(at(c.edi, 0xa8), c.ebx as u16)?;
        c.eax = rd(g, at(c.eax, 0x18))?;
        g.set_u32(at(c.edi, 0xa4), c.eax)?;
        g.set_u16(at(c.edi, 0x1a), c.ebx as u16)?;
        g.set_u8(at(c.edi, 0xae), cl)?;
    } else {
        c.edx = rd(g, at(c.ebp, 4))?;
        g.set_u32(at(c.edi, 0xa4), c.edx)?;
        c.eax = rd(g, at(c.ebp, 8))?;
        g.set_u32(at(c.edi, 0xa8), c.eax)?;
        let cx = g.u16(at(c.ebp, 0x10))?;
        c.ecx = lo16(c.ecx, cx);
        c.edx = byte(c.edx, c.eax as u8);
        g.set_u16(at(c.edi, 0x1a), cx)?;
        g.set_u8(at(c.edi, 0xae), c.eax as u8)?;
        c.ebx = NONE;
        logic(c, NONE, SIGN);
    }
    // `b7c94`: five 12-byte vectors, then the dword `+58`.
    let mut last = [0u32; 3];
    for (src, dst) in [(0x1c, 0x64), (0x28, 0x70), (0x34, 0x7c), (0x40, 0x88), (0x4c, 0x94)] {
        for (i, l) in last.iter_mut().enumerate() {
            let k = 4 * i as u32;
            *l = rd(g, at(c.ebp, src + k))?;
            g.set_u32(at(c.edi, dst + k), *l)?;
        }
    }
    c.eax = last[1];
    c.ecx = last[2];
    c.edx = rd(g, at(c.ebp, 0x58))?;
    g.set_u32(at(c.edi, 0xa0), c.edx)?;
    let on = g.u8(at(c.ebp, 0x18))? & 1 != 0;
    let v = rd(g, at(c.edi, 4))?;
    c.eax = set_bits(c, v, 0x400, on);
    g.set_u32(at(c.edi, 4), c.eax)?;
    c.eax = slot(c, g, 0x1c)?;
    let has = c.eax != 0 && rd(g, at(c.eax, 0xc))? != c.ebx;
    c.eax = byte(c.eax, u8::from(has));
    let v = rd(g, at(c.edi, 4))?;
    c.eax = set_bits(c, v, 0x200, has);
    g.set_u32(at(c.edi, 4), c.eax)?;
    c.eax = at(c.esp, 0x28);
    push(c, g, c.eax)?;
    c.ecx = at(c.esp, 0x30);
    push(c, g, c.ecx)?;
    c.ecx = slot(c, g, 0x1c)?;
    call(C_BF9A0, 0xb7d65)
}

fn b7d65(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let on = c.eax & 0xff != 0;
    let v = rd(g, at(c.edi, 4))?;
    c.eax = set_bits(c, v, 0x8000_0000, on);
    let dx = g.u16(BSP_INDEX)?;
    c.edx = lo16(c.edx, dx);
    c.ecx = slot(c, g, 0x20)?;
    g.set_u32(at(c.edi, 4), c.eax)?;
    g.set_u16(at(c.esi, 4), c.ebx as u16)?;
    g.set_u32(at(c.edi, 0x28), c.ebx)?;
    g.set_u16(at(c.edi, 0x2c), c.ebx as u16)?;
    g.set_u16(at(c.edi, 0x2e), dx)?;
    g.set_u32(at(c.edi, 0x60), c.ebx)?;
    let al = g.u8(at(c.ebp, 0x14))?;
    c.eax = byte(c.eax, al);
    g.set_u8(at(c.edi, 0xb0), al)?;
    g.set_u32(at(c.edi, 0x14), c.ebx)?;
    g.set_u32(at(c.edi, 0x0c), c.ebx)?;
    g.set_u32(at(c.edi, 0x10), c.ebx)?;
    g.set_u16(at(c.edi, 0xac), c.ebx as u16)?;
    g.set_u8(at(c.edi, 0xaf), 0xff)?;
    g.set_u8(at(c.edi, 0x108), 0xff)?;
    g.set_u8(at(c.edi, 0x109), 0xff)?;
    let t = g.u8(at(c.ecx, 2))? & 1;
    logic(c, t.into(), 0x80);
    if t != 0 {
        let v = rd(g, at(c.edi, 4))? | 0x10000;
        g.set_u32(at(c.edi, 4), v)?;
        logic(c, v, SIGN);
    }
    c.edx = rd(g, points::OBJECTS.base)?;
    c.ecx = slot(c, g, 0x24)?;
    c.eax = rd(g, at(c.edx, DATA))?;
    c.eax = rd(g, at(at(c.ecx, c.eax), 8))?;
    let t = g.u8(at(c.eax, 4))? & 1;
    logic(c, t.into(), 0x80);
    if t == 0 {
        return b7e1d(c, g);
    }
    c.edx = rd(g, at(c.eax, 4))?;
    c.esi = slot(c, g, 0x14)?;
    c.edx &= !1;
    logic(c, c.edx, SIGN);
    g.set_u32(at(c.eax, 4), c.edx)?;
    c.eax = c.esi;
    call(C_B9D20, 0xb7dfe)
}

fn b7dfe(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if !test_al(c) {
        return b7e10(c, g);
    }
    for w in [0, 0, 1, 0, c.esi] {
        push(c, g, w)?;
    }
    call(points::OBJECT_ATTACHMENTS.base, 0xb7e10)
}

fn b7e10(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esi = slot(c, g, 0x14)?;
    call(C_B8B70, 0xb7e19)
}

/// The body's physics and ownership fields, `+b3`, then the model's
/// counts and the model iterator.
fn b7e1d(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let dx = g.u16(at(c.ebp, 0x70))?;
    c.edx = lo16(c.edx, dx);
    g.set_u16(at(c.edi, 0xc2), dx)?;
    c.eax = rd(g, at(c.ebp, 0x68))?;
    g.set_u32(at(c.edi, 0xc4), c.eax)?;
    c.ecx = rd(g, at(c.ebp, 0x6c))?;
    g.set_u32(at(c.edi, 0xc8), c.ecx)?;
    g.set_u8(at(c.edi, 0xb1), 0xff)?;
    g.set_u32(at(c.edi, 0xcc), c.ebx)?;
    g.set_u16(at(c.edi, 0xd0), c.ebx as u16)?;
    g.set_u16(at(c.edi, 0xc0), 0)?;
    c.edx = rd(g, at(c.ebp, 0x18))? >> 3;
    let on = c.edx & 1 != 0;
    rmw8(c, g, at(c.edi, 0xc1), |b| if on { b | 1 } else { b & 0xfe })?;
    g.set_u32(at(c.edi, 0xb4), c.ebx)?;
    g.set_u32(at(c.edi, 0xd4), c.ebx)?;
    g.set_u8(at(c.edi, 0xd8), 0)?;
    let ax = g.u16(at(c.ebp, 0xb0))?;
    c.eax = lo16(c.eax, ax);
    g.set_u16(at(c.edi, 0xe0), ax)?;
    let cx = g.u16(at(c.ebp, 0xb2))?;
    c.ecx = lo16(c.ecx, cx);
    g.set_u16(at(c.edi, 0xe2), cx)?;
    let cl = g.u8(at(c.esi, 3))?;
    c.ecx = byte(c.ecx, cl);
    c.edx = 1u32 << (cl & 31);
    logic(c, c.edx & 0xff, 0x80);
    let mut b3 = 0;
    if c.edx & 0x80 == 0 {
        c.ecx = rd(g, points::GAME_TIME.base)?;
        c.edx = rd(g, at(c.ecx, 8))?;
        c.eax = 6;
        arithmetic(c, c.edx, 6, true);
        if c.edx as i32 >= 6 {
            b3 = 6;
        }
    }
    g.set_u8(at(c.edi, 0xb3), b3)?;
    // `b7ed0`.
    c.eax = slot(c, g, 0x1c)?;
    c.edx = 1;
    c.ebx &= !0xff;
    logic(c, c.eax, SIGN);
    set_slot(c, g, 0x2c, 1)?;
    set_slot(c, g, 0x28, 1)?;
    set_slot(c, g, 0x30, 0)?;
    if c.eax == 0 {
        return b7f99(c, g);
    }
    for (field, out) in [(0x78, 0x2c), (0x70, 0x28)] {
        c.ecx = rd(g, at(c.eax, field))?;
        arithmetic(c, c.ecx, c.edx, true);
        if c.ecx as i32 >= c.edx as i32 {
            set_slot(c, g, out, c.ecx)?;
        }
    }
    c.ecx = rd(g, at(c.eax, 0x60))?;
    logic(c, c.ecx, SIGN);
    if c.ecx as i32 > 0 {
        c.edx = rd(g, at(c.eax, 0x64))?;
        c.ecx = rd(g, at(c.edx, 0xbc))?;
        set_slot(c, g, 0x30, c.ecx)?;
    }
    let v = rd(g, at(c.eax, 0x14))?;
    arithmetic(c, v, NONE, true);
    if v == NONE {
        return b7f99(c, g);
    }
    c.eax = at(c.esp, 0x38);
    call(C_1CAED0, 0xb7f2c)
}

fn b7f2c(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = slot(c, g, 0x1c)?;
    c.edx = slot(c, g, 0x20)?;
    c.edx = rd(g, at(c.edx, 0x38))?;
    c.edi = rd(g, at(c.eax, 0x14))?;
    push(c, g, 1)?;
    c.esi = at(c.esp, 0x3c);
    call(C_1CB0D0, 0xb7f45)
}

/// The model has a node: the "has model" byte, `bl` the 32-entry block
/// switch, then `1cb0a0`.
fn b7f45(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if test_al(c) {
        c.ecx = slot(c, g, 0x20)?;
        let cl = g.u8(c.ecx)?;
        c.ecx = byte(c.ecx, cl);
        c.eax = 1u32 << (cl & 31);
        g.set_u8(at(c.esp, 0x13), 1)?;
        let m = c.eax & 0x7e0;
        c.ebx = byte(m.wrapping_neg(), u8::from(m == 0));
        let t = c.eax & 0x380;
        logic(c, t, SIGN);
        if t != 0 {
            c.edx = rd(g, c.ebp)?;
            c.eax = rd(g, points::TAG_ENTRIES.base)?;
            c.edx = (c.edx & 0xffff) << 4;
            c.ecx = rd(g, at(at(c.edx, c.eax), 8))?;
            let t = g.u8(at(c.ecx, 0xbc))? & 4;
            logic(c, t.into(), 0x80);
            if t != 0 {
                c.ebx = byte(c.ebx, 1);
            }
        }
    }
    c.eax = at(c.esp, 0x38);
    call(C_1CB0A0, 0xb7f99)
}

/// The first block sizing: `+11c`, `8 * def[+94]` bytes.
fn b7f99(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edi = slot(c, g, 0x20)?;
    c.esi = slot(c, g, 0x14)?;
    c.edx = g.u16(at(c.edi, 0x94))?.into();
    c.edx = shl(c, c.edx, 3, 16);
    push(c, g, 0)?;
    c.eax = c.esi;
    push(c, g, c.edx)?;
    push(c, g, 0x11c)?;
    call(C_BC380,0xb7fbd)
}

/// After each block sizing: a failure goes to `b80b8`, else the next.
fn blocks(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<Next> {
    if !test_al(c) {
        return b80b8(c, g);
    }
    let (align, size, field, site) = match pc {
        0xb7fbd => {
            c.eax = slot(c, g, 0x30)?;
            push(c, g, 0)?;
            c.ecx = c.eax.wrapping_mul(8);
            (None, c.ecx, 0x120, 0xb7fdf)
        }
        0xb7fdf => {
            let dx = g.u16(at(c.edi, 0xac))?;
            let p = i32::from(dx as i16) * 0x18;
            imul_flags(c, p != i32::from(p as i16));
            c.edx = p as u32 & 0xffff;
            push(c, g, 0)?;
            c.eax = c.esi;
            (None, c.edx, 0x124, 0xb8003)
        }
        0xb8003 => {
            c.edi = slot(c, g, 0x2c)?;
            let p = i64::from(c.edi as i32) * 0x34;
            c.eax = p as u32;
            imul_flags(c, p != i64::from(p as i32));
            push(c, g, 0)?;
            (None, c.eax, 0x114, 0xb8023)
        }
        0xb8023 => {
            c.eax = slot(c, g, 0x28)?;
            c.ecx = c.eax.wrapping_mul(5);
            push(c, g, 0)?;
            c.ecx = shl(c, c.ecx, 1, 32);
            (None, c.ecx, 0x118, 0xb8043)
        }
        0xb8043 | 0xb8063 => {
            logic(c, c.ebx & 0xff, 0x80);
            if c.ebx & 0xff != 0 {
                if pc == 0xb8043 {
                    c.eax = shl(c, c.edi, 5, 32);
                } else {
                    c.edi = shl(c, c.edi, 5, 32);
                    c.eax = c.edi;
                }
            } else {
                c.eax = 0;
                logic(c, 0, SIGN);
            }
            let (field, site) = if pc == 0xb8043 { (0x110, 0xb8063) } else { (0x10c, 0xb8083) };
            (Some(4), c.eax, field, site)
        }
        0xb8083 => {
            let dl = g.u8(at(c.esp, 0x13))?;
            c.edx = byte(c.edx, dl.wrapping_neg());
            push(c, g, 0)?;
            c.eax = c.esi;
            c.edx = if dl != 0 { NONE } else { 0 };
            c.edx &= 0x90;
            logic(c, c.edx, SIGN);
            (None, c.edx, 0x128, 0xb80a4)
        }
        0xb80a4 => {
            c.ecx = rd(g, c.ebp)?;
            return call(C_1C3930, 0xb80b0);
        }
        _ => return Err(unowned(pc)),
    };
    if let Some(a) = align {
        push(c, g, a)?;
    }
    push(c, g, size)?;
    push(c, g, field)?;
    c.eax = c.esi;
    call(C_BC380,site)
}

fn b80b8(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = byte(c.eax, 0);
    logic(c, 0, 0x80);
    g.set_u8(at(c.esp, 0x1b), 1)?;
    b80bf(c, g)
}

fn b80bf(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let ok = test_al(c);
    c.ecx = rd(g, points::OBJECTS.base)?;
    c.edx = rd(g, at(c.ecx, DATA))?;
    c.ecx = slot(c, g, 0x24)?;
    c.ebx = rd(g, at(at(c.edx, c.ecx), 8))?;
    if !ok {
        // `b8387`.
        push(c, g, c.esi)?;
        return call(C_BC300, 0xb838d);
    }
    let al = g.u8(at(c.esp, 0x13))?;
    c.eax = byte(c.eax, al);
    logic(c, al.into(), 0x80);
    if al == 0 {
        g.set_u8(at(c.ebx, 0xb3), 0)?;
        return b8127(c, g);
    }
    c.eax = sx16(g.u16(at(c.ebx, 0x12a))?);
    c.eax = arithmetic(c, c.eax, c.ebx, false);
    call(C_1CAFC0, 0xb80ee)
}

fn b80ee(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = slot(c, g, 0x1c)?;
    c.edx = slot(c, g, 0x20)?;
    c.edx = rd(g, at(c.edx, 0x38))?;
    c.edi = rd(g, at(c.ecx, 0x14))?;
    push(c, g, 1)?;
    c.esi = c.eax;
    call(C_1CB0D0, 0xb8105)
}

fn b8105(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let on = c.eax & 0xff != 0;
    let v = rd(g, at(c.ebx, 4))?;
    c.eax = set_bits(c, v, 0x800, on);
    g.set_u32(at(c.ebx, 4), c.eax)?;
    b8127(c, g)
}

/// Fill the node block with none, then the type new dispatch.
fn b8127(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = slot(c, g, 0x20)?;
    c.eax = rd(g, at(c.edx, 0x94))?;
    logic(c, c.eax, SIGN);
    if c.eax as i32 > 0 {
        c.ecx = sx16(g.u16(at(c.ebx, 0x11c))?);
        c.edi = sx16(g.u16(at(c.ebx, 0x11e))?);
        c.ecx = (c.ecx >> 3) << 3;
        c.edx = c.ecx;
        c.ecx >>= 2;
        c.edi = at(c.edi, c.ebx);
        c.eax = NONE;
        while c.ecx != 0 {
            g.set_u32(c.edi, c.eax)?;
            c.edi = at(c.edi, 4);
            c.ecx -= 1;
        }
        c.ecx = c.edx & 3;
        logic(c, c.ecx, SIGN);
        while c.ecx != 0 {
            g.set_u8(c.edi, c.eax as u8)?;
            c.edi = at(c.edi, 1);
            c.ecx -= 1;
        }
    }
    c.ebx = slot(c, g, 0x14)?;
    c.eax = at(c.esp, 0x1b);
    push(c, g, c.eax)?;
    push(c, g, c.ebp)?;
    call(points::OBJECT_TYPE_NEW.base, 0xb816b)
}

fn b816b(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.ecx = rd(g, points::OBJECTS.base)?;
    c.edx = rd(g, at(c.ecx, DATA))?;
    c.esi = slot(c, g, 0x24)?;
    c.ecx = rd(g, at(at(c.edx, c.esi), 8))?;
    let ok = c.eax & 0xff != 0;
    c.eax = byte(c.eax, u8::from(ok));
    logic(c, u32::from(ok), 0x80);
    set_slot(c, g, 0x1c, c.ecx)?;
    if !ok {
        return call(C_108BF0, 0xb8383);
    }
    c.eax = rd(g, at(c.ecx, 4))? >> 17;
    let al = c.eax as u8 & 1;
    c.eax = byte(c.eax, al);
    logic(c, al.into(), 0x80);
    g.set_u8(at(c.esp, 0x13), al)?;
    c.eax = rd(g, at(c.ebp, 0x18))?;
    logic(c, c.eax & 6, 0x80);
    if c.eax & 6 != 0 {
        let v = rd(g, at(c.ecx, 4))? & 0xfffd_ffff;
        g.set_u32(at(c.ecx, 4), v)?;
        logic(c, v, SIGN);
    }
    c.ecx = rd(g, at(c.ebp, 0xc))?;
    c.eax = c.ebx;
    call(C_BA540, 0xb81b1)
}

/// The model's bounding radii into body `+e4..+f0` (`d5b60`'s record,
/// else zero), then the change colours.
fn b81df(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    logic(c, c.eax, SIGN);
    sse::zero(c, 0);
    sse::movaps(c, 1, 0);
    sse::movaps(c, 2, 0);
    if c.eax != 0 {
        sse::load(c, 1, rd(g, at(c.eax, 0x28))?);
        sse::load(c, 2, rd(g, at(c.eax, 0x8c))?);
    }
    sse::load(c, 3, rd(g, ONE)?);
    g.set_u32(at(c.edi, 0xe4), sse::lane0(c, 1))?;
    sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
    let above = !below_or_equal(c);
    g.set_u32(at(c.edi, 0xe8), sse::lane0(c, 2))?;
    sse::movaps(c, 1, 3);
    if !above {
        sse::movaps(c, 1, 0);
    }
    sse::comiss(c, sse::lane0(c, 2), sse::lane0(c, 0));
    g.set_u32(at(c.edi, 0xec), sse::lane0(c, 1))?;
    if !below_or_equal(c) {
        sse::movaps(c, 0, 3);
    }
    push(c, g, c.ebx)?;
    g.set_u32(at(c.edi, 0xf0), sse::lane0(c, 0))?;
    call(points::OBJECT_CHANGE_COLORS.base, 0xb823a)
}

fn b823a(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edx = rd(g, at(c.ebp, 0xa8))?;
    c.edi = slot(c, g, 0x1c)?;
    c.eax = rd(g, points::OBJECTS.base)?;
    g.set_u32(at(c.edi, 0x24), c.edx)?;
    c.ecx = rd(g, at(c.eax, DATA))?;
    c.edx = rd(g, at(at(c.ecx, c.esi), 8))?;
    let w = g.u16(at(c.edx, 0x12a))?;
    cmp16(c, w.into(), 0xffff);
    if w != 0xffff {
        c.eax = c.ebx;
        return call(C_BA3D0, 0xb8264);
    }
    b8264(c, g)
}

fn b8264(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, c.ebx)?;
    call(C_BD020, 0xb826a)
}

/// With the object globals active, the placement's cached `+b8`/`+bc`
/// record (filled by `bf510` when unset) to `b8600`.
fn b826a(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = rd(g, OBJECT_GLOBALS)?;
    logic(c, c.eax, SIGN);
    if c.eax == 0 {
        return b82ad(c);
    }
    let b = g.u8(c.eax)?;
    cmp8_zero(c, b);
    if b == 0 {
        return b82ad(c);
    }
    let al = g.u8(at(c.ebp, 0xb8))?;
    c.eax = byte(c.eax, al);
    logic(c, al.into(), 0x80);
    if al != 0 {
        c.eax = at(c.ebp, 0xbc);
        return b82a6(c, g);
    }
    c.edi = at(c.ebp, 0xbc);
    push(c, g, c.ebx)?;
    call(C_BF510, 0xb828e)
}

fn b82a6(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, c.eax)?;
    push(c, g, c.ebx)?;
    call(C_B8600, 0xb82ad)
}

fn b82ad(c: &mut CpuAbi) -> Result<Next> {
    c.eax = c.ebx;
    call(points::OBJECT_NODES_NOTIFY.base, 0xb82b4)
}

fn b830b(c: &mut CpuAbi) -> Result<Next> {
    c.eax = c.esi;
    call(C_2E1E0, 0xb8312)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const E: u32 = 0x0070_0f00;
    const RET: u32 = 0x1234;
    const PLACEMENT: u32 = 0x0060_0000;
    const TAGS: u32 = 0x0061_0000;
    const DEF: u32 = 0x0062_0000;
    const DESC: u32 = 0x0063_0000;

    fn guest(tag: u32) -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0040_0000, 0x0040_0000, Permissions::RW).unwrap();
        g.set_u32(E, RET).unwrap();
        g.set_u32(E + 4, PLACEMENT).unwrap();
        g.set_u32(PLACEMENT, tag).unwrap();
        let c = CpuAbi { esp: E, ebx: 0xb0b0, ebp: 0xeb0, esi: 0x5151, edi: 0xd1d1, ..Default::default() };
        (c, g)
    }

    fn step(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Next {
        object_new(&mut Cx { cpu: c, g }, pc).unwrap()
    }

    #[test]
    fn a_placement_with_no_definition_returns_none() {
        let (mut c, mut g) = guest(NONE);
        assert_eq!(step(&mut c, &mut g, ENTRY), Next::Ret { pop: 4 });
        assert_eq!((c.eax, c.esp), (NONE, E));
        assert_eq!((c.ebx, c.ebp, c.esi, c.edi), (0xb0b0, 0xeb0, 0x5151, 0xd1d1));
    }

    #[test]
    fn a_valid_placement_fixes_up_then_allocates_and_a_failed_allocation_returns_none() {
        let (mut c, mut g) = guest(0x0001_0005);
        // The fixup first, with the placement in `ebx` and the flags of
        // the definition's compare with none.
        assert_eq!(step(&mut c, &mut g, ENTRY), Next::Call { target: C_108A40, site: 0xb7b73 });
        assert_eq!((c.ebx, c.ebp, c.esp), (PLACEMENT, PLACEMENT, E - 0xc0));
        assert_eq!(g.u32(c.esp + 0xc).unwrap(), NONE);
        assert_eq!(c.eflags & 0x40, 0);

        // Type 2 is not in `1883`: straight to the allocation of the
        // descriptor's datum size.
        g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
        g.set_u32(TAGS + 5 * 16 + 8, DEF).unwrap();
        g.set_u16(DEF, 2).unwrap();
        g.set_u32(DEF + 0x38, NONE).unwrap();
        g.set_u32(TYPES + 2 * 4, DESC).unwrap();
        g.set_u16(DESC + 8, 0x1f0).unwrap();
        assert_eq!(step(&mut c, &mut g, 0xb7b73), Next::Call { target: C_BC280, site: 0xb7be0 });
        assert_eq!(c.esp, E - 0xc8);
        assert_eq!(g.u32(c.esp).unwrap(), 0x1f0);
        assert_eq!(g.u32(c.esp + 4).unwrap(), 0x5151);
        // `F` is `esp + 10` here: the definition at `F+10`, no model at `F+c`.
        assert_eq!(g.u32(c.esp + 0x20).unwrap(), DEF);
        assert_eq!(g.u32(c.esp + 0x1c).unwrap(), 0);
        assert_eq!(g.u32(c.esp + 0x14).unwrap(), NONE);
        assert_eq!((c.esi, c.ecx), (DESC, 2));

        // `bc280` pops its argument and fails.
        c.esp += 4;
        c.eax = NONE;
        assert_eq!(step(&mut c, &mut g, 0xb7be0), Next::Ret { pop: 4 });
        assert_eq!((c.eax, c.esp), (NONE, E));
        assert_eq!((c.ebx, c.ebp, c.esi), (0xb0b0, 0xeb0, 0x5151));
    }
}
