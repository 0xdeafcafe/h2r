//! Selection and start time of an impulse instance.

/// The instance keeps a pitch range relative to its sound tag, a permutation
/// relative to the range, and a chunk relative to the permutation. Retail
/// sign-extends each offset, including the two one-byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub range: i8,
    pub permutation: i8,
    pub chunk: i16,
}

impl Selection {
    pub fn range_index(self, base: i16) -> i32 {
        i32::from(base) + i32::from(self.range)
    }

    pub fn permutation_index(self, base: i16) -> i32 {
        i32::from(base) + i32::from(self.permutation)
    }

    pub fn chunk_index(self, base: i16) -> i32 {
        i32::from(base) + i32::from(self.chunk)
    }
}

/// Propagation delay applies only beyond the minimum and only to sources
/// without either immediate-play flag.
pub fn delayed_start(delay: i32, minimum: i32, source_flags: u16) -> bool {
    delay > minimum && source_flags & ((1 << 8) | (1 << 11)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_selection_sign_extends_each_field() {
        let choice = Selection {
            range: -3,
            permutation: 5,
            chunk: -8,
        };
        assert_eq!(choice.range_index(10), 7);
        assert_eq!(choice.permutation_index(-8), -3);
        assert_eq!(choice.chunk_index(2), -6);
    }

    #[test]
    fn the_minimum_delay_is_inclusive_and_each_flag_forces_immediate_play() {
        assert!(!delayed_start(18, 18, 0));
        assert!(delayed_start(19, 18, 0));
        assert!(!delayed_start(19, 18, 1 << 8));
        assert!(!delayed_start(19, 18, 1 << 11));
    }
}
