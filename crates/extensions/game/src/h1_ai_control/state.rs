//! Access to converted control state saved by the retail HaloScript VM.

use guest_layout::memory::{GuestMemory, MemFault};

const GLOBALS: u32 = 0x004f_9380;
const GLOBAL_DATA: u32 = 0x44;
pub(super) const VALUE_START: u32 = 0x41d;
pub(super) const STRIDE: u32 = 8;
const VALUE: u32 = 4;

pub(super) fn clock(m: &dyn GuestMemory) -> Result<Option<(u32, u32)>, MemFault> {
    let clock = m.u32(super::points::GAME_TIME.base)?;
    if clock == 0 {
        return Ok(None);
    }
    let rate = m.u16(clock + 2)?;
    if rate == 0 {
        return Ok(None);
    }
    Ok(Some((m.u32(clock + 8)?, u32::from(rate))))
}

pub(super) fn authored_tick(tick: u32, rate: u32) -> u32 {
    let hz = u64::from(hs::h1_ai_slots::timers::TICKS_PER_SECOND);
    (u64::from(tick) * hz / u64::from(rate)) as u32
}

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
    let pool = m.u32(GLOBALS)?;
    if pool == 0 || end > m.u32(pool + guest_layout::datum::layout::MAXIMUM_COUNT)?.saturating_sub(VALUE_START) {
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
    let trailing = teleport_trailer(m)? + u32::from(timers_base(m)?.is_some());
    let index = global_index_from_end(m, hs::h1_ai_slots::HOOK_STATE, trailing)?;
    let Some(index) = index else {
        return Ok(None);
    };
    let base = (m.u32(addr(m, index)?)? >> shift) as u16;
    Ok((base != u16::MAX).then_some(u32::from(base)))
}

pub(super) fn timers_base(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    let trailing = teleport_trailer(m)?;
    let index = match global_index_from_end(m, hs::h1_ai_slots::timers::PHASED_GLOBAL, trailing)? {
        Some(index) => Some(index),
        None => global_index_from_end(m, hs::h1_ai_slots::timers::GLOBAL, trailing)?,
    };
    index.map(|index| m.u32(addr(m, index)?)).transpose()
}

pub(super) fn phased_timers(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    Ok(global_index_from_end(m, hs::h1_ai_slots::timers::PHASED_GLOBAL, teleport_trailer(m)?)?.is_some())
}

fn teleport_trailer(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    let trailing = u32::from(perception_global(m)?.is_some());
    Ok(trailing + u32::from(global_index_from_end(m, hs::h1_ai_slots::TELEPORT_STATE, trailing)?.is_some()))
}

pub(super) fn teleport_base(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    global_index_from_end(m, hs::h1_ai_slots::TELEPORT_STATE, u32::from(perception_global(m)?.is_some()))?
        .map(|index| m.u32(addr(m, index)?))
        .transpose()
}

pub(super) fn perception_global(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    global_index_from_end(m, hs::h1_ai_slots::PERCEPTION_STATE, 0)
}

pub(super) fn script_strings(m: &dyn GuestMemory, offset: u32) -> Result<Option<(u32, u32)>, MemFault> {
    let scenario = m.u32(super::points::SCENARIO.base)?;
    if scenario == 0 {
        return Ok(None);
    }
    let len = m.u32(scenario + 0x1b0)?;
    let start = m.u32(scenario + 0x1b4)?;
    Ok((start != 0 && offset < len && start.checked_add(len).is_some())
        .then(|| (start + offset, len - offset)))
}

fn global_index_from_end(m: &dyn GuestMemory, name: &str, trailing: u32) -> Result<Option<u32>, MemFault> {
    let scenario = m.u32(super::points::SCENARIO.base)?;
    if scenario == 0 {
        return Ok(None);
    }
    let count = m.u32(scenario + 0x1c0)?;
    let Some(index) = count.checked_sub(1 + trailing) else {
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
