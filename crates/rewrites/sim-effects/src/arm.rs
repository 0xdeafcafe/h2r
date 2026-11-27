//! The damage arm's proof, the pattern of the curve evaluator's
//! (`engine::sim_objects::functions::parity`).
//!
//! At each `jpt!` part the arm builds the record ([`crate::damage`]). Until
//! it is proven the guest runs the switch's arm and the prediction is
//! compared with the record the guest passes to `0xd6c80`. After
//! [`PROVEN_AFTER`] matched parts and no mismatch, the arm answers: Rust
//! calls `0xd6c80` with its own record, and one answerable part in
//! [`AUDIT_EVERY`] still goes to the guest and is compared. One mismatch
//! stops answering until the next map load ([`Arm::reset`]); the guest
//! answers every part from then on.

use crate::damage::{self, Record};

/// Matched parts, with no mismatch, before the arm answers.
pub const PROVEN_AFTER: u64 = 4096;

/// After that, one answerable part in this many is still the guest's and
/// compared.
pub const AUDIT_EVERY: u64 = 64;

/// The arm's counters since the map load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Parts the guest answered and the arm compared at `0xd6c80`.
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    /// Parts Rust answered.
    pub answered: u64,
    /// Refusals: a guest read faulted, so the guest answered alone.
    pub declined_fault: u64,
    /// A prediction the guest never reached `0xd6c80` for (a checkpoint
    /// restore between the entry and the call); not a mismatch.
    pub unreached: u64,
    /// The first mismatch's byte offset in the record (`SIZE` for the
    /// second argument).
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

/// Who answers a damage part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Rust calls `0xd6c80` with the predicted record.
    Answer,
    /// The guest's arm runs; compare its record with the prediction.
    Audit,
}

/// What the guest passed to `0xd6c80`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub record: Record,
    pub except: u32,
}

/// The arm: its counters, the audit slot and the one pending prediction.
#[derive(Clone, Debug, Default)]
pub struct Arm {
    stats: Stats,
    answerable: u64,
    pending: Option<Record>,
}

impl Arm {
    pub const fn new() -> Self {
        Arm { stats: Stats::ZERO, answerable: 0, pending: None }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Matched enough with no mismatch.
    pub fn proven(&self) -> bool {
        self.stats.mismatched == 0 && self.stats.matched >= PROVEN_AFTER
    }

    /// A damage part with its prediction: answer it, or let the guest run
    /// and keep the prediction for [`Arm::seen`].
    pub fn decide(&mut self, prediction: Record) -> Decision {
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

    /// The guest reached `0xd6c80` from the arm: compare. Returns whether
    /// it matched; a call with no prediction pending is ignored (`None`).
    pub fn seen(&mut self, seen: &Seen) -> Option<bool> {
        let want = self.pending.take()?;
        self.stats.checked += 1;
        let at = damage::first_difference(&want, &seen.record)
            .or((seen.except != damage::NO_OBJECT).then_some(damage::SIZE));
        match at {
            None => {
                self.stats.matched += 1;
                Some(true)
            }
            Some(at) => {
                self.stats.mismatched += 1;
                self.stats.first_mismatch_at.get_or_insert(at);
                Some(false)
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

    fn rec(n: u8) -> Record {
        let mut r = [0u8; damage::SIZE];
        r[0] = n;
        r
    }

    #[test]
    fn the_arm_answers_after_its_clean_count_and_audits_one_in_64() {
        let mut arm = Arm::new();
        for n in 0..PROVEN_AFTER {
            assert_eq!(arm.decide(rec(n as u8)), Decision::Audit);
            assert_eq!(arm.seen(&Seen { record: rec(n as u8), except: u32::MAX }), Some(true));
        }
        assert!(arm.proven());
        let mut audits = 0;
        for n in 0..640u64 {
            match arm.decide(rec(1)) {
                Decision::Answer => {}
                Decision::Audit => {
                    audits += 1;
                    arm.seen(&Seen { record: rec(1), except: u32::MAX });
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
            arm.seen(&Seen { record: rec(2), except: u32::MAX });
        }
        // An audit that differs in the second argument.
        while arm.decide(rec(2)) != Decision::Audit {}
        assert_eq!(arm.seen(&Seen { record: rec(2), except: 7 }), Some(false));
        assert_eq!(arm.stats().first_mismatch_at, Some(damage::SIZE));
        for _ in 0..1000 {
            assert_eq!(arm.decide(rec(2)), Decision::Audit);
            arm.seen(&Seen { record: rec(2), except: u32::MAX });
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
        assert_eq!(arm.seen(&Seen { record: rec(4), except: u32::MAX }), Some(true));
        assert_eq!(arm.seen(&Seen { record: rec(4), except: u32::MAX }), None);
        assert_eq!((arm.stats().checked, arm.stats().mismatched), (1, 0));
    }
}
