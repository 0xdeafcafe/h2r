//! The two distance words at shader context +0x28 and +0x2c.
//! Read the scale factors from the live image. The first word uses SSE
//! rounding; the second passes through x87 extended precision.

use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;

const DRAW_DISTANCE_SCALE: u32 = 0x0046_70e4;
const BYTE_SCALE: u32 = 0x0045_dc10;
const THRESHOLD: u32 = 0x0045_dbd8;
const RECIPROCAL_NUMERATOR: u32 = 0x0045_dbc0;
const DEFAULT_DISTANCE: u32 = 0x0045_de10;
const LOG_SCALE: u32 = 0x0044_a04c;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ProjectedDistance {
    fade: u8,
    part_scale: u32,
    draw_scale: u32,
    byte_scale: u32,
    threshold: u32,
    numerator: u32,
    default: u32,
    log_scale: u32,
    fcw: u16,
    mxcsr: u32,
}

impl ProjectedDistance {
    /// Includes every input to the calculation so two draws can share the
    /// expensive extended-precision logarithm only for bit-equal operands.
    pub fn read(m: &dyn GuestMemory, part: u32, fade: u8, fcw: u16, mxcsr: u32)
        -> Result<Self, Error> {
        Ok(Self { fade, part_scale: m.u32(part.wrapping_add(0x2c))?,
            draw_scale: m.u32(DRAW_DISTANCE_SCALE)?, byte_scale: m.u32(BYTE_SCALE)?,
            threshold: m.u32(THRESHOLD)?, numerator: m.u32(RECIPROCAL_NUMERATOR)?,
            default: m.u32(DEFAULT_DISTANCE)?, log_scale: m.u32(LOG_SCALE)?, fcw, mxcsr })
    }

    pub fn words(self) -> Result<[u32; 2], Error> {
        let mut sse = Sse { mxcsr: self.mxcsr };
        let weight = sse.int(i32::from(self.fade))?;
        let weight = sse.mul(weight, self.part_scale)?;
        let weight = sse.mul(weight, self.draw_scale)?;
        let weight = sse.mul(weight, self.byte_scale)?;
        let value = if sse.above(weight, self.threshold)? {
            sse.div(self.numerator, weight)?
        } else {
            self.default
        };
        let mut x = X87::new(self.fcw);
        x.fld(value)?;
        X87::ok(x87::abi::ld_const(&mut x.0, x87::Const::Ln2 as u32))?;
        x.fxch(1)?;
        x.unary(x87::abi::unary_op::YL2X)?;
        x.fmul_m(self.log_scale)?;
        Ok([value, x.fst_m32()?])
    }
}

/// Signed bitmap frame converted by the instance texture rebinding loop.
pub fn bitmap_frame(frame: i32, mxcsr: u32) -> Result<u32, Error> {
    Sse { mxcsr }.int(frame)
}

/// A record's polygon stipple change around the emitter (0x4d640,
/// 0x4d720). `enter` selects the per-record fade. Leaving restores one.
/// A comparison within tolerance preserves both the device pattern and
/// the exact bits of the engine's cached fade.
pub fn stipple(m: &dyn GuestMemory, byte: u8, enter: bool, fcw: u16, mxcsr: u32)
    -> Result<Option<(u32, u32)>, Error> {
    const FADE_SCALE: u32 = 0x0045_dbb4;
    const EPSILON: u32 = 0x0045_dbdc;
    const TABLE_SCALE: u32 = 0x0044_541c;
    const TABLE: u32 = 0x0048_7288;
    if byte == 0xff { return Ok(None); }
    let mut x = X87::new(fcw);
    let fade = if enter {
        x.fild(i32::from(byte))?;
        x.fmul_m(m.u32(FADE_SCALE)?)?;
        x.fstp_m32()?
    } else {
        m.u32(crate::light_constants::constant::ONE)?
    };
    x.fld(m.u32(CACHED_FADE)?)?;
    x.arith_m(x87::abi::arith_op::SUB, fade)?;
    x.unary(x87::abi::unary_op::ABS)?;
    x.fld(m.u32(EPSILON)?)?;
    let flags = x.fcomip(1)?;
    x.fstp_st(0)?;
    if flags & 0x41 == 0 { return Ok(None); }
    let scaled = Sse { mxcsr }.mul(fade, m.u32(TABLE_SCALE)?)?;
    x.fld(scaled)?;
    let (code, index) = x87::abi::st_i32(&x.0, 0);
    X87::ok(code)?;
    Ok(Some((fade, TABLE + 0x80 * index.value.clamp(0, 16) as u32)))
}

/// The 32-row pattern a fade byte selects from the engine's table
/// (0x487288, 17 patterns of 0x80 bytes), by the same arithmetic
/// [`stipple`] uses, without its device-cache comparison: the pattern
/// address, or `None` for a solid byte (0xff), which sets no pattern.
pub fn stipple_table(m: &dyn GuestMemory, byte: u8, fcw: u16, mxcsr: u32) -> Result<Option<u32>, Error> {
    const FADE_SCALE: u32 = 0x0045_dbb4;
    const TABLE_SCALE: u32 = 0x0044_541c;
    const TABLE: u32 = 0x0048_7288;
    if byte == 0xff { return Ok(None); }
    let mut x = X87::new(fcw);
    x.fild(i32::from(byte))?;
    x.fmul_m(m.u32(FADE_SCALE)?)?;
    let fade = x.fstp_m32()?;
    let scaled = Sse { mxcsr }.mul(fade, m.u32(TABLE_SCALE)?)?;
    x.fld(scaled)?;
    let (code, index) = x87::abi::st_i32(&x.0, 0);
    X87::ok(code)?;
    Ok(Some(TABLE + 0x80 * index.value.clamp(0, 16) as u32))
}

/// The pattern a record's epilogue (`0x0004d720`) restores: the table
/// entry for the engine's one (`0x0045dbc0`), by [`stipple`]'s leave
/// arithmetic under each of the four x87 rounding modes, so the caller
/// need not know the guest's control word. `None` when the modes pick
/// different entries (the scaled one is not a whole number) or the
/// arithmetic does not run.
pub fn rest_stipple_table(m: &dyn GuestMemory, mxcsr: u32) -> Option<u32> {
    fade_stipple_table(m, m.u32(crate::light_constants::constant::ONE).ok()?, mxcsr)
}

/// The table entry the pattern for the cached fade `fade` (f32 bits, as
/// `0x004b8494` holds it) is: [`stipple`]'s lookup (`fade` times the
/// table scale, converted to an integer, held to 0..=16) under each of
/// the four x87 rounding modes. `None` when the modes pick different
/// entries or the arithmetic does not run.
pub fn fade_stipple_table(m: &dyn GuestMemory, fade: u32, mxcsr: u32) -> Option<u32> {
    const TABLE_SCALE: u32 = 0x0044_541c;
    const TABLE: u32 = 0x0048_7288;
    let scaled = Sse { mxcsr }.mul(fade, m.u32(TABLE_SCALE).ok()?).ok()?;
    let mut picked = None;
    for rounding in 0..4u16 {
        let mut x = X87::new(0x027f | rounding << 10);
        x.fld(scaled).ok()?;
        let (code, index) = x87::abi::st_i32(&x.0, 0);
        X87::ok(code).ok()?;
        let at = TABLE + 0x80 * index.value.clamp(0, 16) as u32;
        if picked.is_some_and(|p| p != at) {
            return None;
        }
        picked = Some(at);
    }
    picked
}

pub const CACHED_FADE: u32 = 0x004b_8494;

/// The render records (`0x004ba138`, `0x20` bytes each): a record's fade
/// byte is at `+0x1a`.
pub const RENDER_RECORDS: u32 = 0x004b_a138;
pub const RENDER_RECORD_SIZE: u32 = 0x20;
pub const RECORD_FADE: u32 = 0x1a;

/// Render record `record`'s fade byte (`+0x1a`).
pub fn record_fade(m: &dyn GuestMemory, record: u16) -> Option<u8> {
    m.u8(RENDER_RECORDS + RENDER_RECORD_SIZE * u32::from(record) + RECORD_FADE).ok()
}

/// The 32 stipple rows a record with fade byte `byte` draws with, from
/// the state its epilogue (`0x0004d720`) leaves: the device holding
/// `rest` and the engine's cached fade its one. The prologue
/// (`0x0004d640`, [`stipple`]) sets the byte's table pattern, or keeps
/// `rest` when the fade is within the cache's tolerance of one (or the
/// byte is solid). Run under every x87 control word the guest may hold
/// (53- and 64-bit precision, the four rounding modes), so the caller
/// need not know it; `None` when they pick different patterns or the
/// arithmetic does not run.
pub fn faded_rows(m: &dyn GuestMemory, byte: u8, rest: &[u32; 32], mxcsr: u32) -> Option<[u32; 32]> {
    let mut picked = None;
    for fcw in [0x027f_u16, 0x037f].into_iter().flat_map(|p| (0..4u16).map(move |r| p | r << 10)) {
        let at = stipple(m, byte, true, fcw, mxcsr).ok()?.map(|(_, at)| at);
        if picked.is_some_and(|p| p != at) {
            return None;
        }
        picked = Some(at);
    }
    let Some(at) = picked? else { return Some(*rest) };
    let mut rows = [0; 32];
    for (i, row) in rows.iter_mut().enumerate() {
        *row = m.u32(at + 4 * i as u32).ok()?;
    }
    Some(rows)
}

#[cfg(test)]
mod stipple_tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    /// The entry a cached fade selects: the rest entry for the engine's
    /// one, and none where the rounding modes disagree.
    #[test]
    fn a_cached_fade_selects_its_table_entry_in_every_rounding_mode() {
        let mut m = FakeGuest::new();
        m.put_f32(0x0044_541c, 16.0).put_f32(crate::light_constants::constant::ONE, 1.0);
        assert_eq!(fade_stipple_table(&m, 1.0f32.to_bits(), 0x1f80), Some(0x0048_7288 + 0x80 * 16));
        assert_eq!(fade_stipple_table(&m, 0.5f32.to_bits(), 0x1f80), Some(0x0048_7288 + 0x80 * 8));
        assert_eq!(fade_stipple_table(&m, 2.0f32.to_bits(), 0x1f80), Some(0x0048_7288 + 0x80 * 16), "held to 16");
        assert_eq!(rest_stipple_table(&m, 0x1f80), fade_stipple_table(&m, 1.0f32.to_bits(), 0x1f80));
        m.put_f32(0x0044_541c, 2.0);
        assert_eq!(fade_stipple_table(&m, 1.25f32.to_bits(), 0x1f80), None, "2.5 rounds two ways");
    }

    #[test]
    fn record_fade_and_tail_use_their_own_threshold_and_rounding() {
        let mut m = FakeGuest::new();
        // Deliberately synthetic factors. The product 2.5 rounds to 2
        // under nearest-even and 3 under round-up at the stipple lookup.
        m.put_f32(0x0045_dbb4, 0.25).put_f32(0x0045_dbdc, 0.02)
            .put_f32(0x0044_541c, 2.0).put_f32(CACHED_FADE, 0.0)
            .put_f32(crate::light_constants::constant::ONE, 1.0);
        assert_eq!(stipple(&m, 5, true, 0x037f, 0x1f80).unwrap(), Some((1.25f32.to_bits(), 0x0048_7388)));
        assert_eq!(stipple(&m, 5, true, 0x0b7f, 0x1f80).unwrap(), Some((1.25f32.to_bits(), 0x0048_7408)));
        m.put_f32(CACHED_FADE, 1.24);
        assert_eq!(stipple(&m, 5, true, 0x037f, 0x1f80).unwrap(), None);
        assert_eq!(stipple(&m, 5, false, 0x037f, 0x1f80).unwrap(), Some((1.0f32.to_bits(), 0x0048_7388)));
        assert_eq!(stipple(&m, 0xff, true, 0x037f, 0x1f80).unwrap(), None);
        // The table lookup alone agrees, whatever the device cache holds.
        assert_eq!(stipple_table(&m, 5, 0x037f, 0x1f80).unwrap(), Some(0x0048_7388));
        assert_eq!(stipple_table(&m, 5, 0x0b7f, 0x1f80).unwrap(), Some(0x0048_7408));
        assert_eq!(stipple_table(&m, 0xff, 0x037f, 0x1f80).unwrap(), None);
    }

    /// The pattern a record's epilogue restores is the table entry for
    /// one, the same under every rounding mode, and the one a leave from
    /// another fade sets; a scale that makes the modes disagree gives
    /// none.
    #[test]
    fn the_rest_pattern_is_the_entry_for_one_in_every_rounding_mode() {
        let mut m = FakeGuest::new();
        m.put_f32(0x0045_dbb4, 0.25).put_f32(0x0045_dbdc, 0.02)
            .put_f32(0x0044_541c, 16.0).put_f32(CACHED_FADE, 0.5)
            .put_f32(crate::light_constants::constant::ONE, 1.0);
        let rest = rest_stipple_table(&m, 0x1f80);
        assert_eq!(rest, Some(0x0048_7288 + 0x80 * 16));
        assert_eq!(stipple(&m, 5, false, 0x037f, 0x1f80).unwrap().map(|(_, at)| at), rest);
        m.put_f32(0x0044_541c, 2.5);
        assert_eq!(rest_stipple_table(&m, 0x1f80), None, "2.5 rounds to 2 or 3");
    }

    /// From the rest state, a faded record draws with its byte's table
    /// pattern; a fade within the tolerance of one keeps the rest rows;
    /// a byte the rounding modes disagree on gives none.
    #[test]
    fn a_faded_record_draws_with_its_bytes_pattern_from_the_rest_state() {
        let mut m = FakeGuest::new();
        m.put_f32(0x0045_dbb4, 0.125).put_f32(0x0045_dbdc, 0.02)
            .put_f32(0x0044_541c, 2.0).put_f32(CACHED_FADE, 1.0)
            .put_f32(crate::light_constants::constant::ONE, 1.0);
        // Byte 4: fade 0.5, scaled 1.0, pattern 1 in every mode.
        for row in 0..32u32 {
            m.put_u32(0x0048_7288 + 0x80 + 4 * row, 0x0101_0000 | row);
        }
        let rest = [u32::MAX; 32];
        let rows = faded_rows(&m, 4, &rest, 0x1f80).unwrap();
        assert_eq!(rows[0], 0x0101_0000);
        assert_eq!(rows[31], 0x0101_001f);
        assert_eq!(faded_rows(&m, 8, &rest, 0x1f80), Some(rest), "fade one: the prologue sets nothing");
        assert_eq!(faded_rows(&m, 0xff, &rest, 0x1f80), Some(rest), "a solid byte");
        assert_eq!(faded_rows(&m, 5, &rest, 0x1f80), None, "1.25 rounds to 1 or 2");
        m.put_u32(RENDER_RECORDS + 3 * RENDER_RECORD_SIZE + RECORD_FADE, 0x1234_5604);
        assert_eq!(record_fade(&m, 3), Some(4));
    }
}
