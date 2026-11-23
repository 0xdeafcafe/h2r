//! Halo 1's flash interpolation (0x000e04b0), which is
//! `hud_kit_core::flash`'s. The tick and restart state come from the
//! corresponding Halo 2 builtin.

use engine::memory::{GuestMemory, MemFault};

pub(super) const GAME_TIME: u32 = 0x0051_0c54;
const STARTED: u32 = 0x1380;
const ENABLED: u32 = 0x1384;

pub(super) use hud_kit_core::flash::colour as flash_colour;

pub(super) fn help_colour(mem: &dyn GuestMemory, tag: u32, hud: u32) -> Result<[f32; 4], MemFault> {
    let mut record = [0; hud_kit_core::flash::RECORD_LEN];
    mem.read(tag + h1_contract::hud::HELP_FLASH, &mut record)?;
    let seconds = if mem.u8(hud + ENABLED)? != 0 {
        Some(elapsed(mem, mem.u32(hud + STARTED)?)?)
    } else {
        None
    };
    Ok(flash_colour(&record, seconds))
}
