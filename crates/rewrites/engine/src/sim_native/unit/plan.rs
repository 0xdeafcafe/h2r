//! Buffer one pose segment's stores until every input and destination is
//! known. Later loads see earlier stores, including through physical aliases.
use super::Memory;
use runtime::{
    Guest, GuestFault,
    guest::{Access, FaultReason},
};

/// Memory held exclusively throughout a segment. A successful write check
/// guarantees the same write will succeed at commit. Loads must be plain RAM.
pub trait PoseMemory: Memory {
    /// Physical start of a contiguous range with the requested access.
    fn checked_offset(&self, addr: u32, len: usize, access: Access, alignment: u32) -> Option<u32>;
}

impl PoseMemory for Guest {
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

// The decoded body ends just past its ret 16. Immediates and x87 FOP
// come from the text, so a writable alias into it needs the original.
const TEXT_START: u32 = crate::points::SIM_UNIT_POSE.base;
const TEXT_LEN: usize = (0x112dbb - TEXT_START) as usize;

pub(super) struct Plan<'a, M> {
    text: std::ops::Range<u64>,
    ram: &'a mut M,
    stores: [Store; 64],
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

impl<'a, M: PoseMemory> Plan<'a, M> {
    pub(super) fn new(ram: &'a mut M) -> Option<Self> {
        let text = u64::from(ram.checked_offset(TEXT_START, TEXT_LEN, Access::Read, 1)?);
        Some(Self {
            ram,
            text: text..text + TEXT_LEN as u64,
            stores: [Store::default(); 64],
            count: 0,
        })
    }

    pub(super) fn commit(self) -> Result<(), GuestFault> {
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

impl<M: PoseMemory> Memory for Plan<'_, M> {
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
        if start < self.text.end && self.text.start < start + src.len() as u64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::unit;
    use runtime::{CpuAbi, guest::Permissions};

    #[test]
    fn overlapping_stores_forward_through_a_second_mapping() {
        let mut ram = Guest::new();
        ram.map(TEXT_START, TEXT_LEN as u32, 0x4000, Permissions::RX)
            .unwrap();
        for addr in [0x10000, 0x20000] {
            ram.map(addr, 0x1000, 0, Permissions::RW).unwrap();
        }
        ram.map(0x30000, TEXT_LEN as u32, 0x4000, Permissions::RW)
            .unwrap();
        let mut plan = Plan::new(&mut ram).unwrap();
        assert!(plan.set_u8(0x30002, 0xf1).is_err());
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
        ram.map(TEXT_START, TEXT_LEN as u32, 0x4000, Permissions::RX)
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
