//! The live instances of a loop track form a ring. The head (including its
//! datum salt) marks the end of a lap. A newly allocated track can also
//! carry the empty datum in its link.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackRing {
    head: u32,
}

impl TrackRing {
    pub const EMPTY: u32 = u32::MAX;

    pub fn new(head: u32) -> Self {
        Self { head }
    }

    pub fn first(self) -> Option<u32> {
        (self.head != Self::EMPTY).then_some(self.head)
    }

    pub fn following(self, next: u32) -> Option<u32> {
        (next != Self::EMPTY && next != self.head).then_some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lap_ends_at_the_original_salted_datum_or_an_empty_link() {
        let ring = TrackRing::new(0xabcd0003);
        assert_eq!(TrackRing::new(TrackRing::EMPTY).first(), None);
        assert_eq!(ring.first(), Some(0xabcd0003));
        assert_eq!(ring.following(0xabce0003), Some(0xabce0003));
        assert_eq!(ring.following(TrackRing::EMPTY), None);
        assert_eq!(ring.following(0xabcd0003), None);
    }
}
