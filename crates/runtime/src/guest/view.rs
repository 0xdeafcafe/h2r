//! Copy-on-write views of a Guest for a second executor, and the hand-back
//! of what a view wrote (docs/runtime/second-executor.md, section 2).

use super::{Guest, PAGE_SIZE, RAM_SIZE, fresh_layout};
use crate::cow::{CopyKind, Ram};
use std::sync::atomic::Ordering;

/// Bytes per line in [`WrittenPage::lines`].
pub const LINE_SIZE: usize = 64;
const LINES: usize = PAGE_SIZE as usize / LINE_SIZE;
const _: () = assert!(LINES == 64);

/// A physical page a view stored to: `lines` has bit `i` set when bytes
/// `[i * 64, i * 64 + 64)` of the page differ from the snapshot's. A page
/// stored to only with the bytes it already held has `lines == 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrittenPage {
    /// Page-aligned physical address.
    pub physical: u32,
    pub lines: u64,
}

/// What [`Guest::hand_back`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HandBack {
    /// Lines merged (set bits of the pages' `lines`).
    pub lines: usize,
    /// Bytes the view had changed, now in the live Guest.
    pub bytes: usize,
    /// Bytes both sides changed, in a word both changed, where the view's
    /// byte differs from the live side's: counted whichever side's word
    /// stayed.
    pub conflicts: usize,
    /// Of `conflicts`, those in render-owned ranges, where the view's word
    /// won.
    pub owned_conflicts: usize,
}

/// A frozen copy of physical RAM: what a view started from.
pub struct FrozenRam(Ram);
impl FrozenRam {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn kind_is_cow(&self) -> bool {
        self.0.is_mapped()
    }
}

impl Guest {
    /// A Guest with this one's mappings, reservations, claims and code
    /// guards over a copy-on-write copy of its RAM, and how the copy was
    /// made. Neither side sees the other's later writes. It has no write
    /// watch subscribers, no access trap and a page table only if this one
    /// has one.
    pub fn cow_snapshot(&self) -> (Guest, CopyKind) {
        let (ram, kind) = self.ram.cow_copy();
        let mut view = Guest {
            #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
            flat: None,
            #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
            flat_requested: false,
            ram,
            mappings: Vec::new(),
            mapping_hints: std::array::from_fn(|_| Default::default()),
            fast_pages: None,
            next_physical: 0,
            reservations: Vec::new(),
            claims: Vec::new(),
            code: Vec::new(),
            next_id: 1,
            watch: Default::default(),
            hle_writes: None,
            layout: fresh_layout(),
            trap: None,
            cow_source_layout: 0, watch_revision: 0,
        };
        view.copy_layout_from(self);
        if self.fast_pages.is_some() {
            view.prepare_fast_pages();
        }
        (view, kind)
    }

    /// Make this Guest a fresh copy-on-write view of `source`, as
    /// [`cow_snapshot`](Self::cow_snapshot) would, keeping this Guest's
    /// write watch subscriptions (with nothing dirty) and, when `source`'s
    /// layout is the one it last copied, its page table and layout: then
    /// the cost is the RAM copy alone. Call it under this Guest's memory
    /// guard, and with `source` borrowed under its own, so no store to
    /// either is in flight.
    pub fn cow_refresh(&mut self, source: &Guest) -> CopyKind {
        self.invalidate_hle_capture();
        let (ram, kind) = source.ram.cow_copy();
        #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
        {
            self.flat = None;
            // A refreshed executor view keeps its ordinary page table and
            // spends no time installing aliases of a short-lived copy.
            self.flat_requested = false;
        }
        self.ram = ram;
        self.watch_revision = self.watch_revision.wrapping_add(1);
        if self.cow_source_layout != source.layout {
            self.copy_layout_from(source);
        }
        self.watch.clear_dirty();
        kind
    }

    fn copy_layout_from(&mut self, source: &Guest) {
        self.mappings = source.mappings.clone();
        self.next_physical = source.next_physical;
        self.reservations = source.reservations.clone();
        self.claims = source.claims.clone();
        self.code = source.code.clone();
        self.next_id = source.next_id;
        for hint in &self.mapping_hints {
            hint.store(usize::MAX, Ordering::Relaxed);
        }
        // A view's pages are never armed: drop any trap.
        self.trap = None;
        // Moves the layout on, and rebuilds the page table if there is one.
        self.refresh_fast_range(0, 1u64 << 32);
        self.cow_source_layout = source.layout;
    }

    /// A frozen copy of physical RAM as it is now (copy-on-write where
    /// [`cow_snapshot`](Self::cow_snapshot) is): the base a view's writes
    /// are compared and merged against.
    pub fn freeze_ram(&self) -> (FrozenRam, CopyKind) {
        let (ram, kind) = self.ram.cow_copy();
        (FrozenRam(ram), kind)
    }

    /// Bit `i` set where line `i` of physical page `physical` (page-aligned)
    /// differs between this Guest's RAM and `base`.
    pub fn changed_lines(&self, base: &FrozenRam, physical: u32) -> u64 {
        changed_lines(&self.ram, base.bytes(), physical as usize)
    }

    /// Put physical `[physical, physical + len)` back to `base`'s bytes,
    /// without telling the write watch: a view's change there is then no
    /// change at all ([`changed_lines`](Self::changed_lines) and
    /// [`hand_back`](Self::hand_back) skip it). The range is clipped to RAM.
    pub fn restore_from(&mut self, base: &FrozenRam, physical: u32, len: u32) {
        let start = (physical as usize).min(RAM_SIZE);
        let end = (physical as usize + len as usize).min(RAM_SIZE);
        self.invalidate_hle_capture();
        self.ram[start..end].copy_from_slice(&base.bytes()[start..end]);
        self.watch_revision = self.watch_revision.wrapping_add(1);
    }

    /// Merge what a view changed into this (live) Guest, over `written`
    /// (from the view's write watch and [`changed_lines`](Self::changed_lines)):
    /// every byte of a listed line where `view` differs from `base` takes
    /// `view`'s value, unless this Guest changed the same aligned word
    /// since `base` too: that word keeps this Guest's value, whole (a
    /// conflict). A pipelined render's view runs before the game thread's
    /// tick in serial order, so where both wrote, the tick's write is the
    /// later one, and a word is never left part one side's and part the
    /// other's. Every other byte keeps this Guest's. Pages are
    /// reported to this Guest's write watch. Call it under this Guest's
    /// memory guard.
    pub fn hand_back(
        &mut self,
        view: &Guest,
        base: &FrozenRam,
        written: &[WrittenPage],
    ) -> HandBack {
        self.hand_back_owned(view, base, written, &[])
    }

    /// [`hand_back`](Self::hand_back), except that in the physical ranges
    /// `owned` (`[start, end)`) the view owns the words: a word both sides
    /// changed takes the view's value whole (still a counted conflict, and
    /// an `owned_conflict`). For state only the render may change between
    /// its launch and its join, such as the D3D device and its push buffer,
    /// where a live change is not a later serial write.
    pub fn hand_back_owned(
        &mut self,
        view: &Guest,
        base: &FrozenRam,
        written: &[WrittenPage],
        owned: &[(u32, u32)],
    ) -> HandBack {
        self.invalidate_hle_capture();
        let base = base.bytes();
        let mut out = HandBack::default();
        for page in written {
            if page.lines == 0 {
                continue;
            }
            let start = page.physical as usize & !(PAGE_SIZE as usize - 1);
            if start >= RAM_SIZE {
                continue;
            }
            let mut lines = page.lines;
            while lines != 0 {
                let i = lines.trailing_zeros() as usize;
                lines &= lines - 1;
                out.lines += 1;
                let at = start + i * LINE_SIZE;
                let line = at..at + LINE_SIZE;
                let live = &mut self.ram[line.clone()];
                let (view, base) = (&view.ram[line.clone()], &base[line]);
                // Word by word: a word this side changed too keeps this
                // side's value whole (the conflict), any other word takes
                // the view's changed bytes.
                for w in (0..LINE_SIZE).step_by(4) {
                    let word = w..w + 4;
                    if view[word.clone()] == base[word.clone()] {
                        continue;
                    }
                    if live[word.clone()] != base[word.clone()] {
                        let n = (w..w + 4)
                            .filter(|&k| view[k] != base[k] && live[k] != view[k])
                            .count();
                        out.conflicts += n;
                        let physical = (at + w) as u32;
                        if owned.iter().any(|&(s, e)| (s..e).contains(&physical)) {
                            out.owned_conflicts += n;
                            out.bytes += (w..w + 4).filter(|&k| live[k] != view[k]).count();
                            live[word.clone()].copy_from_slice(&view[word]);
                        }
                        continue;
                    }
                    for k in word {
                        if view[k] != base[k] {
                            live[k] = view[k];
                            out.bytes += 1;
                        }
                    }
                }
            }
            if self.watch.active() {
                self.watch.note(start, PAGE_SIZE as usize);
            }
        }
        out
    }
}

impl Guest {
    /// The bytes [`hand_back`](Self::hand_back) would count as conflicts,
    /// read only: `(physical, live, view, base)`, at most `limit`.
    pub fn conflicts_with(
        &self,
        view: &Guest,
        base: &FrozenRam,
        written: &[WrittenPage],
        limit: usize,
    ) -> Vec<(u32, u8, u8, u8)> {
        let base = base.bytes();
        let mut out = Vec::new();
        for page in written {
            let start = page.physical as usize & !(PAGE_SIZE as usize - 1);
            if start >= RAM_SIZE {
                continue;
            }
            let mut lines = page.lines;
            while lines != 0 {
                let i = lines.trailing_zeros() as usize;
                lines &= lines - 1;
                let at = start + i * LINE_SIZE;
                for word in (at..at + LINE_SIZE).step_by(4) {
                    let word = word..word + 4;
                    if self.ram[word.clone()] == base[word.clone()] {
                        continue;
                    }
                    for k in word {
                        let (l, v, was) = (self.ram[k], view.ram[k], base[k]);
                        if v != was && l != v {
                            if out.len() == limit {
                                return out;
                            }
                            out.push((k as u32, l, v, was));
                        }
                    }
                }
            }
        }
        out
    }
}

impl Guest {
    /// [`hand_back`](Self::hand_back) for one page against `reference` (the
    /// page's bytes as `view` had them when they were last merged, or the
    /// snapshot's): every byte where `view` differs from `reference` takes
    /// `view`'s value, except in the physical ranges `skip`. Reported to
    /// this Guest's write watch. Call it under this Guest's memory guard.
    pub fn merge_page(
        &mut self,
        view: &Guest,
        reference: &[u8],
        physical: u32,
        skip: &[(u32, u32)],
    ) -> HandBack {
        let start = physical as usize & !(PAGE_SIZE as usize - 1);
        let mut out = HandBack::default();
        if start >= RAM_SIZE || reference.len() != PAGE_SIZE as usize {
            return out;
        }
        self.invalidate_hle_capture();
        let end = start + PAGE_SIZE as usize;
        let skip: Vec<(usize, usize)> = skip
            .iter()
            .map(|&(s, n)| (s as usize, s as usize + n as usize))
            .filter(|&(s, e)| s < end && e > start)
            .collect();
        let skipped = |at: usize| skip.iter().any(|&(s, e)| (s..e).contains(&at));
        for (i, &was) in reference.iter().enumerate() {
            let at = start + i;
            let v = view.ram[at];
            if v == was || skipped(at) {
                continue;
            }
            let l = &mut self.ram[at];
            if *l != was && *l != v {
                out.conflicts += 1;
            }
            *l = v;
            out.bytes += 1;
        }
        if out.bytes != 0 && self.watch.active() {
            self.watch.note(start, PAGE_SIZE as usize);
        }
        out
    }

    /// The virtual addresses mapping physical byte `physical`, lowest
    /// mapping first (diagnostics).
    pub fn virtual_of(&self, physical: u32) -> Vec<u32> {
        self.mappings
            .iter()
            .filter(|m| {
                (m.physical_start..m.physical_start.wrapping_add(m.len)).contains(&physical)
            })
            .map(|m| m.virtual_start + (physical - m.physical_start))
            .collect()
    }

    /// The bytes of physical page `physical` (page-aligned).
    pub fn physical_page(&self, physical: u32) -> Option<&[u8]> {
        let start = physical as usize & !(PAGE_SIZE as usize - 1);
        self.ram.get(start..start + PAGE_SIZE as usize)
    }
}

fn changed_lines(ram: &[u8], base: &[u8], physical: usize) -> u64 {
    let start = physical & !(PAGE_SIZE as usize - 1);
    if start >= RAM_SIZE {
        return 0;
    }
    let page = &ram[start..start + PAGE_SIZE as usize];
    let was = &base[start..start + PAGE_SIZE as usize];
    let mut lines = 0;
    let (page, was) = (
        page.as_chunks::<LINE_SIZE>().0,
        was.as_chunks::<LINE_SIZE>().0,
    );
    for (i, (a, b)) in page.iter().zip(was).enumerate() {
        if a != b {
            lines |= 1 << i;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest::Permissions;
    use crate::watch::CONTIGUOUS_BASE;

    fn live() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x10000, 0x10000, Permissions::RW).unwrap();
        g.map_zeroed(0x40000, 0x4000, Permissions::READ).unwrap();
        g.prepare_fast_pages();
        g
    }

    #[test]
    fn a_view_reads_the_snapshot_and_keeps_its_writes() {
        let mut live = live();
        live.set_u32(0x10000, 1).unwrap();
        live.set_u32(0x1f000, 2).unwrap();
        let (mut view, kind) = live.cow_snapshot();
        if cfg!(target_os = "macos") {
            assert_eq!(kind, CopyKind::Remapped);
        }
        assert_eq!(view.mappings(), live.mappings());
        assert_eq!(view.layout(), live.layout());
        assert_ne!(view.layout_id(), live.layout_id());
        live.set_u32(0x10000, 10).unwrap();
        view.set_u32(0x1f000, 20).unwrap();
        view.set_u32(0x14000, 21).unwrap();
        assert_eq!(
            (view.u32(0x10000).unwrap(), view.u32(0x1f000).unwrap()),
            (1, 20)
        );
        assert_eq!(
            (
                live.u32(0x10000).unwrap(),
                live.u32(0x1f000).unwrap(),
                live.u32(0x14000).unwrap()
            ),
            (10, 2, 0)
        );
        // Permissions came along.
        assert!(view.set_u32(0x40000, 1).is_err());
    }

    #[test]
    fn refresh_keeps_the_page_table_until_the_layout_moves() {
        let mut live = live();
        let (mut view, _) = live.cow_snapshot();
        let id = view.subscribe_writes("test");
        view.watch_writes(id, CONTIGUOUS_BASE, RAM_SIZE as u32)
            .unwrap();
        view.set_u32(0x10000, 5).unwrap();
        let layout = view.layout_id();
        live.set_u32(0x10004, 6).unwrap();
        view.cow_refresh(&live);
        assert_eq!(
            view.layout_id(),
            layout,
            "same source layout: nothing rebuilt"
        );
        assert!(!view.has_dirty(id), "a refresh starts clean");
        assert_eq!(
            (view.u32(0x10000).unwrap(), view.u32(0x10004).unwrap()),
            (0, 6)
        );
        live.map_zeroed(0x80000, 0x1000, Permissions::RW).unwrap();
        live.set_u32(0x80000, 7).unwrap();
        view.cow_refresh(&live);
        assert_ne!(view.layout_id(), layout);
        assert_eq!(view.mappings(), live.mappings());
        assert_eq!(view.u32(0x80000).unwrap(), 7);
        view.set_u32(0x80000, 8).unwrap();
        assert_eq!(view.take_dirty(id).unwrap().len(), 1);
    }

    #[test]
    fn written_lines_are_exactly_the_changed_lines_and_hand_back_merges_bytes() {
        let mut live = live();
        for i in 0..0x10000u32 / 4 {
            live.set_u32(0x10000 + i * 4, i).unwrap();
        }
        let (base, _) = live.freeze_ram();
        let (mut view, _) = live.cow_snapshot();
        let id = view.subscribe_writes("test");
        view.watch_writes(id, CONTIGUOUS_BASE, RAM_SIZE as u32)
            .unwrap();
        // View: two lines of page 0x10000, one byte of 0x11000, an
        // unchanged store to 0x12000, and a line crossing into 0x13040.
        view.set_u32(0x10000, 0xaaaa_aaaa).unwrap();
        view.set_u8(0x10fff, 0xbb).unwrap();
        view.set_u8(0x11040, 0xcc).unwrap();
        view.set_u32(0x12000, live.u32(0x12000).unwrap()).unwrap();
        view.write_bytes(0x1303e, &[0xdd; 4], 1).unwrap();
        // Live: another byte of a line the view changed, and the same byte.
        live.set_u8(0x10001, 0x11).unwrap();
        live.set_u8(0x11040, 0x22).unwrap();
        // The first mapping is at physical 0.
        let mut written: Vec<WrittenPage> = view
            .take_dirty(id)
            .unwrap()
            .into_iter()
            .flat_map(|r| (r.addr..r.addr + r.len).step_by(PAGE_SIZE as usize))
            .map(|a| a & !CONTIGUOUS_BASE)
            .map(|physical| WrittenPage {
                physical,
                lines: view.changed_lines(&base, physical),
            })
            .collect();
        written.sort_by_key(|p| p.physical);
        // The conflicting bytes (the view's of the two words both sides
        // changed), found before the merge without making it.
        let found = live.conflicts_with(&view, &base, &written, 8);
        let at: Vec<u32> = found.iter().map(|c| c.0).collect();
        assert_eq!(
            at,
            vec![0x0000, 0x0001, 0x0002, 0x0003, 0x1040],
            "{found:x?}"
        );
        assert_eq!(found[4].1..=found[4].2, 0x22..=0xcc);
        assert_eq!(live.conflicts_with(&view, &base, &written, 1).len(), 1);
        assert_eq!(live.u8(0x10001).unwrap(), 0x11, "nothing merged");
        assert_eq!(live.virtual_of(0x1040), vec![0x11040]);
        assert_eq!(
            written,
            vec![
                WrittenPage {
                    physical: 0x0000,
                    lines: 1 | 1 << 63
                },
                WrittenPage {
                    physical: 0x1000,
                    lines: 1 << 1
                },
                WrittenPage {
                    physical: 0x2000,
                    lines: 0
                },
                WrittenPage {
                    physical: 0x3000,
                    lines: 1 | 1 << 1
                },
            ]
        );
        let before = live.u32(0x10000).unwrap();
        // The word at 0x10000 and the byte at 0x11040 both sides changed:
        // the live side's word stays whole at 0x10000 (4 of the view's
        // bytes not taken); 0x11040 is in a view-owned range, so the view's
        // word lands there whole. The view's other changes land.
        let merged = live.hand_back_owned(&view, &base, &written, &[(0x1040, 0x1044)]);
        assert_eq!(
            merged,
            HandBack {
                lines: 5,
                bytes: 1 + 4 + 1,
                conflicts: 4 + 1,
                owned_conflicts: 1
            }
        );
        assert_eq!(merged.conflicts, found.len());
        assert_eq!(
            live.u32(0x10000).unwrap(),
            before,
            "conflict: the live word, whole"
        );
        assert_eq!(live.u8(0x10001).unwrap(), 0x11);
        assert_eq!(live.u8(0x10fff).unwrap(), 0xbb);
        assert_eq!(live.u8(0x11040).unwrap(), 0xcc, "owned: the view's");
        assert_eq!(live.read::<4>(0x1303e).unwrap(), [0xdd; 4]);
        // Bytes the view left alone keep the live side's values.
        assert_eq!(live.u32(0x10004).unwrap(), 1);
    }
}
