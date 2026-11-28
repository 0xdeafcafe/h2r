//! The unit update's shell and movement (slice 6a of
//! docs/engine/sim-rewrite-objects.md; contracts in
//! docs/engine/object-update.md, "Slice 6a bodies"): `c6b30` (the `unit`
//! descriptor's slot `+40`), the movement `c60c0`, the direction clamp
//! `cba50`, the idle test `c85c0`, and the aim update `c7840` taken from
//! its port (`sim_physics::unit_aim`).
//!
//! Every body is a segment over the retail frame, the retail code block by
//! block, keyed by retail address: at each call and return the eight
//! integer registers, the `xmm` registers, MXCSR and the x87 state are
//! retail's. Arithmetic flags are left undefined at every transfer of
//! `c60c0`, `cba50` and `c85c0`: no caller and no callee reads them
//! (`scripts/sim-liveness.py`, docs/engine/object-update.md, "Register
//! liveness"). `c6b30` and `c7840` reproduce them, as their ports do.
//!
//! **`c6b30 (unit: stack 0) -> al`; `ret 4`.** `ebx` and `esi` pushed;
//! `esi` holds the unit across the nine calls, `bl` the result. In order
//! `c49b0`, `c58f0` and `c60c0` (stack argument; their `al` starts and is
//! ORed into `bl`), then `c5eb0` and `c6740` (`eax` the unit; ORed), then
//! `114c60`, `c6990`, `c6810`, `c50a0` (`eax` the unit; for their effects).
//! `c60c0` is a body here, and `c49b0`, `c58f0` and `c5eb0` are slice 6b's
//! (`unit_state`); the rest are callouts until slice 6c.
//!
//! **`c60c0 (unit: stack 0) -> al`; `ret 4`.** Frame `E` = `esp` after
//! `sub esp, 7c`; `ebp`, `esi`, then `ebx`, then `edi` pushed (so at every
//! call site `esp` is `E - 10`, at `c6724` `E - c`). `[E+3]` the result
//! byte (0), `[E+4]`, `[E+8]` the aim rates (then the aim fractions
//! `c645a` and `c646f` scale to bytes), `[E+c]` the rate scale (then the
//! look yaw rate), `[E+10]` the unit's limits record (body `+ [body+33e]`),
//! `[E+14]` the definition (then the look pitch rate, then a divisor),
//! `[E+18..20]` the aim before (body `+168`), `[E+3c..44]` the look before
//! (body `+18c`), `[E+48..78]` the frame and arguments of `c7840`, `[E+80]`
//! the unit. Across the calls `ebp` holds the body, `ebx` body `+168`
//! (then `[4687b0]`), `esi` the definition (`[E+14]`), `edi` `[4687a4]`.
//! It moves the aim (`+168`) toward the desired aim (`+15c`) and the look
//! (`+18c`) toward the desired look (`+180`): when the definition's two
//! rates for a pair (`+144`, `+148`; `+150`, `+154`, times `+14c` when body
//! `+1bc` is 1) are both zero the direction is copied and clamped
//! (`cba50`) and its velocity (`+174`, `+198`) set from `[4687a4]`;
//! otherwise `c7840` turns it in the unit's frame (`b9fc0`) when the
//! limits record's bit 5 (aim) or 6 (look) is set, or `11f0d0` turns it in
//! the world. The aim velocity's two fractions of the rate (`11ce20`,
//! `1201a0`, clamped to 0..1) become the bytes at `+243` and `+1f8`
//! through `_ftol2` (`322b0c`). If any of the six components moved by
//! more than `[45dbdc]` the unit is woken for the post walk (`bba20`,
//! fused) and `al` is 1.
//!
//! **`cba50 (unit: eax, direction: stack 0, look: stack 1) -> al`; `ret
//! 8`.** Frame `E` after `sub esp, 50`; `ebx` (the body), `ebp` (the
//! limits: record `+4`, or `+14` for the look), then `esi`, `edi` pushed.
//! When the record's bit (5, or 6 for the look) is clear it returns 0.
//! Otherwise it takes the unit's frame (`b9fc0` into `[E+4]`, `[E+10]`);
//! for the look it first re-bases the frame on the aim (body `+168`, two
//! cross products normalised by `30bf0`, with `11d000` when the first is
//! degenerate); builds the matrix (`1420f0` into `[E+1c]`), takes the
//! direction's yaw and pitch in it (`fpatan`, `fsqrt`), clamps them to
//! the record's four limits and, if either clamped, writes the direction
//! back from the clamped angles (`fsin`, `fcos`). `al` is whether it
//! clamped.
//!
//! **`c85c0 (unit: eax) -> al`; `ret`.** `ecx`, `ebx`, `esi`, `edi`
//! saved; the saved `ecx`'s top byte (`[esp+f]`) becomes the result 1, so
//! `ecx` returns with it. 0 when the unit has a parent (body `+14`), when
//! bit 26 or 27 of its record word (body `+ [body+346] + 4`) is set, or
//! when any of seven `e6fe0(unit, n)` tests answers; then 0 when either
//! held weapon (body `+212` indices into `+218`) answers `100880`
//! (`ebp` the index, `edi` the index byte, `ebx` none). Else 1.

use super::functions::{Ss, above, call, fp_error, fpu, fpu_done, lo, load, movaps, ret, ss, ucomiss_lahf_jp, zero};
use super::{Cx, Next, Result, unowned};
use crate::light_constants::X87;
use guest_layout::points;
use crate::sim_physics::unit_aim;
use crate::sim_physics::{ARITH, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};
use x87::abi::{arith_op, unary_op};

const NONE: u32 = u32::MAX;
/// 1.0, the clamp's upper bound and the default rate scale.
const ONE: u32 = 0x0045_dbc0;
/// The fraction to byte scale.
const BYTE_SCALE: u32 = 0x0045_dc38;
/// The smallest movement that wakes the unit.
const SMALL: u32 = 0x0045_dbdc;
/// The degenerate length `cba50` compares against (L: zero).
const THRESHOLD: u32 = 0x0045_dbd8;
/// Pointers to constant vectors (L: zero, and two axes).
const VECTOR_A4: u32 = 0x0046_87a4;
const VECTOR_88: u32 = 0x0046_8788;
const VECTOR_B0: u32 = 0x0046_87b0;

pub(super) mod callee {
    /// The unit update's children.
    pub const C49B0: u32 = 0xc49b0;
    pub const C58F0: u32 = 0xc58f0;
    pub const C5EB0: u32 = 0xc5eb0;
    pub const C6740: u32 = 0xc6740;
    pub const DIALOGUE: u32 = 0x114c60;
    pub const C6990: u32 = 0xc6990;
    pub const C6810: u32 = 0xc6810;
    pub const C50A0: u32 = 0xc50a0;
    /// `(unit: ecx) -> al`: whether the clamp runs.
    pub const C4970: u32 = 0xc4970;
    /// `(unit: eax, out: esi, edi)`: the unit's frame.
    pub const FRAME: u32 = 0xb9fc0;
    /// Turn a direction in the world (three stack arguments).
    pub const TURN: u32 = 0x11f0d0;
    /// `(ecx, edx) -> xmm0`.
    pub const ANGLE: u32 = 0x11ce20;
    /// `(eax, ecx) -> eax`.
    pub const PROJECT: u32 = 0x1201a0;
    /// `_ftol2`: `st(0)` popped into `edx:eax`, toward zero.
    pub const FTOL: u32 = 0x322b0c;
    /// Normalise `[eax]` in place; the length in `xmm0`.
    pub const NORMALIZE: u32 = 0x30bf0;
    pub const PERPENDICULAR: u32 = 0x11d000;
    /// `(eax, ecx, edx, esi)`: a matrix from a frame.
    pub const MATRIX: u32 = 0x1420f0;
    /// `(unit: eax, n: ecx) -> al`: one unit state test.
    pub const STATE: u32 = 0xe6fe0;
    /// `(weapon: eax, ebx) -> al`.
    pub const WEAPON: u32 = 0x100880;
}

// ---------------------------------------------------------------- c6b30

/// `c6b30`'s return sites, in call order.
pub(super) const SHELL_SITES: [u32; 9] =
    [0xc6b3c, 0xc6b44, 0xc6b4c, 0xc6b55, 0xc6b5e, 0xc6b67, 0xc6b6e, 0xc6b75, 0xc6b7c];

/// `c6b30 (unit: stack 0) -> al` (the port `sim_physics::objects::unit`,
/// an oracle now, over [`Next`]).
pub(super) fn shell(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::UNIT_UPDATE.base {
        push(c, g, c.ebx)?;
        push(c, g, c.esi)?;
        c.esi = g.u32(c.esp.wrapping_add(12))?;
        push(c, g, c.esi)?;
        return Ok(Next::Call { target: callee::C49B0, site: SHELL_SITES[0] });
    }
    let stage = SHELL_SITES.iter().position(|&site| site == pc).ok_or_else(|| unowned(pc))?;
    match stage {
        0 => c.ebx = byte(c.ebx, c.eax as u8),
        1..=4 => {
            c.ebx = byte(c.ebx, (c.ebx | c.eax) as u8);
            logic(c, c.ebx & 0xff, 0x80);
        }
        _ => {}
    }
    let next = |target| Ok(Next::Call { target, site: SHELL_SITES[stage + 1] });
    match stage {
        0 => {
            push(c, g, c.esi)?;
            next(callee::C58F0)
        }
        1 => {
            push(c, g, c.esi)?;
            next(points::UNIT_MOVEMENT.base)
        }
        8 => {
            c.esi = pop(c, g)?;
            c.eax = byte(c.eax, c.ebx as u8);
            c.ebx = pop(c, g)?;
            Ok(Next::Ret { pop: 4 })
        }
        _ => {
            c.eax = c.esi;
            next(match stage {
                2 => callee::C5EB0,
                3 => callee::C6740,
                4 => callee::DIALOGUE,
                5 => callee::C6990,
                6 => callee::C6810,
                _ => callee::C50A0,
            })
        }
    }
}

// ---------------------------------------------------------------- shared

fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

fn sx8(v: u8) -> u32 {
    v as i8 as i32 as u32
}

/// `movss [addr], xmm[r]`.
fn st(c: &CpuAbi, g: &mut Guest, addr: u32, r: usize) -> Result<()> {
    g.set_u32(addr, lo(c, r))
}

/// `op xmm[d], xmm[s]`.
fn ssr(c: &mut CpuAbi, op: Ss, d: usize, s: usize) -> Result<u32> {
    let b = lo(c, s);
    ss(c, op, d, b)
}

/// Copy three dwords, as `mov r, [src]; mov [dst], r` triples do.
fn copy3(g: &mut Guest, dst: u32, src: u32) -> Result<[u32; 3]> {
    let v = [g.u32(src)?, g.u32(src.wrapping_add(4))?, g.u32(src.wrapping_add(8))?];
    for (i, w) in v.iter().enumerate() {
        g.set_u32(dst.wrapping_add(4 * i as u32), *w)?;
    }
    Ok(v)
}

/// `fcomip st, st(1)`: EFLAGS from the x87 compare; ZF, PF and CF.
fn fcomip(c: &mut CpuAbi, x: &mut X87) -> Result<u32> {
    let f = x.fcomip(1).map_err(fp_error)?;
    c.eflags = (c.eflags & !ARITH) | f;
    c.eflags_defined |= ARITH;
    Ok(f)
}

/// `jbe` after a compare.
fn jbe(f: u32) -> bool {
    f & 0x41 != 0
}

fn x_ok<T>(r: std::result::Result<T, crate::light_constants::Error>) -> Result<T> {
    r.map_err(fp_error)
}

/// Whether `pc` is `entry` or one of `sites`.
fn owned(pc: u32, entry: u32, sites: &[u32]) -> Result<()> {
    if pc == entry || sites.contains(&pc) { Ok(()) } else { Err(unowned(pc)) }
}

// ---------------------------------------------------------------- c60c0

/// `c60c0`'s return sites, in address order.
pub(super) const MOVEMENT_SITES: [u32; 16] = [
    0xc61eb, 0xc61fe, 0xc624f, 0xc630e, 0xc633a, 0xc638f, 0xc6407, 0xc6412, 0xc641d, 0xc645f, 0xc6474, 0xc64e8,
    0xc6537, 0xc6659, 0xc6685, 0xc6724,
];

/// The six "moved" tests from `c6685`: the addresses of each test's `fld
/// [45dbdc]` and `fstp st(0)` (for the x87 pointers), the saved
/// component's offset from `E` and the body field it is compared with.
const MOVED: [(u32, u32, u32, u32); 6] = [
    (0xc6692, 0xc669a, 0x18, 0x168),
    (0xc66aa, 0xc66b2, 0x1c, 0x16c),
    (0xc66c2, 0xc66ca, 0x20, 0x170),
    (0xc66da, 0xc66e2, 0x3c, 0x18c),
    (0xc66f2, 0xc66fa, 0x40, 0x190),
    (0xc670a, 0xc6712, 0x44, 0x194),
];

/// `c60c0 (unit: stack 0) -> al`.
pub(super) fn movement(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::UNIT_MOVEMENT.base;
    owned(pc, entry, &MOVEMENT_SITES)?;
    let time = points::GAME_TIME.base;
    // `E` from the `esp` at a site.
    let e = |c: &CpuAbi| c.esp.wrapping_add(0x10);
    let mut at = pc;
    loop {
        at = match at {
            0xc60c0 => {
                c.esp = c.esp.wrapping_sub(0x7c);
                let base = c.esp;
                c.ecx = g.u32(base.wrapping_add(0x80))?;
                c.edx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(c.edx.wrapping_add(0x44))?;
                c.eax = (c.ecx & 0xffff).wrapping_mul(3);
                push(c, g, c.ebp)?;
                c.ebp = g.u32(c.edx.wrapping_add(c.eax.wrapping_mul(4)).wrapping_add(8))?;
                c.eax = g.u32(c.ebp)?;
                c.edx = g.u32(points::TAG_ENTRIES.base)?;
                c.eax = (c.eax & 0xffff) << 4;
                push(c, g, c.esi)?;
                c.esi = g.u32(c.eax.wrapping_add(c.edx).wrapping_add(8))?;
                c.edx = g.u32(c.esi.wrapping_add(0xbc))?;
                c.eax &= !0xff;
                c.edx >>= 10;
                g.set_u32(base.wrapping_add(0x14), c.esi)?;
                g.set_u8(base.wrapping_add(3), c.eax as u8)?;
                if c.edx & 1 != 0 {
                    0xc6734
                } else {
                    let dl = g.u8(c.ebp.wrapping_add(0x10a))? >> 2;
                    c.edx = byte(c.edx, dl);
                    if dl & 1 != 0 { 0xc6734 } else { 0xc6126 }
                }
            }
            0xc6126 => {
                let base = c.esp.wrapping_add(8);
                push(c, g, c.ebx)?;
                c.ebx = c.ebp.wrapping_add(0x168);
                c.eax = c.ebx;
                c.edx = g.u32(c.eax)?;
                g.set_u32(base.wrapping_add(0x18), c.edx)?;
                c.edx = g.u32(c.eax.wrapping_add(4))?;
                c.eax = g.u32(c.eax.wrapping_add(8))?;
                g.set_u32(base.wrapping_add(0x20), c.eax)?;
                c.eax = c.ebp.wrapping_add(0x18c);
                g.set_u32(base.wrapping_add(0x1c), c.edx)?;
                c.edx = g.u32(c.eax)?;
                g.set_u32(base.wrapping_add(0x3c), c.edx)?;
                c.edx = g.u32(c.eax.wrapping_add(4))?;
                c.eax = g.u32(c.eax.wrapping_add(8))?;
                g.set_u32(base.wrapping_add(0x44), c.eax)?;
                let one = g.u8(c.ebp.wrapping_add(0x1bc))? == 1;
                g.set_u32(base.wrapping_add(0x40), c.edx)?;
                if one {
                    load(c, 2, g.u32(c.esi.wrapping_add(0x14c))?);
                    st(c, g, base.wrapping_add(0xc), 2)?;
                } else {
                    load(c, 0, g.u32(ONE)?);
                    st(c, g, base.wrapping_add(0xc), 0)?;
                    movaps(c, 2, 0);
                }
                // c6187
                load(c, 0, g.u32(c.esi.wrapping_add(0x144))?);
                load(c, 1, g.u32(c.esi.wrapping_add(0x148))?);
                push(c, g, c.edi)?;
                c.edi = sx16(g.u16(c.ebp.wrapping_add(0x33e))?);
                ssr(c, Ss::Mul, 0, 2)?;
                c.edi = c.edi.wrapping_add(c.ebp);
                ssr(c, Ss::Mul, 1, 2)?;
                zero(c, 2);
                let moving = ucomiss_lahf_jp(c, lo(c, 0), lo(c, 2))?;
                g.set_u32(base.wrapping_add(0x10), c.edi)?;
                st(c, g, base.wrapping_add(4), 0)?;
                st(c, g, base.wrapping_add(8), 1)?;
                if moving || ucomiss_lahf_jp(c, lo(c, 1), lo(c, 2))? {
                    0xc6224
                } else {
                    // c61ce: the desired aim copied.
                    c.edx = c.ebp.wrapping_add(0x15c);
                    c.eax = c.ebx;
                    let v = copy3(g, c.eax, c.edx)?;
                    c.edi = v[1];
                    c.edx = v[2];
                    return call(c, callee::C4970, 0xc61eb);
                }
            }
            0xc61eb => {
                if c.eax as u8 == 0 {
                    0xc6201
                } else {
                    let base = e(c);
                    c.eax = g.u32(base.wrapping_add(0x80))?;
                    push(c, g, 0)?;
                    push(c, g, c.ebx)?;
                    return call(c, points::UNIT_CLAMP_DIRECTION.base, 0xc61fe);
                }
            }
            0xc61fe => {
                zero(c, 2);
                0xc6201
            }
            0xc6201 => {
                c.edi = g.u32(VECTOR_A4)?;
                c.ecx = c.edi;
                c.eax = c.ebp.wrapping_add(0x174);
                let v = copy3(g, c.eax, c.ecx)?;
                c.edx = v[1];
                c.ecx = v[2];
                0xc6347
            }
            0xc6224 => {
                let dl = g.u8(c.edi)? >> 5;
                c.edx = byte(c.edx, dl);
                if dl & 1 == 0 {
                    0xc6310
                } else {
                    let base = e(c);
                    load(c, 0, g.u32(ONE)?);
                    c.edi = base.wrapping_add(0x64);
                    c.esi = base.wrapping_add(0x4c);
                    c.eax = c.ecx;
                    st(c, g, base.wrapping_add(0x48), 0)?;
                    return call(c, callee::FRAME, 0xc624f);
                }
            }
            0xc624f => {
                let base = e(c);
                let s = |o: u32| base.wrapping_add(o);
                c.eax = g.u32(VECTOR_88)?;
                c.ecx = g.u32(c.eax)?;
                c.edx = g.u32(c.eax.wrapping_add(4))?;
                c.eax = g.u32(c.eax.wrapping_add(8))?;
                load(c, 2, g.u32(s(0x54))?);
                load(c, 0, g.u32(s(0x6c))?);
                load(c, 3, g.u32(s(0x68))?);
                load(c, 1, g.u32(s(0x50))?);
                g.set_u32(s(0x70), c.ecx)?;
                c.ecx = g.u32(s(8))?;
                g.set_u32(s(0x78), c.eax)?;
                c.eax = g.u32(s(0x10))?;
                push(c, g, c.ecx)?;
                g.set_u32(s(0x74), c.edx)?;
                c.edx = g.u32(s(4))?;
                push(c, g, c.edx)?;
                c.edx = g.u32(time)?;
                c.eax = c.eax.wrapping_add(4);
                movaps(c, 5, 0);
                ssr(c, Ss::Mul, 5, 1)?;
                push(c, g, c.eax)?;
                c.eax = g.u32(c.edx.wrapping_add(4))?;
                movaps(c, 4, 2);
                ssr(c, Ss::Mul, 4, 3)?;
                ssr(c, Ss::Sub, 4, 5)?;
                load(c, 5, g.u32(s(0x64))?);
                c.ecx = c.ebp.wrapping_add(0x174);
                st(c, g, s(0x58), 4)?;
                load(c, 4, g.u32(s(0x4c))?);
                push(c, g, c.ecx)?;
                ssr(c, Ss::Mul, 0, 4)?;
                ssr(c, Ss::Mul, 2, 5)?;
                ssr(c, Ss::Mul, 1, 5)?;
                ssr(c, Ss::Mul, 3, 4)?;
                push(c, g, c.eax)?;
                ssr(c, Ss::Sub, 0, 2)?;
                ssr(c, Ss::Sub, 1, 3)?;
                c.eax = c.ebp.wrapping_add(0x15c);
                c.edi = c.ebx;
                c.esi = s(0x48);
                st(c, g, s(0x5c), 0)?;
                st(c, g, s(0x60), 1)?;
                return call(c, points::UNIT_AIM_UPDATE.base, 0xc630e);
            }
            0xc6310 => {
                let base = e(c);
                c.ecx = g.u32(base.wrapping_add(8))?;
                c.edx = g.u32(base.wrapping_add(4))?;
                c.eax = g.u32(time)?;
                push(c, g, c.ecx)?;
                c.ecx = g.u32(c.eax.wrapping_add(4))?;
                push(c, g, c.edx)?;
                c.ebx = c.ebp.wrapping_add(0x174);
                push(c, g, c.ecx)?;
                c.edi = c.ebp.wrapping_add(0x15c);
                c.esi = c.ebp.wrapping_add(0x168);
                return call(c, callee::TURN, 0xc633a);
            }
            // c630e jumps to c633a.
            0xc630e | 0xc633a => {
                zero(c, 2);
                c.edi = g.u32(VECTOR_A4)?;
                c.esi = g.u32(e(c).wrapping_add(0x14))?;
                0xc6347
            }
            0xc6347 => {
                let base = e(c);
                let rate = g.u32(c.esi.wrapping_add(0x144))?;
                if !ucomiss_lahf_jp(c, lo(c, 2), rate)? {
                    st(c, g, base.wrapping_add(4), 2)?;
                    st(c, g, base.wrapping_add(8), 2)?;
                    0xc6450
                } else {
                    c.edx = g.u32(time)?;
                    load(c, 0, g.u32(c.edx.wrapping_add(4))?);
                    ss(c, Ss::Mul, 0, rate)?;
                    c.ebx = c.ebp.wrapping_add(0x168);
                    c.ecx = base.wrapping_add(0x18);
                    c.edx = c.ebx;
                    st(c, g, base.wrapping_add(0x14), 0)?;
                    return call(c, callee::ANGLE, 0xc638f);
                }
            }
            0xc638f => {
                let base = e(c);
                ss(c, Ss::Div, 0, g.u32(base.wrapping_add(0x14))?)?;
                zero(c, 1);
                let below = above(c, lo(c, 1), lo(c, 0))?;
                st(c, g, base.wrapping_add(4), 0)?;
                if below {
                    st(c, g, base.wrapping_add(4), 1)?;
                } else {
                    load(c, 2, g.u32(ONE)?);
                    if above(c, lo(c, 0), lo(c, 2))? {
                        st(c, g, base.wrapping_add(4), 2)?;
                    }
                }
                // c63be
                let s = |o: u32| base.wrapping_add(o);
                c.eax = g.u32(s(0x18))?;
                c.ecx = g.u32(s(0x1c))?;
                c.edx = g.u32(s(0x20))?;
                g.set_u32(s(0x30), c.eax)?;
                c.eax = g.u32(c.ebx)?;
                g.set_u32(s(0x34), c.ecx)?;
                c.ecx = g.u32(c.ebx.wrapping_add(4))?;
                g.set_u32(s(0x38), c.edx)?;
                c.edx = g.u32(c.ebx.wrapping_add(8))?;
                c.ebx = g.u32(VECTOR_B0)?;
                g.set_u32(s(0x28), c.ecx)?;
                g.set_u32(s(0x24), c.eax)?;
                g.set_u32(s(0x2c), c.edx)?;
                c.ecx = c.ebx;
                c.eax = s(0x30);
                st(c, g, s(0x38), 1)?;
                st(c, g, s(0x2c), 1)?;
                return call(c, callee::PROJECT, 0xc6407);
            }
            0xc6407 => {
                c.ecx = c.ebx;
                c.eax = e(c).wrapping_add(0x24);
                return call(c, callee::PROJECT, 0xc6412);
            }
            0xc6412 => {
                c.ecx = c.eax;
                c.edx = e(c).wrapping_add(0x30);
                return call(c, callee::ANGLE, 0xc641d);
            }
            0xc641d => {
                let base = e(c);
                ss(c, Ss::Div, 0, g.u32(base.wrapping_add(0x14))?)?;
                zero(c, 1);
                let below = above(c, lo(c, 1), lo(c, 0))?;
                st(c, g, base.wrapping_add(8), 0)?;
                if below {
                    st(c, g, base.wrapping_add(8), 1)?;
                } else if above(c, lo(c, 0), g.u32(ONE)?)? {
                    load(c, 0, g.u32(ONE)?);
                    st(c, g, base.wrapping_add(8), 0)?;
                }
                0xc6450
            }
            // `fld [E+4]; fmul [45dc38]; call _ftol2`.
            0xc6450 => {
                let src = e(c).wrapping_add(4);
                let mut x = fpu(c);
                x_ok(x.fld(g.u32(src)?))?;
                x_ok(x.fmul_m(g.u32(BYTE_SCALE)?))?;
                fpu_done(c, g, x, 0xc6454, BYTE_SCALE)?;
                return call(c, callee::FTOL, 0xc645f);
            }
            0xc645f => {
                let src = e(c).wrapping_add(8);
                let mut x = fpu(c);
                x_ok(x.fld(g.u32(src)?))?;
                x_ok(x.fmul_m(g.u32(BYTE_SCALE)?))?;
                fpu_done(c, g, x, 0xc6463, BYTE_SCALE)?;
                g.set_u8(c.ebp.wrapping_add(0x243), c.eax as u8)?;
                return call(c, callee::FTOL, 0xc6474);
            }
            0xc6474 => {
                let base = e(c);
                load(c, 2, g.u32(base.wrapping_add(0xc))?);
                g.set_u8(c.ebp.wrapping_add(0x1f8), c.eax as u8)?;
                load(c, 0, g.u32(c.esi.wrapping_add(0x150))?);
                load(c, 1, g.u32(c.esi.wrapping_add(0x154))?);
                ssr(c, Ss::Mul, 0, 2)?;
                ssr(c, Ss::Mul, 1, 2)?;
                zero(c, 2);
                let moving = ucomiss_lahf_jp(c, lo(c, 0), lo(c, 2))?;
                st(c, g, base.wrapping_add(0xc), 0)?;
                st(c, g, base.wrapping_add(0x14), 1)?;
                if moving || ucomiss_lahf_jp(c, lo(c, 1), lo(c, 2))? {
                    0xc6503
                } else {
                    // c64b9: the desired look copied, then clamped.
                    c.eax = c.ebp.wrapping_add(0x180);
                    c.ecx = c.eax;
                    c.eax = c.ebp.wrapping_add(0x18c);
                    c.edx = c.eax;
                    let v = copy3(g, c.edx, c.ecx)?;
                    c.esi = v[1];
                    c.ecx = v[2];
                    push(c, g, 1)?;
                    push(c, g, c.eax)?;
                    c.eax = g.u32(base.wrapping_add(0x80))?;
                    return call(c, points::UNIT_CLAMP_DIRECTION.base, 0xc64e8);
                }
            }
            0xc64e8 => {
                c.edx = c.ebp.wrapping_add(0x198);
                let v = copy3(g, c.edx, c.edi)?;
                c.ecx = v[1];
                c.eax = v[2];
                0xc6685
            }
            0xc6503 => {
                let base = e(c);
                c.ecx = g.u32(base.wrapping_add(0x10))?;
                let dl = g.u8(c.ecx)? >> 6;
                c.edx = byte(c.edx, dl);
                if dl & 1 == 0 {
                    0xc665b
                } else {
                    load(c, 0, g.u32(ONE)?);
                    c.eax = g.u32(base.wrapping_add(0x80))?;
                    c.edi = base.wrapping_add(0x64);
                    c.esi = base.wrapping_add(0x4c);
                    st(c, g, base.wrapping_add(0x48), 0)?;
                    return call(c, callee::FRAME, 0xc6537);
                }
            }
            0xc6537 => {
                let base = e(c);
                let s = |o: u32| base.wrapping_add(o);
                load(c, 4, g.u32(s(0x6c))?);
                load(c, 5, g.u32(s(0x68))?);
                load(c, 7, g.u32(s(0x64))?);
                c.eax = c.ebp.wrapping_add(0x168);
                c.ecx = g.u32(c.eax)?;
                c.edx = g.u32(c.eax.wrapping_add(4))?;
                c.eax = g.u32(c.eax.wrapping_add(8))?;
                g.set_u32(s(0x54), c.eax)?;
                c.eax = g.u32(VECTOR_88)?;
                load(c, 1, g.u32(s(0x54))?);
                g.set_u32(s(0x4c), c.ecx)?;
                c.ecx = g.u32(c.eax)?;
                load(c, 6, g.u32(s(0x4c))?);
                g.set_u32(s(0x50), c.edx)?;
                c.edx = g.u32(c.eax.wrapping_add(4))?;
                c.eax = g.u32(c.eax.wrapping_add(8))?;
                load(c, 2, g.u32(s(0x50))?);
                movaps(c, 0, 4);
                ssr(c, Ss::Mul, 0, 2)?;
                movaps(c, 3, 1);
                ssr(c, Ss::Mul, 3, 5)?;
                ssr(c, Ss::Sub, 3, 0)?;
                g.set_u32(s(0x70), c.ecx)?;
                c.ecx = g.u32(s(0x14))?;
                g.set_u32(s(0x78), c.eax)?;
                c.eax = g.u32(s(0x10))?;
                push(c, g, c.ecx)?;
                movaps(c, 0, 1);
                ssr(c, Ss::Mul, 0, 7)?;
                g.set_u32(s(0x74), c.edx)?;
                c.edx = g.u32(s(0xc))?;
                ssr(c, Ss::Mul, 4, 6)?;
                ssr(c, Ss::Sub, 4, 0)?;
                push(c, g, c.edx)?;
                c.edx = g.u32(time)?;
                c.eax = c.eax.wrapping_add(0x14);
                ssr(c, Ss::Mul, 5, 6)?;
                push(c, g, c.eax)?;
                c.eax = g.u32(c.edx.wrapping_add(4))?;
                movaps(c, 0, 2);
                ssr(c, Ss::Mul, 0, 7)?;
                ssr(c, Ss::Sub, 0, 5)?;
                movaps(c, 7, 4);
                c.ecx = c.ebp.wrapping_add(0x198);
                movaps(c, 5, 0);
                ssr(c, Ss::Mul, 7, 1)?;
                st(c, g, s(0x5c), 4)?;
                ssr(c, Ss::Mul, 5, 2)?;
                push(c, g, c.ecx)?;
                st(c, g, s(0x60), 0)?;
                ssr(c, Ss::Mul, 1, 3)?;
                ssr(c, Ss::Mul, 0, 6)?;
                ssr(c, Ss::Mul, 4, 6)?;
                ssr(c, Ss::Mul, 2, 3)?;
                push(c, g, c.eax)?;
                ssr(c, Ss::Sub, 5, 7)?;
                ssr(c, Ss::Sub, 1, 0)?;
                ssr(c, Ss::Sub, 4, 2)?;
                c.eax = c.ebp.wrapping_add(0x180);
                c.edi = c.ebp.wrapping_add(0x18c);
                c.esi = s(0x48);
                st(c, g, s(0x58), 3)?;
                st(c, g, s(0x64), 5)?;
                st(c, g, s(0x68), 1)?;
                st(c, g, s(0x6c), 4)?;
                return call(c, points::UNIT_AIM_UPDATE.base, 0xc6659);
            }
            0xc665b => {
                let base = e(c);
                c.ecx = g.u32(base.wrapping_add(0x14))?;
                c.edx = g.u32(base.wrapping_add(0xc))?;
                c.eax = g.u32(time)?;
                push(c, g, c.ecx)?;
                c.ecx = g.u32(c.eax.wrapping_add(4))?;
                push(c, g, c.edx)?;
                c.ebx = c.ebp.wrapping_add(0x198);
                push(c, g, c.ecx)?;
                c.edi = c.ebp.wrapping_add(0x180);
                c.esi = c.ebp.wrapping_add(0x18c);
                return call(c, callee::TURN, 0xc6685);
            }
            // c6659 jumps to c6685: the six "moved" tests.
            0xc6659 | 0xc6685 => {
                let base = e(c);
                let mut x = fpu(c);
                let mut moved = true;
                for (i, &(small_at, fstp_at, saved, field)) in MOVED.iter().enumerate() {
                    x_ok(x.fld(g.u32(base.wrapping_add(saved))?))?;
                    if i == 0 {
                        c.edi = pop(c, g)?;
                    }
                    let field = c.ebp.wrapping_add(field);
                    x_ok(x.arith_m(arith_op::SUB, g.u32(field)?))?;
                    x_ok(x.unary(unary_op::ABS))?;
                    x_ok(x.fld(g.u32(SMALL)?))?;
                    let flags = fcomip(c, &mut x)?;
                    x_ok(x.fstp_st(0))?;
                    if jbe(flags) || i == MOVED.len() - 1 {
                        moved = jbe(flags);
                        // The last memory operand is the `fld [45dbdc]`;
                        // the last instruction the `fstp st(0)`.
                        fpu_done(c, g, X87(x.0), small_at, SMALL)?;
                        fpu_done(c, g, x, fstp_at, 0)?;
                        break;
                    }
                }
                if moved { 0xc6716 } else { 0xc672f }
            }
            0xc6716 => {
                // `esp` is `E - c`: `edi` popped.
                c.eax = g.u32(c.esp.wrapping_add(0xc).wrapping_add(0x80))?;
                c.ebx = byte(c.ebx, 1);
                return call(c, points::OBJECT_WAKE_FOR_POST.base, 0xc6724);
            }
            0xc6724 => {
                c.eax = byte(c.eax, c.ebx as u8);
                c.ebx = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.esp = c.esp.wrapping_add(0x7c);
                return ret(c, 4);
            }
            0xc672f => {
                let al = g.u8(c.esp.wrapping_add(0xc).wrapping_add(3))?;
                c.eax = byte(c.eax, al);
                c.ebx = pop(c, g)?;
                0xc6734
            }
            0xc6734 => {
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.esp = c.esp.wrapping_add(0x7c);
                return ret(c, 4);
            }
            _ => return Err(unowned(at)),
        };
    }
}

// ---------------------------------------------------------------- cba50

/// `cba50`'s return sites.
pub(super) const CLAMP_SITES: [u32; 5] = [0xcbaad, 0xcbb67, 0xcbb7d, 0xcbb82, 0xcbb99];

/// `cba50 (unit: eax, direction: stack 0, look: stack 1) -> al`.
pub(super) fn clamp(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::UNIT_CLAMP_DIRECTION.base;
    owned(pc, entry, &CLAMP_SITES)?;
    // `E` from the `esp` at a site (four pushes).
    let e = |c: &CpuAbi| c.esp.wrapping_add(0x10);
    let mut at = pc;
    loop {
        at = match at {
            0xcba50 => {
                c.esp = c.esp.wrapping_sub(0x50);
                let base = c.esp;
                c.edx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(c.edx.wrapping_add(0x44))?;
                c.ecx = (c.eax & 0xffff).wrapping_mul(3);
                push(c, g, c.ebx)?;
                c.ebx = g.u32(c.edx.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(8))?;
                c.ecx = sx16(g.u16(c.ebx.wrapping_add(0x33e))?);
                let look = g.u8(base.wrapping_add(0x58))?;
                c.edx = byte(c.edx, look);
                c.ecx = c.ecx.wrapping_add(c.ebx);
                let bits = g.u8(c.ecx)?;
                push(c, g, c.ebp)?;
                g.set_u8(base.wrapping_add(3), 0)?;
                let dl = if look != 0 {
                    c.ebp = c.ecx.wrapping_add(0x14);
                    bits >> 6
                } else {
                    c.ebp = c.ecx.wrapping_add(4);
                    bits >> 5
                } & 1;
                c.edx = byte(c.edx, dl);
                if dl == 0 {
                    0xcbd39
                } else {
                    push(c, g, c.esi)?;
                    push(c, g, c.edi)?;
                    c.edi = base.wrapping_add(4);
                    c.esi = base.wrapping_add(0x10);
                    return call(c, callee::FRAME, 0xcbaad);
                }
            }
            0xcbaad => {
                let base = e(c);
                let look = g.u8(base.wrapping_add(0x58))?;
                c.eax = byte(c.eax, look);
                if look == 0 {
                    0xcbb82
                } else {
                    let s = |o: u32| base.wrapping_add(o);
                    load(c, 2, g.u32(s(0xc))?);
                    load(c, 4, g.u32(s(8))?);
                    c.ebx = c.ebx.wrapping_add(0x168);
                    c.eax = g.u32(c.ebx)?;
                    c.ecx = g.u32(c.ebx.wrapping_add(4))?;
                    c.edx = g.u32(c.ebx.wrapping_add(8))?;
                    movaps(c, 3, 2);
                    g.set_u32(s(0x18), c.edx)?;
                    load(c, 1, c.edx);
                    movaps(c, 0, 1);
                    ssr(c, Ss::Mul, 0, 4)?;
                    g.set_u32(s(0x14), c.ecx)?;
                    load(c, 5, c.ecx);
                    ssr(c, Ss::Mul, 3, 5)?;
                    ssr(c, Ss::Sub, 0, 3)?;
                    load(c, 3, g.u32(s(4))?);
                    g.set_u32(s(0x10), c.eax)?;
                    load(c, 6, c.eax);
                    rebase_look_up(c)?;
                    c.eax = c.edi;
                    st(c, g, s(4), 4)?;
                    st(c, g, s(8), 1)?;
                    st(c, g, s(0xc), 2)?;
                    return call(c, callee::NORMALIZE, 0xcbb67);
                }
            }
            0xcbb67 => {
                if ucomiss_lahf_jp(c, lo(c, 0), g.u32(THRESHOLD)?)? {
                    0xcbb82
                } else {
                    c.eax = c.edi;
                    c.ecx = c.esi;
                    return call(c, callee::PERPENDICULAR, 0xcbb7d);
                }
            }
            0xcbb7d => return call(c, callee::NORMALIZE, 0xcbb82),
            0xcbb82 => {
                let base = e(c);
                c.esi = g.u32(VECTOR_88)?;
                c.eax = base.wrapping_add(4);
                c.ecx = base.wrapping_add(0x10);
                c.edx = base.wrapping_add(0x1c);
                return call(c, callee::MATRIX, 0xcbb99);
            }
            0xcbb99 => return clamp_angles(c, g, e(c)),
            0xcbd39 => {
                // `esp` is `E - 8`.
                let al = g.u8(c.esp.wrapping_add(8).wrapping_add(3))?;
                c.eax = byte(c.eax, al);
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = c.esp.wrapping_add(0x50);
                return ret(c, 8);
            }
            _ => return Err(unowned(at)),
        };
    }
}

/// Both cross products at the look clamp. By this point the memory loads
/// have cleared every source's upper lanes and left.x is already in xmm0.
/// The register sequence also handles exceptional inputs and sticky flags.
fn rebase_look_up(c: &mut CpuAbi) -> Result<()> {
    let frame_up = [lo(c, 3), lo(c, 4), lo(c, 2)];
    let aim = [lo(c, 6), lo(c, 5), lo(c, 1)];
    if sim_core::det::fast_env(c.mxcsr) && sim_core::det::host_fast_env()
        && let Some(basis) = ::sim_objects::unit::look_up(frame_up, aim, lo(c, 0))
    {
        let [x, y, z] = basis.up;
        let [x_sub, y_sub, z_sub] = basis.subtrahends;
        for (r, value) in [(0, z_sub), (1, y), (2, z), (3, y_sub), (4, x), (7, x_sub)] {
            load(c, r, value);
        }
        return Ok(());
    }
    rebase_look_up_scalar(c)
}

fn rebase_look_up_scalar(c: &mut CpuAbi) -> Result<()> {
    movaps(c, 7, 1);
    ssr(c, Ss::Mul, 7, 3)?;
    ssr(c, Ss::Mul, 2, 6)?;
    ssr(c, Ss::Sub, 2, 7)?;
    ssr(c, Ss::Mul, 4, 6)?;
    movaps(c, 7, 2);
    ssr(c, Ss::Mul, 3, 5)?;
    ssr(c, Ss::Sub, 3, 4)?;
    ssr(c, Ss::Mul, 7, 1)?;
    movaps(c, 4, 3);
    ssr(c, Ss::Mul, 1, 0)?;
    ssr(c, Ss::Mul, 4, 5)?;
    ssr(c, Ss::Mul, 3, 6)?;
    ssr(c, Ss::Mul, 2, 6)?;
    ssr(c, Ss::Mul, 0, 5)?;
    ssr(c, Ss::Sub, 4, 7)?;
    ssr(c, Ss::Sub, 1, 3)?;
    ssr(c, Ss::Sub, 2, 0)?;
    Ok(())
}

/// `cbb99` to the return: the direction's yaw and pitch in the matrix at
/// `[E+1c]`, clamped to the limits at `ebp`, and written back if either
/// clamped.
fn clamp_angles(c: &mut CpuAbi, g: &mut Guest, base: u32) -> Result<Next> {
    let s = |o: u32| base.wrapping_add(o);
    c.eax = g.u32(s(0x54))?;
    load(c, 1, g.u32(c.eax.wrapping_add(4))?);
    load(c, 2, g.u32(c.eax.wrapping_add(8))?);
    load(c, 3, g.u32(s(0x28))?);
    load(c, 4, g.u32(s(0x24))?);
    load(c, 0, g.u32(c.eax)?);
    ssr(c, Ss::Mul, 3, 2)?;
    ssr(c, Ss::Mul, 4, 1)?;
    ssr(c, Ss::Add, 3, 4)?;
    load(c, 4, g.u32(s(0x20))?);
    ssr(c, Ss::Mul, 4, 0)?;
    ssr(c, Ss::Add, 3, 4)?;
    load(c, 4, g.u32(s(0x30))?);
    st(c, g, s(0x10), 3)?;
    load(c, 3, g.u32(s(0x34))?);
    ssr(c, Ss::Mul, 3, 2)?;
    ssr(c, Ss::Mul, 4, 1)?;
    ssr(c, Ss::Add, 3, 4)?;
    load(c, 4, g.u32(s(0x2c))?);
    ssr(c, Ss::Mul, 4, 0)?;
    ssr(c, Ss::Add, 3, 4)?;
    st(c, g, s(0x14), 3)?;
    let mut x = fpu(c);
    x_ok(x.fld(g.u32(s(0x14))?))?;
    x_ok(x.fld(g.u32(s(0x10))?))?;
    load(c, 3, g.u32(s(0x40))?);
    x_ok(x.unary(unary_op::PATAN))?;
    ssr(c, Ss::Mul, 3, 2)?;
    load(c, 2, g.u32(s(0x3c))?);
    ssr(c, Ss::Mul, 2, 1)?;
    load(c, 1, g.u32(s(0x38))?);
    ssr(c, Ss::Add, 3, 2)?;
    ssr(c, Ss::Mul, 1, 0)?;
    ssr(c, Ss::Add, 3, 1)?;
    st(c, g, s(0x18), 3)?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    // The yaw, then the pitch: atan2(z, sqrt(y*y + x*x)).
    g.set_u32(s(4), x_ok(x.fstp_m32())?)?;
    x_ok(x.fld(g.u32(s(0x18))?))?;
    let y = g.u32(s(0x14))?;
    x_ok(x.fld(y))?;
    x_ok(x.fmul_m(y))?;
    let xv = g.u32(s(0x10))?;
    x_ok(x.fld(xv))?;
    x_ok(x.fmul_m(xv))?;
    x_ok(x.arith_reg(arith_op::ADD, 1, 0, true))?;
    x_ok(x.unary(unary_op::SQRT))?;
    x_ok(x.fxch(1))?;
    x_ok(x.fxch(1))?;
    x_ok(x.unary(unary_op::PATAN))?;
    g.set_u32(s(8), x_ok(x.fstp_m32())?)?;
    // The yaw against its minimum, on the x87.
    x_ok(x.fld(g.u32(s(4))?))?;
    x_ok(x.fld(g.u32(c.ebp)?))?;
    let flags = fcomip(c, &mut x)?;
    x_ok(x.fstp_st(0))?;
    let limits = c.ebp;
    if !jbe(flags) {
        load(c, 0, g.u32(limits)?);
        st(c, g, s(4), 0)?;
        g.set_u8(s(3), 1)?;
    } else {
        load(c, 0, g.u32(s(4))?);
        if above(c, lo(c, 0), g.u32(limits.wrapping_add(4))?)? {
            load(c, 0, g.u32(limits.wrapping_add(4))?);
            st(c, g, s(4), 0)?;
            g.set_u8(s(3), 1)?;
        }
    }
    // cbc91: the pitch.
    load(c, 1, g.u32(limits.wrapping_add(8))?);
    load(c, 0, g.u32(s(8))?);
    let clamped = if above(c, lo(c, 1), lo(c, 0))? {
        movaps(c, 0, 1);
        st(c, g, s(8), 0)?;
        g.set_u8(s(3), 1)?;
        true
    } else if above(c, lo(c, 0), g.u32(limits.wrapping_add(0xc))?)? {
        load(c, 0, g.u32(limits.wrapping_add(0xc))?);
        st(c, g, s(8), 0)?;
        g.set_u8(s(3), 1)?;
        true
    } else {
        let cl = g.u8(s(3))?;
        c.ecx = byte(c.ecx, cl);
        cl != 0
    };
    if !clamped {
        // cbd45: the x87 state as the compare left it.
        fpu_done(c, g, X87(x.0), 0xcbc65, limits)?;
        fpu_done(c, g, x, 0xcbc6a, 0)?;
        c.ebp = pop(c, g)?;
        c.eax = byte(c.eax, c.ecx as u8);
        c.ebx = pop(c, g)?;
        c.esp = c.esp.wrapping_add(0x50);
        return ret(c, 8);
    }
    // cbcd1: the direction from the clamped angles, in world space.
    let (pitch, yaw) = (g.u32(s(8))?, g.u32(s(4))?);
    x_ok(x.fld(pitch))?;
    x_ok(x.fcos())?;
    x_ok(x.fld(yaw))?;
    x_ok(x.fcos())?;
    x_ok(x.arith_st(arith_op::MUL, 1))?;
    x_ok(x.fld(yaw))?;
    x_ok(x.fsin())?;
    x_ok(x.arith_st(arith_op::MUL, 2))?;
    x_ok(x.fld(pitch))?;
    x_ok(x.fsin())?;
    let out = c.eax;
    for (i, (up, left, forward)) in [(0x38u32, 0x2cu32, 0x20u32), (0x3c, 0x30, 0x24)].into_iter().enumerate() {
        x_ok(x.fld(g.u32(s(up))?))?;
        x_ok(x.arith_st(arith_op::MUL, 1))?;
        x_ok(x.fld_st(2))?;
        x_ok(x.fmul_m(g.u32(s(left))?))?;
        x_ok(x.arith_reg(arith_op::ADD, 1, 0, true))?;
        x_ok(x.fld(g.u32(s(forward))?))?;
        x_ok(x.arith_st(arith_op::MUL, 4))?;
        x_ok(x.arith_reg(arith_op::ADD, 1, 0, true))?;
        g.set_u32(out.wrapping_add(4 * i as u32), x_ok(x.fstp_m32())?)?;
    }
    x_ok(x.fmul_m(g.u32(s(0x40))?))?;
    x_ok(x.fxch(1))?;
    x_ok(x.fmul_m(g.u32(s(0x34))?))?;
    x_ok(x.arith_reg(arith_op::ADD, 1, 0, true))?;
    x_ok(x.fld(g.u32(s(0x28))?))?;
    x_ok(x.arith_st(arith_op::MUL, 2))?;
    x_ok(x.arith_reg(arith_op::ADD, 1, 0, true))?;
    g.set_u32(out.wrapping_add(8), x_ok(x.fstp_m32())?)?;
    x_ok(x.fstp_st(0))?;
    x_ok(x.fstp_st(0))?;
    fpu_done(c, g, X87(x.0), 0xcbd32, out.wrapping_add(8))?;
    fpu_done(c, g, x, 0xcbd37, 0)?;
    // cbd39
    let al = g.u8(s(3))?;
    c.eax = byte(c.eax, al);
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = c.esp.wrapping_add(0x50);
    ret(c, 8)
}

// ---------------------------------------------------------------- c85c0

/// `c85c0`'s return sites: the seven state tests, then the weapon test.
pub(super) const IDLE_SITES: [u32; 8] = [0xc8623, 0xc8633, 0xc8643, 0xc8650, 0xc8660, 0xc8670, 0xc8680, 0xc86c1];

/// The state tests' `ecx`, in call order.
const IDLE_STATES: [u32; 7] = [0x16, 8, 0x12, 0, 1, 0xa, 0xb];

/// `c85c0 (unit: eax) -> al`.
pub(super) fn idle(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::UNIT_IDLE_TEST.base;
    owned(pc, entry, &IDLE_SITES)?;
    let mut at = pc;
    loop {
        at = match at {
            0xc85c0 => {
                push(c, g, c.ecx)?;
                push(c, g, c.ebx)?;
                c.ebx = g.u32(points::OBJECTS.base)?;
                push(c, g, c.esi)?;
                push(c, g, c.edi)?;
                c.edi = c.eax;
                c.esi = (c.eax & 0xffff).wrapping_mul(3) << 2;
                c.eax = g.u32(c.ebx.wrapping_add(0x44))?;
                c.eax = g.u32(c.esi.wrapping_add(c.eax).wrapping_add(8))?;
                let parent = g.u32(c.eax.wrapping_add(0x14))?;
                g.set_u8(c.esp.wrapping_add(0xf), 1)?;
                if parent != NONE {
                    0xc8684
                } else {
                    let mut out = false;
                    for shift in [26, 27] {
                        c.ecx = sx16(g.u16(c.eax.wrapping_add(0x346))?);
                        c.edx = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(4))? >> shift;
                        if c.edx & 1 != 0 {
                            out = true;
                            break;
                        }
                    }
                    if out {
                        0xc8684
                    } else {
                        c.ecx = IDLE_STATES[0];
                        c.eax = c.edi;
                        return call(c, callee::STATE, IDLE_SITES[0]);
                    }
                }
            }
            0xc8623 | 0xc8633 | 0xc8643 | 0xc8650 | 0xc8660 | 0xc8670 | 0xc8680 => {
                let k = IDLE_SITES.iter().position(|&s| s == at).expect("a state site");
                if c.eax as u8 != 0 {
                    0xc8684
                } else if k + 1 < IDLE_STATES.len() {
                    c.ecx = IDLE_STATES[k + 1];
                    c.eax = c.edi;
                    return call(c, callee::STATE, IDLE_SITES[k + 1]);
                } else {
                    // c868b: the held weapons.
                    c.eax = g.u32(c.ebx.wrapping_add(0x44))?;
                    c.esi = g.u32(c.esi.wrapping_add(c.eax).wrapping_add(8))?;
                    push(c, g, c.ebp)?;
                    c.ebp = 0;
                    c.edi = c.esi.wrapping_add(0x212);
                    0xc86a0
                }
            }
            0xc86a0 => {
                let ax = sx8(g.u8(c.edi)?) & 0xffff;
                c.eax = (c.eax & 0xffff_0000) | ax;
                if ax == 0xffff {
                    0xc86c5
                } else {
                    c.ecx = sx16(ax as u16);
                    c.eax = g.u32(c.esi.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(0x218))?;
                    if c.eax == NONE {
                        0xc86c5
                    } else {
                        c.ebx = NONE;
                        return call(c, callee::WEAPON, 0xc86c1);
                    }
                }
            }
            0xc86c1 => {
                if c.eax as u8 != 0 {
                    // c86d6
                    c.ebp = pop(c, g)?;
                    c.edi = pop(c, g)?;
                    c.esi = pop(c, g)?;
                    c.eax &= !0xff;
                    c.ebx = pop(c, g)?;
                    c.ecx = pop(c, g)?;
                    return ret(c, 0);
                }
                0xc86c5
            }
            0xc86c5 => {
                c.ebp = c.ebp.wrapping_add(1);
                c.edi = c.edi.wrapping_add(1);
                if (c.ebp as i32) < 2 {
                    0xc86a0
                } else {
                    let al = g.u8(c.esp.wrapping_add(0x13))?;
                    c.eax = byte(c.eax, al);
                    c.ebp = pop(c, g)?;
                    c.edi = pop(c, g)?;
                    c.esi = pop(c, g)?;
                    c.ebx = pop(c, g)?;
                    c.ecx = pop(c, g)?;
                    return ret(c, 0);
                }
            }
            0xc8684 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.eax &= !0xff;
                c.ebx = pop(c, g)?;
                c.ecx = pop(c, g)?;
                return ret(c, 0);
            }
            _ => return Err(unowned(at)),
        };
    }
}

// ---------------------------------------------------------------- c7840

/// `c7840`'s return sites (the port's).
pub(super) const AIM_SITES: &[u32] = unit_aim::SITES;

/// `c7840`: the aim update (`eax`, `esi`, `edi` and five stack arguments;
/// `ret 14`), the port `sim_physics::unit_aim` run as a body. The port
/// keeps the flags, `xmm` and x87 state at every transfer.
pub(super) fn aim(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_AIM_UPDATE.base, AIM_SITES)?;
    Ok(match unit_aim::step(c, g, pc)? {
        unit_aim::Exit::Call(target, site) => Next::Call { target, site },
        unit_aim::Exit::Ret(pop) => Next::Ret { pop },
    })
}

#[cfg(test)]
mod tests;
