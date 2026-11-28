//! A contrail's ribbon in Rust: `0x00042cf0`, the vertex builder the
//! sorted contrail handler `0x00035b00` hands the dynamic draw `0x0004b220`
//! (stride 24, two vertices a point), from its entry to its return
//! (docs/graphics/retire-d3d-r2.md, "R2's ribbon arm"). [`predict`] makes
//! the vertices it writes, its return and every other guest write.
//!
//! **Code** (`h2 disasm`): `0x00042cf0(out, bytes, ribbon)` (`ret 0xc`,
//! AL the result, `bytes` unread). `ribbon` is the draw's context: `+0`
//! the contrail, `+4` its definition, `+8` the point list's slot, `+0xc`
//! a block whose `+0x2c` word is the fade mode, `+0x10` a byte that asks
//! for the normal, `+0x14` the vertex count. Points live in the data
//! array `[[0x004ea940] + 0x44]` (0x38 bytes, `+0x34` the next, `-1` the
//! end).
//!
//! 1. The along coordinate starts at the contrail's `+0x1c` and steps by
//!    the definition's `+0x1c` (times the contrail's `+0x14` with
//!    definition byte `+2` bit `0x40`); the across pair is the contrail's
//!    `+0x20` and that plus the definition's `+0x20` (bit `0x80` scales it
//!    the same way). The first point is the contrail's `+0x38` slot
//!    (`ribbon + 8`).
//! 2. Per point: its stage (byte `+3`, the definition's `+0xe8` stages at
//!    `+0xec`, 64 bytes each) gives the colour and width, lerped by the
//!    point's `+0xc` (stage flags `0x20`, `0x10`); point flag `2` blends to
//!    the next stage by `+4`. A stage out of range is black and thin. The
//!    contrail's object (`+8`, unless definition bit `0x80`) tints the
//!    colour by its function source (`0x000bad50`) or `[[0x00468710]]`.
//! 3. The texture words go first (`+0xc`, `+0x10` of both vertices). The
//!    definition's `+0x18` word picks the facing: 0 vertical, 1 and 2 flat
//!    (across the step in x and y), 4 the camera (`0x004b9da0`); 3 and
//!    above 4 return 0 there and then. The facings that ask (`ribbon +
//!    0x10`) leave the normal (ESP `-0xa8` from the entry's) for the
//!    fade; it keeps what it held otherwise, the stack's old bytes at
//!    first.
//! 4. `0x00042c50` fades the alpha by the view's angle to the normal
//!    (mode 1, or 2 inverted; definition bit `0x40` eases it through
//!    `0x0017ca10`). Alpha clamped, the colour is packed as `0x00131e50`.
//! 5. After the list, definition bits 1 and 2 clear unless set: the first
//!    pair's alpha (when the contrail's `+0x30` word of the slot is above
//!    2) and the last pair's, read at `out + 24 * count` from the end.
//!    AL 1.
//!
//! Every operation is the original's: SSE in its operand order under the
//! guest's MXCSR, the x87 parts under its control word, every constant
//! read from the image. A list longer than [`BUDGET`] points is refused
//! ([`Error::Unbounded`]): the original would run on through a cycle.

use std::collections::BTreeMap;

use crate::colour_pass::pack_colour;
use crate::function_eval::{
    self,
    constant::{ONE, SMALL, ZERO},
};
use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;
use crate::particle_emission::Overlay;

use x87::abi::{arith_op, unary_op};

/// The builder.
pub const ENTRY: u32 = 0x0004_2cf0;
/// What it pops: its return and three arguments.
pub const POP: u32 = 0x10;
/// Bytes a point writes: two vertices of 24.
pub const POINT_BYTES: u32 = 0x30;
/// The entry's ESP less the normal's address (`[esp+0x14]` in the body).
pub const NORMAL_DEPTH: u32 = 0xa8;
/// The most points modelled.
pub const BUDGET: u32 = 0x1000;

/// Addresses the builder reads.
pub mod constant {
    /// 0.5 (f32): the half width.
    pub const HALF: u32 = 0x0045_dbbc;
    /// The colour packer's scale (f32).
    pub const PACK: u32 = 0x0045_dc38;
    /// The points' data array (pointer; `+0x44` the data).
    pub const POINTS: u32 = 0x004e_a940;
    /// The objects' data array (pointer; `+0x44` the data, 12 bytes each).
    pub const OBJECTS: u32 = 0x004e_0300;
    /// The tag instances (pointer; 16 bytes each, `+8` the definition).
    pub const TAGS: u32 = 0x004e_3b44;
    /// A pointer to the untinted colour (three f32).
    pub const WHITE: u32 = 0x0046_8710;
    /// A pointer to the flat facings' normal (three f32).
    pub const UP: u32 = 0x0046_87b0;
    /// The view's position (three f32).
    pub const CAMERA: u32 = 0x004b_9da0;
}

/// The model's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prediction {
    /// The first argument: where the vertices go.
    pub out: u32,
    /// The points begun, the last perhaps only its texture words.
    pub points: u32,
    /// `points * 0x30` bytes from `out` as the builder leaves them.
    pub vertices: Vec<u8>,
    /// AL at the return.
    pub ret: u8,
    /// Every guest byte the model writes (the frame's locals aside).
    pub writes: BTreeMap<u32, u8>,
}

impl Prediction {
    /// Whether `addr` is in the vertices.
    pub fn in_vertices(&self, addr: u32) -> bool {
        addr.wrapping_sub(self.out) < self.vertices.len() as u32
    }
}

/// The counter name of a refusal (`ribbon_refused_<name>`).
pub fn refusal(e: Error) -> &'static str {
    crate::particle_emission::refusal(e)
}

struct Model<'a> {
    m: Overlay<'a>,
    s: Sse,
    fcw: u16,
    one: u32,
}

/// A point's position.
type Vec3 = [u32; 3];

impl Model<'_> {
    fn u32(&self, a: u32) -> Result<u32, Error> {
        Ok(self.m.u32(a)?)
    }
    fn u16(&self, a: u32) -> Result<u16, Error> {
        Ok(self.m.u16(a)?)
    }
    fn u8(&self, a: u32) -> Result<u8, Error> {
        Ok(self.m.u8(a)?)
    }
    fn vec3(&self, a: u32) -> Result<Vec3, Error> {
        Ok([self.u32(a)?, self.u32(a.wrapping_add(4))?, self.u32(a.wrapping_add(8))?])
    }
    fn add(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        self.s.add(a, b)
    }
    fn sub(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        self.s.sub(a, b)
    }
    fn mul(&mut self, a: u32, b: u32) -> Result<u32, Error> {
        self.s.mul(a, b)
    }
    fn set(&mut self, a: u32, v: u32) -> Result<(), Error> {
        Ok(self.m.set_u32(a, v)?)
    }

    /// Point `i` of the data array (`[[POINTS] + 0x44] + (i & 0xffff) *
    /// 0x38`).
    fn point(&self, i: u32) -> Result<u32, Error> {
        let data = self.u32(self.u32(constant::POINTS)?.wrapping_add(0x44))?;
        Ok(data.wrapping_add((i & 0xffff).wrapping_mul(0x38)))
    }

    /// The sum on the x87 stack: `fsqrt; fst dword; fabs; fld SMALL;
    /// fcomip; fstp st0`. The f32 length and whether SMALL is above it
    /// (`ja`: too short to normalise).
    fn length(&self, x: &mut X87) -> Result<(u32, bool), Error> {
        x.unary(unary_op::SQRT)?;
        let len = x.fst_m32()?;
        x.unary(unary_op::ABS)?;
        x.fld(self.u32(SMALL)?)?;
        let f = x.fcomip(1)?;
        x.fstp_st(0)?;
        Ok((len, f & 0x41 == 0))
    }

    /// `fld a; fmul b; fld c; fmul d; fsubp`: `a * b - c * d` on the stack.
    fn term(x: &mut X87, a: u32, b: u32, c: u32, d: u32) -> Result<(), Error> {
        x.fld(a)?;
        x.fmul_m(b)?;
        x.fld(c)?;
        x.fmul_m(d)?;
        x.arith_reg(arith_op::SUB, 1, 0, true)
    }

    /// `v * v` added to the stack's top (`fld; fmul; faddp`).
    fn square_add(x: &mut X87, v: u32) -> Result<(), Error> {
        x.fld(v)?;
        x.fmul_m(v)?;
        x.arith_reg(arith_op::ADD, 1, 0, true)
    }

    /// `a + b` with the three of `a` and `b` crossed (`[a1 b2 - a2 b1,
    /// ...]` as the guest orders it), each stored as f32; then the length
    /// of the result. `a` and `b` as the guest's locals.
    fn cross(&self, a: Vec3, b: Vec3) -> Result<(Vec3, u32, bool), Error> {
        let mut x = X87::new(self.fcw);
        Self::term(&mut x, a[2], b[1], a[1], b[2])?;
        let c0 = x.fstp_m32()?;
        Self::term(&mut x, a[0], b[2], a[2], b[0])?;
        let c1 = x.fstp_m32()?;
        Self::term(&mut x, a[1], b[0], a[0], b[1])?;
        let c2 = x.fst_m32()?;
        x.fmul_m(c2)?;
        Self::square_add(&mut x, c1)?;
        Self::square_add(&mut x, c0)?;
        let (len, short) = self.length(&mut x)?;
        Ok(([c0, c1, c2], len, short))
    }

    /// Step 2: the point's alpha and colour lanes and its width.
    fn stage(&mut self, def: u32, pt: u32) -> Result<([u32; 4], u32), Error> {
        let al = self.u8(pt + 3)? as i8 as i32;
        let idx = if al < 0 {
            0
        } else {
            let last = (self.u32(def + 0xe8)? as i32).wrapping_sub(1);
            al.min(last)
        };
        if idx != al {
            return Ok(([0; 4], 0));
        }
        let stages = self.u32(def + 0xec)?;
        let s = stages.wrapping_add((al as u32) << 6);
        let flags = self.u32(s.wrapping_add(0x3c))? as u8;
        let k = if flags & 0x20 != 0 { self.u32(pt + 0xc)? } else { self.one };
        let mut w = self.u32(s.wrapping_add(0x18))?;
        if flags & 0x10 != 0 {
            let a = self.u32(pt + 0xc)?;
            w = self.mul(a, w)?;
        }
        let mut c = [0; 4];
        for (i, lane) in c.iter_mut().enumerate() {
            let lo = s.wrapping_add(0x1c + 4 * i as u32);
            let d = self.sub(self.u32(lo.wrapping_add(0x10))?, self.u32(lo)?)?;
            let d = self.mul(d, k)?;
            *lane = self.add(d, self.u32(lo)?)?;
        }
        if self.u8(pt + 2)? & 2 == 0 {
            return Ok((c, w));
        }
        // The blend to the next stage (`0x00042ec5`).
        let i = (self.u8(pt + 3)? as i8 as i32).wrapping_add(1);
        let n = stages.wrapping_add((i as u32) << 6);
        let flags = self.u8(n.wrapping_add(0x3c))?;
        let mut w2 = self.u32(n.wrapping_add(0x18))?;
        let t = self.u32(pt + 4)?;
        if flags & 0x10 != 0 {
            let a = self.u32(pt + 0xc)?;
            w2 = self.mul(a, w2)?;
        }
        let at = |o: u32| n.wrapping_add(o);
        let mut d = [0; 4];
        d[0] = self.sub(self.u32(at(0x2c))?, self.u32(at(0x1c))?)?;
        w2 = self.sub(w2, w)?;
        for (j, lane) in d.iter_mut().enumerate().skip(1) {
            let o = 4 * j as u32;
            *lane = self.sub(self.u32(at(0x2c + o))?, self.u32(at(0x1c + o))?)?;
        }
        w2 = self.mul(w2, t)?;
        w2 = self.add(w2, w)?;
        for (j, lane) in d.iter_mut().enumerate() {
            let v = self.mul(*lane, k)?;
            *lane = self.add(v, self.u32(at(0x1c + 4 * j as u32))?)?;
        }
        for j in 0..4 {
            let v = self.sub(d[j], c[j])?;
            let v = self.mul(v, t)?;
            c[j] = self.add(v, c[j])?;
        }
        Ok((c, w2))
    }

    /// `0x000bad50` (EAX the object, EDI the index, ESI the out): the
    /// object's function source `index`, when it has one.
    fn function_source(&self, object: u32, index: i32) -> Result<Option<Vec3>, Error> {
        let data = self.u32(self.u32(constant::OBJECTS)?.wrapping_add(0x44))?;
        let datum = self.u32(data.wrapping_add((object & 0xffff) * 12).wrapping_add(8))?;
        let table = (self.u16(datum.wrapping_add(0x126))? as i16 as i32 as u32).wrapping_add(datum);
        let words = self.u16(datum.wrapping_add(0x124))? as i16 as i32 as u32;
        let count = (((u64::from(words) * 0xaaaa_aaab) >> 32) as u32 >> 3) as i32 >> 1;
        if index < 0 || index >= count {
            return Ok(None);
        }
        let at = table.wrapping_add((count.wrapping_add(index) as u32).wrapping_mul(12));
        Ok(Some(self.vec3(at)?))
    }

    /// Step 2's tint: lanes 1 to 3 times the object's colour.
    fn tint(&mut self, c: u32, def: u32, mut lanes: [u32; 4]) -> Result<[u32; 4], Error> {
        let object = self.u32(c + 8)?;
        if object == u32::MAX || self.u8(def)? & 0x80 != 0 {
            return Ok(lanes);
        }
        let mut colour = self.vec3(self.u32(constant::WHITE)?)?;
        let data = self.u32(self.u32(constant::OBJECTS)?.wrapping_add(0x44))?;
        let datum = self.u32(data.wrapping_add((object & 0xffff) * 12).wrapping_add(8))?;
        let tag = self.u32(datum)? & 0xffff;
        let tag = self.u32((tag << 4).wrapping_add(self.u32(constant::TAGS)?).wrapping_add(8))?;
        let pick = self.u16(c + 0xc)? as i16 as i32 as u32;
        let block = self.u32(tag.wrapping_add(0x98))?;
        let index = self.u16(block.wrapping_add(pick.wrapping_mul(24)).wrapping_add(0xc))? as i16 as i32;
        if let Some(v) = self.function_source(object, index)? {
            colour = v;
        }
        for i in 0..3 {
            lanes[i + 1] = self.mul(colour[i], lanes[i + 1])?;
        }
        Ok(lanes)
    }

    /// `0x00042c50` (SI the mode, EAX the normal, ECX the position, the
    /// stack word the definition): 1, or `|view . normal| / |view|` in
    /// x87 (eased with definition bit `0x40`, inverted in mode 2).
    fn fade(&mut self, mode: u16, normal: Vec3, position: u32, def: u32) -> Result<u32, Error> {
        let one = self.u32(ONE)?;
        if mode == 0 {
            return Ok(one);
        }
        let camera = self.vec3(constant::CAMERA)?;
        let mut x = X87::new(self.fcw);
        for (i, c) in camera.iter().enumerate() {
            x.fld(*c)?;
            x.arith_m(arith_op::SUB, self.u32(position + 4 * i as u32)?)?;
        }
        x.fld_st(0)?;
        x.fmul_m(normal[2])?;
        x.fld_st(2)?;
        x.fmul_m(normal[1])?;
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        x.fld_st(3)?;
        x.fmul_m(normal[0])?;
        let ease = self.u8(def)? & 0x40 != 0;
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        x.fld_st(1)?;
        x.arith_st(arith_op::MUL, 2)?;
        x.fld_st(3)?;
        x.arith_st(arith_op::MUL, 4)?;
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        x.fld_st(4)?;
        x.arith_st(arith_op::MUL, 5)?;
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        x.unary(unary_op::SQRT)?;
        x.fstp_st(4)?;
        x.fxch(3)?;
        x.arith_reg(arith_op::DIV, 3, 0, true)?;
        x.fxch(2)?;
        x.unary(unary_op::ABS)?;
        x.fstp_st(2)?;
        x.fstp_st(0)?;
        let mut v = x.fstp_m32()?;
        let mut one = one;
        if ease {
            v = function_eval::shape(&self.m, &mut self.s, self.fcw, 2, v)?;
            one = self.u32(ONE)?;
        }
        if mode == 2 {
            v = self.sub(one, v)?;
        }
        Ok(v)
    }
}

/// What a facing leaves: the two vertices' positions.
struct Pair {
    near: Vec3,
    far: Vec3,
}

impl Model<'_> {
    /// Facing 0 (`0x000430de`): the pair one above the other.
    fn vertical(&mut self, ctx: u32, pt: u32, prev: u32, hw: u32, normal: &mut Vec3) -> Result<Pair, Error> {
        let ask = self.u8(ctx + 0x10)?;
        let [px, py, pz] = self.vec3(pt + 0x1c)?;
        let lo = self.sub(pz, hw)?;
        let hi = self.add(pz, hw)?;
        if ask != 0 {
            let (dx, dy) = if prev != 0 {
                let py2 = self.u32(prev + 0x20)?;
                let dx = self.sub(px, self.u32(prev + 0x1c)?)?;
                (dx, self.sub(py2, py)?)
            } else {
                let n = self.point(self.u32(pt + 0x34)?)?;
                let dy = self.sub(self.u32(pt + 0x20)?, self.u32(n + 0x20)?)?;
                (self.sub(self.u32(n + 0x1c)?, self.u32(pt + 0x1c)?)?, dy)
            };
            *normal = [dy, dx, 0];
            let mut x = X87::new(self.fcw);
            x.fld(dx)?;
            x.fmul_m(dx)?;
            Self::square_add(&mut x, dy)?;
            let (len, short) = self.length(&mut x)?;
            if !short {
                let inv = self.s.div(self.u32(ONE)?, len)?;
                let a = self.mul(inv, dy)?;
                let b = self.mul(inv, normal[1])?;
                let c = self.mul(inv, self.u32(ZERO)?)?;
                *normal = [a, b, c];
            }
        }
        Ok(Pair { near: [px, py, hi], far: [px, py, lo] })
    }

    /// Facings 1 and 2 (`0x000431d8`): the pair across the step in x and y.
    fn flat(&mut self, ctx: u32, pt: u32, prev: u32, hw: u32, normal: &mut Vec3) -> Result<Pair, Error> {
        let (mut a, mut b) = if prev != 0 {
            let a = self.sub(self.u32(prev + 0x20)?, self.u32(pt + 0x20)?)?;
            (a, self.sub(self.u32(pt + 0x1c)?, self.u32(prev + 0x1c)?)?)
        } else {
            let n = self.point(self.u32(pt + 0x34)?)?;
            let a = self.sub(self.u32(pt + 0x20)?, self.u32(n + 0x20)?)?;
            (a, self.sub(self.u32(n + 0x1c)?, self.u32(pt + 0x1c)?)?)
        };
        let mut x = X87::new(self.fcw);
        x.fld(b)?;
        x.fmul_m(b)?;
        Self::square_add(&mut x, a)?;
        let (len, short) = self.length(&mut x)?;
        if !short {
            let inv = self.s.div(self.one, len)?;
            let a2 = self.mul(inv, a)?;
            b = self.mul(inv, b)?;
            a = a2;
        }
        let py = self.u32(pt + 0x20)?;
        let px = self.u32(pt + 0x1c)?;
        let ask = self.u8(ctx + 0x10)?;
        let pz = self.u32(pt + 0x24)?;
        let s = self.mul(a, hw)?;
        let t = self.mul(b, hw)?;
        let y1 = self.sub(py, t)?;
        let y0 = self.add(t, self.u32(pt + 0x20)?)?;
        let x1 = self.sub(self.u32(pt + 0x1c)?, s)?;
        let x0 = self.add(px, s)?;
        if ask != 0 {
            *normal = self.vec3(self.u32(constant::UP)?)?;
        }
        Ok(Pair { near: [x0, y0, pz], far: [x1, y1, pz] })
    }

    /// Facing 4 (`0x000432cd`): the pair across the step and the view.
    fn camera(&mut self, ctx: u32, pt: u32, prev: u32, hw: u32, normal: &mut Vec3) -> Result<Pair, Error> {
        let cam = self.vec3(constant::CAMERA)?;
        let (from, to) = if prev != 0 { (prev, pt) } else { (pt, self.point(self.u32(pt + 0x34)?)?) };
        let mut view = [0; 3];
        let mut step = [0; 3];
        if prev != 0 {
            for i in 0..3 {
                view[i] = self.sub(cam[i], self.u32(from + 0x1c + 4 * i as u32)?)?;
            }
        } else {
            for i in 0..3 {
                view[i] = self.sub(cam[i], self.u32(pt + 0x1c + 4 * i as u32)?)?;
            }
        }
        for (i, s) in step.iter_mut().enumerate() {
            let o = 0x1c + 4 * i as u32;
            *s = self.sub(self.u32(to + o)?, self.u32(from + o)?)?;
        }
        let (mut side, len, short) = self.cross(step, view)?;
        if !short {
            let inv = self.s.div(self.one, len)?;
            for c in &mut side {
                *c = self.mul(inv, *c)?;
            }
        }
        let [px, py, pz] = self.vec3(pt + 0x1c)?;
        let ask = self.u8(ctx + 0x10)?;
        let a = self.mul(side[0], hw)?;
        let x1 = self.sub(px, a)?;
        let x0 = self.add(a, self.u32(pt + 0x1c)?)?;
        let b = self.mul(side[1], hw)?;
        let c = self.mul(side[2], hw)?;
        let y1 = self.sub(py, b)?;
        let y0 = self.add(b, self.u32(pt + 0x20)?)?;
        let z1 = self.sub(pz, c)?;
        let z0 = self.add(pz, c)?;
        if ask != 0 {
            let (n, len, short) = self.cross(side, step)?;
            *normal = n;
            if !short {
                let inv = self.s.div(self.one, len)?;
                for c in normal.iter_mut() {
                    *c = self.mul(inv, *c)?;
                }
            }
        }
        Ok(Pair { near: [x0, y0, z0], far: [x1, y1, z1] })
    }
}

/// The builder entered with ESP `esp` (its return address), under
/// `mxcsr` and the x87 control word `fcw`, to its return.
pub fn predict(m: &dyn GuestMemory, esp: u32, mxcsr: u32, fcw: u16) -> Result<Prediction, Error> {
    let out = m.u32(esp.wrapping_add(4))?;
    let ctx = m.u32(esp.wrapping_add(0xc))?;
    let one = m.u32(ONE)?;
    let mut x = Model {
        m: Overlay { base: m, writes: BTreeMap::new() },
        s: Sse { mxcsr },
        fcw,
        one,
    };
    // The normal's slot keeps the stack's bytes until a facing fills it.
    let mut normal = x.vec3(esp.wrapping_sub(NORMAL_DEPTH))?;

    // 1. The texture words and the first point.
    let def = x.u32(ctx + 4)?;
    let flags = x.u16(def + 2)? as u8;
    let mut step = x.u32(def + 0x1c)?;
    let c = x.u32(ctx)?;
    let mut along = x.u32(c + 0x1c)?;
    if flags & 0x40 != 0 {
        let k = x.u32(c + 0x14)?;
        step = x.mul(k, step)?;
    }
    let advance = x.sub(0, step)?;
    let across = x.u32(c + 0x20)?;
    let mut width = x.u32(def + 0x20)?;
    if flags & 0x80 != 0 {
        let k = x.u32(c + 0x14)?;
        width = x.mul(k, width)?;
    }
    let across_far = x.add(width, across)?;
    let slot = x.u16(ctx + 8)? as i16 as i32 as u32;
    let mut next = x.u32(c.wrapping_add(slot.wrapping_mul(4)).wrapping_add(0x38))?;
    let mut prev = 0;
    let mut at = out;
    let mut points = 0;
    let half = constant::HALF;

    let finish = |x: Model<'_>, points: u32, ret: u8| -> Result<Prediction, Error> {
        let vertices = x.m.bytes(out, (points * POINT_BYTES) as usize)?;
        Ok(Prediction { out, points, vertices, ret, writes: x.m.writes })
    };

    // A list that comes back to a point is a cycle the original runs on
    // through until it faults: refuse it as unbounded before drawing past
    // the buffer. Brent's search: `mark` is a point seen `span` steps back,
    // so a cycle is found within twice its length, with no set kept.
    let (mut mark, mut span, mut steps) = (u32::MAX, 1u32, 0u32);
    while next != u32::MAX {
        if points == BUDGET || next == mark {
            return Err(Error::Unbounded);
        }
        if steps == span {
            (mark, span, steps) = (next, span << 1, 0);
        }
        steps += 1;
        points += 1;
        let pt = x.point(next)?;

        // 2. Colour and width.
        let (lanes, w) = x.stage(def, pt)?;
        let mut lanes = x.tint(c, def, lanes)?;
        let hw = x.mul(w, x.u32(half)?)?;

        // 3. The texture words, the facing.
        x.set(at + 0xc, along)?;
        x.set(at + 0x10, across)?;
        x.set(at + 0x24, along)?;
        x.set(at + 0x28, across_far)?;
        let facing = x.u16(def + 0x18)? as i16 as i32 as u32;
        let pair = match facing {
            0 => x.vertical(ctx, pt, prev, hw, &mut normal)?,
            1 | 2 => x.flat(ctx, pt, prev, hw, &mut normal)?,
            4 => x.camera(ctx, pt, prev, hw, &mut normal)?,
            _ => return finish(x, points, 0),
        };
        for (i, v) in pair.near.into_iter().enumerate() {
            x.set(at + 4 * i as u32, v)?;
        }
        for (i, v) in pair.far.into_iter().enumerate() {
            x.set(at + 0x18 + 4 * i as u32, v)?;
        }

        // 4. The fade and the colour.
        let mode = x.u16(x.u32(ctx + 0xc)?.wrapping_add(0x2c))?;
        let fade = x.fade(mode, normal, pt + 0x1c, def)?;
        let alpha = x.mul(fade, lanes[0])?;
        let one = x.u32(ONE)?;
        lanes[0] = x.s.clamp(alpha, 0, one)?;
        let colour = pack_colour(lanes, x.u32(constant::PACK)?, fcw)?;
        along = x.add(advance, along)?;
        x.set(at + 0x14, colour)?;
        x.set(at + 0x2c, colour)?;
        prev = pt;
        next = x.u32(pt + 0x34)?;
        at = at.wrapping_add(POINT_BYTES);
    }

    // 5. The end pairs' alpha.
    let count = x.u32(ctx + 0x14)?;
    let base = at.wrapping_add(count.wrapping_mul(0xffff_ffe8));
    if x.u8(def)? & 1 == 0 {
        let slot = x.u16(ctx + 8)? as i16 as i32 as u32;
        if x.u16(c.wrapping_add(slot.wrapping_mul(2)).wrapping_add(0x30))? as i16 > 2 {
            let v = x.u32(base.wrapping_add(0x14))? & 0x00ff_ffff;
            x.set(base.wrapping_add(0x14), v)?;
            x.set(base.wrapping_add(0x2c), v)?;
        }
    }
    if x.u8(def)? & 2 == 0 {
        let n = x.u32(ctx + 0x14)?;
        let last = base.wrapping_add(n.wrapping_mul(24)).wrapping_sub(4);
        let v = x.u32(last)? & 0x00ff_ffff;
        x.set(last, v)?;
        let n = x.u32(ctx + 0x14)?;
        x.set(base.wrapping_add(n.wrapping_mul(24)).wrapping_sub(0x1c), v)?;
    }
    finish(x, points, 1)
}

#[cfg(test)]
mod tests;
