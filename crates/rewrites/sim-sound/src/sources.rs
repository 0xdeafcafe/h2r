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
