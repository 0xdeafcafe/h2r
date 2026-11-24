//! The looping-sound request selects an existing instance by tag and owner.
//! A start or update may allocate a missing instance. A stop only consumes
//! the request. These decisions run before any track changes or RNG draws.

/// The mode at the request boundary is a word. The guest tests allocation
/// with a variable shift, so that test sees only the low five bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestMode(pub u32);

impl RequestMode {
    pub fn stopping(self) -> bool {
        self.0 == 2
    }

    pub fn may_allocate(self) -> bool {
        self.0 & 31 <= 1
    }

    pub fn for_existing(self, flags: u8) -> Self {
        if self.0 == 0 && flags & (1 << 4) != 0 {
            Self(1)
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_and_existing_start_use_different_tests() {
        for mode in 0..96 {
            assert_eq!(RequestMode(mode).may_allocate(), matches!(mode % 32, 0 | 1));
            assert_eq!(RequestMode(mode).stopping(), mode == 2);
            let expected = if mode == 0 { 1 } else { mode };
            assert_eq!(RequestMode(mode).for_existing(0x10), RequestMode(expected));
            assert_eq!(RequestMode(mode).for_existing(0x01), RequestMode(mode));
        }
    }
}
