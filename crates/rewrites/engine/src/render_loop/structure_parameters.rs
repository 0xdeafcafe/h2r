//! The active structure's parameter block, as the geometry draw's
//! prologue `0x0004d640` uploads it for a shader with flag bit 0
//! (`+0x16`) when the active structure (`[0x004e0348]`) has one
//! (`+0x214` above 0): ESI the block's first element (`[+0x218]`), it
//! calls `0x0003c650` and then `0x0003d000` (`0x0004d6ef`..`0x0004d709`;
//! docs/graphics/native-objects.md, "Fade, the shader flag and the item's
//! argument"). **Code**, `h2 disasm`:
//!
//! - **`0x0003c650`** builds a rotation from the up vector `a`
//!   (`[[0x004687b0]]`, three floats) by the angle `atan2(v.x, v.y)` of
//!   the view words at `0x004b9dac` and `0x004b9db0` (`fpatan`) plus the
//!   block's `+0x8c`, with `s` and `c` its sine and cosine stored as
//!   floats. It stores thirteen words at `0x004c1b9c` (one, the matrix
//!   terms, three zeros; [`StructureUpload::scratch`]) and sets the bump
//!   environment matrix of stages 1, 2 and 3 through
//!   `SetTextureState_BumpEnv` (`0x003f5e30`): the terms at `+4`, `+8`,
//!   `+0x10` and `+0x14` times the block's `+0x68`, `-(+0x6c)` and
//!   `+0x70`, as `BUMPENVMAT00`, `01`, `10` and `11`.
//! - **`0x0003d000`** sets `c[21..25]` through `SetVertexShaderConstant1`
//!   (`0x003f71d0`, ECX the register plus 96) and the fog colour through
//!   `SetRenderState_FogColor` (`0x003f5370`): `c[21]` the constants
//!   `(1, [0x0045dbcc], [0x0045dbcc], [0x0045dbbc])`; `c[23]` the
//!   direction of the block's angles `+0x74` and `+0x78` mapped to
//!   `[0, 1]` and `[0x0045e458]`; `c[24]` a range from `+0x88`, `+0xa0`
//!   and `+0xa4`, zero unless all three and the range are above
//!   `[0x0045dbdc]`; the fog colour `+0x58` packed by `0x00131ed0`;
//!   `c[22]` from `+0x90`; `c[25]` the point at `+0x34` and one.
//!
//! The order of the operations, the x87 and SSE steps and their
//! rounding are the original's; `xmm0` holds 1.0 from `0x0003d00b` into
//! `c[24]` (`0x003f71d0` leaves it).

use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::{GuestMemory, MemFault};

/// The active structure.
pub const ACTIVE_BSP: u32 = 0x004e_0348;
/// The structure's parameter block: its count and its elements.
pub const BSP_PARAMETER_COUNT: u32 = 0x214;
pub const BSP_PARAMETERS: u32 = 0x218;
/// Where `0x0003c650` stores its thirteen words.
pub const SCRATCH: u32 = 0x004c_1b9c;
/// The first vertex constant `0x0003d000` sets (hardware numbering).
pub const FIRST_CONSTANT: usize = 21;
/// The view words `0x0003c650` takes the angle of.
const VIEW_X: u32 = 0x004b_9dac;
const VIEW_Y: u32 = 0x004b_9db0;
/// The pointer to the up vector.
const UP: u32 = 0x0046_87b0;
const ONE: u32 = 0x0045_dbc0;
const EPSILON: u32 = 0x0045_dbdc;
const HALF: u32 = 0x0045_dbbc;
const C21_YZ: u32 = 0x0045_dbcc;
const C23_W: u32 = 0x0045_e458;
/// A double the range's ends are scaled by.
const RANGE_SCALE: u32 = 0x0045_e450;
const PACK_SCALE: u32 = 0x0045_dc38;

/// Everything the upload sets, as words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StructureUpload {
    /// Per stage 1, 2 and 3, `D3DTSS_BUMPENVMAT00`, `01`, `11`, `10` (the
    /// texture-stage states 22 to 25, in that order).
    pub bump_env: [[u32; 4]; 3],
    /// `c[21..25]`.
    pub constants: [[u32; 4]; 5],
    /// `D3DRS_FOGCOLOR`.
    pub fog_colour: u32,
    /// The words at [`SCRATCH`].
    pub scratch: [u32; 13],
}

/// The first element of the active structure's parameter block, `None`
/// when it has none, so the prologue uploads nothing.
pub fn block(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    let bsp = m.u32(ACTIVE_BSP)?;
    if m.i32(bsp.wrapping_add(BSP_PARAMETER_COUNT))? > 0 {
        return m.u32(bsp.wrapping_add(BSP_PARAMETERS)).map(Some);
    }
    Ok(None)
}

/// What `0x0003c650` and `0x0003d000` set for the block at `block`,
/// under the x87 control word `fcw` and the MXCSR `mxcsr`.
pub fn upload(
    m: &dyn GuestMemory,
    block: u32,
    fcw: u16,
    mxcsr: u32,
) -> Result<StructureUpload, Error> {
    let f = |offset: u32| m.u32(block.wrapping_add(offset));
    let mut sse = Sse { mxcsr };
    let one = m.u32(ONE)?;
    let zero = 0u32;

    // 0x0003c650.
    let up = m.u32(UP)?;
    let a = [
        m.u32(up)?,
        m.u32(up.wrapping_add(4))?,
        m.u32(up.wrapping_add(8))?,
    ];
    let mut x = X87::new(fcw);
    x.fld(m.u32(VIEW_X)?)?;
    x.fld(m.u32(VIEW_Y)?)?;
    x.unary(x87::abi::unary_op::PATAN)?;
    x.fadd_m(f(0x8c)?)?;
    x.fld_st(0)?;
    x.fsin()?;
    let s = x.fstp_m32()?;
    x.fcos()?;
    let c = x.fstp_m32()?;
    x.fld(s)?;
    x.fmul_m(a[0])?;
    let s_a0 = x.fstp_m32()?;
    let a0_sq = sse.mul(a[0], a[0])?;
    let a1_sq = sse.mul(a[1], a[1])?;
    let a2_sq = sse.mul(a[2], a[2])?;
    let s_a1 = sse.mul(a[1], s)?;
    let s_a2 = sse.mul(a[2], s)?;
    let k = sse.sub(one, c)?;
    let diagonal = |sse: &mut Sse, sq: u32| -> Result<u32, Error> {
        let t = sse.sub(one, sq)?;
        let t = sse.mul(t, c)?;
        sse.add(t, sq)
    };
    let m00 = diagonal(&mut sse, a0_sq)?;
    let k_a0 = sse.mul(k, a[0])?;
    let k01 = sse.mul(k_a0, a[1])?;
    let m01 = sse.add(k01, s_a2)?;
    let m03 = sse.sub(k01, s_a2)?;
    let m04 = diagonal(&mut sse, a1_sq)?;
    let k02 = sse.mul(k_a0, a[2])?;
    let m02 = sse.sub(k02, s_a1)?;
    let m08 = diagonal(&mut sse, a2_sq)?;
    let m06 = sse.add(k02, s_a1)?;
    let k_a2 = sse.mul(k, a[2])?;
    let k12 = sse.mul(k_a2, a[1])?;
    let m05 = sse.add(k12, s_a0)?;
    let m07 = sse.sub(k12, s_a0)?;
    // 0x004c1b9c..0x004c1bcc.
    let scratch = [
        one, m00, m01, m02, m03, m04, m05, m06, m07, m08, zero, zero, zero,
    ];
    let mut bump_env = [[0u32; 4]; 3];
    for (stage, (scale, negate)) in [(0x68, false), (0x6c, true), (0x70, false)]
        .into_iter()
        .enumerate()
    {
        let scale = f(scale)?;
        // 0x16, 0x17, 0x19, 0x18: the terms at +4, +8, +0x10, +0x14.
        let mut term = |t: u32| -> Result<u32, Error> {
            let v = sse.mul(scale, t)?;
            if negate { sse.sub(zero, v) } else { Ok(v) }
        };
        let (s00, s01, s10, s11) = (term(m00)?, term(m01)?, term(m03)?, term(m04)?);
        bump_env[stage] = [s00, s01, s11, s10];
    }

    // 0x0003d000.
    let half = m.u32(HALF)?;
    let yz = m.u32(C21_YZ)?;
    let c21 = [one, yz, yz, half];
    let mut x = X87::new(fcw);
    x.fld(f(0x78)?)?;
    x.fsin()?;
    x.fld(f(0x74)?)?;
    x.fcos()?;
    x.arith_st(x87::abi::arith_op::MUL, 1)?;
    let dx = x.fstp_m32()?;
    x.fld(f(0x74)?)?;
    x.fsin()?;
    x.arith_st(x87::abi::arith_op::MUL, 1)?;
    let dy = x.fstp_m32()?;
    x.fstp_st(0)?;
    x.fld(f(0x78)?)?;
    x.fcos()?;
    let dz = x.fstp_m32()?;
    let mut unit = |v: u32| -> Result<u32, Error> {
        x.fld(v)?;
        x.fadd_m(one)?;
        x.fmul_m(half)?;
        x.fstp_m32()
    };
    let c23 = [unit(dx)?, unit(dy)?, unit(dz)?, m.u32(C23_W)?];
    let epsilon = m.u32(EPSILON)?;
    let (b88, ba0, ba4) = (f(0x88)?, f(0xa0)?, f(0xa4)?);
    let in_range =
        sse.above(b88, epsilon)? && sse.above(ba0, epsilon)? && sse.above(ba4, epsilon)? && {
            let span = sse.sub(ba4, ba0)?;
            sse.above(span, epsilon)?
        };
    let c24 = if in_range {
        let scale = u64::from(m.u32(RANGE_SCALE)?) | u64::from(m.u32(RANGE_SCALE + 4)?) << 32;
        let mut x = X87::new(fcw);
        x.fld(one)?;
        x.arith_m(x87::abi::arith_op::DIV, b88)?;
        x.fld_st(0)?;
        x.fmul_m(ba0)?;
        fmul_f64(&mut x, scale)?;
        let near = x.fstp_m32()?;
        x.fmul_m(ba4)?;
        fmul_f64(&mut x, scale)?;
        let far = x.fstp_m32()?;
        let span = sse.sub(far, near)?;
        let inverse = sse.div(one, span)?;
        let scaled = sse.mul(inverse, near)?;
        let offset = sse.sub(zero, scaled)?;
        [inverse, offset, zero, zero]
    } else {
        [zero; 4]
    };
    let fog_colour =
        crate::glow::pack_rgb([f(0x58)?, f(0x5c)?, f(0x60)?], m.u32(PACK_SCALE)?, fcw)?;
    let reciprocal = |sse: &mut Sse, v: u32| -> Result<u32, Error> {
        if sse.above(v, epsilon)? {
            sse.div(one, v)
        } else {
            Ok(v)
        }
    };
    let c22 = [
        f(0x90)?,
        sse.sub(zero, f(0x94)?)?,
        reciprocal(&mut sse, f(0x98)?)?,
        reciprocal(&mut sse, f(0x9c)?)?,
    ];
    let c25 = [f(0x34)?, f(0x38)?, f(0x3c)?, one];
    Ok(StructureUpload {
        bump_env,
        constants: [c21, c22, c23, c24, c25],
        fog_colour,
        scratch,
    })
}

/// `fmul qword`.
fn fmul_f64(x: &mut X87, bits: u64) -> Result<(), Error> {
    X87::ok(x87::abi::arith(
        &mut x.0,
        x87::abi::arith_op::MUL,
        0,
        x87::abi::src_kind::F64,
        bits,
    ))
}

impl StructureUpload {
    /// Stage `stage`'s (1 to 3) matrix as the pixel shader's bump terms
    /// take it: `00`, `01`, `10`, `11`.
    pub fn bump_matrix(&self, stage: usize) -> Option<[f32; 4]> {
        let [s00, s01, s11, s10] = *self.bump_env.get(stage.checked_sub(1)?)?;
        Some([s00, s01, s10, s11].map(f32::from_bits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const BLOCK: u32 = 0x0060_0000;
    const BSP: u32 = 0x0061_0000;
    const UP_VECTOR: u32 = 0x0062_0000;

    fn guest() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_f32(ONE, 1.0)
            .put_f32(EPSILON, 1e-4)
            .put_f32(HALF, 0.5)
            .put_f32(C21_YZ, 0.25)
            .put_f32(C23_W, 2.0)
            .put_f32(PACK_SCALE, 255.0)
            .put(RANGE_SCALE, &0.5f64.to_le_bytes())
            .put_u32(UP, UP_VECTOR)
            .put_f32(UP_VECTOR, 0.0)
            .put_f32(UP_VECTOR + 4, 0.0)
            .put_f32(UP_VECTOR + 8, 1.0)
            .put_f32(VIEW_X, 0.0)
            .put_f32(VIEW_Y, 1.0);
        m.put_u32(ACTIVE_BSP, BSP)
            .put_u32(BSP + BSP_PARAMETER_COUNT, 1)
            .put_u32(BSP + BSP_PARAMETERS, BLOCK);
        for (offset, v) in [
            (0x34, 3.0),
            (0x38, 4.0),
            (0x3c, 5.0),
            (0x58, 1.0),
            (0x5c, 0.5),
            (0x60, 0.0),
            (0x68, 2.0),
            (0x6c, 3.0),
            (0x70, 4.0),
            (0x74, 0.0),
            (0x78, 0.0),
            (0x88, 2.0),
            (0x8c, 0.0),
            (0x90, 7.0),
            (0x94, 1.0),
            (0x98, 4.0),
            (0x9c, 0.0),
            (0xa0, 2.0),
            (0xa4, 6.0),
        ] {
            m.put_f32(BLOCK + offset, v);
        }
        m
    }

    fn bits<const N: usize>(v: [f32; N]) -> [u32; N] {
        v.map(f32::to_bits)
    }

    /// The block's first element when the active structure has one.
    #[test]
    fn the_block_is_the_first_element_of_a_structure_that_has_one() {
        let mut m = guest();
        assert_eq!(block(&m), Ok(Some(BLOCK)));
        m.put_u32(BSP + BSP_PARAMETER_COUNT, 0);
        assert_eq!(block(&m), Ok(None));
        m.put_u32(BSP + BSP_PARAMETER_COUNT, u32::MAX);
        assert_eq!(
            block(&m),
            Ok(None),
            "a negative count uploads nothing (jle)"
        );
    }

    /// With the view along y and no extra angle, `atan2(0, 1)` is zero:
    /// the rotation about z is the identity, so the stages' matrices are
    /// the scales, the second negated; the constants are what the
    /// functions compute for these inputs.
    #[test]
    fn the_upload_is_what_the_two_functions_compute() {
        let m = guest();
        let u = upload(&m, BLOCK, 0x027f, 0x1f80).unwrap();
        assert_eq!(
            u.scratch,
            bits([
                1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0
            ])
        );
        assert_eq!(u.bump_env[0], bits([2.0, 0.0, 2.0, 0.0]));
        // 0 - 0 is +0 under nearest rounding.
        assert_eq!(u.bump_env[1], bits([-3.0, 0.0, -3.0, 0.0]));
        assert_eq!(u.bump_env[2], bits([4.0, 0.0, 4.0, 0.0]));
        assert_eq!(
            u.bump_matrix(2).map(bits),
            Some(bits([-3.0, 0.0, 0.0, -3.0]))
        );
        assert_eq!(u.bump_matrix(0), None);
        assert_eq!(u.constants[0], bits([1.0, 0.25, 0.25, 0.5]));
        // +0x90, 0 - 1, 1/4, and 0 kept (not above the epsilon).
        assert_eq!(u.constants[1], bits([7.0, -1.0, 0.25, 0.0]));
        // Angles of zero: (cos 0 · sin 0, sin 0 · sin 0, cos 0) mapped to [0, 1].
        assert_eq!(u.constants[2], bits([0.5, 0.5, 1.0, 2.0]));
        // t = 1/2: near 2 · t · 0.5 = 0.5, far 6 · t · 0.5 = 1.5.
        assert_eq!(u.constants[3], bits([1.0, -0.5, 0.0, 0.0]));
        assert_eq!(u.constants[4], bits([3.0, 4.0, 5.0, 1.0]));
        assert_eq!(u.fog_colour, 0x00ff_8000);
    }

    /// A quarter turn about z (the view along x): the matrix terms and so
    /// the stages' are the rotation's, the order of the original's
    /// operations kept; a range whose ends are too close sets `c[24]` to
    /// zero.
    #[test]
    fn a_quarter_turn_rotates_the_stages_and_a_short_range_clears_c24() {
        let mut m = guest();
        m.put_f32(VIEW_X, 1.0)
            .put_f32(VIEW_Y, 0.0)
            .put_f32(BLOCK + 0xa4, 2.0);
        let u = upload(&m, BLOCK, 0x027f, 0x1f80).unwrap();
        // sin and cos of pi/2, stored as floats: 1, and the cosine of the
        // rounded angle, close to zero.
        assert_eq!(f32::from_bits(u.scratch[2]), 1.0, "m01 = k·0·0 + s");
        assert_eq!(f32::from_bits(u.scratch[4]), -1.0, "m03 = k·0·0 - s");
        assert!(f32::from_bits(u.scratch[1]).abs() < 1e-6, "m00 = cos");
        assert_eq!(f32::from_bits(u.bump_env[0][1]), 2.0, "stage 1's 01");
        assert_eq!(f32::from_bits(u.bump_env[0][3]), -2.0, "stage 1's 10");
        assert_eq!(u.constants[3], [0; 4], "the range is not above the epsilon");
    }
}
