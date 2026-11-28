//! Buffer one animation segment's stores until every input and destination is
//! known. Later loads see earlier stores, including through physical aliases.
use super::Memory;
use std::mem::MaybeUninit;
use runtime::{
    Guest, GuestFault,
    guest::{Access, FaultReason},
};

/// Memory held exclusively throughout a segment. A successful write check
/// guarantees the same write will succeed at commit. Loads must be plain RAM.
pub trait SegmentMemory: Memory {
    /// Physical start of a contiguous range with the requested access.
    fn checked_offset(&self, addr: u32, len: usize, access: Access, alignment: u32) -> Option<u32>;
}

impl SegmentMemory for Guest {
    fn checked_offset(&self, addr: u32, len: usize, access: Access, alignment: u32) -> Option<u32> {
        if self.has_trap() {
            return None;
        }
        // One page of the fast page table answers most accesses with one
        // lookup; it gives the physical offset the walk below would.
        if let Some(physical) = self.fast_physical(addr, len, access, alignment) {
            return Some(physical);
        }
        self.check(addr, len, access, alignment).ok()?;
        let maps = self.mappings();
        let i = maps
            .partition_point(|m| m.virtual_start <= addr)
            .checked_sub(1)?;
        let mapping = &maps[i];
        let offset = addr - mapping.virtual_start;
        // Leave accesses straddling virtual mappings to the lifted instruction.
        (u64::from(offset) + len as u64 <= u64::from(mapping.len))
            .then(|| mapping.physical_start + offset)
    }
}

// Common segments use only the inline log. A larger node or event copy
// spills and indexes its stores by physical address for forwarding.
#[derive(Clone, Copy, Default)]
pub(crate) struct Store {
    addr: u32,
    physical: u32,
    bytes: [u8; 4],
    len: u8,
    alignment: u32,
}

/// The inline store log, held by the caller of a segment for as long as
/// its [`Plan`] borrows it. Its slots are written before they are read
/// (`Plan::count` of them), so it is never filled: a segment makes no copy
/// of the log, neither to set it up nor to move the plan.
pub(crate) struct Log<const N: usize>([MaybeUninit<Store>; N]);

impl<const N: usize> Log<N> {
    pub(crate) fn new() -> Self {
        Self([const { MaybeUninit::uninit() }; N])
    }

    /// The first `count` stores, each written by `Plan::write_bytes`.
    fn written(&self, count: usize) -> &[Store] {
        let written = &self.0[..count.min(N)];
        // SAFETY: `Plan` writes slot `i` before it counts it (`count` only
        // grows past `i` after `self.0[i]` is written), and `Store` is
        // plain `Copy` data, so the first `count` slots are initialised.
        unsafe { std::slice::from_raw_parts(written.as_ptr().cast::<Store>(), written.len()) }
    }
}

pub(crate) struct Plan<'a, M, const N: usize> {
    text: std::ops::Range<u64>,
    inline_text: std::ops::Range<u64>,
    ram: &'a mut M,
    log: &'a mut Log<N>,
    count: usize,
    allow_spill: bool,
    spill: Vec<Store>,
    forwarded: Option<std::collections::HashMap<u32, [Option<u8>; 4]>>,
}

fn decline(addr: u32, len: usize, access: Access) -> GuestFault {
    GuestFault {
        addr,
        len,
        access,
        reason: FaultReason::InvalidBuffer,
    }
}

impl<'a, M: SegmentMemory, const N: usize> Plan<'a, M, N> {
    fn new(ram: &'a mut M, log: &'a mut Log<N>, start: u32, end: u32) -> Option<Self> {
        let len = end.checked_sub(start)?;
        let text = u64::from(ram.checked_offset(start, len as usize, Access::Read, 1)?);
        Some(Self {
            ram,
            text: text..text + u64::from(len),
            inline_text: 0..0,
            log,
            count: 0,
            allow_spill: false,
            spill: Vec::new(),
            forwarded: None,
        })
    }

    pub(crate) fn for_segment(
        ram: &'a mut M,
        log: &'a mut Log<N>,
        cpu: &runtime::CpuAbi,
        pc: u32,
        text: std::ops::Range<u32>,
        sites: &[u32],
    ) -> Option<Self> {
        if !super::supported(cpu) || (pc != text.start && sites.binary_search(&pc).is_err()) {
            return None;
        }
        Self::new(ram, log, text.start, text.end)
    }

    /// The stores logged so far, oldest first; the spill follows them.
    fn logged(&self) -> &[Store] {
        self.log.written(self.count)
    }

    /// The value of each physical byte after the segment. Keep only the
    /// last write to an overlapping address, including through aliases.
    pub(crate) fn final_bytes(&self) -> Vec<(u32, u8)> {
        let mut bytes = Vec::new();
        for store in self.logged().iter().chain(&self.spill).rev() {
            for (i, &value) in store.bytes[..usize::from(store.len)].iter().enumerate() {
                bytes.push((store.physical + i as u32, value));
            }
        }
        // Stable sort keeps the newest byte first at each address.
        bytes.sort_by_key(|&(addr, _)| addr);
        bytes.dedup_by_key(|(addr, _)| *addr);
        bytes
    }

    /// Called before executing the segment. If the leaf has no contiguous
    /// instruction mapping it cannot run inline (its own gather checks this).
    pub(crate) fn protect_inline_text(&mut self, start: u32, end: u32) {
        let len = end - start;
        if let Some(offset) = self.ram.checked_offset(start, len as usize, Access::Read, 1) {
            let lo = u64::from(offset);
            self.inline_text = lo..lo + u64::from(len);
        }
    }

    pub(crate) fn allow_large_segment(&mut self) {
        self.allow_spill = true;
    }

    fn index_store(words: &mut std::collections::HashMap<u32, [Option<u8>; 4]>, store: &Store) {
        for (i, &byte) in store.bytes[..usize::from(store.len)].iter().enumerate() {
            let p = store.physical + i as u32;
            words.entry(p / 4).or_default()[(p % 4) as usize] = Some(byte);
        }
    }

    fn commit(self) -> Result<(), GuestFault> {
        for store in self.log.written(self.count).iter().chain(&self.spill) {
            self.ram.write_bytes(
                store.addr,
                &store.bytes[..usize::from(store.len)],
                store.alignment,
            )?;
        }
        Ok(())
    }
}

impl<const N: usize> Plan<'_, Guest, N> {
    /// The caller may subscribe to physical writes and read this range to
    /// build a derived lookup table. All guest stores still go via the log.
    /// A staged write in either mapping makes the underlying RAM stale.
    pub(crate) fn unstaged_read(&mut self, addr: u32, len: usize) -> Option<&mut Guest> {
        let start = u64::from(self.ram.checked_offset(addr, len, Access::Read, 1)?);
        let end = start + len as u64;
        let changed = self.log.written(self.count).iter().chain(&self.spill).any(|s| {
            let written = u64::from(s.physical);
            written < end && start < written + u64::from(s.len)
        });
        (!changed).then_some(&mut *self.ram)
    }
}

impl<M: SegmentMemory, const N: usize> Memory for Plan<'_, M, N> {
    fn physical_read(&self, addr: u32, len: usize) -> Option<u32> {
        self.ram.checked_offset(addr, len, Access::Read, 1)
    }

    fn read_bytes(&self, addr: u32, dst: &mut [u8], alignment: u32) -> Result<(), GuestFault> {
        let physical = self
            .ram
            .checked_offset(addr, dst.len(), Access::Read, alignment)
            .ok_or_else(|| decline(addr, dst.len(), Access::Read))?;
        self.ram.read_bytes(addr, dst, alignment)?;
        if let Some(words) = &self.forwarded {
            for (i, byte) in dst.iter_mut().enumerate() {
                let p = physical + i as u32;
                if let Some(Some(value)) = words.get(&(p / 4)).map(|w| w[(p % 4) as usize]) {
                    *byte = value;
                }
            }
            return Ok(());
        }
        let start = u64::from(physical);
        let end = start + dst.len() as u64;
        for store in self.logged() {
            let written = u64::from(store.physical);
            let lo = start.max(written);
            let hi = end.min(written + u64::from(store.len));
            if lo < hi {
                dst[(lo - start) as usize..(hi - start) as usize].copy_from_slice(
                    &store.bytes[(lo - written) as usize..(hi - written) as usize],
                );
            }
        }
        Ok(())
    }

    fn write_bytes(&mut self, addr: u32, src: &[u8], alignment: u32) -> Result<(), GuestFault> {
        let err = || decline(addr, src.len(), Access::Write);
        let physical = self
            .ram
            .checked_offset(addr, src.len(), Access::Write, alignment)
            .ok_or_else(err)?;
        let start = u64::from(physical);
        let end = start + src.len() as u64;
        if (start < self.text.end && self.text.start < end)
            || (start < self.inline_text.end && self.inline_text.start < end)
        {
            return Err(err());
        }
        let mut store = Store::default();
        let dst = store.bytes.get_mut(..src.len()).ok_or_else(err)?;
        dst.copy_from_slice(src);
        store.addr = addr;
        store.physical = physical;
        store.len = src.len() as u8;
        store.alignment = alignment;
        if self.count < N {
            self.log.0[self.count] = MaybeUninit::new(store);
            self.count += 1;
        } else {
            // Bound malformed copy counts before anything reaches guest RAM.
            if !self.allow_spill || self.spill.len() >= 65536 {
                return Err(err());
            }
            let (log, count) = (&*self.log, self.count);
            let words = self.forwarded.get_or_insert_with(|| {
                let mut words = std::collections::HashMap::new();
                for previous in log.written(count) {
                    Self::index_store(&mut words, previous);
                }
                words
            });
            Self::index_store(words, &store);
            self.spill.push(store);
        }
        Ok(())
    }
}

/// Enter only on a declared boundary. Hold the RAM borrow through scatter
/// so checked destinations cannot be remapped after the calculation.
pub(crate) fn segment<const N: usize, M: SegmentMemory>(
    cpu: &mut runtime::CpuAbi,
    ram: &mut M,
    pc: u32,
    text: std::ops::Range<u32>,
    sites: &[u32],
    body: impl FnOnce(&mut runtime::CpuAbi, &mut Plan<'_, M, N>, u32) -> super::Result,
) -> Result<Option<runtime::TransferAbi>, GuestFault> {
    let mut log = Log::new();
    let Some(mut plan) = Plan::for_segment(ram, &mut log, cpu, pc, text, sites) else {
        return Ok(None);
    };
    let mut next = *cpu;
    let Ok(transfer) = body(&mut next, &mut plan, pc) else {
        return Ok(None);
    };
    plan.commit()?;
    *cpu = next;
    Ok(Some(transfer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::unit;
    const TEXT_START: u32 = crate::points::SIM_UNIT_POSE.base;
    const TEXT_LEN: u32 = 0x112dbb - TEXT_START;
    use runtime::{CpuAbi, guest::Permissions};

    #[test]
    fn overlapping_stores_forward_through_a_second_mapping() {
        let mut ram = Guest::new();
        ram.map(TEXT_START, TEXT_LEN, 0x4000, Permissions::RX)
            .unwrap();
        for addr in [0x10000, 0x20000] {
            ram.map(addr, 0x1000, 0, Permissions::RW).unwrap();
        }
        ram.map(0x30000, TEXT_LEN, 0x4000, Permissions::RW)
            .unwrap();
        let mut log = Log::new();
        let mut plan = Plan::<_, 64>::new(&mut ram, &mut log, TEXT_START, TEXT_START + TEXT_LEN).unwrap();
        assert!(plan.set_u8(0x30002, 0xf1).is_err());
        // A parent can spill into another mapping of its inlined leaf.
        plan.protect_inline_text(0x20080, 0x200c0);
        assert!(plan.set_u32(0x1007e, 0xf1).is_err());
        plan.set_u32(0x10002, 0x44332211).unwrap();
        plan.set_u8(0x20004, 0xfa).unwrap();
        assert_eq!(plan.u32(0x20001).unwrap(), 0xfa221100);
        assert_eq!(plan.u16(0x10004).unwrap(), 0x44fa);
        plan.commit().unwrap();
        assert_eq!(ram.u32(0x10002).unwrap(), 0x44fa2211);
    }

    #[test]
    fn a_plan_borrows_its_store_log_so_moving_it_copies_no_log() {
        // The manager's 128-store log is 2.5 KB; the plan is a few words.
        assert!(size_of::<Log<128>>() >= 128 * size_of::<Store>());
        assert!(size_of::<Plan<'static, Guest, 128>>() <= 256);
    }

    #[test]
    fn the_fast_page_table_gives_the_offset_the_mapping_walk_gives() {
        let layout = |ram: &mut Guest| {
            ram.map(0x10000, 0x2000, 0, Permissions::RW).unwrap();
            ram.map(0x20000, 0x1000, 0x2000, Permissions::READ).unwrap();
            ram.map(0x30000, 0x1000, 0x3000, Permissions::RX).unwrap();
            // An alias of the first page, and a partial page the table skips.
            ram.map(0x40000, 0x1000, 0, Permissions::RW).unwrap();
            ram.map(0x50800, 0x400, 0x4800, Permissions::RW).unwrap();
        };
        let (mut walked, mut paged) = (Guest::new(), Guest::new());
        layout(&mut walked);
        layout(&mut paged);
        paged.prepare_fast_pages();
        let mut fast = 0;
        for base in [0x10000u32, 0x11ffc, 0x20000, 0x30010, 0x40ffe, 0x50800, 0x50bfe, 0x60000] {
            for offset in [0u32, 1, 2, 4] {
                for len in [1usize, 2, 4, 8, 16] {
                    for access in [Access::Read, Access::Write, Access::Execute] {
                        for alignment in [1, 4] {
                            let addr = base + offset;
                            let want = walked.checked_offset(addr, len, access, alignment);
                            assert_eq!(walked.fast_physical(addr, len, access, alignment), None);
                            assert_eq!(
                                paged.checked_offset(addr, len, access, alignment),
                                want,
                                "{addr:#x} len {len} {access:?} align {alignment}"
                            );
                            fast += usize::from(paged.fast_physical(addr, len, access, alignment).is_some());
                        }
                    }
                }
            }
        }
        // The table answered the one-page accesses itself.
        assert!(fast > 100, "{fast}");
    }

    #[test]
    fn spilled_stores_forward_through_aliases_and_commit_in_order() {
        let mut ram = Guest::new();
        ram.map(TEXT_START, TEXT_LEN, 0x4000, Permissions::RX).unwrap();
        for va in [0x10000, 0x20000] {
            ram.map(va, 0x1000, 0, Permissions::RW).unwrap();
        }
        let mut log = Log::new();
        let mut plan = Plan::<_, 2>::new(&mut ram, &mut log, TEXT_START, TEXT_START + TEXT_LEN).unwrap();
        plan.allow_large_segment();
        plan.set_u32(0x10002, 0x44332211).unwrap();
        plan.set_u16(0x10005, 0xabcd).unwrap();
        plan.set_u32(0x20000, 0x76543210).unwrap();
        assert_eq!(plan.u32(0x20002).unwrap(), 0xcd337654);
        assert!(plan.unstaged_read(0x10001, 2).is_none());
        assert!(plan.unstaged_read(0x20004, 1).is_none());
        assert!(plan.unstaged_read(0x20007, 1).is_some());
        plan.set_u8(0x10003, 0x12).unwrap();
        assert_eq!(
            plan.final_bytes(),
            [(0, 0x10), (1, 0x32), (2, 0x54), (3, 0x12), (4, 0x33), (5, 0xcd), (6, 0xab)]
        );
        plan.commit().unwrap();
        assert_eq!(ram.u32(0x20002).unwrap(), 0xcd331254);
    }

    #[test]
    fn late_protected_push_discards_frame_store_and_cpu() {
        let mut ram = Guest::new();
        ram.map(TEXT_START, TEXT_LEN, 0x4000, Permissions::RX)
            .unwrap();
        let sp = 0x20000;
        ram.map(sp - 0x1000, 0x1000, 0, Permissions::READ).unwrap();
        ram.map(sp, 0x1000, 0x1000, Permissions::RW).unwrap();
        let mut cpu = CpuAbi {
            esp: sp,
            ebx: 0xdeadbeef,
            ..CpuAbi::default()
        };
        cpu.xmm[0] = [0x99; 16];
        let before = cpu;
        // After the second overlay weight lookup, the fraction store to +20
        // succeeds in the plan before the initializer's return push fails.
        assert_eq!(unit::run(&mut cpu, &mut ram, 0x1127f5).unwrap(), None);
        assert_eq!(cpu, before);
        assert_eq!(ram.u32(sp + 0x20).unwrap(), 0);
    }
}
