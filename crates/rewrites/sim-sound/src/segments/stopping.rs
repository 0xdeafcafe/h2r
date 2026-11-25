//! Out-segment selection for a looping track. The alternate loop can use
//! its own out sound or explicitly request a fade with no out sound.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutSegment {
    Normal,
    Alternate,
}

impl OutSegment {
    /// The guest keeps the masked selector until it chooses a fade time.
    /// An x86 shift uses the low five bits of the segment number.
    pub fn alternate_selector(current: u8) -> u32 {
        let bit = 1u32 << (current & 31);
        match current & 31 {
            3 | 4 | 6 => bit,
            _ => 0,
        }
    }

    pub fn select(selector: u32, flags: u32, alternate_sound: Option<u32>) -> Self {
        if selector != 0 && (flags & (1 << 4) != 0 || alternate_sound.is_some()) {
            Self::Alternate
        } else {
            Self::Normal
        }
    }

    pub fn fade_flag_shift(self) -> u32 {
        match self {
            Self::Normal => 1,
            Self::Alternate => 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_normal_out_only_when_the_alternate_has_neither_a_sound_nor_a_fade() {
        for (current, alternate) in [(0, false), (3, true), (4, true), (5, false),
                                     (6, true), (7, false), (35, true)] {
            let selector = OutSegment::alternate_selector(current);
            for (flags, tag) in [(0, None), (2, None), (16, None), (0, Some(37))] {
                let choose_alt = alternate && (flags == 16 || tag.is_some());
                assert_eq!(OutSegment::select(selector, flags, tag) == OutSegment::Alternate, choose_alt);
            }
        }
    }
}
