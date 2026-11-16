//! Guest write watch: per-consumer dirty bits over physical RAM pages.
//!
//! Rust stores reach [`Guest::write_bytes`]. Optional inline C stores hold
//! the shared memory gate and accumulate physical dirty-page bits; the next
//! Runtime memory guard publishes them through this same watch before HLE
//! can observe or change subscriptions. This is a software barrier, not host
//! page protection.
//! Pages are physical, so every virtual alias of a page dirties the same bit;
//! ranges are reported in the contiguous view `0x80000000 | pa`.
use crate::Guest;
use crate::guest::{PAGE_SIZE, RAM_SIZE};

/// Base of the folded view: physical address `pa` is reported as
/// `CONTIGUOUS_BASE | pa`, the address D3D reads resource data at.
pub const CONTIGUOUS_BASE: u32 = crate::loader::CONTIGUOUS_ALIAS_BASE;
const PAGE_SHIFT: u32 = PAGE_SIZE.trailing_zeros();
const PAGES: usize = RAM_SIZE >> PAGE_SHIFT;
const WORDS: usize = PAGES / 64;
const _: () = assert!(PAGES.is_multiple_of(64) && 1 << PAGE_SHIFT == PAGE_SIZE);

/// One write-watch consumer; returned by [`Guest::subscribe_writes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchId(u32);

/// A run of dirty pages in the folded view: `addr` is `0x80000000 | pa` and
/// page-aligned, `len` a non-zero multiple of the page size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRange {
    pub addr: u32,
    pub len: u32,
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum WatchError {
    #[error("unknown write-watch subscription")]
    UnknownWatch,
    /// Zero length, or not wholly inside `[0x80000000, 0x80000000 + 64 MB)`.
    #[error("range is not inside the contiguous view of RAM")]
    InvalidRange,
}

type Bits = Box<[u64; WORDS]>;
fn bits() -> Bits {
    Box::new([0; WORDS])
}

struct Subscriber {
    id: WatchId,
    owner: &'static str,
    watched: Bits,
    dirty: Bits,
    any_dirty: bool,
    /// Words of `dirty` that may be non-zero: `dirty_words.0..dirty_words.1`.
    dirty_words: (usize, usize),
    /// Some bit of `watched` is set.
    watches_any: bool,
    muted: bool,
}

/// The watch state inside one [`Guest`].
pub(crate) struct WriteWatch {
    subscribers: Vec<Subscriber>,
    /// Union of every subscriber's watched pages.
    union: Bits,
    /// Some page is watched by an unmuted subscriber. Writes test only this
    /// flag while nothing is watched.
    active: bool,
    next_id: u32,
}
impl Default for WriteWatch {
    fn default() -> Self {
        Self {
            subscribers: Vec::new(),
            union: bits(),
            active: false,
            next_id: 1,
        }
    }
}

/// Word index and in-word mask for each word covering pages `[first, last]`.
fn words(first: usize, last: usize) -> impl Iterator<Item = (usize, u64)> {
    (first / 64..=last / 64).map(move |w| {
        let lo = if w == first / 64 { first % 64 } else { 0 };
        let hi = if w == last / 64 { last % 64 } else { 63 };
        (w, (u64::MAX >> (63 - hi)) & (u64::MAX << lo))
    })
}

impl WriteWatch {
    #[inline(always)]
    pub(crate) fn active(&self) -> bool {
        self.active
    }
    /// Record a change to physical `[start, start + len)`. `len` is non-zero
    /// and the range lies in RAM.
    #[inline]
    pub(crate) fn note(&mut self, start: usize, len: usize) {
        let first = start >> PAGE_SHIFT;
        let last = (start + len - 1) >> PAGE_SHIFT;
        // The usual store is in one page nobody watches: one bit test.
        if first == last && self.union[first / 64] & (1 << (first % 64)) == 0 {
            return;
        }
        self.mark(first, last);
    }
    #[inline(never)]
    fn mark(&mut self, first: usize, last: usize) {
        for (w, mask) in words(first, last) {
            let hit = self.union[w] & mask;
            if hit == 0 {
                continue;
            }
            for s in &mut self.subscribers {
                let mine = hit & s.watched[w];
                if mine != 0 && !s.muted {
                    s.dirty[w] |= mine;
                    s.any_dirty = true;
                    s.dirty_words = (s.dirty_words.0.min(w), s.dirty_words.1.max(w + 1));
                }
            }
        }
    }
    /// Recompute the union after a watched set changed. O(subscribers * 256).
    fn refresh(&mut self) {
        self.union.fill(0);
        for s in &mut self.subscribers {
            for (u, w) in self.union.iter_mut().zip(s.watched.iter()) {
                *u |= w;
            }
            s.watches_any = s.watched.iter().any(|&w| w != 0);
        }
        self.refresh_active();
    }
    /// Recompute `active` after muting changed. O(subscribers).
    fn refresh_active(&mut self) {
        self.active = self.subscribers.iter().any(|s| s.watches_any && !s.muted);
    }
    fn get(&mut self, id: WatchId) -> Result<&mut Subscriber, WatchError> {
        self.subscribers
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(WatchError::UnknownWatch)
    }
}

/// Folded `[addr, addr + len)` to an inclusive physical page range.
fn pages(addr: u32, len: u32) -> Result<(usize, usize), WatchError> {
    let start = (addr as u64)
        .checked_sub(CONTIGUOUS_BASE as u64)
        .ok_or(WatchError::InvalidRange)?;
    let end = start + len as u64;
    if len == 0 || end > RAM_SIZE as u64 {
        return Err(WatchError::InvalidRange);
    }
    Ok((
        (start >> PAGE_SHIFT) as usize,
        ((end - 1) >> PAGE_SHIFT) as usize,
    ))
}

impl Guest {
    /// Start a write-watch consumer with no watched pages. Each subscriber
    /// has its own watched and dirty sets; draining one never affects another.
    pub fn subscribe_writes(&mut self, owner: &'static str) -> WatchId {
        let w = &mut self.watch;
        let id = WatchId(w.next_id);
        w.next_id += 1;
        w.subscribers.push(Subscriber {
            id,
            owner,
            watched: bits(),
            dirty: bits(),
            any_dirty: false,
            dirty_words: (WORDS, 0),
            watches_any: false,
            muted: false,
        });
        id
    }
    /// Drop a subscriber and its undrained dirty pages. False if unknown.
    pub fn unsubscribe_writes(&mut self, id: WatchId) -> bool {
        let before = self.watch.subscribers.len();
        self.watch.subscribers.retain(|s| s.id != id);
        self.watch.refresh();
        self.watch.subscribers.len() != before
    }
    /// Owner names of the live subscribers, in subscription order.
    pub fn write_watch_owners(&self) -> Vec<(WatchId, &'static str)> {
        self.watch
            .subscribers
            .iter()
            .map(|s| (s.id, s.owner))
            .collect()
    }
    /// Watch every page touching folded `[addr, addr + len)` (rounded out to
    /// pages). Watching an already watched page is a no-op. Only writes made
    /// after this call are reported; it does not mark anything dirty.
    pub fn watch_writes(&mut self, id: WatchId, addr: u32, len: u32) -> Result<(), WatchError> {
        let (first, last) = pages(addr, len)?;
        let s = self.watch.get(id)?;
        for (w, mask) in words(first, last) {
            s.watched[w] |= mask;
        }
        self.watch.refresh();
        Ok(())
    }
    /// Stop watching every page touching folded `[addr, addr + len)` and
    /// discard their undrained dirty bits.
    pub fn unwatch_writes(&mut self, id: WatchId, addr: u32, len: u32) -> Result<(), WatchError> {
        let (first, last) = pages(addr, len)?;
        let s = self.watch.get(id)?;
        for (w, mask) in words(first, last) {
            s.watched[w] &= !mask;
            s.dirty[w] &= !mask;
        }
        s.any_dirty = s.dirty.iter().any(|&w| w != 0);
        self.watch.refresh();
        Ok(())
    }
    /// True if a watched page changed since the last drain. O(1).
    pub fn has_dirty(&self, id: WatchId) -> bool {
        self.watch
            .subscribers
            .iter()
            .any(|s| s.id == id && s.any_dirty)
    }
    /// Return and clear the dirty pages, coalesced into ascending, disjoint,
    /// non-adjacent runs in the folded view. Empty when nothing changed.
    pub fn take_dirty(&mut self, id: WatchId) -> Result<Vec<DirtyRange>, WatchError> {
        let s = self.watch.get(id)?;
        let mut out: Vec<DirtyRange> = Vec::new();
        let range = |(a, b): (usize, usize)| DirtyRange {
            addr: CONTIGUOUS_BASE | ((a as u32) << PAGE_SHIFT),
            len: ((b - a) as u32) << PAGE_SHIFT,
        };
        if !s.any_dirty {
            return Ok(Vec::new());
        }
        s.any_dirty = false;
        let (lo, hi) = std::mem::replace(&mut s.dirty_words, (WORDS, 0));
        let mut run: Option<(usize, usize)> = None;
        for w in lo..hi {
            let mut word = std::mem::take(&mut s.dirty[w]);
            // A run of set bits at a time, not a bit at a time.
            while word != 0 {
                let low = word.trailing_zeros() as usize;
                let ones = (word >> low).trailing_ones() as usize;
                let (page, next) = (w * 64 + low, w * 64 + low + ones);
                word &= if low + ones == 64 {
                    0
                } else {
                    u64::MAX << (low + ones)
                };
                run = match run {
                    Some((start, end)) if end == page => Some((start, next)),
                    Some(done) => {
                        out.push(range(done));
                        Some((page, next))
                    }
                    None => Some((page, next)),
                };
            }
        }
        out.extend(run.map(range));
        Ok(out)
    }
    /// Run `f` with `id` muted: its writes, and any other write made through
    /// this guard meanwhile, do not dirty `id`'s pages. Other subscribers still
    /// see them. This is how a consumer excludes its own stores. Muting is
    /// restored even if `f` panics.
    pub fn writes_unreported<R>(
        &mut self,
        id: WatchId,
        f: impl FnOnce(&mut Guest) -> R,
    ) -> Result<R, WatchError> {
        let previous = std::mem::replace(&mut self.watch.get(id)?.muted, true);
        self.watch.refresh_active();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        if let Ok(s) = self.watch.get(id) {
            s.muted = previous;
        }
        self.watch.refresh_active();
        match result {
            Ok(r) => Ok(r),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    /// The folded address (`0x80000000 | pa`) of mapped virtual `va`, or None
    /// if `va` is unmapped. Translation uses the current mappings; it is how a
    /// consumer that knows only a virtual address finds the page to watch.
    pub fn fold_address(&self, va: u32) -> Option<u32> {
        let m = &self.mappings()[self.mapping_index(va as u64)?];
        Some(CONTIGUOUS_BASE | (m.physical_start + (va - m.virtual_start)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word_masks_cover_exactly_the_page_span() {
        assert_eq!(words(3, 3).collect::<Vec<_>>(), vec![(0, 1 << 3)]);
        assert_eq!(
            words(62, 65).collect::<Vec<_>>(),
            vec![(0, 0b11 << 62), (1, 0b11)]
        );
        assert_eq!(
            words(0, 191).collect::<Vec<_>>(),
            vec![(0, u64::MAX), (1, u64::MAX), (2, u64::MAX)]
        );
    }
    #[test]
    fn folded_ranges_are_bounded_to_ram() {
        assert_eq!(pages(0x8000_0000, 1), Ok((0, 0)));
        assert_eq!(pages(0x8000_0fff, 2), Ok((0, 1)));
        assert_eq!(pages(0x83ff_f000, 0x1000), Ok((PAGES - 1, PAGES - 1)));
        assert_eq!(pages(0x83ff_f000, 0x1001), Err(WatchError::InvalidRange));
        assert_eq!(pages(0x7fff_f000, 0x1000), Err(WatchError::InvalidRange));
        assert_eq!(pages(0x8000_0000, 0), Err(WatchError::InvalidRange));
    }
}
