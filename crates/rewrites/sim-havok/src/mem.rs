//! A transient view of the small Havok pool. Offsets count up from its
//! first byte. The directory grows down from its end, eight bytes per
//! live allocation, in increasing offset order.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub offset: u32,
    pub bytes: u32,
}

#[derive(Debug)]
pub struct Pool {
    bytes: u32,
    blocks: Vec<Block>,
}

impl Pool {
    /// The guest performs signed comparisons. Decline a malformed
    /// directory rather than treating wrapped sizes as available space.
    pub fn gather(bytes: u32, blocks: Vec<Block>) -> Option<Self> {
        let count = u32::try_from(blocks.len()).ok()?;
        let directory = bytes.checked_sub(count.checked_mul(8)?)?;
        let mut end = 0;
        if bytes > i32::MAX as u32 {
            return None;
        }
        for block in &blocks {
            if block.offset < end {
                return None;
            }
            end = block.offset.checked_add(block.bytes)?;
        }
        (end <= directory).then_some(Self { bytes, blocks })
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The 16-byte-aligned virtual. The guest tests whether it can fit a
    /// new directory entry before looking at any interior hole. Equality
    /// at that first test also fails, even if an earlier hole is free.
    pub fn allocate(&mut self, bytes: u32) -> Option<Block> {
        let count = u32::try_from(self.blocks.len()).ok()?;
        let limit = self.bytes.checked_sub((count + 1).checked_mul(8)?)?;
        let end = self.blocks.last().map_or(0, |b| b.offset + b.bytes);
        if end >= limit {
            return None;
        }
        let mut cursor = 0u32;
        for i in 0..=self.blocks.len() {
            let offset = cursor.checked_next_multiple_of(16)?;
            let next = self.blocks.get(i).map_or(limit, |b| b.offset);
            if offset.checked_add(bytes)? <= next {
                let block = Block { offset, bytes };
                self.blocks.insert(i, block);
                return Some(block);
            }
            if let Some(block) = self.blocks.get(i) {
                cursor = block.offset + block.bytes;
            }
        }
        None
    }

    /// Only a block's starting offset matches. Sliding the trailing
    /// entries leaves the inactive last slot in guest memory untouched.
    pub fn free(&mut self, offset: u32) {
        if let Some(i) = self.blocks.iter().position(|b| b.offset == offset) {
            self.blocks.remove(i);
        }
    }
}

/// Overflow blocks start with a copy of the previous LIFO descriptor. The
/// fast path that has sufficient room never calls these overflow virtuals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifo {
    pub cursor: u32,
    pub remaining: u32,
    pub block: u32,
    pub payload: u32,
}

impl Lifo {
    pub const HEADER_BYTES: u32 = 16;

    pub fn block_bytes(request: u32, minimum: u32) -> Option<u32> {
        request.max(minimum).checked_add(Self::HEADER_BYTES)
    }

    pub fn push(block: u32, bytes: u32, request: u32) -> Option<Self> {
        let payload = block.checked_add(Self::HEADER_BYTES)?;
        Some(Self {
            cursor: payload.checked_add(request)?,
            remaining: bytes.checked_sub(Self::HEADER_BYTES)?.checked_sub(request)?,
            block,
            payload,
        })
    }
}

// ---- the memory system, as retail's four entries drive it ----------------
//
// `0x0022c300` and `0x0022c320` allocate from and free into a directory
// pool (vtable `0x004576a4`); `0x002d7020` and `0x002d7090` push and pop an
// overflow block on any pool's LIFO descriptor, taking the block from the
// dispatcher `[0x00480118]` (`0x001472c0`, `0x00147320`), which routes by
// the mode word `[0x00479890]` and by address between the directory pool,
// the class pool (`0x0022c830`, `0x0022ca00`) and the borrowed block's
// directory pool. Each model below is a transcription over guest words.
// A route the model does not cover (the lazily built overflow object
// `0x00146e30`, a class free of more than `0x2000` bytes, an unaligned or
// unreadable word) returns `None` before anything is written, and the
// adapter lets the guest run.

use crate::layout as l;
use std::collections::BTreeMap;

/// Guest words, read only here; the adapter applies a finished journal.
pub trait Words {
    fn get(&self, addr: u32) -> Option<u32>;
}

/// Reads see this journal's writes; the base never changes until the
/// adapter applies [`Journal::finish`].
pub struct Journal<'a> {
    base: &'a dyn Words,
    writes: BTreeMap<u32, u32>,
}

impl<'a> Journal<'a> {
    pub fn new(base: &'a dyn Words) -> Self {
        Self { base, writes: BTreeMap::new() }
    }

    /// Retail's accesses are word-aligned; anything else declines.
    pub fn get(&self, addr: u32) -> Option<u32> {
        if addr & 3 != 0 {
            return None;
        }
        self.writes.get(&addr).copied().or_else(|| self.base.get(addr))
    }

    pub fn byte(&self, addr: u32) -> Option<u8> {
        Some((self.get(addr & !3)? >> (8 * (addr & 3))) as u8)
    }

    pub fn put(&mut self, addr: u32, value: u32) -> Option<()> {
        if addr & 3 != 0 {
            return None;
        }
        self.writes.insert(addr, value);
        Some(())
    }

    /// The words to store, in address order.
    pub fn finish(self) -> BTreeMap<u32, u32> {
        self.writes
    }
}

const ALLOC_SLOT: u32 = 0x10;
const FREE_SLOT: u32 = 0x14;
const DIRECTORY_ALLOCATE: u32 = 0x0022_c300;
const DIRECTORY_FREE: u32 = 0x0022_c320;
const DIRECTORY_ALLOCATE_BODY: u32 = 0x0022_c0d0;
const DIRECTORY_FREE_BODY: u32 = 0x0022_c280;
const CLASS_ALLOCATE: u32 = 0x0022_c830;
const CLASS_FREE: u32 = 0x0022_ca00;

fn directory_vtable_ok(j: &Journal, pool: u32) -> Option<()> {
    let vt = j.get(pool)?;
    (vt == l::DIRECTORY_VTABLE
        && j.get(vt + 8)? == DIRECTORY_ALLOCATE_BODY
        && j.get(vt + 0xc)? == DIRECTORY_FREE_BODY
        && j.get(vt + ALLOC_SLOT)? == DIRECTORY_ALLOCATE
        && j.get(vt + FREE_SLOT)? == DIRECTORY_FREE)
        .then_some(())
}

fn class_vtable_ok(j: &Journal, pool: u32) -> Option<()> {
    let vt = j.get(pool)?;
    (vt == l::CLASS_VTABLE
        && j.get(vt + ALLOC_SLOT)? == CLASS_ALLOCATE
        && j.get(vt + FREE_SLOT)? == CLASS_FREE)
        .then_some(())
}

fn directory_slot(base: u32, bytes: u32, index: u32) -> u32 {
    base.wrapping_add(bytes).wrapping_sub(8 * (index + 1))
}

/// A directory pool read from its object.
fn read_directory(j: &Journal, pool: u32) -> Option<(u32, Pool)> {
    directory_vtable_ok(j, pool)?;
    let base = j.get(pool + l::DIR_BASE)?;
    let bytes = j.get(pool + l::DIR_BYTES)?;
    let count = j.get(pool + l::DIR_COUNT)?;
    if count > bytes / 8 {
        return None;
    }
    let mut blocks = Vec::with_capacity(count as usize);
    for i in 0..count {
        let slot = directory_slot(base, bytes, i);
        blocks.push(Block { offset: j.get(slot)?, bytes: j.get(slot + 4)? });
    }
    Some((base, Pool::gather(bytes, blocks)?))
}

/// Rewrite the count and every slot from `from` on.
fn write_directory(j: &mut Journal, pool: u32, base: u32, bytes: u32, p: &Pool, from: usize) -> Option<()> {
    j.put(pool + l::DIR_COUNT, p.blocks().len() as u32)?;
    for (i, b) in p.blocks().iter().enumerate().skip(from) {
        let slot = directory_slot(base, bytes, i as u32);
        j.put(slot, b.offset)?;
        j.put(slot + 4, b.bytes)?;
    }
    Some(())
}

/// `0x0022c300`: the 16-byte-aligned allocation. `Some(0)` is the guest's
/// null; a refusal writes nothing.
pub fn directory_allocate(j: &mut Journal, pool: u32, size: u32) -> Option<u32> {
    let (base, mut p) = read_directory(j, pool)?;
    let bytes = j.get(pool + l::DIR_BYTES)?;
    if size > i32::MAX as u32 {
        return None;
    }
    let Some(block) = p.allocate(size) else { return Some(0) };
    // Equal blocks (zero sizes) are interchangeable, and rewriting an
    // unchanged slot is harmless: start at the first equal one.
    let first = p.blocks().iter().position(|b| *b == block)?;
    write_directory(j, pool, base, bytes, &p, first)?;
    Some(base.wrapping_add(block.offset))
}

/// `0x0022c320` into `0x0022c280`: the entry whose address is `ptr` goes.
/// The result is the guest's `eax` when found, the new count. A miss
/// writes nothing and returns 0 (the guest's `eax` is unspecified then).
pub fn directory_free(j: &mut Journal, pool: u32, ptr: u32) -> Option<u32> {
    let (base, mut p) = read_directory(j, pool)?;
    let bytes = j.get(pool + l::DIR_BYTES)?;
    let count = p.blocks().len() as u32;
    let Some(i) = p.blocks().iter().position(|b| base.wrapping_add(b.offset) == ptr) else {
        return Some(0);
    };
    p.free(ptr.wrapping_sub(base));
    // The vacated last slot keeps its stale words.
    write_directory(j, pool, base, bytes, &p, i)?;
    Some(count - 1)
}

fn class_of(j: &Journal, pool: u32, size: u32) -> Option<u32> {
    let size = i32::try_from(size).ok()?;
    let class = if size <= 0x200 {
        i32::from(j.byte(pool + l::CLASS_BY_SIZE + size as u32)? as i8)
    } else {
        j.get(pool + l::CLASS_BY_KILOBYTE + 4 * ((size - 1) >> 10) as u32)? as i32
    };
    (1..l::CLASSES as i32).contains(&class).then_some(class as u32)
}

fn bump(j: &mut Journal, addr: u32, delta: i32) -> Option<u32> {
    let v = j.get(addr)?.wrapping_add(delta as u32);
    j.put(addr, v)?;
    Some(v)
}

/// `0x0022c830`: the class pool's allocation, free list first, then the
/// bump page, carving a page's worth of free blocks for small classes.
/// The call counter moves even when the answer is null.
pub fn class_allocate(j: &mut Journal, pool: u32, size: u32) -> Option<u32> {
    class_vtable_ok(j, pool)?;
    if size as i32 > l::CLASS_PAGE_BYTES as i32 {
        return Some(0);
    }
    let c = class_of(j, pool, size)?;
    let w = 4 * c;
    let heads = pool + l::CLASS_FREE_HEADS + w;
    let allocations = pool + l::CLASS_ALLOCATIONS + w;
    let charged = pool + l::CLASS_CHARGED + w;
    let free = pool + l::CLASS_FREE + w;
    let counted = bump(j, allocations, 1)?;
    let head = j.get(heads)?;
    if head != 0 {
        let next = j.get(head)?;
        j.put(heads, next)?;
        bump(j, free, -1)?;
        return Some(head);
    }
    let slot = j.get(pool + l::CLASS_SIZES + w)?;
    let page_count = pool + l::CLASS_PAGE_COUNT;
    let cursor = pool + l::CLASS_PAGE_CURSOR;
    let limit = pool + l::CLASS_PAGE_LIMIT;
    let result;
    if size as i32 <= 0x200 {
        if j.get(cursor)?.wrapping_add(slot) > j.get(limit)? {
            let page = j.get(page_count)?;
            if page as i32 >= l::CLASS_PAGE_MAX as i32 {
                return Some(0);
            }
            let start = (page << 13).wrapping_add(pool).wrapping_add(l::CLASS_PAGES);
            j.put(pool + l::CLASS_PAGE_BASE, start)?;
            j.put(cursor, start)?;
            j.put(limit, start.wrapping_add(0x1fc0))?;
            bump(j, pool + l::CLASS_PAGES_CARVED, 1)?;
            j.put(page_count, page + 1)?;
        }
        result = j.get(cursor)?;
        let mut next = result.wrapping_add(slot);
        j.put(cursor, next)?;
        if (slot as i32) < 0x100 {
            let mut carved = slot;
            loop {
                if next.wrapping_add(slot) >= j.get(limit)? {
                    break;
                }
                bump(j, charged, 1)?;
                bump(j, free, 1)?;
                bump(j, allocations, -1)?;
                j.put(next, j.get(heads)?)?;
                j.put(heads, next)?;
                next = j.get(cursor)?.wrapping_add(slot);
                carved = carved.wrapping_add(slot);
                j.put(cursor, next)?;
                if (carved as i32) >= 0x100 {
                    break;
                }
            }
        }
    } else {
        let page = j.get(page_count)?;
        if page as i32 >= l::CLASS_PAGE_MAX as i32 {
            return Some(0);
        }
        bump(j, pool + l::CLASS_PAGES_CARVED, 1)?;
        j.put(page_count, page + 1)?;
        result = (page << 13).wrapping_add(pool).wrapping_add(l::CLASS_PAGES);
        // A class whose block is a whole page keeps it.
        if (slot as i32) < l::CLASS_PAGE_BYTES as i32 {
            let mut node = result.wrapping_add(slot);
            let mut carved = slot;
            loop {
                bump(j, charged, 1)?;
                bump(j, free, 1)?;
                bump(j, allocations, -1)?;
                j.put(node, j.get(heads)?)?;
                j.put(heads, node)?;
                carved = carved.wrapping_add(slot);
                node = node.wrapping_add(slot);
                if (carved as i32) >= l::CLASS_PAGE_BYTES as i32 {
                    break;
                }
            }
        }
    }
    // The carving's decrements are undone: the counter reads as it did
    // after the call's own increment.
    j.put(allocations, counted)?;
    bump(j, charged, 1)?;
    Some(result)
}

/// `0x0022ca00`: push `ptr` on its size's free list. A free of more than
/// `0x2000` bytes calls an import and is not modelled.
pub fn class_free(j: &mut Journal, pool: u32, ptr: u32, size: u32) -> Option<()> {
    class_vtable_ok(j, pool)?;
    if size as i32 > l::CLASS_PAGE_BYTES as i32 {
        return None;
    }
    let c = class_of(j, pool, size)?;
    let w = 4 * c;
    bump(j, pool + l::CLASS_FREE + w, 1)?;
    bump(j, pool + l::CLASS_ALLOCATIONS + w, -1)?;
    let heads = pool + l::CLASS_FREE_HEADS + w;
    j.put(ptr, j.get(heads)?)?;
    j.put(heads, ptr)
}

/// The dispatcher's pools, read once per entry.
#[derive(Clone, Copy, Debug)]
pub struct System {
    pub mode: u32,
    pub directory: u32,
    pub classes: u32,
    pub borrowed: u32,
}

impl System {
    pub fn read(j: &Journal) -> Option<Self> {
        Some(Self {
            mode: j.get(l::POOL_MODE)?,
            directory: j.get(l::DIRECTORY_POOL)?,
            classes: j.get(l::CLASS_POOL)?,
            borrowed: j.get(l::BORROWED_POOL)?,
        })
    }

    /// `0x001472c0`. Mode 1 is the directory pool alone. Otherwise the
    /// class pool, then the borrowed directory pool; a null from both
    /// reaches the overflow object, which declines.
    pub fn allocate(&self, j: &mut Journal, size: u32) -> Option<u32> {
        if self.mode == 1 {
            return directory_allocate(j, self.directory, size);
        }
        let from_classes = class_allocate(j, self.classes, size)?;
        if from_classes != 0 {
            return Some(from_classes);
        }
        if self.borrowed == 0 {
            return None;
        }
        match directory_allocate(j, self.borrowed, size)? {
            0 => None,
            block => Some(block),
        }
    }

    /// `0x00147320`: the pool whose address range holds `ptr` takes it back.
    /// Ranges compare signed, as retail's `jg`/`jge` do.
    pub fn free(&self, j: &mut Journal, ptr: u32, size: u32) -> Option<()> {
        if ptr == 0 {
            return Some(());
        }
        let at = ptr as i32;
        let classes = self.classes as i32;
        if classes.wrapping_add(l::CLASS_PAGES as i32) <= at
            && at <= classes.wrapping_add(l::CLASS_PAGE_COUNT as i32)
        {
            return class_free(j, self.classes, ptr, size);
        }
        let directory = self.directory as i32;
        if directory.wrapping_add(0x30) <= at && at <= directory.wrapping_add(0x15030) {
            directory_free(j, self.directory, ptr)?;
            return Some(());
        }
        let borrowed = self.borrowed;
        let (base, bytes) = (j.get(borrowed.checked_add(l::DIR_BASE)?)?, j.get(borrowed + l::DIR_BYTES)?);
        if base as i32 <= at && at <= (base as i32).wrapping_add(bytes as i32) {
            directory_free(j, borrowed, ptr)?;
            return Some(());
        }
        None
    }
}

/// The class `0x2d7020` asks the dispatcher for, and the smallest block.
pub const LIFO_CLASS: u32 = 0x12;
pub const LIFO_MINIMUM: u32 = 0x1000;

/// `0x002d7020`: the new overflow block becomes `this`'s descriptor, the
/// old descriptor is saved in the block's header. Returns the payload.
pub fn lifo_allocate(sys: &System, j: &mut Journal, this: u32, request: u32) -> Option<u32> {
    if request > i32::MAX as u32 {
        return None;
    }
    let bytes = Lifo::block_bytes(request, LIFO_MINIMUM)?;
    let block = sys.allocate(j, bytes)?;
    if block == 0 {
        return None;
    }
    let descriptor = this.checked_add(l::POOL_LIFO_CURSOR)?;
    for i in 0..4 {
        j.put(block + 4 * i, j.get(descriptor + 4 * i)?)?;
    }
    let lifo = Lifo::push(block, bytes, request)?;
    j.put(this + l::POOL_LIFO_REMAINING, lifo.remaining)?;
    j.put(this + l::POOL_LIFO_BLOCK, lifo.block)?;
    j.put(this + l::POOL_LIFO_PAYLOAD, lifo.payload)?;
    j.put(this + l::POOL_LIFO_CURSOR, lifo.cursor)?;
    Some(lifo.payload)
}

/// `0x002d7090`: the descriptor returns to the saved one and the block
/// goes back to the dispatcher, sized remaining + `request` + 16.
pub fn lifo_free(sys: &System, j: &mut Journal, this: u32, request: u32) -> Option<()> {
    let payload = j.get(this + l::POOL_LIFO_PAYLOAD)?;
    let remaining = j.get(this + l::POOL_LIFO_REMAINING)?;
    let block = payload.wrapping_sub(Lifo::HEADER_BYTES);
    let bytes = remaining.wrapping_add(request).wrapping_add(Lifo::HEADER_BYTES);
    for i in 0..4 {
        j.put(this + l::POOL_LIFO_CURSOR + 4 * i, j.get(block + 4 * i)?)?;
    }
    sys.free(j, block, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent occupancy model: try every aligned offset against
    /// the byte ranges in an ascending list. All fixture sizes are positive.
    fn reference(bytes: u32, live: &[Block], request: u32) -> Option<Block> {
        let limit = bytes.checked_sub((live.len() as u32 + 1) * 8)?;
        if live.iter().map(|b| b.offset + b.bytes).max().unwrap_or(0) >= limit {
            return None;
        }
        (0..limit).step_by(16).find_map(|offset| {
            let end = offset + request;
            (end <= limit && live.iter().all(|b| end <= b.offset || offset >= b.offset + b.bytes))
                .then_some(Block { offset, bytes: request })
        })
    }

    #[test]
    fn fragmented_pool_matches_byte_range_first_fit() {
        let bytes = 809;
        let mut pool = Pool::gather(bytes, vec![]).unwrap();
        let mut live = Vec::new();
        let mut seed = 17u32;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            if seed & 3 == 0 && !live.is_empty() {
                let index = (seed >> 8) as usize % live.len();
                let block: Block = live.remove(index);
                pool.free(block.offset);
            } else {
                let request = 1 + (seed >> 12) % 57;
                let want = reference(bytes, &live, request);
                assert_eq!(pool.allocate(request), want);
                if let Some(block) = want {
                    live.push(block);
                    live.sort_by_key(|b| b.offset);
                }
            }
            assert_eq!(pool.blocks(), live.as_slice());
        }
    }

    #[test]
    fn end_of_directory_precedes_hole_search() {
        let block = Block { offset: 32, bytes: 32 };
        let mut pool = Pool::gather(80, vec![block]).unwrap();
        assert_eq!(pool.allocate(1), None);
        pool.free(block.offset + 1);
        assert_eq!(pool.blocks(), &[block]);
    }

    #[test]
    fn zero_sized_allocations_share_an_offset_and_free_the_first() {
        let mut pool = Pool::gather(128, vec![]).unwrap();
        pool.allocate(8).unwrap();
        let zero = pool.allocate(0).unwrap();
        assert_eq!(zero.offset, 0);
        pool.free(0);
        assert_eq!(pool.blocks(), &[Block { offset: 0, bytes: 8 }]);
    }
}

#[cfg(test)]
mod retail_tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Heap(HashMap<u32, u32>);

    impl Words for Heap {
        fn get(&self, addr: u32) -> Option<u32> {
            Some(*self.0.get(&addr).unwrap_or(&0))
        }
    }

    impl Heap {
        fn run<T>(&mut self, f: impl FnOnce(&mut Journal) -> Option<T>) -> Option<T> {
            let mut j = Journal::new(&*self);
            let r = f(&mut j)?;
            let w = j.finish();
            self.0.extend(w);
            Some(r)
        }
        fn w(&self, a: u32) -> u32 {
            self.0.get(&a).copied().unwrap_or(0)
        }
    }

    const DIR: u32 = 0x1000_0000;
    const DIR_BYTES_: u32 = 0x15000;
    const CLS: u32 = 0x2000_0000;

    fn vtables(h: &mut Heap) {
        for (i, v) in [0x22c0a0, 0x22c0c0, 0x22c0d0, 0x22c280, 0x22c300, 0x22c320].into_iter().enumerate() {
            h.0.insert(l::DIRECTORY_VTABLE + 4 * i as u32, v);
        }
        h.0.insert(l::CLASS_VTABLE + 0x10, 0x22c830);
        h.0.insert(l::CLASS_VTABLE + 0x14, 0x22ca00);
    }

    fn directory(h: &mut Heap, at: u32) {
        h.0.insert(at, l::DIRECTORY_VTABLE);
        h.0.insert(at + l::DIR_BASE, at + 0x30);
        h.0.insert(at + l::DIR_BYTES, DIR_BYTES_);
        h.0.insert(at + l::DIR_COUNT, 0);
    }

    /// The constructor `0x0022c3e0`'s tables for the classes the tests use.
    fn classes(h: &mut Heap, at: u32) {
        h.0.insert(at, l::CLASS_VTABLE);
        let ladder = [(8, 1), (0x10, 2), (0x20, 3), (0x30, 4), (0x40, 5), (0x60, 6), (0x80, 7), (0xa0, 8),
            (0xc0, 9), (0x100, 10), (0x140, 11), (0x200, 12)];
        for (size, class) in ladder {
            h.0.insert(at + l::CLASS_SIZES + 4 * class, size);
        }
        for (size, class) in [(0x400, 13), (0x800, 14), (0x1000, 15), (0x2000, 16)] {
            h.0.insert(at + l::CLASS_SIZES + 4 * class, size);
        }
        // Byte table: one word holds four sizes.
        for s in 0..=0x200u32 {
            let class = ladder.iter().find(|(m, _)| s <= *m).unwrap().1;
            let a = (at + l::CLASS_BY_SIZE + s) & !3;
            let old = h.w(a);
            let sh = 8 * ((at + l::CLASS_BY_SIZE + s) & 3);
            h.0.insert(a, (old & !(0xff << sh)) | (class << sh));
        }
        for (k, class) in [13, 14, 15, 15, 16, 16, 16, 16].into_iter().enumerate() {
            h.0.insert(at + l::CLASS_BY_KILOBYTE + 4 * k as u32, class);
        }
    }

    fn directory_blocks(h: &Heap, at: u32) -> Vec<(u32, u32)> {
        let base = h.w(at + l::DIR_BASE);
        (0..h.w(at + l::DIR_COUNT))
            .map(|i| {
                let s = directory_slot(base, DIR_BYTES_, i);
                (h.w(s), h.w(s + 4))
            })
            .collect()
    }

    #[test]
    fn directory_allocates_in_first_fit_order_and_writes_the_slots() {
        let mut h = Heap::default();
        vtables(&mut h);
        directory(&mut h, DIR);
        let base = DIR + 0x30;
        assert_eq!(h.run(|j| directory_allocate(j, DIR, 40)), Some(base));
        assert_eq!(h.run(|j| directory_allocate(j, DIR, 8)), Some(base + 48));
        assert_eq!(h.run(|j| directory_allocate(j, DIR, 100)), Some(base + 64));
        assert_eq!(directory_blocks(&h, DIR), [(0, 40), (48, 8), (64, 100)]);
        // Freeing the middle block opens a hole the next small one takes.
        assert_eq!(h.run(|j| directory_free(j, DIR, base + 48)), Some(2));
        assert_eq!(directory_blocks(&h, DIR), [(0, 40), (64, 100)]);
        // The vacated last slot keeps its stale words.
        let last = directory_slot(base, DIR_BYTES_, 2);
        assert_eq!((h.w(last), h.w(last + 4)), (64, 100));
        assert_eq!(h.run(|j| directory_allocate(j, DIR, 16)), Some(base + 48));
        assert_eq!(directory_blocks(&h, DIR), [(0, 40), (48, 16), (64, 100)]);
        // A miss changes nothing.
        let before = h.0.clone();
        assert_eq!(h.run(|j| directory_free(j, DIR, base + 1)), Some(0));
        assert_eq!(h.0, before);
    }

    #[test]
    fn directory_full_returns_null_and_a_foreign_vtable_declines() {
        let mut h = Heap::default();
        vtables(&mut h);
        directory(&mut h, DIR);
        assert_eq!(h.run(|j| directory_allocate(j, DIR, DIR_BYTES_)), Some(0));
        assert_eq!(h.w(DIR + l::DIR_COUNT), 0);
        h.0.insert(DIR, 0x1234);
        assert_eq!(h.run(|j| directory_allocate(j, DIR, 8)), None);
    }

    #[test]
    fn class_pool_carves_a_page_and_hands_blocks_back_last_carved_first() {
        let mut h = Heap::default();
        vtables(&mut h);
        classes(&mut h, CLS);
        let page = CLS + l::CLASS_PAGES;
        let w = 4 * 4;
        assert_eq!(h.run(|j| class_allocate(j, CLS, 40)), Some(page));
        assert_eq!(h.w(CLS + l::CLASS_PAGE_COUNT), 1);
        assert_eq!(h.w(CLS + l::CLASS_PAGES_CARVED), 1);
        assert_eq!(h.w(CLS + l::CLASS_PAGE_LIMIT), page + 0x1fc0);
        // Five blocks carved after the first: 0x30 each until 0x100 is reached.
        assert_eq!(h.w(CLS + l::CLASS_CHARGED + w), 6);
        assert_eq!(h.w(CLS + l::CLASS_FREE + w), 5);
        assert_eq!(h.w(CLS + l::CLASS_ALLOCATIONS + w), 1);
        assert_eq!(h.w(CLS + l::CLASS_FREE_HEADS + w), page + 0xf0);
        assert_eq!(h.w(CLS + l::CLASS_PAGE_CURSOR), page + 0x120);
        assert_eq!(h.w(page + 0xf0), page + 0xc0);
        assert_eq!(h.w(page + 0x30), 0);
        assert_eq!(h.run(|j| class_allocate(j, CLS, 40)), Some(page + 0xf0));
        assert_eq!(h.w(CLS + l::CLASS_FREE + w), 4);
        assert_eq!(h.w(CLS + l::CLASS_ALLOCATIONS + w), 2);
        assert_eq!(h.run(|j| class_free(j, CLS, page + 0xf0, 40)), Some(()));
        assert_eq!(h.w(CLS + l::CLASS_FREE + w), 5);
        assert_eq!(h.w(CLS + l::CLASS_ALLOCATIONS + w), 1);
        assert_eq!(h.w(CLS + l::CLASS_FREE_HEADS + w), page + 0xf0);
        assert_eq!(h.w(page + 0xf0), page + 0xc0);
    }

    #[test]
    fn class_pool_large_class_takes_a_whole_page_and_exhaustion_counts_the_call() {
        let mut h = Heap::default();
        vtables(&mut h);
        classes(&mut h, CLS);
        let w = 4 * 16;
        assert_eq!(h.run(|j| class_allocate(j, CLS, 0x2000)), Some(CLS + l::CLASS_PAGES));
        assert_eq!(h.w(CLS + l::CLASS_CHARGED + w), 1);
        h.0.insert(CLS + l::CLASS_PAGE_COUNT, l::CLASS_PAGE_MAX);
        assert_eq!(h.run(|j| class_allocate(j, CLS, 0x1800)), Some(0));
        assert_eq!(h.w(CLS + l::CLASS_ALLOCATIONS + w), 2);
        assert_eq!(h.run(|j| class_allocate(j, CLS, 0x2001)), Some(0));
        assert_eq!(h.run(|j| class_free(j, CLS, CLS + 0x5000, 0x2001)), None);
    }

    fn system(h: &mut Heap, mode: u32) -> System {
        vtables(h);
        directory(h, DIR);
        classes(h, CLS);
        h.0.insert(l::POOL_MODE, mode);
        h.0.insert(l::DIRECTORY_POOL, DIR);
        h.0.insert(l::CLASS_POOL, CLS);
        h.run(|j| System::read(j)).unwrap()
    }

    #[test]
    fn lifo_pushes_an_overflow_block_and_pops_it() {
        let mut h = Heap::default();
        let sys = system(&mut h, 1);
        let this = 0x3000_0000;
        for (i, v) in [0xaaa0, 0xbbb0, 0xccc0, 0xddd0].into_iter().enumerate() {
            h.0.insert(this + l::POOL_LIFO_CURSOR + 4 * i as u32, v);
        }
        let payload = h.run(|j| lifo_allocate(&sys, j, this, 0x20)).unwrap();
        let block = DIR + 0x30;
        assert_eq!(payload, block + 16);
        // The old descriptor sits in the block header.
        assert_eq!([h.w(block), h.w(block + 4), h.w(block + 8), h.w(block + 12)], [0xaaa0, 0xbbb0, 0xccc0, 0xddd0]);
        assert_eq!(h.w(this + l::POOL_LIFO_CURSOR), payload + 0x20);
        assert_eq!(h.w(this + l::POOL_LIFO_REMAINING), 0x1000 - 0x20);
        assert_eq!(h.w(this + l::POOL_LIFO_BLOCK), block);
        assert_eq!(h.w(this + l::POOL_LIFO_PAYLOAD), payload);
        assert_eq!(directory_blocks(&h, DIR), [(0, 0x1010)]);
        // A request over the minimum sizes the block by the request.
        let big = h.run(|j| lifo_allocate(&sys, j, this, 0x2000)).unwrap();
        assert_eq!(directory_blocks(&h, DIR), [(0, 0x1010), (0x1010, 0x2010)]);
        assert_eq!(h.w(this + l::POOL_LIFO_REMAINING), 0);
        assert_eq!(big, DIR + 0x30 + 0x1010 + 16);
        h.run(|j| lifo_free(&sys, j, this, 0x2000)).unwrap();
        assert_eq!(h.w(this + l::POOL_LIFO_CURSOR), payload + 0x20);
        h.run(|j| lifo_free(&sys, j, this, 0x20)).unwrap();
        assert_eq!(h.w(this + l::POOL_LIFO_CURSOR), 0xaaa0);
        assert_eq!(h.w(this + l::POOL_LIFO_PAYLOAD), 0xddd0);
        assert_eq!(directory_blocks(&h, DIR), Vec::<(u32, u32)>::new());
    }

    #[test]
    fn class_mode_routes_the_lifo_block_to_its_free_list() {
        let mut h = Heap::default();
        let sys = system(&mut h, 2);
        let this = 0x3000_0000;
        let payload = h.run(|j| lifo_allocate(&sys, j, this, 0x100)).unwrap();
        // 0x1010 bytes is class 16, whose block is a whole page.
        let w = 4 * 16;
        assert_eq!(h.w(CLS + l::CLASS_CHARGED + w), 1);
        assert_eq!(payload, CLS + l::CLASS_PAGES + 16);
        h.run(|j| lifo_free(&sys, j, this, 0x100)).unwrap();
        assert_eq!(h.w(CLS + l::CLASS_FREE_HEADS + w), CLS + l::CLASS_PAGES);
        assert_eq!(h.w(CLS + l::CLASS_FREE + w), 1);
    }

    #[test]
    fn a_failed_class_and_borrowed_route_declines_without_writes() {
        let mut h = Heap::default();
        let sys = system(&mut h, 2);
        h.0.insert(CLS + l::CLASS_PAGE_COUNT, l::CLASS_PAGE_MAX);
        let before = h.0.clone();
        let this = 0x3000_0000;
        assert_eq!(h.run(|j| lifo_allocate(&sys, j, this, 0x100)), None);
        assert_eq!(h.0, before);
    }
}
