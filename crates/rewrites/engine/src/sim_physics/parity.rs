//! The physics capture arms (sim-rewrite-havok.md §7.4, "parity arm"): the
//! guest answers, and the Rust kernel's prediction of every write, register
//! and flag is compared with what the guest left when it returns. One open
//! prediction per arm and thread; a new capture replaces a stale one.
//! Every fallback is counted, never silent (§8).

use super::{broadphase, motion};
use crate::sim_collision::mopp_box;
use runtime::{CpuAbi, Guest};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// One arm's outcome counts. `declined` counts captures that refused the
/// state (float environment, aliasing, capacity) and left the guest sole
/// authority for that call.
pub struct Arm {
    pub name: &'static str,
    matched: AtomicU64,
    mismatched: AtomicU64,
    declined: AtomicU64,
}

impl Arm {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            matched: AtomicU64::new(0),
            mismatched: AtomicU64::new(0),
            declined: AtomicU64::new(0),
        }
    }
}

pub static BROADPHASE: Arm = Arm::new("physics.broadphase");
pub static MOTION_POSE: Arm = Arm::new("physics.motion.pose");
pub static MOTION_FORCES: Arm = Arm::new("physics.motion.forces");
pub static MOPP_BOX: Arm = Arm::new("physics.mopp.box");

pub(super) fn declined(arm: &'static Arm) {
    arm.declined.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn compare(arm: &'static Arm, matched: bool) {
    if matched {
        arm.matched.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let seen = arm.mismatched.fetch_add(1, Ordering::Relaxed);
    if seen == 0 {
        eprintln!(
            "sim: physics {} arm: the guest's result differs from the prediction",
            arm.name
        );
    }
}

/// The arms' counts, in table order, for tests and reports.
pub fn status() -> [(&'static str, u64, u64, u64); 4] {
    let read = |a: &Arm| {
        (
            a.name,
            a.matched.load(Ordering::Relaxed),
            a.mismatched.load(Ordering::Relaxed),
            a.declined.load(Ordering::Relaxed),
        )
    };
    [
        read(&BROADPHASE),
        read(&MOTION_POSE),
        read(&MOTION_FORCES),
        read(&MOPP_BOX),
    ]
}

thread_local! {
    static BROADPHASE_OPEN: RefCell<HashMap<u32, broadphase::Prediction>> = RefCell::new(HashMap::new());
    static POSE_OPEN: RefCell<HashMap<u32, motion::Prediction>> = RefCell::new(HashMap::new());
    static FORCES_OPEN: RefCell<HashMap<u32, motion::Prediction>> = RefCell::new(HashMap::new());
    static MOPP_OPEN: RefCell<HashMap<u32, mopp_box::Prediction>> = RefCell::new(HashMap::new());
}

/// The world broadphase batch update's arm.
pub fn broadphase_begin(thread: u32, cpu: &CpuAbi, guest: &Guest) {
    BROADPHASE_OPEN.with_borrow_mut(|calls| {
        calls.remove(&thread);
        match broadphase::capture(cpu, guest) {
            Some(prediction) => {
                calls.insert(thread, prediction);
            }
            None => declined(&BROADPHASE),
        }
    });
}

pub fn broadphase_end(thread: u32, cpu: &CpuAbi, guest: &Guest) {
    if let Some(prediction) = BROADPHASE_OPEN.with_borrow_mut(|calls| calls.remove(&thread)) {
        compare(&BROADPHASE, prediction.matches(cpu, guest));
    }
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

    #[test]
    fn counts_track_matches_declines_and_the_first_mismatch() {
        // Separate arms keep their counts; the status is table order.
        compare(&BROADPHASE, true);
        compare(&BROADPHASE, true);
        declined(&BROADPHASE);
        compare(&MOTION_POSE, false);
        let status = status();
        assert_eq!(status[0], ("physics.broadphase", 2, 0, 1));
        assert_eq!(status[1], ("physics.motion.pose", 0, 1, 0));
    }
}
