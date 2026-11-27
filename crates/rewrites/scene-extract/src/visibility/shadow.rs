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
//! compares its samples and never answers.

/// One frame in this many is compared with the guest's, whatever the
/// arm's state.
pub const SAMPLE_EVERY: u32 = 64;

/// Sampled matches, with no difference, before an arm answers.
pub const PROVEN_AFTER: u64 = 16;

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

/// What an arm does with one view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Make the Rust result and compare it with the guest's.
    Compare,
    /// Make the Rust result and answer with it, uncompared.
    Answer,
    /// The guest answers; the Rust result is not needed.
    Guest,
}

/// Who answered one view, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Sampled, equal: answered by Rust once proven and on, else the guest.
    Matched,
    /// Sampled, different: the guest answers, and the arm falls back.
    Differed,
    /// Not sampled; Rust answered.
    Answered,
    /// Not sampled; the guest answered (learning, fallen or off).
    Guest,
    /// The Rust result declined or the guest's did not read: the guest
    /// answers, or the view carries nothing.
    Declined,
}

/// An arm's counts since start: every view it saw is in exactly one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub matched: u64,
    pub differed: u64,
    pub answered: u64,
    pub guest: u64,
    pub declined: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Learning,
    Proven,
    Fallen,
}

/// One arm: its state on this map and its counts.
#[derive(Clone, Copy, Debug, Default)]
pub struct Shadow {
    state: State,
    /// Sampled matches on this map.
    clean: u64,
    pub counts: Counts,
}

/// Whether `frame` is a sampled one.
pub fn sampled(frame: u32) -> bool {
    frame % SAMPLE_EVERY == 0
}

impl Shadow {
    /// A learning arm with no counts.
    pub const NEW: Shadow = Shadow {
        state: State::Learning,
        clean: 0,
        counts: Counts {
            matched: 0,
            differed: 0,
            answered: 0,
            guest: 0,
            declined: 0,
        },
    };

    /// The arm answers its unsampled frames.
    pub fn proven(&self) -> bool {
        self.state == State::Proven
    }

    /// The arm has seen a difference on this map.
    pub fn fallen(&self) -> bool {
        self.state == State::Fallen
    }

    /// What to do with a view of `frame`. `on` is
    /// `impl.render.native.visibility`.
    pub fn plan(&self, frame: u32, on: bool) -> Plan {
        if sampled(frame) {
            Plan::Compare
        } else if on && self.proven() {
            Plan::Answer
        } else {
            Plan::Guest
        }
    }

    /// Record a compared view: `equal` is the comparison's result.
    /// Returns whether Rust answers it.
    pub fn compared(&mut self, equal: bool, on: bool) -> (Outcome, bool) {
        if !equal {
            self.counts.differed += 1;
            self.state = State::Fallen;
            return (Outcome::Differed, false);
        }
        self.counts.matched += 1;
        if self.state == State::Learning {
            self.clean += 1;
            if self.clean >= PROVEN_AFTER {
                self.state = State::Proven;
            }
        }
        (Outcome::Matched, on && self.proven())
    }

    /// Record an unsampled view: Rust answers it when `plan` was
    /// [`Plan::Answer`].
    pub fn unsampled(&mut self, plan: Plan) -> (Outcome, bool) {
        if plan == Plan::Answer {
            self.counts.answered += 1;
            (Outcome::Answered, true)
        } else {
            self.counts.guest += 1;
            (Outcome::Guest, false)
        }
    }

    /// Record a view whose Rust or guest side did not make a result.
    pub fn declined(&mut self) -> (Outcome, bool) {
        self.counts.declined += 1;
        (Outcome::Declined, false)
    }

    /// A map load: learn again; the counts stay.
    pub fn new_map(&mut self) {
        self.state = State::Learning;
        self.clean = 0;
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
