//! The colour pass of `render_screen_effects` (`0x00013870`, its call of
//! `0x000214f0`; docs/graphics/native-post-hud.md, stage 5): the numbers
//! it draws with, as pure functions of guest memory, for the native
//! renderer to draw the pass in the game's place.
//!
//! - [`arguments`]: `0x000138b3`..`0x000139b5`, the caller's half. The
//!   two curves `0x000137a0` reads at the view time (`[0x004858a0]`,
//!   through the record at `[0x0050934c]`), the screen record's fade
//!   (`[[0x00485890] + 0x10]`), the clamp and the blend toward the targets
//!   at `0x004b9fd0`: the seven words pushed for `0x000214f0`, or `None`
//!   when the pass is not drawn (the first curve not above 0).
//! - [`parameters`]: `0x000214f0`..`0x000216fb`, the callee's head: the
//!   override record (`[0x005093e4]`, when `[0x00510c50]` is set and its
//!   byte `+5`), which a pending reset clears instead of applying, then
//!   the clamps, and whether anything is drawn.
//! - [`pack_colour`]: `0x00131e50`, the four floats to an ARGB word (x87,
//!   `fistp` under the caller's control word, no masking), and
//!   [`Parameters::colours`] the two combiner colours the pass writes
//!   (`0x0002182e` into `0x00484f9c`, `0x000219da` into `0x00484f90`).
//!
//! The arithmetic is the original's instruction for instruction: SSE
//! through the `simd` crate under the caller's MXCSR, x87 through
//! `x87::abi` under its control word, so the words are bit-equal (NaNs,
//! rounding modes and denormals included). Image constants are read from
//! guest memory, never copied. `engine/tests/colour_pass_differential.rs`
//! checks all three against the lifted original.

use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;

/// Guest globals the pass reads.
pub mod global {
    /// The view time, an f64 (`0x000137a6`).
    pub const TIME: u32 = 0x0048_58a0;
    /// Pointer to the curves record (`0x000137a1`): a byte selecting the
    /// set, then per set three curves of two floats (keys, then the two
    /// outputs' ends).
    pub const CURVES: u32 = 0x0050_934c;
    /// Pointer to the view's screen record (`+0x10`: its fade).
    pub const SCREEN_RECORD: u32 = 0x0048_5890;
    /// The view kind; 6 sets the pass's last argument.
    pub const VIEW_KIND: u32 = 0x0048_5898;
    /// The blend weight, then the two targets it blends toward.
    pub const BLEND: u32 = 0x004b_9fd0;
    /// Pointer whose byte `+5` turns the override record on.
    pub const OVERRIDE_GATE: u32 = 0x0051_0c50;
    /// Pointer to the override record.
    pub const OVERRIDE: u32 = 0x0050_93e4;
}

/// Image constants (addresses; read from the image).
pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    /// The smallest key span the curve interpolates over.
    pub const MIN_SPAN: u32 = 0x0045_dbdc;
    /// The second argument without a screen record.
    pub const SECOND: u32 = 0x0045_dc84;
    /// The third argument's scale.
    pub const THIRD_SCALE: u32 = 0x0045_dbbc;
    /// The callee's local `y` before the override.
    pub const Y: u32 = 0x0045_dd30;
    /// An override field holding this value is unset.
    pub const UNSET: u32 = 0x0045_dbcc;
    /// The first lane of the first combiner colour.
    pub const COLOUR_A_LANE: u32 = 0x0045_dcc8;
    /// `0x00131e50`'s scale (to 0..255).
    pub const PACK_SCALE: u32 = 0x0045_dc38;
}

/// The words pushed for `0x000214f0` (`0x00013991`..`0x000139b0`), first
/// argument first.
pub const ARG0: u32 = 0x407f_7cee;
pub const ARG3: u32 = 0x3e80_0000;

/// The caller's arguments: `[0x407f7cee, second, third, 0.25, 0, 1,
/// view kind word with its low byte "kind is 6"]`.
pub type Arguments = [u32; 7];

/// `0x000137a0`: the two curves at the view time, `(first, second)`.
fn curves(mem: &dyn GuestMemory, sse: &mut Sse, x87: &mut X87) -> Result<(u32, u32), Error> {
    // fld qword, fstp dword: the time rounded to f32 under the control word.
    fld_f64(x87, u64::from(mem.u32(global::TIME)?) | u64::from(mem.u32(global::TIME + 4)?) << 32)?;
    let t = x87.fstp_m32()?;
    let p = mem.u32(global::CURVES)?;
    let (keys, c, a) = if mem.u8(p)? != 0 {
        (p.wrapping_add(0x1c), p.wrapping_add(0x24), p.wrapping_add(0x2c))
    } else {
        (p.wrapping_add(4), p.wrapping_add(0xc), p.wrapping_add(0x14))
    };
    let f = |a: u32| mem.u32(a);
    let e0 = f(keys)?;
    if !sse.below(e0, t)? {
        return Ok((f(c)?, f(a)?));
    }
    let e1 = f(keys.wrapping_add(4))?;
    if sse.above(t, e1)? {
        return Ok((f(c.wrapping_add(4))?, f(a.wrapping_add(4))?));
    }
    let span = sse.sub(e1, e0)?;
    if !sse.above(span, mem.u32(constant::MIN_SPAN)?)? {
        return Ok((f(c.wrapping_add(4))?, f(a.wrapping_add(4))?));
    }
    let x = sse.sub(t, e0)?;
    let x = sse.div(x, span)?;
    let b = sse.sub(f(a.wrapping_add(4))?, f(a)?)?;
    let first = sse.sub(f(c.wrapping_add(4))?, f(c)?)?;
    let b = sse.mul(b, x)?;
    let b = sse.add(b, f(a)?)?;
    let first = sse.mul(first, x)?;
    let first = sse.add(first, f(c)?)?;
    Ok((first, b))
}

/// `fld qword`.
pub(crate) fn fld_f64(x87: &mut X87, bits: u64) -> Result<(), Error> {
    X87::ok(x87::abi::ld_f64(&mut x87.0, bits))
}

/// `fistp dword`, rounding under the control word.
pub(crate) fn fistp(x87: &mut X87) -> Result<u32, Error> {
    let (code, store) = x87::abi::st_i32(&x87.0, 0);
    X87::ok(code)?;
    x87::abi::store_commit(&mut x87.0, store.status, true);
    Ok(store.value as u32)
}

/// `0x000138b3`..`0x000139b5` with the colour byte set: the arguments
/// `render_screen_effects` passes to the colour pass, or `None` when it
/// skips it. `mxcsr` and `x87_control` are the call's.
pub fn arguments(mem: &dyn GuestMemory, mxcsr: u32, x87_control: u16) -> Result<Option<Arguments>, Error> {
    let mut sse = Sse { mxcsr };
    let mut x87 = X87::new(x87_control);
    let zero = 0;
    let second0 = mem.u32(constant::SECOND)?;
    let (first, curve_b) = curves(mem, &mut sse, &mut x87)?;
    let one = mem.u32(constant::ONE)?;
    let screen = mem.u32(global::SCREEN_RECORD)?;
    let mut second = second0;
    if screen != 0 {
        let fade = sse.sub(one, mem.u32(screen.wrapping_add(0x10))?)?;
        let s = sse.sub(one, fade)?;
        second = sse.mul(s, second0)?;
    }
    if !sse.above(first, zero)? {
        return Ok(None);
    }
    let third = sse.add(curve_b, one)?;
    let third = sse.mul(third, first)?;
    let third = sse.mul(third, mem.u32(constant::THIRD_SCALE)?)?;
    let mut third = sse.clamp(third, zero, one)?;
    let w = mem.u32(global::BLEND)?;
    if sse.above(w, zero)? {
        let s = sse.sub(mem.u32(global::BLEND + 4)?, second)?;
        let s = sse.mul(s, w)?;
        second = sse.add(s, second)?;
        let t = sse.sub(mem.u32(global::BLEND + 8)?, third)?;
        let t = sse.mul(t, w)?;
        third = sse.add(t, third)?;
    }
    let kind = mem.u32(global::VIEW_KIND)?;
    let kind = (kind & !0xff) | u32::from(kind == 6);
    Ok(Some([ARG0, second, third, ARG3, 0, 1, kind]))
}

/// What `0x000214f0` draws with after its head: the seven arguments as
/// the override and the clamps leave them (the two bytes keep their
/// words' upper bytes), and its locals `x` and `y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parameters {
    pub args: Arguments,
    pub x: u32,
    pub y: u32,
    /// Whether it draws (the third argument or `x` above 0).
    pub draws: bool,
    /// The override record a pending reset cleared (its stores are
    /// [`Parameters::write_reset`]'s).
    pub reset: Option<u32>,
    /// The `UNSET` value the reset stores.
    pub unset: u32,
}

impl Parameters {
    /// The reset's stores (`0x00021533`..`0x0002156a`), as the game makes
    /// them.
    pub fn write_reset(&self, mem: &mut dyn GuestMemory) -> Result<(), Error> {
        let Some(r) = self.reset else { return Ok(()) };
        mem.set_u8(r, 0)?;
        mem.set_u8(r.wrapping_add(1), 0)?;
        for off in [4, 8, 0xc, 0x10, 0x14] {
            mem.set_u32(r.wrapping_add(off), self.unset)?;
        }
        mem.set_u8(r.wrapping_add(0x18), 1)?;
        mem.set_u8(r.wrapping_add(0x19), 0)?;
        mem.set_u32(r.wrapping_add(0x1c), self.unset)?;
        mem.set_u32(r.wrapping_add(0x20), self.unset)?;
        mem.set_u8(r.wrapping_add(0x1a), 0)?;
        Ok(())
    }

    /// The two combiner colours (`0x00484f9c` for the first pass,
    /// `0x00484f90` for the second): `pack(COLOUR_A_LANE, second x3)` and
    /// `pack(x, third x3)`.
    pub fn colours(&self, mem: &dyn GuestMemory, x87_control: u16) -> Result<[u32; 2], Error> {
        let scale = mem.u32(constant::PACK_SCALE)?;
        let a = mem.u32(constant::COLOUR_A_LANE)?;
        let (second, third) = (self.args[1], self.args[2]);
        Ok([
            pack_colour([a, second, second, second], scale, x87_control)?,
            pack_colour([self.x, third, third, third], scale, x87_control)?,
        ])
    }
}

/// `0x000214f0`..`0x000216fb` (or its early return `0x00021a73`) for
/// `args`. Reads guest memory only; a pending reset is reported, not
/// stored.
pub fn parameters(mem: &dyn GuestMemory, args: Arguments, mxcsr: u32) -> Result<Parameters, Error> {
    let mut sse = Sse { mxcsr };
    let one = mem.u32(constant::ONE)?;
    let mut x = one;
    let mut y = mem.u32(constant::Y)?;
    let mut a = args;
    let unset = mem.u32(constant::UNSET)?;
    let mut reset = None;
    let gate = mem.u32(global::OVERRIDE_GATE)?;
    if gate != 0 && mem.u8(gate.wrapping_add(5))? != 0 {
        let r = mem.u32(global::OVERRIDE)?;
        if mem.u8(r.wrapping_add(1))? != 0 {
            reset = Some(r);
        } else if mem.u8(r)? != 0 {
            for (i, off) in [4u32, 8, 0xc, 0x10, 0x14].into_iter().enumerate() {
                let v = mem.u32(r.wrapping_add(off))?;
                if !sse.equal(v, unset)? {
                    a[i] = v;
                }
            }
            let v = mem.u32(r.wrapping_add(0x1c))?;
            let (b5, b6) = (mem.u8(r.wrapping_add(0x18))?, mem.u8(r.wrapping_add(0x19))?);
            if !sse.equal(v, unset)? {
                x = v;
            }
            a[5] = (a[5] & !0xff) | u32::from(b5);
            a[6] = (a[6] & !0xff) | u32::from(b6);
            let v = mem.u32(r.wrapping_add(0x20))?;
            if !sse.equal(v, unset)? {
                y = v;
            }
        }
    }
    let zero = 0;
    if !sse.above(a[0], zero)? {
        a[0] = zero;
    }
    for v in &mut a[1..5] {
        *v = sse.clamp(*v, zero, one)?;
    }
    x = sse.clamp(x, zero, one)?;
    y = sse.clamp(y, zero, one)?;
    let draws = sse.above(a[2], zero)? || sse.above(x, zero)?;
    Ok(Parameters {
        args: a,
        x,
        y,
        draws,
        reset,
        unset,
    })
}

/// `0x00131e50`: `v` scaled by `scale` (x87, under `x87_control`), each
/// lane `fistp`'d, packed `v[0] << 24 | v[1] << 16 | v[2] << 8 | v[3]`
/// with no masking (an out-of-range lane's bits spill, as the game's do).
pub fn pack_colour(v: [u32; 4], scale: u32, x87_control: u16) -> Result<u32, Error> {
    let mut x87 = X87::new(x87_control);
    for lane in v {
        x87.fld(lane)?;
    }
    x87.fld(scale)?;
    let mul = x87::abi::arith_op::MUL;
    x87.arith_reg(mul, 4, 0, false)?;
    x87.arith_reg(mul, 3, 0, false)?;
    x87.arith_reg(mul, 2, 0, false)?;
    x87.arith_reg(mul, 1, 0, true)?;
    // ST0 is lane 3 now, popped first.
    let l3 = fistp(&mut x87)?;
    let l2 = fistp(&mut x87)?;
    let l1 = fistp(&mut x87)?;
    let l0 = fistp(&mut x87)?;
    Ok(l3 | (l2 << 8) | (l1 << 16) | (l0 << 24))
}
