//! The frame counters, made by the tick driver in every mode
//! (docs/engine/sim-render-split.md, "7. Frame counters", slice 10).
//!
//! A world frame the guest draws moves three words the next tick reads:
//! the render frame counter, the one-shot byte and the present counter
//! (`engine::render_loop::tick_writes`). The headless step makes them once
//! per world frame. [`Counters`] gives a drawn world frame the same words:
//! at the frame's entry it reads them, and when the frame ends having
//! presented it writes `Words::predict` of what it read, whatever the
//! frame did to them (a second present for a reference copy, a loop
//! frame's present, a view that stepped the counter). The words then
//! depend on the logical frame count alone, not on how often or how
//! the render drew.
//!
//! A frame that did not present (a resource cache still loading) and a
//! frame outside the world are left as the guest made them. Wired into
//! [`crate::render_rng::RenderRng`]'s two hooks at `RENDER_FRAME`, so every
//! session that renders has it and the point's hook order is one feature's.

use std::sync::Mutex;

use guest_layout::GuestMemory;
use engine::render_loop::tick_writes::{self, Word, Words};
use sim_bridge::sim_frame::{Addrs, FrameKind, frame_kind, idle_after};

/// The words before a world frame, until it ends; and the count of words
/// the settle had to move, by [`Word::ALL`] order.
#[derive(Default)]
pub(crate) struct Counters {
    before: Mutex<Option<Words>>,
    moved: [std::sync::atomic::AtomicU64; 3],
}

impl Counters {
    /// The frame about to run: read the words when it is a world frame.
    pub(crate) fn enter(&self, a: &Addrs, g: &dyn GuestMemory) {
        let words = (frame_kind(a, g) == FrameKind::World)
            .then(|| Words::read(&a.ticks, g).ok())
            .flatten();
        *self.before.lock().unwrap_or_else(|e| e.into_inner()) = words;
    }

    /// The frame ended: when it presented, the words are the tick driver's.
    /// Returns the words the frame had moved from them.
    pub(crate) fn leave(&self, a: &Addrs, g: &mut dyn GuestMemory) -> Vec<Word> {
        let Some(before) = self.before.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return Vec::new();
        };
        if !idle_after(a, g) {
            return Vec::new();
        }
        let moved = tick_writes::settle(&a.ticks, g, before).unwrap_or_default();
        for w in &moved {
            if let Some(at) = Word::ALL.iter().position(|x| x == w) {
                self.moved[at].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        moved
    }

    /// Frames each word had to be put right in, by [`Word::ALL`] order.
    #[allow(dead_code)]
    pub(crate) fn moved(&self) -> [u64; 3] {
        std::array::from_fn(|i| self.moved[i].load(std::sync::atomic::Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const A: Addrs = Addrs::HALO2;
    const GLOBALS: u32 = 0x0070_0000;

    /// A guest in a world frame with idle caches, its tick words set.
    fn world() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x0048_5000, 0x1000)
            .map(0x004b_a000, 0x1000)
            .map(0x004c_f000, 0x1000)
            .map(0x004e_6000, 0x1000)
            .map(GLOBALS, 0x2000);
        g.put_u32(A.engine_globals, GLOBALS).put_u8(GLOBALS + 0x1120, 1).put_u16(GLOBALS + 2, 0);
        g.put_u32(A.ticks.present_counter, 100).put_u32(A.ticks.frame_counter, 7).put_u8(A.ticks.one_shot, 1);
        g
    }

    fn words(g: &FakeGuest) -> Words {
        Words::read(&A.ticks, g).unwrap()
    }

    /// The words after N logical frames do not depend on how many presents
    /// each drawn frame made.
    #[test]
    fn a_drawn_frame_leaves_the_words_the_headless_step_would() {
        let headless = {
            let mut g = world();
            for _ in 0..40 {
                tick_writes::frame(&A.ticks, &mut g).unwrap();
            }
            words(&g)
        };
        for extra_presents in [0u32, 1, 3] {
            let c = Counters::default();
            let mut g = world();
            for _ in 0..40 {
                c.enter(&A, &g);
                tick_writes::frame(&A.ticks, &mut g).unwrap();
                for _ in 0..extra_presents {
                    tick_writes::present_counter(&A.ticks, &mut g).unwrap();
                }
                c.leave(&A, &mut g);
            }
            assert_eq!(words(&g), headless, "{extra_presents} extra presents");
        }
    }

    #[test]
    fn what_the_frame_moved_is_named_and_counted() {
        let c = Counters::default();
        let mut g = world();
        c.enter(&A, &g);
        tick_writes::frame(&A.ticks, &mut g).unwrap();
        assert!(c.leave(&A, &mut g).is_empty());
        c.enter(&A, &g);
        tick_writes::frame(&A.ticks, &mut g).unwrap();
        tick_writes::present_counter(&A.ticks, &mut g).unwrap();
        assert_eq!(c.leave(&A, &mut g), [Word::PresentCounter]);
        assert_eq!(c.moved(), [0, 0, 1]);
    }

    /// A loading frame did not present, and a menu is no world frame: the
    /// guest's words stand.
    #[test]
    fn a_loading_frame_and_a_menu_are_left_alone() {
        let c = Counters::default();
        let mut g = world();
        c.enter(&A, &g);
        tick_writes::frame(&A.ticks, &mut g).unwrap();
        tick_writes::present_counter(&A.ticks, &mut g).unwrap();
        g.put_u32(A.cache_countdowns.0, 3);
        let kept = words(&g);
        assert!(c.leave(&A, &mut g).is_empty());
        assert_eq!(words(&g), kept);

        let mut menu = world();
        menu.put_u32(A.engine_globals, 0);
        c.enter(&A, &menu);
        tick_writes::present_counter(&A.ticks, &mut menu).unwrap();
        let kept = words(&menu);
        assert!(c.leave(&A, &mut menu).is_empty());
        assert_eq!(words(&menu), kept);
    }

    /// A fault reading the words at the entry leaves the frame to the guest.
    #[test]
    fn unreadable_words_leave_the_frame_to_the_guest() {
        let c = Counters::default();
        let mut g = FakeGuest::new();
        c.enter(&A, &g);
        assert!(c.leave(&A, &mut g).is_empty());
    }
}
