//! Physical availability comes from Runtime (`Guest::find_free_physical`),
//! which counts every mapping and physical reservation, so image pages, DATA
//! exports, stacks, aliases and reserved arenas are never handed out twice.
//! Contiguous requests bounded by a reservation (Halo 2's title arena) are
//! mapped onto it with `Guest::map_reserved`.
use crate::ord;
use crate::{
    Action,
    abi::{Args, Value},
    status::{self, NtStatus},
};
use runtime::{
    Guest, GuestFault,
    guest::{Access, MapError, PAGE_SIZE, Permissions, RAM_SIZE, ReservationId},
};
use std::collections::{BTreeMap, BTreeSet};

/// Guest virtual range reserved for HLE-owned memory (first user: the Bink
/// movie arena). No kernel allocator places anything here, and fixed
/// NtAllocateVirtualMemory requests into it fail with
/// STATUS_CONFLICTING_ADDRESSES. Kernel internals use 0xa1000000..0xb0000000.
/// Contiguous memory is mapped at `0x80000000 + physical`, as on retail.
pub const CONTIGUOUS_ALIAS: u32 = runtime::loader::CONTIGUOUS_ALIAS_BASE;
pub const HLE_RESERVED_START: u32 = 0xa000_0000;
pub const HLE_RESERVED_END: u32 = 0xa100_0000;
pub const HLE_RESERVED: std::ops::Range<u32> = HLE_RESERVED_START..HLE_RESERVED_END;

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_DECOMMIT: u32 = 0x4000;
pub const MEM_RELEASE: u32 = 0x8000;
pub const MEM_FREE: u32 = 0x10000;
pub const MEM_PRIVATE: u32 = 0x20000;
pub const MEM_TOP_DOWN: u32 = 0x100000;
pub const MEM_NOZERO: u32 = 0x800000;
pub const PAGE_NOCACHE: u32 = 0x200;
pub const PAGE_WRITECOMBINE: u32 = 0x400;
pub const PAGE_NOACCESS: u32 = 1;
pub const PAGE_READONLY: u32 = 2;
pub const PAGE_READWRITE: u32 = 4;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;

#[derive(Debug, Clone)]
pub struct Region {
    pub base: u32,
    pub size: u32,
    pub allocation_protect: u32,
    pages: BTreeMap<u32, u32>,
}
#[derive(Debug, Clone)]
pub struct PoolBlock {
    pub size: u32,
    pub capacity: u32,
    pub tag: u32,
    pub live: bool,
}
/// A live stack from MmCreateKernelStack, by its lowest mapped address.
#[derive(Debug, Clone, Copy)]
pub struct KernelStack {
    pub size: u32,
}
#[derive(Debug, Default, Clone)]
pub struct Memory {
    pub regions: BTreeMap<u32, Region>,
    pub pool: BTreeMap<u32, PoolBlock>,
    pub contiguous: BTreeMap<u32, (u32, u32)>,
    /// MmSetAddressProtect values for contiguous pages, which override the
    /// allocation's protection in MmQueryAddressProtect.
    pub contiguous_pages: BTreeMap<u32, u32>,
    pub kernel_stacks: BTreeMap<u32, KernelStack>,
    guards: BTreeSet<u32>,
}

/// Clear page-aligned guest RAM using fixed host scratch, whatever its size.
fn zero(memory: &mut Guest, base: u32, size: u32) -> Result<(), GuestFault> {
    let page = [0u8; PAGE_SIZE as usize];
    let mut offset = 0;
    while offset < size {
        let count = (size - offset).min(PAGE_SIZE);
        let at = base
            .checked_add(offset)
            .ok_or_else(|| crate::strings::overflow(base))?;
        memory.write_bytes(at, &page[..count as usize], 1)?;
        offset += count;
    }
    Ok(())
}
pub(crate) fn round_up(v: u32, alignment: u32) -> Option<u32> {
    if !alignment.is_power_of_two() {
        return None;
    }
    v.checked_add(alignment - 1).map(|x| x & !(alignment - 1))
}
pub(crate) fn permissions(protect: u32) -> Option<Permissions> {
    let cache = protect & (PAGE_NOCACHE | PAGE_WRITECOMBINE);
    if cache == (PAGE_NOCACHE | PAGE_WRITECOMBINE) {
        return None;
    }
    match protect & !(PAGE_NOCACHE | PAGE_WRITECOMBINE) {
        PAGE_NOACCESS => Some(Permissions::NONE),
        PAGE_READONLY => Some(Permissions::READ),
        PAGE_READWRITE => Some(Permissions::RW),
        PAGE_EXECUTE_READ => Some(Permissions::RX),
        PAGE_EXECUTE_READWRITE => Some(Permissions::RWX),
        _ => None,
    }
}
/// Whether [va, va + len) touches a page Runtime guards as lifted guest code
/// (`Guest::guard_code`; the loader guards executable XBE sections).
fn lifted_code(memory: &Guest, va: u32, len: u32) -> bool {
    let (start, end) = (va as u64, va as u64 + len as u64);
    memory
        .code_ranges()
        .iter()
        .any(|r| start < r.end && r.start < end)
}
/// Whether `Guest::map` would accept `[va, va + len)`: no existing mapping
/// and no physical reservation alias window overlaps it.
fn mappable(memory: &Guest, va: u32, len: u32) -> bool {
    !memory
        .mappings()
        .iter()
        .any(|m| overlaps(va, len, m.virtual_start, m.len))
        && !memory
            .physical_reservations()
            .iter()
            .flat_map(|r| r.aliases.iter().map(move |&a| (a, r.len)))
            .any(|(a, n)| overlaps(va, len, a, n))
}
/// Why `Guest::unmap(page, PAGE_SIZE)` would refuse, as a stop reason, or
/// None if it would succeed: it refuses HLE claims, holes and lifted code.
fn unmap_refusal(memory: &Guest, page: u32) -> Option<&'static str> {
    if memory
        .virtual_claims()
        .iter()
        .any(|c| overlaps(page, PAGE_SIZE, c.virtual_start, c.len))
    {
        return Some("freeing virtual memory inside an HLE-owned claim");
    }
    if !memory.mappings().iter().any(|m| {
        page >= m.virtual_start
            && page as u64 + PAGE_SIZE as u64 <= m.virtual_start as u64 + m.len as u64
    }) {
        return Some("freeing committed virtual memory that is no longer mapped");
    }
    lifted_code(memory, page, PAGE_SIZE).then_some("freeing virtual memory that holds lifted code")
}
fn overlaps(a: u32, n: u32, b: u32, m: u32) -> bool {
    (a as u64) < b as u64 + m as u64 && (b as u64) < a as u64 + n as u64
}

impl Memory {
    // Follow-up: range indexing; allocation/commit can scan O(pages × mappings).
    /// Every occupied `[start, end)` overlapping `[base, base + size)`: the
    /// HLE range, stack guards, kernel regions, Runtime mappings and the
    /// alias windows of physical reservations (which only `map_reserved` can
    /// fill). `own` exempts that reservation's windows, for placements made
    /// inside them.
    fn occupants(
        &self,
        memory: &Guest,
        base: u32,
        size: u32,
        own: Option<ReservationId>,
    ) -> Vec<(u64, u64)> {
        let span = |start: u32, len: u32| (start as u64, start as u64 + len as u64);
        let windows = memory
            .physical_reservations()
            .iter()
            .filter(|r| Some(r.id) != own)
            .flat_map(|r| r.aliases.iter().map(move |&a| span(a, r.len)));
        std::iter::once(span(
            HLE_RESERVED_START,
            HLE_RESERVED_END - HLE_RESERVED_START,
        ))
        .chain(self.guards.iter().map(|&g| span(g, PAGE_SIZE)))
        .chain(self.regions.values().map(|r| span(r.base, r.size)))
        .chain(
            memory
                .mappings()
                .iter()
                .map(|m| span(m.virtual_start, m.len)),
        )
        .chain(windows)
        .filter(|&(start, end)| (base as u64) < end && start < base as u64 + size as u64)
        .collect()
    }
    fn vacant(&self, memory: &Guest, base: u32, size: u32) -> bool {
        self.occupants(memory, base, size, None).is_empty()
    }
    fn find_virtual(
        &self,
        memory: &Guest,
        size: u32,
        start: u32,
        end: u32,
        align: u32,
    ) -> Option<u32> {
        self.find_virtual_in(memory, size, start..end, align, None)
    }
    /// Lowest vacant `align`ed range of `size` bytes inside `range`, with
    /// `own`'s alias windows treated as free.
    fn find_virtual_in(
        &self,
        memory: &Guest,
        size: u32,
        range: std::ops::Range<u32>,
        align: u32,
        own: Option<ReservationId>,
    ) -> Option<u32> {
        let mut base = round_up(range.start, align)?;
        while base as u64 + size as u64 <= range.end as u64 {
            let occupied_end = self
                .occupants(memory, base, size, own)
                .into_iter()
                .map(|(_, end)| end)
                .max();
            let Some(occupied_end) = occupied_end else {
                return Some(base);
            };
            base = round_up(u32::try_from(occupied_end).ok()?, align)?;
        }
        None
    }
    /// Highest vacant `align`ed range of `size` bytes inside [start, end):
    /// MEM_TOP_DOWN placement.
    fn find_virtual_top_down(
        &self,
        memory: &Guest,
        size: u32,
        start: u32,
        end: u32,
        align: u32,
    ) -> Option<u32> {
        let mut top = end;
        loop {
            let base = top.checked_sub(size)? & !(align - 1);
            if base < start {
                return None;
            }
            // Retry below the lowest occupant overlapping this candidate.
            let lowest = self
                .occupants(memory, base, size, None)
                .into_iter()
                .map(|(start, _)| start)
                .min();
            let Some(lowest) = lowest else {
                return Some(base);
            };
            top = lowest as u32;
        }
    }
    /// Lowest free physical range in [low, high] (inclusive, as the
    /// MmAllocateContiguousMemoryEx bounds are): Runtime's occupancy, which
    /// includes mappings and physical reservations.
    fn find_physical(
        memory: &Guest,
        size: u32,
        low: u32,
        high: u32,
        alignment: u32,
    ) -> Option<u32> {
        let end = (high as u64 + 1).min(RAM_SIZE as u64) as u32;
        memory.find_free_physical(size, low..end, alignment)
    }
    /// `count` distinct free physical pages, lowest first.
    fn physical_pages(memory: &Guest, count: usize) -> Option<Vec<u32>> {
        let mut pages = Vec::with_capacity(count);
        let mut cursor = 0;
        while pages.len() < count {
            let page = memory.find_free_physical(PAGE_SIZE, cursor..RAM_SIZE as u32, PAGE_SIZE)?;
            pages.push(page);
            cursor = page + PAGE_SIZE;
        }
        Some(pages)
    }
    /// Internal objects use a separate harness VA range, but the same physical
    /// occupancy as every other allocation. A single map makes failure atomic.
    pub(crate) fn internal(&mut self, memory: &mut Guest, size: u32) -> Result<u32, NtStatus> {
        let size = round_up(size.max(1), PAGE_SIZE).ok_or(status::NO_MEMORY)?;
        let base = self
            .find_virtual(memory, size, 0xa100_0000, 0xb000_0000, PAGE_SIZE)
            .ok_or(status::NO_MEMORY)?;
        let physical =
            Self::find_physical(memory, size, 0, u32::MAX, PAGE_SIZE).ok_or(status::NO_MEMORY)?;
        memory
            .map(base, size, physical, Permissions::RW)
            .map_err(|_| status::NO_MEMORY)?;
        // A newly used gap can contain bytes written through an earlier alias.
        zero(memory, base, size).expect("fresh RW mapping");
        Ok(base)
    }
    pub(crate) fn guarded_thread(
        &mut self,
        memory: &mut Guest,
        stack_size: u32,
    ) -> Result<(u32, u32, u32), NtStatus> {
        let span = stack_size
            .checked_add(2 * PAGE_SIZE)
            .ok_or(status::NO_MEMORY)?;
        let base = self
            .find_virtual(memory, span, 0xa100_0000, 0xb000_0000, PAGE_SIZE)
            .ok_or(status::NO_MEMORY)?;
        let backed = stack_size.checked_add(PAGE_SIZE).ok_or(status::NO_MEMORY)?;
        let physical =
            Self::find_physical(memory, backed, 0, u32::MAX, PAGE_SIZE).ok_or(status::NO_MEMORY)?;
        let bottom = base + 2 * PAGE_SIZE;
        memory
            .map(base, PAGE_SIZE, physical, Permissions::RW)
            .expect("preflighted thread object");
        memory
            .map(bottom, stack_size, physical + PAGE_SIZE, Permissions::RW)
            .expect("preflighted thread stack");
        memory
            .write_bytes(base, &[0; PAGE_SIZE as usize], 1)
            .expect("fresh thread object");
        zero(memory, bottom, stack_size).expect("fresh thread stack");
        self.guards.insert(base + PAGE_SIZE);
        Ok((base, bottom, bottom + stack_size))
    }
    /// MmCreateKernelStack: `size` (page multiple) RW zeroed bytes in the
    /// kernel's internal range with an unmapped guard page below. Returns
    /// the lowest stack address.
    pub(crate) fn kernel_stack(&mut self, memory: &mut Guest, size: u32) -> Result<u32, NtStatus> {
        let span = size.checked_add(PAGE_SIZE).ok_or(status::NO_MEMORY)?;
        let base = self
            .find_virtual(memory, span, 0xa100_0000, 0xb000_0000, PAGE_SIZE)
            .ok_or(status::NO_MEMORY)?;
        let physical =
            Self::find_physical(memory, size, 0, u32::MAX, PAGE_SIZE).ok_or(status::NO_MEMORY)?;
        let bottom = base + PAGE_SIZE;
        memory
            .map(bottom, size, physical, Permissions::RW)
            .map_err(|_| status::NO_MEMORY)?;
        zero(memory, bottom, size).expect("fresh kernel stack");
        self.guards.insert(base);
        self.kernel_stacks.insert(bottom, KernelStack { size });
        Ok(bottom)
    }
    /// MmDeleteKernelStack(StackBase = top, StackLimit = bottom): unmap the
    /// stack and release its guard. A range MmCreateKernelStack did not hand
    /// out, or one Runtime refuses to unmap (an HLE claim inside it), is a
    /// stop reason, and the stack stays live and unchanged.
    pub(crate) fn delete_kernel_stack(
        &mut self,
        memory: &mut Guest,
        top: u32,
        bottom: u32,
    ) -> Result<(), &'static str> {
        let Some(stack) = self
            .kernel_stacks
            .get(&bottom)
            .copied()
            .filter(|s| bottom as u64 + s.size as u64 == top as u64)
        else {
            return Err("deleting a stack MmCreateKernelStack did not create");
        };
        match memory.unmap(bottom, stack.size) {
            Ok(()) => {}
            Err(MapError::Claimed) => {
                return Err("deleting a kernel stack inside an HLE-owned claim");
            }
            Err(MapError::LiftedCode) => {
                return Err("deleting a kernel stack that holds lifted code");
            }
            Err(_) => return Err("kernel stack unmap rejected by Runtime"),
        }
        self.kernel_stacks.remove(&bottom);
        self.guards.remove(&(bottom - PAGE_SIZE));
        Ok(())
    }
    pub(crate) fn allocate_pool(&mut self, memory: &mut Guest, size: u32, tag: u32) -> u32 {
        let Some(size) = round_up(size.max(1), 16) else {
            return 0;
        };
        if let Some((&base, block)) = self
            .pool
            .iter_mut()
            .find(|(_, b)| !b.live && b.capacity >= size)
        {
            block.size = size;
            block.tag = tag;
            block.live = true;
            return base;
        }
        let Ok(base) = self.internal(memory, size) else {
            return 0;
        };
        self.pool.insert(
            base,
            PoolBlock {
                size,
                capacity: round_up(size, PAGE_SIZE).unwrap(),
                tag,
                live: true,
            },
        );
        base
    }
    pub(crate) fn free_pool(&mut self, base: u32) -> bool {
        if let Some(b) = self.pool.get_mut(&base).filter(|b| b.live) {
            b.live = false;
            true
        } else {
            false
        }
    }
    fn allocate_virtual(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        let base_ptr = a.get(0);
        let size_ptr = a.get(2);
        memory.check(base_ptr, 4, Access::Write, 1)?;
        memory.check(size_ptr, 4, Access::Write, 1)?;
        let requested = memory.u32(base_ptr)?;
        let requested_size = memory.u32(size_ptr)?;
        let zero_bits = a.get(1);
        let flags = a.get(3);
        let protect = a.get(4);
        if flags & MEM_NOZERO != 0 {
            return Ok(Action::Stop("MEM_NOZERO initialization policy unsupported"));
        }
        if requested_size == 0
            || zero_bits > 21
            || flags & (MEM_COMMIT | MEM_RESERVE) == 0
            || flags & !(MEM_COMMIT | MEM_RESERVE | MEM_TOP_DOWN) != 0
        {
            return Ok(Action::status(status::INVALID_PARAMETER));
        }
        // MEM_TOP_DOWN only chooses where a new reservation goes; with an
        // explicit base address it changes nothing.
        let top_down = flags & MEM_TOP_DOWN != 0;
        let Some(perms) = permissions(protect) else {
            return Ok(Action::Stop("unsupported page protection"));
        };
        let reserve = flags & MEM_RESERVE != 0 || requested == 0;
        let base = if requested == 0 {
            let end = if zero_bits == 0 {
                0x7fff_0000
            } else {
                (1u32 << (32 - zero_bits)).min(0x7fff_0000)
            };
            let Some(size) = round_up(requested_size, PAGE_SIZE) else {
                return Ok(Action::status(status::NO_MEMORY));
            };
            let start = if end <= 0x1000_0000 {
                0x10000
            } else {
                0x1000_0000
            };
            let found = if top_down {
                self.find_virtual_top_down(memory, size, start, end, 0x10000)
            } else {
                self.find_virtual(memory, size, start, end, 0x10000)
            };
            let Some(base) = found else {
                return Ok(Action::status(status::NO_MEMORY));
            };
            base
        } else if reserve {
            requested & !0xffff
        } else {
            requested & !(PAGE_SIZE - 1)
        };
        let Some(size) = requested_size
            .checked_add(requested.saturating_sub(base))
            .and_then(|v| round_up(v, PAGE_SIZE))
        else {
            return Ok(Action::status(status::INVALID_PARAMETER));
        };
        if base < 0x10000
            || base as u64 + size as u64 > 0x7fff_0000
            || (zero_bits != 0 && base as u64 + size as u64 > 1u64 << (32 - zero_bits))
        {
            return Ok(Action::status(status::CONFLICTING_ADDRESSES));
        }
        let region_base = if reserve {
            if !self.vacant(memory, base, size) {
                return Ok(Action::status(status::CONFLICTING_ADDRESSES));
            }
            base
        } else {
            let Some(r) = self.regions.values().find(|r| {
                base >= r.base && base as u64 + size as u64 <= r.base as u64 + r.size as u64
            }) else {
                return Ok(Action::status(status::CONFLICTING_ADDRESSES));
            };
            r.base
        };
        let pages: Vec<_> = (base..base + size)
            .step_by(PAGE_SIZE as usize)
            .filter(|page| reserve || !self.regions[&region_base].pages.contains_key(page))
            .collect();
        let commit = flags & MEM_COMMIT != 0;
        let physical = if commit {
            // Guest::map refuses existing mappings and reservation alias
            // windows; checking both here keeps the commit all-or-nothing.
            if pages.iter().any(|&page| !mappable(memory, page, PAGE_SIZE)) {
                return Ok(Action::status(status::CONFLICTING_ADDRESSES));
            }
            let Some(p) = Self::physical_pages(memory, pages.len()) else {
                return Ok(Action::status(status::NO_MEMORY));
            };
            p
        } else {
            Vec::new()
        };
        if reserve {
            self.regions.insert(
                base,
                Region {
                    base,
                    size,
                    allocation_protect: protect,
                    pages: BTreeMap::new(),
                },
            );
        }
        if commit {
            for (i, &page) in pages.iter().enumerate() {
                if memory
                    .map(page, PAGE_SIZE, physical[i], Permissions::RW)
                    .is_err()
                {
                    // Unreachable after the preflight; undo rather than
                    // leave a partial commit.
                    for &done in &pages[..i] {
                        memory.unmap(done, PAGE_SIZE).expect("mapped just now");
                    }
                    if reserve {
                        self.regions.remove(&base);
                    }
                    return Ok(Action::status(status::CONFLICTING_ADDRESSES));
                }
                zero(memory, page, PAGE_SIZE).expect("fresh RW mapping");
                memory.protect(page, perms).expect("fresh mapping");
            }
            let r = self.regions.get_mut(&region_base).unwrap();
            r.pages.extend(pages.iter().map(|&page| (page, protect)));
        }
        memory.set_u32(base_ptr, base)?;
        memory.set_u32(size_ptr, size)?;
        Ok(Action::status(status::SUCCESS))
    }
    fn query(&self, memory: &mut Guest, base: u32, out: u32) -> Result<Action, GuestFault> {
        memory.check(out, 28, Access::Write, 1)?;
        if base >= 0x8000_0000 {
            return Ok(Action::status(status::INVALID_PARAMETER));
        }
        let page = base & !(PAGE_SIZE - 1);
        let words = if let Some(r) = self
            .regions
            .values()
            .find(|r| page >= r.base && page < r.base + r.size)
        {
            let protect = r.pages.get(&page).copied();
            let mut end = page + PAGE_SIZE;
            while end < r.base + r.size && r.pages.get(&end).copied() == protect {
                end += PAGE_SIZE;
            }
            [
                page,
                r.base,
                r.allocation_protect,
                end - page,
                if protect.is_some() {
                    MEM_COMMIT
                } else {
                    MEM_RESERVE
                },
                protect.unwrap_or(0),
                MEM_PRIVATE,
            ]
        } else if memory.mappings().iter().any(|m| {
            base >= m.virtual_start && (base as u64) < m.virtual_start as u64 + m.len as u64
        }) {
            return Ok(Action::Stop(
                "query of a mapping without allocation-type provenance",
            ));
        } else {
            let end = self
                .regions
                .keys()
                .copied()
                .chain(memory.mappings().iter().map(|m| m.virtual_start))
                .filter(|v| *v > page && *v < 0x8000_0000)
                .min()
                .unwrap_or(0x8000_0000);
            [page, 0, 0, end - page, MEM_FREE, 0, 0]
        };
        for (i, word) in words.into_iter().enumerate() {
            memory.set_u32(out + i as u32 * 4, word)?;
        }
        Ok(Action::status(status::SUCCESS))
    }
    /// MmSetAddressProtect(BaseAddress, NumberOfBytes, NewProtect) over pages
    /// of contiguous allocations or committed virtual memory. The range is
    /// checked first, then changed with `Guest::protect_range` (which splits
    /// mappings at the range edges; other aliases keep their protection);
    /// MmQueryAddressProtect and NtQueryVirtualMemory report the new value.
    fn set_protect(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        let (base, size, protect) = (a.get(0), a.get(1), a.get(2));
        let Some(perms) = permissions(protect) else {
            return Ok(Action::Stop("unsupported page protection"));
        };
        if size == 0 {
            return Ok(Action::Complete(Value::Void));
        }
        let first = base & !(PAGE_SIZE - 1);
        let Some(end) = (base as u64 + size as u64)
            .checked_next_multiple_of(PAGE_SIZE as u64)
            .filter(|e| *e <= 1 << 32)
        else {
            return Ok(Action::Stop("protection range overflows the address space"));
        };
        let pages: Vec<u32> = (first as u64..end)
            .step_by(PAGE_SIZE as usize)
            .map(|p| p as u32)
            .collect();
        let contiguous = |page: u32| {
            self.contiguous
                .iter()
                .any(|(&va, &(len, _))| page >= va && (page as u64) < va as u64 + len as u64)
        };
        let committed = |page: u32| self.regions.values().any(|r| r.pages.contains_key(&page));
        if !pages.iter().all(|&p| contiguous(p) || committed(p)) {
            return Ok(Action::Stop(
                "protection change outside contiguous or committed memory",
            ));
        }
        match memory.protect_range(first, (end - first as u64) as u32, perms) {
            Ok(()) => {}
            Err(MapError::Unmapped) => {
                return Ok(Action::Stop("protection change over unmapped pages"));
            }
            Err(MapError::Claimed) => {
                return Ok(Action::Stop("protection change inside an HLE-owned claim"));
            }
            Err(MapError::LiftedCode) => {
                return Ok(Action::Stop(
                    "protection change would remove Execute from lifted code",
                ));
            }
            Err(_) => return Ok(Action::Stop("protection range rejected by Runtime")),
        }
        for page in pages {
            if let Some(r) = self
                .regions
                .values_mut()
                .find(|r| r.pages.contains_key(&page))
            {
                r.pages.insert(page, protect);
            } else {
                self.contiguous_pages.insert(page, protect);
            }
        }
        Ok(Action::Complete(Value::Void))
    }
    /// NtFreeVirtualMemory(&BaseAddress, &RegionSize, FreeType). Decommit
    /// covers [base & ~0xfff, round_up(base + size)), or to the end of the
    /// allocation when size is 0; release takes the allocation base and size
    /// 0. Committed pages are unmapped, so their physical pages are free again.
    fn free_virtual(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        let (base_ptr, size_ptr, kind) = (a.get(0), a.get(1), a.get(2));
        let base = memory.u32(base_ptr)?;
        let size = memory.u32(size_ptr)?;
        let Some(r) = self
            .regions
            .values()
            .find(|r| base >= r.base && (base as u64) < r.base as u64 + r.size as u64)
        else {
            return Ok(Action::status(status::MEMORY_NOT_ALLOCATED));
        };
        let region_end = r.base as u64 + r.size as u64;
        if kind == MEM_RELEASE && base != r.base {
            return Ok(Action::Stop(
                "release at an interior address is not implemented",
            ));
        }
        if kind != MEM_RELEASE && kind != MEM_DECOMMIT || kind == MEM_RELEASE && size != 0 {
            return Ok(Action::status(status::INVALID_PARAMETER));
        }
        let start = base & !(PAGE_SIZE - 1);
        let end = if size == 0 {
            region_end
        } else {
            (base as u64 + size as u64).next_multiple_of(PAGE_SIZE as u64)
        };
        if end > region_end {
            return Ok(Action::Stop("decommit range extends past its allocation"));
        }
        memory.check(base_ptr, 4, Access::Write, 1)?;
        memory.check(size_ptr, 4, Access::Write, 1)?;
        let region = r.base;
        let committed: Vec<u32> = r
            .pages
            .range(start..)
            .map(|(&page, _)| page)
            .take_while(|&page| (page as u64) < end)
            .collect();
        // Every page Guest::unmap would refuse is found before any page is
        // unmapped, so a refusal leaves the mappings and the region as they
        // were.
        if let Some(reason) = committed
            .iter()
            .find_map(|&page| unmap_refusal(memory, page))
        {
            return Ok(Action::Stop(reason));
        }
        let r = self.regions.get_mut(&region).expect("found above");
        for page in committed {
            memory.unmap(page, PAGE_SIZE).expect("preflighted unmap");
            r.pages.remove(&page);
        }
        if kind == MEM_RELEASE {
            self.regions.remove(&region);
        }
        memory.set_u32(base_ptr, start)?;
        memory.set_u32(size_ptr, (end - start as u64) as u32)?;
        Ok(Action::status(status::SUCCESS))
    }
    pub(crate) fn call(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        Ok(match a.export.ordinal {
            ord::EX_ALLOCATE_POOL | ord::EX_ALLOCATE_POOL_WITH_TAG => {
                Action::value(self.allocate_pool(
                    memory,
                    a.get(0),
                    if a.export.ordinal == ord::EX_ALLOCATE_POOL_WITH_TAG {
                        a.get(1)
                    } else {
                        0
                    },
                ))
            }
            ord::EX_FREE_POOL => {
                if self.free_pool(a.get(0)) {
                    Action::Complete(Value::Void)
                } else {
                    Action::Stop("invalid pool free")
                }
            }
            ord::EX_QUERY_POOL_BLOCK_SIZE => match self.pool.get(&a.get(0)).filter(|b| b.live) {
                Some(b) => Action::value(b.size),
                None => Action::Stop("unknown pool block"),
            },
            ord::MM_ALLOCATE_CONTIGUOUS_MEMORY | ord::MM_ALLOCATE_CONTIGUOUS_MEMORY_EX => {
                let size = round_up(a.get(0), PAGE_SIZE).filter(|v| *v != 0);
                let (low, high, alignment, protect) =
                    if a.export.ordinal == ord::MM_ALLOCATE_CONTIGUOUS_MEMORY {
                        (0, u32::MAX, PAGE_SIZE, PAGE_READWRITE)
                    } else {
                        (a.get(1), a.get(2), a.get(3).max(PAGE_SIZE), a.get(4))
                    };
                // Cache attributes need a device/cache policy, not silently RW.
                let Some(perms) = permissions(protect) else {
                    return Ok(Action::Stop(
                        "unsupported contiguous cache/protection policy",
                    ));
                };
                if a.export.ordinal == ord::MM_ALLOCATE_CONTIGUOUS_MEMORY_EX
                    && a.get(3) != 0
                    && !a.get(3).is_power_of_two()
                {
                    return Ok(Action::value(0));
                }
                let Some(size) = size else {
                    return Ok(Action::value(0));
                };
                // Bounds wholly inside a physical reservation (Halo 2's title
                // arena: XPhysicalAlloc(0x30e4000, 0x61000..0x3144fff) at
                // 0x12b43e) allocate from it, through its 0x80000000 alias.
                if let Some(r) = memory
                    .physical_reservations()
                    .iter()
                    .find(|r| {
                        low >= r.physical_start
                            && (high as u64) < r.physical_start as u64 + r.len as u64
                    })
                    .cloned()
                {
                    if !r.aliases.contains(&(CONTIGUOUS_ALIAS + r.physical_start)) {
                        return Ok(Action::Stop(
                            "reservation has no contiguous (0x80000000) alias window",
                        ));
                    }
                    let Some(va) = self.find_virtual_in(
                        memory,
                        size,
                        CONTIGUOUS_ALIAS + low
                            ..(CONTIGUOUS_ALIAS as u64 + high as u64 + 1).min(u32::MAX as u64)
                                as u32,
                        alignment,
                        Some(r.id),
                    ) else {
                        return Ok(Action::value(0));
                    };
                    memory
                        .map_reserved(r.id, va, size, perms)
                        .expect("vacant window inside the reservation");
                    self.contiguous.insert(va, (size, protect));
                    return Ok(Action::value(va));
                }
                let physical = {
                    let mut candidate = Self::find_physical(memory, size, low, high, alignment);
                    while let Some(c) = candidate {
                        if self.vacant(memory, CONTIGUOUS_ALIAS + c, size) {
                            break;
                        }
                        candidate = c.checked_add(alignment).and_then(|next| {
                            Self::find_physical(memory, size, next, high, alignment)
                        });
                    }
                    candidate
                };
                match physical {
                    Some(physical) => {
                        let va = CONTIGUOUS_ALIAS + physical;
                        memory
                            .map(va, size, physical, perms)
                            .expect("preflighted contiguous map");
                        self.contiguous.insert(va, (size, protect));
                        Action::value(va)
                    }
                    None => Action::value(0),
                }
            }
            ord::MM_FREE_CONTIGUOUS_MEMORY => {
                // Reserved pages stay reserved after the unmap.
                let base = a.get(0);
                let Some(&(size, _)) = self.contiguous.get(&base) else {
                    return Ok(Action::Stop(
                        "freeing memory MmAllocateContiguousMemory did not return",
                    ));
                };
                // Checked first so a refusal changes nothing.
                if lifted_code(memory, base, size) {
                    return Ok(Action::Stop(
                        "freeing contiguous memory that holds lifted code",
                    ));
                }
                match memory.unmap(base, size) {
                    Ok(()) => {}
                    Err(MapError::LiftedCode) => {
                        return Ok(Action::Stop(
                            "freeing contiguous memory that holds lifted code",
                        ));
                    }
                    Err(_) => return Ok(Action::Stop("contiguous unmap rejected by Runtime")),
                }
                self.contiguous.remove(&base);
                let pages = base..base + (size - 1);
                self.contiguous_pages.retain(|p, _| !pages.contains(p));
                Action::Complete(Value::Void)
            }
            ord::MM_GET_PHYSICAL_ADDRESS => {
                let va = a.get(0);
                match memory.mappings().iter().find(|m| {
                    va >= m.virtual_start && (va as u64) < m.virtual_start as u64 + m.len as u64
                }) {
                    // Offset first: virtual_start + len may equal 2^32.
                    Some(m) => Action::value(m.physical_start + (va - m.virtual_start)),
                    None => Action::Stop("physical translation of unmapped address"),
                }
            }
            ord::MM_MAP_IO_SPACE => Action::Stop("MmMapIoSpace requires a device model"),
            ord::MM_QUERY_ADDRESS_PROTECT => {
                let address = a.get(0);
                let value = self
                    .regions
                    .values()
                    .find_map(|r| r.pages.get(&(address & !(PAGE_SIZE - 1))).copied())
                    .or_else(|| {
                        self.contiguous_pages
                            .get(&(address & !(PAGE_SIZE - 1)))
                            .copied()
                    })
                    .or_else(|| {
                        self.contiguous
                            .iter()
                            .find(|(base, (size, _))| {
                                address >= **base && (address as u64) < **base as u64 + *size as u64
                            })
                            .map(|(_, (_, p))| *p)
                    });
                match value {
                    Some(p) => Action::value(p),
                    None => Action::Stop("unknown allocation protection"),
                }
            }
            ord::MM_QUERY_ALLOCATION_SIZE => {
                let address = a.get(0);
                let size = self
                    .regions
                    .values()
                    .find(|r| address >= r.base && address < r.base + r.size)
                    .map(|r| r.size)
                    .or_else(|| {
                        self.contiguous
                            .iter()
                            .find(|(base, (size, _))| {
                                address >= **base && (address as u64) < **base as u64 + *size as u64
                            })
                            .map(|(_, (n, _))| *n)
                    });
                match size {
                    Some(n) => Action::value(n),
                    None => Action::Stop("unknown allocation size"),
                }
            }
            ord::MM_SET_ADDRESS_PROTECT => return self.set_protect(memory, a),
            ord::MM_CREATE_KERNEL_STACK => {
                if a.boolean(1) {
                    return Ok(Action::Stop("debugger kernel stacks unsupported"));
                }
                let Some(size) = round_up(a.get(0), PAGE_SIZE).filter(|n| *n != 0) else {
                    return Ok(Action::Stop("kernel stack size unsupported"));
                };
                // Returns the top: the initial stack pointer side (the
                // caller at 0x335653 deletes it as (top, top - 0x6000)).
                match self.kernel_stack(memory, size) {
                    Ok(bottom) => Action::value(bottom + size),
                    Err(_) => Action::value(0),
                }
            }
            ord::MM_DELETE_KERNEL_STACK => {
                match self.delete_kernel_stack(memory, a.get(0), a.get(1)) {
                    Ok(()) => Action::Complete(Value::Void),
                    Err(reason) => Action::Stop(reason),
                }
            }
            ord::NT_ALLOCATE_VIRTUAL_MEMORY => return self.allocate_virtual(memory, a),
            ord::NT_FREE_VIRTUAL_MEMORY => return self.free_virtual(memory, a),
            ord::NT_QUERY_VIRTUAL_MEMORY => return self.query(memory, a.get(0), a.get(1)),
            _ => Action::Stop("declared export has no handler"),
        })
    }
}

impl crate::Core {
    /// MmPersistContiguousMemory (178): mark or unmark whole pages of a
    /// contiguous allocation to survive a quick reboot. There is no quick
    /// reboot yet, so the effect is the recorded page set
    /// (`Kernel::persistent_pages`). Ranges outside contiguous memory stop.
    pub(crate) fn persist_call(
        &mut self,
        _memory: &mut Guest,
        a: &Args,
    ) -> Result<Action, GuestFault> {
        let (base, size, persist) = (a.get(0), a.get(1), a.boolean(2));
        if size == 0 {
            return Ok(Action::Complete(Value::Void));
        }
        let first = base & !(PAGE_SIZE - 1);
        let Some(end) = base
            .checked_add(size - 1)
            .map(|last| last & !(PAGE_SIZE - 1))
        else {
            return Ok(Action::Stop("persistent range overflows the address space"));
        };
        let inside = self.memory.contiguous.iter().any(|(&va, &(len, _))| {
            first >= va && end as u64 + PAGE_SIZE as u64 <= va as u64 + len as u64
        });
        if !inside {
            return Ok(Action::Stop(
                "persisting memory outside a contiguous allocation",
            ));
        }
        let mut page = first;
        loop {
            if persist {
                self.persistent.insert(page);
            } else {
                self.persistent.remove(&page);
            }
            if page == end {
                break;
            }
            page += PAGE_SIZE;
        }
        Ok(Action::Complete(Value::Void))
    }
}
