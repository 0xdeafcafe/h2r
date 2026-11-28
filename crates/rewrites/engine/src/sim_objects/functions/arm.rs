//! The object-function evaluator's shadow arm (docs/engine/sim-rewrite.md,
//! "Objects and units update (phase 3)"; the pattern of [`super::parity`]):
//! at every entry of `bab40` OBJECT_FUNCTION_VALUE that the object
//! driver runs, the Rust evaluator
//! `sim_objects::functions::evaluator` answers from Rust-owned data
//! (decoded definitions, copied tables, image constants) and the guest
//! state the call reads.
//!
//! **Proof, then take-over.** Until [`PROVEN_AFTER`] answers have matched
//! the slice 3 body's with no mismatch, that Rust body answers every call
//! (`value::value`, a register-level port of retail's) and the
//! prediction made at the entry is compared at the return
//! ([`returned`]). After that the evaluator answers, and one answerable
//! call in [`AUDIT_EVERY`] still goes to the body and is compared. A
//! mismatch stops the take-over until the next map: from then on the
//! body answers every call. A declined call is the body's, counted by
//! reason. Both answers are Rust; neither is a guest call. The counters are process-wide; the audit slot is
//! the count of answerable calls, so it depends on no thread.
//!
//! **What an answer leaves.** The value stored through the out pointer;
//! `al`; `ecx` with its top byte the frame's result byte, as retail pops
//! it; `xmm0`'s low lane where retail leaves the value there; the three
//! arguments and the return address popped (`ret 0c`); the arithmetic
//! flags undefined. `eax` above `al`, `edx`, the upper lanes of `xmm0`,
//! `xmm1`, `xmm2` and `xmm4`, and the x87 status and last-instruction
//! pointers keep their values: no caller reads them after the call (the
//! register liveness in docs/engine/object-update.md, and the curve arm's
//! x87 rule). The audit compares what an answer writes.
//!
//! **Pending audits.** A compared call's prediction waits on a per-thread
//! stack until a `bab40` body segment returns with `esp` at the
//! prediction's end. An entry the stack passes over (its return was taken
//! by the original, or the frame was unwound) is dropped, counted.

use super::{OBJECTS, ONE, TAGS, TYPES, ZERO, base, value};
use crate::function_eval::constant::{
    BYTE, SHAPE_BIAS, SHAPE_SCALE, SHAPES, WAVE_SCALE, WAVES, WRAP_HIGH, WRAP_LOW,
};
use guest_layout::points;
use crate::sim_physics::ARITH;
use ::sim_objects::functions::curve::Image;
use ::sim_objects::functions::evaluator::{
    Answer, BaseNames, Constants, Decline, Env, Globals, Hash, Library, Memory, Tables,
};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// The three stack words `bab40`'s `ret 0c` pops, besides the return.
pub const ARGS: u32 = 0x0c;

/// Matched calls, with no mismatch, before the evaluator answers.
pub const PROVEN_AFTER: u64 = 4096;

/// After that, one answerable call in this many is compared.
pub const AUDIT_EVERY: u64 = 64;

/// Compared calls waiting for their return, at most.
const PENDING_MAX: usize = 64;

/// The arm's counters since the map loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Calls the guest answered and the arm compared at the return.
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    /// Calls the evaluator answered in the guest's place.
    pub answered: u64,
    /// Calls left to the guest, by [`Decline::index`].
    pub declined: [u64; Decline::ALL.len()],
    /// Compared calls whose return was never seen.
    pub dropped: u64,
}

impl Status {
    pub fn declined_total(&self) -> u64 {
        self.declined.iter().sum()
    }
}

static CHECKED: AtomicU64 = AtomicU64::new(0);
static MATCHED: AtomicU64 = AtomicU64::new(0);
static MISMATCHED: AtomicU64 = AtomicU64::new(0);
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static ANSWERABLE: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static DECLINED: [AtomicU64; Decline::ALL.len()] =
    [const { AtomicU64::new(0) }; Decline::ALL.len()];

/// The map's Rust-owned data: built at the first call after a map load.
/// `None` inside: not built yet. `Some(None)`: the image would not read.
static LIBRARY: Mutex<Option<Option<Library>>> = Mutex::new(None);

thread_local! {
    static PENDING: RefCell<Vec<Prediction>> = const { RefCell::new(Vec::new()) };
}

pub fn status() -> Status {
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status {
        checked: read(&CHECKED),
        matched: read(&MATCHED),
        mismatched: read(&MISMATCHED),
        answered: read(&ANSWERED),
        declined: std::array::from_fn(|i| read(&DECLINED[i])),
        dropped: read(&DROPPED),
    }
}

/// Forget the map: the counters, the proof, the Rust-owned data and this
/// thread's pending audits. Called at map load.
pub fn reset() {
    for c in [
        &CHECKED,
        &MATCHED,
        &MISMATCHED,
        &ANSWERED,
        &ANSWERABLE,
        &DROPPED,
    ] {
        c.store(0, Ordering::Relaxed);
    }
    for c in &DECLINED {
        c.store(0, Ordering::Relaxed);
    }
    *LIBRARY.lock().unwrap_or_else(|e| e.into_inner()) = None;
    PENDING.with(|p| p.borrow_mut().clear());
}

fn proven() -> bool {
    MISMATCHED.load(Ordering::Relaxed) == 0 && MATCHED.load(Ordering::Relaxed) >= PROVEN_AFTER
}

/// One call's prediction: what its return must leave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prediction {
    out: u32,
    answer: Answer,
    ecx: u32,
    end_sp: u32,
}

/// What [`enter`] did with a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The evaluator answered: `cpu` and guest memory hold the result and
    /// the popped frame; the caller returns to `target`.
    Answered { target: u32 },
    /// The guest answers; its return is compared ([`returned`]).
    Audit,
    /// The guest answers alone: declined, counted.
    Guest,
}

/// Guest memory as the evaluator reads it.
struct Mem<'a>(&'a Guest);

impl Memory for Mem<'_> {
    fn u32(&self, at: u32) -> Option<u32> {
        self.0.u32(at).ok()
    }
    fn u16(&self, at: u32) -> Option<u16> {
        self.0.u16(at).ok()
    }
    fn u8(&self, at: u32) -> Option<u8> {
        self.0.u8(at).ok()
    }
}

/// At `bab40`'s entry, before anything runs: evaluate and decide who
/// answers.
pub fn enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
    let prediction = match predict(cpu, guest) {
        Ok(p) => p,
        Err(why) => {
            DECLINED[why.index()].fetch_add(1, Ordering::Relaxed);
            return Step::Guest;
        }
    };
    let slot = ANSWERABLE.fetch_add(1, Ordering::Relaxed);
    if !proven() || slot % AUDIT_EVERY == 0 {
        PENDING.with(|p| {
            let mut p = p.borrow_mut();
            if p.len() == PENDING_MAX {
                p.remove(0);
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
            p.push(prediction);
        });
        return Step::Audit;
    }
    let Ok(target) = guest.u32(cpu.esp) else {
        DECLINED[Decline::Fault.index()].fetch_add(1, Ordering::Relaxed);
        return Step::Guest;
    };
    if guest
        .set_u32(prediction.out, prediction.answer.value)
        .is_err()
    {
        DECLINED[Decline::Fault.index()].fetch_add(1, Ordering::Relaxed);
        return Step::Guest;
    }
    let a = prediction.answer;
    cpu.eax = (cpu.eax & !0xff) | u32::from(a.al);
    cpu.ecx = prediction.ecx;
    if let Some(v) = a.xmm0 {
        cpu.xmm[0][..4].copy_from_slice(&v.to_le_bytes());
    }
    cpu.esp = prediction.end_sp;
    cpu.eflags_defined &= !ARITH;
    ANSWERED.fetch_add(1, Ordering::Relaxed);
    Step::Answered { target }
}

/// After a `bab40` body segment returned: compare the prediction whose
/// frame this return pops, and drop the ones it passed over.
pub fn returned(cpu: &CpuAbi, guest: &Guest) {
    let p = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        while p.last().is_some_and(|q| q.end_sp < cpu.esp) {
            p.pop();
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        if p.last().is_some_and(|q| q.end_sp == cpu.esp) {
            p.pop()
        } else {
            None
        }
    });
    if let Some(p) = p {
        compare(p, cpu, guest);
    }
}

/// The guest's return against the prediction. The guest's answer stands.
fn compare(p: Prediction, cpu: &CpuAbi, guest: &Guest) {
    CHECKED.fetch_add(1, Ordering::Relaxed);
    let value = guest.u32(p.out).ok();
    let xmm0 = u32::from_le_bytes(cpu.xmm[0][..4].try_into().expect("four bytes"));
    let a = p.answer;
    let same = value == Some(a.value)
        && cpu.eax as u8 == a.al
        && cpu.ecx == p.ecx
        && a.xmm0.is_none_or(|v| v == xmm0);
    if same {
        MATCHED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if MISMATCHED.fetch_add(1, Ordering::Relaxed) == 0 {
        eprintln!(
            "sim: objects function arm: the guest's bab40 differs from the evaluator \
             (value {value:x?} want {:#010x}, al {:#04x} want {:#04x}, ecx {:#010x} want {:#010x}, \
             xmm0 {xmm0:#010x} want {:x?}); the guest answers every call until the next map",
            a.value, cpu.eax as u8, a.al, cpu.ecx, p.ecx, a.xmm0
        );
    }
}

/// The evaluator's answer to the call `cpu` is entering.
fn predict(cpu: &CpuAbi, guest: &Guest) -> Result<Prediction, Decline> {
    let word = |at: u32| guest.u32(at).map_err(|_| Decline::Fault);
    let handle = word(cpu.esp.wrapping_add(4))?;
    let name = word(cpu.esp.wrapping_add(8))?;
    let out = word(cpu.esp.wrapping_add(0xc))?;
    let x = &cpu.x87;
    let env = Env {
        mxcsr: cpu.mxcsr,
        x87_control: x.control,
        x87_ready: det::x87_env(x.control, x.status, x.tag, 1),
    };
    let mem = Mem(guest);
    let mut library = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
    let library = library
        .get_or_insert_with(|| load(&mem))
        .as_mut()
        .ok_or(Decline::Fault)?;
    let answer = library.value(&mem, env, handle, name)?;
    Ok(Prediction {
        out,
        answer,
        ecx: (cpu.ecx & 0x00ff_ffff) | (u32::from(answer.active) << 24),
        end_sp: cpu.esp.wrapping_add(4 + ARGS),
    })
}

/// The map's library: the image constants (string IDs and hash
/// parameters from the instructions that carry them) and the tables.
fn load(m: &Mem<'_>) -> Option<Library> {
    let w = |at: u32| m.u32(at);
    let names = BaseNames {
        random: w(base::NAME_RANDOM)?,
        bcad0: w(base::NAME_BCAD0)?,
        facing: w(base::NAME_FACING)?,
        one: w(base::NAME_ONE)?,
        zero: w(base::NAME_ZERO)?,
        flag: w(base::NAME_FLAG)?,
        variant: w(base::NAME_VARIANT)?,
        bcbb0: w(base::NAME_BCBB0)?,
        bca70: w(base::NAME_BCA70)?,
        body_ec: w(base::NAME_BODY_EC)?,
        one_b: w(base::NAME_ONE_B)?,
        bcb10_delta: w(base::DELTA_BCB10)?,
        component: w(base::NAME_COMPONENT)?,
        body_f0: w(base::NAME_BODY_F0)?,
        f0_less_one: w(base::NAME_F0_LESS_ONE)?,
        c18e0: w(base::NAME_C18E0)?,
        shield_test: w(base::NAME_SHIELD_TEST)?,
        one_c: w(base::NAME_ONE_C)?,
        body_f8: w(base::NAME_BODY_F8)?,
        body_f4: w(base::NAME_BODY_F4)?,
        component_lo: w(base::COMPONENT_LO)?,
        component_hi: w(base::COMPONENT_HI)?,
        world_78: w(base::NAME_WORLD_78)?,
        component_b: w(base::NAME_COMPONENT_B)?,
        world_70_delta: w(base::DELTA_WORLD_70)?,
        world_7c: w(base::NAME_WORLD_7C)?,
        component_lo_b: w(base::COMPONENT_LO_B)?,
        component_hi_b: w(base::COMPONENT_HI_B)?,
    };
    let constants = Constants {
        globals: Globals {
            objects: OBJECTS,
            tags: TAGS,
            types: TYPES,
            world: base::WORLD,
            game: base::GAME,
            game_time: value::GAME_TIME,
            base_import: points::BASE_OBJECT_IMPORT.base,
        },
        one: w(ONE)?,
        zero: w(ZERO)?,
        name_one: w(value::NAME_ONE)?,
        name_zero: w(value::NAME_ZERO)?,
        time_hash: Hash {
            mul: w(value::HASH_MUL)?,
            add: w(value::HASH_ADD)?,
        },
        time_scale: w(value::HASH_SCALE)?,
        random_hash: Hash {
            mul: w(base::HASH_MUL)?,
            add: w(base::HASH_ADD)?,
        },
        random_scale: w(base::RANDOM_SCALE)?,
        base: names,
        curve: Image {
            one: w(ONE)?,
            shape_scale: w(SHAPE_SCALE)?,
            shape_bias: w(SHAPE_BIAS)?,
            wave_scale: w(WAVE_SCALE)?,
            byte: w(BYTE)?,
            wrap_high: w(WRAP_HIGH)?,
            wrap_low: w(WRAP_LOW)?,
            shapes: SHAPES,
            waves: WAVES,
        },
    };
    Some(Library::new(constants, Tables::read(m, SHAPES, WAVES)?))
}

#[cfg(test)]
mod tests;
