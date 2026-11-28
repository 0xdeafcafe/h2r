//! The tag-function curve evaluator `13b390` in Rust (docs/engine/sim-rewrite.md,
//! phase 3): at every call, the curve every object function value, light
//! and screen effect flows through, the idiomatic kernel
//! `sim_objects::functions::curve` answers from the guest state, counted
//! in `answered`. Nothing is compared. A call the kernel cannot take
//! (the float environment or an operand off its exact path, curve kinds 9
//! and 10, an unreadable operand) is the guest's, counted by reason
//! ([`Status`]); those are the only guest calls left at this point.
//!
//! What an answered call leaves: `xmm0`'s low lane, `esp` and the return,
//! and the arithmetic flags undefined. `eax`, `ecx`, `edx` and `edi` keep
//! their values (retail's evaluator leaves them unwritten on some path, so
//! no caller relies on them: the liveness table in
//! docs/engine/object-update.md), `ebx`, `ebp` and `esi` are retail's, and
//! the x87 stack is as it was (retail leaves it empty).

use super::lo;
use crate::function_eval::constant::{
    BYTE, ONE, SHAPE_BIAS, SHAPE_SCALE, SHAPES, WAVE_SCALE, WAVES, WRAP_HIGH, WRAP_LOW,
};
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
    /// Calls left to the guest, by reason: the float environment or an
    /// operand off the kernel's exact path; curve kinds 9 and 10; a guest
    /// read that faulted.
    pub declined_float: u64,
    pub declined_kind: u64,
    pub declined_fault: u64,
}

static ANSWERED: AtomicU64 = AtomicU64::new(0);
static DECLINED_FLOAT: AtomicU64 = AtomicU64::new(0);
static DECLINED_KIND: AtomicU64 = AtomicU64::new(0);
static DECLINED_FAULT: AtomicU64 = AtomicU64::new(0);

/// The arm's counts, for tests and reports.
pub fn status() -> Status {
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status {
        answered: read(&ANSWERED),
        declined_float: read(&DECLINED_FLOAT),
        declined_kind: read(&DECLINED_KIND),
        declined_fault: read(&DECLINED_FAULT),
    }
}

/// The kernel's answer to one call: the result bits and the `esp` its
/// return leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Prediction {
    bits: u32,
    end_sp: u32,
}

/// What [`enter`] did with a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The kernel answered: `cpu` holds the result and the popped frame;
    /// the caller returns to `target`.
    Answered { target: u32 },
    /// The kernel cannot take the call: the guest answers, counted by
    /// reason.
    Guest,
}

/// At the entry, before anything runs: the kernel answers the call, or
/// counts why it cannot.
pub fn enter(cpu: &mut CpuAbi, guest: &Guest) -> Step {
    let prediction = match capture(cpu, guest) {
        Ok(p) => p,
        Err(why) => {
            count_decline(why);
            return Step::Guest;
        }
    };
    let Ok(target) = guest.u32(cpu.esp) else {
        count_decline(Miss::Fault);
        return Step::Guest;
    };
    cpu.xmm[0][..4].copy_from_slice(&prediction.bits.to_le_bytes());
    cpu.esp = prediction.end_sp;
    cpu.eflags_defined &= !ARITH;
    ANSWERED.fetch_add(1, Ordering::Relaxed);
    Step::Answered { target }
}

/// Zero the counters (at map load: the curves are the map's).
pub fn reset() {
    for counter in [&ANSWERED, &DECLINED_FLOAT, &DECLINED_KIND, &DECLINED_FAULT] {
        counter.store(0, Ordering::Relaxed);
    }
}

/// Why a call was left to the guest, counted.
enum Miss {
    Float,
    Kind,
    Fault,
}

fn count_decline(why: Miss) {
    match why {
        Miss::Float => DECLINED_FLOAT.fetch_add(1, Ordering::Relaxed),
        Miss::Kind => DECLINED_KIND.fetch_add(1, Ordering::Relaxed),
        Miss::Fault => DECLINED_FAULT.fetch_add(1, Ordering::Relaxed),
    };
}

/// The kernel's prediction of the call `cpu` is entering, with the `esp`
/// its return leaves; `Err` when the call is the guest's alone.
fn capture(cpu: &CpuAbi, guest: &Guest) -> Result<Prediction, Miss> {
    if !det::checked_env(cpu.mxcsr)
        || !det::x87_env(cpu.x87.control, cpu.x87.status, cpu.x87.tag, 1)
    {
        return Err(Miss::Float);
    }
    let word = |at: u32| guest.u32(at).map_err(|_| Miss::Fault);
    let image = Image {
        one: word(ONE)?,
        shape_scale: word(SHAPE_SCALE)?,
        shape_bias: word(SHAPE_BIAS)?,
        wave_scale: word(WAVE_SCALE)?,
        byte: word(BYTE)?,
        wrap_high: word(WRAP_HIGH)?,
        wrap_low: word(WRAP_LOW)?,
        shapes: SHAPES,
        waves: WAVES,
    };
    let input = word(cpu.esp.wrapping_add(4))?;
    let range = word(cpu.esp.wrapping_add(8))?;
    let end_sp = cpu.esp.wrapping_add(4 + ARGS);
    match curve::evaluate(
        &image,
        cpu.mxcsr,
        cpu.x87.control,
        cpu.edi,
        input,
        range,
        |at| guest.u32(at).map_err(|_| Miss::Fault),
        |at| guest.u8(at).map_err(|_| Miss::Fault),
    ) {
        Ok(Answer::Value(bits)) => Ok(Prediction { bits, end_sp }),
        Ok(Answer::Declined(curve::Decline::Float)) => Err(Miss::Float),
        Ok(Answer::Declined(curve::Decline::Kind)) => Err(Miss::Kind),
        Err(_) => Err(Miss::Fault),
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
        for (at, v) in [
            (ONE, 1.0f32),
            (SHAPE_SCALE, 1023.0),
            (SHAPE_BIAS, 0.1),
            (WAVE_SCALE, 1024.0 / 28.0),
            (BYTE, 1.0 / 255.0),
            (WRAP_HIGH, 0.75),
            (WRAP_LOW, 0.25),
        ] {
            g.set_u32(at, v.to_bits()).unwrap();
        }
        let reference = 0x0048_7ff0;
        let data = 0x0048_8000;
        g.set_u32(reference, 1).unwrap();
        g.set_u32(reference + 4, data).unwrap();
        g.set_u8(data, 0).unwrap();
        let stack = 0x007f_0000u32;
        g.map_zeroed(stack, 0x1000, Permissions::RW).unwrap();
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

    #[test]
    fn the_kernel_answers_every_call_it_can_and_counts_the_rest() {
        let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let (cpu, g) = call(0x1fa0, 0x027f);
        for n in 1..=3 {
            let mut entering = cpu;
            let Step::Answered { target } = enter(&mut entering, &g) else { panic!("an answerable call") };
            assert_eq!(target, 0, "the return word on the synthetic stack");
            assert_eq!(entering.esp, cpu.esp + 4 + ARGS);
            assert_eq!(lo(&entering, 0), WANT);
            assert_eq!(entering.eflags_defined & ARITH, 0);
            assert_eq!(status().answered, n);
        }

        // A declined environment, kind and operand are the guest's,
        // counted by reason.
        let before = status();
        let (mut env, g2) = call(0x1f80, 0x027f);
        assert_eq!(enter(&mut env, &g2), Step::Guest);
        let (mut nine, mut g9) = call(0x1fa0, 0x027f);
        g9.set_u8(0x0048_8000, 9).unwrap();
        assert_eq!(enter(&mut nine, &g9), Step::Guest);
        let (mut fault, g3) = call(0x1fa0, 0x027f);
        fault.edi = 0x0100_0000;
        assert_eq!(enter(&mut fault, &g3), Step::Guest);
        let after = status();
        assert_eq!(
            (after.declined_float - before.declined_float, after.declined_kind - before.declined_kind, after.declined_fault - before.declined_fault),
            (1, 1, 1)
        );
        assert_eq!(after.answered, before.answered);

        reset();
        assert_eq!(status(), Status::default());
    }
}
