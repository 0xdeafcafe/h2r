//! The shadow pass's constants: `0x00048ee0`, as one function that
//! returns what it stores (docs/graphics/native-lights-constants.md,
//! "The light's constants").
//!
//! `render_light_passes` (`0x00031170`, for a light that casts a shadow)
//! and the object shadows (`0x00032350`) call `0x00048ee0` before the
//! shadow layers. It fills the 36-word shadow block at
//! [`global::BLOCK`] (the projection along the light's forward axis, the
//! shadow map's size, the depth scale, the copies of the light frame's
//! rows), picks the shadow mode, sizes the shadow target, clears the four
//! desired textures, and ends by uploading the block as vertex shader
//! constants.
//!
//! [`shadow_globals`] computes all of it without writing anything;
//! [`ShadowGlobals::write`] stores it as the original does. Two calls
//! are left to whoever stores the block, in this order:
//!
//! - `0x0001cf50`: binds the desired textures (all none now) on the
//!   device, through D3D.
//! - `0x003f7330`, reached by a tail jump: the block uploaded as constants
//!   (EDX [`global::BLOCK`], ECX `0x28` the first register, stack `0x24`
//!   dwords, `ret 4`: vertex constants `c[40]`..`c[48]` in hardware
//!   numbering).
//!
//! Its callees `0x00048e70` (a float to an integer, rounding positive
//! fractions up) and `0x0001ddd0` (a render target's size words) are
//! ported here. The arithmetic is the original's, SSE through [`simd`]
//! and the x87 blocks through [`x87::abi`], as in
//! [`crate::light_constants`]; constants are read from the image at their
//! addresses. `engine/tests/shadow_constants_differential.rs` checks it
//! against the lifted original.

use crate::light_constants::{Error, Fpu, LightGlobals, Sse, X87};
use guest_layout::memory::{GuestMemory, MemFault};

/// `0x00048ee0`: ECX the projection volume, EBX the light's frame block
/// (`record + 0x84`), stack the `ligh` definition; `ret 4` (through
/// `0x003f7330`).
pub const SHADOW_SETUP: u32 = 0x0004_8ee0;
pub const SHADOW_SETUP_CLEANUP: u32 = 4;
/// `0x00048e70`: an f32 (stack) to an integer, `ret 4`.
pub const CEILING: u32 = 0x0004_8e70;
/// `0x0001ddd0`: ECX a render target, ESI its width, EDI its height.
pub const TARGET_SIZE: u32 = 0x0001_ddd0;
/// `0x0001cf50`: binds the desired textures; no arguments.
pub const BIND_TEXTURES: u32 = 0x0001_cf50;
/// `0x003f7330`: vertex shader constants (EDX the source, ECX the first
/// register in hardware numbering, stack the count of dwords), `ret 4`.
/// It writes the push-buffer methods `0x1ea4` (constant load slot, ECX)
/// and `0x0b80` (the dwords).
pub const SET_CONSTANTS: u32 = 0x003f_7330;
pub const SET_CONSTANTS_CLEANUP: u32 = 4;
/// What `0x00048ee0` passes it.
pub const CONSTANTS_REGISTER: u32 = 0x28;
pub const CONSTANTS_DWORDS: u32 = 0x24;

pub mod global {
    /// u32: the scene flags; bit 10 clear forces mode 2.
    pub const SCENE_FLAGS: u32 = 0x004c_1a80;
    /// u32: the pass flags (bits 4 and 5 pick the small shadow map).
    pub const PASS_FLAGS: u32 = 0x004b_a014;
    /// f32: the light's falloff words `0x0004a1e0` stored (`[1]` and `[4]`).
    pub const FALLOFF_1: u32 = 0x0048_5a34;
    pub const FALLOFF_4: u32 = 0x0048_5a40;
    /// u32 x8: the light frame's two copied rows.
    pub const FRAME_COPIES: u32 = 0x0048_5a0c;
    /// u8: the shadow mode (0, 1 or 2).
    pub const MODE: u32 = 0x0048_5a47;
    /// u32 x4: the desired textures, cleared.
    pub const TEXTURES: u32 = 0x0051_f3d8;
    /// The render target table (`0x98`-byte entries) and the shadow
    /// target's index.
    pub const TARGETS: u32 = 0x004b_4b58;
    pub const TARGET_STRIDE: u32 = 0x98;
    pub const SHADOW_TARGET: u32 = 9;
    /// u32 x36: the shadow block.
    pub const BLOCK: u32 = 0x004c_9798;
}

pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    pub const HALF: u32 = 0x0045_dbbc;
    /// The reciprocal when the context's `+0x64` is zero.
    pub const NO_RECIPROCAL: u32 = 0x0044_09d4;
    pub const SIZE_A: u32 = 0x0045_dc64;
    pub const SIZE_B: u32 = 0x0045_dcc8;
    /// The smallest shadow extent.
    pub const SIZE_MIN: u32 = 0x0045_dd60;
    pub const DEPTH_RANGE: u32 = 0x0045_dd40;
    pub const MODE_SCALE: u32 = 0x0045_dc2c;
    pub const NEAR_STEP: u32 = 0x0045_dc7c;
    pub const PLANE_SCALE: u32 = 0x0044_5420;
    pub const PLANE_OFFSET: u32 = 0x0045_ddd4;
    pub const NO_RANGE: u32 = 0x0045_dccc;
}

pub const BLOCK_WORDS: usize = 36;

/// The call's registers and its stack argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    /// ECX.
    pub context: u32,
    /// EBX.
    pub frame: u32,
    /// Stack 0.
    pub definition: u32,
}

/// What `0x00048ee0` stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowGlobals {
    /// The whole block; the word it does not set (14) keeps its value.
    pub block: [u32; BLOCK_WORDS],
    pub mode: u8,
    /// The shadow map's width and height.
    pub size: [i32; 2],
    /// The shadow target's words (`+0x80` width, `+0x84` height, `+0x14`
    /// and `+0x28` packed), when it is the full-size map.
    pub target: Option<[u32; 3]>,
}

impl ShadowGlobals {
    /// Stores everything but the two device calls.
    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        use global::*;
        let b: Vec<u8> = self.block.iter().flat_map(|v| v.to_le_bytes()).collect();
        m.write(MODE, &[self.mode])?;
        m.write(TEXTURES, &[0; 16])?;
        if let Some([w, h, packed]) = self.target {
            let e = TARGETS + SHADOW_TARGET * TARGET_STRIDE;
            m.set_u32(e + 0x80, w)?;
            m.set_u32(e + 0x84, h)?;
            m.set_u32(e + 0x14, packed)?;
            m.set_u32(e + 0x28, packed)?;
        }
        m.write(BLOCK, &b)
    }
}

/// `0x00048e70`: `x` truncated, plus one for a positive `x` with a
/// fraction (so positive values round up); integer arithmetic on the
/// bits, shifts masked as the CPU masks them.
pub fn ceiling(x: u32) -> i32 {
    let e = (x >> 23) & 0xff;
    let below_one = ((e as i32).wrapping_sub(0x7f)) >> 31;
    let cl = 0x96u32.wrapping_sub(e) & 31;
    let mut v = (((x & 0x7f_ffff) | 0x80_0000) as i32) >> cl;
    let sign = (x as i32) >> 31;
    v = (v ^ sign).wrapping_sub(sign) & !below_one;
    let up = if sign != 0 || x == 0 {
        0
    } else if below_one != 0 {
        1
    } else {
        let mask = (1u32 << cl).wrapping_sub(1) & x;
        (mask & 0x7f_ffff != 0) as i32
    };
    v.wrapping_add(up)
}

/// `0x0001ddd0`'s packed size word: zero unless the target's `+0x94`
/// byte is set.
pub fn target_size_word(width: i32, height: i32, enabled: bool) -> u32 {
    let w = width as u32;
    let h = height as u32;
    let a = ((w.wrapping_mul(4) >> 6) << 24).wrapping_sub(0x0100_0000);
    let d = (h << 12).wrapping_sub(0x1000);
    let v = a | d | w.wrapping_sub(1);
    if enabled { v } else { 0 }
}

fn vec3(m: &dyn GuestMemory, at: u32) -> Result<[u32; 3], MemFault> {
    Ok([m.u32(at)?, m.u32(at + 4)?, m.u32(at + 8)?])
}

/// `0x00048ee0` for `call` under `fpu`. Reads guest memory, writes none.
pub fn shadow_globals(m: &dyn GuestMemory, call: &Call, fpu: Fpu) -> Result<ShadowGlobals, Error> {
    compute(m, call, fpu, None)
}

/// Computes a candidate's projection with the light setup's outputs in
/// hand, before either setup has written to the guest.
pub fn shadow_globals_for_light(
    m: &dyn GuestMemory,
    call: &Call,
    fpu: Fpu,
    light: &LightGlobals,
    pass_flags: u32,
) -> Result<ShadowGlobals, Error> {
    compute(m, call, fpu, Some((light, pass_flags)))
}

fn compute(
    m: &dyn GuestMemory,
    call: &Call,
    fpu: Fpu,
    light: Option<(&LightGlobals, u32)>,
) -> Result<ShadowGlobals, Error> {
    use constant::*;
    let falloff = |index: usize| match light {
        Some((light, _)) => Ok(light.falloff[index]),
        None => m.u32(global::FALLOFF_1 + (index as u32 - 1) * 4),
    };
    let mut f = Sse { mxcsr: fpu.mxcsr };
    let mut x = X87::new(fpu.x87_control);
    let ecx = call.context;
    let ebx = call.frame;
    let d = call.definition;
    let mut block = [0u32; BLOCK_WORDS];
    for (i, w) in block.iter_mut().enumerate() {
        *w = m.u32(global::BLOCK + 4 * i as u32)?;
    }
    let one = m.u32(ONE)?;
    let half = m.u32(HALF)?;

    // The projection along forward.
    x.fld(m.u32(ecx + 8)?)?;
    x.unary(x87::abi::unary_op::SQRT)?;
    let c64 = m.u32(ecx + 0x64)?;
    let n = m.u16(d + 0x8e)? as i16;
    let root = x.fstp_m32()?;
    let depth = if n == 1 { m.u32(ecx + 0x28)? } else { 0 };
    let inv = if f.equal(c64, 0)? {
        m.u32(NO_RECIPROCAL)?
    } else {
        f.div(one, c64)?
    };
    let fw = vec3(m, ebx + 0x28)?;
    block[0] = f.mul(fw[0], inv)?;
    block[1] = f.mul(fw[1], inv)?;
    block[2] = f.mul(fw[2], inv)?;
    let a = f.mul(inv, depth)?;
    block[3] = f.sub(0, a)?;
    block[4] = f.sub(0, fw[0])?;
    block[5] = f.sub(0, fw[1])?;
    block[6] = f.sub(0, fw[2])?;
    block[7] = 0;

    let mode: u32 = if (m.u32(global::SCENE_FLAGS)? >> 10) & 1 == 0 {
        2
    } else if f.above(falloff(4)?, 0)? && m.u16(d + 0x14)? == 0 {
        1
    } else {
        0
    };

    // The extent: the context's `+4`, widened by the field of view.
    let c4 = m.u32(ecx + 4)?;
    let c0c = m.u32(ecx + 0xc)?;
    if f.above(c0c, 0)? {
        x.fld(c0c)?;
        x.fmul_m(half)?;
        x.unary(x87::abi::unary_op::TAN)?;
        x.fstp_st(0)?;
        x.fmul_m(c64)?;
        x.arith_reg(x87::abi::arith_op::ADD, 0, 0, false)?;
        x.fadd_m(c4)?;
    } else {
        x.fld(c4)?;
    }
    let pass = match light {
        Some((_, flags)) => flags,
        None => m.u32(global::PASS_FLAGS)?,
    };
    let small = (pass >> 4) & 1 != 0 && (pass >> 5) & 1 == 0;
    let side: i32 = if small { 0x80 } else { 0x200 };
    x.fild(side)?;
    let side_f = x.fstp_m32()?;
    x.fld_st(0)?;
    x.arith_reg(x87::abi::arith_op::MUL, 1, 0, true)?;
    x.fmul_m(m.u32(ecx + 8)?)?;
    x.unary(x87::abi::unary_op::SQRT)?;
    x.fmul_m(m.u32(SIZE_A)?)?;
    x.fld(m.u32(d + 0x10)?)?;
    x.fmul_m(m.u32(SIZE_B)?)?;
    x.arith_reg(x87::abi::arith_op::ADD, 1, 0, true)?;
    x.fld(side_f)?;
    let min = m.u32(SIZE_MIN)?;
    x.arith_m(x87::abi::arith_op::SUB, min)?;
    x.arith_reg(x87::abi::arith_op::MUL, 1, 0, true)?;
    x.fmul_m(falloff(1)?)?;
    x.fadd_m(min)?;
    let extent = x.fst_m32()?;
    x.fld(min)?;
    let flags = x.fcomip(1)?;
    x.fstp_st(0)?;
    let extent = if flags & 0x41 == 0 {
        min
    } else if f.above(extent, side_f)? {
        side_f
    } else {
        extent
    };

    // The map's size: the extent over and times the root, rounded up to
    // 64, then cut to the side squared.
    let a = f.div(extent, root)?;
    let align = |v: i32| v.wrapping_add(0x3f) & !0x3f;
    let mut width = align(ceiling(a));
    let a = f.mul(extent, root)?;
    let mut height = align(ceiling(a));
    let area = side.wrapping_mul(side);
    if height.wrapping_mul(width) > area {
        if width > height {
            width = area.checked_div(height).ok_or(Error::Divide)? & !0x3f;
        } else {
            height = area.checked_div(width).ok_or(Error::Divide)? & !0x3f;
        }
    }

    let range = m.u32(DEPTH_RANGE)?;
    let target = if small {
        block[8] = side_f;
        block[9] = f.int(side)?;
        None
    } else {
        let e = global::TARGETS + global::SHADOW_TARGET * global::TARGET_STRIDE;
        let enabled = m.u8(e + 0x94)? != 0;
        block[8] = f.int(width)?;
        block[9] = f.int(height)?;
        Some([
            width as u32,
            height as u32,
            target_size_word(width, height, enabled),
        ])
    };
    block[10] = range;
    block[11] = one;
    block[32] = one;
    block[33] = one;
    block[34] = 0;
    block[35] = 0;

    let x2 = if mode > 0 {
        let a = f.int(width)?;
        let a = f.mul(a, m.u32(MODE_SCALE)?)?;
        f.div(a, side_f)?
    } else {
        0
    };
    block[15] = 0;
    block[12] = f.add(x2, half)?;
    block[13] = f.sub(half, x2)?;

    // The near plane: a step along forward from the light, as a plane.
    let p = vec3(m, ebx)?;
    let step = m.u32(NEAR_STEP)?;
    let mut q = [0u32; 3];
    for ((qi, fi), pi) in q.iter_mut().zip(fw).zip(p) {
        let a = f.mul(fi, step)?;
        *qi = f.add(a, pi)?;
    }
    let nv = [f.sub(0, fw[0])?, f.sub(0, fw[1])?, f.sub(0, fw[2])?];
    let a = f.mul(nv[2], q[2])?;
    let b = f.mul(nv[1], q[1])?;
    let a = f.add(a, b)?;
    let b = f.mul(nv[0], q[0])?;
    let a = f.add(a, b)?;
    let a = f.mul(a, m.u32(PLANE_OFFSET)?)?;
    let scale = m.u32(PLANE_SCALE)?;
    block[16] = f.mul(nv[0], scale)?;
    block[17] = f.mul(nv[1], scale)?;
    block[18] = f.mul(nv[2], scale)?;
    block[19] = a;

    // The depth plane: forward over the definition's range, from its near
    // distance.
    let d2c = m.u32(d + 0x2c)?;
    let d30 = m.u32(d + 0x30)?;
    let r = if f.above(d30, d2c)? {
        let a = f.sub(d30, d2c)?;
        f.div(one, a)?
    } else {
        m.u32(NO_RANGE)?
    };
    for ((qi, fi), pi) in q.iter_mut().zip(fw).zip(p) {
        let a = f.mul(fi, d2c)?;
        *qi = f.add(a, pi)?;
    }
    if let Some((light, _)) = light {
        block[24..28].copy_from_slice(&light.frame_copy_a);
        block[28..32].copy_from_slice(&light.frame_copy_b);
    } else {
        for i in 0..8 {
            block[24 + i] = m.u32(global::FRAME_COPIES + 4 * i as u32)?;
        }
    }
    let a = f.mul(fw[2], q[2])?;
    let b = f.mul(fw[1], q[1])?;
    let a = f.add(a, b)?;
    let b = f.mul(fw[0], q[0])?;
    let a = f.add(a, b)?;
    let a = f.mul(a, r)?;
    block[20] = f.mul(fw[0], r)?;
    block[21] = f.mul(fw[1], r)?;
    block[22] = f.mul(fw[2], r)?;
    block[23] = f.sub(0, a)?;

    Ok(ShadowGlobals {
        block,
        mode: mode as u8,
        size: [width, height],
        target,
    })
}
