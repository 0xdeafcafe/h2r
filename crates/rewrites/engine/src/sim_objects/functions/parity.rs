//! The tag-function curve evaluator `13b390` in Rust (docs/engine/sim-rewrite.md,
//! phase 3): at every call, the curve every object function value, light
//! and screen effect flows through. The idiomatic kernel
//! `sim_objects::functions::curve` answers from the guest state; a call
//! it does not carry (curve kinds 9 and 10, the float environment or an
//! operand off its exact path) is answered by the register-level port
//! `crate::function_eval::evaluate`, which models every curve, the CRT's
//! `pow` and any MXCSR. Nothing is compared and nothing calls the guest.
//! A call neither can run (an unmasked exception the port does not
//! model, a read that faults) panics with its cause.
//!
//! What an answered call leaves: `xmm0`'s low lane, `esp` and the return,
//! and the arithmetic flags undefined; a port answer also leaves MXCSR's
//! sticky flags as retail's arithmetic sets them. `eax`, `ecx`, `edx` and
//! `edi` keep their values (retail's evaluator leaves them unwritten on
//! some path, so no caller relies on them: the liveness table in
//! docs/engine/object-update.md), `ebx`, `ebp` and `esi` are retail's, and
//! the x87 stack is as it was (retail leaves it empty).

use super::lo;
use crate::function_eval::constant::{
    BYTE, ONE, SHAPE_BIAS, SHAPE_SCALE, SHAPES, WAVE_SCALE, WAVES, WRAP_HIGH, WRAP_LOW,
};
use crate::light_constants::Sse;
use crate::sim_physics::ARITH;
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_objects::functions::curve::{self, Answer, Image};
use std::sync::atomic::{AtomicU64, Ordering};

/// The two stack words `13b390`'s `ret 8` pops, besides the return
/// address.
pub const ARGS: u32 = 8;

/// The evaluator's counters since the map loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Calls the kernel answered.
    pub answered: u64,
    /// Calls the register-level port answered: kinds 9 and 10, and the
    /// float environment or an operand off the kernel's exact path.
    pub ported_kind: u64,
    pub ported_float: u64,
}

static ANSWERED: AtomicU64 = AtomicU64::new(0);
static PORTED_KIND: AtomicU64 = AtomicU64::new(0);
static PORTED_FLOAT: AtomicU64 = AtomicU64::new(0);

/// The arm's counts, for tests and reports.
pub fn status() -> Status {
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status { answered: read(&ANSWERED), ported_kind: read(&PORTED_KIND), ported_float: read(&PORTED_FLOAT) }
}

/// Zero the counters (at map load: the curves are the map's).
pub fn reset() {
    for counter in [&ANSWERED, &PORTED_KIND, &PORTED_FLOAT] {
        counter.store(0, Ordering::Relaxed);
    }
}

/// At the entry: answer the call. `cpu` holds the result and the popped
/// frame; the caller returns to the returned target.
pub fn enter(cpu: &mut CpuAbi, guest: &Guest) -> u32 {
    let word = |at: u32| guest.u32(at).unwrap_or_else(|_| unimplemented!("objects curve: a read at {at:#x} faulted"));
    let target = word(cpu.esp);
    let input = word(cpu.esp.wrapping_add(4));
    let range = word(cpu.esp.wrapping_add(8));
    let bits = match kernel(cpu, guest, input, range) {
        Answer::Value(bits) => {
            ANSWERED.fetch_add(1, Ordering::Relaxed);
            bits
        }
        Answer::Declined(why) => {
            let counter = match why {
                curve::Decline::Kind => &PORTED_KIND,
                curve::Decline::Float => &PORTED_FLOAT,
            };
            counter.fetch_add(1, Ordering::Relaxed);
            port(cpu, guest, input, range)
        }
    };
    cpu.xmm[0][..4].copy_from_slice(&bits.to_le_bytes());
    cpu.esp = cpu.esp.wrapping_add(4 + ARGS);
    cpu.eflags_defined &= !ARITH;
    target
}

/// The idiomatic kernel, or why it does not carry the call.
fn kernel(cpu: &CpuAbi, guest: &Guest, input: u32, range: u32) -> Answer {
    if !det::checked_env(cpu.mxcsr) || !det::x87_env(cpu.x87.control, cpu.x87.status, cpu.x87.tag, 1) {
        return Answer::Declined(curve::Decline::Float);
    }
    let word = |at: u32| guest.u32(at);
    let fault = |e: runtime::GuestFault| -> ! { unimplemented!("objects curve: a read at {:#x} faulted", e.addr) };
    let image = Image {
        one: word(ONE).unwrap_or_else(|e| fault(e)),
        shape_scale: word(SHAPE_SCALE).unwrap_or_else(|e| fault(e)),
        shape_bias: word(SHAPE_BIAS).unwrap_or_else(|e| fault(e)),
        wave_scale: word(WAVE_SCALE).unwrap_or_else(|e| fault(e)),
        byte: word(BYTE).unwrap_or_else(|e| fault(e)),
        wrap_high: word(WRAP_HIGH).unwrap_or_else(|e| fault(e)),
        wrap_low: word(WRAP_LOW).unwrap_or_else(|e| fault(e)),
        shapes: SHAPES,
        waves: WAVES,
    };
    curve::evaluate(&image, cpu.mxcsr, cpu.x87.control, cpu.edi, input, range, |at| guest.u32(at), |at| guest.u8(at))
        .unwrap_or_else(|e| fault(e))
}

/// The register-level port, under the caller's MXCSR (its sticky flags
/// left as retail's) and x87 control word.
fn port(cpu: &mut CpuAbi, guest: &Guest, input: u32, range: u32) -> u32 {
    let mut sse = Sse { mxcsr: cpu.mxcsr };
    match crate::function_eval::evaluate(guest, &mut sse, cpu.x87.control, cpu.edi, input, range) {
        Ok(bits) => {
            cpu.mxcsr = sse.mxcsr;
            bits
        }
        Err(e) => unimplemented!("objects curve: the port stops on {e:?} (MXCSR {:#x}, x87 control {:#x})", cpu.mxcsr, cpu.x87.control),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;
    use std::sync::Mutex;

    /// The counters are process-wide: tests of the arm run one at a time.
    static ARM: Mutex<()> = Mutex::new(());

    /// The image constants, a reference with a kind-0 blob, and a stack
    /// with the two words of the call.
    fn call(mxcsr: u32, control: u16) -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0041_0000, 0x0008_0000, Permissions::RW).unwrap();
        crate::function_eval::put_constants(&mut g);
        let reference = 0x0048_7ff0;
        let data = 0x0048_8000;
        g.set_u32(reference, 1).unwrap();
        g.set_u32(reference + 4, data).unwrap();
        g.set_u8(data, 0).unwrap();
        let stack = 0x007f_0000u32;
        g.map_zeroed(stack, 0x1000, Permissions::RW).unwrap();
        g.set_u32(stack, 0x0012_3456).unwrap();
        g.set_u32(stack + 4, 0.25f32.to_bits()).unwrap();
        g.set_u32(stack + 8, 0.5f32.to_bits()).unwrap();
        let mut cpu = CpuAbi {
            esp: stack,
            edi: reference,
            eflags_defined: ARITH,
            mxcsr,
            ..Default::default()
        };
        cpu.x87.control = control;
        (cpu, g)
    }

    const WANT: u32 = 0x3e80_0000; // 0.25, the identity curve at 0.25

    fn answer(cpu: &CpuAbi, g: &Guest) -> CpuAbi {
        let mut entering = *cpu;
        assert_eq!(enter(&mut entering, g), 0x0012_3456, "the return word");
        assert_eq!(entering.esp, cpu.esp + 4 + ARGS);
        assert_eq!(entering.eflags_defined & ARITH, 0);
        entering
    }

    #[test]
    fn the_kernel_answers_and_the_port_takes_what_it_does_not_carry() {
        let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let (cpu, g) = call(0x1fa0, 0x027f);
        for n in 1..=3 {
            assert_eq!(lo(&answer(&cpu, &g), 0), WANT);
            assert_eq!(status().answered, n);
        }
        // An environment off the kernel's path is the port's, the same
        // identity value.
        let (env, g2) = call(0x1f80, 0x027f);
        assert_eq!(lo(&answer(&env, &g2), 0), WANT);
        // Kind 9 with a zero exponent near zero is 1 (`a + (b - a) * 1`
        // with a = 0, b = 1): the port's.
        let (nine, mut g9) = call(0x1fa0, 0x027f);
        g9.set_u8(0x0048_8000, 9).unwrap();
        g9.set_u32(0x0048_8014 + 4, 1.0f32.to_bits()).unwrap();
        let _ = lo(&answer(&nine, &g9), 0);
        let s = status();
        assert_eq!((s.answered, s.ported_float, s.ported_kind), (3, 1, 1));
        reset();
        assert_eq!(status(), Status::default());
    }

    #[test]
    #[should_panic(expected = "objects curve: a read at 0x1000004 faulted")]
    fn an_unreadable_reference_panics() {
        let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
        let (mut fault, g) = call(0x1fa0, 0x027f);
        fault.edi = 0x0100_0000;
        enter(&mut fault, &g);
    }
}
