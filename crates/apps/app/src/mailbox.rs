//! Triple-buffer index bookkeeping for the frame handoff.
//!
//! Three slots: the producer (guest thread) owns `back`, the consumer
//! (window thread) owns `front`, and `middle` holds the newest finished
//! frame. Publishing swaps back and middle; taking swaps middle and front
//! when a new frame is waiting. Neither side ever waits for the other, the
//! consumer always sees the newest frame, and a frame published over an
//! unread one is counted as dropped.
//!
//! This is only the index logic; [`crate::frame`] attaches GPU textures to
//! the slots and holds the lock.

/// Indices and counters. Slots are `0..3`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TripleBuffer {
    back: usize,
    middle: usize,
    front: usize,
    fresh: bool,
    shown_any: bool,
    published: u64,
    taken: u64,
    dropped: u64,
}

impl Default for TripleBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl TripleBuffer {
    pub fn new() -> Self {
        Self {
            back: 0,
            middle: 1,
            front: 2,
            fresh: false,
            shown_any: false,
            published: 0,
            taken: 0,
            dropped: 0,
        }
    }

    /// The slot the producer writes next.
    pub fn back(&self) -> usize {
        self.back
    }

    /// The slot the consumer last took, if it ever took one.
    pub fn front(&self) -> Option<usize> {
        self.shown_any.then_some(self.front)
    }

    /// A published frame is waiting.
    pub fn has_new(&self) -> bool {
        self.fresh
    }

    /// The producer finished writing `back`. Returns true if this replaced a
    /// frame the consumer never took.
    pub fn publish(&mut self) -> bool {
        std::mem::swap(&mut self.back, &mut self.middle);
        let dropped = self.fresh;
        if dropped {
            self.dropped += 1;
        }
        self.fresh = true;
        self.published += 1;
        dropped
    }

    /// The consumer wants the newest frame. Returns the new front slot if a
    /// frame was waiting, else `None` (keep showing [`front`](Self::front)).
    pub fn take(&mut self) -> Option<usize> {
        if !self.fresh {
            return None;
        }
        std::mem::swap(&mut self.front, &mut self.middle);
        self.fresh = false;
        self.shown_any = true;
        self.taken += 1;
        Some(self.front)
    }

    /// Frames published.
    pub fn published(&self) -> u64 {
        self.published
    }

    /// Frames taken by the consumer.
    pub fn taken(&self) -> u64 {
        self.taken
    }

    /// Frames replaced before the consumer took them.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distinct(t: &TripleBuffer) -> bool {
        t.back != t.middle && t.middle != t.front && t.back != t.front
    }

    #[test]
    fn nothing_before_the_first_publish() {
        let mut t = TripleBuffer::new();
        assert_eq!(t.take(), None);
        assert_eq!(t.front(), None);
    }

    #[test]
    fn publish_then_take_hands_over_the_written_slot() {
        let mut t = TripleBuffer::new();
        let written = t.back();
        assert!(!t.publish());
        assert_eq!(t.take(), Some(written));
        assert_eq!(t.front(), Some(written));
        assert_eq!(t.take(), None, "no new frame");
        assert_eq!(t.front(), Some(written), "keeps showing the last one");
    }

    #[test]
    fn the_newest_frame_wins_and_older_ones_count_as_dropped() {
        let mut t = TripleBuffer::new();
        assert!(!t.publish());
        assert!(t.publish());
        let newest = t.back();
        assert!(t.publish());
        let shown = t.take().unwrap();
        assert_eq!(shown, newest, "the third frame, not the first two");
        assert_eq!((t.published(), t.taken(), t.dropped()), (3, 1, 2));
        assert!(distinct(&t));
        // The taken slot is never the producer's next target.
        assert_ne!(shown, t.back());
    }

    #[test]
    fn producer_never_writes_the_front_slot() {
        // Randomised interleaving (fixed LCG, no deps): the producer's slot is
        // never the one the consumer holds, and the slots stay a permutation.
        let mut t = TripleBuffer::new();
        let mut seed = 0x1234_5678u32;
        let mut last_published = None;
        for _ in 0..10_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if seed >> 31 == 0 {
                last_published = Some(t.back());
                t.publish();
            } else if let Some(s) = t.take() {
                assert_eq!(Some(s), last_published, "consumer gets the newest frame");
            }
            assert!(distinct(&t));
            if let Some(f) = t.front() {
                assert_ne!(f, t.back());
            }
        }
        assert_eq!(
            t.published(),
            t.taken() + t.dropped() + u64::from(t.has_new())
        );
    }
}
