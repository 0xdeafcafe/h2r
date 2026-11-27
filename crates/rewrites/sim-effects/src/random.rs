//! The shared random stream's two draws, `0x259a0` and `0x259d0`, run on the
//! guest's x87 state with the recompiler's own x87 helpers
//! (`x87::abi`), so the result, the status word, the tags and the freed
//! registers end as retail leaves them.
//!
//! The stream is a pointer at `[0x4e7408]`: word 0 feeds the object arm's
//! leaves, word `+4` the decal and `coln` arms. One draw:
//!
//! ```text
//! seed = seed * 0x19660d + 0x3c6ef35f      (stored back)
//! eax  = seed >> 16                        (0..0xffff, never negative)
//! st0  = fild eax * [0x45dc50]             (0x259a0 returns here)
//! st0  = (hi - lo) * st0 + lo              (0x259d0, `lo`, `hi` on the stack)
//! ```

use x87::abi::{self, ARITH_POP, X87Abi, arith_op, src_kind, status};

/// The stream's pointer.
pub const STREAM: u32 = 0x004e_7408;
/// The draw's scale, an `f32` (`fmul [0x45dc50]`).
pub const SCALE: u32 = 0x0045_dc50;

/// The seed after one draw.
pub const fn next(seed: u32) -> u32 {
    seed.wrapping_mul(0x0019_660d).wrapping_add(0x3c6e_f35f)
}

/// An x87 step would fault or stop (an unmasked exception, or one already
/// pending): the guest answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

fn ok(code: u32) -> Result<u32, Refused> {
    if code & status::MASK == status::OK { Ok(code) } else { Err(Refused) }
}

/// `0x259a0`: one draw, its value left in `ST0`. Returns the new seed.
pub fn unit(s: &mut X87Abi, seed: u32, scale: u32) -> Result<u32, Refused> {
    let seed = next(seed);
    ok(abi::ld_int(s, i64::from(seed >> 16)))?;
    ok(abi::arith(s, arith_op::MUL, 0, src_kind::F32, u64::from(scale)))?;
    Ok(seed)
}

/// `0x259d0(lo, hi)`: one draw scaled to `[lo, hi)`, left in `ST0`.
pub fn range(s: &mut X87Abi, seed: u32, scale: u32, lo: u32, hi: u32) -> Result<u32, Refused> {
    let seed = unit(s, seed, scale)?;
    ok(abi::ld_f32(s, hi))?;
    ok(abi::arith(s, arith_op::SUB, 0, src_kind::F32, u64::from(lo)))?;
    ok(abi::arith(s, arith_op::MUL | ARITH_POP, 1, src_kind::ST, 0))?;
    ok(abi::arith(s, arith_op::ADD, 0, src_kind::F32, u64::from(lo)))?;
    Ok(seed)
}

/// `fstp st(0)`: drop the draw.
pub fn drop_top(s: &mut X87Abi) -> Result<(), Refused> {
    ok(abi::st_st(s, 0, true)).map(|_| ())
}

/// `fstp dword`: the draw as an `f32`, popped.
pub fn store_f32(s: &mut X87Abi) -> Result<u32, Refused> {
    let (code, store) = abi::st_f32(s, 0);
    ok(code)?;
    abi::store_commit(s, store.status, true);
    Ok(store.value)
}

/// `fld [limit]; fcomip st, st(1); fstp st(0)` after [`unit`]: whether the
/// arm goes on (`jbe` not taken: `limit` above the draw, ordered).
pub fn below(s: &mut X87Abi, limit: u32) -> Result<bool, Refused> {
    ok(abi::ld_f32(s, limit))?;
    let flags = abi::cmp(s, 0, src_kind::ST, 1, 1, true);
    ok(flags)?;
    drop_top(s)?;
    Ok(flags & (abi::EFL_CF | abi::EFL_ZF) == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALE_BITS: u32 = 0x3780_0000; // 2^-16

    fn fpu() -> X87Abi {
        X87Abi { control: 0x027f, ..X87Abi::default() }
    }

    /// The draw worked by hand: the LCG, then `(seed >> 16) / 65536`
    /// scaled to `[lo, hi)`, the stack balanced after the pop.
    #[test]
    fn a_draw_is_retails_lcg_and_scale() {
        assert_eq!(next(0), 0x3c6e_f35f);
        assert_eq!(next(1), 0x3c6e_f35f + 0x0019_660d);
        let mut s = fpu();
        let seed = range(&mut s, 0, SCALE_BITS, 2.0f32.to_bits(), 4.0f32.to_bits()).unwrap();
        assert_eq!(seed, 0x3c6e_f35f);
        let v = f32::from_bits(store_f32(&mut s).unwrap());
        assert_eq!(v, 2.0 + 2.0 * (0x3c6e as f32 / 65536.0));
        assert_eq!(s.status & abi::SW_TOP, fpu().status & abi::SW_TOP, "balanced");
        assert_eq!(s.tag, fpu().tag);
    }

    #[test]
    fn the_gate_goes_on_only_below_the_limit() {
        // The first draw from 0 is 0x3c6e / 65536, about 0.236.
        for (limit, on) in [(0.5f32, true), (0.2, false), (f32::NAN, false)] {
            let mut s = fpu();
            unit(&mut s, 0, SCALE_BITS).unwrap();
            assert_eq!(below(&mut s, limit.to_bits()), Ok(on), "{limit}");
            assert_eq!(s.status & abi::SW_TOP, 0);
        }
    }

    /// An exception pending from earlier code stops the first waiting
    /// instruction: the guest answers.
    #[test]
    fn a_pending_exception_is_refused() {
        let mut s = X87Abi { status: abi::SW_ES | abi::H2_FP_IE, ..fpu() };
        assert_eq!(unit(&mut s, 0, SCALE_BITS), Err(Refused));
    }
}
