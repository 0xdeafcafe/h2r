//! The ocean's grid setup `0x0003ca90` (EAX the water definition), with
//! its basis `0x0003c9a0` and the normalise `0x00030bf0` it calls, as one
//! Rust function (docs/graphics/retire-d3d-display.md, "The water end";
//! docs/graphics/high-resolution-widescreen.md for the aspect).
//!
//! The water end's ocean calls it first ([`grid`]). With the definition's
//! angle `+0x7c` zero it returns at once. Otherwise it writes three blocks
//! of globals the ocean's draw and its visibility gate read:
//!
//! | Block | Words |
//! |---|---|
//! | [`MATRIX`] | the camera's basis: scale (the height over the water), forward, left, up ([`BASIS_UP`]), position (the camera's x, y, and the water level less the height times `+0xa8`) |
//! | [`ROWS`] | the grid's model rows: forward times the height, left times the sideways span, up, position, by x, y and z |
//! | [`QUAD`] | the visibility quad: four corners in the world |
//!
//! The sideways span is `tan(min(fov / 2, 0.7807)) * aspect` over the
//! grid's own half width (`tan(angle / 2) * +0x80`), 1 when that is no
//! wider, else 13 % more, times the height. The aspect is the raster
//! rectangle's, times 1.5 for an anamorphic lens, then widened to the
//! displayed lens ([`widen_aspect`]) where retail's site hook
//! `OCEAN_GRID_ASPECT` did it.
//!
//! The arithmetic is the original's, instruction for instruction: SSE
//! scalar operations under the caller's MXCSR, the tangents, square roots
//! and the normalise's length through the x87 model under the caller's
//! control word, so every word is bit-equal. Constants are read from the
//! image at their addresses ([`constant`]). An unmasked exception is an
//! [`Error`].
//!
//! `engine/tests/ocean_grid_differential.rs` checks the three blocks
//! against the original, lifted and run on the IR interpreter.

use crate::light_constants::{Error, Sse, X87};
use crate::memory::GuestMemory;

/// The image's constants the setup reads.
pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    pub const HALF: u32 = 0x0045_dbbc;
    /// The least height, and the normalise's least length.
    pub const LEAST: u32 = 0x0045_dbdc;
    /// The half angle's cap (0.7807).
    pub const CAP: u32 = 0x0045_e2d4;
    /// The span's margin (1.13).
    pub const MARGIN: u32 = 0x0045_e2d0;
    /// The anamorphic lens's widening (1.5).
    pub const ANAMORPHIC: u32 = 0x0043_ff08;
    /// The two factors of the quad's near tangent.
    pub const NEAR_A: u32 = 0x0045_e2cc;
    pub const NEAR_B: u32 = 0x0045_e2c8;
    /// The up vector's weight in the quad's y and z.
    pub const UP_WEIGHT: u32 = 0x0045_dbd8;
}

/// The camera's position.
pub const CAMERA: u32 = 0x0048_5618;
/// The horizontal field of view.
pub const FOV: u32 = 0x0048_5640;
/// The raster rectangle: top, left, bottom, right as i16.
pub const RASTER: u32 = 0x0048_5648;
/// The anamorphic lens's byte.
pub const WIDESCREEN: u32 = 0x0048_5ac2;
/// The record whose state 3 keeps the retail aspect.
pub const LENS_STATE: u32 = 0x004e_6948;
/// The basis's forward and the pointer to its up.
pub const BASIS_FORWARD: u32 = 0x004b_9dac;
pub const BASIS_UP: u32 = 0x0046_87b0;
/// The three blocks.
pub const MATRIX: u32 = 0x004c_1b68;
pub const ROWS: u32 = 0x004c_1b38;
pub const QUAD: u32 = 0x004c_1b08;
/// The definition's words.
pub const ANGLE: u32 = 0x7c;
pub const WIDTH: u32 = 0x80;
pub const LEVEL: u32 = 0x84;
pub const FAR: u32 = 0x88;
pub const DEPTH: u32 = 0xa8;

/// What the setup did, and the registers it leaves (EBX, ESI and EDI are
/// the caller's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grid {
    /// The angle was zero: nothing written, EAX the definition, ECX and
    /// EDX the caller's.
    Skipped,
    Built {
        eax: u32,
        /// ECX is 0 (the copy's count).
        edx: u32,
        /// Whether [`widen_aspect`] widened the aspect.
        widened: bool,
    },
}

/// The aspect widened to a lens displayed `factor` times narrower than
/// the guest's, carrying the half angle's cap's loss: `aspect / factor *
/// tan(fov / 2) / tan(cap)` when the half angle is past the cap. `None`
/// (the retail aspect) for a `factor` of 1 or more, one not finite, or an
/// aspect not positive.
pub fn widen_aspect(mem: &dyn GuestMemory, aspect: f32, factor: f32) -> Option<f32> {
    if !(factor.is_finite() && factor > 0.0 && factor < 1.0) {
        return None;
    }
    let uncapped = match (mem.f32(FOV), mem.f32(constant::HALF), mem.f32(constant::CAP)) {
        (Ok(fov), Ok(half), Ok(cap)) if cap > 0.0 && fov * half > cap && fov * half < 1.5 => (fov * half).tan() / cap.tan(),
        _ => 1.0,
    };
    let widened = aspect / factor * uncapped;
    (aspect.is_finite() && aspect > 0.0 && widened.is_finite()).then_some(widened)
}

/// `0x0003ca90(eax = definition)` under the caller's MXCSR (its sticky
/// flags updated) and x87 control word; `lens` the displayed lens's factor
/// for [`widen_aspect`].
pub fn grid(m: &mut dyn GuestMemory, definition: u32, mxcsr: &mut u32, x87_control: u16, lens: f32) -> Result<Grid, Error> {
    let mut f = Sse { mxcsr: *mxcsr };
    let r = setup(m, definition, &mut f, x87_control, lens);
    *mxcsr = f.mxcsr;
    r
}

fn setup(m: &mut dyn GuestMemory, d: u32, f: &mut Sse, control: u16, lens: f32) -> Result<Grid, Error> {
    use constant::*;
    use x87::abi::{arith_op::MUL, unary_op};
    const ZERO: u32 = 0;
    let angle = m.u32(d.wrapping_add(ANGLE))?;
    if f.equal(angle, ZERO)? {
        return Ok(Grid::Skipped);
    }
    let depth = m.u32(d.wrapping_add(DEPTH))?;
    let level = m.u32(d.wrapping_add(LEVEL))?;
    let [cx, cy, cz] = [m.u32(CAMERA)?, m.u32(CAMERA + 4)?, m.u32(CAMERA + 8)?];
    let mut x0 = f.sub(cz, level)?;
    let mut height = x0;
    if f.above(ZERO, x0)? {
        x0 = f.sub(ZERO, x0)?;
        height = x0;
    }
    let least = m.u32(LEAST)?;
    if f.above(least, x0)? {
        height = least;
    }
    let mut x = X87::new(control);
    basis(m, f, &mut x)?;
    let sunk = f.mul(height, depth)?;
    let z = f.sub(level, sunk)?;
    m.set_u32(MATRIX + 0x30, z)?;
    m.set_u32(MATRIX + 0x28, cx)?;
    m.set_u32(MATRIX + 0x2c, cy)?;
    m.set_u32(MATRIX, height)?;

    // The grid's half width, and the raster rectangle's aspect.
    let half = m.u32(HALF)?;
    x.fld(angle)?;
    x.fmul_m(half)?;
    let raster = |o: u32| -> Result<i32, Error> { Ok(i32::from(m.u16(RASTER + o)? as i16)) };
    let (top, left, bottom, right) = (raster(0)?, raster(2)?, raster(4)?, raster(6)?);
    x.unary(unary_op::TAN)?;
    let r = f.int(right)?;
    let l = f.int(left)?;
    let wide = f.sub(r, l)?;
    let tall = f.int(bottom.wrapping_sub(top))?;
    let state = m.u32(LENS_STATE)?;
    let mut aspect = f.div(wide, tall)?;
    x.fstp_st(0)?;
    x.fmul_m(m.u32(d.wrapping_add(WIDTH))?)?;
    let grid_half = x.fstp_m32()?;
    let retail = state != 0 && m.u8(state.wrapping_add(1))? != 0
        && m.u16(state.wrapping_add(2))? != 0xffff
        && m.u32(state.wrapping_add(8))? == 3;
    let mut eax = state;
    if !retail {
        let byte = m.u8(WIDESCREEN)?;
        eax = (state & !0xff) | u32::from(byte);
        if byte != 0 {
            aspect = f.mul(aspect, m.u32(ANAMORPHIC)?)?;
        }
    }
    let widened = match widen_aspect(&*m, f32::from_bits(aspect), lens) {
        Some(w) => {
            aspect = w.to_bits();
            true
        }
        None => false,
    };

    // The sideways span.
    let half_fov = f.mul(m.u32(FOV)?, half)?;
    let cap = m.u32(CAP)?;
    let half_angle = if f.above(cap, half_fov)? { half_fov } else { cap };
    x.fld(half_angle)?;
    let one = m.u32(ONE)?;
    x.unary(unary_op::TAN)?;
    x.fstp_st(0)?;
    x.fmul_m(aspect)?;
    let side = x.fstp_m32()?;
    let mut span = f.div(side, grid_half)?;
    span = if f.above(span, one)? { f.mul(span, m.u32(MARGIN)?)? } else { one };

    // The model rows.
    let at = |o: u32| MATRIX + o;
    let (up_x, fwd_x, left_x, up_y) = (m.u32(at(0x1c))?, m.u32(at(4))?, m.u32(at(0x10))?, m.u32(at(0x20))?);
    let fwd_z = m.u32(at(0xc))?;
    let span = f.mul(span, height)?;
    let fwd_y = m.u32(at(8))?;
    let fz = f.mul(fwd_z, height)?;
    let fx = f.mul(fwd_x, height)?;
    let left_y = m.u32(at(0x14))?;
    let fy = f.mul(fwd_y, height)?;
    let up_z = m.u32(at(0x24))?;
    let left_z = m.u32(at(0x18))?;
    let pz = m.u32(at(0x30))?;
    let lx = f.mul(left_x, span)?;
    let ly = f.mul(left_y, span)?;
    let lz = f.mul(left_z, span)?;
    let rows = [fx, lx, up_x, cx, fy, ly, up_y, cy, fz, lz, up_z, pz];
    for (i, w) in rows.iter().enumerate() {
        m.set_u32(ROWS + 4 * i as u32, *w)?;
    }

    // The quad's tangents: a near one, and two half widths along it.
    x.fld(angle)?;
    x.fmul_m(m.u32(NEAR_A)?)?;
    x.fmul_m(m.u32(NEAR_B)?)?;
    x.unary(unary_op::TAN)?;
    x.fstp_st(0)?;
    let a = x.fstp_m32()?;
    x.fld(angle)?;
    x.fmul_m(half)?;
    x.unary(unary_op::TAN)?;
    x.fstp_st(0)?;
    x.fmul_m(m.u32(d.wrapping_add(WIDTH))?)?;
    x.fld(a)?;
    x.fmul_m(a)?;
    x.fadd_m(one)?;
    x.unary(unary_op::SQRT)?;
    x.arith_st(MUL, 1)?;
    let b = x.fstp_m32()?;
    let nb = f.sub(ZERO, b)?;
    let c = m.u32(d.wrapping_add(FAR))?;
    x.fld(c)?;
    x.fmul_m(c)?;
    x.fadd_m(one)?;
    x.unary(unary_op::SQRT)?;
    x.arith_st(MUL, 1)?;
    let e = x.fstp_m32()?;
    let ne = f.sub(ZERO, e)?;
    x.fstp_st(0)?;

    // The corners: ((left * k + forward * t) + up * w) + position.
    let weight = m.u32(UP_WEIGHT)?;
    let u0 = f.mul(up_x, ZERO)?;
    let fxc = f.mul(fx, c)?;
    let u1 = f.mul(up_y, weight)?;
    let fyc = f.mul(fy, c)?;
    let u2 = f.mul(up_z, weight)?;
    let fzc = f.mul(fz, c)?;
    let mut quad = [0u32; 12];
    // x: left, forward, up, position for every corner.
    let (k0, t0) = (f.mul(lx, nb)?, f.mul(fx, a)?);
    quad[0] = sum(f, k0, t0, u0, cx)?;
    let (k1, t1) = (f.mul(lx, b)?, f.mul(fx, a)?);
    quad[3] = sum(f, k1, t1, u0, cx)?;
    let k2 = f.mul(lx, e)?;
    quad[6] = sum(f, k2, fxc, u0, cx)?;
    let k3 = f.mul(lx, ne)?;
    quad[9] = sum(f, k3, fxc, u0, cx)?;
    // y: the far corners add up before forward.
    let (k0, t0) = (f.mul(ly, nb)?, f.mul(fy, a)?);
    quad[1] = sum(f, k0, t0, u1, cy)?;
    let (k1, t1) = (f.mul(ly, b)?, f.mul(fy, a)?);
    quad[4] = sum(f, k1, t1, u1, cy)?;
    let k2 = f.mul(ly, e)?;
    quad[7] = sum(f, k2, u1, fyc, cy)?;
    let k3 = f.mul(ly, ne)?;
    quad[10] = sum(f, k3, u1, fyc, cy)?;
    // z: as x.
    let (k0, t0) = (f.mul(lz, nb)?, f.mul(fz, a)?);
    quad[2] = sum(f, k0, t0, u2, pz)?;
    let (k1, t1) = (f.mul(lz, b)?, f.mul(fz, a)?);
    quad[5] = sum(f, k1, t1, u2, pz)?;
    let k2 = f.mul(lz, e)?;
    quad[8] = sum(f, k2, fzc, u2, pz)?;
    let k3 = f.mul(lz, ne)?;
    quad[11] = sum(f, k3, fzc, u2, pz)?;
    for (i, w) in quad.iter().enumerate() {
        m.set_u32(QUAD + 4 * i as u32, *w)?;
    }
    Ok(Grid::Built { eax, edx: top as u32, widened })
}

fn read3(m: &dyn GuestMemory, at: u32) -> Result<[u32; 3], Error> {
    Ok([m.u32(at)?, m.u32(at.wrapping_add(4))?, m.u32(at.wrapping_add(8))?])
}

fn write3(m: &mut dyn GuestMemory, at: u32, v: [u32; 3]) -> Result<(), Error> {
    for (i, w) in v.iter().enumerate() {
        m.set_u32(at.wrapping_add(4 * i as u32), *w)?;
    }
    Ok(())
}

/// `((p + q) + r) + s`.
fn sum(f: &mut Sse, p: u32, q: u32, r: u32, s: u32) -> Result<u32, Error> {
    let t = f.add(p, q)?;
    let t = f.add(t, r)?;
    f.add(t, s)
}

/// `p * q - r * s`, each product as the original orders it.
fn cross(f: &mut Sse, p: u32, q: u32, r: u32, s: u32) -> Result<u32, Error> {
    let a = f.mul(p, q)?;
    let b = f.mul(r, s)?;
    f.sub(a, b)
}

/// `0x0003c9a0(eax = BASIS_FORWARD, esi = [BASIS_UP], ebx = MATRIX)`:
/// scale 1, up copied, left = forward x up and forward = up x left, each
/// normalised, position 0. Every read and write in the original's order,
/// so an up vector inside the matrix reads as it does there.
fn basis(m: &mut dyn GuestMemory, f: &mut Sse, x: &mut X87) -> Result<(), Error> {
    let (a, up) = (BASIS_FORWARD, m.u32(BASIS_UP)?);
    m.set_u32(MATRIX, m.u32(constant::ONE)?)?;
    let u = read3(m, up)?;
    write3(m, MATRIX + 0x1c, u)?;
    let (av, u) = (read3(m, a)?, read3(m, up)?);
    let z = cross(f, av[1], u[0], av[0], u[1])?;
    let y = cross(f, u[2], av[0], av[2], u[0])?;
    let w = cross(f, av[2], u[1], u[2], av[1])?;
    write3(m, MATRIX + 0x10, [w, y, z])?;
    normalise(m, f, x, MATRIX + 0x10)?;
    let (c, u) = (read3(m, MATRIX + 0x10)?, read3(m, up)?);
    let z = cross(f, c[0], u[1], c[1], u[0])?;
    let y = cross(f, c[2], u[0], u[2], c[0])?;
    let w = cross(f, u[2], c[1], c[2], u[1])?;
    write3(m, MATRIX + 4, [w, y, z])?;
    normalise(m, f, x, MATRIX + 4)?;
    write3(m, MATRIX + 0x28, [0; 3])
}

/// `0x00030bf0(eax = v)`: the length on the x87 stack, stored as f32; a
/// vector shorter than [`constant::LEAST`] (its unrounded length) stays.
fn normalise(m: &mut dyn GuestMemory, f: &mut Sse, x: &mut X87, at: u32) -> Result<(), Error> {
    use x87::abi::{arith_op::ADD, arith_op::MUL, unary_op};
    let v = read3(m, at)?;
    x.fld(v[2])?;
    x.fld(v[1])?;
    x.fld(v[0])?;
    x.fld_st(0)?;
    x.arith_st(MUL, 1)?;
    x.fld_st(2)?;
    x.arith_st(MUL, 3)?;
    x.arith_reg(ADD, 1, 0, true)?;
    x.fld_st(3)?;
    x.arith_st(MUL, 4)?;
    x.arith_reg(ADD, 1, 0, true)?;
    x.unary(unary_op::SQRT)?;
    x.fstp_st(3)?;
    x.fstp_st(0)?;
    x.fstp_st(0)?;
    let length = x.fst_m32()?;
    x.unary(unary_op::ABS)?;
    x.fld(m.u32(constant::LEAST)?)?;
    let flags = x.fcomip(1)?;
    x.fstp_st(0)?;
    if flags & 0x41 == 0 {
        return Ok(());
    }
    let inv = f.div(m.u32(constant::ONE)?, length)?;
    let v0 = f.mul(m.u32(at)?, inv)?;
    m.set_u32(at, v0)?;
    let v1 = f.mul(inv, m.u32(at.wrapping_add(4))?)?;
    let v2 = f.mul(inv, m.u32(at.wrapping_add(8))?)?;
    m.set_u32(at.wrapping_add(4), v1)?;
    m.set_u32(at.wrapping_add(8), v2)
}

#[cfg(test)]
mod tests;
