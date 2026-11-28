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
//! The island integrate's arm (`0x316a20`) takes over the same way: its
//! prediction is the whole entry, the island solve or each entity's
//! unconstrained virtuals, run on a journal (`island_integrate`). Its
//! declines are also counted by reason
//! (`sim_havok::island_integrate::fallbacks`, read with `havok.islands`).
//! The deactivation test's arm (`0x316b30`, `deactivation`) and the
//! integrate phase's (`0x30fef0`, `integrate_phase`), whose prediction is
//! every island's test and integrate in the phase's order, take over the
//! same way, each with its own reasons (`sim_havok::deactivation`,
//! `sim_havok::integrate_phase`), and so do the structure polygon
//! builder's (`0x1edd00`, `structure_polygon`, `sim_havok::structure`) and
//! the convex agent's process call's (`0x2eeb40`, `convex_agent`,
//! `sim_havok::convex`), which answers the separated-pair path, and the
//! closest-feature query's (`0x3048a0`, `convex_query`,
//! `sim_havok::closest`), which answers the whole query, the triangle case
//! and its penetration call included, and the add-point call's
//! (`0x302c70`, `contact_point`, `sim_havok::contact_point`), which
//! answers a point that moves the slot it matches.
//!
//! The contact commit's arm (`0x318c10`, `contact_commit`,
//! `sim_havok::commit`) answers the call up to its world listener walk
//! (`0x318320`), whose listeners are Halo's: [`commit_enter`] leaves the
//! call at the walk's entry for the caller to make in place, and an
//! audited commit is compared when the guest's walk is entered from the
//! commit ([`commit_walk`]).
//!
//! The two motion arms and the MOPP box arm stay compare-only. A motion
//! prediction holds `esp`, the four callee-saved registers and the body's
//! `0x110` bytes; what retail leaves in `eax`, `ecx`, `edx`, the XMM
//! registers, the x87 stack, status and tag words, the MXCSR flags and
//! `eflags` is not modelled, and no differential test pins it, so an
//! applied motion could not be shown to leave the guest's registers.

use super::{broadphase, contact_commit, contact_point, convex_agent, convex_query, deactivation, integrate_phase, island_integrate, motion, structure_polygon};
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
pub static ISLAND_INTEGRATE: Arm = Arm::new("physics.island.integrate");
pub static DEACTIVATION: Arm = Arm::new("physics.island.deactivation");
pub static INTEGRATE_PHASE: Arm = Arm::new("physics.integrate");
pub static STRUCTURE_POLYGON: Arm = Arm::new("physics.structure.polygon");
pub static CONVEX_AGENT: Arm = Arm::new("physics.agent.convex");
pub static CONVEX_QUERY: Arm = Arm::new("physics.agent.query");
pub static CONTACT_POINT: Arm = Arm::new("physics.agent.point");
pub static CONTACT_COMMIT: Arm = Arm::new("physics.contact.commit");

static ARMS: [&Arm; 12] = [
    &BROADPHASE,
    &MOTION_POSE,
    &MOTION_FORCES,
    &MOPP_BOX,
    &ISLAND_INTEGRATE,
    &DEACTIVATION,
    &INTEGRATE_PHASE,
    &STRUCTURE_POLYGON,
    &CONVEX_AGENT,
    &CONVEX_QUERY,
    &CONTACT_POINT,
    &CONTACT_COMMIT,
];

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
pub fn status() -> [(&'static str, u64, u64, u64); 12] {
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
/// batch update's, the island integrate's, the deactivation test's, the
/// integrate phase's, the structure polygon builder's, the convex agent's,
/// the closest-feature query's, the add-point call's and the contact
/// commit's.
pub fn takeover() -> [Takeover; 9] {
    [
        BROADPHASE.take_over(),
        ISLAND_INTEGRATE.take_over(),
        DEACTIVATION.take_over(),
        INTEGRATE_PHASE.take_over(),
        STRUCTURE_POLYGON.take_over(),
        CONVEX_AGENT.take_over(),
        CONVEX_QUERY.take_over(),
        CONTACT_POINT.take_over(),
        CONTACT_COMMIT.take_over(),
    ]
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
    /// The audited commit whose walk this thread has not entered yet.
    static COMMIT_OPEN: RefCell<Option<contact_commit::Prediction>> = const { RefCell::new(None) };
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

/// The island integrate's arm, at its entry: as [`broadphase_enter`]. A
/// decline is counted by reason as well as on the arm.
pub fn integrate_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step<island_integrate::Prediction> {
    use sim_havok::{island_integrate as model, island_solve};
    enter(
        &ISLAND_INTEGRATE,
        cpu,
        guest,
        |cpu, guest| island_integrate::capture(cpu, guest).map_err(model::count).ok(),
        |p, cpu, guest| {
            if !p.apply(cpu, guest) {
                model::count(model::Fallback::Memory);
                return false;
            }
            // The island's solve ran in Rust: `havok.islands` counts it.
            if p.solved {
                island_solve::count_solved();
            }
            true
        },
    )
}

/// At the guest's return from an audited integrate.
pub fn integrate_compare(prediction: &island_integrate::Prediction, cpu: &CpuAbi, guest: &Guest) {
    compare(&ISLAND_INTEGRATE, prediction.matches(cpu, guest));
}

/// An audited integrate whose return could not be observed.
pub fn integrate_uncompared() {
    sim_havok::island_integrate::count(sim_havok::island_integrate::Fallback::Unobserved);
    uncompared(&ISLAND_INTEGRATE);
}

/// The deactivation test's arm, at its entry: as [`integrate_enter`].
pub fn deactivation_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step<deactivation::Prediction> {
    use sim_havok::deactivation as model;
    enter(
        &DEACTIVATION,
        cpu,
        guest,
        |cpu, guest| deactivation::capture(cpu, guest).map_err(model::count).ok(),
        |p, cpu, guest| {
            let applied = p.apply(cpu, guest);
            if !applied {
                model::count(model::Fallback::Memory);
            }
            applied
        },
    )
}

/// At the guest's return from an audited deactivation test.
pub fn deactivation_compare(prediction: &deactivation::Prediction, cpu: &CpuAbi, guest: &Guest) {
    compare(&DEACTIVATION, prediction.matches(cpu, guest));
}

/// An audited deactivation test whose return could not be observed.
pub fn deactivation_uncompared() {
    sim_havok::deactivation::count(sim_havok::deactivation::Fallback::Unobserved);
    uncompared(&DEACTIVATION);
}

/// The integrate phase's arm, at its entry: as [`integrate_enter`]. Each
/// island an answered phase solved in Rust is counted in `havok.islands`.
pub fn phase_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step<integrate_phase::Prediction> {
    use sim_havok::{integrate_phase as model, island_solve};
    enter(
        &INTEGRATE_PHASE,
        cpu,
        guest,
        |cpu, guest| integrate_phase::capture(cpu, guest).map_err(model::count).ok(),
        |p, cpu, guest| {
            if !p.apply(cpu, guest) {
                model::count(model::Fallback::Memory);
                return false;
            }
            for _ in 0..p.outcome.solved {
                island_solve::count_solved();
            }
            true
        },
    )
}

/// At the guest's return from an audited integrate phase.
pub fn phase_compare(prediction: &integrate_phase::Prediction, cpu: &CpuAbi, guest: &Guest) {
    compare(&INTEGRATE_PHASE, prediction.matches(cpu, guest));
}

/// An audited integrate phase whose return could not be observed.
pub fn phase_uncompared() {
    sim_havok::integrate_phase::count(sim_havok::integrate_phase::Fallback::Unobserved);
    uncompared(&INTEGRATE_PHASE);
}

/// The structure polygon builder's arm, at its entry: as [`integrate_enter`].
pub fn polygon_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step<structure_polygon::Prediction> {
    use sim_havok::structure as model;
    enter(
        &STRUCTURE_POLYGON,
        cpu,
        guest,
        |cpu, guest| structure_polygon::capture(cpu, guest).map_err(model::count).ok(),
        |p, cpu, guest| {
            let applied = p.apply(cpu, guest);
            if !applied {
                model::count(model::Fallback::Memory);
            }
            applied
        },
    )
}

/// At the guest's return from an audited build.
pub fn polygon_compare(prediction: &structure_polygon::Prediction, cpu: &CpuAbi, guest: &Guest) {
    compare(&STRUCTURE_POLYGON, prediction.matches(cpu, guest));
}

/// An audited build whose return could not be observed.
pub fn polygon_uncompared() {
    sim_havok::structure::count(sim_havok::structure::Fallback::Unobserved);
    uncompared(&STRUCTURE_POLYGON);
}

/// The convex agent's process call's arm, at its entry: as [`integrate_enter`].
pub fn convex_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step<convex_agent::Prediction> {
    use sim_havok::convex as model;
    enter(
        &CONVEX_AGENT,
        cpu,
        guest,
        |cpu, guest| convex_agent::capture(cpu, guest).map_err(model::count).ok(),
        |p, cpu, guest| {
            let applied = p.apply(cpu, guest);
            if !applied {
                model::count(model::Fallback::Memory);
            }
            applied
        },
    )
}

/// At the guest's return from an audited process call.
pub fn convex_compare(prediction: &convex_agent::Prediction, cpu: &CpuAbi, guest: &Guest) {
    compare(&CONVEX_AGENT, prediction.matches(cpu, guest));
}

/// An audited process call whose return could not be observed.
pub fn convex_uncompared() {
    sim_havok::convex::count(sim_havok::convex::Fallback::Unobserved);
    uncompared(&CONVEX_AGENT);
}

/// The closest-feature query's arm, at its entry: as [`integrate_enter`].
pub fn query_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step<convex_query::Prediction> {
    use sim_havok::closest as model;
    enter(
        &CONVEX_QUERY,
        cpu,
        guest,
        |cpu, guest| convex_query::capture(cpu, guest).map_err(model::count).ok(),
        |p, cpu, guest| {
            let applied = p.apply(cpu, guest);
            if !applied {
                model::count(model::Fallback::Memory);
            }
            applied
        },
    )
}

/// At the guest's return from an audited query.
pub fn query_compare(prediction: &convex_query::Prediction, cpu: &CpuAbi, guest: &Guest) {
    compare(&CONVEX_QUERY, prediction.matches(cpu, guest));
}

/// An audited query whose return could not be observed.
pub fn query_uncompared() {
    sim_havok::closest::count(sim_havok::closest::Fallback::Unobserved);
    uncompared(&CONVEX_QUERY);
}

/// The add-point call's arm, at its entry: as [`integrate_enter`].
pub fn point_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step<contact_point::Prediction> {
    use sim_havok::contact_point as model;
    enter(
        &CONTACT_POINT,
        cpu,
        guest,
        |cpu, guest| contact_point::capture(cpu, guest).map_err(model::count).ok(),
        |p, cpu, guest| {
            let applied = p.apply(cpu, guest);
            if !applied {
                model::count(model::Fallback::Memory);
            }
            applied
        },
    )
}

/// At the guest's return from an audited add-point call.
pub fn point_compare(prediction: &contact_point::Prediction, cpu: &CpuAbi, guest: &Guest) {
    compare(&CONTACT_POINT, prediction.matches(cpu, guest));
}

/// An audited add-point call whose return could not be observed.
pub fn point_uncompared() {
    sim_havok::contact_point::count(sim_havok::contact_point::Fallback::Unobserved);
    uncompared(&CONTACT_POINT);
}

/// What [`commit_enter`] did with a contact commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    /// The call is at its world listener walk's entry: the stores made,
    /// the registers set, the return to `sim_havok::commit::WALK_SITE`
    /// pushed. The caller calls the walk and runs the guest's tail.
    Answered,
    /// The guest answers; its walk's entry compares ([`commit_walk`]).
    Audit,
    /// The guest answers alone: the call was declined, counted.
    Guest,
}

/// A prediction left open by an earlier commit: its walk was never
/// entered.
fn commit_drop_open() {
    if COMMIT_OPEN.with_borrow_mut(|open| open.take()).is_some() {
        sim_havok::commit::count(sim_havok::commit::Fallback::Unobserved);
        uncompared(&CONTACT_COMMIT);
    }
}

/// The contact commit's arm, at its entry: as [`integrate_enter`], except
/// that an answer stops at the world listener walk.
pub fn commit_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Commit {
    use sim_havok::commit as model;
    commit_drop_open();
    let prediction = match contact_commit::capture(cpu, guest) {
        Ok(p) => p,
        Err(f) => {
            model::count(f);
            declined(&CONTACT_COMMIT);
            return Commit::Guest;
        }
    };
    if !CONTACT_COMMIT.answers() {
        COMMIT_OPEN.with_borrow_mut(|open| *open = Some(prediction));
        return Commit::Audit;
    }
    if !prediction.apply(cpu, guest) {
        model::count(model::Fallback::Memory);
        declined(&CONTACT_COMMIT);
        return Commit::Guest;
    }
    CONTACT_COMMIT.answered.fetch_add(1, Ordering::Relaxed);
    Commit::Answered
}

/// At the world listener walk's entry (`0x318320`): when it was called
/// from an audited commit's site with the stack the prediction holds, the
/// prediction is compared. Any other caller leaves it open.
pub fn commit_walk(cpu: &CpuAbi, guest: &Guest) {
    if guest.u32(cpu.esp).ok() != Some(sim_havok::commit::WALK_SITE) {
        return;
    }
    let Some(prediction) = COMMIT_OPEN.with_borrow_mut(|open| open.take()) else { return };
    if cpu.esp != prediction.head.walk_esp() {
        sim_havok::commit::count(sim_havok::commit::Fallback::Unobserved);
        uncompared(&CONTACT_COMMIT);
        return;
    }
    compare(&CONTACT_COMMIT, prediction.matches(cpu, guest));
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
        assert_eq!(
            names,
            [
                "physics.broadphase",
                "physics.motion.pose",
                "physics.motion.forces",
                "physics.mopp.box",
                "physics.island.integrate",
                "physics.island.deactivation",
                "physics.integrate",
                "physics.structure.polygon",
                "physics.agent.convex",
                "physics.agent.query",
                "physics.agent.point",
                "physics.contact.commit"
            ]
        );
        assert_eq!(
            takeover().map(|t| t.name),
            [
                "physics.broadphase",
                "physics.island.integrate",
                "physics.island.deactivation",
                "physics.integrate",
                "physics.structure.polygon",
                "physics.agent.convex",
                "physics.agent.query",
                "physics.agent.point",
                "physics.contact.commit"
            ]
        );
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
