//! Derive the observer's vertical FOV, rotate its forward/up vectors about
//! two axes and compute the displacement from the original view direction.
use crate::sim_physics::{Result, arithmetic};
use runtime::{CpuAbi, Guest, TransferAbi};
pub mod observer;
use crate::light_constants::{Error, Sse, X87};
use crate::sim_physics::spatial::{finish_fpu, fp_error, load_fpu};
use x87::abi::{arith_op, unary_op};

type Vector = [u32; 3];
type Matrix = [Vector; 3];
type Math<T> = std::result::Result<T, Error>;
pub(crate) const HALF: u32 = 0x0045_dbbc;
pub(crate) const ONE: u32 = 0x0045_dbc0;
pub(crate) const ATAN_DENOMINATOR: u32 = 0x0045_dcc0;
pub(crate) const DOT_FLOOR: u32 = 0x0045_dbdc;
pub(crate) const VIEW_SCALE: u32 = 0x0054_e854;
pub(crate) const ROTATION_AXIS: u32 = 0x0046_87b0;

fn read3(g: &Guest, at: u32) -> Result<Vector> {
    Ok([
        g.u32(at)?,
        g.u32(at.wrapping_add(4))?,
        g.u32(at.wrapping_add(8))?,
    ])
}
fn write3(g: &mut Guest, at: u32, v: Vector) -> Result<()> {
    for (i, value) in v.into_iter().enumerate() {
        g.set_u32(at.wrapping_add(i as u32 * 4), value)?;
    }
    Ok(())
}

// Sum three products left to right. Operand order matters for NaNs.
// Each unrolled matrix application schedules its three terms differently.
fn sum(s: &mut Sse, a: (u32, u32), b: (u32, u32), c: (u32, u32)) -> Math<u32> {
    let a = s.mul(a.0, a.1)?;
    let b = s.mul(b.0, b.1)?;
    let ab = s.add(a, b)?;
    let c = s.mul(c.0, c.1)?;
    s.add(ab, c)
}

fn rotation(
    s: &mut Sse,
    axis: Vector,
    sine_axis: Vector,
    cos: u32,
    one: u32,
    second: bool,
) -> Math<Matrix> {
    let [x, y, z] = axis;
    let [sx, sy, sz] = sine_axis;
    let mut diag = [0; 3];
    for (dest, a) in diag.iter_mut().zip(axis) {
        let aa = s.mul(a, a)?;
        let rest = s.sub(one, aa)?;
        let t = s.mul(rest, cos)?;
        *dest = s.add(t, aa)?;
    }
    let t = s.sub(one, cos)?;
    let xy = if second {
        let ty = s.mul(t, y)?;
        s.mul(ty, x)?
    } else {
        let tx = s.mul(t, x)?;
        s.mul(tx, y)?
    };
    let tz = s.mul(t, z)?;
    let xz = s.mul(tz, x)?;
    let yz = s.mul(tz, y)?;
    Ok([
        [diag[0], s.sub(xy, sz)?, s.add(xz, sy)?],
        [s.add(xy, sz)?, diag[1], s.sub(yz, sx)?],
        [s.sub(xz, sy)?, s.add(yz, sx)?, diag[2]],
    ])
}

fn rotate_pair(
    s: &mut Sse,
    m: Matrix,
    f: Vector,
    u: Vector,
    second: bool,
) -> Math<(Vector, Vector)> {
    let [[a, b, c], [d, e, h], [i, j, k]] = m;
    let [x, y, z] = f;
    let forward = if second {
        [
            sum(s, (c, z), (b, y), (a, x))?,
            sum(s, (h, z), (e, y), (d, x))?,
            sum(s, (j, y), (k, z), (i, x))?,
        ]
    } else {
        [
            sum(s, (x, a), (c, z), (b, y))?,
            sum(s, (h, z), (e, y), (x, d))?,
            sum(s, (j, y), (k, z), (x, i))?,
        ]
    };
    let [x, y, z] = u;
    let up = if second {
        [
            sum(s, (a, x), (z, c), (y, b))?,
            sum(s, (d, x), (z, h), (y, e))?,
            sum(s, (i, x), (y, j), (z, k))?,
        ]
    } else {
        [
            sum(s, (z, c), (y, b), (x, a))?,
            sum(s, (x, d), (z, h), (y, e))?,
            sum(s, (x, i), (y, j), (z, k))?,
        ]
    };
    Ok((forward, up))
}

fn cross(s: &mut Sse, f: Vector, u: Vector) -> Math<Vector> {
    let mut diff = |a, b, c, d| {
        let pos = s.mul(a, b)?;
        let neg = s.mul(c, d)?;
        s.sub(pos, neg)
    };
    Ok([
        diff(f[1], u[2], u[1], f[2])?,
        diff(f[2], u[0], f[0], u[2])?,
        diff(u[1], f[0], f[1], u[0])?,
    ])
}

// atan2 on the x87 stack (Y below the loaded X).
fn atan_one(x: &mut X87) -> Math<()> {
    X87::ok(x87::abi::ld_const(&mut x.0, x87::Const::One as u32))?;
    x.unary(unary_op::PATAN)
}

/// One observer command and the constants selected by the loaded image.
/// Every arithmetic operand is its binary32 bit pattern, except the x87
/// atan denominator, which the original loads as binary64.
struct CameraInput {
    original: Vector,
    up: Vector,
    fov: u32,
    aspect: u32,
    offset: [u32; 2],
    axis: Vector,
    view_scale: u32,
    half: u32,
    one: u32,
    atan_denominator: u64,
    floor: u32,
}

impl CameraInput {
    fn gather(g: &Guest, view: u32) -> Result<Self> {
        Ok(Self {
            original: read3(g, view.wrapping_add(32))?,
            up: read3(g, view.wrapping_add(44))?,
            fov: g.u32(view.wrapping_add(56))?,
            aspect: g.u32(view.wrapping_add(60))?,
            offset: [g.u32(view.wrapping_add(64))?, g.u32(view.wrapping_add(68))?],
            axis: read3(g, g.u32(ROTATION_AXIS)?)?,
            view_scale: g.u32(VIEW_SCALE)?,
            half: g.u32(HALF)?,
            one: g.u32(ONE)?,
            atan_denominator: g.u64(ATAN_DENOMINATOR)?,
            floor: g.u32(DOT_FLOOR)?,
        })
    }
}

/// Only the basis, projection and displacement are persistent. The private
/// frame and final SSE lanes are observable at the guest return boundary.
struct CameraResult {
    forward: Vector,
    up: Vector,
    scaled_fov: u32,
    vertical_fov: u32,
    displacement: u32,
    original: Vector,
    intermediate_up: Vector,
    delta: Vector,
    cosine: u32,
    one_minus_cosine: u32,
    coefficients: [u32; 5],
    lanes: [u32; 8],
}

fn derive(input: CameraInput, s: &mut Sse, x: &mut X87) -> Math<CameraResult> {
    let CameraInput {
        original,
        up,
        fov,
        aspect,
        offset,
        axis,
        view_scale,
        half,
        one,
        atan_denominator,
        floor,
    } = input;
    let scaled = s.div(fov, view_scale)?;
    x.fld(fov)?;
    x.fmul_m(half)?;
    x.unary(unary_op::TAN)?;
    x.fstp_st(0)?;
    x.arith_m(arith_op::DIV, aspect)?;
    X87::ok(x87::abi::ld_f64(&mut x.0, atan_denominator))?;
    x.unary(unary_op::PATAN)?;
    x.arith_st(arith_op::ADD, 0)?;
    let vertical_fov = x.fst_m32()?;
    x.fmul_m(half)?;
    x.unary(unary_op::TAN)?;
    x.fstp_st(0)?;
    x.fld_st(0)?;
    x.fmul_m(offset[0])?;
    atan_one(x)?;
    x.fxch(1)?;
    x.fmul_m(offset[1])?;
    atan_one(x)?;
    let angle = x.fstp_m32()?;
    x.fld_st(0)?;
    x.fsin()?;
    let sin = x.fstp_m32()?;
    x.fcos()?;
    let cos = x.fstp_m32()?;
    x.fld(sin)?;
    x.fmul_m(axis[0])?;
    let sx = x.fstp_m32()?;
    let sy = s.mul(sin, axis[1])?;
    let sz = s.mul(sin, axis[2])?;
    let first = rotation(s, axis, [sx, sy, sz], cos, one, false)?;
    let (f, u) = rotate_pair(s, first, original, up, false)?;
    let axis = cross(s, f, u)?;

    x.fld(angle)?;
    x.fsin()?;
    let sin = x.fstp_m32()?;
    x.fld(angle)?;
    x.fcos()?;
    let cos = x.fstp_m32()?;
    let mut scaled_axis = [0; 3];
    for (dst, a) in scaled_axis.iter_mut().zip(axis) {
        *dst = s.mul(a, sin)?;
    }
    let second = rotation(s, axis, scaled_axis, cos, one, true)?;
    let (forward, new_up) = rotate_pair(s, second, f, u, true)?;
    let dot = sum(
        s,
        (forward[1], original[1]),
        (forward[2], original[2]),
        (forward[0], original[0]),
    )?;
    let divisor = if s.above(dot, floor)? { dot } else { floor };
    let factor = s.div(one, divisor)?;
    let mut delta = [0; 3];
    for i in [2, 1, 0] {
        let a = if i == 0 {
            s.mul(factor, original[i])
        } else {
            s.mul(original[i], factor)
        }?;
        delta[i] = s.sub(a, forward[i])?;
        x.fld(delta[i])?;
        x.fmul_m(delta[i])?;
        if i != 2 {
            x.arith_reg(arith_op::ADD, 1, 0, true)?;
        }
    }
    x.unary(unary_op::SQRT)?;
    let displacement = x.fstp_m32()?;

    Ok(CameraResult {
        forward,
        up: new_up,
        scaled_fov: scaled,
        vertical_fov,
        displacement,
        original,
        intermediate_up: u,
        delta,
        cosine: cos,
        one_minus_cosine: s.sub(one, cos)?,
        coefficients: [
            second[1][0],
            second[2][0],
            second[0][1],
            second[2][1],
            first[1][2],
        ],
        lanes: [
            delta[0],
            delta[1],
            delta[2],
            original[0],
            floor,
            factor,
            second[1][1],
            second[0][2],
        ],
    })
}

/// The bridge checks the original camera port's mapped spans and floating
/// point controls before entering the derived-result adapter.
pub fn run(c: &mut CpuAbi, g: &mut Guest, _: u32) -> Result<TransferAbi> {
    let view = c.esi;
    let sp = c.esp.wrapping_sub(80);
    let mut x = load_fpu(c);
    let mut s = Sse { mxcsr: c.mxcsr };
    let result = derive(CameraInput::gather(g, view)?, &mut s, &mut x).map_err(fp_error)?;

    g.set_u32(view.wrapping_add(80), result.scaled_fov)?;
    g.set_u32(view.wrapping_add(76), result.vertical_fov)?;
    g.set_u32(sp.wrapping_sub(4), c.edi)?;
    g.set_u32(sp, result.cosine)?;
    g.set_u32(sp.wrapping_add(4), result.one_minus_cosine)?;
    write3(g, sp.wrapping_add(8), result.delta)?;
    write3(g, sp.wrapping_add(20), result.original)?;
    write3(g, sp.wrapping_add(32), result.intermediate_up)?;
    for (offset, value) in [48, 52, 56, 64, 72].into_iter().zip(result.coefficients) {
        g.set_u32(sp.wrapping_add(offset), value)?;
    }
    c.eax = view.wrapping_add(32);
    c.ecx = view.wrapping_add(44);
    c.edx = sp.wrapping_add(32);
    write3(g, c.eax, result.forward)?;
    write3(g, c.ecx, result.up)?;
    g.set_u32(view.wrapping_add(72), result.displacement)?;
    finish_fpu(c, g, x, 0x172341, view.wrapping_add(72))?;
    for (dst, lane) in c.xmm.iter_mut().zip(result.lanes) {
        *dst = u128::from(lane).to_le_bytes();
    }
    c.mxcsr = s.mxcsr;
    c.esp = arithmetic(c, sp, 80, false);
    g.ret(c, 0)
}

pub fn supported(c: &CpuAbi, g: &Guest) -> bool {
    c.x87.control & 0xc3f == 0x3f
        && c.x87.status & 0x80 == 0
        && c.mxcsr & 0x1f80 == 0x1f80
        && crate::sim_physics::observer::disjoint(c, g)
}
