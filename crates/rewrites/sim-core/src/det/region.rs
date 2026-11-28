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

/// A result the checked path keeps. Hosts can detect tininess before or
/// after rounding, so the normal/subnormal boundary is out on both,
/// including a rounded `MIN_POSITIVE`. Exact zeros are kept; an inexact
/// rounded zero sets underflow, which the region's status check sees.
fn ordinary32(value: f32) -> bool {
    let bits = value.to_bits() & 0x7fff_ffff;
    bits == 0 || (0x0080_0001..0x7f80_0000).contains(&bits)
}

/// The scalar operations whose floating-point exceptions [`checked`]
/// samples. Each operation runs exactly once in the host's working format.
/// The reference is confined to the thread and lifetime of its region.
pub struct Checked {
    ordinary_results: Cell<bool>,
    /// An SSE operation ([`Checked::add`] to [`Checked::sqrt`]) was
    /// inexact.
    sse_inexact: Cell<bool>,
    /// An x87 operation of a [`super::Chain`] was inexact.
    x87_inexact: Cell<bool>,
    /// [`checked_guest`]: the guest's state, under which operations off
    /// the checked path run instead of declining.
    guest: Option<Guest>,
}

struct Guest {
    /// The guest's MXCSR after the region's modelled SSE operations.
    mxcsr: Cell<u32>,
    /// The region's x87, for every [`super::Chain`] operation.
    x87: super::X87,
}

impl Checked {
    /// Whether an SSE operation so far was inexact: the guest's `addss`,
    /// `subss`, `mulss`, `divss` or `sqrtss` set PE. The region's x87
    /// chains do not count.
    pub fn sse_inexact(&self) -> bool {
        self.sse_inexact.get()
    }

    /// Whether an x87 operation of a [`super::Chain`] so far was inexact:
    /// an `fadd`, `fsub`, `fmul`, `fdiv` or `fsqrt` rounded in the working
    /// precision, an `fst m32` narrowing or an `fistp` dropping a fraction
    /// set the x87 status word's PE (bit 5). SSE operations do not count.
    pub fn x87_inexact(&self) -> bool {
        self.x87_inexact.get() || self.guest.as_ref().is_some_and(|g| g.x87.flags() & 0x20 != 0)
    }

    /// [`checked_guest`]: the MXCSR after the region's SSE operations,
    /// with every flag they raised (PE included).
    pub fn mxcsr(&self) -> Option<u32> {
        let g = self.guest.as_ref()?;
        Some(g.mxcsr.get() | if self.sse_inexact.get() { 0x20 } else { 0 })
    }

    /// [`checked_guest`]: the x87 exception flags (`0x3f`) the region's
    /// chains raised, PE included. Zero outside guest mode.
    pub fn x87_flags(&self) -> u16 {
        self.guest.as_ref().map_or(0, |g| g.x87.flags())
            | if self.x87_inexact.get() { 0x20 } else { 0 }
    }

    /// [`checked_guest`]: the region's x87.
    pub(super) fn x87(&self) -> Option<&super::X87> {
        self.guest.as_ref().map(|g| &g.x87)
    }

    /// Run one operation, sampling the host's inexact flag into `flag`
    /// until one is inexact. Nothing else reads the host's inexact flag
    /// inside a region, so clearing it first loses nothing.
    fn sample<T>(flag: &Cell<bool>, op: impl FnOnce() -> T) -> T {
        if flag.get() {
            return op();
        }
        host::write(host::read() & !host::INEXACT);
        let value = op();
        if host::read() & host::INEXACT != 0 {
            flag.set(true);
        }
        value
    }

    fn sse(&self, op: impl FnOnce() -> f32) -> f32 {
        Self::sample(&self.sse_inexact, op)
    }

    /// One x87 operation in single working precision.
    pub(super) fn x87_32(&self, operands: &[f32], op: impl FnOnce() -> f32) -> f32 {
        if !operands.iter().all(|v| checked_inputs(&[v.to_bits()])) {
            self.decline();
        }
        self.record32(Self::sample(&self.x87_inexact, op))
    }

    /// One x87 operation in double working precision.
    pub(super) fn x87_64(&self, op: impl FnOnce() -> f64) -> f64 {
        self.record64(Self::sample(&self.x87_inexact, op))
    }

    /// An `fistp` that dropped a fraction.
    pub(super) fn x87_rounded(&self) {
        self.x87_inexact.set(true);
    }

    fn record32(&self, value: f32) -> f32 {
        if !ordinary32(value) {
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
        self.scalar(a, b, || host::add32(a, b), simd::addss)
    }
    pub fn sub(&self, a: f32, b: f32) -> f32 {
        self.scalar(a, b, || host::sub32(a, b), simd::subss)
    }
    pub fn mul(&self, a: f32, b: f32) -> f32 {
        self.scalar(a, b, || host::mul32(a, b), simd::mulss)
    }
    pub fn div(&self, a: f32, b: f32) -> f32 {
        self.scalar(a, b, || host::div32(a, b), simd::divss)
    }

    /// `sqrtss`.
    pub fn sqrt(&self, value: f32) -> f32 {
        self.scalar(0.0, value, || host::sqrt32(value), simd::sqrtss)
    }

    /// One SSE scalar operation `a op b` (`sqrtss`: of `b`). On the
    /// checked path (operands zero or normal, the result normal) the
    /// host's; otherwise, under [`checked_guest`], the `simd` crate's
    /// model under the guest's MXCSR, and outside it a decline.
    fn scalar(
        &self,
        a: f32,
        b: f32,
        host_op: impl FnOnce() -> f32,
        model: fn(&mut u32, u128, u32) -> Result<u128, simd::Xm>,
    ) -> f32 {
        let inputs = checked_inputs(&[a.to_bits(), b.to_bits()]);
        let Some(g) = &self.guest else {
            if !inputs {
                self.decline();
            }
            return self.record32(self.sse(host_op));
        };
        let nearest = g.mxcsr.get() & 0x6000 == 0;
        if inputs && nearest {
            let before = host::read();
            let r = self.sse(host_op);
            if ordinary32(r) {
                return r;
            }
            // The guest's result comes from the model; the host's flags
            // for this operation are not the guest's.
            host::write(before);
        }
        let mut m = g.mxcsr.get();
        match model(&mut m, u128::from(a.to_bits()), b.to_bits()) {
            Ok(r) => {
                g.mxcsr.set(m);
                f32::from_bits(r as u32)
            }
            Err(simd::Xm) => {
                self.decline();
                0.0
            }
        }
    }

    /// `comiss a, b` (`quiet`: `ucomiss`): `None` when unordered. Under
    /// [`checked_guest`] a NaN or subnormal operand raises IE or DE in the
    /// guest's MXCSR; outside it one declines.
    pub fn comiss(&self, a: f32, b: f32, quiet: bool) -> Option<std::cmp::Ordering> {
        if checked_inputs(&[a.to_bits(), b.to_bits()]) {
            return a.partial_cmp(&b);
        }
        let Some(g) = &self.guest else {
            self.decline();
            return a.partial_cmp(&b);
        };
        let mut m = g.mxcsr.get();
        let compare = if quiet { simd::ucomiss } else { simd::comiss };
        match compare(&mut m, a.to_bits(), b.to_bits()) {
            Ok(_) => g.mxcsr.set(m),
            Err(simd::Xm) => self.decline(),
        }
        a.partial_cmp(&b)
    }

    /// `fst m32` from double working precision.
    pub(super) fn narrow(&self, value: f64) -> f32 {
        self.record32(Self::sample(&self.x87_inexact, || host::narrow(value)))
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
    region(None, kernel)
}

/// [`checked`] under the guest's `mxcsr` (every exception masked; see
/// [`checked_guest_env`]) and x87 `control` (see [`super::x87_env`]):
/// an SSE operation off the checked path runs on the `simd` crate's model
/// and every [`super::Chain`] operation on [`super::X87`], so neither
/// declines. [`Checked::mxcsr`] and [`Checked::x87_flags`] are the flags
/// the region raised, for the kernel to return.
pub fn checked_guest<T>(mxcsr: u32, control: u16, kernel: impl FnOnce(&Checked) -> Option<T>) -> Option<T> {
    region(
        Some(Guest {
            mxcsr: Cell::new(mxcsr),
            x87: super::X87::new(control),
        }),
        kernel,
    )
}

fn region<T>(guest: Option<Guest>, kernel: impl FnOnce(&Checked) -> Option<T>) -> Option<T> {
    let mut guest = guest;
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
            x87_inexact: Cell::new(false),
            guest: guest.take(),
        };
        let result = std::hint::black_box(kernel(&scope));
        // Under the guest's state every operation proves its own result
        // (the checked path's operand and result bounds, or the model),
        // and host flags from an operation the model replaced are not the
        // guest's; outside it the host's status is the proof.
        let host_ok = scope.guest.is_some() || host::read() & host::REJECT == 0;
        let ok = host_ok && scope.ordinary_results.get();
        drop(restore);
        if ok { result } else { None }
    })
}

#[cfg(test)]
mod tests;
