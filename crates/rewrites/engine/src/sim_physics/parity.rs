//! The physics capture arms (sim-rewrite-havok.md §7.4, "parity arm"): the
//! Rust kernel predicts every write, register and flag of a call, and the
//! prediction is compared with what the guest left when it returns. Every
//! fallback is counted, never silent (§8).
//!
//! **Take-over.** The broadphase batch update's arm answers calls itself
//! once proven, the pattern of the object function curve arm
//! (`sim_objects::functions::parity`): after [`PROVEN_AFTER`] matched
//! calls with no mismatch, [`broadphase_enter`] applies the prediction in
//! the guest's place (counted in `answered`), and every [`AUDIT_EVERY`]th
//! answerable call still goes to the guest and is compared. A mismatch
//! ends the take-over: the guest answers every call until the next map
//! load. [`map_loaded`] starts the proof over (the matched-since-load
//! count, the answerable-call slot and the mismatch latch), so two
//! lockstep peers and a film replay of the same match, whatever ran in
//! their processes before, cross the threshold and audit on the same
//! calls. The `status` counts are totals since start and are never reset.
//!
//! The two motion arms and the MOPP box arm stay compare-only. A motion
//! prediction holds `esp`, the four callee-saved registers and the body's
//! `0x110` bytes; what retail leaves in `eax`, `ecx`, `edx`, the XMM
//! registers, the x87 stack, status and tag words, the MXCSR flags and
//! `eflags` is not modelled, and no differential test pins it, so an
//! applied motion could not be shown to leave the guest's registers.

use super::{broadphase, motion};
use crate::sim_collision::mopp_box;
use runtime::{CpuAbi, Guest};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Matched calls since the map load, with no mismatch, before an arm that
/// can take over answers calls itself.
pub const PROVEN_AFTER: u64 = 4096;

/// After that, one answerable call in this many still goes to the guest
/// and is compared.
pub const AUDIT_EVERY: u64 = 64;

/// The stack bytes the broadphase batch update's `ret 0x14` pops, besides
/// the return address.
pub const BROADPHASE_ARGS: u32 = 0x14;

/// One arm's outcome counts. `declined` counts calls the guest answered
/// uncompared: a capture that refused the state (float environment,
/// aliasing, capacity), an apply that refused its spans, or a return that
/// could not be observed.
pub struct Arm {
    pub name: &'static str,
    matched: AtomicU64,
    mismatched: AtomicU64,
    declined: AtomicU64,
    answered: AtomicU64,
    /// Since the map load: matched calls, answerable calls (the audit
    /// slot), and whether a mismatch was seen.
    proof: AtomicU64,
    slots: AtomicU64,
    ended: AtomicBool,
}

impl Arm {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            matched: AtomicU64::new(0),
            mismatched: AtomicU64::new(0),
            declined: AtomicU64::new(0),
            answered: AtomicU64::new(0),
            proof: AtomicU64::new(0),
            slots: AtomicU64::new(0),
            ended: AtomicBool::new(false),
        }
    }

    fn proven(&self) -> bool {
        !self.ended.load(Ordering::Relaxed) && self.proof.load(Ordering::Relaxed) >= PROVEN_AFTER
    }

    /// Takes the next answerable slot: whether the kernel answers it.
    fn answers(&self) -> bool {
        let slot = self.slots.fetch_add(1, Ordering::Relaxed);
        self.proven() && slot % AUDIT_EVERY != 0
    }

    fn restart_proof(&self) {
        self.proof.store(0, Ordering::Relaxed);
        self.slots.store(0, Ordering::Relaxed);
        self.ended.store(false, Ordering::Relaxed);
    }

    fn take_over(&self) -> Takeover {
        Takeover {
            name: self.name,
            answered: self.answered.load(Ordering::Relaxed),
            proven: self.proven(),
            ended: self.ended.load(Ordering::Relaxed),
        }
    }
}

pub static BROADPHASE: Arm = Arm::new("physics.broadphase");
pub static MOTION_POSE: Arm = Arm::new("physics.motion.pose");
pub static MOTION_FORCES: Arm = Arm::new("physics.motion.forces");
pub static MOPP_BOX: Arm = Arm::new("physics.mopp.box");

static ARMS: [&Arm; 4] = [&BROADPHASE, &MOTION_POSE, &MOTION_FORCES, &MOPP_BOX];

pub(super) fn declined(arm: &Arm) {
    arm.declined.fetch_add(1, Ordering::Relaxed);
}

/// A call the guest answered whose return could not be observed (a full
/// observer table, an access trap): counted with the declines.
pub fn uncompared(arm: &Arm) {
    declined(arm);
}

pub(super) fn compare(arm: &Arm, matched: bool) {
    if matched {
        arm.matched.fetch_add(1, Ordering::Relaxed);
        arm.proof.fetch_add(1, Ordering::Relaxed);
        return;
    }
    arm.mismatched.fetch_add(1, Ordering::Relaxed);
    if !arm.ended.swap(true, Ordering::Relaxed) {
        eprintln!(
            "sim: physics {} arm: the guest's result differs from the prediction; \
             the guest answers every call until the next map load",
            arm.name
        );
    }
}

/// The arms' counts since start, in table order, for tests and reports:
/// (name, matched, mismatched, declined).
pub fn status() -> [(&'static str, u64, u64, u64); 4] {
    ARMS.map(|a| {
        (
            a.name,
            a.matched.load(Ordering::Relaxed),
            a.mismatched.load(Ordering::Relaxed),
            a.declined.load(Ordering::Relaxed),
        )
    })
}

/// An arm's take-over state: calls the kernel answered since start, and,
/// since the map load, whether the arm is proven and whether a mismatch
/// ended the take-over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Takeover {
    pub name: &'static str,
    pub answered: u64,
    pub proven: bool,
    pub ended: bool,
}

/// The take-over state of the arms that answer calls: the broadphase
/// batch update's.
pub fn takeover() -> [Takeover; 1] {
    [BROADPHASE.take_over()]
}

/// At each map load, before the match's first step: every arm proves
/// itself again from zero.
pub fn map_loaded() {
    for arm in ARMS {
        arm.restart_proof();
    }
}

/// What [`broadphase_enter`] did with a call.
pub enum Step<P> {
    /// The kernel answered: `cpu` and guest memory hold the guest's answer
    /// with the frame popped; the caller returns to `target`.
    Answered { target: u32 },
    /// The guest answers; compare its result with the prediction at the
    /// return ([`broadphase_compare`]).
    Audit(P),
    /// The guest answers alone: the call was declined, counted.
    Guest,
}

thread_local! {
    static POSE_OPEN: RefCell<HashMap<u32, motion::Prediction>> = RefCell::new(HashMap::new());
    static FORCES_OPEN: RefCell<HashMap<u32, motion::Prediction>> = RefCell::new(HashMap::new());
    static MOPP_OPEN: RefCell<HashMap<u32, mopp_box::Prediction>> = RefCell::new(HashMap::new());
}

/// The world broadphase batch update's arm, at its entry: capture the
/// prediction and decide who answers. Below the threshold, and at every
/// audit slot, the guest answers and is compared at its return.
pub fn broadphase_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step<broadphase::Prediction> {
    enter(&BROADPHASE, cpu, guest, broadphase::capture, |p, cpu, guest| p.apply(cpu, guest))
}

/// At the guest's return from an audited call.
pub fn broadphase_compare(prediction: &broadphase::Prediction, cpu: &CpuAbi, guest: &Guest) {
    compare(&BROADPHASE, prediction.matches(cpu, guest));
}

fn enter<P>(
    arm: &Arm,
    cpu: &mut CpuAbi,
    guest: &mut Guest,
    capture: impl FnOnce(&CpuAbi, &Guest) -> Option<P>,
    apply: impl FnOnce(&P, &mut CpuAbi, &mut Guest) -> bool,
) -> Step<P> {
    let Some(prediction) = capture(&*cpu, &*guest) else {
        declined(arm);
        return Step::Guest;
    };
    if !arm.answers() {
        return Step::Audit(prediction);
    }
    let Ok(target) = guest.u32(cpu.esp) else {
        declined(arm);
        return Step::Guest;
    };
    if !apply(&prediction, cpu, guest) {
        declined(arm);
        return Step::Guest;
    }
    arm.answered.fetch_add(1, Ordering::Relaxed);
    Step::Answered { target }
}

/// The unconstrained motion's pose method (`vt[+0x08]`, `0x31b1c0`).
pub fn motion_pose_begin(thread: u32, cpu: &CpuAbi, guest: &Guest) {
    POSE_OPEN.with_borrow_mut(|calls| {
        calls.remove(&thread);
        match motion::capture(cpu, guest) {
            Some(prediction) => {
                calls.insert(thread, prediction);
            }
            None => declined(&MOTION_POSE),
        }
    });
}

pub fn motion_pose_end(thread: u32, cpu: &CpuAbi, guest: &Guest) {
    if let Some(prediction) = POSE_OPEN.with_borrow_mut(|calls| calls.remove(&thread)) {
        compare(&MOTION_POSE, prediction.matches(cpu, guest));
    }
}

/// The unconstrained motion's force method (`vt[+0x04]`, `0x31b060`).
pub fn motion_forces_begin(thread: u32, cpu: &CpuAbi, guest: &Guest) {
    FORCES_OPEN.with_borrow_mut(|calls| {
        calls.remove(&thread);
        match motion::capture_forces(cpu, guest) {
            Some(prediction) => {
                calls.insert(thread, prediction);
            }
            None => declined(&MOTION_FORCES),
        }
    });
}

pub fn motion_forces_end(thread: u32, cpu: &CpuAbi, guest: &Guest) {
    if let Some(prediction) = FORCES_OPEN.with_borrow_mut(|calls| calls.remove(&thread)) {
        compare(&MOTION_FORCES, prediction.matches(cpu, guest));
    }
}

/// The integer MOPP box query's arm (phase 2's entry `0x300150`): while the
/// physics switch owns the world step, the query's guest execution is
/// compared with `sim_collision::mopp_box`'s prediction.
pub fn mopp_box_begin(thread: u32, cpu: &CpuAbi, guest: &mut Guest) {
    MOPP_OPEN.with_borrow_mut(|calls| {
        calls.remove(&thread);
        match mopp_box::capture(cpu, guest) {
            Some(prediction) => {
                calls.insert(thread, prediction);
            }
            None => declined(&MOPP_BOX),
        }
    });
}

pub fn mopp_box_end(thread: u32, cpu: &CpuAbi, guest: &Guest) {
    if let Some(prediction) = MOPP_OPEN.with_borrow_mut(|calls| calls.remove(&thread)) {
        compare(&MOPP_BOX, prediction.matches(cpu, guest));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic arm's call: the prediction is the answer's word, and
    /// the guest's answer is `want` written at `ANSWER`.
    const ANSWER: u32 = 0x0001_1000;
    const STACK: u32 = 0x0001_2000;
    const TARGET: u32 = 0x0012_3456;

    fn call() -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0001_0000, 0x4000, runtime::guest::Permissions::RW).unwrap();
        g.set_u32(STACK, TARGET).unwrap();
        (CpuAbi { esp: STACK, eax: 7, ..CpuAbi::default() }, g)
    }

    /// The kernel's prediction: `want` at `ANSWER`, `eax` 1 and the frame
    /// popped; `None` is a decline.
    fn step(arm: &Arm, want: Option<u32>) -> (Step<u32>, CpuAbi, Guest) {
        let (mut cpu, mut g) = call();
        let step = enter(
            arm,
            &mut cpu,
            &mut g,
            |_, _| want,
            |&want: &u32, cpu: &mut CpuAbi, g: &mut Guest| {
                g.set_u32(ANSWER, want).unwrap();
                cpu.eax = 1;
                cpu.esp += 4;
                true
            },
        );
        (step, cpu, g)
    }

    /// Runs one call through the arm, the guest answering `guest` where it
    /// answers; returns whether the kernel answered.
    fn run(arm: &Arm, want: u32, guest: u32) -> bool {
        let (step, cpu, g) = step(arm, Some(want));
        match step {
            Step::Answered { target } => {
                assert_eq!(target, TARGET);
                // The applied state is the guest's answer.
                assert_eq!((g.u32(ANSWER).unwrap(), cpu.eax, cpu.esp), (want, 1, STACK + 4));
                true
            }
            Step::Audit(p) => {
                assert_eq!((g.u32(ANSWER).unwrap(), cpu.eax, cpu.esp), (0, 7, STACK), "an audited call is left alone");
                compare(arm, p == guest);
                false
            }
            Step::Guest => panic!("an answerable call is never declined"),
        }
    }

    #[test]
    fn counts_track_matches_declines_and_the_first_mismatch() {
        static A: Arm = Arm::new("a");
        static B: Arm = Arm::new("b");
        compare(&A, true);
        compare(&A, true);
        declined(&A);
        compare(&B, false);
        let read = |a: &Arm| {
            (a.matched.load(Ordering::Relaxed), a.mismatched.load(Ordering::Relaxed), a.declined.load(Ordering::Relaxed))
        };
        assert_eq!((read(&A), read(&B)), ((2, 0, 1), (0, 1, 0)));
        assert!(B.ended.load(Ordering::Relaxed) && !A.ended.load(Ordering::Relaxed));
    }

    #[test]
    fn status_and_takeover_name_the_arms_in_table_order() {
        let names = status().map(|(name, ..)| name);
        assert_eq!(names, ["physics.broadphase", "physics.motion.pose", "physics.motion.forces", "physics.mopp.box"]);
        assert_eq!(takeover().map(|t| t.name), ["physics.broadphase"]);
    }

    /// The arm's whole life on one synthetic arm: the guest answers below
    /// the threshold; after it the kernel answers all but every
    /// AUDIT_EVERY'th call; one mismatch returns every call to the guest.
    #[test]
    fn the_arm_proves_takes_over_audits_and_stops_on_a_mismatch() {
        static ARM: Arm = Arm::new("test.takeover");
        for _ in 0..PROVEN_AFTER {
            assert!(!ARM.take_over().proven);
            assert!(!run(&ARM, 5, 5), "unproven calls are the guest's");
        }
        assert!(ARM.take_over().proven);
        assert_eq!(ARM.take_over().answered, 0);

        // Slot PROVEN_AFTER is a multiple of AUDIT_EVERY: the guest's.
        let answered: Vec<bool> = (0..2 * AUDIT_EVERY).map(|_| run(&ARM, 5, 5)).collect();
        assert_eq!(answered.iter().filter(|&&a| !a).count(), 2);
        assert!(!answered[0] && !answered[AUDIT_EVERY as usize]);
        assert_eq!(ARM.take_over().answered, 2 * AUDIT_EVERY - 2);

        // A decline never answers, and takes no slot.
        let slots = ARM.slots.load(Ordering::Relaxed);
        let (decline, cpu, g) = step(&ARM, None);
        assert!(matches!(decline, Step::Guest));
        assert_eq!((g.u32(ANSWER).unwrap(), cpu.eax, cpu.esp), (0, 7, STACK));
        assert_eq!(ARM.slots.load(Ordering::Relaxed), slots);
        assert_eq!(ARM.declined.load(Ordering::Relaxed), 1);

        // A mismatch at the next audit ends the take-over.
        while run(&ARM, 5, 6) {}
        assert_eq!(ARM.mismatched.load(Ordering::Relaxed), 1);
        assert_eq!(ARM.take_over(), Takeover { name: "test.takeover", answered: ARM.take_over().answered, proven: false, ended: true });
        let answered = ARM.take_over().answered;
        for _ in 0..4 * AUDIT_EVERY {
            assert!(!run(&ARM, 5, 5));
        }
        assert_eq!(ARM.take_over().answered, answered);

        // The map load starts the proof over: the guest answers again
        // until PROVEN_AFTER matches, from slot zero.
        ARM.restart_proof();
        assert_eq!(ARM.take_over(), Takeover { name: "test.takeover", answered, proven: false, ended: false });
        for _ in 0..PROVEN_AFTER {
            assert!(!run(&ARM, 5, 5));
        }
        // Slot PROVEN_AFTER after the load is an audit; the next answers.
        assert!(!run(&ARM, 5, 5));
        assert!(run(&ARM, 5, 5));
    }

    /// Two peers whose processes ran different matches before cross the
    /// threshold on the same call of the next one.
    #[test]
    fn peers_with_different_histories_agree_after_a_map_load() {
        static FRESH: Arm = Arm::new("test.fresh");
        static SEASONED: Arm = Arm::new("test.seasoned");
        for _ in 0..PROVEN_AFTER + 17 {
            run(&SEASONED, 5, 5);
        }
        while run(&SEASONED, 5, 6) {}
        assert!(SEASONED.take_over().ended);
        for arm in [&FRESH, &SEASONED] {
            arm.restart_proof();
        }
        for call in 0..PROVEN_AFTER + 3 * AUDIT_EVERY {
            assert_eq!(run(&FRESH, 5, 5), run(&SEASONED, 5, 5), "call {call}");
        }
    }

    #[test]
    fn map_loaded_restarts_every_arms_proof() {
        // Only the per-load state of the statics: the totals other tests
        // read are left alone.
        map_loaded();
        for arm in ARMS {
            assert_eq!((arm.proof.load(Ordering::Relaxed), arm.slots.load(Ordering::Relaxed)), (0, 0), "{}", arm.name);
            assert!(!arm.ended.load(Ordering::Relaxed), "{}", arm.name);
        }
    }
}
