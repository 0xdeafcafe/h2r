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
