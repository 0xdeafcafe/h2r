//! A `hudg` flash record and its curve (docs/game/hud-kit.md, "Colours"):
//! Halo 1's interpolation (`0x000e04b0`), which Halo 2's help text is
//! drawn with on converted maps (`game::h1_hud_text::flash`). That Halo
//! 2's own curve is the same is S.
//!
//! A record is 24 bytes: the rest colour and the peak colour (packed
//! ARGB) at `+0` and `+4`, the period `+8`, the delay `+0xc`, the count
//! `+0x10` (`i16`), the flags `+0x12` (bit 0: the flash runs from the peak
//! to the rest colour) and one flash's duration `+0x14`. Where each
//! [`Pulse`]'s record starts in the `hudg` comes from the `hudg`
//! definition ([`record_at`]), so no offset is written twice.

use map::FourCC;
use tags::defs::Defs;

use crate::model::Pulse;

/// A flash record's bytes.
pub const RECORD_LEN: usize = 24;
/// Bit 0 of the flags at `+0x12`: from the peak to the rest colour.
pub const REVERSED: u8 = 1;

// Halo 1 uses the single-precision constant at 0x1f0cbc as one turn.
#[allow(clippy::approx_constant)]
const TURN: f32 = 6.283;

/// The `hudg` field each pulse's record starts at (its rest colour).
/// Help's is unnamed in the retail schema (`word_a4`); the help line
/// reads it as the help colour (E `0x0024d0a6`).
pub fn rest_field(p: Pulse) -> &'static str {
    match p {
        Pulse::Help => "word_a4",
        Pulse::Objective => "objective_rest_colour",
        Pulse::TimeWarning => "time_warning_rest_colour",
        Pulse::TimeExpired => "time_expired_rest_colour",
    }
}

/// Where `p`'s record starts in a `hudg`, from its definition.
pub fn record_at(p: Pulse) -> Option<u32> {
    Defs::builtin()
        .group(FourCC(*b"hudg"))?
        .field(rest_field(p))
        .map(|f| f.offset)
}

/// How far into its flash a text is, 0 (the start colour) to 1 (the other
/// colour), `seconds` after the flash started: a raised cosine over each
/// flash of `on` seconds, `off` seconds apart, `count` flashes, the whole
/// again every `repeat` seconds (none when 0).
pub fn strength(seconds: f32, repeat: f32, off: f32, on: f32, count: u16) -> f32 {
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

/// A packed ARGB colour as four floats, `[a, r, g, b]`, the order the
/// text setup takes them in (`HUD_TEXT_SETUP`'s colour).
pub fn components(argb: u32) -> [f32; 4] {
    argb.to_be_bytes().map(|byte| f32::from(byte) / 255.0)
}

/// The record's colour `elapsed` seconds into its flash, `[a, r, g, b]`;
/// with no flash running, its start colour.
pub fn colour(record: &[u8; RECORD_LEN], elapsed: Option<f32>) -> [f32; 4] {
    let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().expect("word"));
    let primary = components(word(0));
    let alternate = components(word(4));
    let reversed = record[0x12] & REVERSED != 0;
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

fn record_strength(record: &[u8; RECORD_LEN], elapsed: f32) -> f32 {
    let float = |at: usize| f32::from_le_bytes(record[at..at + 4].try_into().expect("float"));
    strength(
        elapsed,
        float(8),
        float(0xc),
        float(0x14),
        u16::from_le_bytes([record[0x10], record[0x11]]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> [u8; RECORD_LEN] {
        let mut r = [0; RECORD_LEN];
        r[0..4].copy_from_slice(&0xff00_0000u32.to_le_bytes());
        r[4..8].copy_from_slice(&0xffff_0000u32.to_le_bytes());
        r[8..12].copy_from_slice(&2.0f32.to_le_bytes());
        r[12..16].copy_from_slice(&0.25f32.to_le_bytes());
        r[16..18].copy_from_slice(&2u16.to_le_bytes());
        r[20..24].copy_from_slice(&0.25f32.to_le_bytes());
        r
    }

    #[test]
    fn a_flash_peaks_mid_flash_and_rests_between() {
        let r = record();
        assert_eq!(colour(&r, None), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(colour(&r, Some(0.0))[1], 0.0, "the start is the rest");
        assert!(colour(&r, Some(0.125))[1] > 0.999, "the first peak");
        assert_eq!(colour(&r, Some(0.35))[1], 0.0, "the delay");
        assert!(colour(&r, Some(0.625))[1] > 0.999, "the second peak");
        assert_eq!(colour(&r, Some(1.1))[1], 0.0, "the count is done");
        assert!(colour(&r, Some(2.125))[1] > 0.999, "the period repeats");
    }

    #[test]
    fn a_reversed_flash_runs_from_the_peak() {
        let mut r = record();
        r[0x12] = REVERSED;
        assert_eq!(colour(&r, None)[1], 1.0);
        assert!(colour(&r, Some(0.125))[1] < 0.001);
    }

    #[test]
    fn a_record_with_no_flashes_or_no_duration_stays_at_rest() {
        assert_eq!(strength(0.1, 2.0, 0.25, 0.25, 0), 0.0);
        assert_eq!(strength(0.1, 2.0, 0.25, 0.0, 2), 0.0);
        assert_eq!(strength(0.1, 2.0, f32::NAN, 0.25, 2), 0.0);
    }

    #[test]
    fn each_pulses_record_is_laid_out_as_the_curve_reads_it() {
        let def = Defs::builtin().group(FourCC(*b"hudg")).expect("hudg");
        let at = |name: &str| def.field(name).map(|f| f.offset);
        for (p, prefix) in [
            (Pulse::Help, "help"),
            (Pulse::Objective, "objective"),
            (Pulse::TimeWarning, "time_warning"),
            (Pulse::TimeExpired, "time_expired"),
        ] {
            let start = record_at(p).expect("the record");
            for (field, offset) in [
                ("pulse_colour", 4),
                ("pulse_period", 8),
                ("pulse_delay", 0xc),
                ("pulse_count", 0x10),
                ("pulse_flags", 0x12),
                ("pulse_duration", 0x14),
            ] {
                assert_eq!(
                    at(&format!("{prefix}_{field}")),
                    Some(start + offset),
                    "{} {field}",
                    p.name()
                );
            }
        }
    }
}
