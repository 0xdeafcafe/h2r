//! Halo 1's flash interpolation (0x000e04b0). The tick and restart state
//! come from the corresponding Halo 2 builtin.

use engine::memory::{GuestMemory, MemFault};

pub(super) const GAME_TIME: u32 = 0x0051_0c54;
const STARTED: u32 = 0x1380;
const ENABLED: u32 = 0x1384;

// Halo 1 uses the single-precision constant at 0x1f0cbc as one turn.
#[allow(clippy::approx_constant)]
const TURN: f32 = 6.283;

pub(super) fn elapsed(mem: &dyn GuestMemory, start: u32) -> Result<f32, MemFault> {
    let time = mem.u32(GAME_TIME)?;
    let ticks = mem.u32(time + 8)?.wrapping_sub(start) as i32;
    Ok(ticks.max(0) as f32 * mem.f32(time + 4)?)
}

pub(super) fn strength(seconds: f32, repeat: f32, off: f32, on: f32, count: u16) -> f32 {
    let cycle = off + on;
    if !cycle.is_finite() || on <= 0.0 || off < 0.0 || count == 0 {
        return 0.0;
    }
    let elapsed = if repeat > 0.0 {
        seconds % repeat
    } else {
        seconds
    };
    if elapsed >= cycle * f32::from(count) {
        return 0.0;
    }
    let phase = elapsed % cycle;
    if phase >= on {
        return 0.0;
    }
    (0.5 * (1.0 - (phase / on * TURN).cos()))
        .clamp(0.0, 1.0)
        .sqrt()
}

fn components(argb: u32) -> [f32; 4] {
    argb.to_be_bytes().map(|byte| f32::from(byte) / 255.0)
}

pub(super) fn flash_colour(record: &[u8; 24], elapsed: Option<f32>) -> [f32; 4] {
    let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().expect("word"));
    let primary = components(word(0));
    let alternate = components(word(4));
    let reversed = record[0x12] & 1 != 0;
    let Some(elapsed) = elapsed else {
        return if reversed { alternate } else { primary };
    };
    let weight = record_strength(record, elapsed);
    let (from, to) = if reversed {
        (alternate, primary)
    } else {
        (primary, alternate)
    };
    std::array::from_fn(|i| from[i] + weight * (to[i] - from[i]))
}

fn record_strength(record: &[u8; 24], elapsed: f32) -> f32 {
    let float = |at: usize| f32::from_le_bytes(record[at..at + 4].try_into().expect("float"));
    strength(
        elapsed,
        float(8),
        float(0xc),
        float(0x14),
        u16::from_le_bytes([record[0x10], record[0x11]]),
    )
}

pub(super) fn help_colour(mem: &dyn GuestMemory, tag: u32, hud: u32) -> Result<[f32; 4], MemFault> {
    let mut record = [0; 24];
    mem.read(tag + h1text::hud::HELP_FLASH, &mut record)?;
    let seconds = if mem.u8(hud + ENABLED)? != 0 {
        Some(elapsed(mem, mem.u32(hud + STARTED)?)?)
    } else {
        None
    };
    Ok(flash_colour(&record, seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_flash_peaks_then_returns_to_base() {
        let mut record = [0; 24];
        record[0..4].copy_from_slice(&0xff00_0000u32.to_le_bytes());
        record[4..8].copy_from_slice(&0xffff_0000u32.to_le_bytes());
        record[8..12].copy_from_slice(&2.0f32.to_le_bytes());
        record[12..16].copy_from_slice(&0.25f32.to_le_bytes());
        record[16..18].copy_from_slice(&2u16.to_le_bytes());
        record[20..24].copy_from_slice(&0.25f32.to_le_bytes());

        assert_eq!(flash_colour(&record, None), [1.0, 0.0, 0.0, 0.0]);
        assert!(flash_colour(&record, Some(0.125))[1] > 0.999);
        assert_eq!(flash_colour(&record, Some(0.35))[1], 0.0);
        assert!(flash_colour(&record, Some(0.625))[1] > 0.999);
        assert_eq!(flash_colour(&record, Some(1.1))[1], 0.0);
        assert!(flash_colour(&record, Some(2.125))[1] > 0.999);
        record[0x12] = 1;
        assert_eq!(flash_colour(&record, None)[1], 1.0);
        assert!(flash_colour(&record, Some(0.125))[1] < 0.001);
    }
}
