//! The edit arena (docs/editor/realtime-edits.md, "Four write primitives"): guest
//! memory for tag blocks that outgrow their place in the metadata.
//!
//! A tag block is a `(count, address)` pair and a run of elements with
//! nothing reserved after it, so a block that must hold more elements than
//! the map gave it is copied here and its pair repointed. The engine
//! follows any mapped pointer, so a block here reads as one in the
//! metadata does.
//!
//! **Where.** [`WINDOW`], a range of the HLE reservation
//! (`kernel::memory::HLE_RESERVED`) no other user maps: from `0xa0403000`
//! up to `0xa0800000`, below
//! DirectSound's arena (`dsound::register::ARENA`, from `0xa0800000`) and
//! above Bink's page (searched upward from `0xa0000000`). The window is
//! only addresses: pages are mapped as the arena's top grows, in
//! [`CHUNK`]s, because physical memory, not the window, bounds it. A
//! mapping that would leave less than [`PHYSICAL_FLOOR`] of physical
//! memory free is refused, so the game's own later allocations still fit.
//!
//! **Allocation** is first fit over free runs kept coalesced, each extent
//! [`ALIGN`]ed. An extent records the block it holds: its tag, the address
//! of the pair that points at it, its stride and its capacity in elements,
//! so `edit.batch` admits writes inside it as writes to that tag and
//! `world.placement.edit` knows the room a moved block has.
//!
//! **Lifetime.** One map activation ([`Arena::bind`]): a new map releases
//! every extent and unmaps every page, since nothing in the new map points
//! here. A BSP switch keeps it, as it keeps the scenario.
//!
//! Every user takes the guest's lock before the arena's (the editor runs
//! inside the guest lock; [`Link::with`] takes it first), so the two never
//! wait on each other.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard};

use runtime::Guest;
use serde_json::{Value, json};

/// The arena's addresses (module doc, "Where"): 4080 KiB.
pub const WINDOW: Range<u32> = 0xa040_4000..0xa080_0000;
pub const PAGE: u32 = 0x1000;
/// Pages are mapped at least this many bytes at a time.
pub const CHUNK: u32 = 0x1_0000;
/// Every extent starts and ends on this.
pub const ALIGN: u32 = 16;
/// Physical memory the arena leaves free for the game's own allocations.
/// Provisional: `edit.arena` reports the free physical memory, and the
/// first live measurement (docs/editor/notes/) is what this should be
/// checked against.
pub const PHYSICAL_FLOOR: u64 = 2 << 20;

/// Maps and unmaps the arena's pages: the runtime's [`Guest`] in the game,
/// an invented one in tests.
pub trait Pages {
    /// Maps `[at, at + len)` zeroed and read-write. Page-aligned.
    fn map(&mut self, at: u32, len: u32) -> Result<(), String>;
    /// Unmaps what [`Self::map`] mapped.
    fn unmap(&mut self, at: u32, len: u32);
    /// Physical bytes no mapping or reservation uses.
    fn physical_free(&mut self) -> u64;
}

impl Pages for Guest {
    fn map(&mut self, at: u32, len: u32) -> Result<(), String> {
        self.map_zeroed(at, len, runtime::guest::Permissions::RW)
            .map_err(|e| format!("mapping {len:#x} bytes at {at:#010x}: {e}"))
    }

    fn unmap(&mut self, at: u32, len: u32) {
        // The window is ours alone: a page missing here was never mapped
        // (or a restore dropped it), which is what unmapping wants.
        let _ = Guest::unmap(self, at, len);
    }

    fn physical_free(&mut self) -> u64 {
        // As the kernel counts free pages (`kernel::memory`): one probe
        // per free page, jumping occupied runs.
        let ram = runtime::guest::RAM_SIZE as u32;
        let (mut pages, mut cursor) = (0u64, 0u32);
        while let Some(page) = self.find_free_physical(PAGE, cursor..ram, PAGE) {
            pages += 1;
            match page.checked_add(PAGE) {
                Some(next) => cursor = next,
                None => break,
            }
        }
        pages * u64::from(PAGE)
    }
}

/// What a new extent will hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    /// The datum of the tag the block belongs to.
    pub tag: u32,
    /// The address of the `(count, address)` pair that will point at it.
    pub root: u32,
    /// Bytes per element.
    pub stride: u32,
    /// Elements it holds room for.
    pub capacity: u32,
}

/// One allocated extent and the block it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub at: u32,
    /// Bytes, a multiple of [`ALIGN`] at least `stride * capacity`.
    pub len: u32,
    pub tag: u32,
    pub root: u32,
    pub stride: u32,
    pub capacity: u32,
}

impl Extent {
    pub fn contains(&self, addr: u32, len: u32) -> bool {
        self.at <= addr && u64::from(addr) + u64::from(len) <= u64::from(self.at) + u64::from(self.len)
    }

    pub fn json(&self) -> Value {
        json!({"tag": format!("{:#010x}", self.tag), "root": self.root, "address": self.at,
            "capacity": self.capacity, "stride": self.stride, "len": self.len})
    }
}

/// Why an extent could not be had: the bytes asked for, the largest
/// extent the arena could give now, and the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Full {
    pub need: u32,
    pub largest_free: u32,
    pub why: String,
}

/// The allocator (module doc).
#[derive(Debug)]
pub struct Arena {
    window: Range<u32>,
    /// The end of the mapped prefix of the window.
    mapped: u32,
    /// Free runs, start to length, coalesced; together with `used` they
    /// tile the window.
    free: BTreeMap<u32, u32>,
    used: BTreeMap<u32, Extent>,
    /// The map activation the extents belong to.
    epoch: Option<u64>,
}

fn round_up(v: u64, to: u32) -> u64 {
    v.div_ceil(u64::from(to)) * u64::from(to)
}

impl Arena {
    /// An empty arena over `window` (page-aligned), nothing mapped.
    pub fn new(window: Range<u32>) -> Arena {
        assert!(
            window.start.is_multiple_of(PAGE) && window.end.is_multiple_of(PAGE) && window.start < window.end,
            "the arena's window is whole pages"
        );
        let mut free = BTreeMap::new();
        free.insert(window.start, window.end - window.start);
        Arena { mapped: window.start, window, free, used: BTreeMap::new(), epoch: None }
    }

    pub fn base(&self) -> u32 {
        self.window.start
    }

    /// The window's bytes.
    pub fn len(&self) -> u32 {
        self.window.end - self.window.start
    }

    pub fn is_empty(&self) -> bool {
        self.used.is_empty()
    }

    /// Bytes mapped so far.
    pub fn mapped(&self) -> u32 {
        self.mapped - self.window.start
    }

    /// Bytes the extents hold.
    pub fn used(&self) -> u32 {
        self.used.values().map(|e| e.len).sum()
    }

    pub fn extents(&self) -> impl Iterator<Item = &Extent> {
        self.used.values()
    }

    /// The extent that starts at `at`.
    pub fn extent(&self, at: u32) -> Option<&Extent> {
        self.used.get(&at)
    }

    /// The extent holding all of `[addr, addr + len)`.
    pub fn containing(&self, addr: u32, len: u32) -> Option<&Extent> {
        let (_, e) = self.used.range(..=addr).next_back()?;
        e.contains(addr, len).then_some(e)
    }

    /// Ties the arena to map activation `epoch`. Another activation than
    /// the one its extents were made in releases them all first; the
    /// first binding adopts whatever is there.
    pub fn bind(&mut self, pages: &mut dyn Pages, epoch: u64) {
        if self.epoch.is_some_and(|e| e != epoch) {
            self.release(pages);
        }
        self.epoch = Some(epoch);
    }

    /// Frees every extent and unmaps every page.
    pub fn release(&mut self, pages: &mut dyn Pages) {
        if self.mapped > self.window.start {
            pages.unmap(self.window.start, self.mapped - self.window.start);
        }
        *self = Arena { epoch: self.epoch, ..Arena::new(self.window.clone()) };
    }

    /// Bytes past the mapped prefix the arena may still map: page-rounded,
    /// within the window, leaving [`PHYSICAL_FLOOR`] free.
    fn spare(&self, pages: &mut dyn Pages) -> u32 {
        let free = pages.physical_free().saturating_sub(PHYSICAL_FLOOR);
        let free = free / u64::from(PAGE) * u64::from(PAGE);
        u32::try_from(free).unwrap_or(u32::MAX).min(self.window.end - self.mapped)
    }

    /// The largest extent [`Self::alloc`] could hand out now.
    pub fn largest_free(&self, pages: &mut dyn Pages) -> u32 {
        let spare = self.spare(pages);
        self.free
            .iter()
            .map(|(&at, &len)| {
                let end = at + len;
                if end <= self.mapped {
                    len
                } else {
                    // Only the top run reaches past the mapped prefix.
                    self.mapped.saturating_sub(at) + (end - at.max(self.mapped)).min(spare)
                }
            })
            .max()
            .unwrap_or(0)
            & !(ALIGN - 1)
    }

    /// The first free run that holds `claim`'s elements, mapped as needed.
    pub fn alloc(&mut self, pages: &mut dyn Pages, claim: Claim) -> Result<Extent, Full> {
        let bytes = u64::from(claim.stride) * u64::from(claim.capacity);
        let need = round_up(bytes, ALIGN);
        let full = |me: &Arena, pages: &mut dyn Pages, why: String| Full {
            need: u32::try_from(need).unwrap_or(u32::MAX),
            largest_free: me.largest_free(pages),
            why,
        };
        if bytes == 0 {
            return Err(full(self, pages, "an extent holds at least one byte".into()));
        }
        if need > u64::from(self.len()) {
            return Err(full(self, pages, format!("{need:#x} bytes is more than the arena's window")));
        }
        let need32 = need as u32;
        let Some((at, run)) = self.free.iter().map(|(&a, &l)| (a, l)).find(|&(_, l)| l >= need32) else {
            return Err(full(self, pages, "no free run in the edit arena is large enough".into()));
        };
        let end = at + need32;
        if end > self.mapped {
            let must = round_up(u64::from(end), PAGE) as u32 - self.mapped;
            let spare = self.spare(pages);
            if must > spare {
                return Err(full(
                    self,
                    pages,
                    format!(
                        "mapping {must:#x} more bytes would leave less than {PHYSICAL_FLOOR:#x} bytes of \
                         physical memory free"
                    ),
                ));
            }
            let chunked = (round_up(u64::from(end), CHUNK).min(u64::from(self.window.end)) as u32 - self.mapped)
                .min(spare)
                .max(must);
            if let Err(why) = pages.map(self.mapped, chunked) {
                return Err(full(self, pages, why));
            }
            self.mapped += chunked;
        }
        self.free.remove(&at);
        if run > need32 {
            self.free.insert(end, run - need32);
        }
        let e = Extent {
            at,
            len: need32,
            tag: claim.tag,
            root: claim.root,
            stride: claim.stride,
            capacity: claim.capacity,
        };
        self.used.insert(at, e);
        Ok(e)
    }

    /// Frees the extent at `at`, joining it to its free neighbours.
    pub fn free(&mut self, at: u32) -> Option<Extent> {
        let e = self.used.remove(&at)?;
        let (mut start, mut len) = (e.at, e.len);
        if let Some((&prev, &prev_len)) = self.free.range(..start).next_back()
            && prev + prev_len == start
        {
            self.free.remove(&prev);
            start = prev;
            len += prev_len;
        }
        if let Some(next_len) = self.free.remove(&(e.at + e.len)) {
            len += next_len;
        }
        self.free.insert(start, len);
        Some(e)
    }

    /// `edit.arena`'s answer.
    pub fn summary(&self, pages: &mut dyn Pages) -> Value {
        let physical_free = pages.physical_free();
        json!({
            "base": self.window.start,
            "len": self.len(),
            "mapped": self.mapped(),
            "used": self.used(),
            "largest_free": self.largest_free(pages),
            "physical_free": physical_free,
            "physical_floor": PHYSICAL_FLOOR,
            "moves": self.used.values().map(Extent::json).collect::<Vec<_>>(),
        })
    }
}

/// The arena the editor and `world.placement.edit` share.
pub type Shared = Arc<Mutex<Arena>>;

/// An empty arena over [`WINDOW`].
pub fn shared() -> Shared {
    Arc::new(Mutex::new(Arena::new(WINDOW)))
}

pub fn lock(a: &Shared) -> MutexGuard<'_, Arena> {
    a.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs a function with the pages, the guest's lock taken for the call
/// (`|f| f(&mut *runtime.memory())` in the game).
pub type WithPages = Arc<dyn Fn(&mut dyn FnMut(&mut dyn Pages)) + Send + Sync>;

/// The arena for a user that does not hold the guest's lock
/// (`world.placement.edit`'s calls, between their memory closures).
#[derive(Clone)]
pub struct Link {
    pub arena: Shared,
    pub pages: WithPages,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").field("arena", &self.arena).finish_non_exhaustive()
    }
}

impl Link {
    /// Runs `f` on the arena and the pages, the guest's lock first.
    pub fn with<T>(&self, f: impl FnOnce(&mut Arena, &mut dyn Pages) -> T) -> T {
        let mut f = Some(f);
        let mut out = None;
        (self.pages)(&mut |pages| {
            if let Some(f) = f.take() {
                out = Some(f(&mut lock(&self.arena), pages));
            }
        });
        out.expect("the pages function ran")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pages that map anything while `physical` bytes are free.
    struct Fake {
        physical: u64,
        refuse: bool,
        mapped: Vec<(u32, u32)>,
        unmapped: Vec<(u32, u32)>,
    }

    impl Fake {
        fn new(physical: u64) -> Fake {
            Fake { physical, refuse: false, mapped: Vec::new(), unmapped: Vec::new() }
        }
    }

    impl Pages for Fake {
        fn map(&mut self, at: u32, len: u32) -> Result<(), String> {
            if self.refuse {
                return Err("refused".into());
            }
            if u64::from(len) > self.physical {
                return Err("out of memory".into());
            }
            self.physical -= u64::from(len);
            self.mapped.push((at, len));
            Ok(())
        }
        fn unmap(&mut self, at: u32, len: u32) {
            self.physical += u64::from(len);
            self.unmapped.push((at, len));
        }
        fn physical_free(&mut self) -> u64 {
            self.physical
        }
    }

    const BASE: u32 = 0x1000_0000;

    fn claim(bytes: u32) -> Claim {
        Claim { tag: 0xe000_0001, root: 0x2000_0000, stride: 1, capacity: bytes }
    }

    fn arena() -> Arena {
        Arena::new(BASE..BASE + 0x10_0000)
    }

    #[test]
    fn first_fit_takes_the_lowest_run_that_holds_it() {
        let mut p = Fake::new(64 << 20);
        let mut a = arena();
        let x = a.alloc(&mut p, claim(0x100)).unwrap();
        let y = a.alloc(&mut p, claim(0x10)).unwrap();
        let z = a.alloc(&mut p, claim(0x100)).unwrap();
        assert_eq!((x.at, y.at, z.at), (BASE, BASE + 0x100, BASE + 0x110));
        // Sizes round up to the alignment.
        let odd = a.alloc(&mut p, claim(3)).unwrap();
        assert_eq!((odd.at, odd.len), (BASE + 0x210, ALIGN));
        // A hole of 0x100 at the bottom: a small claim fits it, a large one
        // goes past it.
        a.free(x.at).unwrap();
        assert_eq!(a.alloc(&mut p, claim(0x200)).unwrap().at, BASE + 0x220);
        assert_eq!(a.alloc(&mut p, claim(0x80)).unwrap().at, BASE);
        assert_eq!(a.alloc(&mut p, claim(0x80)).unwrap().at, BASE + 0x80);
        // The first mapping is one chunk, made once.
        assert_eq!(p.mapped, vec![(BASE, CHUNK)]);
        assert_eq!(a.mapped(), CHUNK);
    }

    #[test]
    fn free_returns_the_extent_and_forgets_it() {
        let mut p = Fake::new(64 << 20);
        let mut a = arena();
        let x = a.alloc(&mut p, Claim { tag: 7, root: 0x40, stride: 0x54, capacity: 16 }).unwrap();
        assert_eq!(a.extent(x.at), Some(&x));
        assert_eq!(a.containing(x.at + 0x54, 0x54), Some(&x));
        assert_eq!(a.containing(x.at + x.len - 4, 8), None, "past its end");
        assert_eq!(a.used(), x.len);
        assert_eq!(a.free(x.at), Some(x));
        assert_eq!(a.free(x.at), None, "a second free is nothing");
        assert_eq!(a.used(), 0);
        assert!(a.containing(x.at, 4).is_none());
    }

    #[test]
    fn freed_neighbours_coalesce_into_one_run() {
        let mut p = Fake::new(64 << 20);
        let mut a = arena();
        let [x, y, z] = [0; 3].map(|_| a.alloc(&mut p, claim(0x100)).unwrap());
        // Free the outer two, then the middle: one run from the base again.
        a.free(x.at);
        a.free(z.at);
        assert_eq!(a.free.len(), 2, "{:?}", a.free);
        a.free(y.at);
        assert_eq!(a.free.len(), 1, "{:?}", a.free);
        assert_eq!(a.free.get(&BASE), Some(&a.len()));
        // So a claim of all three fits at the base.
        assert_eq!(a.alloc(&mut p, claim(0x300)).unwrap().at, BASE);
    }

    #[test]
    fn a_full_arena_refuses_with_the_need_and_the_largest_free() {
        // The window holds 1 MiB; physical memory allows the floor plus
        // three chunks.
        let mut p = Fake::new(PHYSICAL_FLOOR + 3 * u64::from(CHUNK));
        let mut a = arena();
        let big = a.alloc(&mut p, claim(2 * CHUNK)).unwrap();
        assert_eq!(a.mapped(), 2 * CHUNK);
        // One chunk may still be mapped.
        assert_eq!(a.largest_free(&mut p), CHUNK);
        let e = a.alloc(&mut p, claim(CHUNK + 16)).unwrap_err();
        assert_eq!(e.need, CHUNK + 16);
        assert_eq!(e.largest_free, CHUNK);
        assert!(e.why.contains("physical memory"), "{}", e.why);
        // Nothing changed: the last chunk is still there to take.
        assert_eq!(a.alloc(&mut p, claim(CHUNK)).unwrap().at, BASE + 2 * CHUNK);
        assert_eq!(a.largest_free(&mut p), 0);
        // Freeing makes room again without mapping.
        a.free(big.at);
        assert_eq!(a.largest_free(&mut p), 2 * CHUNK);
        assert_eq!(a.alloc(&mut p, claim(CHUNK)).unwrap().at, BASE);
        // A claim larger than the window is refused outright.
        let e = a.alloc(&mut p, claim(a.len() + 1)).unwrap_err();
        assert!(e.why.contains("window"), "{}", e.why);
        // And a mapping the pages refuse is a full arena too, with their
        // reason.
        let mut refusing = Fake { refuse: true, ..Fake::new(64 << 20) };
        let mut b = arena();
        let e = b.alloc(&mut refusing, claim(16)).unwrap_err();
        assert_eq!(e.why, "refused");
        assert_eq!((b.mapped(), b.used()), (0, 0));
    }

    #[test]
    fn a_new_activation_releases_everything() {
        let mut p = Fake::new(64 << 20);
        let mut a = arena();
        a.alloc(&mut p, claim(0x100)).unwrap();
        // The first binding adopts what is there.
        a.bind(&mut p, 3);
        assert_eq!(a.used(), 0x100);
        a.bind(&mut p, 3);
        assert_eq!(a.used(), 0x100, "the same activation keeps it");
        a.bind(&mut p, 4);
        assert_eq!(a.used(), 0);
        assert_eq!(a.mapped(), 0);
        assert_eq!(p.unmapped, vec![(BASE, CHUNK)]);
        assert_eq!(a.alloc(&mut p, claim(0x10)).unwrap().at, BASE);
    }

    #[test]
    fn the_window_is_whole_pages_inside_the_hle_reservation() {
        assert!(WINDOW.start.is_multiple_of(PAGE) && WINDOW.end.is_multiple_of(PAGE));
        const { assert!(WINDOW.start >= 0xa000_0000 && WINDOW.end <= 0xa100_0000) };
        // Clear of the NV2A window's pages and DirectSound's arena.
        const { assert!(WINDOW.start >= 0xa040_3000 + PAGE) };
        const { assert!(WINDOW.end <= 0xa080_0000) };
    }
}
