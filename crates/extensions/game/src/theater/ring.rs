//! The player's private machine keyframes. Each physical page is copied on
//! first capture and shared until the write watch reports a change.
use anyhow::{Context, Result, ensure};
use runtime::guest::{GuestLayout, PAGE_SIZE, RAM_SIZE};
use runtime::watch::CONTIGUOUS_BASE;
use runtime::{DirtyRange, Guest, WatchId};
use std::collections::VecDeque;

const PAGE: usize = PAGE_SIZE as usize;
const ENTRIES: usize = 12;
const NEW_PAGE: u32 = u32::MAX;

struct Page {
    bytes: Box<[u8; PAGE]>,
    users: usize,
}

#[derive(Default)]
struct Pool {
    slots: Vec<Option<Page>>,
    free: Vec<u32>,
}

impl Pool {
    fn bytes(&self) -> usize {
        (self.slots.len() - self.free.len()) * PAGE
    }

    fn page(&self, id: u32) -> &Page {
        self.slots[id as usize].as_ref().unwrap()
    }

    fn retain(&mut self, id: u32) {
        self.slots[id as usize].as_mut().unwrap().users += 1;
    }

    fn release(&mut self, id: u32) {
        let slot = &mut self.slots[id as usize];
        let page = slot.as_mut().unwrap();
        page.users -= 1;
        if page.users == 0 {
            *slot = None;
            self.free.push(id);
        }
    }

    fn copy(&mut self, bytes: &[u8]) -> u32 {
        let page = Page {
            bytes: Box::new(bytes.try_into().expect("one physical page")),
            users: 1,
        };
        if let Some(id) = self.free.pop() {
            self.slots[id as usize] = Some(page);
            id
        } else {
            // At most twelve tables, each with RAM_SIZE / PAGE entries.
            let id = self.slots.len() as u32;
            self.slots.push(Some(page));
            id
        }
    }
}

struct Entry<T> {
    tick: u64,
    pages: Box<[u32]>,
    data: T,
    // Includes the page table and encoded sections/cursors.
    entry_bytes: usize,
}

pub(super) struct Ring<T> {
    entries: VecDeque<Entry<T>>,
    pool: Pool,
    cap: usize,
    watch: Option<WatchId>,
    // Guest RAM agrees with this table outside the write watch's dirty runs.
    // It can be an older entry after a jump.
    live: Option<u64>,
}

fn dirty_pages(ranges: &[DirtyRange]) -> impl Iterator<Item = usize> + '_ {
    ranges.iter().flat_map(|r| {
        let first = ((r.addr - CONTIGUOUS_BASE) / PAGE_SIZE) as usize;
        first..first + (r.len / PAGE_SIZE) as usize
    })
}

impl<T> Ring<T> {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            pool: Pool::default(),
            cap,
            watch: None,
            live: None,
        }
    }

    pub(super) fn cap(&self) -> usize {
        self.cap
    }

    pub(super) fn bytes(&self) -> usize {
        self.pool.bytes() + self.entries.iter().map(|e| e.entry_bytes).sum::<usize>()
    }

    pub(super) fn ticks(&self) -> Vec<u64> {
        let mut ticks: Vec<_> = self.entries.iter().map(|entry| entry.tick).collect();
        ticks.sort_unstable();
        ticks
    }

    pub(super) fn closest(&self, target: u64) -> Option<u64> {
        self.entries
            .iter()
            .map(|e| e.tick)
            .filter(|&t| t <= target)
            .max()
    }

    fn entry(&self, tick: u64) -> Option<&Entry<T>> {
        self.entries.iter().find(|e| e.tick == tick)
    }

    pub(super) fn get(&self, tick: u64) -> Option<&T> {
        self.entry(tick).map(|entry| &entry.data)
    }

    pub(super) fn clear(&mut self, guest: &mut Guest) {
        if let Some(watch) = self.watch {
            guest.unsubscribe_writes(watch);
        }
        *self = Self::new(self.cap);
    }

    fn remove(&mut self, index: usize) {
        let gone = self.entries.remove(index).unwrap();
        for &id in &gone.pages {
            self.pool.release(id);
        }
        if self.live == Some(gone.tick) {
            self.live = None;
        }
    }

    fn entry_cost(&self, ram_bytes: usize, data_bytes: usize) -> Option<usize> {
        let table = (ram_bytes / PAGE).checked_mul(size_of::<u32>())?;
        let cost = table.checked_add(data_bytes)?;
        (cost.checked_add(ram_bytes)? <= self.cap).then_some(cost)
    }

    /// Refusing a capture too large to keep never consumes the guest's dirt.
    pub(super) fn capture(
        &mut self,
        tick: u64,
        guest: &mut Guest,
        data: T,
        data_bytes: usize,
    ) -> Result<bool> {
        let Some(cost) = self.entry_cost(RAM_SIZE, data_bytes) else {
            return Ok(false);
        };
        let watch = match self.watch {
            Some(watch) => watch,
            None => {
                let watch = guest.subscribe_writes("theater ring");
                guest.watch_writes(watch, CONTIGUOUS_BASE, RAM_SIZE as u32)?;
                self.watch = Some(watch);
                watch
            }
        };
        let dirty = guest.take_dirty(watch)?;
        self.capture_pages(tick, guest.physical_ram(), &dirty, data, cost);
        Ok(true)
    }

    fn capture_pages(&mut self, tick: u64, ram: &[u8], dirty: &[DirtyRange], data: T, cost: usize) {
        let mut pages = match self.live.and_then(|t| self.entry(t)) {
            Some(base) => {
                let mut table = base.pages.clone();
                for i in dirty_pages(dirty) {
                    let bytes = &ram[i * PAGE..(i + 1) * PAGE];
                    if bytes != &self.pool.page(table[i]).bytes[..] {
                        table[i] = NEW_PAGE;
                    }
                }
                table
            }
            None => vec![NEW_PAGE; ram.len() / PAGE].into_boxed_slice(),
        };

        // Pin the inherited pages while the old entries are evicted. Make
        // room *before* allocating a single new 4 KiB page. The entry alone
        // fits, even if every old table has to go.
        let mut needed = cost;
        for &id in &pages {
            if id == NEW_PAGE {
                needed += PAGE;
            } else {
                self.pool.retain(id);
            }
        }
        if let Some(i) = self.entries.iter().position(|entry| entry.tick == tick) {
            self.remove(i);
        }
        while self.entries.len() >= ENTRIES || self.bytes() > self.cap - needed {
            self.remove(0);
        }
        for (id, page) in pages.iter_mut().zip(ram.chunks_exact(PAGE)) {
            if *id == NEW_PAGE {
                *id = self.pool.copy(page);
            }
        }
        self.entries.push_back(Entry {
            tick,
            pages,
            data,
            entry_bytes: cost,
        });
        self.live = Some(tick);
    }

    /// Restore only pages which can differ from the target. `restore_with`
    /// validates the guest layout first and invalidates the other consumers'
    /// memory caches after the copy.
    pub(super) fn restore_ram(
        &mut self,
        tick: u64,
        guest: &mut Guest,
        layout: &GuestLayout,
    ) -> Result<()> {
        let watch = self.watch.context("ring has no write watch")?;
        ensure!(
            self.entry(tick).is_some(),
            "ring has no entry at tick {tick}"
        );
        let dirt = guest.take_dirty(watch)?;
        let base = self.live.take();
        // Once dirt has been drained a failed restore must leave no live
        // basis. A subsequent capture or restore then visits every page.
        guest.restore_with(layout, |ram| {
            self.copy_to(ram, base, tick, &dirt);
            Ok::<_, std::convert::Infallible>(())
        })??;
        // Guest::restore_with marks *all* watched pages dirty on adoption.
        // Discard just this subscriber's notification, before loading parts
        // that may legitimately write to guest RAM.
        guest.take_dirty(watch)?;
        self.live = Some(tick);
        Ok(())
    }

    fn copy_to(&self, ram: &mut [u8], base: Option<u64>, target: u64, dirty: &[DirtyRange]) {
        let target = self.entry(target).unwrap();
        let base = base.and_then(|t| self.entry(t));
        let mut dirt = dirty_pages(dirty).peekable();
        for (i, &id) in target.pages.iter().enumerate() {
            let written = dirt.next_if_eq(&i).is_some();
            if written || base.is_none_or(|e| e.pages[i] != id) {
                ram[i * PAGE..(i + 1) * PAGE].copy_from_slice(&self.pool.page(id).bytes[..]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    fn range(first: usize, count: usize) -> DirtyRange {
        DirtyRange {
            addr: CONTIGUOUS_BASE + first as u32 * PAGE_SIZE,
            len: count as u32 * PAGE_SIZE,
        }
    }

    #[test]
    fn unchanged_pages_share_storage_and_the_cap_evicts_oldest_first() {
        // Three resident physical pages, one changed page and two tables fit
        // exactly, including both entries' encoded sections.
        let mut ram = vec![0; 3 * PAGE];
        let cost = 3 * size_of::<u32>() + 8;
        let mut ring = Ring::new(4 * PAGE + 2 * cost);
        assert_eq!(ring.entry_cost(ram.len(), 8), Some(cost));
        ring.capture_pages(150, &ram, &[], "first", cost);
        ram[PAGE] = 17;
        ring.capture_pages(300, &ram, &[range(0, 2)], "second", cost);
        assert_eq!(ring.bytes(), ring.cap());
        let a = &ring.entry(150).unwrap().pages;
        let b = &ring.entry(300).unwrap().pages;
        assert_eq!(a[0], b[0]); // reported dirty, bytes equal
        assert_ne!(a[1], b[1]);
        assert_eq!(a[2], b[2]); // untouched
        ram[PAGE * 2] = 21;
        ring.capture_pages(450, &ram, &[range(2, 1)], "third", cost);
        assert_eq!(ring.bytes(), ring.cap());
        assert_eq!(ring.ticks(), [300, 450]);
        assert_eq!(ring.closest(299), None);
        assert_eq!(ring.closest(449), Some(300));
        let mut old = ram.clone();
        ring.copy_to(&mut old, Some(450), 300, &[]);
        assert_eq!(old[PAGE], 17);
        assert_eq!(old[PAGE * 2], 0);
        assert_eq!(ring.get(450), Some(&"third"));

        // Force eviction of the live basis too. Its unchanged pages must
        // survive until the new table takes ownership of them.
        let bigger = cost + PAGE + cost;
        assert_eq!(ring.entry_cost(ram.len(), bigger - 12), Some(bigger));
        ram[0] = 88;
        ring.capture_pages(600, &ram, &[range(0, 1)], "large", bigger);
        assert_eq!(ring.ticks(), [600]);
        assert_eq!(ring.bytes(), ring.cap());
        let mut restored = vec![0x55; ram.len()];
        ring.copy_to(&mut restored, None, 600, &[]);
        assert_eq!(restored, ram);
    }

    #[test]
    fn newest_twelve_captures_survive_rewatch_and_replacement() {
        let mut ring = Ring::new(2 * PAGE);
        let ram = [71; PAGE];
        for tick in (1..=14).map(|n| n * 150) {
            ring.capture_pages(tick, &ram, &[], tick, 4);
        }
        assert_eq!(ring.ticks(), (3..=14).map(|n| n * 150).collect::<Vec<_>>());
        assert_eq!(ring.bytes(), PAGE + ENTRIES * 4);
        // Rewatch a point, then watch an older stretch. Eviction is in
        // capture order, even when tick numbers go backwards.
        ring.capture_pages(450, &ram, &[], 999, 4);
        ring.capture_pages(150, &ram, &[], 150, 4);
        assert_eq!(ring.get(450), Some(&999));
        assert!(ring.get(600).is_none());
        assert_eq!(ring.ticks().len(), ENTRIES);
    }

    #[test]
    fn restore_uses_the_write_watch_and_keeps_other_consumers_dirty() {
        let mut guest = Guest::new();
        const VA: u32 = 0x10000;
        guest.map(VA, PAGE_SIZE, 0, Permissions::RW).unwrap();
        guest
            .map(VA + PAGE_SIZE, PAGE_SIZE, PAGE_SIZE, Permissions::RW)
            .unwrap();
        let layout = guest.layout();
        let observer = guest.subscribe_writes("test");
        guest
            .watch_writes(observer, CONTIGUOUS_BASE, 2 * PAGE_SIZE)
            .unwrap();
        let mut ring = Ring::new(RAM_SIZE + 3 * (RAM_SIZE / PAGE) * 4 + 2 * PAGE);
        guest.set_u32(VA, 15).unwrap();
        assert!(ring.capture(150, &mut guest, (), 0).unwrap());
        guest.set_u32(VA, 30).unwrap();
        assert!(ring.capture(300, &mut guest, (), 0).unwrap());
        guest.set_u32(VA + PAGE_SIZE, 41).unwrap();
        // Refusing a capture must leave its dirt available for restore.
        assert!(!ring.capture(450, &mut guest, (), usize::MAX).unwrap());
        guest.take_dirty(observer).unwrap();
        ring.restore_ram(150, &mut guest, &layout).unwrap();
        assert_eq!(guest.u32(VA).unwrap(), 15);
        assert_eq!(guest.u32(VA + PAGE_SIZE).unwrap(), 0);
        assert_eq!(guest.take_dirty(observer).unwrap(), [range(0, 2)]);
        guest.set_u32(VA + PAGE_SIZE, 52).unwrap();
        assert!(ring.capture(200, &mut guest, (), 0).unwrap());
        ring.restore_ram(300, &mut guest, &layout).unwrap();
        assert_eq!(guest.u32(VA).unwrap(), 30);
        assert_eq!(guest.u32(VA + PAGE_SIZE).unwrap(), 0);
        ring.restore_ram(200, &mut guest, &layout).unwrap();
        assert_eq!(guest.u32(VA).unwrap(), 15);
        assert_eq!(guest.u32(VA + PAGE_SIZE).unwrap(), 52);
        ring.clear(&mut guest);
        assert_eq!(ring.bytes(), 0);
        assert_eq!(guest.write_watch_owners(), [(observer, "test")]);
    }
}
