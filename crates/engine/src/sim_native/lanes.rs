//! Four binary32 lanes on the host's vector unit, for the packed SSE
//! operations whose operands all pass the fast-path range check.
//!
//! Exactness. The guest's `addps`, `subps` and `mulps` round each lane
//! once, to nearest, independently of the other lanes. In the fast path
//! MXCSR rounds to nearest with PE already sticky, and every operand
//! magnitude is zero or in `[2^-63, 2^63)`. A sum, difference or product of
//! two such values is zero or normal (at least `2^-86` and below `2^127`),
//! with no overflow, underflow, NaN or infinity, so MXCSR cannot change and
//! neither FZ nor DAZ has anything to flush. A single correctly
//! rounded IEEE binary32 add, subtract or multiply per lane on the host
//! gives the same bits. NEON `fadd/fsub/fmul .4s` on AArch64 and SSE
//! `addps/subps/mulps` on x86-64 are those operations: no fused multiply,
//! no reassociation, with the host's default round-to-nearest. Signed zero
//! follows IEEE on both hosts. Every intermediate the host could flush is
//! outside the fast path, so a flushing host mode would not change results.
//! The scalar fallback performs the same per-lane operation.
//!
//! [`enabled`] selects these lanes or the scalar per-lane loop at run time
//! (`impl.sim.native_simd`). Both give the same bits; the switch exists for
//! A/B timing.
use super::{Op, Vec4};
use std::sync::atomic::{AtomicBool, Ordering};

static SIMD: AtomicBool = AtomicBool::new(true);

/// Use host vector lanes for the fast path (the default) or the scalar
/// per-lane loop.
pub fn set_enabled(on: bool) {
    SIMD.store(on, Ordering::Relaxed);
}

#[cfg(any(test, feature = "differential"))]
thread_local! {
    static FORCE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Run `f` with the lanes forced on or off on this thread only, so
/// parallel tests can compare both paths without racing on the global.
#[cfg(any(test, feature = "differential"))]
pub fn with<R>(on: bool, f: impl FnOnce() -> R) -> R {
    let previous = FORCE.with(|cell| cell.replace(Some(on)));
    let result = f();
    FORCE.with(|cell| cell.set(previous));
    result
}

#[cfg(any(test, feature = "differential"))]
fn forced() -> Option<bool> {
    FORCE.with(std::cell::Cell::get)
}

#[cfg(not(any(test, feature = "differential")))]
#[inline(always)]
fn forced() -> Option<bool> {
    None
}

#[inline]
pub fn enabled() -> bool {
    forced().unwrap_or_else(|| SIMD.load(Ordering::Relaxed))
}

/// Magnitudes accepted by the fast path: zero or `[LOW, HIGH)`. These
/// match [`super::ordinary`].
const LOW: u32 = 0x2000_0000;
const HIGH: u32 = 0x5f00_0000;

#[cfg(target_arch = "aarch64")]
mod host {
    use super::*;
    use std::arch::aarch64::*;

    #[inline]
    pub fn ordinary(a: Vec4, b: Vec4) -> bool {
        // SAFETY: NEON is part of the AArch64 baseline. The loads read
        // four u32 from arrays of four.
        unsafe {
            let abs = vdupq_n_u32(0x7fff_ffff);
            let low = vdupq_n_u32(LOW);
            let span = vdupq_n_u32(HIGH - LOW);
            let a = vandq_u32(vld1q_u32(a.as_ptr()), abs);
            let b = vandq_u32(vld1q_u32(b.as_ptr()), abs);
            // m == 0, or m - LOW < HIGH - LOW unsigned.
            let a_ok = vorrq_u32(vceqzq_u32(a), vcltq_u32(vsubq_u32(a, low), span));
            let b_ok = vorrq_u32(vceqzq_u32(b), vcltq_u32(vsubq_u32(b, low), span));
            vminvq_u32(vandq_u32(a_ok, b_ok)) == u32::MAX
        }
    }

    #[inline]
    pub fn binary32(op: Op, a: Vec4, b: Vec4) -> Vec4 {
        let mut out = [0; 4];
        // SAFETY: as above; the store writes four u32 to an array of four.
        unsafe {
            let x = vreinterpretq_f32_u32(vld1q_u32(a.as_ptr()));
            let y = vreinterpretq_f32_u32(vld1q_u32(b.as_ptr()));
            let r = match op {
                Op::Add => vaddq_f32(x, y),
                Op::Sub => vsubq_f32(x, y),
                Op::Mul => vmulq_f32(x, y),
            };
            vst1q_u32(out.as_mut_ptr(), vreinterpretq_u32_f32(r));
        }
        out
    }
}

#[cfg(target_arch = "x86_64")]
mod host {
    use super::*;
    use std::arch::x86_64::*;

    #[inline]
    pub fn ordinary(a: Vec4, b: Vec4) -> bool {
        // SAFETY: SSE2 is part of the x86-64 baseline. The unaligned loads
        // read 16 bytes from arrays of four u32.
        unsafe {
            let abs = _mm_set1_epi32(0x7fff_ffff);
            let below = _mm_set1_epi32(LOW as i32 - 1);
            let high = _mm_set1_epi32(HIGH as i32);
            let zero = _mm_setzero_si128();
            let a = _mm_and_si128(_mm_loadu_si128(a.as_ptr().cast()), abs);
            let b = _mm_and_si128(_mm_loadu_si128(b.as_ptr().cast()), abs);
            // Masked magnitudes are non-negative, so signed compares
            // order them correctly.
            let a_ok = _mm_or_si128(
                _mm_cmpeq_epi32(a, zero),
                _mm_and_si128(_mm_cmpgt_epi32(a, below), _mm_cmplt_epi32(a, high)),
            );
            let b_ok = _mm_or_si128(
                _mm_cmpeq_epi32(b, zero),
                _mm_and_si128(_mm_cmpgt_epi32(b, below), _mm_cmplt_epi32(b, high)),
            );
            _mm_movemask_epi8(_mm_and_si128(a_ok, b_ok)) == 0xffff
        }
    }

    #[inline]
    pub fn binary32(op: Op, a: Vec4, b: Vec4) -> Vec4 {
        let mut out = [0; 4];
        // SAFETY: as above; the store writes 16 bytes to an array of four.
        unsafe {
            let x = _mm_castsi128_ps(_mm_loadu_si128(a.as_ptr().cast()));
            let y = _mm_castsi128_ps(_mm_loadu_si128(b.as_ptr().cast()));
            let r = match op {
                Op::Add => _mm_add_ps(x, y),
                Op::Sub => _mm_sub_ps(x, y),
                Op::Mul => _mm_mul_ps(x, y),
            };
            _mm_storeu_si128(out.as_mut_ptr().cast(), _mm_castps_si128(r));
        }
        out
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod host {
    pub use super::scalar::{binary32, ordinary};
}

/// The reference: one host scalar operation per lane.
pub mod scalar {
    use super::{Op, Vec4};

    pub fn ordinary(a: Vec4, b: Vec4) -> bool {
        a.into_iter().chain(b).all(super::super::ordinary)
    }

    pub fn binary32(op: Op, a: Vec4, b: Vec4) -> Vec4 {
        std::array::from_fn(|i| super::super::binary32(op, a[i], b[i]))
    }
}

pub use host::{binary32, ordinary};

#[cfg(test)]
mod tests;
