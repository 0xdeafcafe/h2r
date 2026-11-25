//! Candidate selection for an already-playing looping track. The loaded
//! track and the executable supply the sound references and transition
//! order. An intermediate sound may play on top of the new segment.

#[derive(Clone, Copy)]
pub struct Segments(u32);

impl Segments {
    pub fn from_track_flags(flags: u32) -> Self {
        let intro = flags & 1;
        let normal = if flags & 2 != 0 {
            (1 << 2) | (1 << 6)
        } else {
            0
        };
        let alternate = if flags & 4 != 0 {
            (1 << 3) | (1 << 5)
        } else {
            0
        };
        Self(intro | normal | alternate)
    }

    pub fn bits(self) -> u32 {
        self.0
    }

    /// Whether this segment belongs to the alternate sequence.
    pub fn alternate(segment: u8) -> bool {
        matches!(segment & 31, 3 | 4 | 6)
    }

    pub fn overlay(self, segment: u32) -> bool {
        self.0 & 1u32.wrapping_shl(segment) != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    pub segment: u32,
    pub primary: u32,
    pub overlay: u32,
    /// Last node examined. A suspended track request keeps this in its
    /// frame while it starts either sound.
    pub last: u32,
}

/// A transition chain terminates when its successor is itself. A missing
/// reference advances to the following segment. Among eligible overlays,
/// the last one before a primary (or the terminal node) wins.
pub fn select<E>(
    current: u32,
    missing: u32,
    options: Segments,
    mut successor: impl FnMut(u32) -> Result<u32, E>,
    mut reference: impl FnMut(u32) -> Result<Option<u32>, E>,
) -> Result<Selection, E> {
    let mut choice = Selection {
        segment: successor(current)?,
        primary: missing,
        overlay: missing,
        last: missing,
    };
    let mut previous = current;
    while choice.segment != previous {
        if let Some(tag) = reference(choice.segment)? {
            choice.last = tag;
            if !options.overlay(choice.segment) {
                choice.primary = tag;
                break;
            }
            choice.overlay = tag;
        }
        previous = choice.segment;
        choice.last = previous;
        choice.segment = successor(previous)?;
    }
    Ok(choice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_emits_the_last_overlay_before_the_primary() {
        let options = Segments::from_track_flags(2);
        let choice = select(
            0,
            u32::MAX,
            options,
            |kind| {
                Ok::<_, ()>(match kind {
                    0 => 2,
                    2 => 6,
                    _ => 4,
                })
            },
            |kind| Ok(Some(100 + kind)),
        )
        .unwrap();
        assert_eq!(
            choice,
            Selection {
                segment: 4,
                primary: 104,
                overlay: 106,
                last: 104
            }
        );
    }

    #[test]
    fn missing_reference_ends_at_the_fixed_point() {
        let choice = select(
            0,
            u32::MAX,
            Segments::from_track_flags(7),
            |kind| Ok::<_, ()>((kind + 1).min(3)),
            |_| Ok(None),
        )
        .unwrap();
        assert_eq!(
            choice,
            Selection {
                segment: 3,
                primary: u32::MAX,
                overlay: u32::MAX,
                last: 3
            }
        );
    }
}
