//! One physical allocator derives availability from Runtime's mappings, so
//! image pages, DATA exports, stacks and aliases cannot be handed out twice.
use crate::ord;
use crate::{
    Action,
    abi::{Args, Value},
    status::{self, NtStatus},
};
use runtime::{
    Guest, GuestFault,
    guest::{Access, PAGE_SIZE, Permissions, RAM_SIZE},
};
use std::collections::{BTreeMap, BTreeSet};

/// Guest virtual range reserved for HLE-owned memory (first user: the Bink
/// movie arena). No kernel allocator places anything here, and fixed
/// NtAllocateVirtualMemory requests into it fail with
/// STATUS_CONFLICTING_ADDRESSES. Kernel internals use 0xa1000000..0xb0000000.
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
#[derive(Debug, Default, Clone)]
pub struct Memory {
    pub regions: BTreeMap<u32, Region>,
    pub pool: BTreeMap<u32, PoolBlock>,
    pub contiguous: BTreeMap<u32, (u32, u32)>,
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
        PAGE_READONLY => Some(Permissions::READ),
        PAGE_READWRITE => Some(Permissions::RW),
        PAGE_EXECUTE_READ => Some(Permissions::RX),
        PAGE_EXECUTE_READWRITE => Some(Permissions::RWX),
        _ => None,
    }
}
fn overlaps(a: u32, n: u32, b: u32, m: u32) -> bool {
    (a as u64) < b as u64 + m as u64 && (b as u64) < a as u64 + n as u64
}

impl Memory {
    // Follow-up: range indexing; allocation/commit can scan O(pages × mappings).
    fn vacant(&self, memory: &Guest, base: u32, size: u32) -> bool {
        !overlaps(
            base,
            size,
            HLE_RESERVED_START,
            HLE_RESERVED_END - HLE_RESERVED_START,
        ) && !self
            .guards
            .iter()
            .any(|guard| overlaps(base, size, *guard, PAGE_SIZE))
            && !self
                .regions
                .values()
                .any(|r| overlaps(base, size, r.base, r.size))
            && !memory
                .mappings()
                .iter()
                .any(|m| overlaps(base, size, m.virtual_start, m.len))
    }
    fn find_virtual(
        &self,
        memory: &Guest,
        size: u32,
        start: u32,
        end: u32,
        align: u32,
    ) -> Option<u32> {
        let mut base = round_up(start, align)?;
        while base as u64 + size as u64 <= end as u64 {
            if self.vacant(memory, base, size) {
                return Some(base);
            }
            let occupied_end = self
                .regions
                .values()
                .filter(|r| overlaps(base, size, r.base, r.size))
                .map(|r| r.base as u64 + r.size as u64)
                .chain(
                    memory
                        .mappings()
                        .iter()
                        .filter(|m| overlaps(base, size, m.virtual_start, m.len))
                        .map(|m| m.virtual_start as u64 + m.len as u64),
                )
                .chain(
                    self.guards
                        .iter()
                        .filter(|g| overlaps(base, size, **g, PAGE_SIZE))
                        .map(|g| *g as u64 + PAGE_SIZE as u64),
                )
                .chain(
                    overlaps(
                        base,
                        size,
                        HLE_RESERVED_START,
                        HLE_RESERVED_END - HLE_RESERVED_START,
                    )
                    .then_some(HLE_RESERVED_END as u64),
                )
                .max()?;
            base = round_up(u32::try_from(occupied_end).ok()?, align)?;
        }
        None
    }
    fn find_physical(
        memory: &Guest,
        size: u32,
        low: u32,
        high: u32,
        alignment: u32,
    ) -> Option<u32> {
        let mut base = round_up(low, alignment)?;
        let end = (high as u64 + 1).min(RAM_SIZE as u64);
        loop {
            if base as u64 + size as u64 > end {
                return None;
            }
            let conflict = memory
                .mappings()
                .iter()
                .filter(|m| overlaps(base, size, m.physical_start, m.len))
                .map(|m| m.physical_start as u64 + m.len as u64)
                .max();
            if let Some(end) = conflict {
                base = round_up(u32::try_from(end).ok()?, alignment)?;
            } else {
                return Some(base);
            }
        }
    }
    fn physical_pages(memory: &Guest, count: usize) -> Option<Vec<u32>> {
        let mut occupied: Vec<_> = memory
            .mappings()
            .iter()
            .map(|m| (m.physical_start, m.physical_start + m.len))
            .collect();
        occupied.sort_unstable();
        occupied.push((RAM_SIZE as u32, RAM_SIZE as u32));
        let mut cursor = 0;
        let mut pages = Vec::new();
        for (start, end) in occupied {
            while pages.len() < count && cursor + PAGE_SIZE <= start {
                pages.push(cursor);
                cursor += PAGE_SIZE;
            }
            if pages.len() == count {
                return Some(pages);
            }
            cursor = round_up(cursor.max(end), PAGE_SIZE)?;
        }
        None
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
        if flags & (MEM_TOP_DOWN | MEM_NOZERO) != 0 {
            return Ok(Action::Stop(
                "allocation placement/initialization flags unsupported",
            ));
        }
        if requested_size == 0
            || zero_bits > 21
            || flags == 0
            || flags & !(MEM_COMMIT | MEM_RESERVE) != 0
        {
            return Ok(Action::status(status::INVALID_PARAMETER));
        }
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
            let Some(base) = self.find_virtual(memory, size, start, end, 0x10000) else {
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
            if pages.iter().any(|page| {
                memory
                    .mappings()
                    .iter()
                    .any(|m| overlaps(*page, PAGE_SIZE, m.virtual_start, m.len))
            }) {
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
            let r = self.regions.get_mut(&region_base).unwrap();
            for (i, page) in pages.iter().enumerate() {
                memory
                    .map(*page, PAGE_SIZE, physical[i], Permissions::RW)
                    .expect("preflighted commit");
                memory.write_bytes(*page, &[0; PAGE_SIZE as usize], 1)?;
                memory.protect(*page, perms).expect("fresh mapping");
                r.pages.insert(*page, protect);
            }
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
                let physical = size.and_then(|size| {
                    let mut candidate = Self::find_physical(memory, size, low, high, alignment)?;
                    loop {
                        if self.vacant(memory, 0x8000_0000 + candidate, size) {
                            break Some(candidate);
                        }
                        candidate = Self::find_physical(
                            memory,
                            size,
                            candidate.checked_add(alignment)?,
                            high,
                            alignment,
                        )?;
                    }
                });
                if let (Some(size), Some(physical)) = (size, physical) {
                    let va = 0x8000_0000 + physical;
                    if !self.vacant(memory, va, size) {
                        return Ok(Action::value(0));
                    }
                    memory
                        .map(va, size, physical, perms)
                        .expect("preflighted contiguous map");
                    self.contiguous.insert(va, (size, protect));
                    Action::value(va)
                } else {
                    Action::value(0)
                }
            }
            ord::MM_FREE_CONTIGUOUS_MEMORY => {
                Action::Stop("runtime cannot unmap contiguous allocations")
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
            ord::NT_ALLOCATE_VIRTUAL_MEMORY => return self.allocate_virtual(memory, a),
            ord::NT_FREE_VIRTUAL_MEMORY => {
                let base = memory.u32(a.get(0))?;
                let size = memory.u32(a.get(1))?;
                let Some(r) = self
                    .regions
                    .values()
                    .find(|r| base >= r.base && base < r.base + r.size)
                else {
                    return Ok(Action::status(status::MEMORY_NOT_ALLOCATED));
                };
                if a.get(2) == MEM_RELEASE && base != r.base {
                    return Ok(Action::Stop(
                        "release at an interior address is not implemented",
                    ));
                }
                if a.get(2) == MEM_RELEASE && size == 0 && r.pages.is_empty() {
                    memory.check(a.get(0), 4, Access::Write, 1)?;
                    memory.check(a.get(1), 4, Access::Write, 1)?;
                    let size = r.size;
                    self.regions.remove(&base);
                    memory.set_u32(a.get(0), base)?;
                    memory.set_u32(a.get(1), size)?;
                    Action::status(status::SUCCESS)
                } else if a.get(2) != MEM_RELEASE && a.get(2) != MEM_DECOMMIT
                    || a.get(2) == MEM_RELEASE && size != 0
                {
                    Action::status(status::INVALID_PARAMETER)
                } else {
                    Action::Stop("runtime cannot unmap committed pages")
                }
            }
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
