//! Decisions made by the game sound tick and its announcer queue. Each bitmap
//! word and source record is borrowed for one decision. A child updating one
//! sound can free or start another before the walk continues.

/// Search the occupied slots in one bitmap word, from `first` up to the
/// exclusive high water mark. Bits beyond the live extent can be set.
pub fn next_in_word(bits: u32, first: u32, high_water: u32) -> Option<u32> {
    let remaining = bits & (u32::MAX << (first % 32));
    if remaining == 0 {
        return None;
    }
    let slot = (first & !31) + remaining.trailing_zeros();
    (slot < high_water).then_some(slot)
}

pub fn due(deadline: i32, tick: i32) -> bool {
    deadline != -1 && deadline <= tick
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadAction {
    Wait,
    CountDown(i16),
    Start,
    Remove,
}

/// A head with one tick left reaches zero on this update and starts on the
/// next. Completion releases the queue on a subsequent update.
pub fn head_action(delay: i16, playing: bool) -> HeadAction {
    match delay {
        0 => HeadAction::Start,
        1.. => HeadAction::CountDown(delay - 1),
        _ if playing => HeadAction::Wait,
        _ => HeadAction::Remove,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_lookup_matches_a_scalar_walk_including_partial_words() {
        for bits in [0, u32::MAX, 0x0380_1002, 0x8000_0001] {
            for first in 32..64 {
                for limit in first..=64 {
                    let scalar = (first..limit).find(|i| (bits >> (i % 32)) & 1 == 1);
                    assert_eq!(next_in_word(bits, first, limit), scalar);
                }
            }
        }
    }

    #[test]
    fn due_compares_signed_ticks_and_reserves_minus_one() {
        assert!(due(i32::MIN, i32::MIN));
        assert!(!due(-1, i32::MAX));
        assert!(!due(0, -1));
        assert!(due(0, 0));
    }

    #[test]
    fn queue_waits_for_the_start_tick_and_a_completion() {
        assert_eq!(head_action(1, false), HeadAction::CountDown(0));
        assert_eq!(head_action(0, false), HeadAction::Start);
        assert_eq!(head_action(-1, true), HeadAction::Wait);
        assert_eq!(head_action(i16::MIN, false), HeadAction::Remove);
    }
}

/// The promotion counter uses signed wrapping tick arithmetic. Rewinding
/// the sound clock increases the remaining value by the ticks rewound.
pub fn decay_counter(counter: i32, elapsed: i32) -> i32 {
    counter.wrapping_sub(elapsed).max(0)
}

/// A rule adds its increment before testing its threshold. Each rule passed
/// on the way to a higher one loses its accumulated credit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromotionRule {
    pub enabled: i16,
    pub threshold: i32,
    pub increment: i32,
}

impl PromotionRule {
    pub fn advance(self, counter: i32, has_next: bool) -> (i32, bool) {
        let credit = counter.wrapping_add(self.increment);
        if self.enabled > 0 && credit >= self.threshold && has_next {
            (0, true)
        } else {
            (credit, false)
        }
    }
}

/// A higher promotion can interrupt a lower level's cooldown. A lower
/// promotion that is accepted after the cutoff does not take ownership of
/// the cooldown until the active rule has expired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission { Suppress, Admit, AdmitAndArm }

pub fn admission(rule: i32, active: i32, clock: i32, until: i32) -> Admission {
    if rule <= active && clock < until {
        Admission::Suppress
    } else if rule >= active {
        Admission::AdmitAndArm
    } else {
        Admission::Admit
    }
}

#[cfg(test)]
mod promotion_tests {
    use super::*;

    #[test]
    fn promotion_counts_decays_and_spends_only_passed_rules() {
        let rule = PromotionRule { threshold: 6, increment: 4, enabled: 1 };
        assert_eq!(rule.advance(decay_counter(5, 4), true), (5, false));
        assert_eq!(rule.advance(decay_counter(5, 3), true), (0, true));
        assert_eq!(rule.advance(decay_counter(5, 3), false), (6, false));
        assert_eq!(rule.advance(i32::MAX, true), (i32::MIN + 3, false));
        assert_eq!(decay_counter(i32::MIN, 1), i32::MAX);
        assert_eq!(decay_counter(2, 4), 0);
    }

    #[test]
    fn promotion_suppresses_until_the_exact_expiry_or_a_higher_rule() {
        assert_eq!(admission(1, 1, 8, 9), Admission::Suppress);
        assert_eq!(admission(0, 1, 8, 9), Admission::Suppress);
        assert_eq!(admission(2, 1, 8, 9), Admission::AdmitAndArm);
        assert_eq!(admission(1, 1, 9, 9), Admission::AdmitAndArm);
    }
}
