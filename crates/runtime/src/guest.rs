//! Flat guest address space. The Xbox has 64 MB of unified RAM, and this is
//! the one place that does raw byte access on it.

/// Retail Xbox RAM.
pub const RAM_SIZE: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("guest access out of range: {addr:#010x}+{len}")]
pub struct Fault {
    pub addr: u32,
    pub len: usize,
}

pub struct Guest {
    ram: Box<[u8]>,
}

impl Default for Guest {
    fn default() -> Self {
        Self::new()
    }
}

impl Guest {
    pub fn new() -> Self {
        Self { ram: vec![0; RAM_SIZE].into_boxed_slice() }
    }

    fn range(&self, addr: u32, len: usize) -> Result<std::ops::Range<usize>, Fault> {
        let start = addr as usize;
        let end = start.checked_add(len).filter(|e| *e <= self.ram.len());
        end.map(|end| start..end).ok_or(Fault { addr, len })
    }

    pub fn bytes(&self, addr: u32, len: usize) -> Result<&[u8], Fault> {
        Ok(&self.ram[self.range(addr, len)?])
    }

    pub fn bytes_mut(&mut self, addr: u32, len: usize) -> Result<&mut [u8], Fault> {
        let r = self.range(addr, len)?;
        Ok(&mut self.ram[r])
    }

    pub fn read<const N: usize>(&self, addr: u32) -> Result<[u8; N], Fault> {
        Ok(self.bytes(addr, N)?.try_into().unwrap())
    }

    pub fn write<const N: usize>(&mut self, addr: u32, v: [u8; N]) -> Result<(), Fault> {
        self.bytes_mut(addr, N)?.copy_from_slice(&v);
        Ok(())
    }

    pub fn u8(&self, addr: u32) -> Result<u8, Fault> {
        Ok(self.read::<1>(addr)?[0])
    }
    pub fn u16(&self, addr: u32) -> Result<u16, Fault> {
        self.read(addr).map(u16::from_le_bytes)
    }
    pub fn u32(&self, addr: u32) -> Result<u32, Fault> {
        self.read(addr).map(u32::from_le_bytes)
    }
    pub fn f32(&self, addr: u32) -> Result<f32, Fault> {
        self.read(addr).map(f32::from_le_bytes)
    }
    pub fn set_u8(&mut self, addr: u32, v: u8) -> Result<(), Fault> {
        self.write(addr, [v])
    }
    pub fn set_u16(&mut self, addr: u32, v: u16) -> Result<(), Fault> {
        self.write(addr, v.to_le_bytes())
    }
    pub fn set_u32(&mut self, addr: u32, v: u32) -> Result<(), Fault> {
        self.write(addr, v.to_le_bytes())
    }
    pub fn set_f32(&mut self, addr: u32, v: f32) -> Result<(), Fault> {
        self.write(addr, v.to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_little_endian() {
        let mut g = Guest::new();
        g.set_u32(0x1000, 0xDEAD_BEEF).unwrap();
        assert_eq!(g.u8(0x1000).unwrap(), 0xEF);
        assert_eq!(g.u32(0x1000).unwrap(), 0xDEAD_BEEF);
        g.set_f32(0x2000, 1.5).unwrap();
        assert_eq!(g.f32(0x2000).unwrap(), 1.5);
    }

    #[test]
    fn faults_past_end() {
        let g = Guest::new();
        let last = (RAM_SIZE - 2) as u32;
        assert_eq!(g.u32(last), Err(Fault { addr: last, len: 4 }));
        assert_eq!(g.u32(u32::MAX), Err(Fault { addr: u32::MAX, len: 4 }));
    }
}
