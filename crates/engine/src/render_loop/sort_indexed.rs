//! The engine's index sort `0x0013df80`, which the transparents pass
//! `0x0001e370` calls to order its 16-bit entry indices (0x70-byte records)
//! before it draws them one indirect call each (docs/graphics/render-loop.md).
//! Port grade, as [`super::sort`] (the layer sort): every register the
//! original leaves (except `xmm2`, below) and every stack byte it writes,
//! including the pushed comparator arguments and return addresses, so the
//! whole of guest memory is equal afterwards.
//!
//! The saving: the original calls its comparator through a pointer for every
//! comparison, a guest indirect transfer each. The transparents' comparator
//! `0x0001e700` is a pure read of two records, so the port evaluates it in
//! Rust with its register and stack effects and makes no guest call. Any
//! other comparator is a [`Step::Call`] with the registers and stack as the
//! original has them at its `call`.
//!
//! The read (Code, `h2 disasm`):
//!
//! - Convention: EAX the element count, ECX the base of a `u16` array,
//!   stack `(unused, compare, context)`, `ret 0xc`. `compare(a, b,
//!   context)` is stdcall with the two indices zero-extended, AL non-zero
//!   when `a` sorts after `b`. Nothing is done for fewer than two elements.
//! - The same CRT shape as the layer sort, specialised to 2-byte elements:
//!   a 30-deep partition stack in its frame, ranges of 8 or fewer to a
//!   selection sort (`0x0013e140`, EAX the last element, stdcall `(lo,
//!   compare, context)`, `ret 0xc`) that the port runs inline, middle pivot
//!   swapped to the front, the larger side pushed. Not stable.
//! - `0x0001e700` (`ret 0xc`): records at `context + 0x70 * index` (indices
//!   sign-extended). Unequal dwords `+0` decide (a after b when a's is
//!   larger, signed). Else, when a's byte `+0xc` is set (or else b's), two
//!   dot products in single precision against the other record's vector at
//!   `+0x64` and the camera vector at `0x004b9da0`, compared with the
//!   flagged record's `+0x1c`. Else floats `+4`, then dwords `+8`.
//! - `xmm2` is not reproduced: `Regs` has no `xmm2`, and the comparator
//!   leaves the flagged record's `+0x1c` there. The SSE arithmetic follows
//!   x86 (NaN operands propagate the first source; invalid gives the
//!   default NaN) with round-to-nearest and no flush-to-zero, MXCSR's reset
//!   state.

use super::{Regs, Step};
use crate::memory::{GuestMemory, MemFault};

/// The index sort's entry.
pub const INDEX_SORT: u32 = 0x0013_df80;
/// Its selection sort for short ranges.
pub const SHORT_SORT: u32 = 0x0013_e140;
/// The transparents' comparator.
pub const COMPARE_TRANSPARENT: u32 = 0x0001_e700;
/// The camera vector it reads.
pub const CAMERA: u32 = 0x004b_9da0;
/// Bytes a record takes.
pub const RECORD_SIZE: u32 = 0x70;
/// Bytes a comparator pops.
pub const COMPARE_CLEANUP: u32 = 0xc;
/// Bytes the sort pops.
pub const RET_CLEANUP: u32 = 0xc;

/// Return addresses of the comparator calls and of the short sort.
const RET_LO: u32 = 0x0013_e01a;
const RET_HI: u32 = 0x0013_e042;
const RET_SHORT: u32 = 0x0013_e175;
const RET_SHORT_SORT: u32 = 0x0013_dfc5;

fn lo16(r: u32, v: u16) -> u32 {
    (r & !0xffff) | v as u32
}

/// x86 `mulss`/`addss` on the low lane: a NaN operand propagates (the
/// first source when both are), quieted; an invalid operation gives the
/// default NaN.
fn sse(a: u32, b: u32, op: fn(f32, f32) -> f32) -> u32 {
    let (fa, fb) = (f32::from_bits(a), f32::from_bits(b));
    if fa.is_nan() {
        return a | 0x0040_0000;
    }
    if fb.is_nan() {
        return b | 0x0040_0000;
    }
    let r = op(fa, fb);
    if r.is_nan() { 0xffc0_0000 } else { r.to_bits() }
}
fn mulss(a: u32, b: u32) -> u32 {
    sse(a, b, |x, y| x * y)
}
fn addss(a: u32, b: u32) -> u32 {
    sse(a, b, |x, y| x + y)
}
/// `comiss x, y; jbe` not taken: ordered and above.
fn above(x: u32, y: u32) -> bool {
    f32::from_bits(x) > f32::from_bits(y)
}
/// The low lane with the rest cleared, as `movss xmm, [mem]` leaves it.
fn lane(v: u32) -> u128 {
    v as u128
}

/// `0x0013df80` in progress.
#[derive(Clone, Copy, Debug)]
pub struct IndexSort {
    /// ESP at entry (pointing at the return address).
    e: u32,
    ret_to: u32,
    /// Where to continue after a guest comparator returns.
    resume_at: u32,
}

impl IndexSort {
    /// Enters `0x0013df80` with the CPU at its first instruction.
    pub fn start(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<(IndexSort, Step), MemFault> {
        let e = r.esp;
        let mut s = IndexSort {
            e,
            ret_to: m.u32(e)?,
            resume_at: 0,
        };
        let step = s.run(INDEX_SORT, r, m)?;
        Ok((s, step))
    }

    /// Continues after the comparator [`Step::Call`] asked for returned.
    pub fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        self.run(self.resume_at, r, m)
    }

    fn push(r: &mut Regs, m: &mut dyn GuestMemory, v: u32) -> Result<(), MemFault> {
        r.esp = r.esp.wrapping_sub(4);
        m.set_u32(r.esp, v)
    }
    fn pop(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
        let v = m.u32(r.esp)?;
        r.esp = r.esp.wrapping_add(4);
        Ok(v)
    }

    /// `call [compare]` with its three arguments pushed, returning to
    /// `ret`: natively for `0x0001e700` (`None`, continue at `ret`), else a
    /// guest call.
    fn compare(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        target: u32,
        ret: u32,
    ) -> Result<Option<Step>, MemFault> {
        if target != COMPARE_TRANSPARENT {
            self.resume_at = ret;
            return Ok(Some(Step::Call {
                target,
                site: ret,
                cleanup: COMPARE_CLEANUP,
            }));
        }
        Self::push(r, m, ret)?;
        let c = r.esp;
        transparent(r, m, c)?;
        r.esp = c.wrapping_add(4 + COMPARE_CLEANUP);
        Ok(None)
    }

    /// The original from `pc` to its next foreign call or its return. The
    /// labels are the original's addresses.
    fn run(
        &mut self,
        mut pc: u32,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
    ) -> Result<Step, MemFault> {
        let e = self.e;
        // The frame, by entry ESP.
        let depth = e.wrapping_sub(0xf4);
        let lostk = e.wrapping_sub(0xf0);
        let histk = e.wrapping_sub(0x78);
        let cmp_arg = e.wrapping_add(8);
        let ctx_arg = e.wrapping_add(0xc);
        // The short sort's entry ESP (its return address slot).
        let s = e.wrapping_sub(0x114);
        let slot = |base: u32, i: u32| base.wrapping_add(i.wrapping_mul(4));
        loop {
            match pc {
                INDEX_SORT => {
                    r.esp = e.wrapping_sub(0xf4);
                    let n = r.eax;
                    Self::push(r, m, r.esi)?;
                    r.esi = r.ecx;
                    if n < 2 {
                        pc = 0x0013_e0d4;
                        continue;
                    }
                    Self::push(r, m, r.ebp)?;
                    Self::push(r, m, r.ebx)?;
                    m.set_u32(depth, 0)?;
                    r.ebp = r.esi.wrapping_add(n.wrapping_mul(2)).wrapping_sub(2);
                    Self::push(r, m, r.edi)?;
                    pc = 0x0013_dfa1;
                }
                0x0013_dfa1 => {
                    r.eax = (((r.ebp.wrapping_sub(r.esi)) as i32 >> 1) as u32).wrapping_add(1);
                    pc = if r.eax > 8 { 0x0013_dfe1 } else { 0x0013_dfad };
                }
                0x0013_dfad => {
                    r.eax = m.u32(ctx_arg)?;
                    r.ecx = m.u32(cmp_arg)?;
                    Self::push(r, m, r.eax)?;
                    Self::push(r, m, r.ecx)?;
                    Self::push(r, m, r.esi)?;
                    r.eax = r.ebp;
                    Self::push(r, m, RET_SHORT_SORT)?;
                    debug_assert_eq!(r.esp, s);
                    // 0x0013e140
                    Self::push(r, m, r.ebp)?;
                    r.ebp = m.u32(s + 4)?;
                    Self::push(r, m, r.edi)?;
                    r.edi = r.eax;
                    if r.edi <= r.ebp {
                        pc = 0x0013_e196;
                        continue;
                    }
                    Self::push(r, m, r.ebx)?;
                    Self::push(r, m, r.esi)?;
                    pc = 0x0013_e150;
                }
                0x0013_e150 => {
                    r.esi = r.ebp.wrapping_add(2);
                    r.ebx = r.ebp;
                    pc = if r.esi > r.edi {
                        0x0013_e181
                    } else {
                        0x0013_e160
                    };
                }
                0x0013_e160 => {
                    r.eax = m.u32(s + 0xc)?;
                    r.ecx = m.u16(r.ebx)? as u32;
                    r.edx = m.u16(r.esi)? as u32;
                    Self::push(r, m, r.eax)?;
                    Self::push(r, m, r.ecx)?;
                    Self::push(r, m, r.edx)?;
                    let target = m.u32(s + 8)?;
                    if let Some(step) = self.compare(r, m, target, RET_SHORT)? {
                        return Ok(step);
                    }
                    pc = RET_SHORT;
                }
                RET_SHORT => {
                    if r.eax & 0xff != 0 {
                        r.ebx = r.esi;
                    }
                    r.esi = r.esi.wrapping_add(2);
                    pc = if r.esi <= r.edi {
                        0x0013_e160
                    } else {
                        0x0013_e181
                    };
                }
                0x0013_e181 => {
                    r.ecx = lo16(r.ecx, m.u16(r.edi)?);
                    r.eax = lo16(r.eax, m.u16(r.ebx)?);
                    m.set_u16(r.ebx, r.ecx as u16)?;
                    m.set_u16(r.edi, r.eax as u16)?;
                    r.edi = r.edi.wrapping_sub(2);
                    if r.edi > r.ebp {
                        pc = 0x0013_e150;
                        continue;
                    }
                    r.esi = Self::pop(r, m)?;
                    r.ebx = Self::pop(r, m)?;
                    pc = 0x0013_e196;
                }
                0x0013_e196 => {
                    r.edi = Self::pop(r, m)?;
                    r.ebp = Self::pop(r, m)?;
                    // ret 0xc to 0x0013dfc5
                    r.esp = r.esp.wrapping_add(4 + 0xc);
                    pc = RET_SHORT_SORT;
                }
                RET_SHORT_SORT => {
                    r.eax = m.u32(depth)?.wrapping_sub(1);
                    m.set_u32(depth, r.eax)?;
                    if (r.eax as i32) < 0 {
                        pc = 0x0013_e0d1;
                        continue;
                    }
                    r.esi = m.u32(slot(lostk, r.eax))?;
                    r.ebp = m.u32(slot(histk, r.eax))?;
                    pc = 0x0013_dfa1;
                }
                0x0013_dfe1 => {
                    r.edx = lo16(r.edx, m.u16(r.esi)?);
                    r.eax >>= 1;
                    let mid = r.esi.wrapping_add(r.eax.wrapping_mul(2));
                    r.ecx = lo16(r.ecx, m.u16(mid)?);
                    r.eax = mid;
                    m.set_u16(r.eax, r.edx as u16)?;
                    m.set_u16(r.esi, r.ecx as u16)?;
                    r.ebx = r.esi;
                    r.edi = r.ebp.wrapping_add(2);
                    pc = 0x0013_dff8;
                }
                0x0013_dff8 => {
                    r.ebx = r.ebx.wrapping_add(2);
                    if r.ebx > r.ebp {
                        pc = 0x0013_e020;
                        continue;
                    }
                    r.eax = m.u32(ctx_arg)?;
                    r.ecx = m.u16(r.esi)? as u32;
                    r.edx = m.u16(r.ebx)? as u32;
                    Self::push(r, m, r.eax)?;
                    Self::push(r, m, r.ecx)?;
                    Self::push(r, m, r.edx)?;
                    let target = m.u32(cmp_arg)?;
                    if let Some(step) = self.compare(r, m, target, RET_LO)? {
                        return Ok(step);
                    }
                    pc = RET_LO;
                }
                RET_LO => {
                    pc = if r.eax & 0xff == 0 {
                        0x0013_dff8
                    } else {
                        0x0013_e020
                    };
                }
                0x0013_e020 => {
                    r.edi = r.edi.wrapping_sub(2);
                    if r.edi <= r.esi {
                        pc = 0x0013_e046;
                        continue;
                    }
                    r.eax = m.u32(ctx_arg)?;
                    r.ecx = m.u16(r.edi)? as u32;
                    r.edx = m.u16(r.esi)? as u32;
                    Self::push(r, m, r.eax)?;
                    Self::push(r, m, r.ecx)?;
                    Self::push(r, m, r.edx)?;
                    let target = m.u32(cmp_arg)?;
                    if let Some(step) = self.compare(r, m, target, RET_HI)? {
                        return Ok(step);
                    }
                    pc = RET_HI;
                }
                RET_HI => {
                    pc = if r.eax & 0xff == 0 {
                        0x0013_e020
                    } else {
                        0x0013_e046
                    };
                }
                0x0013_e046 => {
                    if r.edi < r.ebx {
                        pc = 0x0013_e058;
                        continue;
                    }
                    r.ecx = lo16(r.ecx, m.u16(r.edi)?);
                    r.eax = lo16(r.eax, m.u16(r.ebx)?);
                    m.set_u16(r.ebx, r.ecx as u16)?;
                    m.set_u16(r.edi, r.eax as u16)?;
                    pc = 0x0013_dff8;
                }
                0x0013_e058 => {
                    r.eax = lo16(r.eax, m.u16(r.esi)?);
                    r.edx = lo16(r.edx, m.u16(r.edi)?);
                    m.set_u16(r.esi, r.edx as u16)?;
                    m.set_u16(r.edi, r.eax as u16)?;
                    r.eax = r.edi.wrapping_sub(r.esi).wrapping_sub(2) & !1;
                    r.ecx = r.ebp.wrapping_sub(r.ebx) & !1;
                    if (r.eax as i32) < (r.ecx as i32) {
                        pc = 0x0013_e0a6;
                        continue;
                    }
                    r.edx = r.esi.wrapping_add(2);
                    if r.edx < r.edi {
                        r.eax = m.u32(depth)?;
                        r.edi = r.edi.wrapping_sub(2);
                        m.set_u32(slot(lostk, r.eax), r.esi)?;
                        m.set_u32(slot(histk, r.eax), r.edi)?;
                        r.eax = r.eax.wrapping_add(1);
                        m.set_u32(depth, r.eax)?;
                    }
                    if r.ebx >= r.ebp {
                        pc = RET_SHORT_SORT;
                        continue;
                    }
                    r.esi = r.ebx;
                    pc = 0x0013_dfa1;
                }
                0x0013_e0a6 => {
                    if r.ebx < r.ebp {
                        r.eax = m.u32(depth)?;
                        m.set_u32(slot(lostk, r.eax), r.ebx)?;
                        m.set_u32(slot(histk, r.eax), r.ebp)?;
                        r.eax = r.eax.wrapping_add(1);
                        m.set_u32(depth, r.eax)?;
                    }
                    r.eax = r.esi.wrapping_add(2);
                    if r.eax >= r.edi {
                        pc = RET_SHORT_SORT;
                        continue;
                    }
                    r.ebp = r.edi.wrapping_sub(2);
                    pc = 0x0013_dfa1;
                }
                0x0013_e0d1 => {
                    r.edi = Self::pop(r, m)?;
                    r.ebx = Self::pop(r, m)?;
                    r.ebp = Self::pop(r, m)?;
                    pc = 0x0013_e0d4;
                }
                0x0013_e0d4 => {
                    r.esi = Self::pop(r, m)?;
                    r.esp = e.wrapping_add(4 + RET_CLEANUP);
                    return Ok(Step::Return { to: self.ret_to });
                }
                _ => unreachable!("index sort port: no block at {pc:#x}"),
            }
        }
    }
}

/// `0x0001e700` with ESP at `c` (its return address): AL, ECX, EDX,
/// `xmm0` and `xmm1` as it leaves them.
fn transparent(r: &mut Regs, m: &mut dyn GuestMemory, c: u32) -> Result<(), MemFault> {
    let ia = m.u16(c + 4)? as i16 as i32 as u32;
    let ib = m.u16(c + 8)? as i16 as i32 as u32;
    let ctx = m.u32(c + 0xc)?;
    let a = ia.wrapping_mul(RECORD_SIZE).wrapping_add(ctx);
    let b = ib.wrapping_mul(RECORD_SIZE).wrapping_add(ctx);
    r.eax = a;
    r.ecx = b;
    r.edx = m.u32(a)?.wrapping_sub(m.u32(b)?);
    if r.edx != 0 {
        r.eax = ((r.edx as i32) > 0) as u32;
        return Ok(());
    }
    let fa = m.u8(a + 0xc)?;
    r.edx = fa as u32;
    if fa != 0 {
        // The flagged record `a` against `b`'s vector.
        let (x0, x1, f1c) = dots(m, a, b)?;
        r.ecx = above(f1c, x1) as u32;
        r.xmm0 = lane(x0);
        r.xmm1 = lane(f1c);
        let near = r.ecx != if above(f1c, x0) { 1 } else { 0 };
        r.edx = if near { 1 } else { u32::MAX };
        r.eax = near as u32;
        return Ok(());
    }
    let fb = m.u8(b + 0xc)?;
    r.edx = fb as u32;
    if fb != 0 {
        let (x0, x1, f1c) = dots(m, b, a)?;
        let flag = above(f1c, x1) as u32;
        r.xmm0 = lane(x0);
        r.xmm1 = lane(f1c);
        let after = flag == if above(f1c, x0) { 1 } else { 0 };
        r.ecx = if after { 1 } else { u32::MAX };
        r.edx = r.ecx;
        r.eax = after as u32;
        return Ok(());
    }
    let (a4, b4) = (m.u32(a + 4)?, m.u32(b + 4)?);
    r.xmm0 = lane(a4);
    if above(a4, b4) {
        r.edx = 1;
        r.eax = 1;
        return Ok(());
    }
    r.xmm0 = lane(b4);
    if above(b4, a4) {
        r.edx = u32::MAX;
        r.eax = 0;
        return Ok(());
    }
    r.edx = m.u32(a + 8)?.wrapping_sub(m.u32(b + 8)?);
    r.eax = ((r.edx as i32) > 0) as u32;
    Ok(())
}

/// The two single-precision sums for flagged record `f` against `o`, in the
/// original's operation order: (`o` vector `+0x64` . `f` `+0x10`, the
/// camera . `f` `+0x10`, `f`'s `+0x1c`).
fn dots(m: &mut dyn GuestMemory, f: u32, o: u32) -> Result<(u32, u32, u32), MemFault> {
    let fv = |k: u32| m.u32(f + 0x10 + 4 * k);
    let (f0, f1, f2) = (fv(0)?, fv(1)?, fv(2)?);
    let mut x0 = mulss(m.u32(o + 0x6c)?, f2);
    let mut x1 = mulss(m.u32(o + 0x68)?, f1);
    let mut x2 = mulss(f2, m.u32(CAMERA + 8)?);
    x0 = addss(x0, x1);
    x1 = mulss(m.u32(o + 0x64)?, f0);
    x0 = addss(x0, x1);
    x1 = mulss(f1, m.u32(CAMERA + 4)?);
    x1 = addss(x1, x2);
    x2 = mulss(f0, m.u32(CAMERA)?);
    x1 = addss(x1, x2);
    Ok((x0, x1, m.u32(f + 0x1c)?))
}
