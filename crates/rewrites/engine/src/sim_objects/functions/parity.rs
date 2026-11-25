//! The object function evaluator's capture arm
//! (docs/engine/sim-rewrite.md, phase 3; the pattern of phase 5's
//! `sim_physics::parity`): at every call of the tag-function curve
//! evaluator `13b390` — the curve every object function value, light and
//! screen effect flows through — the idiomatic kernel
//! `sim_objects::functions::curve` predicts the result from the guest
//! state, the guest answers (or the slice 3 bodies, which reach it as a
//! callout), and the prediction is compared with what the answer left in
//! `xmm0`. Mismatches are counted and the first is named on stderr;
//! [`status`] carries the counters for tests and reports. A declined call
//! (float environment, curve kinds 9 and 10, an unreadable operand) is
//! the guest's alone and counted by reason, never silent.

use super::lo;
use crate::function_eval::constant::{
    BYTE, ONE, SHAPE_BIAS, SHAPE_SCALE, SHAPES, WAVE_SCALE, WAVES, WRAP_HIGH, WRAP_LOW,
};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_objects::functions::curve::{self, Answer, Image};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// The two stack words `13b390`'s `ret 8` pops, besides the return
/// address.
const ARGS: u32 = 8;

/// The arm's counters since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Calls compared at the return.
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
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
        declined_float: read(&DECLINED_FLOAT),
        declined_kind: read(&DECLINED_KIND),
        declined_fault: read(&DECLINED_FAULT),
    }
}

/// One open prediction per thread: the result bits and the `esp` the
/// return leaves. A new capture replaces a stale one.
thread_local! {
    static OPEN: RefCell<HashMap<u32, (u32, u32)>> = RefCell::new(HashMap::new());
}

/// At the entry, before the guest runs: gather the call, run the kernel,
/// and keep the prediction for this thread's return.
pub fn curve_begin(thread: u32, cpu: &CpuAbi, guest: &Guest) {
    let captured = capture(cpu, guest);
    OPEN.with_borrow_mut(|calls| {
        calls.remove(&thread);
        match captured {
            Ok((bits, end_sp)) => {
                calls.insert(thread, (bits, end_sp));
            }
            Err(why) => count_decline(why),
        }
    });
}

/// At the return, after the guest ran: compare its result with the
/// prediction. The guest's answer stands either way.
pub fn curve_end(thread: u32, cpu: &CpuAbi) {
    let Some((want, end_sp)) = OPEN.with_borrow_mut(|calls| calls.remove(&thread)) else {
        return;
    };
    CHECKED.fetch_add(1, Ordering::Relaxed);
    let got = lo(cpu, 0);
    if got == want && cpu.esp == end_sp {
        MATCHED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let seen = MISMATCHED.fetch_add(1, Ordering::Relaxed);
    if seen == 0 {
        eprintln!(
            "sim: objects function-curve arm: the guest's result differs from the prediction \
             (xmm0 {got:#010x}, want {want:#010x}; esp {:#010x}, want {end_sp:#010x})",
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
fn capture(cpu: &CpuAbi, guest: &Guest) -> Result<(u32, u32), Miss> {
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
        Ok(Answer::Value(bits)) => Ok((bits, end_sp)),
        Ok(Answer::Declined(curve::Decline::Float)) => Err(Miss::Float),
        Ok(Answer::Declined(curve::Decline::Kind)) => Err(Miss::Kind),
        Err(_) => Err(Miss::Fault),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    /// The image constants, a reference with a kind-0 blob, and a stack
    /// with the two words of the call.
    fn call(mxcsr: u32, control: u16) -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0041_0000, 0x0008_0000, Permissions::RW).unwrap();
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
            mxcsr,
            ..Default::default()
        };
        cpu.x87.control = control;
        (cpu, g)
    }

    #[test]
    fn counts_track_matches_declines_and_the_first_mismatch() {
        let before = status();
        let (cpu, g) = call(0x1fa0, 0x027f);
        // A matched call: the prediction equals the identity curve at
        // 0.25, clamped.
        curve_begin(7, &cpu, &g);
        let mut answer = cpu;
        answer.esp += 4 + ARGS;
        answer.xmm[0] = u128::from(0.25f32.to_bits()).to_le_bytes();
        curve_end(7, &answer);
        // A mismatched call: the guest's word differs.
        curve_begin(7, &cpu, &g);
        answer.xmm[0] = u128::from(0.75f32.to_bits()).to_le_bytes();
        curve_end(7, &answer);
        // A declined environment and a declined kind.
        let (env, g) = call(0x1f80, 0x027f);
        curve_begin(7, &env, &g);
        let mut nine = call(0x1fa0, 0x027f);
        nine.1.set_u8(0x0048_8000, 9).unwrap();
        curve_begin(7, &nine.0, &nine.1);
        // An unreadable operand: the reference is unmapped.
        let (mut fault, g) = call(0x1fa0, 0x027f);
        fault.edi = 0x0100_0000;
        curve_begin(7, &fault, &g);
        let after = status();
        let delta = Status {
            checked: after.checked - before.checked,
            matched: after.matched - before.matched,
            mismatched: after.mismatched - before.mismatched,
            declined_float: after.declined_float - before.declined_float,
            declined_kind: after.declined_kind - before.declined_kind,
            declined_fault: after.declined_fault - before.declined_fault,
        };
        assert_eq!(
            delta,
            Status {
                checked: 2,
                matched: 1,
                mismatched: 1,
                declined_float: 1,
                declined_kind: 1,
                declined_fault: 1,
            }
        );
    }
}
