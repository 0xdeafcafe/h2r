//! Checked virtual mappings over one 64 MB physical store. Physical offsets
//! identify aliases; high virtual addresses are never masked into RAM.

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

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MapError {
    #[error("invalid virtual or physical mapping range")]
    InvalidRange,
    #[error("overlapping virtual mapping")]
    Overlap,
    #[error("64 MB physical memory exhausted")]
    OutOfMemory,
}

pub struct Guest {
    ram: Box<[u8]>,
    mappings: Vec<Mapping>,
    next_physical: u32,
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
            next_physical: 0,
        }
    }
    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
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
        let end = va as u64 + len as u64;
        if len == 0
            || end > 1u64 << 32
            || physical as u64 + len as u64 > RAM_SIZE as u64
            || ((va as u64) < MMIO_END && end > MMIO_START)
        {
            return Err(MapError::InvalidRange);
        }
        if self.mappings.iter().any(|m| {
            (va as u64) < m.virtual_start as u64 + m.len as u64 && end > m.virtual_start as u64
        }) {
            return Err(MapError::Overlap);
        }
        self.mappings.push(Mapping {
            virtual_start: va,
            len,
            physical_start: physical,
            permissions,
        });
        self.mappings.sort_unstable_by_key(|m| m.virtual_start);
        self.next_physical = self.next_physical.max(physical + len);
        Ok(())
    }

    /// Allocate distinct page-aligned physical storage for a virtual range.
    pub fn map_zeroed(
        &mut self,
        va: u32,
        len: u32,
        permissions: Permissions,
    ) -> Result<(), MapError> {
        let physical = self
            .next_physical
            .checked_add(PAGE_SIZE - 1)
            .ok_or(MapError::OutOfMemory)?
            & !(PAGE_SIZE - 1);
        if physical as u64 + len as u64 > RAM_SIZE as u64 {
            return Err(MapError::OutOfMemory);
        }
        self.map(va, len, physical, permissions)
    }
    pub fn protect(&mut self, va: u32, permissions: Permissions) -> Result<(), MapError> {
        self.mappings
            .iter_mut()
            .find(|m| m.virtual_start == va)
            .ok_or(MapError::InvalidRange)?
            .permissions = permissions;
        Ok(())
    }

    fn mapping_index(&self, addr: u64) -> Option<usize> {
        let i = self
            .mappings
            .partition_point(|m| m.virtual_start as u64 <= addr)
            .checked_sub(1)?;
        let m = &self.mappings[i];
        (addr < m.virtual_start as u64 + m.len as u64).then_some(i)
    }

    fn span(&self, index: usize, addr: u64, end: u64) -> std::ops::Range<usize> {
        let m = &self.mappings[index];
        let start = (m.physical_start as u64 + addr - m.virtual_start as u64) as usize;
        start..start + (end.min(m.virtual_start as u64 + m.len as u64) - addr) as usize
    }

    /// Validate once, then copy without allocating. Adjacent mappings use a
    /// second linear walk; the usual one-mapping access needs one binary search.
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
            let n = span.len();
            self.ram[span].copy_from_slice(&value[offset..offset + n]);
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
