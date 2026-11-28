//! Thread-local floating-point status on the two native simulation targets.
//! Arithmetic whose flags matter uses side-effecting asm. Ordinary Rust
//! arithmetic can be folded or hoisted even when its result is black_box'ed.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use core::arch::asm;

#[cfg(target_arch = "x86_64")]
mod status {
    use super::asm;

    pub type Word = u32;
    // IE, ZE, OE, UE in MXCSR.
    pub const REJECT: Word = 0x1d;
    // PE in MXCSR.
    pub const INEXACT: Word = 0x20;

    pub fn read() -> Word {
        let mut word = 0u32;
        // SAFETY: SSE2 is baseline and the destination is a writable m32.
        unsafe { asm!("stmxcsr [{p}]", p = in(reg) &mut word, options(nostack, preserves_flags)) };
        word
    }

    pub fn write(word: Word) {
        // SAFETY: all control/reserved bits come from this thread's MXCSR.
        unsafe { asm!("ldmxcsr [{p}]", p = in(reg) &word, options(nostack)) };
    }

    pub fn fast_env() -> bool {
        read() & 0x7f80 == 0x1f80
    }

    pub fn start() -> Option<Word> {
        let old = read();
        // All six exceptions masked, nearest-even, gradual underflow.
        if old & 0xffc0 != 0x1f80 {
            return None;
        }
        write(old & !0x3f);
        Some(old)
    }

    #[test]
    fn unsupported_mxcsr_controls_decline_before_arithmetic() {
        struct Restore(Word);
        impl Drop for Restore {
            fn drop(&mut self) {
                write(self.0);
            }
        }
        let saved = Restore(read());
        // Preserve the status bits and set masked exceptions and nearest
        // rounding. Flip one supported control bit in each case.
        let baseline = (saved.0 & !0xffc0) | 0x1f80;
        for bit in [1 << 15, 1 << 13] {
            write(baseline | bit);
            assert_eq!(crate::det::checked::<()>(|_| panic!("kernel ran")), None);
            assert_eq!(fast_env(), bit == 1 << 15);
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod status {
    use super::asm;

    pub type Word = u64;
    // IOC, DZC, OFC, UFC in FPSR.
    pub const REJECT: Word = 0xf;
    // IXC in FPSR.
    pub const INEXACT: Word = 0x10;

    pub fn read() -> Word {
        let word;
        // SAFETY: FPSR is readable in userspace.
        unsafe { asm!("mrs {}, fpsr", out(reg) word, options(nostack, preserves_flags)) };
        word
    }

    pub fn write(word: Word) {
        // SAFETY: restores userspace status or clears its cumulative flags.
        unsafe { asm!("msr fpsr, {}", in(reg) word, options(nostack)) };
    }

    fn control() -> u64 {
        let control: u64;
        // SAFETY: FPCR is readable in userspace.
        unsafe { asm!("mrs {}, fpcr", out(reg) control, options(nostack, preserves_flags)) };
        control
    }

    // Nearest rounding, masked exceptions, standard FP behavior (AH=0).
    const FAST_CONTROL: u64 = (3 << 22) | (1 << 15) | (0x1f << 8) | (1 << 1);
    pub fn fast_env() -> bool {
        control() & FAST_CONTROL == 0
    }

    pub fn start() -> Option<Word> {
        // FZ and FIZ must also be clear for a full-range operation.
        if control() & (FAST_CONTROL | (1 << 24) | 1) != 0 {
            return None;
        }
        let old = read();
        write(old & !0x9f);
        Some(old)
    }

    #[test]
    fn unsupported_fpcr_controls_decline_before_arithmetic() {
        fn set_control(value: u64) {
            // SAFETY: changes only this test thread's userspace FPCR.
            unsafe { asm!("msr fpcr, {}", in(reg) value, options(nostack)) };
        }
        struct Restore(u64);
        impl Drop for Restore {
            fn drop(&mut self) {
                set_control(self.0);
            }
        }
        let saved = Restore(control());
        let baseline = saved.0 & !(FAST_CONTROL | (1 << 24) | 1);
        for bit in [1 << 24, 1 << 22] {
            set_control(baseline | bit);
            assert_eq!(crate::det::checked::<()>(|_| panic!("kernel ran")), None);
            assert_eq!(fast_env(), bit == 1 << 24);
        }
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod status {
    pub type Word = u32;
    pub const REJECT: Word = 0;
    pub const INEXACT: Word = 0;
    pub fn read() -> Word {
        0
    }
    pub fn write(_: Word) {}
    pub fn fast_env() -> bool {
        false
    }
    pub fn start() -> Option<Word> {
        None
    }
}

pub(super) use status::*;

// The scalar instructions must execute even if a later comparison discards
// the result. Do not mark these asm blocks pure or preserves_flags:
// floating-point status is a side effect of every operation.
macro_rules! binary {
    ($name:ident, $ty:ty, $x86:literal, $arm:literal, $width:literal) => {
        #[inline]
        pub(super) fn $name(a: $ty, b: $ty) -> $ty {
            #[cfg(target_arch = "x86_64")]
            {
                let mut out = a;
                // SAFETY: SSE2 baseline; register operands, no memory access.
                unsafe { asm!(concat!($x86, " {out}, {b}"),
                    out = inout(xmm_reg) out, b = in(xmm_reg) b, options(nostack)) };
                out
            }
            #[cfg(target_arch = "aarch64")]
            {
                let out;
                // SAFETY: AArch64 baseline scalar FP, register operands.
                unsafe { asm!(concat!($arm, " {out:", $width, "}, {a:", $width, "}, {b:", $width, "}"),
                    out = lateout(vreg) out, a = in(vreg) a, b = in(vreg) b, options(nostack)) };
                out
            }
            #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
            {
                let _ = (a, b);
                unreachable!("checked regions decline on this target")
            }
        }
    };
}

binary!(add32, f32, "addss", "fadd", "s");
binary!(sub32, f32, "subss", "fsub", "s");
binary!(mul32, f32, "mulss", "fmul", "s");
binary!(div32, f32, "divss", "fdiv", "s");
binary!(add64, f64, "addsd", "fadd", "d");
binary!(sub64, f64, "subsd", "fsub", "d");
binary!(mul64, f64, "mulsd", "fmul", "d");
binary!(div64, f64, "divsd", "fdiv", "d");

macro_rules! unary {
    ($name:ident, $src:ty, $dst:ty, $x86:literal, $arm:literal) => {
        #[inline]
        pub(super) fn $name(value: $src) -> $dst {
            #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
            {
                let out;
                // SAFETY: baseline scalar FP instructions, register operands.
                unsafe {
                    #[cfg(target_arch = "x86_64")]
                    asm!($x86, out = lateout(xmm_reg) out, value = in(xmm_reg) value, options(nostack));
                    #[cfg(target_arch = "aarch64")]
                    asm!($arm, out = lateout(vreg) out, value = in(vreg) value, options(nostack));
                }
                out
            }
            #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
            {
                let _ = value;
                unreachable!("checked regions decline on this target")
            }
        }
    };
}

unary!(
    narrow,
    f64,
    f32,
    "cvtsd2ss {out}, {value}",
    "fcvt {out:s}, {value:d}"
);
unary!(
    sqrt32,
    f32,
    f32,
    "sqrtss {out}, {value}",
    "fsqrt {out:s}, {value:s}"
);
unary!(
    sqrt64,
    f64,
    f64,
    "sqrtsd {out}, {value}",
    "fsqrt {out:d}, {value:d}"
);
