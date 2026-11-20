//! Bounded writeback for host-owned register and shader cache pages. During
//! the borrow all aliases see the latest bytes. Every logical write notifies
//! the normal physical write watch. The handoff only publishes the bytes.

use super::{Access, Guest, GuestFault, PAGE_SIZE};
use crate::{DirtyRange, WatchError, WatchId};

const LINE: usize = 64;
const PAGE: usize = PAGE_SIZE as usize;

struct Page {
    physical: usize,
    values: Box<[u8; PAGE]>,
    // One bit per written byte. No untouched RAM is copied on entry or exit.
    bytes: [u64; PAGE / LINE],
    lines: u64,
}

struct Alias {
    start: u32,
    end: u64,
    offset: usize,
    page: usize,
    permissions: super::Permissions,
}

#[derive(Default)]
pub struct Writeback {
    layout: u64,
    addresses: &'static [u32],
    aliases: Vec<Alias>,
    filter: [u64; 4],
    pages: Vec<Page>,
    dirty: Vec<usize>,
}

/// The sole access to RAM while host code operates on buffered pages. Its
/// destructor runs on normal return and unwind, before the next guest call.
pub struct WritebackView<'a> {
    guest: &'a mut Guest,
    cache: &'a mut Writeback,
}

impl Writeback {
    /// Select just the pages containing the requested register addresses.
    /// Page mappings and all their aliases are reused across polls and
    /// refreshed after every layout change, including a different executor.
    pub fn with<T>(
        &mut self, guest: &mut Guest, addresses: &'static [u32],
        run: impl FnOnce(&mut WritebackView<'_>) -> T,
    ) -> T {
        if self.layout != guest.layout_id() || !std::ptr::eq(self.addresses, addresses) {
            self.layout = guest.layout_id();
            self.addresses = addresses;
            self.aliases.clear();
            self.filter.fill(0);
            self.pages.clear();
            for &at in addresses {
                let Some(mapping) = guest.mapping_index(u64::from(at)).map(|i| &guest.mappings[i]) else { continue };
                let physical = (mapping.physical_start + (at - mapping.virtual_start)) & !(PAGE_SIZE - 1);
                if self.pages.iter().any(|page| page.physical == physical as usize) { continue; }
                let aliases = guest.physical_aliases(physical, PAGE_SIZE);
                let page = self.pages.len();
                self.pages.push(Page { physical: physical as usize, values: Box::new([0; PAGE]),
                    bytes: [0; PAGE / LINE], lines: 0 });
                for m in aliases {
                    let end = u64::from(m.virtual_start) + u64::from(m.len);
                    let first = m.virtual_start / PAGE_SIZE;
                    let last = ((end - 1) as u32) / PAGE_SIZE;
                    for n in first..=last {
                        self.filter[(n as usize >> 6) & 3] |= 1 << (n % 64);
                    }
                    self.aliases.push(Alias { start: m.virtual_start, end, page,
                        offset: (m.physical_start - physical) as usize, permissions: m.permissions });
                }
            }
            self.aliases.sort_unstable_by_key(|alias| alias.start);
        }
        run(&mut WritebackView { guest, cache: self })
    }

    // Include the following gap in the result: an unaligned mapping may
    // begin partway through the same virtual page as an ordinary RAM write.
    #[inline]
    fn segment(&self, addr: u32, len: usize) -> (Option<&Alias>, usize) {
        let n = addr / PAGE_SIZE;
        let gap = len.min(PAGE - addr as usize % PAGE);
        if self.filter[(n as usize >> 6) & 3] & (1 << (n % 64)) == 0 { return (None, gap); }
        let index = self.aliases.partition_point(|alias| alias.end <= u64::from(addr));
        match self.aliases.get(index) {
            Some(a) if a.start <= addr => (Some(a), len.min((a.end - u64::from(addr)) as usize)),
            Some(a) => (None, len.min((a.start - addr) as usize)),
            None => (None, gap),
        }
    }
}

fn mask(offset: usize, len: usize) -> u64 {
    (u64::MAX >> (64 - len)) << (offset % LINE)
}

// Pass runs of set bits, coalescing adjacent bytes in a line.
fn runs(mut bits: u64, mut copy: impl FnMut(usize, usize)) {
    while bits != 0 {
        let first = bits.trailing_zeros() as usize;
        let count = (bits >> first).trailing_ones() as usize;
        copy(first, count);
        bits &= !mask(first, count);
    }
}

impl WritebackView<'_> {
    pub fn read(&self, addr: u32, data: &mut [u8]) -> Result<(), GuestFault> {
        if self.cache.dirty.is_empty() || data.is_empty() {
            return self.guest.read_bytes(addr, data, 1);
        }
        let (alias, count) = self.cache.segment(addr, data.len());
        if count == data.len() {
            let Some(alias) = alias else { return self.guest.read_bytes(addr, data, 1); };
            let page = &self.cache.pages[alias.page];
            if page.lines == 0 { return self.guest.read_bytes(addr, data, 1); }
            let off = alias.offset + (addr - alias.start) as usize;
            if alias.permissions.allows(Access::Read) && data.len() <= LINE - off % LINE {
                let wanted = mask(off, data.len());
                if page.bytes[off / LINE] & wanted == wanted {
                    data.copy_from_slice(&page.values[off..off + data.len()]);
                    return Ok(());
                }
            }
        }
        self.guest.read_bytes(addr, data, 1)?;
        let mut cursor = 0;
        while cursor < data.len() {
            let at = addr + cursor as u32;
            let (alias, count) = self.cache.segment(at, data.len() - cursor);
            if let Some(alias) = alias {
                let offset = alias.offset + (at - alias.start) as usize;
                let page = &self.cache.pages[alias.page];
                let mut pos = offset;
                while pos < offset + count {
                    let n = (LINE - pos % LINE).min(offset + count - pos);
                    let line = pos / LINE;
                    let written = page.bytes[line] & mask(pos, n);
                    runs(written, |first, len| {
                        let src = line * LINE + first;
                        let dst = cursor + (src - offset);
                        data[dst..dst + len].copy_from_slice(&page.values[src..src + len]);
                    });
                    pos += n;
                }
            }
            cursor += count;
        }
        Ok(())
    }

    fn store(&mut self, i: usize, offset: usize, data: &[u8]) {
        let page = &mut self.cache.pages[i];
        if page.lines == 0 { self.cache.dirty.push(i); }
        let end = offset + data.len();
        page.values[offset..end].copy_from_slice(data);
        let mut pos = offset;
        while pos < end {
            let n = (LINE - pos % LINE).min(end - pos);
            page.bytes[pos / LINE] |= mask(pos, n);
            page.lines |= 1 << (pos / LINE);
            pos += n;
        }
        if self.guest.watch.active() { self.guest.watch.note(page.physical + offset, data.len()); }
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), GuestFault> {
        // A taped HLE call needs the ordered writes at their original virtual
        // addresses, including two stores to different aliases of one register.
        if self.guest.hle_writes.is_some() || self.guest.has_trap() || data.is_empty() {
            return self.guest.write_bytes(addr, data, 1);
        }
        let (alias, count) = self.cache.segment(addr, data.len());
        if count == data.len() {
            if let Some(a) = alias && a.permissions.allows(Access::Write) {
                let offset = a.offset + (addr - a.start) as usize;
                self.store(a.page, offset, data);
                return Ok(());
            }
            return self.guest.write_bytes(addr, data, 1);
        }
        // All or nothing even across an unmapped or protected page.
        self.guest.check(addr, data.len(), Access::Write, 1)?;
        let mut cursor = 0;
        while cursor < data.len() {
            let at = addr + cursor as u32;
            let (alias, count) = self.cache.segment(at, data.len() - cursor);
            let words = &data[cursor..cursor + count];
            if let Some(a) = alias {
                let offset = a.offset + (at - a.start) as usize;
                self.store(a.page, offset, words);
            } else {
                self.guest.write_bytes(at, words, 1)?;
            }
            cursor += count;
        }
        Ok(())
    }

    /// Dirty watch timing follows logical store order, including a write
    /// observed and drained before the final value is published into RAM.
    pub fn take_dirty(&mut self, id: WatchId) -> Result<Vec<DirtyRange>, WatchError> {
        self.guest.take_dirty(id)
    }

    fn handoff(&mut self) {
        for i in self.cache.dirty.drain(..) {
            let page = &mut self.cache.pages[i];
            let mut lines = std::mem::take(&mut page.lines);
            while lines != 0 {
                let line = lines.trailing_zeros() as usize;
                lines &= lines - 1;
                let bits = std::mem::take(&mut page.bytes[line]);
                runs(bits, |first, count| {
                    let start = line * LINE + first;
                    let physical = page.physical + start;
                    self.guest.ram[physical..physical + count]
                        .copy_from_slice(&page.values[start..start + count]);
                });
            }
        }
    }
}

impl Drop for WritebackView<'_> {
    fn drop(&mut self) {
        self.handoff();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest::Permissions;
    use crate::watch::CONTIGUOUS_BASE;

    const CACHE: u32 = 0x12000;
    const ALIAS: u32 = 0x34000;
    const OTHER: u32 = CACHE + PAGE_SIZE;
    const PHYSICAL: u32 = 0x3000;
    const PAGES: &[u32] = &[CACHE];

    fn scene() -> (Guest, WatchId) {
        let mut g = Guest::new();
        g.map(CACHE, PAGE_SIZE, PHYSICAL, Permissions::RW).unwrap();
        g.map(ALIAS, PAGE_SIZE, PHYSICAL, Permissions::RW).unwrap();
        g.map(OTHER, PAGE_SIZE, PHYSICAL + PAGE_SIZE, Permissions::RW).unwrap();
        let w = g.subscribe_writes("opaque cache observer");
        g.watch_writes(w, CONTIGUOUS_BASE | PHYSICAL, 2 * PAGE_SIZE).unwrap();
        (g, w)
    }

    fn observed(g: &Guest) -> Vec<u8> {
        let mut bytes = vec![0; 2 * PAGE];
        g.read_bytes(CACHE, &mut bytes, 1).unwrap();
        bytes
    }

    #[test]
    fn captured_writeback_keeps_virtual_alias_and_order() {
        let (mut guest, _) = scene();
        let (_, outputs) = guest.capture_hle_writes(6, |g| {
            Writeback::default().with(g, PAGES, |view| {
                view.write(CACHE, &[1]).unwrap();
                view.write(ALIAS, &[2]).unwrap();
                view.write(OTHER - 2, &[3, 4, 5, 6]).unwrap();
            });
        });
        assert_eq!(outputs.unwrap().writes, [
            (CACHE, vec![1]), (ALIAS, vec![2]), (OTHER - 2, vec![3, 4, 5, 6]),
        ]);
        assert_eq!(guest.u8(CACHE).unwrap(), 2);
        assert_eq!(guest.u8(OTHER).unwrap(), 5);
    }

    #[test]
    fn guest_sees_serial_bytes_between_native_spans_and_no_repeated_watch_events() {
        let (mut reference, ref_watch) = scene();
        let (mut mixed, watch) = scene();
        let mut model = Writeback::default();

        for span in 1u8..=2 {
            let writes: [(u32, &[u8]); 5] = [
                (CACHE + 62, &[span, 2, 3, 4, 5]),
                (ALIAS + 64, &[7, span]),
                (CACHE + 115, &[8, 9]),
                (ALIAS + 62, &[span + 20]),
                (OTHER - 2, &[11, span, 13, 14]),
            ];
            for (at, bytes) in writes { reference.write_bytes(at, bytes, 1).unwrap(); }
            let events = reference.take_dirty(ref_watch).unwrap();
            model.with(&mut mixed, PAGES, |m| {
                for (at, bytes) in writes { m.write(at, bytes).unwrap(); }
                let mut seen = [0; 60];
                m.read(ALIAS + 60, &mut seen).unwrap();
                assert_eq!(&seen, &observed(&reference)[60..120]);
                assert_eq!(m.take_dirty(watch).unwrap(), events);
                // Guest RAM's shader cache is published only on handoff.
                let base = PHYSICAL as usize;
                assert_ne!(m.guest.physical_ram()[base + 62], span + 20);
                assert_eq!(m.guest.physical_ram()[base + PAGE], 13);
            });
            assert_eq!(observed(&reference), observed(&mixed));
            assert!(mixed.take_dirty(watch).unwrap().is_empty());

            // One guest draw overwrites part of the first native draw's
            // state; the following native span must inherit the rest.
            if span == 1 {
                reference.write_bytes(ALIAS + 63, &[101, 102, 103], 1).unwrap();
                mixed.write_bytes(ALIAS + 63, &[101, 102, 103], 1).unwrap();
            }
        }
    }

    #[test]
    fn protected_crossing_has_no_partial_effect_and_remap_changes_aliases() {
        let (mut g, _) = scene();
        let mut model = Writeback::default();
        g.protect(OTHER, Permissions::READ).unwrap();
        model.with(&mut g, PAGES, |m| {
            m.write(CACHE + 9, &[81]).unwrap();
            assert!(m.write(OTHER - 1, &[1, 2]).is_err());
            let mut two = [99; 2];
            m.read(OTHER - 1, &mut two).unwrap();
            assert_eq!(two, [0, 0]);
        });
        g.map(0x62004, 17, PHYSICAL + 9, Permissions::RW).unwrap();
        model.with(&mut g, PAGES, |m| {
            m.write(0x62004, &[76]).unwrap();
            assert_eq!(m.guest.physical_ram()[PHYSICAL as usize + 9], 81);
            let mut seen = [0];
            m.read(CACHE + 9, &mut seen).unwrap();
            assert_eq!(seen, [76]);
        });
        assert_eq!(observed(&g)[9], 76);
    }
}
