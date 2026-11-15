//! Write tracking for guest memory that host copies were made from.
//!
//! # The invalidation contract
//!
//! Guest code writes texture, vertex and index data straight into memory;
//! on the Xbox the GPU reads the same bytes, so nothing tells D3D the data
//! changed (`Lock` is a no-op in 5849 for most resources, and Halo inlines
//! or skips it). The device therefore caches host copies (wgpu textures)
//! keyed by header address, and needs to learn about writes to their bytes.
//!
//! 1. **Unit.** Writes are tracked per 4 KiB page of *guest virtual*
//!    address, as seen through [`GuestMemory`](crate::guest::GuestMemory).
//!    Aliases (the Xbox maps the same physical page at several virtual
//!    addresses: `0x80000000|pa`, write-combined `0xF0000000|pa`, and the
//!    title's own arena mapping) must be folded by the runtime: it reports a
//!    write once per alias the device reads through, or (simpler) always in
//!    the contiguous alias the device reads (`AddressMap::to_virtual`).
//! 2. **Who reports.** Every writer of guest memory that is not this device:
//!    generated code stores, kernel services that fill buffers (`NtReadFile`
//!    into a texture, `memcpy` HLE), other HLE crates. Calls are
//!    [`WriteTracker::note_write`]`(addr, len)`. The device's own writes
//!    (headers it creates, state arrays, push-buffer bookkeeping) are not
//!    reported and need not be.
//! 3. **When.** Before the next D3D entry point runs. D3D calls are
//!    synchronous on the calling guest thread, so a write barrier in the
//!    store path (or a page-fault handler that reports the first write to a
//!    write-protected page and then unprotects it) is sufficient. Writes from
//!    other guest threads must be reported before that thread releases
//!    whatever lock orders it with the rendering thread; the device holds no
//!    knowledge of guest synchronisation.
//! 4. **Filtering.** Reporting every store is expensive; the runtime may
//!    restrict itself to *watched* pages: [`WriteTracker::is_watched`] is a
//!    bitmap lookup (and [`WriteTracker::watched_bitmap`] exposes the bitmap
//!    for generated code). A page becomes watched when the device makes a
//!    host copy from it, and stays watched until [`WriteTracker::unwatch_all`].
//!    Writes to unwatched pages may be dropped.
//! 5. **Fallbacks.** If a runtime cannot honour (2)-(4) yet,
//!    [`UploadPolicy::Always`] re-reads every resource on every use, and
//!    [`UploadPolicy::Verify`] trusts the tracker but also hashes the bytes on
//!    use and raises [`crate::Event::MissedInvalidation`] when they changed
//!    without a report, which pinpoints the missing barrier.
//!
//! Generations: each page has a counter, bumped by `note_write`. A host copy
//! remembers the tracker's global generation at the time it read the bytes;
//! it is stale iff any page it covers has a newer generation.

/// Tracking granularity.
pub const PAGE_SHIFT: u32 = 12;
const WATCH_WORDS: usize = (1usize << (32 - PAGE_SHIFT)) / 64;

/// How the device decides whether a cached host copy is still valid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UploadPolicy {
    /// Trust [`WriteTracker`] reports (the contract above).
    #[default]
    Tracked,
    /// Trust reports, but also hash the guest bytes on every use and report
    /// unreported changes (debugging the runtime's write barrier).
    Verify,
    /// Re-read and re-upload on every use.
    Always,
}

/// Page generations plus the watched-page bitmap.
pub struct WriteTracker {
    generation: u64,
    // Sparse two-level array: no hashing on the per-draw query, 8 KiB per
    // populated 4 MiB region, with the same full 32-bit virtual page keys.
    pages: Vec<Option<Box<[u64; 1024]>>>,
    watched: Vec<u64>,
}

impl Default for WriteTracker {
    fn default() -> Self {
        Self {
            generation: 1,
            pages: (0..1024).map(|_| None).collect(),
            watched: vec![0; WATCH_WORDS],
        }
    }
}

impl WriteTracker {
    /// A tracker with nothing watched.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current global generation. A copy taken now is valid until a
    /// covered page is written.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Reports a guest write to `[addr, addr + len)`.
    pub fn note_write(&mut self, addr: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.generation += 1;
        let g = self.generation;
        for p in pages(addr, len) {
            if self.watched_page(p) {
                self.pages[(p >> 10) as usize].get_or_insert_with(|| Box::new([0; 1024]))
                    [(p & 1023) as usize] = g;
            }
        }
    }

    /// Whether any page of `[addr, addr + len)` was written after `since`.
    pub fn dirty_since(&self, addr: u32, len: u32, since: u64) -> bool {
        pages(addr, len).any(|p| {
            self.pages[(p >> 10) as usize]
                .as_ref()
                .is_some_and(|chunk| chunk[(p & 1023) as usize] > since)
        })
    }

    /// Starts watching `[addr, addr + len)`.
    pub fn watch(&mut self, addr: u32, len: u32) {
        for p in pages(addr, len) {
            self.watched[(p / 64) as usize] |= 1 << (p % 64);
        }
    }

    /// Stops watching everything (e.g. on device reset). Cached copies must
    /// be dropped at the same time, since writes are no longer recorded.
    pub fn unwatch_all(&mut self) {
        self.watched.iter_mut().for_each(|w| *w = 0);
        self.pages.iter_mut().for_each(|p| *p = None);
    }

    /// Whether a write at `addr` must be reported.
    pub fn is_watched(&self, addr: u32) -> bool {
        self.watched_page(addr >> PAGE_SHIFT)
    }

    /// The watched-page bitmap: bit `p % 64` of word `p / 64` is page `p`
    /// (`p = addr >> PAGE_SHIFT`), 128 KiB for the 4 GiB space.
    pub fn watched_bitmap(&self) -> &[u64] {
        &self.watched
    }

    fn watched_page(&self, p: u32) -> bool {
        self.watched[(p / 64) as usize] & (1 << (p % 64)) != 0
    }
}

fn pages(addr: u32, len: u32) -> impl Iterator<Item = u32> {
    let first = addr >> PAGE_SHIFT;
    let last = ((addr as u64 + len.max(1) as u64 - 1) >> PAGE_SHIFT)
        .min(u32::MAX as u64 >> PAGE_SHIFT) as u32;
    first..=last
}

/// FNV-1a, for [`UploadPolicy::Verify`].
pub(crate) fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_generations_match_page_map_across_the_address_space() {
        use std::collections::HashMap;
        let mut t = WriteTracker::new();
        let mut reference = HashMap::new();
        let mut rng = 0x713f_ab19u32;
        for addr in [0, 0x3ff000, 0x400000, 0x80001000, 0xfffff000] {
            t.watch(addr, 4096);
        }
        for i in 0..10_000 {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let addr = if i % 5 == 0 { u32::MAX - 100 } else { rng };
            let len = rng & 0x3fff;
            if i % 3 == 0 {
                t.watch(addr, len);
            }
            let before = t.generation();
            t.note_write(addr, len);
            if len != 0 {
                for p in pages(addr, len) {
                    if t.is_watched(p << PAGE_SHIFT) {
                        reference.insert(p, t.generation());
                    }
                }
            }
            for since in [0, before, t.generation()] {
                let expected =
                    pages(addr, len).any(|p| reference.get(&p).is_some_and(|g| *g > since));
                assert_eq!(t.dirty_since(addr, len, since), expected);
            }
            if i % 701 == 0 {
                t.unwatch_all();
                reference.clear();
            }
        }
    }

    #[test]
    #[ignore = "microbenchmark; run release with --nocapture"]
    fn dirty_query_benchmark() {
        use std::{collections::HashMap, hint::black_box, time::Instant};
        let mut t = WriteTracker::new();
        let mut reference = HashMap::new();
        for p in 0..1024u32 {
            let addr = 0x80000000 + p * 4096;
            t.watch(addr, 4096);
            t.note_write(addr, 4096);
            reference.insert(addr >> 12, t.generation());
        }
        let since = t.generation();
        for old in [true, false] {
            let start = Instant::now();
            let mut dirty = 0;
            for i in 0..2_000_000 {
                let addr = black_box(0x80000000 + (i & 1023) * 4096);
                dirty += if old {
                    pages(addr, 16384)
                        .any(|p| reference.get(&p).is_some_and(|g| *g > black_box(since)))
                } else {
                    t.dirty_since(addr, 16384, black_box(since))
                } as u64;
            }
            eprintln!(
                "dirty_query old={old} ns/query={:.3} sink={dirty}",
                start.elapsed().as_nanos() as f64 / 2_000_000.0
            );
        }
    }

    #[test]
    fn writes_only_count_on_watched_pages() {
        let mut t = WriteTracker::new();
        let g0 = t.generation();
        t.note_write(0x8000_1000, 4);
        assert!(
            !t.dirty_since(0x8000_1000, 4, g0),
            "unwatched writes are dropped"
        );
        t.watch(0x8000_1000, 0x2000);
        assert!(t.is_watched(0x8000_2fff) && !t.is_watched(0x8000_3000));
        let g1 = t.generation();
        t.note_write(0x8000_2ffc, 8); // straddles into an unwatched page
        assert!(t.dirty_since(0x8000_1000, 0x2000, g1));
        assert!(
            !t.dirty_since(0x8000_1000, 0x1000, g1),
            "first page untouched"
        );
        let g2 = t.generation();
        assert!(!t.dirty_since(0x8000_1000, 0x2000, g2));
        t.unwatch_all();
        assert!(!t.is_watched(0x8000_1000));
    }
}
