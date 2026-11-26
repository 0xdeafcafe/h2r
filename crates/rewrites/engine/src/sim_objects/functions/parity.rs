//! The object function evaluator's capture arm and its take-over
//! (docs/engine/sim-rewrite.md, phase 3; the pattern of phase 5's
//! `sim_physics::parity`): at every call of the tag-function curve
//! evaluator `13b390` — the curve every object function value, light and
//! screen effect flows through — the idiomatic kernel
//! `sim_objects::functions::curve` predicts the result from the guest
//! state. While the arm is unproven the guest answers (or the slice 3
//! bodies, which reach it as a callout), and the prediction is compared
//! with what the answer left in `xmm0`. Mismatches are counted and the
//! first is named on stderr; [`status`] carries the counters for tests
//! and reports. A declined call (float environment, curve kinds 9 and 10,
//! an unreadable operand) is the guest's alone and counted by reason,
//! never silent.
//!
//! **Take-over.** After [`PROVEN_AFTER`] matched calls with no mismatch
//! ever seen, [`enter`] answers the call itself: `xmm0` and the return
//! from the kernel's prediction, counted in `answered`. Every
//! [`AUDIT_EVERY`]th answerable call after that still goes to the guest
//! and is compared, so a later mismatch stops the take-over for good (a
//! mismatch is never undone; the guest answers everything from then on).
//! The counters are process-wide and the audit slot is chosen by the
//! count of answerable calls, so the choice depends on no thread.
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

/// Matched calls, with no mismatch, before the arm answers calls itself.
pub const PROVEN_AFTER: u64 = 4096;

/// After that, one answerable call in this many still goes to the guest
/// and is compared.
pub const AUDIT_EVERY: u64 = 64;

/// The arm's counters since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Calls the guest answered and the arm compared at the return.
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    /// Calls the kernel answered in the guest's place.
    pub answered: u64,
    /// Calls left to the guest, by reason: the float environment or an
    /// operand off the kernel's exact path; curve kinds 9 and 10; a guest
    /// read that faulted.
    pub declined_float: u64,
    pub declined_kind: u64,
    pub declined_fault: u64,
}

static CHECKED: AtomicU64 = AtomicU64::new(0);
static MATCHED: AtomicU64 = AtomicU64::new(0);
static MISMATCHED: AtomicU64 = AtomicU64::new(0);
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static ANSWERABLE: AtomicU64 = AtomicU64::new(0);
static DECLINED_FLOAT: AtomicU64 = AtomicU64::new(0);
static DECLINED_KIND: AtomicU64 = AtomicU64::new(0);
static DECLINED_FAULT: AtomicU64 = AtomicU64::new(0);

/// The arm's counts, for tests and reports.
pub fn status() -> Status {
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status {
        checked: read(&CHECKED),
        matched: read(&MATCHED),
        mismatched: read(&MISMATCHED),
        answered: read(&ANSWERED),
        declined_float: read(&DECLINED_FLOAT),
        declined_kind: read(&DECLINED_KIND),
        declined_fault: read(&DECLINED_FAULT),
    }
}

/// The kernel's prediction of one call: the result bits and the `esp` its
/// return leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prediction {
    bits: u32,
    end_sp: u32,
}

/// What [`enter`] did with a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The kernel answered: `cpu` holds the result and the popped frame;
    /// the caller returns to `target`.
    Answered { target: u32 },
    /// The guest answers; compare its result with the prediction at the
    /// return ([`compare`]).
    Audit(Prediction),
    /// The guest answers alone: the call was declined, counted.
    Guest,
}

/// At the entry, before anything runs: run the kernel and decide who
/// answers. Whether the arm has earned the take-over is read from the
/// counters.
pub fn enter(cpu: &mut CpuAbi, guest: &Guest) -> Step {
    let prediction = match capture(cpu, guest) {
        Ok(p) => p,
        Err(why) => {
            count_decline(why);
            return Step::Guest;
        }
    };
    let slot = ANSWERABLE.fetch_add(1, Ordering::Relaxed);
    if !proven() || slot % AUDIT_EVERY == 0 {
        return Step::Audit(prediction);
    }
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

/// Forget everything the arm has seen: the counters, the mismatch and
/// the audit slot. Called at map load, so a proof earned on one map is
/// earned again on the next (the curves and the image constants are the
/// map's), and an arm a mismatch stopped starts over compare-only.
pub fn reset() {
    for counter in [
        &CHECKED,
        &MATCHED,
        &MISMATCHED,
        &ANSWERED,
        &ANSWERABLE,
        &DECLINED_FLOAT,
        &DECLINED_KIND,
        &DECLINED_FAULT,
    ] {
        counter.store(0, Ordering::Relaxed);
    }
}

/// Whether the arm has matched enough calls with no mismatch.
fn proven() -> bool {
    MISMATCHED.load(Ordering::Relaxed) == 0 && MATCHED.load(Ordering::Relaxed) >= PROVEN_AFTER
}

/// At the return, after the guest ran: compare its result with the
/// prediction. The guest's answer stands either way.
pub fn compare(prediction: Prediction, cpu: &CpuAbi) {
    CHECKED.fetch_add(1, Ordering::Relaxed);
    let got = lo(cpu, 0);
    let Prediction { bits: want, end_sp } = prediction;
    if got == want && cpu.esp == end_sp {
        MATCHED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let seen = MISMATCHED.fetch_add(1, Ordering::Relaxed);
    if seen == 0 {
        eprintln!(
            "sim: objects function-curve arm: the guest's result differs from the prediction \
             (xmm0 {got:#010x}, want {want:#010x}; esp {:#010x}, want {end_sp:#010x}); \
             the guest answers every call from here",
            cpu.esp
        );
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

    /// The guest's answer to `cpu`'s call: `bits` in `xmm0`, the frame
    /// popped.
    fn guest_answer(cpu: &CpuAbi, bits: u32) -> CpuAbi {
        let mut answer = *cpu;
        answer.esp += 4 + ARGS;
        answer.xmm[0] = u128::from(bits).to_le_bytes();
        answer
    }

    const WANT: u32 = 0x3e80_0000; // 0.25, the identity curve at 0.25

    /// The arm's whole life, in one test: the counters are process-wide,
    /// and the take-over is a function of them.
    #[test]
    fn the_arm_compares_then_takes_over_audits_and_stops_on_a_mismatch() {
        let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let (cpu, g) = call(0x1fa0, 0x027f);
        let start = status();
        assert_eq!(start.answered, 0);

        // Unproven: the guest answers, and every call is compared.
        let mut compared = 0;
        while status().matched < PROVEN_AFTER {
            let mut entering = cpu;
            let Step::Audit(p) = enter(&mut entering, &g) else { panic!("unproven calls are the guest's") };
            assert_eq!((entering.esp, entering.xmm[0]), (cpu.esp, cpu.xmm[0]), "an audited call leaves the registers alone");
            compare(p, &guest_answer(&cpu, WANT));
            compared += 1;
        }
        assert!(compared > 0);
        assert_eq!((status().mismatched, status().answered), (0, 0));

        // Proven: the kernel answers all but every AUDIT_EVERY'th call,
        // which is compared like before.
        let (mut answered, mut audited) = (0, 0);
        for _ in 0..2 * AUDIT_EVERY {
            let mut entering = cpu;
            match enter(&mut entering, &g) {
                Step::Answered { target } => {
                    answered += 1;
                    assert_eq!(target, 0, "the return word on the synthetic stack");
                    assert_eq!(entering.esp, cpu.esp + 4 + ARGS);
                    assert_eq!(lo(&entering, 0), WANT);
                    assert_eq!(entering.eflags_defined & ARITH, 0);
                }
                Step::Audit(p) => {
                    audited += 1;
                    compare(p, &guest_answer(&cpu, WANT));
                }
                Step::Guest => panic!("an answerable call is never declined"),
            }
        }
        assert_eq!((answered, audited), (2 * AUDIT_EVERY - 2, 2));
        assert_eq!(status().answered - start.answered, answered);

        // A declined environment, kind and operand are the guest's, and
        // do not count as answerable.
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

        // A mismatch at an audited call ends the take-over for good.
        let mut audit = None;
        for _ in 0..AUDIT_EVERY {
            let mut entering = cpu;
            if let Step::Audit(p) = enter(&mut entering, &g) {
                audit = Some(p);
                break;
            }
        }
        compare(audit.expect("an audit within AUDIT_EVERY calls"), &guest_answer(&cpu, 0x3f40_0000));
        assert_eq!(status().mismatched, 1);
        let answered = status().answered;
        for _ in 0..2 * AUDIT_EVERY {
            let mut entering = cpu;
            assert!(matches!(enter(&mut entering, &g), Step::Audit(_)));
        }
        assert_eq!(status().answered, answered);
    }

    /// A map load puts a proven arm, and one a mismatch stopped, back to
    /// compare-only.
    #[test]
    fn a_reset_puts_a_proven_arm_back_to_comparing() {
        let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
        let (cpu, g) = call(0x1fa0, 0x027f);
        let prove = || {
            while status().matched < PROVEN_AFTER {
                let mut entering = cpu;
                let Step::Audit(p) = enter(&mut entering, &g) else { panic!("unproven calls are the guest's") };
                compare(p, &guest_answer(&cpu, WANT));
            }
        };
        reset();
        prove();
        let mut entering = cpu;
        let mut took_over = false;
        for _ in 0..AUDIT_EVERY {
            took_over |= matches!(enter(&mut entering, &g), Step::Answered { .. });
        }
        assert!(took_over, "a proven arm answers");

        reset();
        assert_eq!(status(), Status::default());
        for _ in 0..2 * AUDIT_EVERY {
            let mut entering = cpu;
            assert!(matches!(enter(&mut entering, &g), Step::Audit(_)), "after a reset every call is compared");
        }

        // The proof is earned again, and a mismatch is forgotten too.
        reset();
        let mut entering = cpu;
        let Step::Audit(p) = enter(&mut entering, &g) else { panic!("unproven") };
        compare(p, &guest_answer(&cpu, 0x3f40_0000));
        assert_eq!(status().mismatched, 1);
        reset();
        prove();
        let answered = (0..AUDIT_EVERY).filter(|_| matches!(enter(&mut { cpu }, &g), Step::Answered { .. })).count();
        assert!(answered > 0);
    }
}
