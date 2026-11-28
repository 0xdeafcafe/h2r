//! The physical bytes behind a virtual range, page by page. The XBE loader
//! maps every image page on its own (`runtime::loader::load_xbe_with`) and
//! the kernel maps every committed page of virtual memory on its own
//! (`kernel::memory`, `NtAllocateVirtualMemory`), so a range that crosses
//! a page can lie in several mappings with unrelated physical pages. One
//! lookup per page finds them all; contiguous pieces merge.
use super::{Memory, SegmentMemory};
use runtime::guest::Access;

const PAGE: u32 = 0x1000;

/// A virtual range's physical pieces, as half-open `[start, end)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spans {
    head: [(u64, u64); 4],
    n: u8,
    /// Pieces past the fourth: only a range over many scattered pages.
    rest: Vec<(u64, u64)>,
}

impl Spans {
    /// `[addr, addr + len)` in `m`'s physical addresses. `None` when a page
    /// of it is not readable or the range wraps the address space.
    pub fn of(m: &(impl Memory + ?Sized), addr: u32, len: usize) -> Option<Self> {
        Self::build(addr, len, |at, n| m.physical_read(at, n))
    }

    /// [`Spans::of`] for `access` (a write needs writable pages), with
    /// `addr` a multiple of `alignment`.
    pub fn checked(
        m: &impl SegmentMemory,
        addr: u32,
        len: usize,
        access: Access,
        alignment: u32,
    ) -> Option<Self> {
        if !addr.is_multiple_of(alignment.max(1)) {
            return None;
        }
        Self::build(addr, len, |at, n| m.checked_offset(at, n, access, 1))
    }

    fn build(addr: u32, len: usize, lookup: impl Fn(u32, usize) -> Option<u32>) -> Option<Self> {
        let end = u64::from(addr).checked_add(len as u64).filter(|&e| e <= 1 << 32)?;
        let mut spans = Self::default();
        let mut at = u64::from(addr);
        while at < end {
            let next = ((at | u64::from(PAGE - 1)) + 1).min(end);
            let lo = u64::from(lookup(at as u32, (next - at) as usize)?);
            spans.push(lo, lo + (next - at));
            at = next;
        }
        Some(spans)
    }

    fn push(&mut self, lo: u64, hi: u64) {
        let last = if self.rest.is_empty() {
            usize::from(self.n).checked_sub(1).map(|i| &mut self.head[i])
        } else {
            self.rest.last_mut()
        };
        if let Some(last) = last
            && last.1 == lo
        {
            last.1 = hi;
            return;
        }
        if usize::from(self.n) < self.head.len() {
            self.head[usize::from(self.n)] = (lo, hi);
            self.n += 1;
        } else {
            self.rest.push((lo, hi));
        }
    }

    /// The pieces, lowest virtual address first.
    pub fn iter(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.head[..usize::from(self.n)].iter().chain(&self.rest).copied()
    }

    /// Whether any byte of `self` is a byte of `other`.
    pub fn overlaps(&self, other: &Spans) -> bool {
        self.iter().any(|(a, b)| other.iter().any(|(c, d)| a < d && c < b))
    }

    /// Whether any byte of `self` lies in physical `[lo, hi)`.
    pub fn overlaps_range(&self, lo: u64, hi: u64) -> bool {
        self.iter().any(|(a, b)| a < hi && lo < b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};

    #[test]
    fn a_range_across_page_mappings_has_each_page_and_merges_neighbours() {
        let mut g = Guest::new();
        // Two virtual pages on unrelated physical pages, a third right
        // after the second.
        g.map(0x40000, PAGE, 0x20000, Permissions::RX).unwrap();
        g.map(0x41000, PAGE, 0x10000, Permissions::RX).unwrap();
        g.map(0x42000, PAGE, 0x11000, Permissions::RX).unwrap();
        let s = Spans::of(&g, 0x40ff0, 0x1020).unwrap();
        assert_eq!(s.iter().collect::<Vec<_>>(), [(0x20ff0, 0x21000), (0x10000, 0x11010)]);
        assert!(s.overlaps_range(0x10fff, 0x11001));
        assert!(!s.overlaps_range(0x21000, 0x30000));
        let other = Spans::of(&g, 0x42000, 4).unwrap();
        assert!(s.overlaps(&other));
        assert!(Spans::of(&g, 0x42ff0, 0x20).is_none(), "0x43000 is unmapped");
        assert_eq!(Spans::of(&g, 0x40000, 0).unwrap().iter().count(), 0);
    }

    #[test]
    fn many_scattered_pages_spill_past_four_pieces() {
        let mut g = Guest::new();
        for i in 0..6 {
            g.map(0x40000 + i * PAGE, PAGE, 0x10000 + 2 * i * PAGE, Permissions::READ).unwrap();
        }
        let s = Spans::of(&g, 0x40000, 6 * PAGE as usize).unwrap();
        assert_eq!(s.iter().count(), 6);
        assert_eq!(s.iter().last(), Some((0x1a000, 0x1b000)));
    }
}
