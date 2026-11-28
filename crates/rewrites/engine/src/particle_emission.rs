//! One particle's emission in Rust: `0x00224550`, the per-particle callback
//! of the particle draw (`0x002243c0`), from its entry to its call of the
//! emitter `0x0023ed30` (docs/graphics/retire-d3d.md, "R2's emission
//! arm"). [`predict`] makes the eight words it pushes, the three vectors
//! they and EBX point at, and the guest writes it makes on the way.
//!
//! **Code** (`h2 disasm`): `0x00224550(particle, state, position, axis,
//! size, tint, draw)` (`ret 0x1c`). `state` is the particle's input block:
//! sixteen cached input values, `+0x44` the mask of those cached, `+0x48`,
//! `+0x4c` and `+0x50` the records the inputs read. `draw` is the draw's
//! context: `+0` the particle definition, `+4` the bitmap, `+8` the
//! sequence, `+0xc` a scale.
//!
//! 1. The inputs the definition reads (`+0x98 & 0xf80f`) and the block has
//!    not cached are computed in bit order (`0x00173ba0` through
//!    `0x00248df0`) and stored; the mask takes them all.
//! 2. Rotation `+0x4c` (times `0x0043f17c`), colour `+0x1c` (`0x00246d80`)
//!    and alpha `+0x2c` (times `tint[0]`) are evaluated (`0x00246cd0`, the
//!    tag function [`crate::function_eval`] then the property's range and
//!    modifier). Definition flag `0x180` tints the colour by the
//!    particle's packed `+0x3c`. The colour is `tint[1..4]` times that,
//!    each lane and the alpha clamped to 0..1, packed (`0x00131e50`).
//! 3. Flag `0x10000`: the sprite window and corners are the definition's
//!    (`+0xac`, `+0x9c`). Otherwise from the bitmap: with a sequence bitmap
//!    (type 3) flag `0x10` picks the sequence from the particle's `+0x16`
//!    (stored at `draw + 8`), property `+0x5c` the frame and the next, and
//!    their blend (unless flag `0x20`); the corners follow the frames'
//!    aspect.
//! 4. Flags `2` and `4` with the particle's `+0x14` and `+0x16` bit 0 flip
//!    the window in u and v. Flag `0x2000` adds `0x004123ec` to the
//!    rotation. Size `+0x3c` is scaled by `size`.
//!
//! The emitter's words: `position`, `axis`, size, rotation, the packed
//! colour, the frame blend, then the two frames' windows (ESP `-0x30` and
//! `-0x10` from the entry's), EBX the corners (`-0x40`). Every operation is
//! the original's: SSE in its operand order under the guest's MXCSR, the
//! x87 parts under its control word, every constant read from the image.

use std::collections::BTreeMap;

use crate::colour_pass::pack_colour;
use crate::function_eval::{self, constant::{BYTE, ONE, ZERO}};
use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::{GuestMemory, MemFault};
use crate::render_loop::opaque_animation;

/// The callback.
pub const ENTRY: u32 = 0x0022_4550;
/// The particle's inputs: what the callback caches (`+0x98` masked).
pub const CALLBACK_INPUTS: u32 = 0xf80f;
/// The emitter it calls once, last.
pub const EMIT: u32 = 0x0023_ed30;
/// The emitter's return into the callback.
pub const EMIT_RETURN: u32 = 0x0022_4cd7;
/// The callback's entry ESP less the emitter's entry ESP.
pub const EMIT_DEPTH: u32 = 0x94;
/// What the callback pops: its return and seven arguments.
pub const POP: u32 = 0x20;

/// Constants the callback reads (f32).
pub mod constant {
    /// The rotation's scale.
    pub const ANGLE: u32 = 0x0043_f17c;
    /// 0.5, the corners without a bitmap.
    pub const HALF: u32 = 0x0045_dbbc;
    /// The packed tint's and the corners' scale.
    pub const TINT: u32 = 0x0045_dc0c;
    /// The corners' offset.
    pub const CORNER: u32 = 0x0045_dbcc;
    /// Flag `0x2000`'s rotation offset.
    pub const SPIN: u32 = 0x0041_23ec;
    /// A 16-bit input word as a value.
    pub const WORD: u32 = 0x0045_dc50;
    /// The colour packer's scale.
    pub const PACK: u32 = 0x0045_dc38;
    /// `0x00248df0` case 8's clock (a pointer).
    pub const CLOCK: u32 = 0x0051_0c54;
    /// `0x00248df0` cases 9 and 10's table (a pointer).
    pub const RECORDS: u32 = 0x004e_a93c;
    /// The view: position, forward and up (`0x00248df0` cases 12, 13).
    pub const CAMERA: u32 = 0x0048_5618;
    pub const FORWARD: u32 = 0x0048_5624;
    pub const UP: u32 = 0x0048_5630;
}

/// The callback's call to the emitter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emission {
    /// The eight words as the emitter reads them (`[esp+4]` up).
    pub words: [u32; 8],
    /// EBX: the corners' address.
    pub ebx: u32,
    /// What `words[6]`, `words[7]` and EBX point at: the first frame's
    /// window, the second's, the corners.
    pub window: [u32; 4],
    pub next_window: [u32; 4],
    pub corners: [u32; 4],
}

/// The model's answer: the emission and every guest byte it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prediction {
    pub emission: Emission,
    pub writes: BTreeMap<u32, u8>,
}

/// The counter name of a refusal (`emission_refused_<name>`).
pub fn refusal(e: Error) -> &'static str {
    match e {
        Error::Memory(_) => "fault",
        Error::Simd => "simd",
        Error::X87(_) => "x87",
        Error::Divide => "divide",
        Error::Unbounded => "unbounded",
        Error::Data => "data",
        Error::BoundTexture => "bound_texture",
    }
}

/// Guest memory under the model's writes; the guest is never written.
pub(crate) struct Overlay<'a> {
    pub(crate) base: &'a dyn GuestMemory,
    pub(crate) writes: BTreeMap<u32, u8>,
}

impl GuestMemory for Overlay<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.base.read(addr, buf)?;
        for (i, b) in buf.iter_mut().enumerate() {
            if let Some(&w) = self.writes.get(&addr.wrapping_add(i as u32)) {
                *b = w;
            }
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        // The guest's store faults where its read would.
        let mut probe = vec![0; data.len()];
        self.base.read(addr, &mut probe)?;
        for (i, &b) in data.iter().enumerate() {
            self.writes.insert(addr.wrapping_add(i as u32), b);
        }
        Ok(())
    }
}

/// The callback's evaluators over an [`Overlay`]: the walk
/// (`crate::particle_walk`) shares them.
pub(crate) struct Model<'a> {
    pub(crate) m: Overlay<'a>,
    pub(crate) s: Sse,
    fcw: u16,
    one: u32,
}

/// `[block + 4 * i]`, `i` sign-extended as the guest's `movsx`.
fn slot(block: u32, i: i16) -> u32 {
    block.wrapping_add((i as i32 as u32).wrapping_mul(4))
}

impl<'a> Model<'a> {
    /// Over `m`, writing nothing to it, under `mxcsr` and `fcw`.
    pub(crate) fn new(m: &'a dyn GuestMemory, mxcsr: u32, fcw: u16) -> Result<Self, Error> {
        let one = m.u32(ONE)?;
        Ok(Model { m: Overlay { base: m, writes: BTreeMap::new() }, s: Sse { mxcsr }, fcw, one })
    }

    pub(crate) fn u32(&self, a: u32) -> Result<u32, Error> {
        Ok(self.m.u32(a)?)
    }
    pub(crate) fn u16(&self, a: u32) -> Result<u16, Error> {
        Ok(self.m.u16(a)?)
    }
    fn u8(&self, a: u32) -> Result<u8, Error> {
        Ok(self.m.u8(a)?)
    }
    fn add(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        self.s.add(a, b)
    }
    fn sub(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        self.s.sub(a, b)
    }
    pub(crate) fn mul(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        self.s.mul(a, b)
    }
    fn mul_at(&mut self, a: u32, at: u32) -> Result<u32, Error> {
        let b = self.u32(at)?;
        self.s.mul(a, b)
    }
    fn cvtt(&mut self, v: u32) -> Result<i32, Error> {
        Ok(simd::cvttss2si(&mut self.s.mxcsr, v).map_err(|_| Error::Simd)? as i32)
    }

    /// `fld dword; fstp dword`: a signalling NaN comes back quiet.
    fn through_x87(&self, v: u32) -> Result<u32, Error> {
        let mut x = X87::new(self.fcw);
        x.fld(v)?;
        x.fstp_m32()
    }

    /// `v - (float)(int)v`, plus one when that is below zero: the
    /// `0x00248df0` cases' wrap (`0x002490f4`).
    fn wrap(&mut self, v: u32) -> Result<u32, Error> {
        let t = self.cvtt(v)?;
        let t = self.s.int(t)?;
        let f = self.sub(v, t)?;
        if self.s.above(0, f)? {
            self.add(f, self.one)
        } else {
            Ok(f)
        }
    }

    /// `0x00016ae0`: the float's floor as an integer.
    fn floor(&mut self, v: u32) -> Result<i32, Error> {
        let t = self.cvtt(v)?;
        let back = self.s.int(t)?;
        let zero = self.u32(ZERO)?;
        let ne = simd::cmpss(&mut self.s.mxcsr, back as u128, v, 4).map_err(|_| Error::Simd)?;
        let lt = simd::cmpss(&mut self.s.mxcsr, v as u128, zero, 1).map_err(|_| Error::Simd)?;
        Ok(t.wrapping_sub(i32::from((ne & lt) as u32 & 1 != 0)))
    }

    /// `0x0004efb0`: scales `v` to unit length in x87, unless its squared
    /// length is zero.
    fn normalize(&mut self, mut v: [u32; 3]) -> Result<[u32; 3], Error> {
        let xx = self.mul(v[0], v[0])?;
        let yy = self.mul(v[1], v[1])?;
        let sum = self.add(xx, yy)?;
        let zz = self.mul(v[2], v[2])?;
        let sum = self.add(sum, zz)?;
        let zero = self.u32(ZERO)?;
        if self.s.equal(sum, zero)? {
            return Ok(v);
        }
        let mut x = X87::new(self.fcw);
        x.fld(sum)?;
        x.unary(x87::abi::unary_op::SQRT)?;
        x.arith_m(x87::abi::arith_op::DIVR, self.one)?;
        for c in &mut v[..2] {
            x.fld_st(0)?;
            x.fmul_m(*c)?;
            *c = x.fstp_m32()?;
        }
        x.fmul_m(v[2])?;
        v[2] = x.fstp_m32()?;
        Ok(v)
    }

    /// The unit vector from the view to the particle record `p`.
    fn direction(&mut self, p: u32) -> Result<[u32; 3], Error> {
        let mut d = [0; 3];
        for (i, c) in d.iter_mut().enumerate() {
            let at = 4 * i as u32;
            let a = self.u32(p + 0x1c + at)?;
            let b = self.u32(constant::CAMERA + at)?;
            *c = self.sub(a, b)?;
        }
        self.normalize(d)
    }

    /// `0x00248df0`: input `i` of a particle whose records are `s`
    /// (`state + 0x48`), `l` (`state + 0x4c`) and `p` (`state + 0x50`).
    /// Every case is modelled: the callback's mask `0xf80f` reaches the
    /// particle's, the walk's `0x107f0` the system's and location's.
    pub(crate) fn input(&mut self, i: u32, s: u32, l: u32, p: u32) -> Result<u32, Error> {
        let v = match i {
            0..=3 | 11 | 14 | 15 if p == 0 => 0,
            4..=6 if s == 0 => 0,
            7 | 16 if l == 0 => 0,
            4 => self.u32(s.wrapping_add(4))?,
            5 | 6 => self.u32(s.wrapping_add(0x14 + 4 * i))?,
            7 => self.u32(l.wrapping_add(0x1c))?,
            // A clock: its `+8` ticks times its `+4` seconds a tick.
            8 => {
                let clock = self.u32(constant::CLOCK)?;
                let ticks = self.s.int(self.u32(clock.wrapping_add(8))? as i32)?;
                self.mul_at(ticks, clock.wrapping_add(4))?
            }
            // The system's `+0x18` names a record of `[0x004ea93c]`
            // (stride 0x190): its `+0x6c` and `+0x70`. No null check.
            9 | 10 => {
                let k = self.u32(s.wrapping_add(0x18))?;
                if k == u32::MAX {
                    0
                } else {
                    let table = self.u32(constant::RECORDS)?;
                    let base = self.u32(table + 0x44)?;
                    let at = base.wrapping_add((k & 0xffff).wrapping_mul(0x190));
                    self.u32(at.wrapping_add(if i == 9 { 0x6c } else { 0x70 }))?
                }
            }
            16 => {
                let b = self.s.int(i32::from(self.u8(l.wrapping_add(3))?))?;
                self.mul_at(b, BYTE)?
            }
            0 => self.u32(p + 8)?,
            1 => self.u32(p + 0x10)?,
            2 | 3 | 14 | 15 => {
                let at = if i < 4 { p + 0x10 + 2 * i } else { (p + 2 * i).wrapping_sub(4) };
                let w = self.s.int(i32::from(self.u16(at)?))?;
                self.mul_at(w, constant::WORD)?
            }
            11 => {
                let v = self.u32(p + 0x38)?;
                self.wrap(v)?
            }
            12 | 13 if p == 0 || l == 0 => 0,
            12 => {
                let d = self.direction(p)?;
                let a = self.mul_at(d[2], constant::FORWARD + 8)?;
                let b = self.mul_at(d[1], constant::FORWARD + 4)?;
                let v = self.add(a, b)?;
                let c = self.u32(constant::FORWARD)?;
                let c = self.mul(c, d[0])?;
                let v = self.add(v, c)?;
                let v = self.mul_at(v, p + 0x38)?;
                self.wrap(v)?
            }
            13 => {
                let f = [0, 4, 8].map(|o| self.m.u32(constant::FORWARD + o));
                let u = [0, 4, 8].map(|o| self.m.u32(constant::UP + o));
                let (f0, f1, f2) = (f[0]?, f[1]?, f[2]?);
                let (u0, u1, u2) = (u[0]?, u[1]?, u[2]?);
                let a = self.mul(u2, f1)?;
                let b = self.mul(u1, f2)?;
                let c0 = self.sub(b, a)?;
                let a = self.mul(u2, f0)?;
                let b = self.mul(f2, u0)?;
                let c = self.mul(f1, u0)?;
                let e = self.mul(u1, f0)?;
                let c1 = self.sub(a, b)?;
                let c2 = self.sub(c, e)?;
                let d = self.direction(p)?;
                let a = self.mul(c2, d[2])?;
                let b = self.mul(c1, d[1])?;
                let v = self.add(a, b)?;
                let c = self.mul(c0, d[0])?;
                let v = self.add(v, c)?;
                let v = self.mul_at(v, p + 0x38)?;
                self.wrap(v)?
            }
            _ => return Err(Error::Data),
        };
        Ok(v)
    }

    /// A property's modifier (`+6` the input, `+4` the operation: 1 adds,
    /// 2 multiplies).
    fn modify(&mut self, state: u32, property: u32, v: u32) -> Result<u32, Error> {
        let i = self.u16(property + 6)? as i16;
        if i < 0 {
            return Ok(v);
        }
        match self.u16(property + 4)? {
            1 => {
                let a = self.u32(slot(state, i))?;
                self.add(a, v)
            }
            2 => {
                let a = self.u32(slot(state, i))?;
                self.mul(a, v)
            }
            _ => Ok(v),
        }
    }

    /// The inputs of `wanted` the block at `state` has not cached, made by
    /// `0x00173ba0` in bit order from the block's records, then the mask
    /// takes them all.
    pub(crate) fn cache(&mut self, state: u32, wanted: u32) -> Result<(), Error> {
        let cached = self.u32(state + 0x44)?;
        let (s, l, p) = (self.u32(state + 0x48)?, self.u32(state + 0x4c)?, self.u32(state + 0x50)?);
        let mut bits = wanted & !cached;
        while bits != 0 {
            let i = bits.trailing_zeros();
            bits &= !(1 << i);
            let v = self.input(i, s, l, p)?;
            self.m.set_u32(state + 4 * i, v)?;
        }
        self.m.set_u32(state + 0x44, cached | wanted)?;
        Ok(())
    }

    /// `0x00246cd0`: a scalar property of the inputs at `state`.
    pub(crate) fn property(&mut self, state: u32, property: u32) -> Result<u32, Error> {
        let range = self.u32(slot(state, self.u16(property + 2)? as i16))?;
        let range = self.through_x87(range)?;
        let input = self.u32(slot(state, self.u16(property)? as i16))?;
        let input = self.through_x87(input)?;
        let reference = property + 8;
        let mut v = function_eval::evaluate(&self.m, &mut self.s, self.fcw, reference, input, range)?;
        let blob = self.u32(reference + 4)?;
        if self.u8(blob.wrapping_add(1))? & 0xf0 == 0 {
            v = self.s.clamp(v, 0, self.one)?;
            let lo = self.u32(blob.wrapping_add(4))?;
            let hi = self.u32(blob.wrapping_add(8))?;
            let span = self.sub(hi, lo)?;
            let v2 = self.mul(span, v)?;
            v = self.add(v2, lo)?;
        }
        self.modify(state, property, v)
    }

    /// `0x00246d80`: a colour property, as three floats.
    pub(crate) fn colour(&mut self, state: u32, property: u32) -> Result<[u32; 3], Error> {
        let blob = self.u32(property + 0xc)?;
        let range = if self.u8(blob.wrapping_add(1))? & 1 != 0 {
            self.u32(slot(state, self.u16(property + 2)? as i16))?
        } else {
            0
        };
        let input = if self.u8(blob)? == 1 {
            0
        } else {
            self.u32(slot(state, self.u16(property)? as i16))?
        };
        let reference = property + 8;
        let v = function_eval::evaluate(&self.m, &mut self.s, self.fcw, reference, input, range)?;
        let v = self.modify(state, property, v)?;
        opaque_animation::rgb(&self.m, reference, v, self.fcw, self.s.mxcsr)
    }

    /// `0x00131bb0`: a packed colour's red, green and blue as floats.
    fn unpack(&self, c: u32) -> Result<[u32; 3], Error> {
        let scale = self.u32(BYTE)?;
        let mut x = X87::new(self.fcw);
        let mut out = [0; 3];
        for (lane, shift) in out.iter_mut().zip([16, 8, 0]) {
            x.fild(((c >> shift) & 0xff) as i32)?;
            x.fmul_m(scale)?;
            *lane = x.fstp_m32()?;
        }
        Ok(out)
    }

    /// The flips: bit 0 in u (flag 2 and the particle's `+0x14` bit 0),
    /// bit 1 in v (flag 4 and `+0x16` bit 0).
    fn flips(&self, flags: u32, particle: u32) -> Result<u32, Error> {
        let mut f = 0;
        if flags & 2 != 0 && self.u8(particle + 0x14)? & 1 != 0 {
            f |= 1;
        }
        if flags & 4 != 0 && self.u8(particle + 0x16)? & 1 != 0 {
            f |= 2;
        }
        Ok(f)
    }

    /// A window `(u0, u1, v0, v1)` as `(u, v, width, height)`, flipped.
    fn window(&mut self, w: [u32; 4], flips: u32, width: u32, height: u32) -> Result<[u32; 4], Error> {
        let h = if flips & 2 != 0 { self.sub(w[2], w[3])? } else { height };
        let wd = if flips & 1 != 0 { self.sub(w[0], w[1])? } else { width };
        let v = if flips & 2 != 0 { w[3] } else { w[2] };
        let u = if flips & 1 != 0 { w[1] } else { w[0] };
        Ok([u, v, wd, h])
    }
}

/// [`predict`]'s seven arguments.
fn args(m: &dyn GuestMemory, esp: u32) -> Result<[u32; 7], Error> {
    let mut a = [0; 7];
    for (i, w) in a.iter_mut().enumerate() {
        *w = m.u32(esp.wrapping_add(4 + 4 * i as u32))?;
    }
    Ok(a)
}

/// The callback entered with ESP `esp` (its return address), under
/// `mxcsr` and the x87 control word `fcw`, up to its call of the emitter.
pub fn predict(m: &dyn GuestMemory, esp: u32, mxcsr: u32, fcw: u16) -> Result<Prediction, Error> {
    let [particle, state, position, axis, size, tint, draw] = args(m, esp)?;
    let mut x = Model::new(m, mxcsr, fcw)?;
    let one = x.one;
    let definition = x.u32(draw)?;

    // 1. The inputs.
    let wanted = x.u32(definition + 0x98)? & CALLBACK_INPUTS;
    x.cache(state, wanted)?;

    // 2. Rotation, colour, alpha.
    let r = x.property(state, definition + 0x4c)?;
    let mut rotation = x.mul_at(r, constant::ANGLE)?;
    let rgb = x.colour(state, definition + 0x1c)?;
    let a = x.property(state, definition + 0x2c)?;
    let alpha = x.mul_at(a, tint)?;
    let flags = x.u32(definition)?;
    let rgb = if flags & 0x180 != 0 {
        let t = x.unpack(x.u32(particle + 0x3c)?)?;
        let k = x.u32(constant::TINT)?;
        let mut out = [0; 3];
        for i in 0..3 {
            let v = x.mul(rgb[i], t[i])?;
            out[i] = x.mul(v, k)?;
        }
        out
    } else {
        rgb
    };
    let mut lanes = [alpha, 0, 0, 0];
    for i in 0..3 {
        let t = x.u32(tint + 4 + 4 * i as u32)?;
        lanes[i + 1] = x.mul(t, rgb[i])?;
    }
    for lane in &mut lanes {
        *lane = x.s.clamp(*lane, 0, one)?;
    }
    let colour = pack_colour(lanes, x.u32(constant::PACK)?, fcw)?;

    // 3. The windows and corners.
    let (window, next_window, corners, blend) = if flags & 0x1_0000 != 0 {
        let w = [0xac, 0xb0, 0xb4, 0xb8].map(|o| x.m.u32(definition + o));
        let w = [w[0]?, w[1]?, w[2]?, w[3]?];
        let c = [0x9c, 0xa0, 0xa4, 0xa8].map(|o| x.m.u32(definition + o));
        let c = [c[0]?, c[1]?, c[2]?, c[3]?];
        let flips = x.flips(flags, particle)?;
        let width = if flips & 1 != 0 { x.sub(w[0], w[1])? } else { x.sub(w[1], w[0])? };
        let height = if flips & 2 != 0 { x.sub(w[2], w[3])? } else { x.sub(w[3], w[2])? };
        let u = if flips & 1 != 0 { w[1] } else { w[0] };
        let v = if flips & 2 != 0 { w[3] } else { w[2] };
        let win = [u, v, width, height];
        (win, win, c, 0)
    } else {
        sprite(&mut x, flags, particle, state, draw, definition)?
    };

    // 4. Rotation offset, size.
    if flags & 0x2000 != 0 {
        rotation = x.add(rotation, x.u32(constant::SPIN)?)?;
    }
    let s = x.property(state, definition + 0x3c)?;
    let size = x.mul(s, size)?;

    let frame = esp.wrapping_sub(0x70);
    let emission = Emission {
        words: [
            position,
            axis,
            size,
            rotation,
            colour,
            blend,
            frame.wrapping_add(0x40),
            frame.wrapping_add(0x60),
        ],
        ebx: frame.wrapping_add(0x30),
        window,
        next_window,
        corners,
    };
    Ok(Prediction { emission, writes: x.m.writes })
}

/// Step 3 without flag `0x10000`: the bitmap's frames.
fn sprite(
    x: &mut Model<'_>,
    flags: u32,
    particle: u32,
    state: u32,
    draw: u32,
    definition: u32,
) -> Result<([u32; 4], [u32; 4], [u32; 4], u32), Error> {
    let one = x.one;
    let half = x.u32(constant::HALF)?;
    let mut a = [0, one, 0, one];
    let mut b = [0, one, 0, one];
    let (mut c0, mut c1) = (half, half);
    let mut f = 0;
    let bitmap = x.u32(draw + 4)?;
    if bitmap != 0 && x.u16(bitmap)? == 3 {
        if flags & 0x10 != 0 {
            let n = x.u16(definition + 0xa)? as i16;
            let n = if n < 1 { 1 } else { i32::from(n) };
            let pick = i32::from(x.u16(particle + 0x16)?) % n;
            let mut seq = pick.wrapping_add(i32::from(x.u16(definition + 8)? as i16));
            if seq < 0 {
                seq = 0;
            } else {
                let last = (x.u32(bitmap + 0x3c)? as i32).wrapping_sub(1);
                if seq > last {
                    seq = last;
                }
            }
            let at = x.u32(bitmap + 0x40)?.wrapping_add((seq as u32).wrapping_mul(0x3c));
            x.m.set_u32(draw + 8, at)?;
        }
        let sequence = x.u32(draw + 8)?;
        if sequence != 0 {
            let count = x.u32(sequence + 0x34)? as i32;
            let frames = if x.u8(definition)? & 8 != 0 { count.wrapping_sub(1) } else { count };
            if count > 0 {
                let v = x.property(state, definition + 0x5c)?;
                let n = x.s.int(frames)?;
                let t = x.mul(v, n)?;
                let k = x.floor(t)?;
                let sequence = x.u32(draw + 8)?;
                let sprites = x.u32(sequence + 0x38)?;
                let count = x.u32(sequence + 0x34)? as i32;
                if count == 0 {
                    return Err(Error::Divide);
                }
                let first = sprites.wrapping_add((k.wrapping_rem(count) as u32) << 5);
                let next = sprites.wrapping_add((k.wrapping_add(1).wrapping_rem(count) as u32) << 5);
                if x.u8(definition)? & 0x20 == 0 {
                    let kf = x.s.int(k)?;
                    f = x.sub(t, kf)?;
                }
                for i in 0..4 {
                    a[i] = x.u32(first + 8 + 4 * i as u32)?;
                    b[i] = x.u32(next + 8 + 4 * i as u32)?;
                }
                let rest = x.sub(one, f)?;
                let p = x.u32(first + 0x18)?;
                let p = x.mul(p, rest)?;
                let q = x.u32(next + 0x18)?;
                let q = x.mul(q, f)?;
                c0 = x.add(p, q)?;
                let p = x.u32(first + 0x1c)?;
                let p = x.mul(p, rest)?;
                let q = x.u32(next + 0x1c)?;
                let q = x.mul(q, f)?;
                c1 = x.add(p, q)?;
            }
        }
    }

    // The frames' aspect (`0x002249cc`).
    let wa = x.sub(a[1], a[0])?;
    let ha = x.sub(a[3], a[2])?;
    let hb = x.sub(b[3], b[2])?;
    let iwa = x.s.div(one, wa)?;
    let iha = x.s.div(one, ha)?;
    let wb = x.sub(b[1], b[0])?;
    let iwb = x.s.div(one, wb)?;
    let ihb = x.s.div(one, hb)?;
    let flips = x.flips(flags, particle)?;
    let (mut sx, mut sy) = (one, one);
    let x7 = if x.s.above(wa, ha)? {
        sy = x.mul(ha, iwa)?;
        one
    } else {
        sx = x.mul(wa, iha)?;
        sx
    };
    let rest = x.sub(one, f)?;
    let x6 = if x.s.above(wb, hb)? {
        let v = x.mul(hb, iwb)?;
        let v = x.mul(v, f)?;
        let w = x.mul(rest, sy)?;
        sy = x.add(v, w)?;
        sx
    } else {
        let v = x.mul(wb, ihb)?;
        let w = x.mul(rest, x7)?;
        let v = x.mul(v, f)?;
        x.add(v, w)?
    };
    let p = x.mul(rest, iwa)?;
    let q = x.mul(rest, iha)?;
    let r = x.mul(iwb, f)?;
    let s = x.mul(ihb, f)?;
    let u_scale = x.add(p, r)?;
    let v_scale = x.add(q, s)?;
    let k = x.u32(draw + 0xc)?;
    let spread = x.mul(k, x6)?;
    let window = x.window(a, flips, wa, ha)?;
    let next_window = x.window(b, flips, wb, hb)?;

    // The corners (`0x00224bdf`).
    let v = x.mul(v_scale, c1)?;
    let u = x.mul(u_scale, c0)?;
    let tint = x.u32(constant::TINT)?;
    let v = x.mul(v, tint)?;
    let v = x.sub(v, one)?;
    let u = x.mul(u, tint)?;
    let corner = x.u32(constant::CORNER)?;
    let u = x.sub(u, one)?;
    let e = x.sub(one, u)?;
    let g = x.sub(one, v)?;
    let k0 = x.sub(corner, u)?;
    let k0 = x.mul(k0, spread)?;
    let k1 = x.sub(corner, v)?;
    let k1 = x.mul(k1, sy)?;
    let e = x.mul(e, spread)?;
    let g = x.mul(g, sy)?;
    let k2 = x.sub(e, k0)?;
    let k3 = x.sub(g, k1)?;
    Ok((window, next_window, [k0, k1, k2, k3], f))
}

#[cfg(test)]
mod tests;
