//! The view end's camera, bit for bit (docs/graphics/retire-d3d-display.md,
//! "The view setup and view end"): [`update`] is `0x0002f970` (EAX the
//! camera [`CAMERA`], ESI a 16-byte local) and [`finish`] is `0x0002fd90`
//! (EDX the local, EBX [`CAMERA_POINTS`], ESI [`CAMERA`]) with its callees,
//! the projection `0x0002fb70` (`engine::view_projection`) and the inverse
//! `0x00141590` ([`inverse`]).
//!
//! Each reads guest memory where the body reads it and writes it where the
//! body writes it, in the body's order, the local included. The bodies'
//! own frames (below the caller's ESP) are dead on return and are not
//! written. The scalar SSE is x86's: a NaN operand gives the first NaN
//! operand quietened, an invalid result the indefinite `0xffc00000`. The
//! x87 parts run at double precision (control word `0x027f`), where every
//! product of two floats is exact and no intermediate leaves the double's
//! range, so `f64` is the x87 here; a NaN on the x87 is refused (`x87`).
//! MXCSR must round to nearest with every exception masked and no DAZ or
//! FTZ (`simd` otherwise); its sticky flags and the x87 status word are not
//! kept.

use guest_layout::memory::GuestMemory;
use engine::view_projection::{self, Call, Refusal};

pub use crate::view_setup::{CAMERA, CAMERA_POINTS};

type Cause = &'static str;

/// `one`, `half` and the near extent the planes end with.
pub const ONE: u32 = 0x0045_dbc0;
pub const HALF: u32 = 0x0045_dbbc;
pub const NEAR: u32 = 0x0045_dbcc;
/// The window's scale in [`update`].
pub const WINDOW_SCALE: u32 = 0x0045_dc0c;
/// The lens shift's factors in [`update`] (`+0x68` set).
pub const SHIFT_X: u32 = 0x0045_dd54;
pub const SHIFT_Y: u32 = 0x0045_dd50;
/// The axes' least length [`finish`] normalises.
pub const MIN_LENGTH: u32 = 0x0045_dbdc;
/// The lens byte [`update`] sets, and the settings whose word `+0x10` = 2
/// sets it too.
pub const LENS_FLAG: u32 = 0x004b_a019;
pub const SETTINGS: u32 = 0x004e_0350;

/// The float state the bodies run under: MXCSR (sticky flags aside) and
/// the x87 control word.
pub const MXCSR: u32 = 0x1f80;
pub const X87_CONTROL: u16 = 0x027f;

const INDEFINITE: u32 = 0xffc0_0000;

/// What [`update`] leaves in the registers the caller can see. EDI is
/// restored; the XMM registers are not returned: [`finish`] writes all
/// eight before it reads one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Updated {
    /// `(CAMERA & !0xff) | 1`: `mov al, 1`.
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// What [`finish`] leaves: EAX 0, ECX 0 and EDX `CAMERA_POINTS + 0x78`
/// (the `rep stosd`), and lane 0 of XMM0 to XMM7, whose lanes 1 to 3 are
/// all zero. EDI is restored and the x87 stack is empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Finished {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub xmm: [u32; 8],
}

fn simd_state(mxcsr: u32) -> Result<(), Cause> {
    if mxcsr & !0x3f == MXCSR { Ok(()) } else { Err("simd") }
}

// Scalar SSE on binary32 bits, operands in the instruction's order.

fn ss(a: u32, b: u32, op: fn(f32, f32) -> f32) -> u32 {
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    if x.is_nan() {
        a | 0x0040_0000
    } else if y.is_nan() {
        b | 0x0040_0000
    } else {
        let r = op(x, y);
        if r.is_nan() { INDEFINITE } else { r.to_bits() }
    }
}

fn add(a: u32, b: u32) -> u32 {
    ss(a, b, |x, y| x + y)
}

fn sub(a: u32, b: u32) -> u32 {
    ss(a, b, |x, y| x - y)
}

fn mul(a: u32, b: u32) -> u32 {
    ss(a, b, |x, y| x * y)
}

fn div(a: u32, b: u32) -> u32 {
    ss(a, b, |x, y| x / y)
}

/// `cvtsi2ss`, rounding to nearest.
fn cvt(i: u32) -> u32 {
    (i as i32 as f32).to_bits()
}

/// `ucomiss a, b` then `lahf; test ah, 44h; jnp`: equal and ordered.
fn equal(a: u32, b: u32) -> bool {
    f32::from_bits(a) == f32::from_bits(b)
}

/// `comiss a, b; jbe` not taken: above and ordered.
fn above(a: u32, b: u32) -> bool {
    f32::from_bits(a) > f32::from_bits(b)
}

// The x87 at double precision.

fn fld(bits: u32) -> Result<f64, Cause> {
    let v = f32::from_bits(bits);
    if v.is_nan() { Err("x87") } else { Ok(f64::from(v)) }
}

fn fx(v: f64) -> Result<f64, Cause> {
    if v.is_nan() { Err("x87") } else { Ok(v) }
}

/// `fst dword`.
fn fst(v: f64) -> u32 {
    (v as f32).to_bits()
}

/// `fld a; fmul a; fld b; fmul b; faddp; fld c; fmul c; faddp; fsqrt`.
fn length(a: u32, b: u32, c: u32) -> Result<f64, Cause> {
    let sq = |w: u32| -> Result<f64, Cause> { fx(fld(w)? * fld(w)?) };
    let s = fx(fx(sq(a)? + sq(b)?)? + sq(c)?)?;
    fx(s.sqrt())
}

/// `fst [esp]; fabs; fld dword [MIN_LENGTH]; fcomip; fstp st0; ja`: the
/// stored length, and whether the body normalises (the jump not taken).
fn long_enough(len: f64, min: u32) -> Result<(u32, bool), Cause> {
    Ok((fst(len), !(fld(min)? > len.abs())))
}

struct G<'a>(&'a mut dyn GuestMemory);

impl G<'_> {
    fn r(&self, at: u32) -> Result<u32, Cause> {
        self.0.u32(at).map_err(|_| "unread")
    }
    fn b(&self, at: u32) -> Result<u8, Cause> {
        self.0.u8(at).map_err(|_| "unread")
    }
    fn h(&self, at: u32) -> Result<u16, Cause> {
        self.0.u16(at).map_err(|_| "unread")
    }
    /// `movsx r32, word`.
    fn sx(&self, at: u32) -> Result<u32, Cause> {
        Ok(self.h(at)? as i16 as i32 as u32)
    }
    fn w(&mut self, at: u32, v: u32) -> Result<(), Cause> {
        self.0.set_u32(at, v).map_err(|_| "unwritable")
    }
    fn wb(&mut self, at: u32, v: u8) -> Result<(), Cause> {
        self.0.set_u8(at, v).map_err(|_| "unwritable")
    }
}

fn low(r: u32, byte: u8) -> u32 {
    (r & !0xff) | u32::from(byte)
}

fn c(off: u32) -> u32 {
    CAMERA.wrapping_add(off)
}

fn p(off: u32) -> u32 {
    CAMERA_POINTS.wrapping_add(off)
}

/// `0x0002f970` (**Code** `0x0002f970..0x0002fb66`): the camera's window
/// over its viewport to the local's four extents, the lens byte
/// [`LENS_FLAG`], then the camera's `+0x58` lens (its centre `+0x5c`,
/// `+0x60` and size `+0x64`) or its `+0x68` shift (`+0x6c`, `+0x70`) on the
/// extents.
pub fn update(mem: &mut dyn GuestMemory, local: u32, mxcsr: u32) -> Result<Updated, Cause> {
    simd_state(mxcsr)?;
    let mut g = G(mem);
    let l = |off: u32| local.wrapping_add(off);
    let mut edx = g.sx(c(0x3a))?;
    let mut ecx = g.sx(c(0x3e))?;
    let mut x1 = g.r(WINDOW_SCALE)?;
    ecx = ecx.wrapping_add(edx);
    edx = g.sx(c(0x38))?;
    let x5 = g.r(HALF)?;
    let mut x3 = cvt(ecx);
    ecx = g.sx(c(0x3c))?;
    let edi = edx.wrapping_add(ecx);
    ecx = ecx.wrapping_sub(edx);
    edx = g.sx(c(0x34))?;
    let mut x0 = cvt(edi);
    let edi = g.sx(c(0x30))?;
    let mut x2 = cvt(ecx);
    ecx = g.sx(c(0x32))?;
    x1 = div(x1, x2);
    edx = edx.wrapping_sub(edi);
    x2 = cvt(edx);
    edx = g.sx(c(0x36))?;
    edx = edx.wrapping_sub(ecx);
    x3 = mul(x3, x5);
    let mut x4 = cvt(edx);
    x2 = div(x2, x4);
    x4 = cvt(ecx);
    x4 = sub(x4, x3);
    x2 = mul(x2, x1);
    x4 = mul(x4, x2);
    g.w(l(0), x4)?;
    ecx = g.sx(c(0x36))?;
    x4 = cvt(ecx);
    x4 = sub(x4, x3);
    x4 = mul(x4, x2);
    g.w(l(4), x4)?;
    edx = g.sx(c(0x34))?;
    x0 = mul(x0, x5);
    x3 = x0;
    x2 = cvt(edx);
    x3 = sub(x3, x2);
    x3 = mul(x3, x1);
    g.w(l(8), x3)?;
    ecx = g.sx(c(0x30))?;
    x2 = cvt(ecx);
    x0 = sub(x0, x2);
    x0 = mul(x0, x1);
    g.w(l(0xc), x0)?;

    let lens = g.b(c(0x58))?;
    ecx = low(ecx, lens);
    if lens == 0 {
        let shift = g.b(c(0x68))?;
        ecx = low(ecx, shift);
        g.wb(LENS_FLAG, 0)?;
        if shift != 0 {
            g.wb(LENS_FLAG, 1)?;
        }
    } else {
        g.wb(LENS_FLAG, 1)?;
    }
    ecx = g.r(SETTINGS)?;
    if ecx != 0 {
        let two = u8::from(g.h(ecx.wrapping_add(0x10))? == 2);
        let flag = g.b(LENS_FLAG)? | two;
        edx = low(edx, two);
        ecx = low(ecx, flag);
        g.wb(LENS_FLAG, flag)?;
    }
    let eax = (CAMERA & !0xff) | 1;
    let lens = g.b(c(0x58))?;
    ecx = low(ecx, lens);
    if lens != 0 {
        let mut x0 = g.r(c(0x5c))?;
        let mut x2 = g.r(c(0x64))?;
        let x6 = g.r(ONE)?;
        let mut x1 = g.r(l(4))?;
        x1 = sub(x1, g.r(l(0))?);
        let mut x3 = g.r(c(0x64))?;
        let mut x4 = g.r(l(0xc))?;
        x4 = sub(x4, g.r(l(8))?);
        x2 = mul(x2, x1);
        x0 = add(x0, x6);
        x0 = mul(x0, x5);
        x0 = mul(x0, x1);
        x1 = g.r(c(0x60))?;
        x0 = add(x0, g.r(l(0))?);
        x1 = add(x1, x6);
        x3 = mul(x3, x4);
        x1 = mul(x1, x5);
        x1 = mul(x1, x4);
        x1 = add(x1, g.r(l(8))?);
        x4 = x0;
        x2 = mul(x2, x5);
        x4 = sub(x4, x2);
        x2 = add(x2, x0);
        x3 = mul(x3, x5);
        x0 = x1;
        x0 = sub(x0, x3);
        x3 = add(x3, x1);
        g.w(l(0), x4)?;
        g.w(l(8), x0)?;
        g.w(l(0xc), x3)?;
        g.w(l(4), x2)?;
        return Ok(Updated { eax, ecx, edx });
    }
    let shift = g.b(c(0x68))?;
    ecx = low(ecx, shift);
    if shift != 0 {
        let mut x0 = g.r(l(4))?;
        x0 = sub(x0, g.r(l(0))?);
        x0 = mul(x0, g.r(SHIFT_X)?);
        x0 = mul(x0, g.r(c(0x6c))?);
        let mut x1 = g.r(l(0xc))?;
        x1 = sub(x1, g.r(l(8))?);
        let mut x2 = g.r(l(0))?;
        x1 = mul(x1, g.r(SHIFT_Y)?);
        x1 = mul(x1, g.r(c(0x70))?);
        x2 = add(x2, x0);
        g.w(l(0), x2)?;
        x2 = g.r(l(4))?;
        x2 = add(x2, x0);
        x0 = g.r(l(8))?;
        x0 = sub(x0, x1);
        g.w(l(8), x0)?;
        x0 = g.r(l(0xc))?;
        x0 = sub(x0, x1);
        g.w(l(0xc), x0)?;
        g.w(l(4), x2)?;
    }
    Ok(Updated { eax, ecx, edx })
}

fn refused(r: Refusal) -> Cause {
    match r {
        Refusal::Memory => "unread",
        Refusal::Layout => "layout",
        Refusal::Unsupported => "projection",
    }
}

/// `0x0002fd90` (**Code** `0x0002fd90..0x0003028d`) with `esp` its ESP on
/// entry (the return address's slot; the view end's ESP less `0x20`): the
/// projection `0x0002fb70` of the camera over the local's extents into its
/// frame, the camera's right, up and back axes (normalised when at least
/// [`MIN_LENGTH`] long) and position to `CAMERA_POINTS + 0x38..0x68` with
/// `one` at `+0x34`, their inverse `0x00141590` to `+0x00..0x34`, the
/// projection's offsets and scales to `+0x68..0x78`, `+0xb8` and `+0xbc`,
/// and the clip rows `+0x78..0xb8`, whose depth row takes the camera's
/// plane `+0x48..0x54` through the inverse when `+0x40` is 0.
pub fn finish(mem: &mut dyn GuestMemory, local: u32, esp: u32, mxcsr: u32, x87_control: u16) -> Result<Finished, Cause> {
    simd_state(mxcsr)?;
    if x87_control != X87_CONTROL {
        return Err("x87");
    }
    let output = esp.wrapping_sub(0x74).wrapping_add(0x34);
    let built = view_projection::build(&*mem, Call { camera: CAMERA, extents: local, output }, mxcsr, x87_control)
        .map_err(refused)?;
    let b = |off: u32| built.words[(off as usize - 0x34) / 4];
    let mut g = G(mem);

    // The axes: x = up × forward, y = forward × x, z = 0 − forward.
    let mut x2 = g.r(c(0x20))?;
    let mut x0 = g.r(c(0x1c))?;
    x0 = mul(x0, g.r(c(0x14))?);
    x2 = mul(x2, g.r(c(0x10))?);
    let mut x1 = g.r(c(0x20))?;
    x1 = mul(x1, g.r(c(0xc))?);
    let mut x3 = g.r(c(0x10))?;
    x3 = mul(x3, g.r(c(0x18))?);
    x2 = sub(x2, x0);
    x0 = g.r(c(0x14))?;
    x0 = mul(x0, g.r(c(0x18))?);
    x0 = sub(x0, x1);
    x1 = g.r(c(0x1c))?;
    x1 = mul(x1, g.r(c(0xc))?);
    x1 = sub(x1, x3);
    let (mut s2c, mut s28, mut s24) = (x1, x0, x2);
    let len = length(s2c, s28, s24)?;
    let mut x5 = g.r(c(0x14))?;
    x3 = g.r(c(0x10))?;
    let mut x4 = g.r(c(0x14))?;
    let mut x6 = g.r(c(0xc))?;
    x3 = mul(x3, x1);
    x5 = mul(x5, x0);
    x5 = sub(x5, x3);
    x3 = g.r(c(0xc))?;
    x4 = mul(x4, x2);
    x3 = mul(x3, x1);
    x3 = sub(x3, x4);
    x4 = g.r(c(0x10))?;
    x6 = mul(x6, x0);
    x4 = mul(x4, x2);
    x4 = sub(x4, x6);
    x6 = 0;
    let mut s0c = sub(x6, g.r(c(0xc))?);
    let mut s10 = sub(x6, g.r(c(0x10))?);
    let mut s14 = sub(x6, g.r(c(0x14))?);
    let mut x7 = g.r(ONE)?;
    let (mut s18, mut s1c, mut s20) = (x5, x3, x4);
    let min = g.r(MIN_LENGTH)?;
    let (s0, normalise) = long_enough(len, min)?;
    if normalise {
        let k = div(x7, s0);
        s24 = mul(k, x2);
        x2 = k;
        x5 = s18;
        x0 = mul(x0, x2);
        x1 = mul(x1, x2);
        s28 = x0;
        s2c = x1;
    }
    let (s0, normalise) = long_enough(length(s20, s1c, s18)?, min)?;
    if normalise {
        x0 = div(x7, s0);
        x1 = mul(x0, x5);
        x3 = mul(x3, x0);
        x4 = mul(x4, x0);
        (s18, s1c, s20) = (x1, x3, x4);
    }
    let (s0, normalise) = long_enough(length(s14, s10, s0c)?, min)?;
    if normalise {
        x0 = div(x7, s0);
        s0c = mul(x0, s0c);
        s10 = mul(s10, x0);
        s14 = mul(s14, x0);
    }
    let _ = (x0, x1, x2, x3, x4, x5);
    for (k, v) in [s24, s28, s2c, s18, s1c, s20, s0c, s10, s14].into_iter().enumerate() {
        g.w(p(0x38 + 4 * k as u32), v)?;
    }
    for k in 0..3 {
        let v = g.r(c(4 * k))?;
        g.w(p(0x5c + 4 * k), v)?;
    }
    g.w(p(0x34), x7)?;
    inverse(&mut g, p(0x34), p(0))?;

    // The projection's offsets and scales.
    let x0h = g.r(HALF)?;
    let mut x1 = mul(b(0x5c), b(0x44));
    x1 = mul(x1, x0h);
    g.w(p(0x68), b(0x64))?;
    g.w(p(0xb8), x1)?;
    x1 = mul(b(0x60), b(0x48));
    g.w(p(0x6c), b(0x68))?;
    g.w(p(0x70), b(0x6c))?;
    x1 = mul(x1, x0h);
    g.w(p(0xbc), x1)?;
    g.w(p(0x74), b(0x70))?;

    // The depth row: the plane through the inverse, or `+0x40`'s.
    let c40 = g.r(c(0x40))?;
    let (mut x0, mut x1, mut x2, mut x3);
    if equal(c40, x6) {
        let mut x4 = g.r(c(0x4c))?;
        let mut x5 = g.r(c(0x50))?;
        x1 = g.r(c(0x48))?;
        x2 = g.r(p(0x1c))?;
        x0 = g.r(p(0x10))?;
        x3 = g.r(p(0x20))?;
        x0 = mul(x0, x4);
        x2 = mul(x2, x5);
        x2 = add(x2, x0);
        x0 = g.r(p(4))?;
        x0 = mul(x0, x1);
        x2 = add(x2, x0);
        x0 = g.r(p(0x14))?;
        x0 = mul(x0, x4);
        x3 = mul(x3, x5);
        x3 = add(x3, x0);
        x0 = g.r(p(8))?;
        x0 = mul(x0, x1);
        x3 = add(x3, x0);
        x0 = g.r(p(0x24))?;
        x0 = mul(x0, x5);
        x5 = g.r(p(0x18))?;
        x5 = mul(x5, x4);
        x4 = g.r(p(0xc))?;
        x4 = mul(x4, x1);
        x1 = g.r(c(0x54))?;
        x1 = mul(x1, g.r(p(0))?);
        x0 = add(x0, x5);
        x0 = add(x0, x4);
        x4 = g.r(p(0x30))?;
        x4 = mul(x4, x0);
        x1 = add(x1, x4);
        x4 = g.r(p(0x2c))?;
        x4 = mul(x4, x3);
        x1 = add(x1, x4);
        x4 = mul(x2, g.r(p(0x28))?);
        x1 = add(x1, x4);
    } else {
        x1 = sub(x6, c40);
        x2 = x6;
        x3 = x6;
        x0 = x7;
    }
    x7 = div(x7, x0);
    x0 = x7;
    let s4 = g.r(c(0x44))?;
    let s0 = x0;
    let x5 = mul(x0, x1);
    let mut x4 = sub(x6, x5);
    let s8 = x4;
    let sum = fx(fx(fx(fld(s0)? * fld(x3)?)?.abs() + fx(fld(s0)? * fld(x2)?)?.abs())? + fld(g.r(ONE)?)?)?;
    let st = fx(fx(fld(s4)? - fld(s8)?)? * sum)?;
    let st = fx(fld(s4)? / st)?;
    let s2c = fst(st);
    x1 = x0;
    x7 = s2c;
    x0 = mul(x0, x7);
    x0 = mul(x0, x3);
    x1 = mul(x1, x7);
    x1 = mul(x1, x2);
    x3 = mul(x7, x4);
    x2 = sub(x6, x3);
    if above(x2, x6) && equal(g.r(c(0x40))?, x6) {
        x1 = sub(x6, x1);
        x0 = sub(x6, x0);
        x4 = sub(x6, x2);
        x3 = sub(x6, x7);
        x2 = x4;
    } else {
        x3 = s2c;
    }

    // The clip rows.
    x4 = b(0x5c);
    for k in 0..16 {
        g.w(p(0x78 + 4 * k), 0)?;
    }
    g.w(p(0x78), x4)?;
    x4 = sub(x6, x1);
    x1 = b(0x60);
    g.w(p(0x8c), x1)?;
    x1 = sub(x6, x0);
    x0 = sub(x6, b(0x54));
    g.w(p(0x98), x0)?;
    x0 = sub(x6, b(0x58));
    g.w(p(0x9c), x0)?;
    x0 = g.r(NEAR)?;
    x6 = sub(x6, x3);
    g.w(p(0x80), x4)?;
    g.w(p(0x90), x1)?;
    g.w(p(0xa0), x6)?;
    g.w(p(0xa4), x0)?;
    g.w(p(0xb0), x2)?;
    Ok(Finished { eax: 0, ecx: 0, edx: p(0x78), xmm: [x0, x1, x2, x3, x4, x5, x6, x7] })
}

/// `0x00141590` (ECX `input`, EAX `output`; **Code**
/// `0x00141590..0x001416ba`): the inverse of the 13-word transform at
/// `input` (scale, three axes, position) to `output`, zero when the scale
/// is 0.
fn inverse(g: &mut G<'_>, input: u32, output: u32) -> Result<(), Cause> {
    let i = |off: u32| input.wrapping_add(off);
    let o = |off: u32| output.wrapping_add(off);
    let zero = 0u32;
    if equal(g.r(i(0))?, zero) {
        for k in 0..13 {
            g.w(o(4 * k), 0)?;
        }
        return Ok(());
    }
    let s = g.r(i(0))?;
    let one = g.r(ONE)?;
    let (mut x0, mut x1, mut x3) = (sub(zero, g.r(i(0x28))?), sub(zero, g.r(i(0x2c))?), sub(zero, g.r(i(0x30))?));
    if equal(s, one) {
        g.w(o(0), one)?;
    } else {
        let k = div(one, s);
        x0 = mul(k, x0);
        g.w(o(0), k)?;
        x1 = mul(k, x1);
        x3 = mul(k, x3);
    }
    let v = g.r(i(4))?;
    g.w(o(4), v)?;
    let edx = g.r(i(0x14))?;
    let mut x5 = g.r(o(4))?;
    g.w(o(0x14), edx)?;
    let v = g.r(i(0x24))?;
    g.w(o(0x24), v)?;
    let mut x2 = g.r(i(0x10))?;
    let edx = g.r(i(8))?;
    g.w(o(8), x2)?;
    g.w(o(0x10), edx)?;
    x2 = g.r(i(0x1c))?;
    let eax = g.r(i(0xc))?;
    x5 = mul(x5, x0);
    g.w(o(0x1c), eax)?;
    let mut x4 = g.r(o(0x1c))?;
    x4 = mul(x4, x3);
    x4 = add(x4, x5);
    x5 = g.r(o(0x10))?;
    g.w(o(0xc), x2)?;
    x2 = g.r(i(0x20))?;
    let ecx = g.r(i(0x18))?;
    x5 = mul(x5, x1);
    x4 = add(x4, x5);
    x5 = g.r(o(8))?;
    g.w(o(0x28), x4)?;
    x5 = mul(x5, x0);
    g.w(o(0x20), ecx)?;
    x4 = g.r(o(0x20))?;
    x4 = mul(x4, x3);
    x4 = add(x4, x5);
    x5 = mul(x1, g.r(o(0x14))?);
    x4 = add(x4, x5);
    g.w(o(0x2c), x4)?;
    x4 = g.r(o(0xc))?;
    x4 = mul(x4, x0);
    x0 = g.r(o(0x24))?;
    g.w(o(0x18), x2)?;
    x0 = mul(x0, x3);
    x4 = add(x4, x0);
    x2 = mul(x2, x1);
    x4 = add(x4, x2);
    g.w(o(0x30), x4)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    use engine::view_projection::guest as vp;

    const LOCAL: u32 = 0x0054_f000;
    /// The view end's ESP is `LOCAL + 0x10`; `0x0002fd90`'s on entry is
    /// that less `0x20`.
    const ESP: u32 = LOCAL - 0x10;

    fn f(m: &FlatMemory, at: u32) -> f32 {
        m.f32(at).unwrap()
    }

    fn put(m: &mut FlatMemory, at: u32, v: f32) {
        m.set_f32(at, v).unwrap();
    }

    fn shorts(m: &mut FlatMemory, at: u32, v: [u16; 4]) {
        for (k, w) in v.into_iter().enumerate() {
            m.set_u16(at + 2 * k as u32, w).unwrap();
        }
    }

    fn image() -> FlatMemory {
        let mut m = FlatMemory::new(0x0045_0000, 0x0010_0000);
        put(&mut m, ONE, 1.0);
        put(&mut m, HALF, 0.5);
        put(&mut m, NEAR, -1.0);
        put(&mut m, WINDOW_SCALE, 2.0);
        put(&mut m, SHIFT_X, 0.25);
        put(&mut m, SHIFT_Y, 1.0 / 480.0);
        put(&mut m, MIN_LENGTH, 1e-4);
        put(&mut m, vp::OFFSET_SCALE, 0.5);
        put(&mut m, vp::ANAMORPHIC, 4.0 / 3.0);
        put(&mut m, vp::ANAMORPHIC_TAN, 0.75);
        put(&mut m, CAMERA + vp::CAMERA_FOV, 1.2);
        shorts(&mut m, c(0x30), [0, 0, 480, 640]);
        shorts(&mut m, c(0x38), [0, 0, 480, 640]);
        m
    }

    fn window(m: &mut FlatMemory) -> [u32; 4] {
        update(m, LOCAL, MXCSR).unwrap();
        std::array::from_fn(|k| m.u32(LOCAL + 4 * k as u32).unwrap())
    }

    #[test]
    fn the_float_state_is_checked() {
        let mut m = image();
        assert_eq!(update(&mut m, LOCAL, 0x9fc0), Err("simd"));
        assert!(update(&mut m, LOCAL, 0x1fa0).is_ok(), "sticky flags are allowed");
        assert_eq!(finish(&mut m, LOCAL, ESP, MXCSR, 0x037f), Err("x87"));
    }

    #[test]
    fn sse_nan_rules_are_x86s() {
        let (qa, qb, sb) = (0x7fc0_0001, 0xffc0_0002, 0x7f80_0003);
        assert_eq!(add(qa, qb), qa, "the first NaN operand");
        assert_eq!(mul(1f32.to_bits(), sb), sb | 0x0040_0000, "quietened");
        assert_eq!(sub(f32::INFINITY.to_bits(), f32::INFINITY.to_bits()), INDEFINITE);
        assert_eq!(cvt(-3i32 as u32), (-3f32).to_bits());
    }

    #[test]
    fn the_window_maps_to_the_extents_and_clears_the_lens_byte() {
        let mut m = image();
        m.set_u8(LENS_FLAG, 7).unwrap();
        let r = update(&mut m, LOCAL, MXCSR).unwrap();
        let x1 = 2.0f32 / 480.0;
        let x2 = (480.0f32 / 640.0) * x1;
        assert_eq!(f(&m, LOCAL).to_bits(), ((0.0f32 - 320.0) * x2).to_bits());
        assert_eq!(f(&m, LOCAL + 4).to_bits(), ((640.0f32 - 320.0) * x2).to_bits());
        assert_eq!(f(&m, LOCAL + 8).to_bits(), ((240.0f32 - 480.0) * x1).to_bits());
        assert_eq!(f(&m, LOCAL + 0xc).to_bits(), ((240.0f32 - 0.0) * x1).to_bits());
        assert_eq!(m.u8(LENS_FLAG).unwrap(), 0);
        assert_eq!(r, Updated { eax: (CAMERA & !0xff) | 1, ecx: 0, edx: 480 });
    }

    #[test]
    fn the_settings_word_two_sets_the_lens_byte() {
        let mut m = image();
        m.set_u32(SETTINGS, 0x0050_0000).unwrap();
        m.set_u16(0x0050_0010, 2).unwrap();
        let r = update(&mut m, LOCAL, MXCSR).unwrap();
        assert_eq!(m.u8(LENS_FLAG).unwrap(), 1);
        assert_eq!(r.ecx, 0x0050_0000, "CL ends as the lens bytes, both 0");
        assert_eq!(r.edx, (480 & !0xff) | 1, "DL the sete");
        m.set_u16(0x0050_0010, 3).unwrap();
        update(&mut m, LOCAL, MXCSR).unwrap();
        assert_eq!(m.u8(LENS_FLAG).unwrap(), 0);
    }

    #[test]
    fn the_lens_recentres_and_resizes_the_extents() {
        let mut m = image();
        let e = window(&mut m).map(f32::from_bits);
        m.set_u8(c(0x58), 1).unwrap();
        put(&mut m, c(0x5c), 0.5);
        put(&mut m, c(0x60), -0.5);
        put(&mut m, c(0x64), 0.25);
        let r = update(&mut m, LOCAL, MXCSR).unwrap();
        let (w, h) = (e[1] - e[0], e[3] - e[2]);
        let cx = (0.5f32 + 1.0) * 0.5 * w + e[0];
        let cy = (-0.5f32 + 1.0) * 0.5 * h + e[2];
        let (dx, dy) = (0.25 * w * 0.5f32, 0.25 * h * 0.5f32);
        let got: [f32; 4] = std::array::from_fn(|k| f(&m, LOCAL + 4 * k as u32));
        assert_eq!(got.map(f32::to_bits), [cx - dx, dx + cx, cy - dy, dy + cy].map(f32::to_bits));
        assert_eq!(m.u8(LENS_FLAG).unwrap(), 1);
        assert_eq!(r.ecx & 0xff, 1);
    }

    #[test]
    fn the_shift_moves_the_extents() {
        let mut m = image();
        let e = window(&mut m).map(f32::from_bits);
        m.set_u8(c(0x68), 1).unwrap();
        put(&mut m, c(0x6c), 2.0);
        put(&mut m, c(0x70), 3.0);
        update(&mut m, LOCAL, MXCSR).unwrap();
        let sx = (e[1] - e[0]) * 0.25 * 2.0;
        let sy = (e[3] - e[2]) * (1.0 / 480.0) * 3.0;
        let got: [f32; 4] = std::array::from_fn(|k| f(&m, LOCAL + 4 * k as u32));
        assert_eq!(got.map(f32::to_bits), [e[0] + sx, e[1] + sx, e[2] - sy, e[3] - sy].map(f32::to_bits));
        assert_eq!(m.u8(LENS_FLAG).unwrap(), 1);
    }

    /// A camera at the origin looking down x with z up, and the local's
    /// extents from [`update`].
    fn camera(m: &mut FlatMemory, forward: [f32; 3]) {
        for (k, v) in forward.into_iter().chain([0.0, 0.0, 1.0]).enumerate() {
            put(m, c(0xc + 4 * k as u32), v);
        }
        window(m);
    }

    fn words(m: &FlatMemory, at: u32, n: u32) -> Vec<u32> {
        (0..n).map(|k| m.u32(at + 4 * k).unwrap()).collect()
    }

    #[test]
    fn the_axes_are_normalised_and_inverted() {
        let mut m = image();
        camera(&mut m, [2.0, 0.0, 0.0]);
        for (k, v) in [5.0f32, 6.0, 7.0].into_iter().enumerate() {
            put(&mut m, c(4 * k as u32), v);
        }
        let r = finish(&mut m, LOCAL, ESP, MXCSR, X87_CONTROL).unwrap();
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        // x = (0, −1, 0); y = (−0, 0, 1) from the unnormalised x; z = −forward.
        assert_eq!(words(&m, p(0x34), 13), bits(&[1.0, 0.0, -1.0, 0.0, -0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 5.0, 6.0, 7.0]));
        // The inverse: the axes transposed, the position through them.
        let inv = words(&m, p(0), 13);
        assert_eq!(inv[..10], bits(&[1.0, 0.0, -0.0, -1.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0])[..]);
        assert_eq!(inv[10..], bits(&[6.0, -7.0, 5.0])[..]);
        assert_eq!((r.eax, r.ecx, r.edx), (0, 0, p(0x78)));
    }

    #[test]
    fn short_axes_stay_as_they_are() {
        let mut m = image();
        put(&mut m, MIN_LENGTH, 5.0);
        camera(&mut m, [2.0, 0.0, 0.0]);
        finish(&mut m, LOCAL, ESP, MXCSR, X87_CONTROL).unwrap();
        assert_eq!(f(&m, p(0x3c)), -2.0);
        assert_eq!(f(&m, p(0x4c)), 4.0);
        assert_eq!(f(&m, p(0x50)), -2.0);
    }

    fn projection(m: &FlatMemory) -> [u32; 16] {
        let out = ESP - 0x40;
        view_projection::build(m, Call { camera: CAMERA, extents: LOCAL, output: out }, MXCSR, X87_CONTROL).unwrap().words
    }

    #[test]
    fn the_projection_fills_the_offsets_and_the_clip_rows() {
        let mut m = image();
        camera(&mut m, [1.0, 0.0, 0.0]);
        put(&mut m, c(0x40), -0.5);
        put(&mut m, c(0x44), 1.5);
        m.set_u32(p(0x84), 0xdead_beef).unwrap();
        let b = projection(&m).map(f32::from_bits);
        let r = finish(&mut m, LOCAL, ESP, MXCSR, X87_CONTROL).unwrap();
        assert_eq!(words(&m, p(0x68), 4), b[12..16].iter().map(|v| v.to_bits()).collect::<Vec<_>>());
        assert_eq!(f(&m, p(0xb8)).to_bits(), (b[10] * b[4] * 0.5).to_bits());
        assert_eq!(f(&m, p(0xbc)).to_bits(), (b[11] * b[5] * 0.5).to_bits());
        // `+0x40` not 0: x7 = 1.5 / (1.5 − −0.5), the jump to `[esp+2c]`.
        let rows = words(&m, p(0x78), 16).into_iter().map(f32::from_bits).collect::<Vec<_>>();
        assert_eq!(rows[0].to_bits(), b[10].to_bits());
        assert_eq!(m.u32(p(0x84)).unwrap(), 0, "the rows are cleared first");
        assert_eq!(rows[5].to_bits(), b[11].to_bits());
        assert_eq!(rows[8].to_bits(), (0.0 - b[8]).to_bits());
        assert_eq!(rows[9].to_bits(), (0.0 - b[9]).to_bits());
        assert_eq!(rows[10], -0.75);
        assert_eq!(rows[11], -1.0);
        assert_eq!(rows[14], 0.375);
        assert_eq!(r.xmm[7], 0.75f32.to_bits());
    }

    /// `+0x40` 0: the plane `(−1, 0, 0, d)` through the inverse, `+0x44` 1.
    fn plane(d: f32) -> FlatMemory {
        let mut m = image();
        camera(&mut m, [1.0, 0.0, 0.0]);
        put(&mut m, c(0x44), 1.0);
        put(&mut m, c(0x48), -1.0);
        put(&mut m, c(0x54), d);
        m
    }

    #[test]
    fn a_plane_in_front_flips_the_depth_row() {
        let mut m = plane(1.0);
        let r = finish(&mut m, LOCAL, ESP, MXCSR, X87_CONTROL).unwrap();
        // x7 = 1 / (1 − −1); x2 = 0 − x7·−1 above 0: the flip.
        assert_eq!(f(&m, p(0xa0)), 0.5);
        assert_eq!(f(&m, p(0xb0)), -0.5);
        assert_eq!(r.xmm[7], 0.5f32.to_bits());
        assert_eq!(r.xmm[3], (-0.5f32).to_bits());
    }

    #[test]
    fn a_plane_behind_keeps_the_depth_row() {
        let mut m = plane(-0.5);
        finish(&mut m, LOCAL, ESP, MXCSR, X87_CONTROL).unwrap();
        // x7 = 1 / (1 − 0.5); x2 = 0 − x7·0.5.
        assert_eq!(f(&m, p(0xa0)), -2.0);
        assert_eq!(f(&m, p(0xb0)), -1.0);
        assert_eq!(f(&m, p(0xa4)), -1.0);
    }

    #[test]
    fn an_x87_nan_is_refused() {
        let mut m = plane(1.0);
        m.set_u32(c(0x44), 0x7fc0_0000).unwrap();
        assert_eq!(finish(&mut m, LOCAL, ESP, MXCSR, X87_CONTROL), Err("x87"));
    }

    fn transform(m: &mut FlatMemory, at: u32, w: [f32; 13]) {
        for (k, v) in w.into_iter().enumerate() {
            put(m, at + 4 * k as u32, v);
        }
    }

    const IN: u32 = 0x0050_0000;
    const OUT: u32 = 0x0050_0100;

    fn inverted(m: &mut FlatMemory) -> Vec<f32> {
        inverse(&mut G(m), IN, OUT).unwrap();
        (0..13).map(|k| f(m, OUT + 4 * k)).collect()
    }

    #[test]
    fn the_inverse_of_a_zero_scale_is_zero() {
        let mut m = image();
        transform(&mut m, OUT, [9.0; 13]);
        transform(&mut m, IN, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 1.0, 2.0, 3.0]);
        assert_eq!(inverted(&mut m), vec![0.0; 13]);
    }

    #[test]
    fn the_inverse_divides_by_a_scale_other_than_one() {
        let mut m = image();
        let a = [0.0f32, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let t = [1.0f32, 2.0, 3.0];
        let mut w = [0f32; 13];
        w[0] = 2.0;
        w[1..10].copy_from_slice(&a);
        w[10..].copy_from_slice(&t);
        transform(&mut m, IN, w);
        let got = inverted(&mut m);
        let k = 1.0f32 / 2.0;
        let tp = [k * (0.0 - t[0]), k * (0.0 - t[1]), k * (0.0 - t[2])];
        let mut want = vec![k];
        for row in 0..3 {
            for axis in 0..3 {
                want.push(a[3 * axis + row]);
            }
        }
        // `+0x28..0x30`: each input axis dotted with the scaled position,
        // z then x then y.
        for axis in 0..3 {
            let v = &a[3 * axis..3 * axis + 3];
            want.push((v[2] * tp[2] + v[0] * tp[0]) + v[1] * tp[1]);
        }
        assert_eq!(got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), want.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
        w[0] = 1.0;
        transform(&mut m, IN, w);
        assert_eq!(inverted(&mut m)[0], 1.0);
    }
}
