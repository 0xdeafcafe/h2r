//! A software undo journal for an alias executor
//! (docs/runtime/second-executor.md, "The alias and its journal").
//!
//! While a journal is on, every fast page table entry that allows writes
//! has its write bit cleared, the way `crate::trap` arms a page, so the
//! first store to a page, from generated C or from Rust, takes the
//! checked slow path ([`Guest::write_bytes_from`]). There the journal
//! saves the physical page's bytes before the store changes them and
//! gives the entry its write bit back; later stores to that page are
//! inline. [`Guest::journal_undo`] copies every saved page back and
//! clears the bits again, so the next round journals the same pages
//! afresh. Pages are keyed by physical page: two virtual aliases of one
//! page save it once.
//!
//! The journal sees stores. Mapping changes (`map_zeroed` zeroing fresh
//! storage) are not journaled; an alias executor makes none, since its
//! kernel calls are refused.

use super::{FastPage, Guest, PAGE_SIZE, RAM_SIZE};

const PAGE: usize = PAGE_SIZE as usize;
const PHYSICAL_PAGES: usize = RAM_SIZE / PAGE;
const VIRTUAL_PAGES: usize = 1 << 20;
/// A fast page table entry's write bit.
const WRITE: u32 = 2;

fn bit(set: &[u64], i: usize) -> bool {
    set[i / 64] & (1 << (i % 64)) != 0
}
fn put(set: &mut [u64], i: usize, on: bool) {
    if on {
        set[i / 64] |= 1 << (i % 64);
    } else {
        set[i / 64] &= !(1 << (i % 64));
    }
}

pub(crate) struct Journal {
    /// Physical pages saved this round.
    saved: Box<[u64]>,
    /// Their page numbers, in the order they were saved.
    pages: Vec<u32>,
    /// Their bytes before the round's first store, `PAGE` each, in
    /// `pages`' order. Capacity is kept across rounds.
    bytes: Vec<u8>,
    /// Virtual pages whose fast entry would allow writes: the journal
    /// cleared their write bit.
    armed: Box<[u64]>,
    /// Armed virtual pages whose write bit the journal gave back this
    /// round (their physical page is saved).
    opened: Vec<u32>,
}

impl Journal {
    fn new() -> Self {
        Self {
            saved: vec![0; PHYSICAL_PAGES / 64].into_boxed_slice(),
            pages: Vec::new(),
            bytes: Vec::new(),
            armed: vec![0; VIRTUAL_PAGES / 64].into_boxed_slice(),
            opened: Vec::new(),
        }
    }

    /// Save every physical page `[start, start + len)` touches that this
    /// round has not saved, from `ram` as it is now (before the store).
    fn save(&mut self, ram: &[u8], start: usize, len: usize) {
        if len == 0 {
            return;
        }
        let first = start / PAGE;
        let last = ((start + len - 1) / PAGE).min(PHYSICAL_PAGES - 1);
        for page in first..=last {
            if bit(&self.saved, page) {
                continue;
            }
            put(&mut self.saved, page, true);
            self.pages.push(page as u32);
            self.bytes
                .extend_from_slice(&ram[page * PAGE..page * PAGE + PAGE]);
        }
    }

    /// Clear the write bit of every entry in `[first, last)` that has one,
    /// and remember which they were. Entries without one are not armed.
    fn arm(&mut self, fast: &mut [FastPage], first: usize, last: usize) {
        for (page, entry) in fast.iter_mut().enumerate().take(last).skip(first) {
            let writable = entry.permissions & WRITE != 0;
            put(&mut self.armed, page, writable);
            entry.permissions &= !WRITE;
        }
    }
}

impl Guest {
    /// Start journaling stores (module docs), if not already: every fast
    /// page table entry that allows writes loses its write bit. A no-op
    /// while a journal is on; its armed entries stay armed across rounds.
    pub fn begin_journal(&mut self) {
        if self.journal.is_some() {
            return;
        }
        self.journal = Some(Box::new(Journal::new()));
        self.arm_journal(0, VIRTUAL_PAGES);
    }

    /// Whether a journal is on.
    pub fn journaling(&self) -> bool {
        self.journal.is_some()
    }

    /// Undo this round: put back every saved page and clear its write bit
    /// again, so the next store to it journals it afresh. The bytes go
    /// back without telling the write watch. Returns the pages restored.
    pub fn journal_undo(&mut self) -> usize {
        if self.journal.is_none() {
            return 0;
        }
        self.invalidate_hle_capture();
        let Some(j) = self.journal.as_deref_mut() else {
            return 0;
        };
        let restored = j.pages.len();
        for (i, &page) in j.pages.iter().enumerate() {
            let at = page as usize * PAGE;
            self.ram[at..at + PAGE].copy_from_slice(&j.bytes[i * PAGE..i * PAGE + PAGE]);
            put(&mut j.saved, page as usize, false);
        }
        j.pages.clear();
        j.bytes.clear();
        if let Some(fast) = self.fast_pages.as_deref_mut() {
            for &page in &j.opened {
                if bit(&j.armed, page as usize) {
                    fast[page as usize].permissions &= !WRITE;
                }
            }
        }
        j.opened.clear();
        self.watch_revision = self.watch_revision.wrapping_add(1);
        restored
    }

    /// Stop journaling: undo the round ([`journal_undo`](Self::journal_undo))
    /// and give every armed entry its write bit back.
    pub fn end_journal(&mut self) {
        self.journal_undo();
        let Some(j) = self.journal.take() else {
            return;
        };
        if let Some(fast) = self.fast_pages.as_deref_mut() {
            for (word, &bits) in j.armed.iter().enumerate() {
                let mut bits = bits;
                while bits != 0 {
                    let page = word * 64 + bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    fast[page].permissions |= WRITE;
                }
            }
        }
    }

    /// The physical pages saved this round (page-aligned physical
    /// addresses), in the order they were saved.
    pub fn journaled_pages(&self) -> Vec<u32> {
        self.journal
            .as_ref()
            .map(|j| j.pages.iter().map(|&p| p * PAGE_SIZE).collect())
            .unwrap_or_default()
    }

    /// Arm virtual pages `[first, last)` of the fast page table, if a
    /// journal is on: called after every rebuild of those entries
    /// ([`Guest::refresh_fast_range`]), so a remap or protect, or a table
    /// built in the middle of a round, stays armed. A rebuilt entry whose
    /// page is already saved is opened again by its next store, which
    /// saves nothing.
    pub(super) fn arm_journal(&mut self, first: usize, last: usize) {
        let (Some(j), Some(fast)) = (self.journal.as_deref_mut(), self.fast_pages.as_deref_mut())
        else {
            return;
        };
        j.arm(fast, first, last.min(VIRTUAL_PAGES));
    }

    /// Before a slow-path store's bytes land at physical `[start, start +
    /// len)`: save the pages it touches.
    #[inline]
    pub(super) fn journal_save(&mut self, start: usize, len: usize) {
        if let Some(j) = self.journal.as_deref_mut() {
            j.save(&self.ram, start, len);
        }
    }

    /// After a slow-path store to virtual `[addr, addr + len)`: give each
    /// armed page whose physical page is now saved its write bit back.
    pub(super) fn journal_open(&mut self, addr: u32, len: usize) {
        let (Some(j), Some(fast)) = (self.journal.as_deref_mut(), self.fast_pages.as_deref_mut())
        else {
            return;
        };
        if len == 0 {
            return;
        }
        let first = (addr >> 12) as usize;
        let last = ((addr as u64 + len as u64 - 1) >> 12) as usize;
        for page in first..=last.min(VIRTUAL_PAGES - 1) {
            let entry = &mut fast[page];
            let physical = entry.physical as usize / PAGE;
            if bit(&j.armed, page)
                && entry.permissions & WRITE == 0
                && physical < PHYSICAL_PAGES
                && bit(&j.saved, physical)
            {
                entry.permissions |= WRITE;
                j.opened.push(page as u32);
            }
        }
    }
}
