//! The sphere in view space and the tangents delimiting its screen rectangle.

use super::{BoundsQuery, Error, Fpu, Sse, X87, constant};
use crate::function_eval::constant::{INDEFINITE, NO_REPORT};
use guest_layout::memory::GuestMemory;
use x87::abi::{self, arith_op, unary_op};

const LOW_ANGLE: u32 = 0x0045_de34;
const HIGH_ANGLE: u32 = 0x0041_23ec;
const LOW_SLOPE: u32 = 0x0045_de3c;
const HIGH_SLOPE: u32 = 0x0045_de38;
const DEPTH_LIMIT: u32 = 0x0045_de30;
const ASIN_ONE: u32 = 0x0046_2742;

/// The CRT's asin (0x00373014). Its comparisons use the argument stored as a
/// double, but the calculation still uses the extended value on the stack.
fn asin(m: &dyn GuestMemory, x: &mut X87) -> Result<u32, Error> {
    let control = x.0.control;
    let (status, d) = abi::st_f64(&x.0, 0);
    X87::ok(status)?;
    abi::store_commit(&mut x.0, d.status, false);
    let magnitude = d.value & !(1 << 63);
    let one = 1.0f64.to_bits();
    let report = m.u32(NO_REPORT)? == 0;
    if magnitude > one && report {
        // The domain error can call the CRT handler and write errno.
        return Err(Error::Data);
    }
    let nan = f64::from_bits(d.value).is_nan();
    // The CRT changes the control word only on the finite-argument path.
    if magnitude < f64::INFINITY.to_bits() {
        X87::ok(abi::ldcw(&mut x.0, (control & 0x300) | 0x7f))?;
    }
    if magnitude < one {
        // atan2(t, sqrt((1-t)*(1+t))). Each fld1 pushes an exact one.
        X87::ok(abi::ld_const(&mut x.0, 0))?;
        x.arith_st(arith_op::ADD, 1)?;
        X87::ok(abi::ld_const(&mut x.0, 0))?;
        x.arith_st(arith_op::SUB, 2)?;
        x.arith_reg(arith_op::MUL, 1, 0, true)?;
        x.unary(unary_op::SQRT)?;
        x.unary(unary_op::PATAN)?;
    } else if !nan {
        let mut bytes = [0; 10];
        m.read(
            if magnitude == one {
                ASIN_ONE
            } else {
                INDEFINITE
            },
            &mut bytes,
        )?;
        x.fstp_st(0)?;
        X87::ok(abi::ld_f80(&mut x.0, bytes))?;
        if magnitude == one && d.value >> 63 != 0 {
            x.unary(unary_op::CHS)?;
        }
    }
    // With a nondefault control word the CRT's precision report spills
    // through a double. PE is sticky and may already be set on entry.
    // If these instructions haven't raised it, both exits must round to
    // the same caller word before a plan can own this result.
    let mut direct = None;
    if report && control != 0x027f {
        if x.0.status & abi::H2_FP_PE == 0 {
            let mut other = X87(x.0);
            X87::ok(abi::ldcw(&mut other.0, control))?;
            direct = Some(other.fstp_m32()?);
        }
        let (code, stored) = abi::st_f64(&x.0, 0);
        X87::ok(code)?;
        abi::store_commit(&mut x.0, stored.status, true);
        X87::ok(abi::ld_f64(&mut x.0, stored.value))?;
    }
    X87::ok(abi::ldcw(&mut x.0, control))?;
    let rounded = x.fstp_m32()?;
    if direct.is_some_and(|word| word != rounded) {
        return Err(Error::X87(abi::status::UNMODELLED));
    }
    Ok(rounded)
}

fn tangent(x: &mut X87, angle: u32) -> Result<u32, Error> {
    x.fld(angle)?;
    x.unary(unary_op::TAN)?;
    x.fstp_st(0)?; // discard FPTAN's one
    x.fstp_m32()
}

fn axis_bounds(
    m: &dyn GuestMemory,
    f: &mut Sse,
    x: &mut X87,
    lateral: u32,
    depth: u32,
    depth_squared: u32,
    radius: u32,
) -> Result<[u32; 2], Error> {
    x.fld(lateral)?;
    x.fld_st(0)?;
    x.arith_reg(arith_op::MUL, 1, 0, true)?;
    x.fadd_m(depth_squared)?;
    x.unary(unary_op::SQRT)?;
    // Compare the extended distance, divide by its rounded f32.
    let distance = x.fst_m32()?;
    x.fld(radius)?;
    let relation = x.fcomip(1)?;
    x.fstp_st(0)?;
    if relation & abi::EFL_CF == 0 {
        return Ok([m.u32(LOW_SLOPE)?, m.u32(HIGH_SLOPE)?]);
    }

    x.fld(radius)?;
    x.arith_m(arith_op::DIV, distance)?;
    let half_angle = asin(m, x)?;
    x.fld(lateral)?;
    x.fld(depth)?;
    x.unary(unary_op::CHS)?;
    x.unary(unary_op::PATAN)?;
    let angle = x.fstp_m32()?;
    let lo = f.sub(angle, half_angle)?;
    let lo = if f.above(m.u32(LOW_ANGLE)?, lo)? {
        m.u32(LOW_SLOPE)?
    } else {
        tangent(x, lo)?
    };
    let hi = f.add(angle, half_angle)?;
    let hi = if f.above(hi, m.u32(HIGH_ANGLE)?)? {
        m.u32(HIGH_SLOPE)?
    } else {
        tangent(x, hi)?
    };
    Ok([lo, hi])
}

/// `0x001650a0`: the six EDI output words, or `None` for AL clear. All reads
/// and arithmetic complete before the caller publishes any part of the light.
pub fn screen_bounds(
    m: &dyn GuestMemory,
    query: BoundsQuery,
    fpu: Fpu,
) -> Result<Option<[u32; 6]>, Error> {
    let mut f = Sse { mxcsr: fpu.mxcsr };
    let view = |off| m.u32(query.view.wrapping_add(off));
    let scale = view(4)?;
    let mut point = [0; 3];
    for (i, p) in point.iter_mut().enumerate() {
        *p = m.u32(query.centre.wrapping_add(4 * i as u32))?;
    }
    if !f.equal(scale, m.u32(constant::ONE)?)? {
        for p in &mut point {
            *p = f.mul(scale, *p)?;
        }
    }
    let mut centre = [0; 3];
    for (i, p) in centre.iter_mut().enumerate() {
        let off = 4 * i as u32;
        let z = f.mul(view(0x20 + off)?, point[2])?;
        let y = f.mul(view(0x14 + off)?, point[1])?;
        let zy = f.add(z, y)?;
        let x = f.mul(view(8 + off)?, point[0])?;
        let zyx = f.add(zy, x)?;
        *p = f.add(zyx, view(0x2c + off)?)?;
    }
    let depth = centre[2];
    let radius = query.word;
    if f.above(depth, radius)? {
        return Ok(None);
    }
    if fpu.x87_control & 0x3f != 0x3f {
        return Err(Error::X87(abi::status::UNMODELLED));
    }
    let depth_squared = f.mul(depth, depth)?;
    let mut x = X87::new(fpu.x87_control);
    let horizontal = axis_bounds(m, &mut f, &mut x, centre[0], depth, depth_squared, radius)?;
    let vertical = axis_bounds(m, &mut f, &mut x, centre[1], depth, depth_squared, radius)?;
    let limit = m.u32(DEPTH_LIMIT)?;
    let near = f.sub(depth, radius)?;
    let near = if f.above(near, limit)? { limit } else { near };
    let far = f.add(depth, radius)?;
    let far = if f.above(far, limit)? { limit } else { far };
    Ok(Some([
        horizontal[0],
        horizontal[1],
        vertical[0],
        vertical[1],
        near,
        far,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn eye_inside_uses_both_slope_limits_and_clamps_the_back_depth() {
        let mut m = FakeGuest::new();
        let view = 0x20000;
        let centre = 0x10000;
        let fpu = Fpu {
            mxcsr: 0x1f80,
            x87_control: 0x027f,
        };
        m.put_f32(constant::ONE, 1.0)
            .put_f32(LOW_SLOPE, -123.0)
            .put_f32(HIGH_SLOPE, 234.0)
            .put_f32(DEPTH_LIMIT, -0.25)
            .map(centre, 12)
            .put_f32(view + 4, 1.0)
            .put_f32(view + 8, 1.0)
            .put_f32(view + 0x18, 1.0)
            .put_f32(view + 0x28, 1.0);
        let query = BoundsQuery {
            centre,
            view,
            word: 2.0f32.to_bits(),
        };
        assert_eq!(
            screen_bounds(&m, query, fpu),
            Ok(Some(
                [-123.0, 234.0, -123.0, 234.0, -2.0, -0.25].map(f32::to_bits)
            ))
        );
        m.put_f32(centre + 8, 3.0);
        assert_eq!(screen_bounds(&m, query, fpu), Ok(None));
        let bad = BoundsQuery {
            centre: 0xffff_fffc,
            ..query
        };
        assert!(matches!(screen_bounds(&m, bad, fpu), Err(Error::Memory(_))));
    }
}
