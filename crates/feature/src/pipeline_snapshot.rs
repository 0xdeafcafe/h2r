//! Admission checks for the render worker's COW view. The census supplies
//! observed page addresses. The view still snapshots all mapped RAM so a
//! newly reached page has the frame's bytes even before it has been seen.

use std::collections::BTreeMap;

use runtime::Guest;
use runtime::guest::{Mapping, PAGE_SIZE};

use crate::render_census::ScopeSet;

const MAX_HITS: usize = 65_536;

pub(super) fn arm(guest: &mut Guest) {
    guest.install_trap(MAX_HITS);
    guest.arm_all(true, true);
}

#[derive(Default)]
pub(super) struct Footprint {
    layouts: BTreeMap<u32, Vec<Mapping>>,
    dropped: u64,
}

impl Footprint {
    pub(super) fn take(guest: &mut Guest) -> Self {
        let log = guest.take_trap_hits();
        guest.disarm_all();
        // The worker has its own Guest. Count accesses from every phase,
        // including native render observers and the device's HLE.
        let scope = ScopeSet::from_log(&log, None, false);
        let layouts = scope
            .reads
            .keys()
            .chain(scope.writes.keys())
            .map(|&page| page * PAGE_SIZE)
            .map(|addr| (addr, layout(guest, addr).collect()))
            .collect();
        Self {
            layouts,
            dropped: log.dropped,
        }
    }

    fn changed_page(&self, live: &Guest) -> Option<u32> {
        self.layouts.iter().find_map(|(&addr, expected)| {
            layout(live, addr).ne(expected.iter().cloned()).then_some(addr)
        })
    }
}

// Compare only the part of each mapping on this page. A split in an
// adjacent page leaves the captured translation here intact.
fn layout(guest: &Guest, addr: u32) -> impl Iterator<Item = Mapping> + '_ {
    let end = addr as u64 + PAGE_SIZE as u64;
    let mappings = guest.mappings();
    let first = mappings.partition_point(|m| m.virtual_start as u64 + m.len as u64 <= addr as u64);
    mappings[first..]
        .iter()
        .take_while(move |m| (m.virtual_start as u64) < end)
        .map(move |m| {
            let start = addr.max(m.virtual_start);
            let end = end.min(m.virtual_start as u64 + m.len as u64);
            Mapping {
                virtual_start: start,
                physical_start: m.physical_start + (start - m.virtual_start),
                len: (end - start as u64) as u32,
                permissions: m.permissions,
            }
        })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Gap {
    Mapping(u32),
    Trace,
}

#[derive(Default)]
pub(super) struct Coverage {
    previous: Option<Footprint>,
    current: Option<Footprint>,
}

impl Coverage {
    pub(super) fn begin(&mut self) {
        self.current = None;
    }

    pub(super) fn add(&mut self, segment: Footprint) {
        let current = self.current.get_or_insert_with(Footprint::default);
        current.layouts.extend(segment.layouts);
        current.dropped += segment.dropped;
    }

    pub(super) fn complete(&mut self) -> (usize, Option<u32>) {
        let current = self.current.take();
        let mut count = 0;
        let mut first = None;
        if let (Some(previous), Some(current)) = (&self.previous, &current) {
            for &addr in current.layouts.keys() {
                if !previous.layouts.contains_key(&addr) {
                    count += 1;
                    first.get_or_insert(addr);
                }
            }
        }
        self.previous = current;
        (count, first)
    }

    pub(super) fn check(&self, live: &Guest) -> Result<(), Gap> {
        let Some(previous) = &self.previous else {
            return Ok(());
        };
        if let Some(page) = previous.changed_page(live) {
            return Err(Gap::Mapping(page));
        }
        if previous.dropped != 0 {
            return Err(Gap::Trace);
        }
        Ok(())
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn census_pages_are_frozen_through_aliases_and_across_page_boundaries() {
        let mut live = Guest::new();
        live.map(0x1000, 0x2000, 0, Permissions::RW).unwrap();
        live.map(0x8000_0000, 0x2000, 0, Permissions::RW).unwrap();
        live.set_u64(0x1ffc, 42).unwrap();
        let (mut view, _) = live.cow_snapshot();
        let mut coverage = Coverage::default();
        arm(&mut view);
        live.set_u64(0x8000_0ffc, 43).unwrap();
        assert_eq!(view.u64(0x1ffc).unwrap(), 42);
        assert_eq!(view.u64(0x8000_0ffc).unwrap(), 42);
        let footprint = Footprint::take(&mut view);
        assert_eq!(
            footprint.layouts.keys().copied().collect::<Vec<_>>(),
            [0x1000, 0x2000, 0x8000_0000, 0x8000_1000]
        );
        coverage.add(footprint);
        coverage.complete();
        assert_eq!(coverage.check(&live), Ok(()));
        live.protect_range(0x2000, 0x1000, Permissions::READ).unwrap();
        assert_eq!(coverage.check(&live), Err(Gap::Mapping(0x2000)));
        live.protect_range(0x2000, 0x1000, Permissions::RW).unwrap();
        live.unmap(0x8000_1000, 0x1000).unwrap();
        assert_eq!(coverage.check(&live), Err(Gap::Mapping(0x8000_1000)));
        live.map(0x8000_1000, 0x1000, 0x2000, Permissions::RW)
            .unwrap();
        assert_eq!(coverage.check(&live), Err(Gap::Mapping(0x8000_1000)));
    }

    #[test]
    fn a_truncated_trace_cannot_clear_the_guard() {
        let live = Guest::new();
        let mut coverage = Coverage::default();
        coverage.add(Footprint {
            dropped: 1,
            ..Footprint::default()
        });
        coverage.complete();
        assert_eq!(coverage.check(&live), Err(Gap::Trace));
        coverage.reset();
        assert_eq!(coverage.check(&live), Ok(()));
    }

    #[test]
    fn a_new_render_page_is_available_from_the_full_snapshot() {
        let mut live = Guest::new();
        live.map_zeroed(0x1000, 0x2000, Permissions::RW).unwrap();
        live.set_u32(0x2000, 7).unwrap();
        let mut coverage = Coverage::default();
        let (mut view, _) = live.cow_snapshot();
        arm(&mut view);
        view.u32(0x1000).unwrap();
        coverage.add(Footprint::take(&mut view));
        coverage.complete();
        let (mut view, _) = live.cow_snapshot();
        arm(&mut view);
        live.set_u32(0x2000, 8).unwrap();
        assert_eq!(view.u32(0x2000).unwrap(), 7);
        coverage.add(Footprint::take(&mut view));
        assert_eq!(coverage.complete(), (1, Some(0x2000)));
    }
}
