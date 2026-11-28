//! A sound's end tick: `start + duration_ticks(duration_ms)`, the engine's
//! own rounding (docs/engine/sound-instances.md).

use crate::events::TICKS_PER_SECOND;

/// The engine's `0.001f32` (`0x45dc70`): milliseconds to seconds.
const MS_TO_SECONDS: f32 = f32::from_bits(0x3a83_126f);

/// Ticks a sound of `duration_ms` runs, as the engine computes them
/// (`0x18a13b`, `0x18a1c0`): the `snd!` `+0x10` milliseconds as an `f32`,
/// times `0.001f32`, times the tick rate as an `f32`, then `fistp` (round to
/// nearest, ties to even). Not a ceiling: 1 ms is 0 ticks, 17 ms is 1.
pub fn duration_ticks(duration_ms: u32) -> u32 {
    let ms = duration_ms as i32 as f32;
    let ticks = (ms * MS_TO_SECONDS * TICKS_PER_SECOND as f32).round_ties_even();
    if ticks > 0.0 { ticks as u32 } else { 0 }
}

/// The tick a sound of `duration_ms` started on `start` ends on.
pub fn end_tick(start: u32, duration_ms: u32) -> u32 {
    start.saturating_add(duration_ticks(duration_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_tick_from_duration() {
        assert_eq!(end_tick(100, 0), 100);
        assert_eq!(end_tick(100, 1), 100);
        assert_eq!(end_tick(100, 16), 100);
        assert_eq!(end_tick(100, 17), 101);
        assert_eq!(end_tick(100, 51), 102);
        assert_eq!(end_tick(100, 352), 111);
        assert_eq!(end_tick(100, 1000), 130);
        assert_eq!(end_tick(u32::MAX - 1, 1000), u32::MAX);
    }
}
