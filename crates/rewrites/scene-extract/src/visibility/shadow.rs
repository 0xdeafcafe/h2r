//! The shadow arms of the main view's visibility
//! (docs/graphics/native-visibility.md, "The shadow arms"): one each for
//! the cluster set (the portal traversal), the window frusta and the
//! object set. An arm decides, per view, whether the Rust result is
//! compared with the guest's, answers in its place, or is not made.
//!
//! | State | Sampled frame (1 in [`SAMPLE_EVERY`]) | Other frames |
//! |---|---|---|
//! | learning | compare; the guest answers | the guest answers, nothing is made |
//! | proven | compare; Rust answers when equal | Rust answers when the switch is on |
//! | fallen | compare, counted; the guest answers | the guest answers |
//!
//! An arm is proven after [`PROVEN_AFTER`] sampled matches with no
//! difference. One difference makes it fall back to the guest for the
//! rest of the map; a map load starts every arm learning again, and the
//! counts stay. With `impl.render.native.visibility=off` an arm still
//! compares its samples and never answers. The state machine is
//! `engine::shadow_arm`'s, which the byte-compared arms share.

pub use engine::shadow_arm::{Counts, Outcome, PROVEN_AFTER, Plan, SAMPLE_EVERY, Shadow, sampled};

/// The three arms, in the order their counters are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Clusters,
    Frusta,
    Objects,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Self::Clusters, Self::Frusta, Self::Objects];

    pub fn name(self) -> &'static str {
        match self {
            Self::Clusters => "clusters",
            Self::Frusta => "frusta",
            Self::Objects => "objects",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first unsampled frame after `n` samples of 64.
    fn off_sample(n: u32) -> u32 {
        n * SAMPLE_EVERY + 1
    }

    #[test]
    fn one_frame_in_sixty_four_is_compared() {
        let s = Shadow::default();
        let compared = (0..4 * SAMPLE_EVERY)
            .filter(|&f| s.plan(f, true) == Plan::Compare)
            .count();
        assert_eq!(compared, 4);
        assert_eq!(s.plan(0, true), Plan::Compare);
        assert_eq!(
            s.plan(1, true),
            Plan::Guest,
            "learning: the guest answers, nothing is made"
        );
    }

    #[test]
    fn the_arm_answers_after_its_clean_matches_and_not_before() {
        let mut s = Shadow::default();
        for n in 0..PROVEN_AFTER {
            assert_eq!(s.plan(off_sample(n as u32), true), Plan::Guest);
            assert_eq!(
                s.compared(true, true),
                (Outcome::Matched, n + 1 == PROVEN_AFTER)
            );
        }
        assert!(s.proven());
        assert_eq!(s.plan(off_sample(0), true), Plan::Answer);
        assert_eq!(s.unsampled(Plan::Answer), (Outcome::Answered, true));
        assert_eq!(
            s.plan(SAMPLE_EVERY, true),
            Plan::Compare,
            "a proven arm still samples"
        );
        assert_eq!(
            s.counts,
            Counts {
                matched: PROVEN_AFTER,
                answered: 1,
                ..Counts::default()
            }
        );
    }

    #[test]
    fn one_difference_falls_back_to_the_guest_for_the_map() {
        let mut s = Shadow::default();
        for _ in 0..PROVEN_AFTER {
            s.compared(true, true);
        }
        assert_eq!(s.compared(false, true), (Outcome::Differed, false));
        assert!(s.fallen());
        assert_eq!(s.plan(off_sample(1), true), Plan::Guest);
        assert_eq!(s.unsampled(Plan::Guest), (Outcome::Guest, false));
        // Later matches are counted and never answer again on this map.
        for _ in 0..2 * PROVEN_AFTER {
            assert_eq!(s.compared(true, true), (Outcome::Matched, false));
        }
        assert_eq!(s.plan(off_sample(2), true), Plan::Guest);
        s.new_map();
        assert!(!s.fallen() && !s.proven());
        assert_eq!(s.counts.differed, 1, "a map load keeps the counts");
    }

    #[test]
    fn a_difference_while_learning_also_falls_back() {
        let mut s = Shadow::default();
        s.compared(true, true);
        s.compared(false, true);
        for _ in 0..PROVEN_AFTER {
            s.compared(true, true);
        }
        assert!(s.fallen());
    }

    #[test]
    fn off_the_arm_compares_and_never_answers() {
        let mut s = Shadow::default();
        for _ in 0..PROVEN_AFTER {
            assert_eq!(s.compared(true, false), (Outcome::Matched, false));
        }
        assert!(s.proven());
        assert_eq!(s.plan(off_sample(0), false), Plan::Guest);
        assert_eq!(s.plan(0, false), Plan::Compare);
        assert_eq!(s.declined(), (Outcome::Declined, false));
        assert_eq!(s.counts.declined, 1);
    }
}
