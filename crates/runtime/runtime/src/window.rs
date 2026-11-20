//! The one stable flat RAM window (docs/runtime/contract-thin-transfers-flat-ram.md,
//! section 4.1, slice F2).
//!
//! Each runtime's memory ([`crate::ffi::Memory`]) owns one guarded 4 GiB
//! reservation for its whole life. Guest address `a` is at `base + a`.
//! Replacing or restoring the [`Guest`](crate::Guest) re-aliases RAM into
//! the same window, so `base` never changes ([`crate::Runtime::flat_base`]).
//!
//! **What the window holds.** A guest page is in the window when it is a
//! whole page mapped readable and its physical address is congruent with
//! its virtual address modulo the host page size. A host page is aliased
//! onto RAM when every mapped guest page in it is such a page and they all
//! sit in the same physical host page; its protection is the union of
//! theirs. Everything else is a guard: unmapped addresses, the NV2A range,
//! pages the access trap has armed, non-congruent pages and the host pages
//! they share. A guarded page is slow, not wrong: an access faults, takes
//! its fixup ([`Window::catch_faults`]) and goes through the helper.
//!
//! The bookkeeping (which pages are in the window, how many mapped pages
//! are outside it) follows the rule above whatever the RAM's backing. RAM
//! is aliased in only when its backing can be ([`crate::cow::Ram::alias`]:
//! flat RAM selected); [`WindowStats::aliased`] says which windows hold RAM.
//!
//! **Cards.** One byte per 4 KiB guest page and, 1,024 bytes below them,
//! one per 4 MiB region; zero means written. A store marks both (the
//! tests' `Window::mark` is the Rust model of the two C stores);
//! `Guest::fold_cards` turns them into write-watch notes before anything
//! reads or changes watch state or the mappings.
//!
//! **Counters**, always on and process-wide through [`process_stats`]:
//! fixups taken, mapped pages outside the window, and bytes the congruent
//! allocator skipped (`Guest::map_zeroed`). The agent API's
//! `profile.memory` answers them (docs/runtime/agent-api.md).

use crate::cow::Ram;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const PAGE: usize = 4096;
const VA_PAGES: usize = 1 << 20;
const VA_SIZE: usize = 1 << 32;
/// A fresh allocation's physical address is congruent with its virtual
/// address modulo this, on every host, so a snapshot's layout does not
/// depend on where it was made (`Guest::map_zeroed`).
pub const CONGRUENCE: u32 = 16 * 1024;
/// Region bytes below the cards: one per 4 MiB.
const REGIONS: usize = 1024;
const CARDS_PER_REGION: usize = VA_PAGES / REGIONS;
/// A card or region byte nothing has written since the last fold.
const CLEAN: u8 = 1;

/// What one guest page is to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Nothing the window must hold: unmapped, or mapped with no access.
    Unmapped,
    /// Mapped, but not a whole readable page congruent with its backing,
    /// or armed by the access trap: always a guard.
    Out,
    /// A whole readable page congruent with `physical`.
    In { physical: u32, write: bool },
}

/// The window counters ([`process_stats`], [`crate::Runtime::window_stats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowStats {
    /// Windows counted (one per runtime memory).
    pub windows: u64,
    /// Of those, the windows whose RAM is aliased in.
    pub aliased: u64,
    /// Faulting accesses sent to their slow path by a fixup.
    pub fixups: u64,
    /// Mapped guest pages outside the window: non-congruent, part of a
    /// host page with one, byte-granular or trap-armed.
    pub pages_outside: u64,
    /// Physical bytes the congruent allocator skipped, in all.
    pub congruence_skipped: u64,
}

impl WindowStats {
    fn add(&mut self, w: &Window) {
        self.windows += 1;
        self.aliased += u64::from(w.aliased.load(Relaxed));
        self.fixups += w.fixups.load(Relaxed);
        self.pages_outside += w.outside.load(Relaxed);
        self.congruence_skipped += w.skipped.load(Relaxed);
    }
}

/// Every live window, for [`process_stats`].
static WINDOWS: Mutex<Vec<Weak<Window>>> = Mutex::new(Vec::new());

/// The counters of every live window in the process, summed.
pub fn process_stats() -> WindowStats {
    let windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    let mut stats = WindowStats::default();
    for w in windows.iter().filter_map(Weak::upgrade) {
        stats.add(&w);
    }
    stats
}

struct Inner {
    /// One bit per guest page: in the window.
    inside: Box<[u64]>,
    /// One bit per guest page: mapped but outside the window.
    outside: Box<[u64]>,
    outside_count: u64,
}

fn bit(bits: &[u64], i: usize) -> bool {
    bits[i / 64] & (1 << (i % 64)) != 0
}
fn set_bit(bits: &mut [u64], i: usize, on: bool) {
    if on {
        bits[i / 64] |= 1 << (i % 64);
    } else {
        bits[i / 64] &= !(1 << (i % 64));
    }
}

struct Cards(Box<[AtomicU8]>);

impl Cards {
    fn new() -> Self {
        Self((0..REGIONS + VA_PAGES).map(|_| AtomicU8::new(CLEAN)).collect())
    }
    fn region(&self, r: usize) -> &AtomicU8 {
        &self.0[r]
    }
    fn card(&self, vpn: usize) -> &AtomicU8 {
        &self.0[REGIONS + vpn]
    }
}

pub struct Window {
    /// The fault region over the reservation, once asked for. Declared
    /// first: it is unregistered before the reservation and `fixups` go.
    region: Mutex<Option<hostfault::Region>>,
    reservation: os::Reservation,
    /// Host page size: 4 KiB or 16 KiB.
    host: usize,
    /// The [`Guest`](crate::Guest) whose mappings the window shows
    /// (`Guest::ident`); a replaced Guest no longer touches it.
    owner: AtomicU64,
    inner: Mutex<Inner>,
    fixups: AtomicU64,
    outside: AtomicU64,
    skipped: AtomicU64,
    aliased: AtomicBool,
    cards: OnceLock<Cards>,
}

impl Window {
    /// A new window with nothing in it, or `None` where there is none:
    /// another host page size, a target without one, or the reservation
    /// refused.
    pub(crate) fn new() -> Option<Arc<Self>> {
        if cfg!(miri) {
            return None;
        }
        let host = os::page_size()?;
        if !matches!(host, 4096 | 16384) {
            return None;
        }
        // A guard host page at either end: a span past the top lands in it.
        let reservation = os::Reservation::new(VA_SIZE + 2 * host)?;
        let window = Arc::new(Self {
            region: Mutex::new(None),
            reservation,
            host,
            owner: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                inside: vec![0; VA_PAGES / 64].into_boxed_slice(),
                outside: vec![0; VA_PAGES / 64].into_boxed_slice(),
                outside_count: 0,
            }),
            fixups: AtomicU64::new(0),
            outside: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            aliased: AtomicBool::new(false),
            cards: OnceLock::new(),
        });
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(|w| w.strong_count() > 0);
        windows.push(Arc::downgrade(&window));
        Some(window)
    }

    /// The host address of guest address 0. Fixed for the window's life.
    pub(crate) fn base(&self) -> *mut u8 {
        self.reservation.start().wrapping_add(self.host)
    }
    /// The host page size the window is built on.
    pub(crate) fn host_page(&self) -> usize {
        self.host
    }
    /// Guest pages per host page: 1 or 4.
    pub(crate) fn pages_per_host(&self) -> usize {
        self.host / PAGE
    }
    pub(crate) fn owner(&self) -> u64 {
        self.owner.load(Relaxed)
    }
    /// Make `guest` (its `Guest::ident`) the one the window shows, with
    /// its allocator's skipped bytes. The caller then refreshes the whole
    /// window from it.
    pub(crate) fn claim(&self, guest: u64, skipped: u64) {
        self.owner.store(guest, Relaxed);
        self.skipped.store(skipped, Relaxed);
    }
    pub(crate) fn set_skipped(&self, skipped: u64) {
        self.skipped.store(skipped, Relaxed);
    }
    /// RAM was aliased in at the last refresh.
    pub(crate) fn is_aliased(&self) -> bool {
        self.aliased.load(Relaxed)
    }
    /// Guest address `va`'s page is in the window.
    #[cfg(test)]
    pub(crate) fn holds(&self, va: u32) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        bit(&inner.inside, (va >> 12) as usize)
    }
    pub(crate) fn stats(&self) -> WindowStats {
        let mut stats = WindowStats::default();
        stats.add(self);
        stats
    }

    /// Send faults inside the window to their fixups, counting them.
    /// Registered once; later calls do nothing.
    pub(crate) fn catch_faults(&self) -> Result<(), hostfault::Error> {
        let mut region = self.region.lock().unwrap_or_else(|e| e.into_inner());
        if region.is_none() {
            *region = Some(hostfault::register_region(
                self.reservation.start() as usize,
                self.reservation.len(),
                on_fault,
                &self.fixups as *const AtomicU64 as usize,
            )?);
        }
        Ok(())
    }

    /// Rebuild guest pages `[first, first + slots.len())` of the window
    /// from `slots`, aliasing `ram` where the rule allows and it can be.
    /// `first` and the length are whole host pages.
    pub(crate) fn refresh(&self, ram: &Ram, first: usize, slots: &[Slot]) {
        let k = self.pages_per_host();
        debug_assert!(first.is_multiple_of(k) && slots.len().is_multiple_of(k));
        debug_assert!(first + slots.len() <= VA_PAGES);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let can_alias = os::can_alias(ram, self.host);
        // Retire every old alias in the range first, whatever RAM it was
        // of: the Guest's RAM may have been replaced since.
        let guarded = self.guard(first * PAGE, (first + slots.len()) * PAGE);
        let mut aliased = can_alias && guarded;
        // A run of host pages with consecutive backing and one protection.
        let mut run: Option<(usize, usize, usize, Prot)> = None;
        for (h, group) in slots.chunks(k).enumerate() {
            let vpn0 = first + h * k;
            let mut backing = None;
            let mut prot = Prot::None;
            let mut whole = true;
            for (i, slot) in group.iter().enumerate() {
                match *slot {
                    Slot::Unmapped => {}
                    Slot::Out => whole = false,
                    Slot::In { physical, write } => {
                        // Congruent, so the host page starts i pages lower.
                        let host_page = physical as usize - i * PAGE;
                        match backing {
                            None => backing = Some(host_page),
                            Some(b) if b != host_page => whole = false,
                            Some(_) => {}
                        }
                        prot = prot.union(if write { Prot::ReadWrite } else { Prot::Read });
                    }
                }
            }
            let hold = whole && backing.is_some();
            for (i, slot) in group.iter().enumerate() {
                let vpn = vpn0 + i;
                let inside = hold && matches!(slot, Slot::In { .. });
                set_bit(&mut inner.inside, vpn, inside);
                let now = *slot != Slot::Unmapped && !inside;
                if bit(&inner.outside, vpn) != now {
                    set_bit(&mut inner.outside, vpn, now);
                    if now {
                        inner.outside_count += 1;
                    } else {
                        inner.outside_count -= 1;
                    }
                }
            }
            if !hold || !aliased {
                aliased &= self.flush(ram, run.take());
                continue;
            }
            let physical = backing.expect("held");
            run = match run {
                Some((start, at, pages, p))
                    if start + pages == vpn0 && at + pages * PAGE == physical && p == prot =>
                {
                    Some((start, at, pages + k, p))
                }
                done => {
                    aliased &= self.flush(ram, done);
                    Some((vpn0, physical, k, prot))
                }
            };
        }
        aliased &= self.flush(ram, run);
        self.aliased.store(aliased, Relaxed);
        self.outside.store(inner.outside_count, Relaxed);
    }

    /// Alias one run, `(first page, physical, pages, protection)`. False
    /// if the system refused; the run is a guard again.
    fn flush(&self, ram: &Ram, run: Option<(usize, usize, usize, Prot)>) -> bool {
        let Some((vpn, physical, pages, prot)) = run else {
            return true;
        };
        let at = self.base().wrapping_add(vpn * PAGE);
        if os::alias(ram, at, physical, pages * PAGE, prot) {
            return true;
        }
        self.guard(vpn * PAGE, (vpn + pages) * PAGE);
        false
    }

    /// Make guest `[lo, hi)` (whole host pages) a guard.
    fn guard(&self, lo: usize, hi: usize) -> bool {
        os::guard(self.base().wrapping_add(lo), hi - lo)
    }

    /// Mark guest address `va`'s card and region written, as the two C
    /// stores after a store instruction do.
    #[cfg(test)]
    pub(crate) fn mark(&self, va: u32) {
        let cards = self.cards.get_or_init(Cards::new);
        cards.card((va >> 12) as usize).store(0, Relaxed);
        cards.region((va >> 22) as usize).store(0, Relaxed);
    }
    /// The cards (`cards[va >> 12]`, regions at `cards - 1024`), made on
    /// first use: what a flat module's per-context block points at.
    pub(crate) fn cards(&self) -> *mut u8 {
        let cards = self.cards.get_or_init(Cards::new);
        cards.0.as_ptr().wrapping_add(REGIONS).cast::<u8>().cast_mut()
    }
    /// Any card has been made yet.
    pub(crate) fn has_cards(&self) -> bool {
        self.cards.get().is_some()
    }
    /// Clean every card and call `f` with each written guest page number.
    pub(crate) fn take_cards(&self, mut f: impl FnMut(u32)) {
        let Some(cards) = self.cards.get() else {
            return;
        };
        for r in 0..REGIONS {
            if cards.region(r).load(Relaxed) != 0 {
                continue;
            }
            // The region first: a card written meanwhile marks it again.
            cards.region(r).store(CLEAN, Relaxed);
            for vpn in r * CARDS_PER_REGION..(r + 1) * CARDS_PER_REGION {
                if cards.card(vpn).load(Relaxed) == 0 {
                    cards.card(vpn).store(CLEAN, Relaxed);
                    f(vpn as u32);
                }
            }
        }
    }
    /// Whether `f` is true of any written guest page, leaving the cards.
    pub(crate) fn any_card(&self, mut f: impl FnMut(u32) -> bool) -> bool {
        let Some(cards) = self.cards.get() else {
            return false;
        };
        (0..REGIONS)
            .filter(|&r| cards.region(r).load(Relaxed) == 0)
            .flat_map(|r| r * CARDS_PER_REGION..(r + 1) * CARDS_PER_REGION)
            .any(|vpn| cards.card(vpn).load(Relaxed) == 0 && f(vpn as u32))
    }
    /// Forget every written card.
    pub(crate) fn clear_cards(&self) {
        self.take_cards(|_| {});
    }
}

/// The region callback: a faulting access with a fixup row resumes at its
/// landing label, and is counted.
fn on_fault(fault: &mut hostfault::Fault, data: usize) -> bool {
    if !hostfault::fixup(fault, 0) {
        return false;
    }
    // SAFETY: `data` is the registering Window's `fixups`, which outlives
    // its region (the region is unregistered first, see `Window`).
    unsafe { &*(data as *const AtomicU64) }.fetch_add(1, Relaxed);
    true
}

/// A host page's protection through the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prot {
    None,
    Read,
    ReadWrite,
}

impl Prot {
    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Prot::ReadWrite, _) | (_, Prot::ReadWrite) => Prot::ReadWrite,
            (Prot::Read, _) | (_, Prot::Read) => Prot::Read,
            _ => Prot::None,
        }
    }
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
mod os {
    use super::Prot;
    use crate::cow::Ram;
    use std::ptr::NonNull;

    #[cfg(target_os = "linux")]
    const RESERVE: libc::c_int =
        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE;
    #[cfg(not(target_os = "linux"))]
    const RESERVE: libc::c_int = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;

    pub(super) fn page_size() -> Option<usize> {
        // SAFETY: sysconf takes no pointers.
        usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()
    }

    pub(super) struct Reservation {
        ptr: NonNull<u8>,
        len: usize,
    }
    impl Reservation {
        pub(super) fn new(len: usize) -> Option<Self> {
            // SAFETY: an anonymous reservation the system places.
            let p = unsafe {
                libc::mmap(std::ptr::null_mut(), len, libc::PROT_NONE, RESERVE, -1, 0)
            };
            if p == libc::MAP_FAILED {
                return None;
            }
            NonNull::new(p.cast()).map(|ptr| Self { ptr, len })
        }
        pub(super) fn start(&self) -> *mut u8 {
            self.ptr.as_ptr()
        }
        pub(super) fn len(&self) -> usize {
            self.len
        }
    }
    impl Drop for Reservation {
        fn drop(&mut self) {
            // SAFETY: ours; also removes every alias inside it.
            unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
        }
    }
    // SAFETY: the reservation is plain address space the Window owns; its
    // contents change only under the Window's lock and the Guest's.
    unsafe impl Send for Reservation {}
    // SAFETY: as above.
    unsafe impl Sync for Reservation {}

    pub(super) fn can_alias(ram: &Ram, host: usize) -> bool {
        ram.can_alias() && (ram.as_ptr() as usize).is_multiple_of(host)
    }

    /// Replace `[at, at + len)`, inside the reservation, with a guard.
    pub(super) fn guard(at: *mut u8, len: usize) -> bool {
        // SAFETY: MAP_FIXED replaces only pages of our own reservation.
        unsafe {
            libc::mmap(
                at.cast(),
                len,
                libc::PROT_NONE,
                RESERVE | libc::MAP_FIXED,
                -1,
                0,
            ) != libc::MAP_FAILED
        }
    }

    /// Alias RAM `[physical, physical + len)` at `at`, inside the
    /// reservation, with `prot`.
    pub(super) fn alias(ram: &Ram, at: *mut u8, physical: usize, len: usize, prot: Prot) -> bool {
        // SAFETY: `at` is host-page aligned inside our reservation and the
        // Window's lock is held; RAM's owner synchronises the bytes.
        if !unsafe { ram.alias(at, physical, len) } {
            return false;
        }
        match prot {
            Prot::ReadWrite => true,
            // SAFETY: the alias just made, inside our reservation.
            Prot::Read => unsafe { libc::mprotect(at.cast(), len, libc::PROT_READ) == 0 },
            // SAFETY: as above.
            Prot::None => unsafe { libc::mprotect(at.cast(), len, libc::PROT_NONE) == 0 },
        }
    }
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64"))))]
mod os {
    //! No window on this target yet (contract change slice F7).
    use super::Prot;
    use crate::cow::Ram;

    pub(super) fn page_size() -> Option<usize> {
        None
    }
    pub(super) struct Reservation;
    impl Reservation {
        pub(super) fn new(_: usize) -> Option<Self> {
            None
        }
        pub(super) fn start(&self) -> *mut u8 {
            std::ptr::null_mut()
        }
        pub(super) fn len(&self) -> usize {
            0
        }
    }
    pub(super) fn can_alias(_: &Ram, _: usize) -> bool {
        false
    }
    pub(super) fn guard(_: *mut u8, _: usize) -> bool {
        true
    }
    pub(super) fn alias(_: &Ram, _: *mut u8, _: usize, _: usize, _: Prot) -> bool {
        false
    }
}

#[cfg(all(
    test,
    not(miri),
    any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64"))
))]
mod tests;
