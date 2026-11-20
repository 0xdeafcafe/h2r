//! The two distance words at shader context +0x28 and +0x2c.
//! Read the scale factors from the live image. The first word uses SSE
//! rounding; the second passes through x87 extended precision.

use crate::light_constants::{Error, Sse, X87};
use crate::memory::GuestMemory;

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

pub const CACHED_FADE: u32 = 0x004b_8494;

#[cfg(test)]
mod stipple_tests {
    use super::*;
    use crate::testing::FakeGuest;

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
    }
}
