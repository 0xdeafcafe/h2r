//! Access to converted control state saved by the retail HaloScript VM.

use engine::memory::{GuestMemory, MemFault};

const GLOBALS: u32 = 0x004f_9380;
const GLOBAL_DATA: u32 = 0x44;
const VALUE_START: u32 = 0x41d;
pub(super) const STRIDE: u32 = 8;
const VALUE: u32 = 4;

pub(super) fn addr(m: &dyn GuestMemory, index: u32) -> Result<u32, MemFault> {
    let array = m.u32(GLOBALS)?;
    let data = m.u32(array + GLOBAL_DATA)?;
    Ok(data + (VALUE_START + index) * STRIDE + VALUE)
}

pub(super) fn boolean(m: &dyn GuestMemory, index: u16) -> Result<bool, MemFault> {
    m.u8(addr(m, u32::from(index))?).map(|value| value != 0)
}

pub(super) fn registry(
    m: &dyn GuestMemory,
    base: u32,
    words: u32,
) -> Result<Option<u32>, MemFault> {
    let Some(end) = base.checked_add(words) else {
        return Ok(None);
    };
    if end > 0xc00 - hs::vm::EXTERNAL_GLOBALS as u32 {
        return Ok(None);
    }
    let scenario = m.u32(super::points::SCENARIO.base)?;
    if scenario == 0 || end > m.u32(scenario + 0x1c0)? {
        return Ok(None);
    }
    Ok(Some(addr(m, base)?))
}

pub(super) fn hook_base(
    m: &dyn GuestMemory,
    kind: hs::h1_ai_slots::Registry,
) -> Result<Option<u32>, MemFault> {
    let Some(shift) = kind.hook_shift() else {
        return Ok(None);
    };
    let Some(index) = last_global_index(m, hs::h1_ai_slots::HOOK_STATE)? else {
        return Ok(None);
    };
    let base = (m.u32(addr(m, index)?)? >> shift) as u16;
    Ok((base != u16::MAX).then_some(u32::from(base)))
}

fn last_global_index(m: &dyn GuestMemory, name: &str) -> Result<Option<u32>, MemFault> {
    let scenario = m.u32(super::points::SCENARIO.base)?;
    if scenario == 0 {
        return Ok(None);
    }
    let count = m.u32(scenario + 0x1c0)?;
    let Some(index) = count.checked_sub(1) else {
        return Ok(None);
    };
    let definitions = m.u32(scenario + 0x1c4)?;
    let at = definitions + 40 * index;
    let mut label = [0; 32];
    m.read(at, &mut label)?;
    let nul = label
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(label.len());
    Ok((&label[..nul] == name.as_bytes()).then_some(index))
}
