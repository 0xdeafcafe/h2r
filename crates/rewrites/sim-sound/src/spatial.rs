//! Channel routing and the timestamp of a transition between direct and
//! submix output. The clock and fade interval are in milliseconds.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceSpace {
    Plain,
    World,
    Listener,
    Other(i8),
}

impl SourceSpace {
    pub fn decode(packed: u8) -> Self {
        let kind = ((packed << 4) as i8) >> 4;
        match kind {
            0 => Self::Plain,
            1 => Self::World,
            2 => Self::Listener,
            value => Self::Other(value),
        }
    }

    pub fn kind(self) -> i8 {
        match self {
            Self::Plain => 0,
            Self::World => 1,
            Self::Listener => 2,
            Self::Other(value) => value,
        }
    }
}

/// When routing changes in the middle of a fade, reflect the remaining
/// time into the new ramp. The guest clock is a wrapping signed dword.
pub fn transition_start(previous: i32, clock: i32, interval: i32) -> i32 {
    let age = interval.wrapping_sub(previous).wrapping_sub(clock);
    clock.wrapping_sub(age.max(0).min(interval))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_above_the_low_nibble_do_not_select_a_space() {
        for flags in 0..16 {
            for (nibble, kind) in [(0, 0), (1, 1), (2, 2), (3, 3), (8, -8), (15, -1)] {
                assert_eq!(SourceSpace::decode(flags << 4 | nibble).kind(), kind);
            }
        }
    }

    #[test]
    fn direction_change_clamps_or_reflects_the_transition_time() {
        assert_eq!(transition_start(10, 70, 100), 50);
        assert_eq!(transition_start(40, 70, 100), 70);
        assert_eq!(transition_start(-90, 70, 100), -30);
        assert_eq!(transition_start(i32::MIN, i32::MIN, 100), i32::MAX - 99);
    }
}
