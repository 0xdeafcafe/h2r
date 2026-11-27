//! The joint constraint builders, `vt[+0x24]` of the six classes Halo's
//! `0x001d77d0` builds (docs/engine/havok-step.md, "Constraints"):
//! `thiscall` on the constraint with the pair words and the cursor
//! (`ret 8`). Each writes a header (`0x0031e770`) and then its rows and
//! schemas at the cursor: linear rows (`0x30` bytes, schema
//! [`SCHEMA_FREE_LINEAR`]) and angular rows (`0x20` bytes,
//! [`SCHEMA_FREE_ANGULAR`]).
//!
//! The constraint's pivots and axes are in each body's frame; the bodies'
//! transforms (`motion+0x80`: three basis columns and the translation) are
//! read from the motions, the centres, inverse masses and inertias from
//! the accumulators. Every float operation is one guest instruction in the
//! guest's operand order ([`Sse`], [`Fx`]); a non-finite float the x87
//! code would load declines ([`Fallback::Float`]).
use crate::fp::{Arith, Exact};
use crate::island_solve::{Fallback, CONSTRAINT_ENTITY_A, CONSTRAINT_ENTITY_B, ENTITY_MOTION};
use crate::jacobian::{lane, shuf, splat, vector, with_w, Cursor, Fpu, Fx, Scratch, Side, Sse};
use crate::mem::Journal;
use crate::solver::{finite, load, SCHEMA_FREE_ANGULAR, SCHEMA_FREE_LINEAR};
use x87::{F80, Mode};

/// Image constants the builders read, by address.
pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    /// Added to every joint row's effective mass.
    pub const SOFTNESS: u32 = 0x0041_5594;
    /// The three world axes the point-to-point rows use.
    pub const AXES: u32 = 0x0046_1ea0;
    /// The four-lane sign mask.
    pub const SIGN: u32 = 0x0041_24a0;
}

/// The header schema `0x0031e770` writes (the contact builder's id).
pub const SCHEMA_HEADER: u32 = 0x0001_1801;
/// Every joint's output stride: an impulse and a right-hand side.
pub const OUTPUT_STRIDE: u32 = 8;
const LINEAR_ROW: u32 = 0x30;
const ANGULAR_ROW: u32 = 0x20;

/// The joint classes, by Halo's kind (`vt[+0x20]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joint {
    /// Kind 0, `0x00314220`: three linear rows holding two pivots together.
    BallSocket,
    /// Kind 1, `0x00313ef0`: two angular rows keeping the axes parallel,
    /// then the ball-and-socket rows.
    Hinge,
    /// Kind 8, `0x003149e0`: one linear row holding the pivots at a rest
    /// length.
    StiffSpring,
}

pub const BALL_SOCKET_BUILD: u32 = 0x0031_4220;
pub const HINGE_BUILD: u32 = 0x0031_3ef0;
pub const STIFF_SPRING_BUILD: u32 = 0x0031_49e0;

impl Joint {
    pub const ALL: [Self; 3] = [Self::BallSocket, Self::Hinge, Self::StiffSpring];

    pub fn of(build: u32) -> Option<Self> {
        match build {
            BALL_SOCKET_BUILD => Some(Self::BallSocket),
            HINGE_BUILD => Some(Self::Hinge),
            STIFF_SPRING_BUILD => Some(Self::StiffSpring),
            _ => None,
        }
    }

    pub fn build_address(self) -> u32 {
        match self {
            Self::BallSocket => BALL_SOCKET_BUILD,
            Self::Hinge => HINGE_BUILD,
            Self::StiffSpring => STIFF_SPRING_BUILD,
        }
    }
}

/// One body's transform, `motion+0x80`.
#[derive(Clone, Copy, Debug)]
struct Transform {
    basis: [u128; 3],
    translation: u128,
}

/// The float units, the image constants and the two sides of one call.
struct Build<'s> {
    sse: Sse,
    x: Fx,
    one: u32,
    softness: u32,
    sign: u128,
    a: Side,
    b: Side,
    /// `pairs+0x0c`, the right-hand-side scale.
    rhs_scale: u32,
    s: &'s mut Scratch,
    cursor: Cursor,
}

impl Build<'_> {
    fn ld(&self, bits: u32) -> Result<F80, Fallback> {
        if finite(bits) { Ok(load(bits)) } else { Err(Fallback::Float) }
    }

    /// `c0 x + c1 y + c2 z` (`0x002da6e0` and the builders' inline loops).
    fn rotate(&mut self, t: &Transform, v: u128) -> u128 {
        self.sse.rotate(&t.basis, v)
    }

    /// `(c0 x + c1 y) + (c2 z + t)` (`0x002da610`, `0x0031f160`).
    fn point(&mut self, t: &Transform, v: u128) -> u128 {
        let s = &mut self.sse;
        let z = s.mul(t.basis[2], splat(lane(v, 2)));
        let zt = s.add(z, t.translation);
        let y = s.mul(t.basis[1], splat(lane(v, 1)));
        let x = s.mul(t.basis[0], splat(lane(v, 0)));
        let xy = s.add(x, y);
        s.add(xy, zt)
    }

    /// The header `0x0031e770`: id, the first row, the output, the two
    /// accumulators, the stride.
    fn header(&mut self, acc: [u32; 2], output: u32) -> Result<(), Fallback> {
        let at = self.cursor.schemas;
        for (i, v) in [SCHEMA_HEADER, self.cursor.rows, output, acc[0], acc[1], OUTPUT_STRIDE].into_iter().enumerate() {
            self.s.put(at.wrapping_add(4 * i as u32), v).ok_or(Fallback::Memory)?;
        }
        self.cursor.schemas = at.wrapping_add(0x18);
        Ok(())
    }

    fn schema(&mut self, id: u32) -> Result<(), Fallback> {
        self.s.put(self.cursor.schemas, id).ok_or(Fallback::Memory)?;
        self.cursor.schemas = self.cursor.schemas.wrapping_add(4);
        Ok(())
    }

    fn put_vectors(&mut self, vs: &[u128]) -> Result<(), Fallback> {
        for (i, v) in vs.iter().enumerate() {
            let at = self.cursor.rows.checked_add(0x10 * i as u32).ok_or(Fallback::Overflow)?;
            self.s.put_vector(at, *v).ok_or(Fallback::Overflow)?;
        }
        Ok(())
    }

    /// One linear row along `axis` between the world points `wa` and `wb`
    /// (`0x0031ec30`, and each pass of `0x0031f160` and `0x0031f3e0`):
    /// the lever arms crossed with the axis into each body's frame, the
    /// effective mass with the softness, the separation along the axis
    /// times the scale. Returns the row's address.
    fn linear(&mut self, wa: u128, wb: u128, axis: u128) -> Result<u32, Fallback> {
        let (a, b) = (self.a, self.b);
        let s = &mut self.sse;
        let ra = s.sub(wa, a.center);
        let rb = s.sub(wb, b.center);
        let right = s.mul(shuf(ra, 0xd2), shuf(axis, 0xc9));
        let left = s.mul(shuf(ra, 0xc9), shuf(axis, 0xd2));
        let ang_a = s.sub(left, right);
        let ang_a = a.local(s, ang_a);
        let right = s.mul(shuf(axis, 0xd2), shuf(rb, 0xc9));
        let left = s.mul(shuf(axis, 0xc9), shuf(rb, 0xd2));
        let ang_b = s.sub(left, right);
        let ang_b = b.local(s, ang_b);
        let masses = self.x.add(self.ld(b.inverse_mass())?, self.ld(a.inverse_mass())?);
        let masses = self.x.add(masses, self.ld(self.softness)?);
        let q = self.sse.quad(&a, &b, ang_a, ang_b);
        let mass = self.x.add(masses, self.ld(q)?);
        let inverse = self.x.st(self.x.div(self.ld(self.one)?, mass));
        let mass = self.x.st(mass);
        let s = &mut self.sse;
        let d = s.sub(wb, wa);
        let e = s.mul(d, axis);
        let e = s.hsum(e);
        let rhs = self.x.st(self.x.mul(self.ld(e)?, self.ld(self.rhs_scale)?));
        let at = self.cursor.rows;
        self.put_vectors(&[with_w(axis, rhs), with_w(ang_a, inverse), with_w(ang_b, mass)])?;
        self.cursor.rows = at.wrapping_add(LINEAR_ROW);
        self.schema(SCHEMA_FREE_LINEAR)?;
        Ok(at)
    }

    /// The three world-axis rows holding `wa` and `wb` together
    /// (`0x0031f160`, `0x0031f3e0`).
    fn points(&mut self, axes: &[u128; 3], wa: u128, wb: u128) -> Result<(), Fallback> {
        for axis in axes {
            self.linear(wa, wb, *axis)?;
        }
        Ok(())
    }

    /// One angular row about `axis` (`0x0031e7b0`): the axis into A's
    /// frame, its negation into B's, the inverse effective mass with the
    /// softness, and `-(v . u)` times the scale as the right-hand side.
    fn angular(&mut self, u: u128, v: u128, axis: u128) -> Result<(), Fallback> {
        let (a, b) = (self.a, self.b);
        let s = &mut self.sse;
        let ang_a = a.local(s, axis);
        let ang_b = b.local(s, axis ^ self.sign);
        let q = self.sse.quad(&a, &b, ang_a, ang_b);
        let x = &self.x;
        let inverse = x.st(x.div(self.ld(self.one)?, x.add(self.ld(q)?, self.ld(self.softness)?)));
        let l = |i: usize| -> Result<F80, Fallback> { Ok(x.mul(self.ld(lane(v, i))?, self.ld(lane(u, i))?)) };
        let dot = x.add(x.add(l(2)?, l(1)?), l(0)?);
        let rhs = x.st(x.mul(dot, self.ld(self.rhs_scale)?).neg());
        let at = self.cursor.rows;
        self.put_vectors(&[with_w(ang_a, inverse), with_w(ang_b, rhs)])?;
        self.cursor.rows = at.wrapping_add(ANGULAR_ROW);
        self.schema(SCHEMA_FREE_ANGULAR)
    }
}

fn transform(j: &Journal, constraint: u32, side: u32) -> Result<Transform, Fallback> {
    let word = |at: u32| j.get(at).ok_or(Fallback::Memory);
    let entity = word(constraint.wrapping_add(side))?;
    let motion = word(entity.wrapping_add(ENTITY_MOTION))?;
    let v = |off: u32| vector(j, motion.wrapping_add(0x80 + off)).ok_or(Fallback::Memory);
    Ok(Transform { basis: [v(0)?, v(0x10)?, v(0x20)?], translation: v(0x30)? })
}

/// `vt[+0x24]` of `joint` on the constraint at `c`: its header, rows and
/// schemas at `cursor`.
pub fn build(
    j: &mut Journal,
    s: &mut Scratch,
    fpu: Fpu,
    joint: Joint,
    c: u32,
    pairs: u32,
    cursor: &mut Cursor,
) -> Result<(), Fallback> {
    use Fallback::{Float, Memory};
    if !fpu.masked() {
        return Err(Float);
    }
    let word = |at: u32| j.get(at).ok_or(Memory);
    let acc = [word(pairs)?, word(pairs.wrapping_add(4))?];
    let rhs_scale = word(pairs.wrapping_add(0x0c))?;
    let local = |off: u32| vector(j, c.wrapping_add(off)).ok_or(Memory);
    let ta = transform(j, c, CONSTRAINT_ENTITY_A)?;
    let tb = transform(j, c, CONSTRAINT_ENTITY_B)?;
    let axes = [0, 0x10, 0x20].map(|off| vector(j, constant::AXES + off));
    let [Some(ax), Some(ay), Some(az)] = axes else { return Err(Memory) };
    let mut b = Build {
        sse: Sse { e: Exact::new(fpu.mxcsr) },
        x: Fx { mode: Mode::from_cw(fpu.control) },
        one: word(constant::ONE)?,
        softness: word(constant::SOFTNESS)?,
        sign: vector(j, constant::SIGN).ok_or(Memory)?,
        a: Side::read(s, acc[0]).ok_or(Memory)?,
        b: Side::read(s, acc[1]).ok_or(Memory)?,
        rhs_scale,
        s,
        cursor: *cursor,
    };
    let axes = [ax, ay, az];
    match joint {
        Joint::BallSocket => {
            b.header(acc, c.wrapping_add(0x40))?;
            let wa = b.point(&ta, local(0x20)?);
            let wb = b.point(&tb, local(0x30)?);
            b.points(&axes, wa, wb)?;
        }
        Joint::Hinge => {
            b.header(acc, c.wrapping_add(0x70))?;
            let pivot_a = b.rotate(&ta, local(0x20)?);
            let axis_a = b.rotate(&ta, local(0x30)?);
            let perp_a = b.rotate(&ta, local(0x40)?);
            let pivot_a = b.sse.add(pivot_a, ta.translation);
            let pivot_b = b.rotate(&tb, local(0x50)?);
            let axis_b = b.rotate(&tb, local(0x60)?);
            let pivot_b = b.sse.add(pivot_b, tb.translation);
            b.angular(perp_a, axis_b, axis_a)?;
            let flipped = axis_b ^ b.sign;
            b.angular(axis_a, flipped, perp_a)?;
            b.points(&axes, pivot_a, pivot_b)?;
        }
        Joint::StiffSpring => {
            b.header(acc, c.wrapping_add(0x44))?;
            let wa = b.point(&ta, local(0x20)?);
            let wb = b.point(&tb, local(0x30)?);
            let e = &mut b.sse;
            let d = e.sub(wa, wb);
            let sq = e.mul(d, d);
            let n = e.hsum(sq);
            let r = e.e.rsqrt(n);
            let t = e.e.mul(n, r);
            let t = e.e.mul(t, r);
            let u = e.e.sub(3.0f32.to_bits(), t);
            let h = e.e.mul(0.5f32.to_bits(), r);
            let k = e.e.mul(h, u);
            let length = e.e.mul(n, k);
            let axis = e.mul(splat(k), d);
            let row = b.linear(wa, wb, axis)?;
            // `0x0031e740`: the rest length less the length, times the
            // scale, over the row's right-hand side.
            let rest = b.ld(word(c.wrapping_add(0x40))?)?;
            let error = load(b.x.st(b.x.sub(rest, b.ld(length)?)));
            let rhs = b.x.st(b.x.mul(error, b.ld(rhs_scale)?));
            b.s.put(row + 0x0c, rhs).ok_or(Memory)?;
        }
    }
    if b.sse.e.trapped {
        return Err(Float);
    }
    *cursor = b.cursor;
    Ok(())
}
