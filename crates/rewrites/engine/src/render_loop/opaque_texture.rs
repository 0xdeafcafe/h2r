//! Texture-stage arithmetic in the standard shader binder. Float words in the
//! cache are inputs to later shaders, including when the stage is unbound.

use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;

/// Ordered comparisons for the slot cache, preserving MXCSR denormal mode.
pub fn above(a: u32, b: u32, mxcsr: u32) -> Result<bool, Error> {
    Sse { mxcsr }.above(a, b)
}

pub fn equal(a: u32, b: u32, mxcsr: u32) -> Result<bool, Error> {
    Sse { mxcsr }.equal(a, b)
}

/// `cvttss2si` followed by the negative fractional correction.
pub fn frame_floor(bits: u32, mxcsr: u32) -> Result<i32, Error> {
    let mut sse = Sse { mxcsr };
    let trunc = simd::cvttss2si(&mut sse.mxcsr, bits).map_err(|_| Error::Simd)? as i32;
    let integer = sse.int(trunc)?;
    let less_than_zero = sse.above(0, bits)?;
    Ok(trunc.wrapping_sub(i32::from(less_than_zero && !sse.equal(integer, bits)?)))
}

/// `fldln2; fxch; fyl2x; fmul; fstp` on the lightmap's larger signed side.
pub fn lightmap_lod(m: &dyn GuestMemory, dimension: i16, control: u16) -> Result<u32, Error> {
    const LOG_SCALE: u32 = 0x0044_a04c;
    let mut fp = X87::new(control);
    fp.fild(i32::from(dimension))?;
    X87::ok(x87::abi::ld_const(&mut fp.0, x87::Const::Ln2 as u32))?;
    fp.fxch(1)?;
    fp.unary(x87::abi::unary_op::YL2X)?;
    fp.fmul_m(m.u32(LOG_SCALE)?)?;
    fp.fstp_m32()
}

/// The requested fractional LOD passed to bitmap residency, and the integer
/// threshold retained at stage +0x10 after a request. The two clamps differ.
pub fn resident_lod(m: &dyn GuestMemory, bias: Option<(u32, u32)>, mxcsr: u32)
    -> Result<(u32, u32), Error>
{
    let mut sse = Sse { mxcsr };
    let requested = if let Some((stage, draw)) = bias {
        let sum = sse.add(stage, draw)?;
        let biased = sse.sub(sum, m.u32(crate::light_constants::constant::HALF)?)?;
        sse.clamp(biased, 0, m.u32(0x0045_dc10)?)?
    } else { 0 };
    let floor = sse.int(frame_floor(requested, mxcsr)?)?;
    let clamped = sse.clamp(floor, 0, m.u32(crate::light_constants::constant::TWO)?)?;
    let integer = simd::cvttss2si(&mut sse.mxcsr, clamped).map_err(|_| Error::Simd)?;
    Ok((requested, sse.int(integer as i32)?))
}

/// For a finite f32 frame and a positive i32 count the exact remainder fits
/// in f64. The CRT returns a double; the binder stores it under the x87
/// caller's rounding control before taking the SSE floor.
pub fn wrapped_image(frame: f32, count: i32, fcw: u16, mxcsr: u32) -> Result<i32, Error> {
    let remainder = f64::from(frame) % f64::from(count);
    let (bits, _) = x87::F80::from_f64_bits(remainder.to_bits()).to_f32_bits(x87::Mode::from_cw(fcw));
    frame_floor(bits, mxcsr)
}
