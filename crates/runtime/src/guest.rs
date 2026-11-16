//! Checked virtual mappings over one 64 MB physical store. Physical offsets
//! identify aliases; high virtual addresses are never masked into RAM.

use std::sync::atomic::{AtomicUsize, Ordering};

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
    ram: Box<[u8]>,
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

impl Default for Guest {
    fn default() -> Self {
        Self::new()
    }
}
impl Guest {
    /// RAM starts zeroed and entirely unmapped, including guest address zero.
    pub fn new() -> Self {
        Self {
            ram: vec![0; RAM_SIZE].into_boxed_slice(),
            mappings: Vec::new(),
            mapping_hints: std::array::from_fn(|_| AtomicUsize::new(usize::MAX)),
            fast_pages: None,
            next_physical: 0,
            reservations: Vec::new(),
            claims: Vec::new(),
            code: Vec::new(),
            next_id: 1,
            watch: Default::default(),
        }
    }
    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
    }
    /// All of physical RAM, read-only, for whole-machine observers such as
    /// the lockstep per-tick hash (docs/high-framerate.md). Reading it is
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

    /// Allocate distinct page-aligned physical storage for a virtual range.
    /// Storage comes from never-used RAM above every earlier mapping, skipping
    /// physical reservations. When that runs out, the lowest free page-aligned
    /// gap left by [`Guest::unmap`] is reused and zeroed first.
    pub fn map_zeroed(
        &mut self,
        va: u32,
        len: u32,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        self.check_virtual(va, len)?;
        if let Some(physical) = self.fresh_physical(len) {
            return self.map(va, len, physical, permissions);
        }
        let physical = self
            .find_free_physical(len, 0..RAM_SIZE as u32, PAGE_SIZE)
            .ok_or(MapError::OutOfMemory)?;
        self.map(va, len, physical, permissions)?;
        self.fill_zero(physical as usize, len as usize);
        Ok(())
    }
    fn fresh_physical(&self, len: u32) -> Option<u32> {
        let mut start = page_end(self.next_physical as u64);
        loop {
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
                None => return Some(start as u32),
            }
        }
    }
    pub fn protect(&mut self, va: u32, permissions: Permissions) -> Result<(), MapError> {
        let m = self
            .mappings
            .iter_mut()
            .find(|m| m.virtual_start == va)
            .ok_or(MapError::InvalidRange)?;
        m.permissions = permissions;
        let end = m.virtual_start as u64 + m.len as u64;
        self.refresh_fast_range(va as u64, end);
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
        self.split_at(start);
        self.split_at(end);
        self.mappings
            .retain(|m| !(start..end).contains(&(m.virtual_start as u64)));
        if let Some(pages) = &mut self.fast_pages {
            pages[(start >> 12) as usize..(end >> 12) as usize].fill(FastPage::default());
        }
    }

    /// Zero physical bytes, reporting the change to the write watch.
    fn fill_zero(&mut self, physical: usize, len: usize) {
        self.ram[physical..physical + len].fill(0);
        if self.watch.active() {
            self.watch.note(physical, len);
        }
    }

    /// Built only when a generated module requests the optional C fast path.
    pub(crate) fn prepare_fast_pages(&mut self) {
        if self.fast_pages.is_some() {
            return;
        }
        self.fast_pages = Some(vec![FastPage::default(); 1 << 20].into_boxed_slice());
        self.refresh_fast_range(0, 1u64 << 32);
    }
    // Guest::map permits byte-granular mappings. Only a whole virtual page
    // backed by a whole aligned physical page can use the C copy/dirty-bit
    // protocol. Partial pages and shifted backing remain on checked_start.
    fn refresh_fast_range(&mut self, start: u64, end: u64) {
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
    }

    pub(crate) fn fast_parts(&mut self) -> (*mut u8, *const FastPage, bool) {
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
    pub fn read_bytes(&self, addr: u32, out: &mut [u8], alignment: u32) -> Result<(), GuestFault> {
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
    /// No bytes change unless every mapping in the access is writable.
    pub fn write_bytes(
        &mut self,
        addr: u32,
        value: &[u8],
        alignment: u32,
    ) -> Result<(), GuestFault> {
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
    pub fn read<const N: usize>(&self, addr: u32) -> Result<[u8; N], GuestFault> {
        let mut out = [0; N];
        self.read_bytes(addr, &mut out, 1)?;
        Ok(out)
    }
    pub fn write<const N: usize>(&mut self, addr: u32, v: [u8; N]) -> Result<(), GuestFault> {
        self.write_bytes(addr, &v, 1)
    }
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
