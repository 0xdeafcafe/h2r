//! Checked, side-effect-free-until-commit collision kernels.
use super::host;
use std::cell::Cell;

/// Guest MXCSR for a region that accepts the full normal binary32 range.
/// The older range-proved kernels can use [`super::fast_env`] even with
/// flush-to-zero or denormals-are-zero enabled.
pub fn checked_env(mxcsr: u32) -> bool {
    super::fast_env(mxcsr) && mxcsr & 0x8040 == 0
}

/// [`checked_env`] with PE sticky or not, as [`super::guest_env`]: the
/// live game's `0x1f80`. [`Checked::sse_inexact`] says whether the
/// region's operations would have set it.
pub fn checked_guest_env(mxcsr: u32) -> bool {
    checked_env(mxcsr | 0x20)
}

/// The host control check for range-proved [`super::Fp`] operations:
/// masked exceptions and nearest rounding. Their operand bounds rule out
/// anything the host's flush controls could change.
pub fn host_fast_env() -> bool {
    host::fast_env()
}

/// Accept signed zeros and finite, normal binary32 guest operands. A
/// subnormal source can set the guest denormal-operand flag even when its
/// arithmetic result is normal. Inspect bits without touching host status.
pub fn checked_inputs(values: &[u32]) -> bool {
    values.iter().all(|&value| {
        let bits = value & 0x7fff_ffff;
        bits == 0 || (0x0080_0000..0x7f80_0000).contains(&bits)
    })
}

/// The scalar operations whose floating-point exceptions [`checked`]
/// samples. Each operation runs exactly once in the host's working format.
/// The reference is confined to the thread and lifetime of its region.
pub struct Checked {
    ordinary_results: Cell<bool>,
    /// An SSE operation ([`Checked::add`] to [`Checked::sqrt`]) was
    /// inexact.
    sse_inexact: Cell<bool>,
}

impl Checked {
    /// Whether an SSE operation so far was inexact: the guest's `addss`,
    /// `subss`, `mulss`, `divss` or `sqrtss` set PE. The region's x87
    /// chains do not count.
    pub fn sse_inexact(&self) -> bool {
        self.sse_inexact.get()
    }

    /// Run one SSE operation, sampling the host's inexact flag until one
    /// is inexact. Nothing else reads the host's inexact flag inside a
    /// region, so clearing it first loses nothing.
    fn sse(&self, op: impl FnOnce() -> f32) -> f32 {
        if self.sse_inexact.get() {
            return op();
        }
        host::write(host::read() & !host::INEXACT);
        let value = op();
        if host::read() & host::INEXACT != 0 {
            self.sse_inexact.set(true);
        }
        value
    }

    fn record32(&self, value: f32) -> f32 {
        let bits = value.to_bits() & 0x7fff_ffff;
        // Hosts can detect tininess before or after rounding. Decline the
        // normal/subnormal boundary on both, including a rounded MIN_POSITIVE.
        // Exact zeros are allowed. An inexact rounded zero sets underflow.
        if bits != 0 && !(0x0080_0001..0x7f80_0000).contains(&bits) {
            self.decline();
        }
        value
    }

    pub(super) fn record64(&self, value: f64) -> f64 {
        let bits = value.to_bits() & 0x7fff_ffff_ffff_ffff;
        if bits != 0 && !(0x0010_0000_0000_0001..0x7ff0_0000_0000_0000).contains(&bits) {
            self.decline();
        }
        value
    }

    pub(super) fn decline(&self) {
        self.ordinary_results.set(false);
    }

    fn operands(&self, a: f32, b: f32) {
        if !checked_inputs(&[a.to_bits(), b.to_bits()]) {
            self.decline();
        }
    }

    pub fn add(&self, a: f32, b: f32) -> f32 {
        self.operands(a, b);
        self.record32(self.sse(|| host::add32(a, b)))
    }
    pub fn sub(&self, a: f32, b: f32) -> f32 {
        self.operands(a, b);
        self.record32(self.sse(|| host::sub32(a, b)))
    }
    pub fn mul(&self, a: f32, b: f32) -> f32 {
        self.operands(a, b);
        self.record32(self.sse(|| host::mul32(a, b)))
    }
    pub fn div(&self, a: f32, b: f32) -> f32 {
        self.operands(a, b);
        self.record32(self.sse(|| host::div32(a, b)))
    }

    pub fn sqrt(&self, value: f32) -> f32 {
        if !checked_inputs(&[value.to_bits()]) {
            self.decline();
        }
        self.record32(self.sse(|| host::sqrt32(value)))
    }

    pub(super) fn narrow(&self, value: f64) -> f32 {
        self.record32(host::narrow(value))
    }
}

/// Run one gather/compute kernel under a fresh host status word. The caller
/// checks [`checked_env`] and [`checked_inputs`] on guest inputs before entry, uses
/// the supplied operations (or [`super::Chain`]) for arithmetic, and
/// scatters only after `Some`. Ordinary Rust arithmetic is unsuitable for
/// tests that depend on exception flags: the compiler can fold it away.
///
/// The region declines on unsupported hosts or host controls, a dangerous
/// arithmetic flag, or a non-ordinary result. Each invocation restores the
/// thread's incoming status, including on unwinding. Precision/inexact by
/// itself is accepted. A kernel whose guest contract observes sticky flags
/// must also model those guest writes. Nested regions decline before the
/// closure runs, so a captured outer value always uses its own flag scope.
pub fn checked<T>(kernel: impl FnOnce(&Checked) -> Option<T>) -> Option<T> {
    thread_local! {
        static ACTIVE: Cell<bool> = const { Cell::new(false) };
    }
    struct Restore<'a>(host::Word, &'a Cell<bool>);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            host::write(self.0);
            self.1.set(false);
        }
    }

    ACTIVE.with(|active| {
        if active.get() {
            return None;
        }
        let restore = Restore(host::start()?, active);
        active.set(true);
        let scope = Checked {
            ordinary_results: Cell::new(true),
            sse_inexact: Cell::new(false),
        };
        let result = std::hint::black_box(kernel(&scope));
        let ok = host::read() & host::REJECT == 0 && scope.ordinary_results.get();
        drop(restore);
        if ok { result } else { None }
    })
}

#[cfg(test)]
mod tests;
