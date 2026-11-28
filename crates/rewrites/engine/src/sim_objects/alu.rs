//! Integer flag idioms the slice 11 bodies reproduce at their transfers:
//! shifts, 16-bit compares and tests, and byte read-modify-writes
//! (docs/engine/sim-rewrite-objects.md section 5, rule 3). The 32-bit
//! `add`, `sub`, `cmp`, `and`, `or`, `xor` and `test` forms are
//! `sim_physics::{arithmetic, logic}`.

use super::Result;
use crate::sim_physics::{ARITH, logic, status};
use runtime::{CpuAbi, Guest};

const AF: u32 = 0x10;
const OF: u32 = 0x800;

fn mask(bits: u32) -> u32 {
    if bits >= 32 { u32::MAX } else { (1 << bits) - 1 }
}

/// `shr r/m, n` on a `bits`-wide operand (`n` already a constant below
/// `bits`): CF the last bit out, OF the operand's top bit when `n` is 1
/// and undefined otherwise, AF undefined. Returns the result.
pub(crate) fn shr(c: &mut CpuAbi, v: u32, n: u32, bits: u32) -> u32 {
    let v = v & mask(bits);
    let n = n & 31;
    if n == 0 {
        return v;
    }
    let r = v >> n;
    let sign = 1 << (bits - 1);
    let cf = (v >> (n - 1)) & 1;
    let of = if n == 1 { u32::from(v & sign != 0) << 11 } else { 0 };
    c.eflags = (c.eflags & !ARITH) | status(r, sign) | cf | of;
    let undefined = if n == 1 { AF } else { AF | OF };
    c.eflags_defined = (c.eflags_defined | ARITH) & !undefined;
    r
}

/// `shl r/m, n` on a `bits`-wide operand: CF the last bit out, OF (for
/// `n` 1) the result's top bit xor CF, AF undefined.
pub(crate) fn shl(c: &mut CpuAbi, v: u32, n: u32, bits: u32) -> u32 {
    let v = v & mask(bits);
    let n = n & 31;
    if n == 0 {
        return v;
    }
    let r = (v << n) & mask(bits);
    let sign = 1 << (bits - 1);
    let cf = (v >> (bits - n)) & 1;
    let of = if n == 1 { (u32::from(r & sign != 0) ^ cf) << 11 } else { 0 };
    c.eflags = (c.eflags & !ARITH) | status(r, sign) | cf | of;
    let undefined = if n == 1 { AF } else { AF | OF };
    c.eflags_defined = (c.eflags_defined | ARITH) & !undefined;
    r
}

/// `cmp a16, b16`.
pub(crate) fn cmp16(c: &mut CpuAbi, a: u32, b: u32) {
    let (a, b) = (a & 0xffff, b & 0xffff);
    let v = a.wrapping_sub(b) & 0xffff;
    let cf = u32::from(a < b);
    let of = u32::from((a ^ b) & (a ^ v) & 0x8000 != 0) << 11;
    c.eflags = (c.eflags & !ARITH) | status(v, 0x8000) | cf | ((a ^ b ^ v) & AF) | of;
    c.eflags_defined |= ARITH;
}

/// `test r16, r16`.
pub(crate) fn test16(c: &mut CpuAbi, v: u32) {
    logic(c, v & 0xffff, 0x8000);
}

/// `jle` after a `test` or `cmp`: ZF, or SF not equal to OF.
pub(crate) fn less_or_equal(c: &CpuAbi) -> bool {
    c.eflags & 0x40 != 0 || (c.eflags & 0x80 != 0) != (c.eflags & OF != 0)
}

/// `and`/`or byte ptr [addr], k`: the byte store, with the flags of the
/// operation.
pub(crate) fn rmw8(c: &mut CpuAbi, g: &mut Guest, addr: u32, f: impl FnOnce(u8) -> u8) -> Result<u8> {
    let v = f(g.u8(addr)?);
    g.set_u8(addr, v)?;
    logic(c, v.into(), 0x80);
    Ok(v)
}

/// `test byte ptr [addr], k`.
pub(crate) fn test8(c: &mut CpuAbi, g: &Guest, addr: u32, k: u8) -> Result<bool> {
    let v = g.u8(addr)? & k;
    logic(c, v.into(), 0x80);
    Ok(v != 0)
}

/// `dec r32`: CF kept.
pub(crate) fn dec32(c: &mut CpuAbi, v: u32) -> u32 {
    let r = v.wrapping_sub(1);
    let af = u32::from(v & 15 == 0) << 4;
    let of = u32::from(v == 0x8000_0000) << 11;
    let keep = ARITH & !1;
    c.eflags = (c.eflags & !keep) | status(r, 1 << 31) | af | of;
    c.eflags_defined |= keep;
    r
}

/// `inc r32`: CF kept.
pub(crate) fn inc32(c: &mut CpuAbi, v: u32) -> u32 {
    let r = v.wrapping_add(1);
    let af = u32::from(v & 15 == 15) << 4;
    let of = u32::from(v == 0x7fff_ffff) << 11;
    let keep = ARITH & !1;
    c.eflags = (c.eflags & !keep) | status(r, 1 << 31) | af | of;
    c.eflags_defined |= keep;
    r
}

/// `cmp r8, imm8` / `cmp r8, r8`.
pub(crate) fn cmp8(c: &mut CpuAbi, a: u32, b: u32) {
    let (a, b) = (a & 0xff, b & 0xff);
    let v = a.wrapping_sub(b) & 0xff;
    let cf = u32::from(a < b);
    let of = u32::from((a ^ b) & (a ^ v) & 0x80 != 0) << 11;
    c.eflags = (c.eflags & !ARITH) | status(v, 0x80) | cf | ((a ^ b ^ v) & AF) | of;
    c.eflags_defined |= ARITH;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_set_cf_from_the_last_bit_out() {
        let mut c = CpuAbi::default();
        assert_eq!(shr(&mut c, 0x1800, 12, 32), 1);
        assert_eq!(c.eflags & 1, 1);
        assert_eq!(c.eflags_defined & (AF | OF), 0);
        assert_eq!(shr(&mut c, 0x81, 1, 8), 0x40);
        assert_eq!(c.eflags & (1 | OF), 1 | OF);
        assert_eq!(shl(&mut c, 0x8000_0001, 4, 32), 0x10);
        assert_eq!(c.eflags & 1, 0);
        assert_eq!(shl(&mut c, 0x1000_0000, 4, 32), 0);
        assert_eq!(c.eflags & 0x41, 0x41);
    }

    #[test]
    fn a_16_bit_compare_sees_only_the_low_word() {
        let mut c = CpuAbi::default();
        cmp16(&mut c, 0x1234_ffff, 0xffff);
        assert_eq!(c.eflags & ARITH, 0x44);
        cmp16(&mut c, 1, 2);
        assert_eq!(c.eflags & 0x81, 0x81);
        test16(&mut c, 0x0001_0000);
        assert!(less_or_equal(&c));
        test16(&mut c, 0x8000);
        assert!(less_or_equal(&c));
        test16(&mut c, 1);
        assert!(!less_or_equal(&c));
    }

    #[test]
    fn dec_keeps_carry() {
        let mut c = CpuAbi { eflags: 1, ..Default::default() };
        assert_eq!(dec32(&mut c, 1), 0);
        assert_eq!(c.eflags & 0x41, 0x41);
    }
}
