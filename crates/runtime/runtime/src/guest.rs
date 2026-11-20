//! Checked virtual mappings over one 64 MB physical store. Physical offsets
//! identify aliases; high virtual addresses are never masked into RAM.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

mod state;
mod view;
mod writeback;
pub use writeback::{Writeback, WritebackView};
pub use state::{ClaimRecord, GuestLayout, ReservationRecord, RestoreError};
pub use view::{FrozenRam, HandBack, LINE_SIZE, WrittenPage};

pub const RAM_SIZE: usize = 64 * 1024 * 1024;
pub const PAGE_SIZE: u32 = 4096;
const MMIO_START: u64 = 0xfd00_0000;
const MMIO_END: u64 = 0xfe00_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Execute,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultReason {
    Unmapped,
    Protection,
    AddressOverflow,
    Misaligned,
    UnsupportedDevice,
    InvalidWidth,
    InvalidOperation,
    InvalidBuffer,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("guest {access:?} at {addr:#010x}+{len}: {reason:?}")]
pub struct GuestFault {
    pub addr: u32,
    pub len: usize,
    pub access: Access,
    pub reason: FaultReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions(u8);
impl Permissions {
    /// PAGE_NOACCESS: every access faults with `FaultReason::Protection`.
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1);
    pub const RW: Self = Self(3);
    pub const RX: Self = Self(5);
    pub const RWX: Self = Self(7);
    pub(crate) fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    fn allows(self, access: Access) -> bool {
        self.0
            & match access {
                Access::Read => 1,
                Access::Write => 2,
                Access::Execute => 4,
            }
            != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub virtual_start: u32,
    pub len: u32,
    pub physical_start: u32,
    pub permissions: Permissions,
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum MapError {
    #[error("invalid virtual or physical mapping range")]
    InvalidRange,
    #[error("overlapping virtual mapping")]
    Overlap,
    #[error("64 MB physical memory exhausted")]
    OutOfMemory,
    /// Part of the requested virtual range has no mapping.
    #[error("range is not entirely mapped")]
    Unmapped,
    /// The range touches a physical reservation or one of its alias windows.
    #[error("range conflicts with a physical reservation")]
    Reserved,
    /// The range touches an HLE-owned virtual claim.
    #[error("range touches an HLE-owned virtual claim")]
    Claimed,
    /// Unmapping, or removing Execute from, a page of guarded lifted code.
    /// Generated code checks Execute only at dispatch and jumps locally
    /// across pages, so the change could not be enforced.
    #[error("range would remove Execute from lifted code")]
    LiftedCode,
}

/// Identifies one [`PhysicalReservation`]; returned by
/// [`Guest::reserve_physical`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReservationId(u32);

/// Physical RAM withheld from every allocator. It is reachable only through
/// [`Guest::map_reserved`] at the same offset inside one of its alias windows,
/// each `[alias, alias + len)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalReservation {
    pub id: ReservationId,
    pub physical_start: u32,
    pub len: u32,
    pub aliases: Vec<u32>,
    pub owner: &'static str,
}
impl PhysicalReservation {
    fn end(&self) -> u64 {
        self.physical_start as u64 + self.len as u64
    }
}

/// Identifies one [`VirtualClaim`]; returned by [`Guest::claim_virtual`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClaimId(u32);

/// Virtual range owned by HLE. Guest-driven [`Guest::unmap`] and
/// [`Guest::protect_range`] refuse to touch it; its owner uses the `_owned`
/// variants. Claims do not stop ordinary mapping inside them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualClaim {
    pub id: ClaimId,
    pub virtual_start: u32,
    pub len: u32,
    pub owner: &'static str,
}
impl VirtualClaim {
    fn end(&self) -> u64 {
        self.virtual_start as u64 + self.len as u64
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct FastPage {
    pub physical: u32,
    pub permissions: u32,
}

pub struct Guest {
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    flat: Option<crate::flat::Arena>,
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    flat_requested: bool,
    ram: crate::cow::Ram,
    mappings: Vec<Mapping>,
    // Hints only: every hit rechecks the current mapping's virtual range.
    // Never cache permissions or RAM pointers. Atomics retain Guest: Sync.
    mapping_hints: [AtomicUsize; 8],
    fast_pages: Option<Box<[FastPage]>>,
    next_physical: u32,
    reservations: Vec<PhysicalReservation>,
    claims: Vec<VirtualClaim>,
    code: Vec<(u64, u64)>,
    next_id: u32,
    pub(crate) watch: crate::watch::WriteWatch,
    hle_writes: Option<HleWrites>,
    /// Names the current mappings and permissions ([`Guest::layout_id`]).
    layout: u64,
    /// Names what lifted code can run ([`Guest::code_id`]).
    code_id: u64,
    /// The access trap, when a research observer installed one
    /// ([`crate::trap`]).
    trap: Option<Box<crate::trap::AccessTrap>>,
    /// For a copy-on-write view ([`Guest::cow_refresh`]): the source's
    /// layout its mappings and page table were last copied under (0: none).
    cow_source_layout: u64,
    /// RAM replaced without per-page notifications (COW refresh or rollback).
    watch_revision: u64,
    /// The runtime memory's flat RAM window ([`crate::window`]), once a
    /// memory has attached this Guest. It shows this Guest's mappings only
    /// while its owner is `ident`: a replaced Guest no longer touches it.
    window: Option<std::sync::Arc<crate::window::Window>>,
    /// Unique in the process, for the window's owner test.
    ident: u64,
    /// Physical bytes the congruent allocator has skipped
    /// ([`Guest::map_zeroed`]).
    congruence_skipped: u64,
    /// The runtime runs a flat-RAM module: RAM is kept in a backing the
    /// window can alias ([`Guest::require_aliasable_ram`]).
    aliasable_ram: bool,
}

/// Guest ABI outputs. The caller recreates zeroed allocations with
/// `map_zeroed` before applying the ordered writes. An allocation never
/// aliases existing RAM, so every earlier write is independent of its bytes.
#[derive(Debug, Default)]
pub struct HleOutputs {
    pub zeroed: Vec<(u32, u32, Permissions)>,
    pub writes: Vec<(u32, Vec<u8>)>,
}

struct HleWrites {
    outputs: HleOutputs,
    remaining: usize,
    expected_layout: u64,
    incomplete: bool,
}

impl HleWrites {
    fn store(&mut self, addr: u32, value: &[u8]) {
        if value.is_empty() || self.incomplete {
            return;
        }
        if self.outputs.writes.len() == 4096 || value.len() > self.remaining {
            self.incomplete = true;
            return;
        }
        self.remaining -= value.len();
        self.outputs.writes.push((addr, value.to_vec()));
    }
}

/// Layout identities, unique across every [`Guest`] in the process, so a
/// replaced or restored Guest never repeats one an earlier layout had.
static NEXT_LAYOUT: AtomicU64 = AtomicU64::new(1);
fn fresh_layout() -> u64 {
    NEXT_LAYOUT.fetch_add(1, Ordering::Relaxed)
}

fn overlaps(a: u64, a_end: u64, b: u64, b_end: u64) -> bool {
    a < b_end && b < a_end
}
fn page_aligned(v: u64) -> bool {
    v.is_multiple_of(PAGE_SIZE as u64)
}
fn page_end(v: u64) -> u64 {
    v.div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64
}
/// The lowest address at or after page-aligned `start` congruent with
/// `va`'s page modulo [`crate::window::CONGRUENCE`].
fn congruent_at_or_after(start: u64, va: u32) -> u64 {
    let c = crate::window::CONGRUENCE as u64;
    let want = va as u64 & (c - 1) & !(PAGE_SIZE as u64 - 1);
    start + ((want + c - (start & (c - 1))) & (c - 1))
}

impl Default for Guest {
    fn default() -> Self {
        Self::new()
    }
}
impl Guest {
    /// RAM starts zeroed and entirely unmapped, including guest address zero.
    pub fn new() -> Self {
        Self {
            #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
            flat: None,
            #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
            flat_requested: crate::flat::enabled(),
            ram: crate::cow::Ram::zeroed(RAM_SIZE),
            mappings: Vec::new(),
            mapping_hints: std::array::from_fn(|_| AtomicUsize::new(usize::MAX)),
            fast_pages: None,
            next_physical: 0,
            reservations: Vec::new(),
            claims: Vec::new(),
            code: Vec::new(),
            next_id: 1,
            watch: Default::default(),
            hle_writes: None,
            layout: fresh_layout(),
            code_id: fresh_layout(),
            trap: None,
            cow_source_layout: 0, watch_revision: 0,
            window: None,
            ident: fresh_layout(),
            congruence_skipped: 0,
            aliasable_ram: false,
        }
    }
    /// Identifies the mappings and their permissions: a new value, never
    /// seen before, after any change to either (a map, protect or unmap,
    /// a snapshot layout adopted). The runner remembers the layout a target
    /// last passed its execute check under, and checks again only once this
    /// has moved ([`crate::Runtime::layout_epoch`]).
    pub(crate) fn layout_id(&self) -> u64 {
        self.layout
    }
    /// Identifies which lifted code ([`Guest::guard_code`]) has the
    /// execute bit: a new value, never seen before, after a guard is added,
    /// after a whole-mapping [`Guest::protect`] of a mapping with lifted
    /// code, and after a snapshot layout or a view's source layout is
    /// adopted. The other mapping changes cannot take Execute from lifted
    /// code (`LiftedCode`), so they leave it. The runtime's entry table is
    /// checked again only once this has moved ([`crate::direct`]).
    pub fn code_id(&self) -> u64 {
        self.code_id
    }
    /// The code identity moves on ([`Guest::code_id`]).
    pub(crate) fn code_moved(&mut self) {
        self.code_id = fresh_layout();
    }
    /// `addr` is lifted code ([`Guest::guard_code`]) mapped with the execute
    /// bit: what the entry table needs before a direct call may skip the
    /// runner's execute check.
    pub(crate) fn lifted_executable(&self, addr: u32) -> bool {
        self.touches_code(addr as u64, addr as u64 + 1) && self.check(addr, 1, Access::Execute, 1).is_ok()
    }
    /// Identity of the virtual layout and revision of unreported RAM replacements.
    /// Together with a write subscription these validate cached virtual reads.
    pub fn write_watch_epoch(&self) -> (u64, u64) {
        (self.layout, self.watch_revision)
    }

    /// Subscribe to the physical pages backing a previously read virtual range.
    /// A layout change requires subscribing to its new backing pages.
    pub fn watch_virtual_read(&mut self, id: crate::WatchId, addr: u32, len: usize)
        -> Result<(), crate::watch::WatchError> {
        use crate::watch::{CONTIGUOUS_BASE, WatchError};
        let mut index = self.checked_start(addr, len, Access::Read, 1)
            .ok().flatten().ok_or(WatchError::InvalidRange)?;
        let mut at = u64::from(addr);
        let end = at + len as u64;
        while at < end {
            let range = self.span(index, at, end);
            self.watch_writes(id, CONTIGUOUS_BASE + range.start as u32, range.len() as u32)?;
            at += range.len() as u64;
            index += 1;
        }
        Ok(())
    }

    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
    }
    /// All of physical RAM, read-only, for whole-machine observers such as
    /// the lockstep per-tick hash (docs/game/high-framerate.md). Reading it is
    /// uncharged and changes no guest-visible state.
    pub fn physical_ram(&self) -> &[u8] {
        &self.ram
    }
    pub fn physical_reservations(&self) -> &[PhysicalReservation] {
        &self.reservations
    }
    pub fn virtual_claims(&self) -> &[VirtualClaim] {
        &self.claims
    }
    /// Declare `[va, va + len)`, rounded out to whole pages, as lifted guest
    /// code that generated handlers may run. From then on [`Guest::unmap`],
    /// [`Guest::protect_range`] and their `_owned` variants refuse with
    /// `LiftedCode` to unmap any of those pages or to apply permissions
    /// without Execute to them. The loader guards every executable XBE
    /// section; whoever registers a generated module for code elsewhere must
    /// guard it too. Guards are permanent. Zero or overflowing bounds are
    /// `InvalidRange`. The whole-mapping [`Guest::protect`] is not guarded.
    pub fn guard_code(&mut self, va: u32, len: u32) -> Result<(), MapError> {
        let end = va as u64 + len as u64;
        if len == 0 || end > 1u64 << 32 {
            return Err(MapError::InvalidRange);
        }
        self.code
            .push((va as u64 & !(PAGE_SIZE as u64 - 1), page_end(end)));
        self.code_moved();
        Ok(())
    }
    /// Page-rounded ranges passed to `guard_code`; ends may equal 2^32.
    pub fn code_ranges(&self) -> Vec<std::ops::Range<u64>> {
        self.code.iter().map(|&(a, b)| a..b).collect()
    }
    fn touches_code(&self, start: u64, end: u64) -> bool {
        self.code.iter().any(|&(a, b)| overlaps(start, end, a, b))
    }

    /// Explicit aliases may share physical bytes, but virtual ranges cannot
    /// overlap and the NV2A aperture cannot acquire an ordinary RAM mapping.
    pub fn map(
        &mut self,
        va: u32,
        len: u32,
        physical: u32,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        if physical as u64 + len as u64 > RAM_SIZE as u64 {
            return Err(MapError::InvalidRange);
        }
        self.check_virtual(va, len)?;
        if self.physical_reserved(physical as u64, physical as u64 + len as u64) {
            return Err(MapError::Reserved);
        }
        self.fold_cards();
        self.insert_mapping(va, len, physical, permissions);
        self.next_physical = self.next_physical.max(physical + len);
        Ok(())
    }
    /// The virtual half of [`Guest::map`]'s validation.
    fn check_virtual(&self, va: u32, len: u32) -> Result<(), MapError> {
        let end = va as u64 + len as u64;
        if len == 0 || end > 1u64 << 32 || ((va as u64) < MMIO_END && end > MMIO_START) {
            return Err(MapError::InvalidRange);
        }
        if self.mappings.iter().any(|m| {
            (va as u64) < m.virtual_start as u64 + m.len as u64 && end > m.virtual_start as u64
        }) {
            return Err(MapError::Overlap);
        }
        if self.in_alias_window(va as u64, end) {
            return Err(MapError::Reserved);
        }
        Ok(())
    }
    fn insert_mapping(&mut self, va: u32, len: u32, physical: u32, permissions: Permissions) {
        self.mappings.push(Mapping {
            virtual_start: va,
            len,
            physical_start: physical,
            permissions,
        });
        self.mappings.sort_unstable_by_key(|m| m.virtual_start);
        self.refresh_fast_range(va as u64, va as u64 + len as u64);
    }

    /// Allocate distinct page-aligned physical storage for a virtual range,
    /// congruent with `va` modulo [`crate::window::CONGRUENCE`] so the flat
    /// RAM window can hold it ([`crate::window`]). Storage comes from
    /// never-used RAM above every earlier mapping, skipping physical
    /// reservations and up to 12 KiB to reach the residue (counted in
    /// [`crate::window::WindowStats::congruence_skipped`]). When that runs
    /// out, the lowest free gap left by [`Guest::unmap`] is reused and zeroed
    /// first ([`Guest::find_free_physical_for`]).
    pub fn map_zeroed(
        &mut self,
        va: u32,
        len: u32,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        self.check_virtual(va, len)?;
        let before = self.layout;
        if let Some((physical, skipped)) = self.fresh_physical(va, len) {
            self.map(va, len, physical, permissions)?;
            if skipped != 0 {
                self.congruence_skipped += skipped;
                if let Some(w) = self.owned_window() {
                    w.set_skipped(self.congruence_skipped);
                }
            }
        } else {
            let physical = self
                .find_free_physical_for(va, len, 0..RAM_SIZE as u32)
                .ok_or(MapError::OutOfMemory)?;
            self.map(va, len, physical, permissions)?;
            self.fill_zero(physical as usize, len as usize);
        }
        if let Some(w) = &mut self.hle_writes {
            if w.expected_layout != before || w.outputs.zeroed.len() == 4096 {
                w.incomplete = true;
            }
            if !w.incomplete {
                w.outputs.zeroed.push((va, len, permissions));
                w.expected_layout = self.layout;
            }
        }
        Ok(())
    }
    /// Never-used RAM for `[va, va + len)`, congruent with `va`, and the
    /// bytes skipped to reach the residue.
    fn fresh_physical(&self, va: u32, len: u32) -> Option<(u32, u64)> {
        let mut start = page_end(self.next_physical as u64);
        let mut skipped = 0;
        loop {
            let congruent = congruent_at_or_after(start, va);
            skipped += congruent - start;
            start = congruent;
            let end = start + len as u64;
            if end > RAM_SIZE as u64 {
                return None;
            }
            match self
                .reservations
                .iter()
                .filter(|r| overlaps(start, end, r.physical_start as u64, r.end()))
                .map(|r| r.end())
                .max()
            {
                Some(next) => start = page_end(next),
                None => return Some((start as u32, skipped)),
            }
        }
    }
    /// Lowest page-aligned base inside `range` with `len` free bytes per
    /// [`Guest::physical_free`] for a mapping at `va`: congruent with `va`
    /// modulo [`crate::window::CONGRUENCE`] when there is one, so the flat
    /// RAM window can hold it, else any (slow through the window, not
    /// wrong; counted in [`crate::window::WindowStats::pages_outside`]).
    /// None if `len` is zero or nothing fits.
    pub fn find_free_physical_for(
        &self,
        va: u32,
        len: u32,
        range: std::ops::Range<u32>,
    ) -> Option<u32> {
        if len == 0 {
            return None;
        }
        let limit = (range.end as u64).min(RAM_SIZE as u64);
        let mut start = range.start as u64;
        loop {
            start = congruent_at_or_after(page_end(start), va);
            let end = start + page_end(len as u64);
            if end > limit {
                break;
            }
            match self.physical_conflict_end(start, end) {
                Some(next) => start = next,
                None => return Some(start as u32),
            }
        }
        self.find_free_physical(len, range, PAGE_SIZE)
    }
    pub fn protect(&mut self, va: u32, permissions: Permissions) -> Result<(), MapError> {
        if !self.mappings.iter().any(|m| m.virtual_start == va) {
            return Err(MapError::InvalidRange);
        }
        self.fold_cards();
        let m = self
            .mappings
            .iter_mut()
            .find(|m| m.virtual_start == va)
            .ok_or(MapError::InvalidRange)?;
        m.permissions = permissions;
        let end = m.virtual_start as u64 + m.len as u64;
        self.refresh_fast_range(va as u64, end);
        // Unguarded: this can take Execute from lifted code.
        if self.touches_code(va as u64, end) {
            self.code_moved();
        }
        Ok(())
    }

    /// True if no mapping uses any page of `[physical, physical + len)` and no
    /// reservation covers it. Mapping extents are rounded out to whole pages.
    pub fn physical_free(&self, physical: u32, len: u32) -> bool {
        let start = physical as u64 & !(PAGE_SIZE as u64 - 1);
        let end = page_end(physical as u64 + len as u64);
        len != 0 && end <= RAM_SIZE as u64 && self.physical_conflict_end(start, end).is_none()
    }
    /// Lowest `alignment`-aligned (at least page-aligned) base inside `range`
    /// with `len` free bytes per [`Guest::physical_free`]. `range.end` is
    /// exclusive and clipped to RAM. None if `len` is zero, the alignment is
    /// not a power of two, or nothing fits.
    pub fn find_free_physical(
        &self,
        len: u32,
        range: std::ops::Range<u32>,
        alignment: u32,
    ) -> Option<u32> {
        if len == 0 || !alignment.is_power_of_two() {
            return None;
        }
        let align = alignment.max(PAGE_SIZE) as u64;
        let limit = (range.end as u64).min(RAM_SIZE as u64);
        let mut start = (range.start as u64).div_ceil(align) * align;
        loop {
            let end = start + page_end(len as u64);
            if end > limit {
                return None;
            }
            match self.physical_conflict_end(start, end) {
                Some(next) => start = next.div_ceil(align) * align,
                None => return Some(start as u32),
            }
        }
    }
    fn physical_conflict_end(&self, start: u64, end: u64) -> Option<u64> {
        let mapped = self
            .mappings
            .iter()
            .map(|m| {
                let p = m.physical_start as u64;
                (p & !(PAGE_SIZE as u64 - 1), page_end(p + m.len as u64))
            })
            .chain(
                self.reservations
                    .iter()
                    .map(|r| (r.physical_start as u64, r.end())),
            );
        mapped
            .filter(|&(a, b)| overlaps(start, end, a, b))
            .map(|(_, b)| b)
            .max()
    }
    fn physical_reserved(&self, start: u64, end: u64) -> bool {
        self.reservations
            .iter()
            .any(|r| overlaps(start, end, r.physical_start as u64, r.end()))
    }
    fn in_alias_window(&self, start: u64, end: u64) -> bool {
        self.reservations.iter().any(|r| {
            r.aliases
                .iter()
                .any(|&a| overlaps(start, end, a as u64, a as u64 + r.len as u64))
        })
    }
    fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Withhold physical `[physical, physical + len)` from every allocator and
    /// from [`Guest::map`]. Each alias base `a` reserves the virtual window
    /// `[a, a + len)`; only [`Guest::map_reserved`] can map there, and only at
    /// the identical offset, so every alias sees the same bytes. All values
    /// must be page-aligned and `len` non-zero. Call this before anything uses
    /// the range: an existing mapping in a window or of those physical pages
    /// is `Overlap`. Another reservation's physical range or window is
    /// `Reserved`. Bad bounds, RAM overflow and windows touching the NV2A
    /// aperture are `InvalidRange`. The reserved bytes are zeroed.
    pub fn reserve_physical(
        &mut self,
        physical: u32,
        len: u32,
        aliases: &[u32],
        owner: &'static str,
    ) -> Result<ReservationId, MapError> {
        let (p, n) = (physical as u64, len as u64);
        if n == 0 || !page_aligned(p) || !page_aligned(n) || p + n > RAM_SIZE as u64 {
            return Err(MapError::InvalidRange);
        }
        for &a in aliases {
            let a = a as u64;
            if !page_aligned(a) || a + n > 1u64 << 32 || overlaps(a, a + n, MMIO_START, MMIO_END) {
                return Err(MapError::InvalidRange);
            }
        }
        for (i, &a) in aliases.iter().enumerate() {
            let a = a as u64;
            if aliases[..i]
                .iter()
                .any(|&b| overlaps(a, a + n, b as u64, b as u64 + n))
                || self.in_alias_window(a, a + n)
            {
                return Err(MapError::Reserved);
            }
        }
        if self.physical_reserved(p, p + n) {
            return Err(MapError::Reserved);
        }
        if self.mappings.iter().any(|m| {
            let (v, q) = (m.virtual_start as u64, m.physical_start as u64);
            overlaps(p, p + n, q, q + m.len as u64)
                || aliases
                    .iter()
                    .any(|&a| overlaps(a as u64, a as u64 + n, v, v + m.len as u64))
        }) {
            return Err(MapError::Overlap);
        }
        self.fill_zero(p as usize, n as usize);
        let id = ReservationId(self.fresh_id());
        self.reservations.push(PhysicalReservation {
            id,
            physical_start: physical,
            len,
            aliases: aliases.to_vec(),
            owner,
        });
        Ok(id)
    }
    /// Map `[va, va + len)` inside one alias window of `reservation` onto the
    /// reserved physical bytes at the same offset. Bytes are not cleared, so
    /// a later remap sees what an earlier one wrote. Unknown reservation or
    /// zero/unaligned bounds are `InvalidRange`; a range not wholly inside one
    /// of its windows is `Reserved`; an existing mapping there is `Overlap`.
    pub fn map_reserved(
        &mut self,
        reservation: ReservationId,
        va: u32,
        len: u32,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        let (v, n) = (va as u64, len as u64);
        let r = self
            .reservations
            .iter()
            .find(|r| r.id == reservation)
            .ok_or(MapError::InvalidRange)?;
        if n == 0 || !page_aligned(v) || !page_aligned(n) {
            return Err(MapError::InvalidRange);
        }
        let alias = r
            .aliases
            .iter()
            .copied()
            .find(|&a| v >= a as u64 && v + n <= a as u64 + r.len as u64)
            .ok_or(MapError::Reserved)?;
        let physical = r.physical_start + (va - alias);
        if self.mappings.iter().any(|m| {
            overlaps(
                v,
                v + n,
                m.virtual_start as u64,
                m.virtual_start as u64 + m.len as u64,
            )
        }) {
            return Err(MapError::Overlap);
        }
        self.fold_cards();
        self.insert_mapping(va, len, physical, permissions);
        Ok(())
    }

    /// Mark `[va, va + len)` as HLE-owned. Guest-driven [`Guest::unmap`] and
    /// [`Guest::protect_range`] then fail with `Claimed` on any overlap. The
    /// claim does not map anything and does not block [`Guest::map`].
    /// Zero/unaligned/overflowing bounds are `InvalidRange`; overlapping
    /// another claim is `Claimed`.
    pub fn claim_virtual(
        &mut self,
        va: u32,
        len: u32,
        owner: &'static str,
    ) -> Result<ClaimId, MapError> {
        let (v, n) = (va as u64, len as u64);
        if n == 0 || !page_aligned(v) || !page_aligned(n) || v + n > 1u64 << 32 {
            return Err(MapError::InvalidRange);
        }
        if self
            .claims
            .iter()
            .any(|c| overlaps(v, v + n, c.virtual_start as u64, c.end()))
        {
            return Err(MapError::Claimed);
        }
        let id = ClaimId(self.fresh_id());
        self.claims.push(VirtualClaim {
            id,
            virtual_start: va,
            len,
            owner,
        });
        Ok(id)
    }

    /// Mapped virtual windows that reach any byte of physical
    /// `[physical, physical + len)`, each clipped to that physical range.
    pub fn physical_aliases(&self, physical: u32, len: u32) -> Vec<Mapping> {
        let (start, end) = (physical as u64, physical as u64 + len as u64);
        self.mappings
            .iter()
            .filter_map(|m| {
                let p = m.physical_start as u64;
                let lo = start.max(p);
                let hi = end.min(p + m.len as u64);
                (lo < hi).then(|| Mapping {
                    virtual_start: (m.virtual_start as u64 + lo - p) as u32,
                    len: (hi - lo) as u32,
                    physical_start: lo as u32,
                    permissions: m.permissions,
                })
            })
            .collect()
    }

    /// Change the permissions of exactly `[va, va + len)`, splitting mappings
    /// at both edges. Other virtual aliases of the same physical bytes keep
    /// their own permissions; split pieces keep their physical offsets. The
    /// whole range is validated first, so failure changes nothing:
    /// zero/unaligned/overflowing bounds are `InvalidRange`, any unmapped byte
    /// is `Unmapped`, any overlap with an HLE claim is `Claimed`, and
    /// permissions without Execute on a [`Guest::guard_code`] page are
    /// `LiftedCode`.
    pub fn protect_range(
        &mut self,
        va: u32,
        len: u32,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        let (start, end) = self.validate_range(va, len, None)?;
        self.check_code(start, end, Some(permissions))?;
        self.apply_protect(start, end, permissions);
        Ok(())
    }
    /// [`Guest::protect_range`] for the owner of `claim`; the range must lie
    /// wholly inside that claim, otherwise `Claimed`.
    pub fn protect_range_owned(
        &mut self,
        claim: ClaimId,
        va: u32,
        len: u32,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        let (start, end) = self.validate_range(va, len, Some(claim))?;
        self.check_code(start, end, Some(permissions))?;
        self.apply_protect(start, end, permissions);
        Ok(())
    }
    /// Remove exactly `[va, va + len)` from the virtual map, splitting
    /// mappings at both edges. Physical bytes are untouched: another alias
    /// still reads them, and a reservation keeps its pages reserved. Pages no
    /// mapping or reservation references become free for
    /// [`Guest::find_free_physical`] and are zeroed by `map_zeroed` on reuse.
    /// Validation and errors match [`Guest::protect_range`]; unmapping any
    /// guarded code page is `LiftedCode`. Failure changes nothing.
    pub fn unmap(&mut self, va: u32, len: u32) -> Result<(), MapError> {
        let (start, end) = self.validate_range(va, len, None)?;
        self.check_code(start, end, None)?;
        self.apply_unmap(start, end);
        Ok(())
    }
    /// [`Guest::unmap`] for the owner of `claim`; the range must lie wholly
    /// inside that claim, otherwise `Claimed`.
    pub fn unmap_owned(&mut self, claim: ClaimId, va: u32, len: u32) -> Result<(), MapError> {
        let (start, end) = self.validate_range(va, len, Some(claim))?;
        self.check_code(start, end, None)?;
        self.apply_unmap(start, end);
        Ok(())
    }
    fn validate_range(
        &self,
        va: u32,
        len: u32,
        claim: Option<ClaimId>,
    ) -> Result<(u64, u64), MapError> {
        let (start, end) = (va as u64, va as u64 + len as u64);
        if len == 0 || !page_aligned(start) || !page_aligned(len as u64) || end > 1u64 << 32 {
            return Err(MapError::InvalidRange);
        }
        let allowed = match claim {
            None => !self
                .claims
                .iter()
                .any(|c| overlaps(start, end, c.virtual_start as u64, c.end())),
            Some(id) => self
                .claims
                .iter()
                .any(|c| c.id == id && start >= c.virtual_start as u64 && end <= c.end()),
        };
        if !allowed {
            return Err(MapError::Claimed);
        }
        let mut index = self.mapping_index(start).ok_or(MapError::Unmapped)?;
        let mut cursor = start;
        while cursor < end {
            let m = self
                .mappings
                .get(index)
                .filter(|m| {
                    m.virtual_start as u64 <= cursor
                        && cursor < m.virtual_start as u64 + m.len as u64
                })
                .ok_or(MapError::Unmapped)?;
            cursor = m.virtual_start as u64 + m.len as u64;
            index += 1;
        }
        Ok((start, end))
    }
    /// `permissions` None means unmap. Checked after `validate_range`, so
    /// bounds, holes and claims report first.
    fn check_code(
        &self,
        start: u64,
        end: u64,
        permissions: Option<Permissions>,
    ) -> Result<(), MapError> {
        let removes_execute = permissions.is_none_or(|p| !p.allows(Access::Execute));
        if removes_execute && self.touches_code(start, end) {
            return Err(MapError::LiftedCode);
        }
        Ok(())
    }
    /// Split the mapping strictly containing `addr`, if any, into two pieces
    /// with consistent physical offsets.
    fn split_at(&mut self, addr: u64) {
        let Some(i) = self.mapping_index(addr) else {
            return;
        };
        let m = &mut self.mappings[i];
        let offset = addr - m.virtual_start as u64;
        if offset == 0 {
            return;
        }
        let tail = Mapping {
            virtual_start: addr as u32,
            len: m.len - offset as u32,
            physical_start: m.physical_start + offset as u32,
            permissions: m.permissions,
        };
        m.len = offset as u32;
        self.mappings.insert(i + 1, tail);
    }
    fn apply_protect(&mut self, start: u64, end: u64, permissions: Permissions) {
        self.fold_cards();
        self.split_at(start);
        self.split_at(end);
        for m in &mut self.mappings {
            if (start..end).contains(&(m.virtual_start as u64)) {
                m.permissions = permissions;
            }
        }
        self.refresh_fast_range(start, end);
    }
    fn apply_unmap(&mut self, start: u64, end: u64) {
        self.fold_cards();
        self.layout = fresh_layout();
        self.split_at(start);
        self.split_at(end);
        self.mappings
            .retain(|m| !(start..end).contains(&(m.virtual_start as u64)));
        if let Some(pages) = &mut self.fast_pages {
            pages[(start >> 12) as usize..(end >> 12) as usize].fill(FastPage::default());
        }
        #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
        self.refresh_flat(start, end);
        self.refresh_window(start, end);
    }

    /// Zero physical bytes, reporting the change to the write watch.
    fn fill_zero(&mut self, physical: usize, len: usize) {
        self.ram[physical..physical + len].fill(0);
        if self.watch.active() {
            self.watch.note(physical, len);
        }
    }

    /// Built when a generated module requests the optional C fast path
    /// (or a test or bench wants the same table). Rust accesses within one
    /// page use it too ([`read_bytes`](Self::read_bytes),
    /// [`write_bytes`](Self::write_bytes)).
    pub fn prepare_fast_pages(&mut self) {
        if self.fast_pages.is_some() {
            return;
        }
        self.fast_pages = Some(vec![FastPage::default(); 1 << 20].into_boxed_slice());
        self.refresh_fast_range(0, 1u64 << 32);
    }
    // Guest::map permits byte-granular mappings. Only a whole virtual page
    // backed by a whole aligned physical page can use the C copy/dirty-bit
    // protocol. Partial pages and shifted backing remain on checked_start.
    // Every mapping and permission change ends here (or in apply_unmap),
    // so this is where the layout moves on.
    fn refresh_fast_range(&mut self, start: u64, end: u64) {
        self.layout = fresh_layout();
        self.refresh_window(start, end);
        let Some(pages) = &mut self.fast_pages else {
            return;
        };
        let first = (start >> 12) as usize;
        let last = (page_end(end) >> 12) as usize;
        pages[first..last].fill(FastPage::default());
        for m in &self.mappings {
            let lo = page_end(m.virtual_start as u64).max((first as u64) << 12);
            let hi = ((m.virtual_start as u64 + m.len as u64) & !(PAGE_SIZE as u64 - 1))
                .min((last as u64) << 12);
            if lo >= hi {
                continue;
            }
            let physical = m.physical_start as u64 + lo - m.virtual_start as u64;
            if !physical.is_multiple_of(PAGE_SIZE as u64) {
                continue;
            }
            for va in (lo..hi).step_by(PAGE_SIZE as usize) {
                pages[(va >> 12) as usize] = FastPage {
                    physical: (physical + va - lo) as u32,
                    permissions: m.permissions.0 as u32,
                };
            }
        }
        // Armed pages stay armed through a remap or protect.
        if let Some(trap) = &self.trap {
            for page in first..last {
                let still =
                    (trap.armed(page, false) as u32) | ((trap.armed(page, true) as u32) << 1);
                pages[page].permissions &= !still;
            }
        }
        #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
        self.refresh_flat(start, end);
    }

    /// Installs the access trap ([`crate::trap`]), keeping up to `cap` hits
    /// between [`take_trap_hits`](Self::take_trap_hits). Nothing is armed
    /// yet. A no-op when one is installed.
    pub fn install_trap(&mut self, cap: usize) {
        if self.trap.is_none() {
            self.trap = Some(Box::new(crate::trap::AccessTrap::new(cap)));
        }
    }
    /// Disarms every page, restores the fast page table and removes the
    /// trap.
    pub fn remove_trap(&mut self) {
        self.disarm_all();
        self.trap = None;
    }
    pub fn has_trap(&self) -> bool {
        self.trap.is_some()
    }
    /// Arms every mapped page for the kinds asked (and clears the host's
    /// logged-once marks for them): the next access of that kind to each
    /// page is logged. Pages already armed stay armed. Stickiness is kept.
    pub fn arm_all(&mut self, read: bool, write: bool) {
        let Some(trap) = &self.trap else {
            return;
        };
        let clear = (if read { 1 } else { 0 }) | (if write { 2 } else { 0 });
        for m in &self.mappings {
            let first = (m.virtual_start >> 12) as usize;
            let last = ((m.virtual_start as u64 + m.len as u64 - 1) >> 12) as usize;
            for page in first..=last {
                trap.arm(page, read, write);
                if let Some(pages) = &mut self.fast_pages {
                    pages[page].permissions &= !clear;
                }
            }
        }
        // Armed pages are guards in the window.
        self.refresh_window(0, 1u64 << 32);
    }
    /// Makes `pages` (virtual page numbers) sticky, and every other page
    /// not: armed for both kinds, every access to them is logged.
    pub fn set_sticky_pages(&mut self, pages: &[u32]) {
        let Some(trap) = &self.trap else {
            return;
        };
        for p in trap.armed_pages() {
            trap.set_sticky(p, false);
        }
        for &p in pages {
            let p = p as usize & ((1 << 20) - 1);
            trap.arm(p, true, true);
            trap.set_sticky(p, true);
            if let Some(fast) = &mut self.fast_pages {
                fast[p].permissions &= !3;
            }
        }
        for &p in pages {
            let va = (p as u64 & ((1 << 20) - 1)) << 12;
            self.refresh_window(va, va + PAGE_SIZE as u64);
        }
    }
    /// Arm read-only watches of selected virtual pages. Read masks cover
    /// 64-byte lines, so this tracks ranges rather than individual fields.
    pub fn watch_read_lines(&mut self, pages: impl IntoIterator<Item = u32>) {
        self.install_trap(4096);
        let Some(trap) = &self.trap else { return };
        let mut armed = Vec::new();
        for page in pages.into_iter().filter(|p| *p < (1 << 20)) {
            let page = page as usize;
            trap.arm(page, true, false);
            trap.set_sticky(page, true);
            if let Some(fast) = &mut self.fast_pages {
                fast[page].permissions &= !1;
            }
            armed.push(page);
        }
        for page in armed {
            let va = (page as u64) << 12;
            self.refresh_window(va, va + PAGE_SIZE as u64);
        }
    }

    /// Disarms every page and restores its fast page table entry.
    pub fn disarm_all(&mut self) {
        let Some(trap) = &self.trap else {
            return;
        };
        let pages = trap.armed_pages();
        for &p in &pages {
            trap.disarm(p);
        }
        for p in pages {
            self.open_fast_page(p);
        }
    }
    /// What the trap logged since the last take.
    pub fn take_trap_hits(&mut self) -> crate::trap::TrapLog {
        self.trap.as_ref().map(|t| t.take()).unwrap_or_default()
    }
    /// Watch the 32-bit words at `words` for writes: their pages are armed
    /// for good (sticky), and every write through the checked path that
    /// overlaps one of them (generated code, the interpreter, HLE and Rust
    /// through this Guest; not raw physical writers) is printed at once as
    /// `trap: focus <word> = <value> written by pc <pc> ...` and kept in
    /// [`focus_hits`](Self::focus_hits). Installs the trap if none is.
    pub fn set_trap_focus(&mut self, words: &[u32]) {
        self.install_trap(1024);
        let Some(trap) = &self.trap else {
            return;
        };
        trap.set_focus(words);
        for &w in words {
            for page in [w >> 12, (w + 3) >> 12] {
                let p = page as usize;
                trap.arm(p, true, true);
                trap.set_sticky(p, true);
                if let Some(fast) = &mut self.fast_pages {
                    fast[p].permissions &= !3;
                }
            }
        }
        for &w in words {
            for page in [w >> 12, w.wrapping_add(3) >> 12] {
                let va = (page as u64) << 12;
                self.refresh_window(va, va + PAGE_SIZE as u64);
            }
        }
    }
    /// The focus writes so far: (word, value written, the access).
    pub fn focus_hits(&self) -> Vec<(u32, u32, crate::trap::TrapHit)> {
        self.trap
            .as_ref()
            .map(|t| t.focus_hits())
            .unwrap_or_default()
    }
    /// A guest (`pc` non-zero) or host access the slow path allowed.
    fn trap_note(&mut self, pc: u32, addr: u32, len: usize, write: bool) {
        let Some(trap) = &self.trap else {
            return;
        };
        if write {
            for word in trap.focus_overlaps(addr, len) {
                let value = self
                    .fold_address(word)
                    .map(|p| (p & !crate::watch::CONTIGUOUS_BASE) as usize)
                    .and_then(|p| self.ram.get(p..p + 4))
                    .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                trap.focus_hit(word, value, pc, addr, len);
            }
        }
        let open = trap.note(pc, addr, len, write);
        if open.is_empty() {
            return;
        }
        for &p in open.pages() {
            self.open_fast_page(p);
        }
    }
    /// Rebuilds virtual page `page`'s fast entry from its live mapping, as
    /// [`refresh_fast_range`](Self::refresh_fast_range) would, without the
    /// kinds the trap still has armed there.
    fn open_fast_page(&mut self, page: usize) {
        let va = (page as u64) << 12;
        let entry = self
            .mapping_index(va)
            .map(|i| &self.mappings[i])
            .filter(|m| va + PAGE_SIZE as u64 <= m.virtual_start as u64 + m.len as u64)
            .map(|m| {
                (
                    m.physical_start as u64 + va - m.virtual_start as u64,
                    m.permissions,
                )
            })
            .filter(|(physical, _)| physical.is_multiple_of(PAGE_SIZE as u64))
            .map_or(FastPage::default(), |(physical, p)| FastPage {
                physical: physical as u32,
                permissions: p.0 as u32,
            });
        let mut still = 0;
        if let Some(t) = &self.trap {
            if t.armed(page, false) {
                still |= 1;
            }
            if t.armed(page, true) {
                still |= 2;
            }
        }
        if let Some(pages) = &mut self.fast_pages {
            // Disarming a census trap changes only the permission bits.
            let flags = pages[page].permissions & !7;
            pages[page] = FastPage {
                physical: entry.physical,
                permissions: flags | (entry.permissions & !still),
            };
        }
        self.refresh_window(va, va + PAGE_SIZE as u64);
    }

    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    fn refresh_flat(&mut self, start: u64, end: u64) {
        let Some(pages) = &mut self.fast_pages else {
            return;
        };
        if self
            .flat
            .as_mut()
            .is_some_and(|f| !f.refresh(&self.ram, pages, start, end))
        {
            self.flat = None;
        }
    }

    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    pub(crate) fn flat_view(&mut self) -> Option<crate::flat::View> {
        let pages = self.fast_pages.as_mut()?;
        if self.trap.is_some() {
            return None;
        }
        if self.flat_requested {
            self.flat_requested = false;
            self.flat = crate::flat::Arena::new(&self.ram, pages);
        }
        let arena = self.flat.as_mut()?;
        arena.rearm(&self.watch);
        Some(arena.view(&self.ram, &self.watch))
    }

    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    pub(crate) fn publish_flat(&mut self) {
        if let Some(arena) = &mut self.flat {
            arena.publish(&mut self.watch);
        }
    }

    /// Show this Guest in `window` from now on ([`crate::window`]): the
    /// runtime memory calls it for a new Guest and for one put in place of
    /// another. Rebuilds the whole window and forgets written cards.
    pub(crate) fn attach_window(&mut self, window: std::sync::Arc<crate::window::Window>) {
        window.claim(self.ident, self.congruence_skipped);
        window.clear_cards();
        self.window = Some(window);
        self.refresh_window(0, 1u64 << 32);
    }
    /// `window` shows this Guest.
    pub(crate) fn shown_in(&self, window: &std::sync::Arc<crate::window::Window>) -> bool {
        self.window
            .as_ref()
            .is_some_and(|w| std::sync::Arc::ptr_eq(w, window))
            && window.owner() == self.ident
    }
    /// The window, while it shows this Guest.
    fn owned_window(&self) -> Option<&std::sync::Arc<crate::window::Window>> {
        self.window.as_ref().filter(|w| w.owner() == self.ident)
    }
    /// Rebuild the window's view of `[start, end)` (rounded out to whole
    /// host pages) from the mappings and the trap. Every mapping, permission
    /// and trap change ends here.
    fn refresh_window(&self, start: u64, end: u64) {
        let Some(w) = self.owned_window() else {
            return;
        };
        let k = w.pages_per_host();
        let first = (start >> 12) as usize / k * k;
        let last = ((page_end(end) >> 12) as usize).div_ceil(k) * k;
        let last = last.min(1 << 20);
        // At most 64 Ki pages (512 KiB of slots) at a time.
        const CHUNK: usize = 1 << 16;
        let mut at = first;
        while at < last {
            let to = (at + CHUNK).min(last);
            let slots = self.window_slots(at, to, w.host_page() as u64);
            w.refresh(&self.ram, self.aliases_ram(), at, &slots);
            at = to;
        }
    }
    /// What guest pages `[first, last)` are to the window.
    fn window_slots(&self, first: usize, last: usize, host: u64) -> Vec<crate::window::Slot> {
        use crate::window::Slot;
        let mut slots = vec![Slot::Unmapped; last - first];
        let (lo, hi) = ((first as u64) << 12, (last as u64) << 12);
        // Mappings are sorted and disjoint, so their ends are sorted too.
        let from = self
            .mappings
            .partition_point(|m| m.virtual_start as u64 + m.len as u64 <= lo);
        for m in &self.mappings[from..] {
            let (start, end) = (m.virtual_start as u64, m.virtual_start as u64 + m.len as u64);
            if start >= hi {
                break;
            }
            let readable = m.permissions.allows(Access::Read);
            for vpn in (start.max(lo) >> 12)..(page_end(end).min(hi) >> 12) {
                let va = vpn << 12;
                let i = vpn as usize - first;
                let whole = start <= va && va + PAGE_SIZE as u64 <= end;
                if !whole {
                    // A byte-granular piece: the helper path only.
                    if readable {
                        slots[i] = Slot::Out;
                    }
                    continue;
                }
                let physical = m.physical_start as u64 + va - start;
                slots[i] = if !readable {
                    Slot::Unmapped
                } else if !physical.is_multiple_of(PAGE_SIZE as u64)
                    || physical % host != va % host
                {
                    Slot::Out
                } else {
                    Slot::In {
                        physical: physical as u32,
                        write: m.permissions.allows(Access::Write),
                    }
                };
            }
        }
        if let Some(trap) = &self.trap {
            for (i, slot) in slots.iter_mut().enumerate() {
                let page = first + i;
                if *slot != Slot::Unmapped && (trap.armed(page, false) || trap.armed(page, true)) {
                    *slot = Slot::Out;
                }
            }
        }
        slots
    }
    /// The Guest's RAM was replaced under unchanged mappings: retire the
    /// window's aliases of the old RAM and alias the new, if it is to be.
    pub(crate) fn window_ram_replaced(&self) {
        if self.owned_window().is_some_and(|w| w.is_aliased() || self.aliases_ram()) {
            self.refresh_window(0, 1u64 << 32);
        }
    }
    /// The window is to alias this Guest's RAM, where the backing can be:
    /// a flat-RAM runtime's Guest ([`Guest::require_aliasable_ram`]), or
    /// RAM `H2_FLAT_RAM=1` made aliasable from the start. A paged runtime's
    /// view or file restore, which macOS could alias, keeps the window's
    /// bookkeeping only.
    fn aliases_ram(&self) -> bool {
        self.aliasable_ram || self.ram.can_alias()
    }
    /// Turn the window's written cards into write-watch notes: the
    /// physical page of each written guest page and of the page after it
    /// (a store may cross). Runs before anything reads or changes watch
    /// state and before a mapping change, so cards are translated under
    /// the mappings they were written under. With no active watch the
    /// cards are only cleaned.
    pub(crate) fn fold_cards(&mut self) {
        let Some(w) = self.owned_window().filter(|w| w.has_cards()).cloned() else {
            return;
        };
        if !self.watch.active() {
            w.clear_cards();
            return;
        }
        w.take_cards(|vpn| {
            self.note_virtual(vpn as u64);
            self.note_virtual(vpn as u64 + 1);
        });
    }
    /// Note the physical bytes behind guest page `vpn`, whichever
    /// mappings cover them.
    fn note_virtual(&mut self, vpn: u64) {
        let (lo, hi) = (vpn << 12, (vpn + 1) << 12);
        if hi > 1u64 << 32 {
            return;
        }
        let from = self
            .mappings
            .partition_point(|m| m.virtual_start as u64 + m.len as u64 <= lo);
        for m in &self.mappings[from..] {
            let (start, end) = (m.virtual_start as u64, m.virtual_start as u64 + m.len as u64);
            if start >= hi {
                break;
            }
            let (a, b) = (start.max(lo), end.min(hi));
            let physical = (m.physical_start as u64 + a - start) as usize;
            self.watch.note(physical, (b - a) as usize);
        }
    }
    /// Whether a written card reaches a physical page `watched` says
    /// (by physical page number), leaving the cards.
    pub(crate) fn cards_reach(&self, watched: impl Fn(usize) -> bool) -> bool {
        let Some(w) = self.owned_window() else {
            return false;
        };
        if !self.watch.active() {
            return false;
        }
        w.any_card(|vpn| {
            [vpn as u64, vpn as u64 + 1].into_iter().any(|vpn| {
                let (lo, hi) = (vpn << 12, (vpn + 1) << 12);
                if hi > 1u64 << 32 {
                    return false;
                }
                let from = self
                    .mappings
                    .partition_point(|m| m.virtual_start as u64 + m.len as u64 <= lo);
                self.mappings[from..]
                    .iter()
                    .take_while(|m| (m.virtual_start as u64) < hi)
                    .any(|m| {
                        let (start, end) =
                            (m.virtual_start as u64, m.virtual_start as u64 + m.len as u64);
                        let (a, b) = (start.max(lo), end.min(hi));
                        let p = m.physical_start as u64 + a - start;
                        let q = p + (b - a) - 1;
                        ((p >> 12)..=(q >> 12)).any(|page| watched(page as usize))
                    })
            })
        })
    }
    /// This Guest's window, for a test.
    #[cfg(test)]
    pub(crate) fn window(&self) -> Option<&std::sync::Arc<crate::window::Window>> {
        self.owned_window()
    }
    /// The runtime runs a flat-RAM module (`generated::FEATURE_FLAT_RAM`):
    /// keep RAM in a backing the window can alias
    /// ([`crate::cow::Ram::can_share`]) and alias it, through a
    /// copy-on-write restore ([`Guest::restore_cow`]) and a view's refresh
    /// ([`Guest::cow_refresh`]) too
    /// (docs/runtime/contract-thin-transfers-flat-ram.md, slice F5). RAM
    /// that cannot be aliased (made before the module registered, a Linux
    /// file restore, a heap copy) is copied over, its bytes and every
    /// mapping unchanged.
    pub(crate) fn require_aliasable_ram(&mut self) {
        let aliased = self.aliases_ram();
        self.aliasable_ram = true;
        if self.ensure_aliasable_ram() {
            self.refresh_fast_range(0, 1u64 << 32);
        } else if !aliased {
            // Already shareable (a macOS snapshot or file restore), but
            // shown unaliased until now.
            self.refresh_window(0, 1u64 << 32);
        }
    }
    /// Copy RAM into an aliasable backing if this Guest needs one and does
    /// not have it; true if RAM moved. The caller then rebuilds the page
    /// table and the window from the new RAM.
    pub(crate) fn ensure_aliasable_ram(&mut self) -> bool {
        #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
        {
            if !self.aliasable_ram || self.ram.can_share() {
                return false;
            }
            self.fold_cards();
            if !self.ram.make_aliasable() {
                return false;
            }
            // The span model's aliases were of the old RAM. A flat-RAM
            // module never reads them.
            self.flat = None;
            self.flat_requested = false;
            self.watch_revision = self.watch_revision.wrapping_add(1);
            true
        }
        #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64"))))]
        false
    }
    /// RAM is the copy-on-write file mapping of the last
    /// [`Guest::restore_cow`], not a copy of the image, for a test.
    #[cfg(test)]
    pub(crate) fn ram_maps_file(&self) -> bool {
        self.ram.maps_file()
    }
    /// A Guest whose RAM the window can alias, for a test.
    #[cfg(test)]
    pub(crate) fn new_aliasable() -> Self {
        let mut g = Self::new();
        g.ram = crate::cow::Ram::zeroed_with(RAM_SIZE, true);
        g
    }

    pub(crate) fn fast_parts(&mut self) -> (*mut u8, *const FastPage, bool) {
        self.invalidate_hle_capture();
        (
            self.ram.as_mut_ptr(),
            self.fast_pages
                .as_ref()
                .map_or(std::ptr::null(), |p| p.as_ptr()),
            self.watch.active(),
        )
    }
    pub(crate) fn note_fast_page(&mut self, page: usize) {
        if self.watch.active() {
            self.watch
                .note(page * PAGE_SIZE as usize, PAGE_SIZE as usize);
        }
    }

    pub(crate) fn mapping_index(&self, addr: u64) -> Option<usize> {
        let hint = &self.mapping_hints[((addr >> 12) & 7) as usize];
        let cached = hint.load(Ordering::Relaxed);
        if let Some(m) = self.mappings.get(cached)
            && m.virtual_start as u64 <= addr
            && addr < m.virtual_start as u64 + m.len as u64
        {
            return Some(cached);
        }
        let i = self
            .mappings
            .partition_point(|m| m.virtual_start as u64 <= addr)
            .checked_sub(1)?;
        let m = &self.mappings[i];
        if addr >= m.virtual_start as u64 + m.len as u64 {
            return None;
        }
        // Mapping edits may move indices; a stale hint simply misses the
        // range test above. Permissions and physical offsets are read from
        // the live mapping by the caller, including after protect/remap.
        hint.store(i, Ordering::Relaxed);
        Some(i)
    }

    fn span(&self, index: usize, addr: u64, end: u64) -> std::ops::Range<usize> {
        let m = &self.mappings[index];
        let start = (m.physical_start as u64 + addr - m.virtual_start as u64) as usize;
        start..start + (end.min(m.virtual_start as u64 + m.len as u64) - addr) as usize
    }

    /// Validate once, then copy without allocating. Adjacent mappings use a
    /// second linear walk; the usual one-mapping access needs one hinted lookup.
    fn checked_start(
        &self,
        addr: u32,
        len: usize,
        access: Access,
        alignment: u32,
    ) -> Result<Option<usize>, GuestFault> {
        let fault = |reason| GuestFault {
            addr,
            len,
            access,
            reason,
        };
        let end = (addr as u64)
            .checked_add(len as u64)
            .filter(|e| *e <= 1u64 << 32)
            .ok_or_else(|| fault(FaultReason::AddressOverflow))?;
        if len != 0 && (addr as u64) < MMIO_END && end > MMIO_START {
            return Err(fault(FaultReason::UnsupportedDevice));
        }
        if !alignment.is_power_of_two() || !addr.is_multiple_of(alignment) {
            return Err(fault(FaultReason::Misaligned));
        }
        if len == 0 {
            return Ok(None);
        }
        let first = self
            .mapping_index(addr as u64)
            .ok_or_else(|| fault(FaultReason::Unmapped))?;
        let mut index = first;
        let mut cursor = addr as u64;
        while cursor < end {
            let m = self
                .mappings
                .get(index)
                .filter(|m| {
                    m.virtual_start as u64 <= cursor
                        && cursor < m.virtual_start as u64 + m.len as u64
                })
                .ok_or_else(|| fault(FaultReason::Unmapped))?;
            if !m.permissions.allows(access) {
                return Err(fault(FaultReason::Protection));
            }
            cursor = end.min(m.virtual_start as u64 + m.len as u64);
            index += 1;
        }
        Ok(Some(first))
    }
    pub fn check(
        &self,
        addr: u32,
        len: usize,
        access: Access,
        alignment: u32,
    ) -> Result<(), GuestFault> {
        self.checked_start(addr, len, access, alignment).map(|_| ())
    }
    /// The physical offset of `[addr, addr + len)` when it lies in one
    /// whole page of the fast page table that allows `need` (permission
    /// bits), is not device memory and meets `alignment`: the access
    /// [`checked_start`](Self::checked_start) would accept, found with one
    /// table lookup. `None` sends the access the long way, which also
    /// reports any fault.
    #[inline]
    fn fast_offset(&self, addr: u32, len: usize, need: u32, alignment: u32) -> Option<usize> {
        let pages = self.fast_pages.as_deref()?;
        let off = (addr & (PAGE_SIZE - 1)) as usize;
        if len == 0
            || off + len > PAGE_SIZE as usize
            || !alignment.is_power_of_two()
            || !addr.is_multiple_of(alignment)
            || ((addr as u64) < MMIO_END && addr as u64 + len as u64 > MMIO_START)
        {
            return None;
        }
        let p = pages[(addr >> 12) as usize];
        (p.permissions & need != 0).then_some(p.physical as usize + off)
    }

    /// Inlined for the fast page table's one-lookup case, so a fixed-size
    /// read (`u32`, `f32`, a host struct) becomes a load at the call site;
    /// everything else takes [`read_bytes_slow`](Self::read_bytes_slow).
    #[inline]
    pub fn read_bytes(&self, addr: u32, out: &mut [u8], alignment: u32) -> Result<(), GuestFault> {
        if let Some(at) = self.fast_offset(addr, out.len(), 1, alignment) {
            out.copy_from_slice(&self.ram[at..at + out.len()]);
            return Ok(());
        }
        self.read_bytes_slow(addr, out, alignment)
    }
    /// [`read_bytes`](Self::read_bytes) for guest code at `pc` (the ABI
    /// helpers): the same access, attributed to `pc` by the access trap.
    #[inline]
    pub fn read_bytes_from(
        &mut self,
        pc: u32,
        addr: u32,
        out: &mut [u8],
        alignment: u32,
    ) -> Result<(), GuestFault> {
        if let Some(at) = self.fast_offset(addr, out.len(), 1, alignment) {
            out.copy_from_slice(&self.ram[at..at + out.len()]);
            return Ok(());
        }
        if self.trap.is_some() {
            return self.read_bytes_trapped(pc, addr, out, alignment);
        }
        self.read_bytes_slow(addr, out, alignment)
    }
    #[inline(never)]
    fn read_bytes_trapped(
        &mut self,
        pc: u32,
        addr: u32,
        out: &mut [u8],
        alignment: u32,
    ) -> Result<(), GuestFault> {
        self.read_bytes_unnoted(addr, out, alignment)?;
        if !out.is_empty() {
            self.trap_note(pc, addr, out.len(), false);
        }
        Ok(())
    }
    #[inline(never)]
    fn read_bytes_slow(&self, addr: u32, out: &mut [u8], alignment: u32) -> Result<(), GuestFault> {
        self.read_bytes_unnoted(addr, out, alignment)?;
        if let Some(t) = &self.trap
            && !out.is_empty()
        {
            // The host never disarms a page, so nothing to open.
            t.note(0, addr, out.len(), false);
        }
        Ok(())
    }
    fn read_bytes_unnoted(
        &self,
        addr: u32,
        out: &mut [u8],
        alignment: u32,
    ) -> Result<(), GuestFault> {
        let Some(mut index) = self.checked_start(addr, out.len(), Access::Read, alignment)? else {
            return Ok(());
        };
        let mut offset = 0;
        while offset < out.len() {
            let span = self.span(
                index,
                addr as u64 + offset as u64,
                addr as u64 + out.len() as u64,
            );
            let n = span.len();
            out[offset..offset + n].copy_from_slice(&self.ram[span]);
            offset += n;
            index += 1;
        }
        Ok(())
    }
    /// [`read_bytes`](Self::read_bytes) of `len` bytes appended to `out`,
    /// without zeroing them first. `out` is unchanged on a fault.
    pub fn read_append(
        &self,
        addr: u32,
        len: usize,
        out: &mut Vec<u8>,
        alignment: u32,
    ) -> Result<(), GuestFault> {
        if let Some(at) = self.fast_offset(addr, len, 1, alignment) {
            out.extend_from_slice(&self.ram[at..at + len]);
            return Ok(());
        }
        let Some(mut index) = self.checked_start(addr, len, Access::Read, alignment)? else {
            return Ok(());
        };
        out.reserve(len);
        let (mut cursor, end) = (addr as u64, addr as u64 + len as u64);
        while cursor < end {
            let span = self.span(index, cursor, end);
            cursor += span.len() as u64;
            out.extend_from_slice(&self.ram[span]);
            index += 1;
        }
        if let Some(t) = &self.trap {
            t.note(0, addr, len, false);
        }
        Ok(())
    }
    /// Capture the virtual writes and zero-initialised allocations of one
    /// synchronous ABI call. Physical reservation changes, other mapping
    /// operations, direct RAM replacements and nested capture cannot be
    /// reproduced as ABI outputs and return `None`, as does exceeding the
    /// byte/record budget. The call itself always runs normally.
    pub fn capture_hle_writes<T>(
        &mut self,
        byte_limit: usize,
        f: impl FnOnce(&mut Self) -> T,
    ) -> (T, Option<HleOutputs>) {
        let fixed = (self.reservations.len(), self.code.len());
        let mut previous = self.hle_writes.replace(HleWrites {
            remaining: byte_limit,
            outputs: HleOutputs::default(),
            expected_layout: self.layout,
            incomplete: false,
        });
        if let Some(outer) = &mut previous {
            outer.incomplete = true;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        let captured = std::mem::replace(&mut self.hle_writes, previous);
        let value = result.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        let outputs = captured
            .filter(|w| {
                !w.incomplete
                    && w.expected_layout == self.layout
                    && fixed == (self.reservations.len(), self.code.len())
            })
            .map(|w| w.outputs);
        (value, outputs)
    }

    fn invalidate_hle_capture(&mut self) {
        if let Some(w) = &mut self.hle_writes {
            w.incomplete = true;
        }
    }

    /// No bytes change unless every mapping in the access is writable.
    pub fn write_bytes(
        &mut self,
        addr: u32,
        value: &[u8],
        alignment: u32,
    ) -> Result<(), GuestFault> {
        self.write_bytes_from(0, addr, value, alignment)
    }
    /// [`write_bytes`](Self::write_bytes) for guest code at `pc` (the ABI
    /// helpers; 0 is the host): attributed to `pc` by the access trap.
    #[inline]
    pub fn write_bytes_from(
        &mut self,
        pc: u32,
        addr: u32,
        value: &[u8],
        alignment: u32,
    ) -> Result<(), GuestFault> {
        if let Some(at) = self.fast_offset(addr, value.len(), 2, alignment) {
            self.ram[at..at + value.len()].copy_from_slice(value);
            // The write barrier, as below.
            if self.watch.active() {
                self.watch.note(at, value.len());
            }
            if let Some(w) = &mut self.hle_writes {
                w.store(addr, value);
            }
            return Ok(());
        }
        let Some(mut index) = self.checked_start(addr, value.len(), Access::Write, alignment)?
        else {
            return Ok(());
        };
        let mut offset = 0;
        while offset < value.len() {
            let span = self.span(
                index,
                addr as u64 + offset as u64,
                addr as u64 + value.len() as u64,
            );
            let (start, n) = (span.start, span.len());
            self.ram[span].copy_from_slice(&value[offset..offset + n]);
            // Every RAM store passes here, so this is the whole write barrier.
            if self.watch.active() {
                self.watch.note(start, n);
            }
            offset += n;
            index += 1;
        }
        if let Some(w) = &mut self.hle_writes {
            w.store(addr, value);
        }
        if self.trap.is_some() && !value.is_empty() {
            self.trap_note(pc, addr, value.len(), true);
        }
        Ok(())
    }

    /// Stack helpers operate on an already borrowed memory guard. They never
    /// acquire another lock, so HLE can use them inside a compound operation.
    pub fn call(
        &mut self,
        cpu: &mut crate::CpuAbi,
        target: u32,
        resume: u32,
    ) -> Result<crate::TransferAbi, GuestFault> {
        let esp = cpu.esp.wrapping_sub(4);
        self.set_u32(esp, resume)?;
        cpu.esp = esp;
        Ok(crate::TransferAbi::call(target, resume))
    }
    pub fn ret(
        &self,
        cpu: &mut crate::CpuAbi,
        cleanup: u32,
    ) -> Result<crate::TransferAbi, GuestFault> {
        let target = self.u32(cpu.esp)?;
        cpu.esp = cpu.esp.wrapping_add(4).wrapping_add(cleanup);
        Ok(crate::TransferAbi::ret(target))
    }
    #[inline]
    pub fn read<const N: usize>(&self, addr: u32) -> Result<[u8; N], GuestFault> {
        let mut out = [0; N];
        self.read_bytes(addr, &mut out, 1)?;
        Ok(out)
    }
    pub fn write<const N: usize>(&mut self, addr: u32, v: [u8; N]) -> Result<(), GuestFault> {
        self.write_bytes(addr, &v, 1)
    }
    #[inline]
    pub fn u8(&self, addr: u32) -> Result<u8, GuestFault> {
        Ok(self.read::<1>(addr)?[0])
    }
    pub fn set_u8(&mut self, addr: u32, v: u8) -> Result<(), GuestFault> {
        self.write(addr, [v])
    }
    pub fn u80(&self, addr: u32) -> Result<[u8; 10], GuestFault> {
        self.read(addr)
    }
    pub fn set_u80(&mut self, addr: u32, v: [u8; 10]) -> Result<(), GuestFault> {
        self.write(addr, v)
    }
}
macro_rules! scalar {
    ($read:ident, $write:ident, $ty:ty) => {
        impl Guest {
            #[inline]
            pub fn $read(&self, addr: u32) -> Result<$ty, GuestFault> {
                self.read(addr).map(<$ty>::from_le_bytes)
            }
            pub fn $write(&mut self, addr: u32, v: $ty) -> Result<(), GuestFault> {
                self.write(addr, v.to_le_bytes())
            }
        }
    };
}
scalar!(u16, set_u16, u16);
scalar!(u32, set_u32, u32);
scalar!(u64, set_u64, u64);
scalar!(u128, set_u128, u128);
scalar!(f32, set_f32, f32);
scalar!(f64, set_f64, f64);
