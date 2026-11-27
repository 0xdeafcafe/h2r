//! A part arm's proof, the pattern of the curve evaluator's
//! (`engine::sim_objects::functions::parity`). One [`Arm`] each for the
//! damage, object and sound parts, over its own prediction type.
//!
//! At each part the arm predicts what the guest's arm will pass to its
//! leaves. Until it is proven the guest runs the switch's arm and the
//! prediction is compared where the guest calls the leaf. After
//! [`PROVEN_AFTER`] matched parts and no mismatch, the arm answers: Rust
//! makes the leaf calls with its own values, and one answerable part in
//! [`AUDIT_EVERY`] still goes to the guest and is compared. One mismatch
//! stops answering until the next map load ([`Arm::reset`]); the guest
//! answers every part from then on.

/// Matched parts, with no mismatch, before the arm answers.
pub const PROVEN_AFTER: u64 = 4096;

/// After that, one answerable part in this many is still the guest's and
/// compared.
pub const AUDIT_EVERY: u64 = 64;

/// The arm's counters since the map load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Parts the guest answered and the arm compared at its leaf.
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    /// Parts Rust answered.
    pub answered: u64,
    /// Refusals: a guest read faulted, so the guest answered alone.
    pub declined_fault: u64,
    /// A prediction the guest never reached the leaf for (a checkpoint
    /// restore between the entry and the call, or an arm that returned
    /// early); not a mismatch.
    pub unreached: u64,
    /// Where the first mismatch was, as the prediction type numbers it
    /// ([`Prediction::first_difference`]).
    pub first_mismatch_at: Option<usize>,
}

impl Stats {
    pub const ZERO: Stats = Stats {
        checked: 0,
        matched: 0,
        mismatched: 0,
        answered: 0,
        declined_fault: 0,
        unreached: 0,
        first_mismatch_at: None,
    };

    pub fn declined(&self) -> u64 {
        self.declined_fault + self.unreached
    }
}

/// What an arm predicts and compares.
pub trait Prediction: Clone {
    /// The first place `self` and `seen` differ, numbered by the type.
    fn first_difference(&self, seen: &Self) -> Option<usize>;
}

/// Who answers a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Rust makes the leaf calls with its prediction.
    Answer,
    /// The guest's arm runs; compare its record with the prediction.
    Audit,
}

/// The arm: its counters, the audit slot and the one pending prediction.
#[derive(Clone, Debug)]
pub struct Arm<T> {
    stats: Stats,
    answerable: u64,
    pending: Option<T>,
}

impl<T> Default for Arm<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arm<T> {
    pub const fn new() -> Self {
        Arm { stats: Stats::ZERO, answerable: 0, pending: None }
    }
}

impl<T: Prediction> Arm<T> {

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Matched enough with no mismatch.
    pub fn proven(&self) -> bool {
        self.stats.mismatched == 0 && self.stats.matched >= PROVEN_AFTER
    }

    /// A part with its prediction: answer it, or let the guest run and
    /// keep the prediction for [`Arm::seen`].
    pub fn decide(&mut self, prediction: T) -> Decision {
        let slot = self.answerable;
        self.answerable += 1;
        if self.proven() && slot % AUDIT_EVERY != 0 {
            self.stats.answered += 1;
            return Decision::Answer;
        }
        if self.pending.replace(prediction).is_some() {
            self.stats.unreached += 1;
        }
        Decision::Audit
    }

    /// The guest reached the leaf from the arm: compare. Returns whether it
    /// matched; a call with no prediction pending is ignored (`None`).
    pub fn seen(&mut self, seen: &T) -> Option<bool> {
        let want = self.pending.take()?;
        Some(self.verdict(want.first_difference(seen)))
    }

    /// The pending prediction, for an arm compared in stages.
    pub fn pending_mut(&mut self) -> Option<&mut T> {
        self.pending.as_mut()
    }

    /// Ends a staged comparison: the pending prediction is dropped and the
    /// result counted (`None` matched, `Some(at)` differed at `at`).
    pub fn verdict(&mut self, at: Option<usize>) -> bool {
        self.pending = None;
        self.stats.checked += 1;
        match at {
            None => {
                self.stats.matched += 1;
                true
            }
            Some(at) => {
                self.stats.mismatched += 1;
                self.stats.first_mismatch_at.get_or_insert(at);
                false
            }
        }
    }

    /// A refused part, by its reason.
    pub fn declined_fault(&mut self) {
        self.stats.declined_fault += 1;
    }

    /// A new map: the proof is earned again, a stopped arm starts over.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::{self, Call};

    fn rec(n: u8) -> Call {
        let mut record = [0u8; damage::SIZE];
        record[0] = n;
        Call { record, except: damage::NO_OBJECT }
    }

    type Arm = super::Arm<Call>;

    #[test]
    fn the_arm_answers_after_its_clean_count_and_audits_one_in_64() {
        let mut arm = Arm::new();
        for n in 0..PROVEN_AFTER {
            assert_eq!(arm.decide(rec(n as u8)), Decision::Audit);
            assert_eq!(arm.seen(&rec(n as u8)), Some(true));
        }
        assert!(arm.proven());
        let mut audits = 0;
        for n in 0..640u64 {
            match arm.decide(rec(1)) {
                Decision::Answer => {}
                Decision::Audit => {
                    audits += 1;
                    arm.seen(&rec(1));
                }
            }
            let _ = n;
        }
        assert_eq!(audits, 10);
        assert_eq!(arm.stats().answered, 630);
    }

    #[test]
    fn one_difference_returns_every_part_to_the_guest() {
        let mut arm = Arm::new();
        for _ in 0..PROVEN_AFTER {
            arm.decide(rec(2));
            arm.seen(&rec(2));
        }
        // An audit that differs in the second argument.
        while arm.decide(rec(2)) != Decision::Audit {}
        assert_eq!(arm.seen(&Call { except: 7, ..rec(2) }), Some(false));
        assert_eq!(arm.stats().first_mismatch_at, Some(damage::SIZE));
        for _ in 0..1000 {
            assert_eq!(arm.decide(rec(2)), Decision::Audit);
            arm.seen(&rec(2));
        }
        arm.reset();
        assert_eq!(arm.stats(), Stats::default());
    }

    #[test]
    fn an_unreached_prediction_is_counted_not_compared() {
        let mut arm = Arm::new();
        arm.decide(rec(3));
        arm.decide(rec(4));
        assert_eq!(arm.stats().unreached, 1);
        assert_eq!(arm.seen(&rec(4)), Some(true));
        assert_eq!(arm.seen(&rec(4)), None);
        assert_eq!((arm.stats().checked, arm.stats().mismatched), (1, 0));
    }
}
