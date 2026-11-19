//! Access to converted control state saved by the retail HaloScript VM.

use engine::memory::{GuestMemory, MemFault};

const GLOBALS: u32 = 0x004f_9380;
const GLOBAL_DATA: u32 = 0x44;
const VALUE_START: u32 = 0x41d;
const VALUE_STRIDE: u32 = 8;
const VALUE: u32 = 4;

pub(super) fn addr(m: &dyn GuestMemory, index: u32) -> Result<u32, MemFault> {
    let array = m.u32(GLOBALS)?;
    let data = m.u32(array + GLOBAL_DATA)?;
    Ok(data + (VALUE_START + index) * VALUE_STRIDE + VALUE)
}

pub(super) fn boolean(m: &dyn GuestMemory, index: u16) -> Result<bool, MemFault> {
    m.u8(addr(m, u32::from(index))?).map(|value| value != 0)
}
