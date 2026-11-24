//! Buffer one animation segment's stores until every input and destination is
//! known. Later loads see earlier stores, including through physical aliases.
use super::Memory;
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

// No store in the body is wider than a dword. Keeping the log on the
// stack avoids an allocation at every animation-manager boundary.
#[derive(Clone, Copy, Default)]
struct Store {
    addr: u32,
    physical: u32,
    bytes: [u8; 4],
    len: u8,
    alignment: u32,
}

pub(crate) struct Plan<'a, M, const N: usize> {
    text: std::ops::Range<u64>,
    inline_text: std::ops::Range<u64>,
    ram: &'a mut M,
    stores: [Store; N],
    count: usize,
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
    fn new(ram: &'a mut M, start: u32, end: u32) -> Option<Self> {
        let len = end.checked_sub(start)?;
        let text = u64::from(ram.checked_offset(start, len as usize, Access::Read, 1)?);
        Some(Self {
            ram,
            text: text..text + u64::from(len),
            inline_text: 0..0,
            stores: [Store::default(); N],
            count: 0,
        })
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

    fn commit(self) -> Result<(), GuestFault> {
        for store in &self.stores[..self.count] {
            self.ram.write_bytes(
                store.addr,
                &store.bytes[..usize::from(store.len)],
                store.alignment,
            )?;
        }
        Ok(())
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
        let start = u64::from(physical);
        let end = start + dst.len() as u64;
        for store in &self.stores[..self.count] {
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
        let slot = self.stores.get_mut(self.count).ok_or_else(err)?;
        let dst = slot.bytes.get_mut(..src.len()).ok_or_else(err)?;
        dst.copy_from_slice(src);
        slot.addr = addr;
        slot.physical = physical;
        slot.len = src.len() as u8;
        slot.alignment = alignment;
        self.count += 1;
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
    if !super::supported(cpu) || (pc != text.start && sites.binary_search(&pc).is_err()) {
        return Ok(None);
    }
    let Some(mut plan) = Plan::new(ram, text.start, text.end) else {
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
        let mut plan = Plan::<_, 64>::new(&mut ram, TEXT_START, TEXT_START + TEXT_LEN).unwrap();
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
