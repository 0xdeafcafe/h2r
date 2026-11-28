//! The physics arms (sim-rewrite-havok.md §7.4): the Rust kernel computes
//! every write, register and flag of a call. Every decline is counted by
//! reason, never silent (§8).
//!
//! **Answering arms.** The broadphase batch update (`broadphase`), the
//! island integrate (`0x316a20`, `island_integrate`: the island solve or
//! each entity's unconstrained virtuals, on a journal), the deactivation
//! test (`0x316b30`, `deactivation`), the integrate phase (`0x30fef0`,
//! `integrate_phase`: every island's test and integrate in the phase's
//! order), the structure polygon builder (`0x1edd00`, `structure_polygon`),
//! the convex agent's process call (`0x2eeb40`, `convex_agent`: the
//! separated-pair path), the closest-feature query (`0x3048a0`,
//! `convex_query`: the whole query, the triangle case and its penetration
//! call included) and the add-point call (`0x302c70`, `contact_point`: a
//! point that moves the slot it matches) answer every call their model
//! captures, from the first call of the process. There is no proof
//! window and no guest audit: bit-exactness is the differential tests'
//! job (each module's `matches` against the lifted entry). A call the
//! model refuses is counted on the arm and by reason in its
//! `sim_havok::*::fallbacks`, and runs the lifted entry.
//!
//! The contact commit (`0x318c10`, `contact_commit`, `sim_havok::commit`)
//! answers the call up to its world listener walk (`0x318320`), whose
//! listeners are Halo's: [`commit_enter`] leaves the call at the walk's
//! entry for the caller to make in place.
//!
//! The two motion arms and the MOPP box arm compare only. A motion
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
use std::sync::atomic::{AtomicU64, Ordering};

/// The stack bytes the broadphase batch update's `ret 0x14` pops, besides
/// the return address.
pub const BROADPHASE_ARGS: u32 = 0x14;

/// One arm's outcome counts. `matched` and `mismatched` count the
/// compare-only arms' calls; `answered` counts the calls an answering arm
/// returned from Rust; `declined` counts calls the model refused (float
/// environment, aliasing, capacity, an unmodelled case) or whose apply
/// refused its spans, which ran the lifted entry.
pub struct Arm {
    pub name: &'static str,
    matched: AtomicU64,
    mismatched: AtomicU64,
    declined: AtomicU64,
    answered: AtomicU64,
}

impl Arm {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            matched: AtomicU64::new(0),
            mismatched: AtomicU64::new(0),
            declined: AtomicU64::new(0),
            answered: AtomicU64::new(0),
        }
    }

    fn take_over(&self) -> Takeover {
        Takeover { name: self.name, answered: self.answered.load(Ordering::Relaxed) }
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

pub(super) fn compare(arm: &Arm, matched: bool) {
    let counter = if matched { &arm.matched } else { &arm.mismatched };
    counter.fetch_add(1, Ordering::Relaxed);
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

/// The calls an answering arm returned from Rust since start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Takeover {
    pub name: &'static str,
    pub answered: u64,
}

/// The answering arms' counts: the broadphase batch update's, the island
/// integrate's, the deactivation test's, the integrate phase's, the
/// structure polygon builder's, the convex agent's, the closest-feature
/// query's, the add-point call's and the contact commit's.
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

/// What an answering arm did with a call.
pub enum Step {
    /// The kernel answered: `cpu` and guest memory hold the answer with
    /// the frame popped; the caller returns to `target`.
    Answered { target: u32 },
    /// The model refused the call, counted: the lifted entry runs it.
    Guest,
}

thread_local! {
    static POSE_OPEN: RefCell<HashMap<u32, motion::Prediction>> = RefCell::new(HashMap::new());
    static FORCES_OPEN: RefCell<HashMap<u32, motion::Prediction>> = RefCell::new(HashMap::new());
    static MOPP_OPEN: RefCell<HashMap<u32, mopp_box::Prediction>> = RefCell::new(HashMap::new());
}

/// The world broadphase batch update's arm, at its entry: compute the call
/// and answer it.
pub fn broadphase_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
    enter(&BROADPHASE, cpu, guest, broadphase::capture, |p, cpu, guest| p.apply(cpu, guest))
}

fn enter<P>(
    arm: &Arm,
    cpu: &mut CpuAbi,
    guest: &mut Guest,
    capture: impl FnOnce(&CpuAbi, &Guest) -> Option<P>,
    apply: impl FnOnce(&P, &mut CpuAbi, &mut Guest) -> bool,
) -> Step {
    let Some(prediction) = capture(&*cpu, &*guest) else {
        declined(arm);
        return Step::Guest;
    };
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
pub fn integrate_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
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

/// The deactivation test's arm, at its entry: as [`integrate_enter`].
pub fn deactivation_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
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

/// The integrate phase's arm, at its entry: as [`integrate_enter`]. Each
/// island an answered phase solved in Rust is counted in `havok.islands`.
pub fn phase_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
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

/// The structure polygon builder's arm, at its entry: as [`integrate_enter`].
pub fn polygon_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
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

/// The convex agent's process call's arm, at its entry: as [`integrate_enter`].
pub fn convex_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
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

/// The closest-feature query's arm, at its entry: as [`integrate_enter`].
pub fn query_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
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

/// The add-point call's arm, at its entry: as [`integrate_enter`].
pub fn point_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
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

/// What [`commit_enter`] did with a contact commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    /// The call is at its world listener walk's entry: the stores made,
    /// the registers set, the return to `sim_havok::commit::WALK_SITE`
    /// pushed. The caller calls the walk and runs the guest's tail.
    Answered,
    /// The guest answers alone: the call was declined, counted.
    Guest,
}

/// The contact commit's arm, at its entry: as [`integrate_enter`], except
/// that an answer stops at the world listener walk.
pub fn commit_enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Commit {
    use sim_havok::commit as model;
    let prediction = match contact_commit::capture(cpu, guest) {
        Ok(p) => p,
        Err(f) => {
            model::count(f);
            declined(&CONTACT_COMMIT);
            return Commit::Guest;
        }
    };
    if !prediction.apply(cpu, guest) {
        model::count(model::Fallback::Memory);
        declined(&CONTACT_COMMIT);
        return Commit::Guest;
    }
    CONTACT_COMMIT.answered.fetch_add(1, Ordering::Relaxed);
    Commit::Answered
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

    /// The kernel's answer: `want` at `ANSWER`, `eax` 1 and the frame
    /// popped; `None` is a decline.
    fn step(arm: &Arm, want: Option<u32>) -> (Step, CpuAbi, Guest) {
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

    #[test]
    fn counts_track_matches_mismatches_and_declines() {
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

    /// An answering arm answers its first call and every call after it;
    /// the guest is never asked. A decline leaves the call untouched for
    /// the lifted entry and is counted.
    #[test]
    fn the_arm_answers_from_the_first_call() {
        static ARM: Arm = Arm::new("test.answer");
        for n in 1..=3 {
            let (step, cpu, g) = step(&ARM, Some(5));
            let Step::Answered { target } = step else { panic!("call {n} declined") };
            assert_eq!(target, TARGET);
            assert_eq!((g.u32(ANSWER).unwrap(), cpu.eax, cpu.esp), (5, 1, STACK + 4));
            assert_eq!(ARM.take_over(), Takeover { name: "test.answer", answered: n });
        }
        let (decline, cpu, g) = step(&ARM, None);
        assert!(matches!(decline, Step::Guest));
        assert_eq!((g.u32(ANSWER).unwrap(), cpu.eax, cpu.esp), (0, 7, STACK));
        assert_eq!(ARM.declined.load(Ordering::Relaxed), 1);
        assert_eq!(ARM.take_over().answered, 3);
        // No answer is compared: the arm's matched and mismatched stay zero.
        assert_eq!((ARM.matched.load(Ordering::Relaxed), ARM.mismatched.load(Ordering::Relaxed)), (0, 0));
    }
}
