//! The engine's qsort `0x0013da70` (docs/graphics/layer-draw.md), which the
//! layer build `0x0003bab0` calls to order a layer's entries before the entry
//! loop draws them. Port grade, as [`super::Machine`]: every register and
//! every stack byte the original writes, including the dead ones (the
//! pushed comparator arguments, return addresses and the comparators' saved
//! ESI), so the whole of guest memory is equal afterwards.
//!
//! The saving: the original calls its comparator through a pointer for every
//! comparison, a guest-to-guest indirect transfer each. The two comparators
//! the layer build passes (`0x0003bb80`, and `0x0003bbd0` for layer 5) are
//! pure reads of two entries, so the port evaluates them in Rust with their
//! exact register and stack effects and makes no guest call at all. Any
//! other comparator is a [`Step::Call`] with the registers and stack as the
//! original has them at its `call`.
//!
//! The read (Code, `h2 disasm`):
//!
//! - Convention: ECX the element count, EAX the base, stack `(size,
//!   compare, context)`, `ret 0xc`. `compare(a, b, context)` is stdcall and
//!   returns AL non-zero when `a` sorts after `b`. Nothing is done for fewer
//!   than two elements or a zero size.
//! - It is the old CRT qsort shape with an explicit stack of 30 partitions
//!   in its frame: ranges of 8 or fewer elements go to a selection sort
//!   (`0x0013dcd0`, stdcall `(lo, hi, size, aligned, compare, context)`,
//!   `ret 0x18`), which the port also runs inline; larger ones take the
//!   middle element as pivot (swapped to the front), partition, and push the
//!   larger side. Swaps go dword by dword when the base and size are both
//!   multiples of 4, byte by byte otherwise.
//! - Not stable: equal entries end in an order only this exact algorithm
//!   reproduces, which is why the port follows it step for step rather than
//!   using a Rust sort.

use super::{Regs, Step};
use crate::memory::{GuestMemory, MemFault};

/// The qsort's entry.
pub const QSORT: u32 = 0x0013_da70;
/// Its selection sort for short ranges.
pub const SHORT_SORT: u32 = 0x0013_dcd0;
/// The layer build's comparators: most layers, and layer 5.
pub const COMPARE_LAYER: u32 = 0x0003_bb80;
pub const COMPARE_LAYER_5: u32 = 0x0003_bbd0;
/// Bytes a comparator pops.
pub const COMPARE_CLEANUP: u32 = 0xc;
/// Bytes the qsort pops.
pub const RET_CLEANUP: u32 = 0xc;

/// Return addresses of the comparator calls (the original's).
const RET_SHORT: u32 = 0x0013_dd0b;
const RET_LO: u32 = 0x0013_db73;
const RET_HI: u32 = 0x0013_db97;
/// The short sort's return into the qsort.
const RET_SHORT_SORT: u32 = 0x0013_daeb;

/// `0x0013da70` in progress.
#[derive(Clone, Copy, Debug)]
pub struct Sort {
    /// ESP at entry (pointing at the return address).
    e: u32,
    ret_to: u32,
    /// Where to continue after the guest comparator returns.
    resume_at: u32,
}

fn lo8(r: u32, v: u8) -> u32 {
    (r & !0xff) | v as u32
}
fn lo16(r: u32, v: u16) -> u32 {
    (r & !0xffff) | v as u32
}

impl Sort {
    /// Enters `0x0013da70` with the CPU at its first instruction.
    pub fn start(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<(Sort, Step), MemFault> {
        let e = r.esp;
        let mut s = Sort {
            e,
            ret_to: m.u32(e)?,
            resume_at: 0,
        };
        let step = s.run(0x0013_da70, r, m)?;
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
    /// `ret`: natively for the known comparators (`None`, continue at
    /// `ret`), else a guest call.
    fn compare(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        target: u32,
        ret: u32,
    ) -> Result<Option<Step>, MemFault> {
        if target != COMPARE_LAYER && target != COMPARE_LAYER_5 {
            self.resume_at = ret;
            return Ok(Some(Step::Call {
                target,
                site: ret,
                cleanup: COMPARE_CLEANUP,
            }));
        }
        Self::push(r, m, ret)?;
        let c = r.esp;
        let a = m.u32(c + 4)?;
        let b = m.u32(c + 8)?;
        r.eax = a;
        // Each key as (a's value, b's value); the registers they go
        // through are set as the original sets them.
        let after = if target == COMPARE_LAYER {
            r.ecx = m.u32(a + 0x10)?;
            m.set_u32(c - 4, r.esi)?;
            r.esi = b;
            r.edx = m.u32(b + 0x10)?;
            self.layer_keys(r, m, a, b)?
        } else {
            r.ecx = lo8(r.ecx, m.u8(a + 5)?);
            m.set_u32(c - 4, r.esi)?;
            r.esi = b;
            r.edx = lo8(r.edx, m.u8(b + 5)?);
            self.layer5_keys(r, m, a, b)?
        };
        r.eax = lo8(r.eax, after as u8);
        r.esi = m.u32(c - 4)?;
        r.esp = c + 4 + COMPARE_CLEANUP;
        Ok(None)
    }

    /// `0x0003bb80` after its first loads: `[+0x10]`, word `[+2]`, byte
    /// `[+5]`, word `[+8]`, unsigned, a after b.
    fn layer_keys(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        a: u32,
        b: u32,
    ) -> Result<bool, MemFault> {
        if r.ecx != r.edx {
            return Ok(r.ecx > r.edx);
        }
        r.ecx = lo16(r.ecx, m.u16(a + 2)?);
        r.edx = lo16(r.edx, m.u16(b + 2)?);
        if r.ecx as u16 != r.edx as u16 {
            return Ok(r.ecx as u16 > r.edx as u16);
        }
        r.ecx = lo8(r.ecx, m.u8(a + 5)?);
        r.edx = lo8(r.edx, m.u8(b + 5)?);
        if r.ecx as u8 != r.edx as u8 {
            return Ok(r.ecx as u8 > r.edx as u8);
        }
        r.eax = lo16(r.eax, m.u16(a + 8)?);
        Ok(r.eax as u16 > m.u16(b + 8)?)
    }

    /// `0x0003bbd0` after its first loads: byte `[+5]`, word `[+6]`,
    /// `[+0x10]`, word `[+2]`, `[+0xc]`, word `[+0]`.
    fn layer5_keys(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        a: u32,
        b: u32,
    ) -> Result<bool, MemFault> {
        if r.ecx as u8 != r.edx as u8 {
            return Ok(r.ecx as u8 > r.edx as u8);
        }
        r.ecx = lo16(r.ecx, m.u16(a + 6)?);
        r.edx = lo16(r.edx, m.u16(b + 6)?);
        if r.ecx as u16 != r.edx as u16 {
            return Ok(r.ecx as u16 > r.edx as u16);
        }
        r.ecx = m.u32(a + 0x10)?;
        r.edx = m.u32(b + 0x10)?;
        if r.ecx != r.edx {
            return Ok(r.ecx > r.edx);
        }
        r.ecx = lo16(r.ecx, m.u16(a + 2)?);
        r.edx = lo16(r.edx, m.u16(b + 2)?);
        if r.ecx as u16 != r.edx as u16 {
            return Ok(r.ecx as u16 > r.edx as u16);
        }
        r.ecx = m.u32(a + 0xc)?;
        r.edx = m.u32(b + 0xc)?;
        if r.ecx != r.edx {
            return Ok(r.ecx > r.edx);
        }
        r.eax = lo16(r.eax, m.u16(a)?);
        Ok(r.eax as u16 > m.u16(b)?)
    }

    /// The original from `pc` to its next foreign call or its return. The
    /// labels are the original's addresses; locals are read and written in
    /// its frame (`e` the entry ESP).
    fn run(
        &mut self,
        mut pc: u32,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
    ) -> Result<Step, MemFault> {
        let e = self.e;
        // The frame, by entry ESP.
        let depth = e.wrapping_sub(0x100);
        let aligned = e.wrapping_sub(0xfc);
        let hi_var = e.wrapping_sub(0x10c);
        let lo_var = e.wrapping_sub(0xf8);
        let scan_var = e.wrapping_sub(0x108);
        let tmp_dword = e.wrapping_sub(0xf4);
        let tmp_byte = e.wrapping_sub(0x101);
        let lostk = e.wrapping_sub(0xf0);
        let histk = e.wrapping_sub(0x78);
        let size_arg = e.wrapping_add(4);
        let cmp_arg = e.wrapping_add(8);
        let ctx_arg = e.wrapping_add(0xc);
        // The short sort's entry ESP (its return address slot).
        let s = e.wrapping_sub(0x138);
        loop {
            match pc {
                0x0013_da70 => {
                    r.esp = e.wrapping_sub(0x10c);
                    Self::push(r, m, r.ebx)?;
                    r.ebx = m.u32(size_arg)?;
                    if r.ecx < 2 || r.ebx == 0 {
                        pc = 0x0013_dcb8;
                        continue;
                    }
                    m.set_u32(depth, 0)?;
                    let al = r.ebx & 3 == 0 && r.eax & 3 == 0;
                    m.set_u8(aligned, al as u8)?;
                    r.ecx = r.ecx.wrapping_sub(1).wrapping_mul(r.ebx);
                    Self::push(r, m, r.ebp)?;
                    r.ecx = r.ecx.wrapping_add(r.eax);
                    Self::push(r, m, r.esi)?;
                    r.ebp = r.eax;
                    m.set_u32(hi_var, r.ecx)?;
                    Self::push(r, m, r.edi)?;
                    pc = 0x0013_dab9;
                }
                0x0013_dab9 => {
                    m.set_u32(lo_var, r.ebp)?;
                    pc = 0x0013_dac0;
                }
                0x0013_dac0 => {
                    let span = r.ecx.wrapping_sub(r.ebp);
                    r.eax = span / r.ebx;
                    r.edx = span % r.ebx;
                    r.eax = r.eax.wrapping_add(1);
                    pc = if r.eax > 8 { 0x0013_db0d } else { 0x0013_dace };
                }
                0x0013_dace => {
                    r.eax = m.u32(ctx_arg)?;
                    r.edx = m.u32(cmp_arg)?;
                    Self::push(r, m, r.eax)?;
                    r.eax = m.u32(aligned)?;
                    Self::push(r, m, r.edx)?;
                    Self::push(r, m, r.eax)?;
                    Self::push(r, m, r.ebx)?;
                    Self::push(r, m, r.ecx)?;
                    Self::push(r, m, r.ebp)?;
                    Self::push(r, m, RET_SHORT_SORT)?;
                    debug_assert_eq!(r.esp, s);
                    // 0x0013dcd0
                    Self::push(r, m, r.ebp)?;
                    r.ebp = m.u32(s + 8)?;
                    Self::push(r, m, r.edi)?;
                    r.edi = m.u32(s + 4)?;
                    if r.ebp <= r.edi {
                        pc = 0x0013_dd5f;
                        continue;
                    }
                    Self::push(r, m, r.ebx)?;
                    r.ebx = m.u32(s + 0xc)?;
                    r.eax = r.edi.wrapping_add(r.ebx);
                    Self::push(r, m, r.esi)?;
                    m.set_u32(s + 8, r.eax)?;
                    pc = 0x0013_dcf5;
                }
                0x0013_dcf1 => {
                    r.edi = m.u32(s + 4)?;
                    pc = 0x0013_dcf5;
                }
                0x0013_dcf5 => {
                    r.esi = m.u32(s + 8)?;
                    pc = if r.esi > r.ebp {
                        0x0013_dd16
                    } else {
                        0x0013_dd00
                    };
                }
                0x0013_dd00 => {
                    r.ecx = m.u32(s + 0x18)?;
                    Self::push(r, m, r.ecx)?;
                    Self::push(r, m, r.edi)?;
                    Self::push(r, m, r.esi)?;
                    let target = m.u32(s + 0x14)?;
                    if let Some(step) = self.compare(r, m, target, RET_SHORT)? {
                        return Ok(step);
                    }
                    pc = RET_SHORT;
                }
                RET_SHORT => {
                    if r.eax & 0xff != 0 {
                        r.edi = r.esi;
                    }
                    r.esi = r.esi.wrapping_add(r.ebx);
                    pc = if r.esi <= r.ebp {
                        0x0013_dd00
                    } else {
                        0x0013_dd16
                    };
                }
                0x0013_dd16 => {
                    let flag = m.u8(s + 0x10)?;
                    r.ecx = r.ebp;
                    r.eax = r.edi;
                    if flag != 0 {
                        r.edx = r.ebx >> 2;
                        loop {
                            r.edi = m.u32(r.ecx)?;
                            r.esi = m.u32(r.eax)?;
                            m.set_u32(r.eax, r.edi)?;
                            r.eax = r.eax.wrapping_add(4);
                            m.set_u32(r.ecx, r.esi)?;
                            r.ecx = r.ecx.wrapping_add(4);
                            r.edx = r.edx.wrapping_sub(1);
                            if (r.edx as i32) <= 0 {
                                break;
                            }
                        }
                    } else {
                        r.esi = r.ebx;
                        loop {
                            r.ebx = lo8(r.ebx, m.u8(r.ecx)?);
                            r.edx = lo8(r.edx, m.u8(r.eax)?);
                            m.set_u8(r.eax, r.ebx as u8)?;
                            r.eax = r.eax.wrapping_add(1);
                            m.set_u8(r.ecx, r.edx as u8)?;
                            r.ecx = r.ecx.wrapping_add(1);
                            r.esi = r.esi.wrapping_sub(1);
                            if (r.esi as i32) <= 0 {
                                break;
                            }
                        }
                        r.ebx = m.u32(s + 0xc)?;
                    }
                    r.eax = m.u32(s + 4)?;
                    r.ebp = r.ebp.wrapping_sub(r.ebx);
                    if r.ebp > r.eax {
                        pc = 0x0013_dcf1;
                        continue;
                    }
                    r.esi = Self::pop(r, m)?;
                    r.ebx = Self::pop(r, m)?;
                    pc = 0x0013_dd5f;
                }
                0x0013_dd5f => {
                    r.edi = Self::pop(r, m)?;
                    r.ebp = Self::pop(r, m)?;
                    // ret 0x18 to 0x0013daeb
                    r.esp = r.esp.wrapping_add(4 + 0x18);
                    pc = RET_SHORT_SORT;
                }
                RET_SHORT_SORT => {
                    r.eax = m.u32(depth)?.wrapping_sub(1);
                    m.set_u32(depth, r.eax)?;
                    if (r.eax as i32) < 0 {
                        pc = 0x0013_dcb5;
                        continue;
                    }
                    r.edx = m.u32(histk.wrapping_add(r.eax.wrapping_mul(4)))?;
                    r.ebp = m.u32(lostk.wrapping_add(r.eax.wrapping_mul(4)))?;
                    m.set_u32(hi_var, r.edx)?;
                    r.ecx = r.edx;
                    pc = 0x0013_dab9;
                }
                0x0013_db0d => {
                    let flag = m.u8(aligned)?;
                    r.ecx = lo8(r.ecx, flag);
                    r.eax = (r.eax >> 1).wrapping_mul(r.ebx).wrapping_add(r.ebp);
                    if flag != 0 {
                        r.edx = r.ebx >> 2;
                        r.ecx = r.ebp;
                        loop {
                            r.esi = m.u32(r.ecx)?;
                            r.edi = m.u32(r.eax)?;
                            m.set_u32(r.eax, r.esi)?;
                            r.eax = r.eax.wrapping_add(4);
                            m.set_u32(r.ecx, r.edi)?;
                            r.ecx = r.ecx.wrapping_add(4);
                            r.edx = r.edx.wrapping_sub(1);
                            if (r.edx as i32) <= 0 {
                                break;
                            }
                        }
                    } else {
                        r.esi = r.ebx;
                        r.edi = r.ebp;
                        loop {
                            r.edx = lo8(r.edx, m.u8(r.edi)?);
                            r.ecx = lo8(r.ecx, m.u8(r.eax)?);
                            m.set_u8(r.eax, r.edx as u8)?;
                            r.eax = r.eax.wrapping_add(1);
                            m.set_u8(r.edi, r.ecx as u8)?;
                            r.edi = r.edi.wrapping_add(1);
                            r.esi = r.esi.wrapping_sub(1);
                            if (r.esi as i32) <= 0 {
                                break;
                            }
                        }
                    }
                    // 0x0013db4f
                    r.eax = m.u32(hi_var)?;
                    r.edi = r.ebp;
                    r.esi = r.eax.wrapping_add(r.ebx);
                    pc = 0x0013_db58;
                }
                0x0013_db58 => {
                    r.eax = m.u32(hi_var)?;
                    r.edi = r.edi.wrapping_add(r.ebx);
                    if r.edi > r.eax {
                        pc = 0x0013_db77;
                        continue;
                    }
                    r.ecx = m.u32(ctx_arg)?;
                    Self::push(r, m, r.ecx)?;
                    Self::push(r, m, r.ebp)?;
                    Self::push(r, m, r.edi)?;
                    let target = m.u32(cmp_arg)?;
                    if let Some(step) = self.compare(r, m, target, RET_LO)? {
                        return Ok(step);
                    }
                    pc = RET_LO;
                }
                RET_LO => {
                    pc = if r.eax & 0xff == 0 {
                        0x0013_db58
                    } else {
                        0x0013_db77
                    };
                }
                0x0013_db77 => {
                    m.set_u32(scan_var, r.edi)?;
                    pc = 0x0013_db80;
                }
                0x0013_db80 => {
                    r.esi = r.esi.wrapping_sub(r.ebx);
                    if r.esi <= r.ebp {
                        pc = 0x0013_db9b;
                        continue;
                    }
                    r.edx = m.u32(ctx_arg)?;
                    Self::push(r, m, r.edx)?;
                    Self::push(r, m, r.esi)?;
                    Self::push(r, m, r.ebp)?;
                    let target = m.u32(cmp_arg)?;
                    if let Some(step) = self.compare(r, m, target, RET_HI)? {
                        return Ok(step);
                    }
                    pc = RET_HI;
                }
                RET_HI => {
                    pc = if r.eax & 0xff == 0 {
                        0x0013_db80
                    } else {
                        0x0013_db9b
                    };
                }
                0x0013_db9b => {
                    let flag = m.u8(aligned)?;
                    r.eax = lo8(r.eax, flag);
                    r.ecx = r.esi;
                    if r.esi < r.edi {
                        pc = 0x0013_dbf2;
                        continue;
                    }
                    if flag != 0 {
                        r.edx = r.ebx >> 2;
                        r.eax = r.edi;
                        loop {
                            r.ebp = m.u32(r.ecx)?;
                            r.edi = m.u32(r.eax)?;
                            m.set_u32(r.eax, r.ebp)?;
                            r.eax = r.eax.wrapping_add(4);
                            m.set_u32(r.ecx, r.edi)?;
                            r.ecx = r.ecx.wrapping_add(4);
                            r.edx = r.edx.wrapping_sub(1);
                            if (r.edx as i32) <= 0 {
                                break;
                            }
                        }
                        r.ebp = m.u32(lo_var)?;
                        r.edi = m.u32(scan_var)?;
                    } else {
                        r.eax = m.u32(scan_var)?;
                        r.edi = r.ebx;
                        loop {
                            r.ebx = lo8(r.ebx, m.u8(r.ecx)?);
                            r.edx = lo8(r.edx, m.u8(r.eax)?);
                            m.set_u8(r.eax, r.ebx as u8)?;
                            r.eax = r.eax.wrapping_add(1);
                            m.set_u8(r.ecx, r.edx as u8)?;
                            r.ecx = r.ecx.wrapping_add(1);
                            r.edi = r.edi.wrapping_sub(1);
                            if (r.edi as i32) <= 0 {
                                break;
                            }
                        }
                        r.ebx = m.u32(size_arg)?;
                        r.edi = m.u32(scan_var)?;
                    }
                    pc = 0x0013_db58;
                }
                0x0013_dbf2 => {
                    let flag = r.eax as u8;
                    r.eax = r.ebp;
                    if flag != 0 {
                        r.edx = r.ebx >> 2;
                        loop {
                            r.edi = m.u32(r.eax)?;
                            m.set_u32(tmp_dword, r.edi)?;
                            r.edi = m.u32(r.ecx)?;
                            m.set_u32(r.eax, r.edi)?;
                            r.edi = m.u32(tmp_dword)?;
                            r.eax = r.eax.wrapping_add(4);
                            m.set_u32(r.ecx, r.edi)?;
                            r.ecx = r.ecx.wrapping_add(4);
                            r.edx = r.edx.wrapping_sub(1);
                            if (r.edx as i32) <= 0 {
                                break;
                            }
                        }
                        r.edi = m.u32(scan_var)?;
                    } else {
                        m.set_u32(scan_var, r.ebx)?;
                        loop {
                            r.edx = lo8(r.edx, m.u8(r.eax)?);
                            m.set_u8(tmp_byte, r.edx as u8)?;
                            r.edx = lo8(r.edx, m.u8(r.ecx)?);
                            m.set_u8(r.eax, r.edx as u8)?;
                            r.edx = lo8(r.edx, m.u8(tmp_byte)?);
                            m.set_u8(r.ecx, r.edx as u8)?;
                            r.edx = m.u32(scan_var)?;
                            r.eax = r.eax.wrapping_add(1);
                            r.ecx = r.ecx.wrapping_add(1);
                            r.edx = r.edx.wrapping_sub(1);
                            m.set_u32(scan_var, r.edx)?;
                            if (r.edx as i32) <= 0 {
                                break;
                            }
                        }
                    }
                    pc = 0x0013_dc44;
                }
                0x0013_dc44 => {
                    r.ecx = m.u32(hi_var)?;
                    r.eax = r.esi.wrapping_sub(r.ebp).wrapping_sub(1);
                    r.edx = r.ecx.wrapping_sub(r.edi);
                    if (r.eax as i32) < (r.edx as i32) {
                        // 0x0013dc85
                        if r.edi < r.ecx {
                            r.eax = m.u32(depth)?;
                            m.set_u32(lostk.wrapping_add(r.eax.wrapping_mul(4)), r.edi)?;
                            m.set_u32(histk.wrapping_add(r.eax.wrapping_mul(4)), r.ecx)?;
                            r.eax = r.eax.wrapping_add(1);
                            m.set_u32(depth, r.eax)?;
                        }
                        r.ecx = r.ebx.wrapping_add(r.ebp);
                        if r.ecx >= r.esi {
                            pc = RET_SHORT_SORT;
                            continue;
                        }
                        r.esi = r.esi.wrapping_sub(r.ebx);
                        m.set_u32(hi_var, r.esi)?;
                        r.ecx = r.esi;
                        pc = 0x0013_dac0;
                        continue;
                    }
                    r.eax = r.ebx.wrapping_add(r.ebp);
                    if r.eax < r.esi {
                        r.eax = m.u32(depth)?;
                        r.esi = r.esi.wrapping_sub(r.ebx);
                        m.set_u32(lostk.wrapping_add(r.eax.wrapping_mul(4)), r.ebp)?;
                        m.set_u32(histk.wrapping_add(r.eax.wrapping_mul(4)), r.esi)?;
                        r.eax = r.eax.wrapping_add(1);
                        m.set_u32(depth, r.eax)?;
                    }
                    if r.edi >= r.ecx {
                        pc = RET_SHORT_SORT;
                        continue;
                    }
                    r.ecx = m.u32(hi_var)?;
                    r.ebp = r.edi;
                    pc = 0x0013_dab9;
                }
                0x0013_dcb5 => {
                    r.edi = Self::pop(r, m)?;
                    r.esi = Self::pop(r, m)?;
                    r.ebp = Self::pop(r, m)?;
                    pc = 0x0013_dcb8;
                }
                0x0013_dcb8 => {
                    r.ebx = Self::pop(r, m)?;
                    r.esp = e.wrapping_add(4 + RET_CLEANUP);
                    return Ok(Step::Return { to: self.ret_to });
                }
                _ => unreachable!("qsort port: no block at {pc:#x}"),
            }
        }
    }
}
