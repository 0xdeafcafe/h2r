//! Evaluating a shader animation in the current view. Float values stay as
//! guest words so the material cache observes the SSE and x87 rounding modes.

use crate::colour_pass::{constant::PACK_SCALE, global::TIME};
use crate::function_eval::{self, constant::{BYTE, ONE, SMALL}};
use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;
use x87::abi::{self, arith_op, src_kind};

const ANGLE_SCALE: u32 = 0x0043_f17c;
const HALF: u32 = 0x0045_dbbc;
const COLOUR_WEIGHT_SCALE: u32 = 0x0047_ff50;
const THREE: u32 = 0x0045_dee0;
const DEFAULT_COLOUR: u32 = 0x0046_86cc;

fn double(m: &dyn GuestMemory, at: u32) -> Result<u64, Error> {
    Ok(u64::from(m.u32(at)?) | u64::from(m.u32(at + 4)?) << 32)
}

/// The ordinary BSP evaluator (0x1b230) with no object input callback.
/// For time parameters the view clock is a double and the period is a
/// single. The quotient is rounded to single before taking the remainder.
pub fn evaluate(m: &dyn GuestMemory, property: u32, fcw: u16, mxcsr: u32) -> Result<u32, Error> {
    let reference = property + 12;
    let input = if m.u32(property)? == 0 {
        let mut x = X87::new(fcw);
        x.fld(m.u32(property + 8)?)?;
        X87::ok(abi::arith(&mut x.0, arith_op::DIVR, 0, src_kind::F64, double(m, TIME)?))?;
        let phase = x.fstp_m32()?;
        if m.u8(m.u32(reference + 4)?)? == 3 {
            phase
        } else {
            function_eval::fmod_one(m, fcw, phase)?
        }
    } else {
        0
    };
    function_eval::evaluate(m, &mut Sse { mxcsr }, fcw, reference, input, 0)
}

pub fn ranged(m: &dyn GuestMemory, reference: u32, v: u32, mxcsr: u32) -> Result<u32, Error> {
    crate::screen_effects::ranged(m, &mut Sse { mxcsr }, m.u32(reference + 4)?, v)
}

/// 0x13bc00 interpolates the four packed bytes in parallel, with signed
/// high-word multiplication and saturation of the final 16-bit lanes.
fn interpolate(a: u32, b: u32, weight: i16) -> u32 {
    let mut result = 0;
    for lane in 0..4 {
        let lo = ((a >> (lane*8)) & 255) as i32;
        let hi = ((b >> (lane*8)) & 255) as i32;
        let high_word = ((hi-lo)*8 * i32::from(weight)) >> 16;
        result |= ((lo + ((high_word+1) >> 1)).clamp(0,255) as u32) << (lane*8);
    }
    result
}

pub fn colour(m: &dyn GuestMemory, reference: u32, value: u32, fcw: u16, mxcsr: u32) -> Result<u32, Error> {
    let p = m.u32(reference+4)?;
    if p == 0 || m.i32(reference)? <= 0 { return Ok(u32::MAX); }
    let flags = m.u8(p+1)?;
    let kind = flags >> 4;
    if m.u8(p)? == 1 && (flags & 1 == 0 || kind <= 1) { return Ok(m.u32(p+4)?); }
    let mut s = Sse { mxcsr };
    let one = m.u32(ONE)?;
    let v = s.clamp(value, 0, one)?;
    let (a, b, t) = match kind {
        0 => {
            let v = s.mul(v, m.u32(PACK_SCALE)?)?;
            let v = super::states::integer_value(f32::from_bits(v), fcw) & 255;
            return Ok(0xff00_0000 | (v * 0x010101));
        }
        1 => return Ok(m.u32(p+4)?),
        2 => (4, 16, v),
        3 | 4 => {
            let mut x = X87::new(fcw);
            x.fld(v)?;
            if kind == 3 { x.arith_st(arith_op::ADD, 0)?; }
            else { X87::ok(abi::arith(&mut x.0, arith_op::MUL, 0, src_kind::F64, double(m, THREE)?))?; }
            let v = x.fstp_m32()?;
            // 0x16ae0 truncates a nonnegative input. NaNs give the
            // integer-indefinite result and do not satisfy its negative test.
            let segment = simd::cvttss2si(&mut s.mxcsr, v).map_err(|_| Error::Simd)? as i32;
            x.fild(segment)?;
            x.arith_m(arith_op::SUBR, v)?;
            let fraction = x.fstp_m32()?;
            if kind == 3 {
                if segment == 0 { (4, 8, fraction) }
                else { (8, 16, if segment >= 2 { one } else { fraction }) }
            } else {
                let (i,t) = if segment >= 3 { (2,one) } else { (segment as u32, fraction) };
                let offset = i.wrapping_mul(4).wrapping_add(4);
                (offset, offset.wrapping_add(4), t)
            }
        }
        _ => return Ok(u32::MAX),
    };
    let weight = s.mul(t, m.u32(COLOUR_WEIGHT_SCALE)?)?;
    let weight = simd::cvtss2si(&mut s.mxcsr, weight).map_err(|_| Error::Simd)? as i16;
    Ok(interpolate(m.u32(p.wrapping_add(a))?, m.u32(p.wrapping_add(b))?, weight))
}

/// Writes of 0x131bb0, or the default RGB triplet for a missing function.
pub fn rgb(m: &dyn GuestMemory, reference: u32, value: u32, fcw: u16, mxcsr: u32) -> Result<[u32;3],Error> {
    if m.u32(reference+4)? == 0 || m.i32(reference)? <= 0 {
        let p = m.u32(DEFAULT_COLOUR)?;
        return Ok([m.u32(p+4)?,m.u32(p+8)?,m.u32(p+12)?]);
    }
    let packed = colour(m, reference, value, fcw, mxcsr)?;
    let scale = m.u32(BYTE)?;
    let mut x = X87::new(fcw);
    let mut out = [0;3];
    for (lane,shift) in out.iter_mut().zip([16,8,0]) {
        x.fild(((packed >> shift) & 255) as i32)?;
        x.fmul_m(scale)?;
        *lane = x.fstp_m32()?;
    }
    Ok(out)
}

/// 0x1b580 followed by the pixel binding's clamp and ARGB conversion.
/// The same scalar animation can address R/G/B/A or the RGB triplet.
pub fn pixel(m: &dyn GuestMemory, reference: u32, value: u32, component: u8, transform: u8,
    fcw: u16, mxcsr: u32) -> Result<u32, Error> {
    let mut rgba = [0; 4];
    if component == 4 {
        rgba[..3].copy_from_slice(&rgb(m, reference, value, fcw, mxcsr)?);
    } else {
        let v = ranged(m, reference, value, mxcsr)?;
        let mut s = Sse { mxcsr };
        let v = super::world_registers::scalar_transform(m, &mut s, v, transform, fcw)?;
        if let Some(lane) = rgba.get_mut(usize::from(component)) { *lane = v; }
    }
    super::world_registers::pack(m, &mut rgba, false, fcw, mxcsr)
}

/// Shader transform expansion at 0x1b790. A masked register write
/// preserves all components not selected by this binding's operand mode.
pub fn transform(m: &mut dyn GuestMemory, slot: u32, dest: u32, mode: u32, fcw:u16, mxcsr:u32)
    ->Result<(),Error> {
    let mut s = Sse { mxcsr };
    let rotation = m.u32(slot+0x34)?;
    let one = m.u32(ONE)?;
    let mut x = X87::new(fcw);
    x.fld(rotation)?;
    x.unary(abi::unary_op::ABS)?;
    x.fld(m.u32(SMALL)?)?;
    let small_rotation = x.fcomip(1)? & 0x41 == 0;
    x.fstp_st(0)?;
    let (sin,cos) = if small_rotation {
        (0,one)
    } else {
        let angle = s.mul(rotation,m.u32(ANGLE_SCALE)?)?;
        x.fld(angle)?;
        x.fsin()?;
        let sin = x.fstp_m32()?;
        x.fld(angle)?;
        x.fcos()?;
        (sin,x.fstp_m32()?)
    };
    let (sx,sy) = (m.u32(slot+0x1c)?,m.u32(slot+0x20)?);
    let half = m.u32(HALF)?;
    let d = s.sub(one,cos)?;
    let plus = s.add(d,sin)?;
    let minus = s.sub(d,sin)?;
    let tx = s.mul(plus,sx)?;
    let tx = s.mul(tx,half)?;
    let tx = s.add(tx,m.u32(slot+0x28)?)?;
    let ty = s.mul(minus,sy)?;
    let ty = s.mul(ty,half)?;
    let ty = s.add(ty,m.u32(slot+0x2c)?)?;
    let rx = s.mul(sx,cos)?;
    let ry = s.mul(sy,cos)?;
    let ox = s.mul(sx,sin)?;
    let ox = s.sub(0,ox)?;
    let oy = s.mul(sy,sin)?;
    let z = m.u32(slot+0x24)?;
    let tz = m.u32(slot+0x30)?;
    let (first, words): (u32,&[u32]) = match mode {
        5..=8 => (mode-5,&[rx]),
        9 => (0,&[rx,ry]),
        10 => (2,&[rx,ry]),
        11 => (0,&[tx,ty]),
        12 => (2,&[tx,ty]),
        13 => (0,&[rx,ry,tx,ty]),
        14 | 18 => (0,&[rx,ox,0,tx]),
        15 | 19 => (0,&[oy,ry,0,ty]),
        16 => (0,&[rx,ry,z]),
        17 => (0,&[tx,ty,tz]),
        20 => (0,&[0,0,z,tz]),
        _ => return Err(Error::Data),
    };
    for (i,&word) in words.iter().enumerate() { m.set_u32(dest + (first+i as u32)*4,word)?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn packed_colour_interpolates_each_byte_with_signed_deltas() {
        let mut m = FakeGuest::new();
        let (reference, blob) = (0x1000, 0x2000);
        m.put_u32(reference, 32).put_u32(reference+4, blob)
            .put_u8(blob, 2).put_u8(blob+1, 2 << 4)
            .put_u32(blob+4, 0xff10_e040)
            .put_u32(blob+16, 0x7f30_2080)
            .put_f32(ONE, 1.0)
            .put_f32(COLOUR_WEIGHT_SCALE, 16384.0);
        assert_eq!(colour(&m, reference, 0.5f32.to_bits(), 0x37f, 0x1f80), Ok(0xbf20_8060));
    }

    #[test]
    fn transform_writes_only_its_selected_pair() {
        let mut m = FakeGuest::new();
        let (slot, dest) = (0x1000, 0x2000);
        m.put_f32(slot+0x34, 0.0)
            .put_f32(slot+0x1c, 3.0).put_f32(slot+0x20, 4.0)
            .put_f32(slot+0x28, 5.0).put_f32(slot+0x2c, 6.0)
            .put_f32(SMALL, 0.001)
            .put_f32(ONE, 1.0).put_f32(HALF, 0.5)
            .put_u32(dest, 1234).put_u32(dest+4, 5678);
        transform(&mut m, slot, dest, 12, 0x37f, 0x1f80).unwrap();
        assert_eq!([m.u32(dest),m.u32(dest+4)], [Ok(1234), Ok(5678)]);
        assert_eq!([m.f32(dest+8),m.f32(dest+12)], [Ok(5.0),Ok(6.0)]);
    }
}
