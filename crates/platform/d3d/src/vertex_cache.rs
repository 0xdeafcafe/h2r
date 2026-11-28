//! Vertex streams kept across draws while the guest leaves them alone.
//!
//! A draw reads each stream's vertex window out of guest memory. Most of a
//! level's vertices (BSP, object models) never change, so the device keeps
//! a snapshot of each window it reads, keyed by guest address and length,
//! and validates it with the [`WriteTracker`] generation of the pages it
//! covers (the same contract the texture cache relies on, [`crate::dirty`]).
//! A clean window is handed to the backend as the same shared snapshot
//! ([`CachedBytes`]): no copy on the guest's thread, and a backend that
//! keeps a GPU buffer per snapshot id uploads nothing either.
//!
//! Admission: a window is copied as before the first time it is seen (a
//! *ghost* entry remembers it), and snapshotted only when it is drawn again
//! with its pages unwritten. A window that has been found written twice
//! (dynamic geometry: particles, decals, skinned or streamed vertices) is
//! snapshotted again only after it has stayed clean across
//! [`SETTLE_FRAMES`] frames, so a buffer rewritten every frame costs a
//! lookup and the old copy, not an allocation and a free.
//!
//! Memory is bounded: snapshots by bytes, entries (ghosts included) by
//! count, both least recently used first.
//!
//! Index lists are kept the same way ([`IndexCache`]): a clean list comes
//! back already scanned and rebased onto its vertex window.

use rustc_hash::FxHashMap as HashMap;

use gfx::external::{CachedBytes, CachedIndices, next_snapshot_id};
use crate::dirty::WriteTracker;

/// What a [`RangeCache`] keeps per range.
pub trait Snapshot: Clone {
    /// Host bytes it holds (for the budget).
    fn size(&self) -> usize;
}

impl Snapshot for CachedBytes {
    fn size(&self) -> usize {
        self.bytes.len()
    }
}

impl Snapshot for CachedIndices {
    fn size(&self) -> usize {
        4 * self.rebased.len()
    }
}

/// Vertex windows.
pub type VertexCache = RangeCache<CachedBytes>;
/// Index lists.
pub type IndexCache = RangeCache<CachedIndices>;

/// Times a window must be found written before it counts as dynamic.
pub const DYNAMIC_STRIKES: u8 = 2;
/// Frames a dynamic window must stay clean before it is snapshotted again.
pub const SETTLE_FRAMES: u64 = 2;
/// Default snapshot budget (`H2_VERTEX_CACHE_MB` overrides it).
pub const DEFAULT_BUDGET: usize = 128 << 20;
/// Most entries (snapshots and ghosts) kept.
pub const MAX_ENTRIES: usize = 1 << 16;

/// What the device should do with a window.
#[derive(Clone, Debug)]
pub enum Probe<T> {
    /// Use this snapshot: the window's pages are clean since it was read.
    Hit(T),
    /// Read the window and [`RangeCache::fill`] it.
    Fill,
    /// Copy the window for this draw only.
    Copy,
}

struct Entry<T> {
    /// Tracker generation when the bytes were last read (or found clean).
    generation: u64,
    /// Frame the window was last found written (or first seen).
    since_frame: u64,
    /// LRU clock.
    used: u64,
    /// Times found written.
    strikes: u8,
    snapshot: Option<T>,
}

/// Snapshots of guest memory ranges (see the module docs).
pub struct RangeCache<T> {
    entries: HashMap<(u32, u32), Entry<T>>,
    epoch: u64,
    bytes: usize,
    budget: usize,
    tick: u64,
    frame: u64,
}

impl<T: Snapshot> RangeCache<T> {
    /// An empty cache keeping at most `budget` bytes of snapshots.
    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::default(),
            epoch: next_snapshot_id(),
            bytes: 0,
            budget,
            tick: 0,
            frame: 0,
        }
    }

    /// The budget from `H2_VERTEX_CACHE_MB`, else [`DEFAULT_BUDGET`].
    pub fn budget_from_env() -> usize {
        std::env::var("H2_VERTEX_CACHE_MB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map_or(DEFAULT_BUDGET, |mb| mb << 20)
    }

    /// Section preparation primes new static windows once. A previously
    /// seen window, including a dynamic ghost, keeps the usual admission
    /// and invalidation policy on its next draw.
    pub(crate) fn needs_static_window(&self, va: u32, len: u32) -> bool {
        len as usize <= self.budget && !self.entries.contains_key(&(va, len))
    }

    /// Looks up `[va, va + len)`. Anything but a hit leaves the caller to
    /// read the window, and to [`WriteTracker::watch`] it so later stores
    /// are seen.
    pub fn probe(&mut self, va: u32, len: u32, tracker: &WriteTracker) -> Probe<T> {
        self.tick += 1;
        let now = tracker.generation();
        let (tick, frame) = (self.tick, self.frame);
        let Some(e) = self.entries.get_mut(&(va, len)) else {
            self.entries.insert(
                (va, len),
                Entry {
                    generation: now,
                    since_frame: frame,
                    used: tick,
                    strikes: 0,
                    snapshot: None,
                },
            );
            self.evict();
            return Probe::Copy;
        };
        e.used = tick;
        if tracker.dirty_since(va, len, e.generation) {
            if let Some(s) = e.snapshot.take() {
                self.bytes -= s.size();
            }
            e.strikes = e.strikes.saturating_add(1);
            e.generation = now;
            e.since_frame = frame;
            return Probe::Copy;
        }
        match &e.snapshot {
            Some(s) => Probe::Hit(s.clone()),
            None if e.strikes < DYNAMIC_STRIKES || frame >= e.since_frame + SETTLE_FRAMES => {
                Probe::Fill
            }
            None => Probe::Copy,
        }
    }

    /// Stores the snapshot `make` builds (given its unique id) from
    /// `[va, va + len)` as read at tracker generation `generation`, after a
    /// [`Probe::Fill`]; returns it.
    pub fn fill(&mut self, va: u32, len: u32, generation: u64, make: impl FnOnce(u64) -> T) -> T {
        let snapshot = make(next_snapshot_id());
        self.tick += 1;
        let e = self.entries.entry((va, len)).or_insert(Entry {
            generation,
            since_frame: self.frame,
            used: self.tick,
            strikes: 0,
            snapshot: None,
        });
        if let Some(old) = e.snapshot.replace(snapshot.clone()) {
            self.bytes -= old.size();
        }
        e.generation = generation;
        e.used = self.tick;
        self.bytes += snapshot.size();
        self.evict();
        snapshot
    }

    /// Drops the snapshot of `[va, va + len)` (its bytes changed without a
    /// tracker report: [`crate::UploadPolicy::Verify`] caught it). The
    /// bytes just read were taken at `generation`.
    pub fn invalidate(&mut self, va: u32, len: u32, generation: u64) {
        if let Some(e) = self.entries.get_mut(&(va, len)) {
            if let Some(s) = e.snapshot.take() {
                self.bytes -= s.size();
            }
            e.strikes = e.strikes.saturating_add(1);
            e.generation = generation;
            e.since_frame = self.frame;
        }
    }

    /// Counts a frame (for [`SETTLE_FRAMES`]).
    pub fn end_frame(&mut self) {
        self.frame += 1;
    }

    /// Identity of this cache since its last reset. A prepared packet can
    /// span a resource reset even when its old snapshot is still referenced.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.epoch = next_snapshot_id();
        self.entries.clear();
        self.bytes = 0;
    }

    /// Bytes held in snapshots.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Entries, ghosts included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Over budget: drops the least recently used entries until a quarter
    /// of the budget is free (so eviction runs rarely, not every fill).
    fn evict(&mut self) {
        if self.bytes <= self.budget && self.entries.len() <= MAX_ENTRIES {
            return;
        }
        let mut order: Vec<(u64, (u32, u32))> =
            self.entries.iter().map(|(k, e)| (e.used, *k)).collect();
        order.sort_unstable();
        let (bytes_goal, count_goal) = (self.budget / 4 * 3, MAX_ENTRIES / 4 * 3);
        for (_, k) in order {
            if self.bytes <= bytes_goal && self.entries.len() <= count_goal {
                break;
            }
            if let Some(e) = self.entries.remove(&k)
                && let Some(s) = e.snapshot
            {
                self.bytes -= s.size();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const VA: u32 = 0x8010_0000;

    fn fill(c: &mut VertexCache, t: &WriteTracker, va: u32, len: u32) -> CachedBytes {
        c.fill(va, len, t.generation(), |id| CachedBytes {
            id,
            bytes: Arc::from(vec![7; len as usize]),
        })
    }

    #[test]
    fn a_window_is_snapshotted_on_its_second_clean_use() {
        let (mut c, mut t) = (VertexCache::new(1 << 20), WriteTracker::new());
        assert!(matches!(c.probe(VA, 256, &t), Probe::Copy));
        t.watch(VA, 256);
        assert!(matches!(c.probe(VA, 256, &t), Probe::Fill));
        let s = fill(&mut c, &t, VA, 256);
        match c.probe(VA, 256, &t) {
            Probe::Hit(h) => assert_eq!(h.id, s.id),
            p => panic!("{p:?}"),
        }
        assert_eq!(c.bytes(), 256);
        // Another page's write leaves it alone; a write into it drops it.
        t.note_write(VA + 0x1000, 4);
        assert!(matches!(c.probe(VA, 256, &t), Probe::Hit(_)));
        t.note_write(VA + 255, 1);
        assert!(matches!(c.probe(VA, 256, &t), Probe::Copy));
        assert_eq!(c.bytes(), 0);
        // Refilled under a new id.
        assert!(matches!(c.probe(VA, 256, &t), Probe::Fill));
        let s2 = fill(&mut c, &t, VA, 256);
        assert_ne!(s2.id, s.id);
    }

    #[test]
    fn a_window_written_every_frame_stops_being_snapshotted() {
        let (mut c, mut t) = (VertexCache::new(1 << 20), WriteTracker::new());
        t.watch(VA, 4096);
        let mut fills = 0;
        for _ in 0..20 {
            t.note_write(VA, 64);
            // Drawn twice a frame: the second use is clean.
            for _ in 0..2 {
                match c.probe(VA, 4096, &t) {
                    Probe::Fill => {
                        fills += 1;
                        fill(&mut c, &t, VA, 4096);
                    }
                    Probe::Hit(_) | Probe::Copy => {}
                }
            }
            c.end_frame();
        }
        assert!(fills <= DYNAMIC_STRIKES as usize, "{fills} fills");
        // Once it stays clean for a while it is cached again.
        let mut hit = false;
        for _ in 0..=SETTLE_FRAMES + 1 {
            match c.probe(VA, 4096, &t) {
                Probe::Fill => {
                    fill(&mut c, &t, VA, 4096);
                }
                Probe::Hit(_) => hit = true,
                Probe::Copy => {}
            }
            c.end_frame();
        }
        assert!(hit);
    }

    #[test]
    fn snapshots_stay_within_the_budget_least_recently_used_first() {
        let (mut c, mut t) = (VertexCache::new(64 << 10), WriteTracker::new());
        for i in 0..32u32 {
            let va = VA + i * 0x4000;
            t.watch(va, 0x4000);
            c.probe(va, 0x4000, &t);
            assert!(matches!(c.probe(va, 0x4000, &t), Probe::Fill));
            fill(&mut c, &t, va, 0x4000);
            // Keep the first one warm.
            assert!(matches!(c.probe(VA, 0x4000, &t), Probe::Hit(_)));
            assert!(c.bytes() <= 64 << 10, "{} bytes", c.bytes());
        }
        assert!(matches!(c.probe(VA, 0x4000, &t), Probe::Hit(_)));
        assert!(!matches!(c.probe(VA + 0x4000, 0x4000, &t), Probe::Hit(_)));
    }
}
