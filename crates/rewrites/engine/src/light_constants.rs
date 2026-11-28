//! The per-light constants: `0x0004a1e0` and its callee `0x000497a0`, as
//! one function that returns the globals block they write
//! (docs/graphics/native-lights-constants.md,
//! "The light's constants").
//!
//! `render_light_passes` (`0x00031170`) calls `0x0004a1e0` once per light,
//! before its layer passes. The call fills a block of globals the light
//! templates read: the colours and falloff, frame, colour scale mode,
//! projected screen rectangle, bounding sphere and shadow byte. Its last
//! call, `0x0003a550`, marks shader inputs dirty, uploads the frame and
//! draws a shader gel when the definition has one. The device scissor
//! is inherited at that call.
//!
//! [`light_globals`] accepts an answer for the sphere projection, and
//! [`projected_light_globals`] computes the answer with [`screen_bounds`].
//! [`LightGlobals::write`] publishes the returned block.
//!
//! The arithmetic is the original's, instruction for instruction: SSE
//! scalar operations through [`simd`] under the caller's MXCSR, and the x87
//! block of the special frame (definition flags `0x20` and `0x40000`)
//! through [`x87::abi`] under the caller's control word, so every word is
//! bit-equal, NaNs and rounding included. Constants are read from the image
//! at their addresses ([`constant`]), so none is copied here. An unmasked
//! floating-point exception, or an x87 operation the crate does not model
//! (a transcendental under a directed rounding mode), is an [`Error`]: the
//! light stays with the original.
//!
//! `engine/tests/light_constants_differential.rs` checks the block against
//! the original, lifted and run on the IR interpreter, word for word.

use crate::memory::{GuestMemory, MemFault};

mod screen_bounds;
pub use screen_bounds::screen_bounds;

/// `0x0004a1e0`: EAX the light's frame block, ECX the light volume, EDX
/// the light datum (or -1), stack `(definition, unused, shadow, weight)`.
pub const SET_LIGHT: u32 = crate::points::LIGHT_CONSTANTS.base;
pub const SET_LIGHT_CLEANUP: u32 = 0x10;
/// `0x000497a0`: the light's frame and colour scale (EAX the frame block
/// out, ECX the context, eight stack arguments).
pub const LIGHT_FRAME: u32 = 0x0004_97a0;
pub const LIGHT_FRAME_CLEANUP: u32 = 0x20;
/// `0x001650a0`: a sphere's screen bounds (EAX the centre, ECX the view's
/// projection, EDI six words out, stack one word); AL says whether it is
/// on screen.
pub const SCREEN_BOUNDS: u32 = 0x0016_50a0;
pub const SCREEN_BOUNDS_CLEANUP: u32 = 4;
/// `0x0003a550`: dirty inputs, upload the light frame and draw the shader gel.
pub const UPLOAD_LIGHT_FRAME: u32 = 0x0003_a550;

/// The globals the call reads and writes.
pub mod global {
    /// u32: the light datum array's header address (`render::light`).
    pub const LIGHT_ARRAY: u32 = 0x004e_030c;
    /// u32: the current view (the camera block `0x001650a0` projects with).
    pub const VIEW: u32 = 0x0054_7f88;
    /// The special frame's origin, forward and up (definition flags `0x20`
    /// and `0x40000`); the first two words are also the x87 block's angles.
    pub const SPECIAL_ORIGIN: u32 = 0x004b_9da0;
    pub const SPECIAL_FORWARD: u32 = 0x004b_9dac;
    pub const SPECIAL_UP: u32 = 0x004b_9db8;
    /// u32: address of three words copied into the frame (`+0x50`).
    pub const FRAME_TAIL: u32 = 0x0046_87a4;
    /// i16 x4: the viewport rectangle: `y0` at `+0`, `x0` at `+2`, `y1` at
    /// `+4`, `x1` at `+6`.
    pub const VIEWPORT: u32 = 0x0048_5a8a;

    /// u8, set to 1 (the passes' tail zeroes it).
    pub const ACTIVE: u32 = 0x0048_58bc;
    /// u32: the definition's address.
    pub const DEFINITION: u32 = 0x0048_58c4;
    /// u16: the context's first word.
    pub const CONTEXT_WORD: u32 = 0x0048_58c8;
    /// u32: the record's driver object, or -1.
    pub const DRIVER: u32 = 0x0048_58cc;
    /// u8: the rectangle came from a record.
    pub const RECTANGLE_VALID: u32 = 0x0048_58d0;
    /// f32 x4: the bounding sphere (centre, radius).
    pub const SPHERE: u32 = 0x0048_5934;
    /// f32 x3 each: the two colours.
    pub const COLOUR_A: u32 = 0x0048_5944;
    pub const COLOUR_B: u32 = 0x0048_5950;
    /// f32: the colour scale `0x000497a0` takes, and a zero after it.
    pub const SCALE: u32 = 0x0048_595c;
    pub const ZERO: u32 = 0x0048_5960;
    /// f32 x6: the screen rectangle and two depth words.
    pub const RECTANGLE: u32 = 0x0048_5964;
    /// u32 x31: `0x000497a0`'s frame block.
    pub const FRAME: u32 = 0x0048_597c;
    /// u32 x4 each: copies of the frame's words 8..12 and 12..16, taken
    /// before the record's offsets are added.
    pub const FRAME_COPY_A: u32 = 0x0048_5a0c;
    pub const FRAME_COPY_B: u32 = 0x0048_5a1c;
    /// u32: the colour scale mode (0 none, 1 half, 2 quarter).
    pub const MODE: u32 = 0x0048_5a2c;
    /// f32 x5: falloff and cutoff words.
    pub const FALLOFF: u32 = 0x0048_5a30;
    /// u8: the shadow argument.
    pub const SHADOW: u32 = 0x0048_5a44;
}

/// Constants in the image, read where the code reads them.
pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    pub const MINUS_ONE: u32 = 0x0045_dbcc;
    pub const HALF: u32 = 0x0045_dbbc;
    pub const TWO: u32 = 0x0045_dc0c;
    pub const QUARTER: u32 = 0x0045_dc20;
    /// The x87 block's four multipliers.
    pub const WAVE_A: u32 = 0x0045_df74;
    pub const WAVE_B: u32 = 0x0045_df70;
    pub const WAVE_C: u32 = 0x0045_df6c;
    pub const WAVE_D: u32 = 0x0045_df68;
    /// The x87 block's results are scaled by these.
    pub const SWAY_UP: u32 = 0x0045_08f8;
    pub const SWAY_SIDE: u32 = 0x0045_08f4;
    /// Added to the context's `+0x28` for the projective tail.
    pub const BIAS: u32 = 0x0045_df64;
}

/// Words in the frame block.
pub const FRAME_WORDS: usize = 31;

/// `0x0003a550` (the call's last) uploads the frame block and the five
/// words after it as vertex constants through `0x003f7330`: ECX `0x1f`,
/// the first register in hardware numbering (`c[31]`..`c[39]`), stack
/// `0x24` dwords.
pub const FRAME_REGISTER: u32 = 0x1f;
pub const FRAME_UPLOAD_DWORDS: usize = 0x24;
/// The frame words the engine's vector inputs 5 (`0x00033a0b`, colour A
/// scaled and clamped) and 6 (`0x00033a25`, colour B) return.
pub const FRAME_COLOUR_A: usize = 24;
pub const FRAME_COLOUR_B: usize = 28;

/// The call's registers and stack arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    /// EAX: the light's frame block (`record + 0x84` from `0x00031170`).
    pub frame: u32,
    /// ECX: the light volume.
    pub context: u32,
    /// EDX: the light datum, or `u32::MAX` for none.
    pub light: u32,
    /// Stack 0: the `ligh` definition's address.
    pub definition: u32,
    /// Stack 2: the shadow argument (its low byte is stored).
    pub shadow: u32,
    /// Stack 3: the weight (f32 bits).
    pub weight: u32,
}

/// The floating-point state the call runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fpu {
    pub mxcsr: u32,
    pub x87_control: u16,
}

/// What `0x001650a0` is asked: EAX, ECX and the pushed word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundsQuery {
    /// The record's `+0x28`.
    pub centre: u32,
    /// The view's `+4`.
    pub view: u32,
    /// The record's `+0x34` word.
    pub word: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Memory(MemFault),
    /// An unmasked SSE exception.
    Simd,
    /// An unmasked x87 exception or an operation the x87 crate stops on.
    X87(u32),
    /// An integer division by zero (`#DE` in the original).
    Divide,
    /// A loop the original would run on for too long or for ever (the
    /// blur's steps at infinity): the port stops at its budget.
    Unbounded,
    /// Tag data the port cannot read as the original does (a function
    /// blob that does not parse, tables of the wrong size).
    Data,
    /// A typed shadow would have to unbind a live guest texture.
    BoundTexture,
}

impl From<MemFault> for Error {
    fn from(f: MemFault) -> Self {
        Error::Memory(f)
    }
}

/// Everything `0x0004a1e0` stores, as words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightGlobals {
    pub definition: u32,
    pub context_word: u16,
    pub driver: u32,
    pub colour_a: [u32; 3],
    pub colour_b: [u32; 3],
    pub scale: u32,
    pub falloff: [u32; 5],
    /// The whole block; words the call does not set keep the values it
    /// had (read from memory).
    pub frame: [u32; FRAME_WORDS],
    pub frame_copy_a: [u32; 4],
    pub frame_copy_b: [u32; 4],
    pub mode: u32,
    pub rectangle: [u32; 6],
    pub rectangle_valid: bool,
    /// Only with a record.
    pub sphere: Option<[u32; 4]>,
    pub shadow: u8,
}

impl LightGlobals {
    /// The 36 dwords `0x0003a550` uploads at [`FRAME_REGISTER`]: the frame
    /// block, then the five words after it, which the call leaves as they
    /// were (read from `m`).
    pub fn frame_constants(&self, m: &dyn GuestMemory) -> Result<[u32; FRAME_UPLOAD_DWORDS], MemFault> {
        let mut out = [0u32; FRAME_UPLOAD_DWORDS];
        out[..FRAME_WORDS].copy_from_slice(&self.frame);
        for (i, w) in out.iter_mut().enumerate().skip(FRAME_WORDS) {
            *w = m.u32(global::FRAME + 4 * i as u32)?;
        }
        Ok(out)
    }

    /// Engine vector input 5: colour A as the frame holds it.
    pub fn colour_a_input(&self) -> [u32; 3] {
        [0, 1, 2].map(|i| self.frame[FRAME_COLOUR_A + i])
    }

    /// Engine vector input 6: colour B as the frame holds it (before
    /// `render_light_passes` may zero the colour B globals, which the
    /// frame does not see).
    pub fn colour_b_input(&self) -> [u32; 3] {
        [0, 1, 2].map(|i| self.frame[FRAME_COLOUR_B + i])
    }

    /// The block as a call left it in memory: what [`Self::write`] stores,
    /// read back (the sphere always). For a caller whose call has run, as
    /// the first-person lights' (`0x000404b0`, its call at `0x0004057e`),
    /// whose light volume is a local of its own frame.
    pub fn stored(m: &dyn GuestMemory) -> Result<Self, MemFault> {
        use global::*;
        fn words<const N: usize>(m: &dyn GuestMemory, at: u32) -> Result<[u32; N], MemFault> {
            let mut out = [0u32; N];
            for (i, w) in out.iter_mut().enumerate() {
                *w = m.u32(at + 4 * i as u32)?;
            }
            Ok(out)
        }
        Ok(Self {
            definition: m.u32(DEFINITION)?,
            context_word: m.u16(CONTEXT_WORD)?,
            driver: m.u32(DRIVER)?,
            colour_a: words(m, COLOUR_A)?,
            colour_b: words(m, COLOUR_B)?,
            scale: m.u32(SCALE)?,
            falloff: words(m, FALLOFF)?,
            frame: words(m, FRAME)?,
            frame_copy_a: words(m, FRAME_COPY_A)?,
            frame_copy_b: words(m, FRAME_COPY_B)?,
            mode: m.u32(MODE)?,
            rectangle: words(m, RECTANGLE)?,
            rectangle_valid: m.u8(RECTANGLE_VALID)? != 0,
            sphere: Some(words(m, SPHERE)?),
            shadow: m.u8(SHADOW)?,
        })
    }

    /// Stores the block before the shader input update and frame upload.
    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        use global::*;
        fn words(m: &mut dyn GuestMemory, at: u32, w: &[u32]) -> Result<(), MemFault> {
            let b: Vec<u8> = w.iter().flat_map(|v| v.to_le_bytes()).collect();
            m.write(at, &b)
        }
        m.write(ACTIVE, &[1])?;
        words(m, DEFINITION, &[self.definition])?;
        m.write(CONTEXT_WORD, &self.context_word.to_le_bytes())?;
        words(m, DRIVER, &[self.driver])?;
        words(m, COLOUR_A, &self.colour_a)?;
        words(m, COLOUR_B, &self.colour_b)?;
        words(m, SCALE, &[self.scale, 0])?;
        words(m, FALLOFF, &self.falloff)?;
        words(m, FRAME, &self.frame)?;
        words(m, FRAME_COPY_A, &self.frame_copy_a)?;
        words(m, FRAME_COPY_B, &self.frame_copy_b)?;
        words(m, MODE, &[self.mode])?;
        words(m, RECTANGLE, &self.rectangle)?;
        m.write(RECTANGLE_VALID, &[self.rectangle_valid as u8])?;
        if let Some(s) = self.sphere {
            words(m, SPHERE, &s)?;
        }
        m.write(SHADOW, &[self.shadow])
    }
}

/// SSE scalar operations as the Pentium III does them.
pub(crate) struct Sse {
    pub(crate) mxcsr: u32,
}

impl Sse {
    pub(crate) fn lane(r: Result<u128, simd::Xm>) -> Result<u32, Error> {
        r.map(|v| v as u32).map_err(|_| Error::Simd)
    }
    pub(crate) fn add(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        Self::lane(simd::addss(&mut self.mxcsr, a as u128, b))
    }
    pub(crate) fn sub(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        Self::lane(simd::subss(&mut self.mxcsr, a as u128, b))
    }
    pub(crate) fn mul(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        Self::lane(simd::mulss(&mut self.mxcsr, a as u128, b))
    }
    pub(crate) fn div(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        Self::lane(simd::divss(&mut self.mxcsr, a as u128, b))
    }
    pub(crate) fn int(&mut self, i: i32) -> Result<u32, Error> {
        Self::lane(simd::cvtsi2ss(&mut self.mxcsr, 0, i as u32))
    }
    pub(crate) fn comiss(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        simd::comiss(&mut self.mxcsr, a, b).map_err(|_| Error::Simd)
    }
    /// `comiss a, b; ja`.
    pub(crate) fn above(&mut self, a: u32, b: u32) -> Result<bool, Error> {
        Ok(self.comiss(a, b)? & 0x41 == 0)
    }
    /// `comiss a, b; jb`.
    pub(crate) fn below(&mut self, a: u32, b: u32) -> Result<bool, Error> {
        Ok(self.comiss(a, b)? & 0x01 != 0)
    }
    /// `ucomiss a, b; lahf; test ah, 44h; jnp`: ordered and equal.
    pub(crate) fn equal(&mut self, a: u32, b: u32) -> Result<bool, Error> {
        let f = simd::ucomiss(&mut self.mxcsr, a, b).map_err(|_| Error::Simd)?;
        Ok(f & 0x44 == 0x40)
    }
    /// `lo` if `lo > v`, else `hi` if `v > hi`, else `v`: every clamp in
    /// both functions has this shape.
    pub(crate) fn clamp(&mut self, v: u32, lo: u32, hi: u32) -> Result<u32, Error> {
        Ok(if self.above(lo, v)? {
            lo
        } else if self.above(v, hi)? {
            hi
        } else {
            v
        })
    }
}

/// The x87 stack for the special frame's block.
pub(crate) struct X87(pub(crate) x87::abi::X87Abi);

impl X87 {
    pub(crate) fn new(control: u16) -> Self {
        X87(x87::abi::X87Abi {
            control,
            ..Default::default()
        })
    }
    pub(crate) fn ok(code: u32) -> Result<(), Error> {
        if code & x87::abi::status::MASK == x87::abi::status::OK {
            Ok(())
        } else {
            Err(Error::X87(code))
        }
    }
    pub(crate) fn fld(&mut self, bits: u32) -> Result<(), Error> {
        Self::ok(x87::abi::ld_f32(&mut self.0, bits))
    }
    /// `fld qword`.
    pub(crate) fn fld_m64(&mut self, bits: u64) -> Result<(), Error> {
        Self::ok(x87::abi::ld_f64(&mut self.0, bits))
    }
    /// `fistp dword`, rounded by the control word.
    pub(crate) fn fistp_m32(&mut self) -> Result<i32, Error> {
        let (code, store) = x87::abi::st_i32(&self.0, 0);
        Self::ok(code)?;
        x87::abi::store_commit(&mut self.0, store.status, true);
        Ok(store.value)
    }
    pub(crate) fn arith_m(&mut self, op: u32, bits: u32) -> Result<(), Error> {
        Self::ok(x87::abi::arith(
            &mut self.0,
            op,
            0,
            x87::abi::src_kind::F32,
            bits as u64,
        ))
    }
    pub(crate) fn arith_st(&mut self, op: u32, i: u32) -> Result<(), Error> {
        Self::ok(x87::abi::arith(
            &mut self.0,
            op,
            0,
            x87::abi::src_kind::ST,
            i as u64,
        ))
    }
    pub(crate) fn fmul_m(&mut self, bits: u32) -> Result<(), Error> {
        self.arith_m(x87::abi::arith_op::MUL, bits)
    }
    pub(crate) fn fadd_m(&mut self, bits: u32) -> Result<(), Error> {
        self.arith_m(x87::abi::arith_op::ADD, bits)
    }
    pub(crate) fn unary(&mut self, op: u32) -> Result<(), Error> {
        Self::ok(x87::abi::unary(&mut self.0, op))
    }
    pub(crate) fn fsin(&mut self) -> Result<(), Error> {
        self.unary(x87::abi::unary_op::SIN)
    }
    pub(crate) fn fcos(&mut self) -> Result<(), Error> {
        self.unary(x87::abi::unary_op::COS)
    }
    pub(crate) fn fxch(&mut self, i: u32) -> Result<(), Error> {
        Self::ok(x87::abi::xch(&mut self.0, i))
    }
    /// `fst dword` (no pop).
    pub(crate) fn fst_m32(&mut self) -> Result<u32, Error> {
        let (code, store) = x87::abi::st_f32(&self.0, 0);
        Self::ok(code)?;
        x87::abi::store_commit(&mut self.0, store.status, false);
        Ok(store.value)
    }
    /// `fild dword`.
    pub(crate) fn fild(&mut self, v: i32) -> Result<(), Error> {
        Self::ok(x87::abi::ld_int(&mut self.0, v as i64))
    }
    /// `fld st(i)`.
    pub(crate) fn fld_st(&mut self, i: u32) -> Result<(), Error> {
        Self::ok(x87::abi::ld_st(&mut self.0, i))
    }
    /// `fstp st(i)`.
    pub(crate) fn fstp_st(&mut self, i: u32) -> Result<(), Error> {
        Self::ok(x87::abi::st_st(&mut self.0, i, true))
    }
    /// `op st(dst), st(src)`, popping after with `pop` (`fmulp`, `faddp`).
    pub(crate) fn arith_reg(&mut self, op: u32, dst: u32, src: u32, pop: bool) -> Result<(), Error> {
        let op = if pop { op | x87::abi::ARITH_POP } else { op };
        Self::ok(x87::abi::arith(
            &mut self.0,
            op,
            dst,
            x87::abi::src_kind::ST,
            src as u64,
        ))
    }
    /// `fcomip st, st(i)`: the EFLAGS bits (ZF `0x40`, PF `0x04`, CF `0x01`).
    pub(crate) fn fcomip(&mut self, i: u32) -> Result<u32, Error> {
        let r = x87::abi::cmp(&mut self.0, 0, x87::abi::src_kind::ST, i as u64, 1, true);
        Self::ok(r)?;
        Ok(r & 0x45)
    }
    /// `fstp dword`.
    pub(crate) fn fstp_m32(&mut self) -> Result<u32, Error> {
        let (code, store) = x87::abi::st_f32(&self.0, 0);
        Self::ok(code)?;
        x87::abi::store_commit(&mut self.0, store.status, true);
        Ok(store.value)
    }
}

/// `(sin(a * WAVE_A + b), (sin(a * WAVE_B) * cos(b * WAVE_C) * sin(a) +
/// cos(b * WAVE_D)) * HALF)`, each stored as f32: `0x000498a7`..`0x0004997c`.
fn sway(m: &dyn GuestMemory, control: u16, a: u32, b: u32) -> Result<(u32, u32), Error> {
    use constant::*;
    let mut x = X87::new(control);
    x.fld(a)?;
    x.fmul_m(m.u32(WAVE_A)?)?;
    x.fadd_m(b)?;
    x.fsin()?;
    let side = x.fstp_m32()?;
    x.fld(a)?;
    x.fmul_m(m.u32(WAVE_B)?)?;
    x.fld(a)?;
    x.fsin()?;
    x.fld(b)?;
    x.fmul_m(m.u32(WAVE_C)?)?;
    x.fcos()?;
    x.fld(b)?;
    x.fmul_m(m.u32(WAVE_D)?)?;
    x.fcos()?;
    x.fxch(3)?;
    x.fsin()?;
    x.arith_st(x87::abi::arith_op::MUL, 1)?;
    x.arith_st(x87::abi::arith_op::MUL, 2)?;
    x.arith_st(x87::abi::arith_op::ADD, 3)?;
    x.fmul_m(m.u32(HALF)?)?;
    let up = x.fstp_m32()?;
    Ok((side, up))
}

fn vec3(m: &dyn GuestMemory, at: u32) -> Result<[u32; 3], MemFault> {
    Ok([m.u32(at)?, m.u32(at.wrapping_add(4))?, m.u32(at.wrapping_add(8))?])
}

/// `0x000497a0`'s results.
struct Frame {
    words: [u32; FRAME_WORDS],
    copy_a: [u32; 4],
    copy_b: [u32; 4],
    mode: u32,
}

/// `0x000497a0`, with the arguments `0x0004a1e0` gives it: the frame block
/// at [`global::FRAME`], `p` the light's frame block, `d` the definition,
/// the two colours, the scale.
fn light_frame(
    m: &dyn GuestMemory,
    f: &mut Sse,
    control: u16,
    ctx: u32,
    p: u32,
    d: u32,
    colour_a: [u32; 3],
    colour_b: [u32; 3],
    k: u32,
) -> Result<Frame, Error> {
    use constant::*;
    let mut out = [0u32; FRAME_WORDS];
    for (i, w) in out.iter_mut().enumerate() {
        *w = m.u32(global::FRAME + 4 * i as u32)?;
    }
    let minus_one = m.u32(MINUS_ONE)?;
    let one = m.u32(ONE)?;
    let half = m.u32(HALF)?;
    let zero = 0u32;

    // Which of two slots the context's projection block goes in.
    let (persp, edi) = if m.u16(ctx)? == 0 {
        (ctx + 0x38, 0)
    } else {
        (0, ctx + 0x38)
    };
    let flags = m.u32(d)?;
    let special = flags & 0x20 != 0 && flags & 0x40000 != 0;
    let mut o = vec3(
        m,
        if special {
            global::SPECIAL_ORIGIN
        } else {
            ctx + 0x2c
        },
    )?;
    let fw = vec3(m, if special { global::SPECIAL_FORWARD } else { p + 0x28 })?;
    let g = vec3(
        m,
        if special {
            global::SPECIAL_FORWARD
        } else if edi != 0 {
            edi + 4
        } else {
            p + 0x28
        },
    )?;
    let u = vec3(m, if special { global::SPECIAL_UP } else { p + 0x34 })?;
    let [fx, fy, fz] = fw;
    let [ux, uy, uz] = u;

    let v = if special {
        let l28 = f.mul(ux, fy)?;
        let x1 = f.mul(uy, fx)?;
        let l24 = f.sub(l28, x1)?;
        let l2c = f.mul(uz, fx)?;
        let x2 = f.mul(ux, fz)?;
        let l20 = f.sub(l2c, x2)?;
        let l30 = f.mul(uy, fz)?;
        let x3 = f.mul(uz, fy)?;
        let l4c = f.sub(l30, x3)?;
        let (side, up) = sway(m, control, o[0], o[1])?;
        let q = f.mul(up, m.u32(SWAY_UP)?)?;
        let a = f.mul(q, ux)?;
        o[0] = f.add(a, o[0])?;
        let a = f.mul(uy, q)?;
        o[1] = f.add(a, o[1])?;
        let a = f.mul(uz, q)?;
        o[2] = f.add(a, o[2])?;
        let t = f.mul(side, m.u32(SWAY_SIDE)?)?;
        let a = f.mul(t, l4c)?;
        o[0] = f.add(a, o[0])?;
        let a = f.mul(t, l20)?;
        let y = f.add(a, o[1])?;
        let vz = f.sub(x1, l28)?;
        let vy = f.sub(x2, l2c)?;
        let vx = f.sub(x3, l30)?;
        let a = f.mul(t, l24)?;
        o[2] = f.add(a, o[2])?;
        o[1] = y;
        [vx, vy, vz]
    } else {
        vec3(m, ctx + 0x1c)?
    };

    // The scale: -1 over the projection's depth range.
    let n = m.u16(d + 0x8e)? as i16;
    let mut s = minus_one;
    if n != 1 {
        let x0 = if persp != 0 {
            m.u32(persp + 0xc)?
        } else {
            m.u32(edi + 0x2c)?
        };
        s = f.div(minus_one, x0)?;
    } else if edi != 0 {
        let x0 = f.sub(m.u32(edi + 0x2c)?, m.u32(edi + 0x28)?)?;
        s = f.div(minus_one, x0)?;
    }

    let ty = m.u32(ctx)?;
    if ty != 1 {
        out[0] = o[0];
        out[1] = o[1];
        out[2] = o[2];
        out[3] = one;
        out[4] = o[0];
        out[5] = o[1];
        out[6] = o[2];
    }
    if ty == 0 {
        let c = vec3(m, ctx + 0x1c)?;
        out[8] = f.mul(c[0], s)?;
        out[9] = f.mul(c[1], s)?;
        out[10] = f.mul(c[2], s)?;
        out[12] = f.mul(m.u32(p + 0x34)?, s)?;
        out[13] = f.mul(m.u32(p + 0x38)?, s)?;
        out[14] = f.mul(m.u32(p + 0x3c)?, s)?;
        out[11] = zero;
        out[15] = zero;
    } else if ty != 1 {
        let [gx, gy, gz] = g;
        let [vx, vy, vz] = v;
        let a = f.mul(uy, gx)?;
        let b = f.mul(gy, ux)?;
        let c0 = f.sub(a, b)?;
        let gz_uy = f.mul(gz, uy)?;
        let a = f.mul(gz, ux)?;
        let b = f.mul(uz, gx)?;
        let c1 = f.sub(a, b)?;
        let a = f.mul(gy, uz)?;
        let c2 = f.sub(a, gz_uy)?;
        let a = f.mul(vy, gx)?;
        let gy_vz = f.mul(gy, vz)?;
        let b = f.mul(gy, vx)?;
        let d0 = f.sub(a, b)?;
        let gz_vx = f.mul(gz, vx)?;
        let vz_gx = f.mul(vz, gx)?;
        let gz_vy = f.mul(gz, vy)?;
        let d1 = f.sub(gy_vz, gz_vy)?;
        let r = f.div(minus_one, m.u32(edi + 0x14)?)?;
        let c0 = f.mul(c0, r)?;
        let c2 = f.mul(c2, r)?;
        let c2 = f.add(c2, fx)?;
        let c1 = f.mul(c1, r)?;
        let r = f.div(minus_one, m.u32(edi + 0x18)?)?;
        let c1 = f.add(c1, fy)?;
        let d2 = f.sub(gz_vx, vz_gx)?;
        let e5 = f.mul(d1, r)?;
        let e5 = f.add(e5, fx)?;
        let e3 = f.mul(d0, r)?;
        let c0 = f.add(c0, fz)?;
        let e4 = f.mul(d2, r)?;
        let e4 = f.add(e4, fy)?;
        let e3 = f.add(e3, fz)?;
        let h = f.mul(s, half)?;
        out[10] = f.mul(c0, h)?;
        out[8] = f.mul(c2, h)?;
        out[9] = f.mul(c1, h)?;
        out[12] = f.mul(h, e5)?;
        out[14] = f.mul(e3, h)?;
        out[13] = f.mul(e4, h)?;
        out[11] = zero;
        out[15] = zero;
    }

    if n != 1 {
        let tail = m.u32(global::FRAME_TAIL)?;
        out[16] = f.mul(fx, s)?;
        out[17] = f.mul(fy, s)?;
        out[18] = f.mul(fz, s)?;
        out[19] = f.mul(s, half)?;
        let t = vec3(m, tail)?;
        out[20..23].copy_from_slice(&t);
        out[23] = zero;
    } else if ty != 1 {
        let src = if special { global::SPECIAL_FORWARD } else { p + 0x28 };
        out[16] = f.mul(s, m.u32(src)?)?;
        out[17] = f.mul(m.u32(src + 4)?, s)?;
        out[18] = f.mul(m.u32(src + 8)?, s)?;
        let a = f.add(m.u32(edi + 0x28)?, m.u32(ctx + 0x28)?)?;
        out[19] = f.mul(a, s)?;
        if m.u8(d)? & 0x20 != 0 {
            let tail = m.u32(global::FRAME_TAIL)?;
            let t = vec3(m, tail)?;
            out[20..23].copy_from_slice(&t);
            out[23] = one;
        } else {
            let a = f.sub(m.u32(edi + 0x2c)?, m.u32(edi + 0x28)?)?;
            let a = f.mul(a, m.u32(SWAY_SIDE)?)?;
            let x2 = f.mul(a, half)?;
            out[20] = f.mul(out[16], x2)?;
            out[21] = f.mul(x2, out[17])?;
            out[22] = f.mul(out[18], x2)?;
            let a = f.add(m.u32(ctx + 0x28)?, m.u32(BIAS)?)?;
            let a = f.mul(a, x2)?;
            let a = f.mul(a, s)?;
            out[23] = f.add(a, half)?;
        }
    }

    let copy_a = [out[8], out[9], out[10], out[11]];
    let copy_b = [out[12], out[13], out[14], out[15]];
    let k6c = m.u32(p + 0x6c)?;
    for i in 0..3 {
        let a = f.mul(out[16 + i], k6c)?;
        out[8 + i] = f.add(a, out[8 + i])?;
    }
    let k70 = m.u32(p + 0x70)?;
    for i in 0..3 {
        let a = f.mul(out[16 + i], k70)?;
        out[12 + i] = f.add(a, out[12 + i])?;
    }

    // The colours, scaled down when either is too bright.
    let mut a = colour_a;
    let mut b = colour_b;
    for c in &mut b {
        *c = f.mul(*c, k)?;
    }
    for c in &mut a {
        *c = f.mul(*c, k)?;
    }
    let m1 = if f.above(a[1], a[2])? { a[1] } else { a[2] };
    let max_a = if f.above(a[0], m1)? { a[0] } else { m1 };
    let m2 = if f.above(b[1], b[2])? { b[1] } else { b[2] };
    let max_b = if f.above(b[0], m2)? { b[0] } else { m2 };
    let max = if f.above(max_a, max_b)? { max_a } else { max_b };
    let (scale, mode) = if f.above(max, m.u32(TWO)?)? {
        (Some(m.u32(QUARTER)?), 2)
    } else if f.above(max, one)? {
        (Some(half), 1)
    } else {
        (None, 0)
    };
    for (i, v) in a.into_iter().chain(b).enumerate() {
        let v = match scale {
            Some(q) => f.mul(v, q)?,
            None => v,
        };
        out[24 + i + (i >= 3) as usize] = f.clamp(v, zero, one)?;
    }
    Ok(Frame {
        words: out,
        copy_a,
        copy_b,
        mode,
    })
}

/// `0x0004a1e0`: the block it stores for `call` under `fpu`, `bounds`
/// standing in for `0x001650a0`. Reads guest memory, writes none.
pub fn light_globals(
    m: &dyn GuestMemory,
    call: &Call,
    fpu: Fpu,
    bounds: &mut dyn FnMut(BoundsQuery) -> Option<[u32; 6]>,
) -> Result<LightGlobals, Error> {
    compute_globals(m, call, fpu, |query| Ok(bounds(query)))
}

/// Compute the light block with the sphere projection used by the guest.
/// A projection error refuses the setup. An offscreen sphere takes the
/// original's whole-viewport fallback.
pub fn projected_light_globals(
    m: &dyn GuestMemory,
    call: &Call,
    fpu: Fpu,
) -> Result<LightGlobals, Error> {
    compute_globals(m, call, fpu, |query| screen_bounds(m, query, fpu))
}

fn compute_globals(
    m: &dyn GuestMemory,
    call: &Call,
    fpu: Fpu,
    mut bounds: impl FnMut(BoundsQuery) -> Result<Option<[u32; 6]>, Error>,
) -> Result<LightGlobals, Error> {
    use constant::*;
    let mut f = Sse { mxcsr: fpu.mxcsr };
    let eax = call.frame;
    let record = if call.light == u32::MAX {
        0
    } else {
        let base = m.u32(m.u32(global::LIGHT_ARRAY)?.wrapping_add(0x44))?;
        (call.light & 0xffff).wrapping_mul(0x110).wrapping_add(base)
    };
    let view = m.u32(global::VIEW)?;
    // The guest pointers name contiguous blocks. An end that would wrap
    // cannot describe one, even when the low guest page is mapped.
    for (base, last) in [(eax, 0x87), (call.context, 0x67), (call.definition, 0x8f),
                         (record, 0x55), (view, 0xb3)] {
        base.checked_add(last).ok_or(Error::Data)?;
    }
    let driver = if record != 0 && m.u16(record + 0x54)? != 0xffff {
        m.u32(record + 0x4c)?
    } else {
        u32::MAX
    };
    let mut colour_a = vec3(m, eax + 0x50)?;
    let mut colour_b = vec3(m, eax + 0x5c)?;
    let scale = m.u32(eax + 0x68)?;
    let context_word = m.u16(call.context)?;

    let falloff = if record != 0 {
        let a7c = m.u32(eax + 0x7c)?;
        let a80 = m.u32(eax + 0x80)?;
        let x1 = f.mul(a7c, call.weight)?;
        let x0 = if f.above(a80, a7c)? { a7c } else { a80 };
        for c in &mut colour_a {
            *c = f.mul(*c, x1)?;
        }
        for c in &mut colour_b {
            *c = f.mul(*c, x0)?;
        }
        [
            m.u32(eax + 0x74)?,
            m.u32(eax + 0x78)?,
            x1,
            x0,
            m.u32(eax + 0x84)?,
        ]
    } else {
        let one = m.u32(ONE)?;
        [one, one, one, one, 0]
    };

    let frame = light_frame(
        m,
        &mut f,
        fpu.x87_control,
        call.context,
        eax,
        call.definition,
        colour_a,
        colour_b,
        scale,
    )?;

    let short = |at: u32| -> Result<i32, MemFault> { Ok(m.u16(at)? as i16 as i32) };
    let (x0, y0, x1, y1) = (
        short(global::VIEWPORT + 2)?,
        short(global::VIEWPORT)?,
        short(global::VIEWPORT + 6)?,
        short(global::VIEWPORT + 4)?,
    );
    let fallback = |f: &mut Sse| -> Result<[u32; 6], Error> {
        Ok([
            f.int(x0)?,
            f.int(x1)?,
            f.int(y0)?,
            f.int(y1)?,
            f.sub(0, m.u32(view + 0x8c)?)?,
            f.sub(0, m.u32(view + 0x74)?)?,
        ])
    };
    let (rectangle, sphere) = if record == 0 {
        (fallback(&mut f)?, None)
    } else {
        let q = BoundsQuery {
            centre: record + 0x28,
            view: view + 4,
            word: m.u32(record + 0x34)?,
        };
        let rect = match bounds(q)? {
            None => fallback(&mut f)?,
            Some(r) => match project(m, &mut f, view, r, [x0, x1, y0, y1])? {
                Some(rect) => rect,
                None => fallback(&mut f)?,
            },
        };
        let mut s = [0u32; 4];
        for (i, w) in s.iter_mut().enumerate() {
            *w = m.u32(record + 0x18 + 4 * i as u32)?;
        }
        (rect, Some(s))
    };

    Ok(LightGlobals {
        definition: call.definition,
        context_word,
        driver,
        colour_a,
        colour_b,
        scale,
        falloff,
        frame: frame.words,
        frame_copy_a: frame.copy_a,
        frame_copy_b: frame.copy_b,
        mode: frame.mode,
        rectangle,
        rectangle_valid: record != 0,
        sphere,
        shadow: call.shadow as u8,
    })
}

/// `0x0004a3fa`..`0x0004a644`: the bounds mapped onto the viewport, or
/// `None` where the original falls back to the whole viewport.
fn project(
    m: &dyn GuestMemory,
    f: &mut Sse,
    view: u32,
    r: [u32; 6],
    [vx0, vx1, vy0, vy1]: [i32; 4],
) -> Result<Option<[u32; 6]>, Error> {
    let a4 = m.u32(view + 0xa4)?;
    let a8 = m.u32(view + 0xa8)?;
    let ac = m.u32(view + 0xac)?;
    let b0 = m.u32(view + 0xb0)?;
    if f.equal(a8, a4)? || f.equal(b0, ac)? {
        return Ok(None);
    }
    if f.below(a8, r[0])? || f.below(r[1], a4)? || f.below(b0, r[2])? || f.below(r[3], ac)? {
        return Ok(None);
    }
    let x3 = f.clamp(r[0], a4, a8)?;
    let x4 = f.clamp(r[1], a4, a8)?;
    let t = f.sub(0, r[3])?;
    let x2 = f.clamp(t, ac, b0)?;
    let t = f.sub(0, r[2])?;
    let x1 = f.clamp(t, ac, b0)?;
    let x3 = f.sub(x3, a4)?;
    let width = f.sub(a8, a4)?;
    let x4 = f.sub(x4, a4)?;
    let x2 = f.sub(x2, ac)?;
    let x1 = f.sub(x1, ac)?;
    let w = vx1.wrapping_sub(vx0);
    let h = vy1.wrapping_sub(vy0);
    let a = f.int(w)?;
    let a = f.mul(a, x3)?;
    let a = f.div(a, width)?;
    let o = f.int(vx0)?;
    let left = f.add(a, o)?;
    let a = f.int(w)?;
    let a = f.mul(a, x4)?;
    let a = f.div(a, width)?;
    let o = f.int(vx0)?;
    let right = f.add(a, o)?;
    let height = f.sub(b0, ac)?;
    let a = f.int(h)?;
    let a = f.mul(a, x2)?;
    let a = f.div(a, height)?;
    let o = f.int(vy0)?;
    let top = f.add(a, o)?;
    let a = f.int(h)?;
    let a = f.mul(a, x1)?;
    let a = f.div(a, height)?;
    let o = f.int(vy0)?;
    let bottom = f.add(a, o)?;
    Ok(Some([left, right, top, bottom, r[4], r[5]]))
}
