//! Which guest pages are slow, and why: the pages outside the flat RAM
//! window by reason and owner, and one page as the inline paths see it
//! (the agent's `profile.memory.pages`, docs/runtime/agent-api-render.md,
//! "Flat RAM window"). The reasons follow the window's own rule
//! ([`crate::window`]): they are computed from the same slots.

use super::{Guest, Mapping, PAGE_SIZE};
use crate::window::Slot;

/// Why a mapped guest page is outside the flat RAM window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutsideReason {
    /// A mapping covers only part of the page.
    ByteGranular,
    /// Its physical address is not page-aligned.
    Unaligned,
    /// Its physical address is not congruent with its virtual one modulo
    /// the host page.
    NonCongruent,
    /// Whole and congruent itself, but its host page also holds a page
    /// that is not, or one backed by another physical host page.
    SharesHostPage,
    /// The access trap ([`crate::trap`]) has it armed.
    Trapped,
}

impl OutsideReason {
    pub fn name(self) -> &'static str {
        match self {
            Self::ByteGranular => "byte_granular",
            Self::Unaligned => "unaligned",
            Self::NonCongruent => "non_congruent",
            Self::SharesHostPage => "shares_host_page",
            Self::Trapped => "trapped",
        }
    }
}

/// Consecutive outside pages with one reason in one mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideRun {
    pub va: u32,
    pub pages: u32,
    pub reason: OutsideReason,
    /// The mapping that holds the run's first page.
    pub mapping: Mapping,
    /// The HLE virtual claim over the run, else the physical reservation
    /// under it.
    pub owner: Option<&'static str>,
}

/// One guest page as the inline paths see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageView {
    pub va: u32,
    /// The mapping that holds the page, or part of it.
    pub mapping: Option<Mapping>,
    /// The fast page table entry, `(physical, permissions)`: what the
    /// paged inline path checks (bit 0 read, bit 1 write). None without
    /// a table.
    pub fast: Option<(u32, u32)>,
    /// The access trap has it armed `(read, write)`.
    pub trapped: (bool, bool),
    /// Why it is outside the window, or None when it is in it or unmapped.
    pub outside: Option<OutsideReason>,
    pub owner: Option<&'static str>,
}

impl Guest {
    /// The host page the window is (or would be) built on.
    fn window_host(&self) -> u64 {
        self.owned_window()
            .map_or_else(crate::window::host_page, |w| w.host_page()) as u64
    }

    /// Every mapped page outside the flat RAM window, in runs, lowest
    /// address first. Its page count is the window's `pages_outside`.
    pub fn outside_runs(&self) -> Vec<OutsideRun> {
        self.outside_runs_on(self.window_host())
    }

    pub(crate) fn outside_runs_on(&self, host: u64) -> Vec<OutsideRun> {
        let k = (host / PAGE_SIZE as u64) as usize;
        let mut groups: Vec<usize> = self
            .mappings
            .iter()
            .flat_map(|m| {
                let first = m.virtual_start as usize >> 12;
                let last = (m.virtual_start as u64 + m.len as u64).div_ceil(PAGE_SIZE as u64) as usize;
                (first / k)..last.div_ceil(k)
            })
            .collect();
        groups.sort_unstable();
        groups.dedup();
        let mut runs: Vec<OutsideRun> = Vec::new();
        for g in groups {
            for (i, reason) in self.group_reasons(g * k, k, host).into_iter().enumerate() {
                let Some(reason) = reason else { continue };
                let va = ((g * k + i) as u32) << 12;
                let Some(mapping) = self.page_mapping(va) else { continue };
                if let Some(r) = runs.last_mut()
                    && r.reason == reason
                    && r.mapping == mapping
                    && r.va as u64 + ((r.pages as u64) << 12) == va as u64
                {
                    r.pages += 1;
                    continue;
                }
                runs.push(OutsideRun {
                    va,
                    pages: 1,
                    reason,
                    owner: self.page_owner(va, &mapping),
                    mapping,
                });
            }
        }
        runs
    }

    /// Guest address `va`'s page as the inline paths see it.
    pub fn page_view(&self, va: u32) -> PageView {
        self.page_view_on(va, self.window_host())
    }

    pub(crate) fn page_view_on(&self, va: u32, host: u64) -> PageView {
        let va = va & !(PAGE_SIZE - 1);
        let vpn = (va >> 12) as usize;
        let k = (host / PAGE_SIZE as u64) as usize;
        let first = vpn / k * k;
        let mapping = self.page_mapping(va);
        PageView {
            va,
            fast: self
                .fast_pages
                .as_ref()
                .map(|p| (p[vpn].physical, p[vpn].permissions)),
            trapped: self
                .trap
                .as_ref()
                .map_or((false, false), |t| (t.armed(vpn, false), t.armed(vpn, true))),
            outside: self.group_reasons(first, k, host)[vpn - first],
            owner: mapping.as_ref().and_then(|m| self.page_owner(va, m)),
            mapping,
        }
    }

    /// Why each page of the host page `[first, first + k)` is outside the
    /// window (None: in it, or unmapped), by the rule `Window::refresh`
    /// applies to the same slots.
    fn group_reasons(&self, first: usize, k: usize, host: u64) -> Vec<Option<OutsideReason>> {
        let slots = self.window_slots(first, first + k, host);
        let mut backing = None;
        let mut whole = true;
        for (i, slot) in slots.iter().enumerate() {
            match *slot {
                Slot::Unmapped => {}
                Slot::Out => whole = false,
                Slot::In { physical, .. } => {
                    let host_page = physical as usize - i * PAGE_SIZE as usize;
                    match backing {
                        None => backing = Some(host_page),
                        Some(b) if b != host_page => whole = false,
                        Some(_) => {}
                    }
                }
            }
        }
        slots
            .iter()
            .enumerate()
            .map(|(i, slot)| match slot {
                Slot::Unmapped => None,
                Slot::In { .. } if whole => None,
                Slot::In { .. } => Some(OutsideReason::SharesHostPage),
                Slot::Out => Some(self.out_reason(first + i, host)),
            })
            .collect()
    }

    /// Why page `vpn`, a [`Slot::Out`], is one.
    fn out_reason(&self, vpn: usize, host: u64) -> OutsideReason {
        let va = (vpn as u64) << 12;
        let whole = self.mappings.iter().find(|m| {
            m.virtual_start as u64 <= va && va + PAGE_SIZE as u64 <= m.virtual_start as u64 + m.len as u64
        });
        match whole {
            None => OutsideReason::ByteGranular,
            Some(m) => {
                let physical = m.physical_start as u64 + va - m.virtual_start as u64;
                if !physical.is_multiple_of(PAGE_SIZE as u64) {
                    OutsideReason::Unaligned
                } else if physical % host != va % host {
                    OutsideReason::NonCongruent
                } else {
                    OutsideReason::Trapped
                }
            }
        }
    }

    /// The lowest mapping over any byte of `va`'s page.
    fn page_mapping(&self, va: u32) -> Option<Mapping> {
        let (lo, hi) = (va as u64, va as u64 + PAGE_SIZE as u64);
        self.mappings
            .iter()
            .find(|m| (m.virtual_start as u64) < hi && lo < m.virtual_start as u64 + m.len as u64)
            .cloned()
    }

    /// The claim over `va`, else the reservation under its physical page.
    fn page_owner(&self, va: u32, m: &Mapping) -> Option<&'static str> {
        let va64 = va as u64;
        if let Some(c) = self
            .claims
            .iter()
            .find(|c| c.virtual_start as u64 <= va64 && va64 < c.end())
        {
            return Some(c.owner);
        }
        let physical = m.physical_start as u64 + va64.saturating_sub(m.virtual_start as u64);
        self.reservations
            .iter()
            .find(|r| r.physical_start as u64 <= physical && physical < r.end())
            .map(|r| r.owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest::Permissions;

    const HOST: u64 = 16 * 1024;

    fn reasons(g: &Guest) -> Vec<(u32, u32, &'static str)> {
        g.outside_runs_on(HOST)
            .iter()
            .map(|r| (r.va, r.pages, r.reason.name()))
            .collect()
    }

    #[test]
    fn names_each_outside_page_by_the_window_rule() {
        let mut g = Guest::new();
        // Congruent and alone in its host page: inside.
        g.map(0x0001_0000, 0x4000, 0x0010_0000, Permissions::RW).unwrap();
        // Physical residue 0x1000 under a residue-0 page.
        g.map(0x0002_0000, 0x1000, 0x0020_1000, Permissions::RW).unwrap();
        // Two congruent pages of one host page on different physical host
        // pages.
        g.map(0x0003_0000, 0x1000, 0x0030_0000, Permissions::RW).unwrap();
        g.map(0x0003_1000, 0x1000, 0x0040_1000, Permissions::RW).unwrap();
        // A piece of a page.
        g.map(0x0005_0000, 0x800, 0x0050_0000, Permissions::RW).unwrap();
        // A page-sized mapping on an unaligned physical address.
        g.map(0x0006_0000, 0x1000, 0x0060_0800, Permissions::RW).unwrap();
        assert_eq!(
            reasons(&g),
            [
                (0x0002_0000, 1, "non_congruent"),
                (0x0003_0000, 1, "shares_host_page"),
                (0x0003_1000, 1, "shares_host_page"),
                (0x0005_0000, 1, "byte_granular"),
                (0x0006_0000, 1, "unaligned"),
            ]
        );
        assert_eq!(g.page_view_on(0x0001_0123, HOST).outside, None);
        let v = g.page_view_on(0x0003_1000, HOST);
        assert_eq!(v.outside, Some(OutsideReason::SharesHostPage));
        assert_eq!(v.mapping.map(|m| m.physical_start), Some(0x0040_1000));
        assert_eq!(g.page_view_on(0x0009_0000, HOST).mapping, None);
    }

    #[test]
    fn a_run_carries_its_mapping_and_owner() {
        let mut g = Guest::new();
        g.map(0x0002_0000, 0x3000, 0x0020_1000, Permissions::RW).unwrap();
        g.claim_virtual(0x0002_0000, 0x3000, "test claim").unwrap();
        let runs = g.outside_runs_on(HOST);
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].va, runs[0].pages), (0x0002_0000, 3));
        assert_eq!(runs[0].mapping.physical_start, 0x0020_1000);
        assert_eq!(runs[0].owner, Some("test claim"));
    }

    #[test]
    fn a_page_mapped_beside_a_neighbour_shares_its_physical_host_page() {
        let mut g = Guest::new();
        let physical = |g: &Guest, va: u32| {
            g.mappings().iter().find(|m| m.virtual_start == va).unwrap().physical_start
        };
        g.map_zeroed(0x0001_0000, 0x1000, Permissions::RW).unwrap();
        // Something else takes the never-used RAM after it.
        g.map_zeroed(0x0090_0000, 0x1000, Permissions::RW).unwrap();
        g.set_u32(0x0090_0000, 0x5a5a_5a5a).unwrap();
        g.map_zeroed(0x0001_1000, 0x1000, Permissions::RW).unwrap();
        assert_eq!(physical(&g, 0x0001_1000), physical(&g, 0x0001_0000) + 0x1000);
        assert_eq!(g.u32(0x0001_1000).unwrap(), 0);
        // A fresh lone page leaves room in its host page for its
        // neighbours.
        let lone = g.find_free_physical_for(0x0002_3000, 0x1000, 0..super::super::RAM_SIZE as u32);
        let lone = lone.unwrap();
        assert_eq!(lone % 0x4000, 0x3000);
        assert!(g.physical_free(lone - 0x3000, 0x4000));
        g.map(0x0002_3000, 0x1000, lone, Permissions::RW).unwrap();
        assert_eq!(g.physical_beside(0x0002_0000, 0x3000), Some(lone - 0x3000));
        g.map_zeroed(0x0002_0000, 0x3000, Permissions::RW).unwrap();
        assert!(reasons(&g).is_empty(), "{:?}", reasons(&g));
    }

    #[test]
    fn a_trapped_page_is_named_trapped() {
        let mut g = Guest::new();
        g.map(0x0001_0000, 0x4000, 0x0010_0000, Permissions::RW).unwrap();
        g.install_trap(4);
        g.set_sticky_pages(&[0x10]);
        assert_eq!(reasons(&g), [(0x0001_0000, 1, "trapped"), (0x0001_1000, 3, "shares_host_page")]);
    }
}
