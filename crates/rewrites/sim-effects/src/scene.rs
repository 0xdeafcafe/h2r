//! Visual parts for the renderer's scene.
//!
//! A decal, light or lens flare part becomes a [`PartEvent`] in Rust. The
//! queue is not guest memory and is not saved, so the arena hash and the
//! checkpoint never see it. The frame step drains it into each sealed
//! scene (`frame_scene::FrameScene::part_events`).

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::parts::{Part, PartClass};

/// What the renderer is told was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Decal,
    Light,
    /// `lens` and `MGS2`: retail draws none; the renderer may.
    Flare,
}

/// One visual part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartEvent {
    /// Position in creation order, from 1; never reused.
    pub seq: u64,
    pub kind: Kind,
    pub part: Part,
    /// The guest made it too (its decal or light datum): the renderer
    /// draws those from the guest arrays until it reads these events.
    pub guest_made: bool,
}

/// The queue holds this many events.
pub const CAP: usize = 4096;

/// The queue's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub queued: u64,
    pub taken: u64,
    /// Dropped oldest first past [`CAP`] (no renderer took them).
    pub dropped: u64,
}

/// The events since the renderer last took them.
#[derive(Debug, Default)]
pub struct Queue {
    events: VecDeque<PartEvent>,
    next: u64,
    stats: Stats,
}

impl Queue {
    pub const fn new() -> Self {
        Queue { events: VecDeque::new(), next: 0, stats: Stats { queued: 0, taken: 0, dropped: 0 } }
    }

    /// Queues `part` if it is visual; returns whether it did.
    pub fn push(&mut self, part: Part) -> bool {
        let (kind, guest_made) = match part.class() {
            PartClass::Decal => (Kind::Decal, true),
            PartClass::Light => (Kind::Light, true),
            PartClass::Nothing => (Kind::Flare, false),
            _ => return false,
        };
        if self.events.len() == CAP {
            self.events.pop_front();
            self.stats.dropped += 1;
        }
        self.next += 1;
        self.events.push_back(PartEvent { seq: self.next, kind, part, guest_made });
        self.stats.queued += 1;
        true
    }

    /// Every queued event, oldest first.
    pub fn take(&mut self) -> Vec<PartEvent> {
        self.stats.taken += self.events.len() as u64;
        self.events.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// A new map: the old map's events are dropped (counted).
    pub fn clear(&mut self) {
        self.stats.dropped += self.events.len() as u64;
        self.events.clear();
    }
}

/// The process's queue: the game thread pushes, the frame step takes.
pub static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());

/// [`Queue::take`] on [`QUEUE`].
pub fn take() -> Vec<PartEvent> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// [`Queue::push`] on [`QUEUE`].
pub fn push(part: Part) -> bool {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push(part)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::group;

    pub(crate) fn part(tick: u32, group: u32) -> Part {
        Part {
            tick,
            definition: 0xe000_0001,
            group,
            tag: 0xe100_0002 + tick,
            position: [tick as f32, 1.0, 2.0],
            direction: [0.0, 0.0, 1.0],
            scale: 1.0,
        }
    }

    #[test]
    fn only_visual_parts_queue_in_order() {
        let mut q = Queue::new();
        let groups = [group::DAMAGE, group::DECAL, group::OBJECT, group::LENS, group::LIGHT, group::SOUND];
        let queued: Vec<bool> = groups.iter().enumerate().map(|(t, &g)| q.push(part(t as u32, g))).collect();
        assert_eq!(queued, [false, true, false, true, true, false]);
        let ev = q.take();
        assert_eq!(
            ev.iter().map(|e| (e.seq, e.kind, e.guest_made)).collect::<Vec<_>>(),
            [(1, Kind::Decal, true), (2, Kind::Flare, false), (3, Kind::Light, true)]
        );
        assert!(q.is_empty());
        for t in 0..(CAP as u32 + 10) {
            q.push(part(t, group::DECAL));
        }
        assert_eq!((q.len(), q.stats().dropped), (CAP, 10));
        assert_eq!(q.take()[0].part.tick, 10);
    }
}
