//! Source decisions made while the sound frame walks its mixer channels.

/// The timing window of a source marked for a gain ramp. Both edges and
/// the current time are signed values in the game's audio clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FadeWindow {
    pub start: i32,
    pub end: i32,
}

impl FadeWindow {
    /// A downward ramp can release a channel once its output is quiet.
    pub fn finished_fade_out(self, quiet: bool) -> bool {
        quiet && self.start > self.end
    }

    /// The ramp bit clears when an upward ramp reaches its deadline.
    pub fn finished_fade_in(self, clock: i32) -> bool {
        self.start < self.end && clock >= self.end
    }
}

/// The specialized channel updater at the end of the mixer walk handles
/// four kinds of source. The x86 variable shift uses five count bits.
pub fn uses_specialized_channel(kind: u8) -> bool {
    (1..=4).contains(&(kind & 31))
}

/// A looping sound holds a reference in the lower seven bits of its
/// companion record. The high bit has a separate owner.
pub fn release_loop_ref(state: u8) -> u8 {
    (state & 0x80) | (state.wrapping_sub(1) & 0x7f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_windows_distinguish_equal_edges_and_signed_deadlines() {
        for start in [-17, 0, 9] {
            for end in [-17, 0, 9] {
                let fade = FadeWindow { start, end };
                assert!(!fade.finished_fade_out(false));
                assert_eq!(fade.finished_fade_out(true), end < start);
                for clock in [end - 1, end, end + 1] {
                    assert_eq!(fade.finished_fade_in(clock), start < end && end <= clock);
                }
            }
        }
    }

    #[test]
    fn channel_dispatch_matches_the_bit_test_for_every_kind() {
        for kind in 0..=u8::MAX {
            let guest_mask = 1u32.wrapping_shl(u32::from(kind));
            assert_eq!(uses_specialized_channel(kind), guest_mask & 0x1e != 0);
        }
    }

    #[test]
    fn releasing_a_reference_preserves_ownership_even_on_wrap() {
        for state in 0..=u8::MAX {
            let result = release_loop_ref(state);
            assert_eq!(result & 0x80, state & 0x80);
            assert_eq!((u16::from(result & 127) + 1) % 128, u16::from(state & 127));
        }
    }
}

/// A walk from the slot after the callback. The pool can change while the
/// callback runs, so the high-water mark and allocation words come from
/// this visit. One host bit scan skips a whole run of unoccupied slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceScan {
    pub next: Option<u32>,
    /// The last slot examined, including on exhaustion. The boundary
    /// publishes that slot's bit mask in the guest iterator registers.
    pub last_examined: Option<u32>,
}

impl InstanceScan {
    pub fn read<E>(
        start: i32,
        limit: i32,
        mut word: impl FnMut(u32) -> Result<u32, E>,
    ) -> Result<Self, E> {
        if start < 0 || start >= limit {
            return Ok(Self {
                next: None,
                last_examined: None,
            });
        }
        let mut slot = start as u32;
        let end = limit as u32;
        loop {
            let base = slot & !31;
            let occupied = word(slot >> 5)? & (u32::MAX << (slot & 31));
            if occupied != 0 {
                let candidate = base + occupied.trailing_zeros();
                if candidate < end {
                    return Ok(Self {
                        next: Some(candidate),
                        last_examined: Some(candidate),
                    });
                }
            }
            slot = base + 32;
            if slot >= end {
                return Ok(Self {
                    next: None,
                    last_examined: Some(end - 1),
                });
            }
        }
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;

    #[test]
    fn bitmap_walk_matches_a_slot_iterator_with_partial_final_words() {
        let bits = [0x8000_0002u32, 0x0000_0005, 0x4000_0000];
        for start in -1..96 {
            for end in 0..=96 {
                let next = (start.max(0)..end)
                    .filter(|_| start >= 0)
                    .map(|slot| slot as u32)
                    .find(|slot| bits[(slot / 32) as usize] & (1 << (slot % 32)) != 0);
                let scan =
                    InstanceScan::read(start, end, |i| Ok::<_, ()>(bits[i as usize])).unwrap();
                assert_eq!(scan.next, next);
                assert_eq!(
                    scan.last_examined,
                    next.or_else(|| (start >= 0 && start < end).then(|| (end - 1) as u32))
                );
            }
        }
    }
}
