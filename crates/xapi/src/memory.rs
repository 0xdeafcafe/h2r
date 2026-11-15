//! Checked guest memory. Validate the entire range before copying, reject
//! overflow and unmapped/protected bytes, and never partially write. A successful
//! check must guarantee subsequent access under the same memory guard succeeds.
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryFault {
    pub address: u32,
    pub length: usize,
    pub write: bool,
}

pub trait GuestMemory {
    fn check(&self, address: u32, length: usize, write: bool) -> Result<()>;
    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<()>;
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<()>;
}

pub fn offset(address: u32, delta: usize) -> Result<u32> {
    address
        .checked_add(u32::try_from(delta).map_err(|_| Error::INVALID_PARAMETER)?)
        .ok_or(Error::Memory(MemoryFault {
            address,
            length: delta,
            write: false,
        }))
}

pub fn read<const N: usize>(memory: &impl GuestMemory, address: u32) -> Result<[u8; N]> {
    memory.check(address, N, false)?;
    let mut bytes = [0; N];
    memory.read(address, &mut bytes)?;
    Ok(bytes)
}

pub fn u32_at(memory: &impl GuestMemory, address: u32) -> Result<u32> {
    Ok(u32::from_le_bytes(read(memory, address)?))
}

pub fn ansi(memory: &impl GuestMemory, address: u32, limit: usize) -> Result<String> {
    if address == 0 {
        return Err(Error::INVALID_PARAMETER);
    }
    let mut bytes = Vec::new();
    for index in 0..limit {
        let byte = read::<1>(memory, offset(address, index)?)?[0];
        if byte == 0 {
            return String::from_utf8(bytes).map_err(|_| Error::INVALID_PARAMETER);
        }
        if !byte.is_ascii() {
            return Err(Error::INVALID_PARAMETER);
        }
        bytes.push(byte);
    }
    Err(Error::INVALID_PARAMETER)
}
