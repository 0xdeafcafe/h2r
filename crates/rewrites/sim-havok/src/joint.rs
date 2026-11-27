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
use crate::solver::{
    finite, load, SCHEMA_CAPPED_ANGULAR, SCHEMA_CAPPED_LINEAR, SCHEMA_FREE_ANGULAR, SCHEMA_FREE_LINEAR,
    SCHEMA_RANGE_ANGULAR, SCHEMA_RANGE_LINEAR,
};
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
    /// Scales an angular limit row's inverse effective mass.
    pub const HALF: u32 = 0x0045_dbbc;
    pub const ZERO: u32 = 0x0045_dbd8;
    /// The angle approximation `0x003725e0`: the divisor's guard, its two
    /// polynomial terms, `pi / 2` and `pi`.
    pub const ATAN_GUARD: u32 = 0x0043_e50c;
    pub const ATAN_SQUARE: u32 = 0x0043_e518;
    pub const ATAN_CUBE: u32 = 0x0043_e514;
    pub const HALF_PI: u32 = 0x0041_23ec;
    pub const PI: u32 = 0x0043_e510;
    /// The limited hinge's wrap: a jump in angle past this adds or takes
    /// a turn (`0x00313a4c`).
    pub const WRAP: u32 = 0x0041_485c;
    pub const TURN: u32 = 0x0041_23e8;
    /// The ragdoll cone's squared-length floor (`0x00312e6f`).
    pub const CONE_FLOOR: u32 = 0x0041_46e4;
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
    /// Kind 2, `0x00313810`: the hinge's rows, an angle limit and a
    /// friction row.
    LimitedHinge,
    /// Kind 6, `0x00314fc0`: three angular rows locking the orientation,
    /// two linear rows across the slide axis, a range along it and a
    /// friction row.
    Prismatic,
    /// Kind 7, `0x00312fe0`: two cone limits, a twist limit, the
    /// ball-and-socket rows and three friction rows.
    Ragdoll,
}

pub const BALL_SOCKET_BUILD: u32 = 0x0031_4220;
pub const HINGE_BUILD: u32 = 0x0031_3ef0;
pub const STIFF_SPRING_BUILD: u32 = 0x0031_49e0;
pub const LIMITED_HINGE_BUILD: u32 = 0x0031_3810;
pub const PRISMATIC_BUILD: u32 = 0x0031_4fc0;
pub const RAGDOLL_BUILD: u32 = 0x0031_2fe0;

impl Joint {
    pub const ALL: [Self; 6] =
        [Self::BallSocket, Self::Hinge, Self::StiffSpring, Self::LimitedHinge, Self::Prismatic, Self::Ragdoll];

    pub fn of(build: u32) -> Option<Self> {
        match build {
            BALL_SOCKET_BUILD => Some(Self::BallSocket),
            HINGE_BUILD => Some(Self::Hinge),
            STIFF_SPRING_BUILD => Some(Self::StiffSpring),
            LIMITED_HINGE_BUILD => Some(Self::LimitedHinge),
            PRISMATIC_BUILD => Some(Self::Prismatic),
            RAGDOLL_BUILD => Some(Self::Ragdoll),
            _ => None,
        }
    }

    pub fn build_address(self) -> u32 {
        match self {
            Self::BallSocket => BALL_SOCKET_BUILD,
            Self::Hinge => HINGE_BUILD,
            Self::StiffSpring => STIFF_SPRING_BUILD,
            Self::LimitedHinge => LIMITED_HINGE_BUILD,
            Self::Prismatic => PRISMATIC_BUILD,
            Self::Ragdoll => RAGDOLL_BUILD,
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
struct Build<'s, 'j> {
    sse: Sse,
    x: Fx,
    one: u32,
    softness: u32,
    sign: u128,
    a: Side,
    b: Side,
    /// `pairs+0x0c`, the right-hand-side scale.
    rhs_scale: u32,
    /// `pairs+0x08`, the friction rows' scale.
    friction_scale: u32,
    half: u32,
    j: &'s Journal<'j>,
    s: &'s mut Scratch,
    cursor: Cursor,
}

impl Build<'_, '_> {
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

    fn schemas(&mut self, words: &[u32]) -> Result<(), Fallback> {
        for w in words {
            self.schema(*w)?;
        }
        Ok(())
    }

    /// A linear row's angular parts and effective mass along `axis`, the
    /// lever arms from the bodies' centres to `pa` and `pb`, each crossed
    /// with the axis into its body's frame (`0x0031ec30`, `0x0031ea90`,
    /// `0x0031ee10`, `0x0031f160`, `0x0031f3e0`): `(A, B, 1 / mass, mass)`.
    fn lever(&mut self, pa: u128, pb: u128, axis: u128) -> Result<(u128, u128, u32, u32), Fallback> {
        let (a, b) = (self.a, self.b);
        let s = &mut self.sse;
        let ra = s.sub(pa, a.center);
        let rb = s.sub(pb, b.center);
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
        Ok((ang_a, ang_b, inverse, self.x.st(mass)))
    }

    fn put_linear(&mut self, axis: u128, rhs: u32, lever: (u128, u128, u32, u32)) -> Result<u32, Fallback> {
        let (ang_a, ang_b, inverse, mass) = lever;
        let at = self.cursor.rows;
        self.put_vectors(&[with_w(axis, rhs), with_w(ang_a, inverse), with_w(ang_b, mass)])?;
        self.cursor.rows = at.wrapping_add(LINEAR_ROW);
        Ok(at)
    }

    /// `(wb - wa) . axis` times the scale, the linear rows' right-hand side.
    fn separation(&mut self, wa: u128, wb: u128, axis: u128) -> Result<u32, Fallback> {
        let s = &mut self.sse;
        let d = s.sub(wb, wa);
        let e = s.mul(d, axis);
        let e = s.hsum(e);
        Ok(self.x.st(self.x.mul(self.ld(e)?, self.ld(self.rhs_scale)?)))
    }

    /// One free linear row along `axis` between the world points `wa` and
    /// `wb` (`0x0031ec30`, and each pass of `0x0031f160` and `0x0031f3e0`).
    /// Returns the row's address.
    fn linear(&mut self, wa: u128, wb: u128, axis: u128) -> Result<u32, Fallback> {
        let lever = self.lever(wa, wb, axis)?;
        let rhs = self.separation(wa, wb, axis)?;
        let at = self.put_linear(axis, rhs, lever)?;
        self.schema(SCHEMA_FREE_LINEAR)?;
        Ok(at)
    }

    /// `0x0031ee10`: the free row with the range `[high, low]` times the
    /// negated scale.
    fn range_linear(&mut self, wa: u128, wb: u128, axis: u128, high: u32, low: u32) -> Result<(), Fallback> {
        let lever = self.lever(wa, wb, axis)?;
        let rhs = self.separation(wa, wb, axis)?;
        self.put_linear(axis, rhs, lever)?;
        let n = self.ld(self.rhs_scale)?.neg();
        let x = &self.x;
        let (high, low) = (x.st(x.mul(n, self.ld(high)?)), x.st(x.mul(n, self.ld(low)?)));
        self.schemas(&[SCHEMA_RANGE_LINEAR, high, low])
    }

    /// `0x0031ea90`: a capped linear row through the one point `p`, its
    /// right-hand side `target` times the scale, its cap `limit` times the
    /// friction scale.
    fn capped_linear(&mut self, p: u128, axis: u128, limit: u32, target: u32) -> Result<(), Fallback> {
        let lever = self.lever(p, p, axis)?;
        let x = &self.x;
        let rhs = x.st(x.mul(self.ld(target)?, self.ld(self.rhs_scale)?));
        let cap = x.st(x.mul(self.ld(limit)?, self.ld(self.friction_scale)?));
        self.put_linear(axis, rhs, lever)?;
        self.schemas(&[SCHEMA_CAPPED_LINEAR, cap])
    }

    /// The three world-axis rows holding `wa` and `wb` together
    /// (`0x0031f160`, `0x0031f3e0`).
    fn points(&mut self, axes: &[u128; 3], wa: u128, wb: u128) -> Result<(), Fallback> {
        for axis in axes {
            self.linear(wa, wb, *axis)?;
        }
        Ok(())
    }

    /// An angular row's parts about `axis`: the axis into A's frame, its
    /// negation into B's, the inverse effective mass with the softness
    /// (`0x0031e7b0`, `0x0031e900`, `0x0031f000`).
    fn angular_parts(&mut self, axis: u128) -> Result<(u128, u128, u32), Fallback> {
        let (a, b) = (self.a, self.b);
        let s = &mut self.sse;
        let ang_a = a.local(s, axis);
        let ang_b = b.local(s, axis ^ self.sign);
        let q = self.sse.quad(&a, &b, ang_a, ang_b);
        let x = &self.x;
        let inverse = x.st(x.div(self.ld(self.one)?, x.add(self.ld(q)?, self.ld(self.softness)?)));
        Ok((ang_a, ang_b, inverse))
    }

    fn put_angular(&mut self, ang_a: u128, ang_b: u128) -> Result<(), Fallback> {
        let at = self.cursor.rows;
        self.put_vectors(&[ang_a, ang_b])?;
        self.cursor.rows = at.wrapping_add(ANGULAR_ROW);
        Ok(())
    }

    /// One free angular row about `axis` (`0x0031e7b0`), `-(v . u)` times
    /// the scale as the right-hand side.
    fn angular(&mut self, u: u128, v: u128, axis: u128) -> Result<(), Fallback> {
        let (ang_a, ang_b, inverse) = self.angular_parts(axis)?;
        let x = &self.x;
        let l = |i: usize| -> Result<F80, Fallback> { Ok(x.mul(self.ld(lane(v, i))?, self.ld(lane(u, i))?)) };
        let dot = x.add(x.add(l(2)?, l(1)?), l(0)?);
        let rhs = x.st(x.mul(dot, self.ld(self.rhs_scale)?).neg());
        self.put_angular(with_w(ang_a, inverse), with_w(ang_b, rhs))?;
        self.schema(SCHEMA_FREE_ANGULAR)
    }

    /// `0x0031f000`: an angular range row about `axis`, the bounds `high`
    /// and `low` and the right-hand side `value` each times the negated
    /// scale, the inverse effective mass halved, angular scale `0.5`.
    fn range_angular(&mut self, axis: u128, high: u32, low: u32, value: u32) -> Result<(), Fallback> {
        let (ang_a, ang_b, inverse) = self.angular_parts(axis)?;
        let n = self.ld(self.rhs_scale)?.neg();
        let x = &self.x;
        let high = x.st(x.mul(n, self.ld(high)?));
        let low = x.st(x.mul(n, self.ld(low)?));
        let rhs = x.st(x.mul(n, self.ld(value)?));
        let inverse = x.st(x.mul(self.ld(inverse)?, self.ld(self.half)?));
        self.put_angular(with_w(ang_a, inverse), with_w(ang_b, rhs))?;
        self.schemas(&[SCHEMA_RANGE_ANGULAR, high, low, HALF_F32])
    }

    /// One pass of `0x0031e900`: a capped angular row about `axis`, its cap
    /// the friction scale times `limit`, its right-hand side `target`
    /// times the scale.
    fn capped_angular(&mut self, axis: u128, limit: u32, target: u32) -> Result<(), Fallback> {
        let (ang_a, ang_b, inverse) = self.angular_parts(axis)?;
        let x = &self.x;
        let cap = x.st(x.mul(self.ld(self.friction_scale)?, self.ld(limit)?));
        let rhs = x.st(x.mul(self.ld(target)?, self.ld(self.rhs_scale)?));
        self.put_angular(with_w(ang_a, inverse), with_w(ang_b, rhs))?;
        self.schemas(&[SCHEMA_CAPPED_ANGULAR, cap])
    }

    fn word(&self, at: u32) -> Result<u32, Fallback> {
        self.j.get(at).ok_or(Fallback::Memory)
    }

    fn constant(&self, at: u32) -> Result<F80, Fallback> {
        self.ld(self.word(at)?)
    }

    /// `0x003725e0`: the angle of `(x, y)` by a cubic in the smaller over
    /// the larger magnitude, folded into the quadrant by `x` and `y`'s
    /// signs. Working precision; the caller stores it.
    fn atan(&self, y: u32, x_bits: u32) -> Result<F80, Fallback> {
        use x87::Cmp;
        let x = &self.x;
        let (ly, lx) = (self.ld(y)?, self.ld(x_bits)?);
        let (ay, ax) = (ly.abs(), lx.abs());
        let guard = self.constant(constant::ATAN_GUARD)?;
        let wide = !matches!(x87::compare(ay, ax, false).0, Cmp::Less | Cmp::Equal);
        let t = if wide { x.div(ax, x.add(ay, guard)) } else { x.div(ay, x.add(ax, guard)) };
        let t2 = x.mul(t, t);
        let first = x.sub(t, x.mul(self.constant(constant::ATAN_SQUARE)?, t2));
        let third = x.mul(x.mul(t2, t), self.constant(constant::ATAN_CUBE)?);
        let mut r = x.sub(first, third);
        if wide {
            r = x.sub(self.constant(constant::HALF_PI)?, r);
        }
        let zero = self.constant(constant::ZERO)?;
        if x87::compare(lx, zero, false).0 == Cmp::Less {
            r = x.sub(self.constant(constant::PI)?, r);
        }
        if x87::compare(ly, zero, false).0 == Cmp::Less {
            r = r.neg();
        }
        Ok(r)
    }

    /// `v` normalised by one Newton step on `rsqrtss`, as the stiff spring
    /// and the ragdoll's twist do: `(k v, |v|^2 k)`.
    fn normalise(&mut self, v: u128) -> (u128, u32) {
        let e = &mut self.sse;
        let sq = e.mul(v, v);
        let n = e.hsum(sq);
        let r = e.e.rsqrt(n);
        let t = e.e.mul(n, r);
        let t = e.e.mul(t, r);
        let u = e.e.sub(3.0f32.to_bits(), t);
        let h = e.e.mul(0.5f32.to_bits(), r);
        let k = e.e.mul(h, u);
        let length = e.e.mul(n, k);
        (e.mul(splat(k), v), length)
    }

    fn cross(&mut self, u: u128, v: u128) -> u128 {
        let s = &mut self.sse;
        let right = s.mul(shuf(u, 0xd2), shuf(v, 0xc9));
        let left = s.mul(shuf(u, 0xc9), shuf(v, 0xd2));
        s.sub(left, right)
    }

    fn dot(&mut self, u: u128, v: u128) -> u32 {
        let p = self.sse.mul(u, v);
        self.sse.hsum(p)
    }

    /// One cone of `0x00312e00`: when `twist x other` is long enough, a
    /// range row about it normalised, bounded by `high` and `low`, its
    /// value `twist . other`.
    fn cone(&mut self, twist: u128, other: u128, high: u32, low: u32) -> Result<(), Fallback> {
        let c = self.cross(twist, other);
        let n = self.dot(c, c);
        let floor = self.constant(constant::CONE_FLOOR)?;
        if x87::compare(self.ld(n)?, floor, false).0 == x87::Cmp::Less {
            return Ok(());
        }
        let root = x87::sqrt(self.ld(n)?, self.x.mode).0;
        let d = self.dot(twist, other);
        let inverse = self.x.st(self.x.div(self.ld(self.one)?, root));
        let axis = self.sse.mul(splat(inverse), c);
        self.range_angular(axis, high, low, d)
    }
}

const HALF_F32: u32 = 0x3f00_0000;
/// The first cone's low bound (`0x00312ec0`).
const CONE_LOW: u32 = 0x42c8_0000;

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
        friction_scale: word(pairs.wrapping_add(8))?,
        half: word(constant::HALF)?,
        j: &*j,
        s,
        cursor: *cursor,
    };
    let axes = [ax, ay, az];
    let mut wrapped = None;
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
        Joint::LimitedHinge => {
            b.header(acc, c.wrapping_add(0x90))?;
            let pivot_a = b.rotate(&ta, local(0x20)?);
            let axis_a = b.rotate(&ta, local(0x30)?);
            let perp_a = b.rotate(&ta, local(0x40)?);
            let base_a = b.rotate(&ta, local(0x50)?);
            let pivot_a = b.sse.add(pivot_a, ta.translation);
            let pivot_b = b.rotate(&tb, local(0x60)?);
            let axis_b = b.rotate(&tb, local(0x70)?);
            let base_b = b.rotate(&tb, local(0x80)?);
            let pivot_b = b.sse.add(pivot_b, tb.translation);
            b.angular(base_a, axis_b, perp_a)?;
            let flipped = axis_b ^ b.sign;
            b.angular(perp_a, flipped, base_a)?;
            // The angle of B's base about the axis, kept within a turn of
            // the last one (`c+0xd4`).
            let cos = b.dot(base_b, perp_a);
            let normal = b.cross(base_b, axis_b);
            let sin = b.dot(normal, perp_a);
            let angle = b.atan(cos, sin)?;
            let last = b.ld(word(c.wrapping_add(0xd4))?)?;
            let wrap = b.constant(constant::WRAP)?;
            let turn = b.constant(constant::TURN)?;
            let x = &b.x;
            let angle = if crate::jacobian::above(x.sub(last, angle), wrap) {
                x.add(angle, turn)
            } else if crate::jacobian::above(x.sub(angle, last), wrap) {
                x.sub(angle, turn)
            } else {
                angle
            };
            let angle = x.st(angle);
            wrapped = Some(angle);
            b.range_angular(axis_a, word(c.wrapping_add(0xc8))?, word(c.wrapping_add(0xcc))?, angle)?;
            b.points(&axes, pivot_a, pivot_b)?;
            let friction = word(c.wrapping_add(0xd0))?;
            if x87::compare(b.ld(friction)?, b.constant(constant::ZERO)?, true).0 != x87::Cmp::Equal {
                b.capped_angular(axis_a, friction, word(c.wrapping_add(0xc4))?)?;
            }
        }
        Joint::Prismatic => {
            b.header(acc, c.wrapping_add(0xac))?;
            let w = [0x70, 0x80, 0x90].map(&local);
            let [Ok(w0), Ok(w1), Ok(w2)] = w else { return Err(Memory) };
            let w = [w0, w1, w2].map(|v| b.rotate(&ta, v));
            b.angular(w[0], tb.basis[1], w[2])?;
            b.angular(w[1], tb.basis[2], w[0])?;
            b.angular(w[2], tb.basis[0], w[1])?;
            let pa = b.point(&ta, local(0x20)?);
            let pb = b.point(&tb, local(0x30)?);
            let slide = b.rotate(&tb, local(0x50)?);
            // B's pivot moved along the slide to A's: the free rows hold
            // that point to A's across the slide.
            let d = b.sse.sub(pa, pb);
            let e = b.dot(d, slide);
            let along = b.sse.mul(splat(e), slide);
            let q = b.sse.add(pb, along);
            let across = b.rotate(&tb, local(0x60)?);
            b.linear(pa, q, across)?;
            let third = b.cross(slide, across);
            b.linear(pa, q, third)?;
            b.range_linear(pa, pb, slide, word(c.wrapping_add(0xa0))?, word(c.wrapping_add(0xa4))?)?;
            let friction = word(c.wrapping_add(0xa8))?;
            if crate::jacobian::above(b.ld(friction)?, b.constant(constant::ZERO)?) {
                b.capped_linear(pa, slide, friction, word(c.wrapping_add(0xe0))?)?;
            }
        }
        Joint::Ragdoll => {
            b.header(acc, c.wrapping_add(0x98))?;
            let pivot_a = b.rotate(&ta, local(0x20)?);
            let plane_a = b.rotate(&ta, local(0x30)?);
            let twist_a = b.rotate(&ta, local(0x40)?);
            let pivot_a = b.sse.add(pivot_a, ta.translation);
            let pivot_b = b.rotate(&tb, local(0x50)?);
            let plane_b = b.rotate(&tb, local(0x60)?);
            let twist_b = b.rotate(&tb, local(0x70)?);
            let pivot_b = b.sse.add(pivot_b, tb.translation);
            b.cone(twist_a, twist_b, word(c.wrapping_add(0x80))?, CONE_LOW)?;
            b.cone(twist_a, plane_b, word(c.wrapping_add(0x84))?, word(c.wrapping_add(0x88))?)?;
            // The twist about the mean of the two twist axes.
            let mean = b.sse.add(twist_b, twist_a);
            let (axis, _) = b.normalise(mean);
            let p = b.cross(axis, plane_b);
            let q = b.cross(p, axis);
            let y = b.dot(p, plane_a);
            let x_bits = b.dot(q, plane_a);
            let angle = b.atan(y, x_bits)?;
            let angle = b.x.st(angle);
            b.range_angular(axis, word(c.wrapping_add(0x8c))?, word(c.wrapping_add(0x90))?, angle)?;
            b.points(&axes, pivot_a, pivot_b)?;
            let friction = word(c.wrapping_add(0x94))?;
            if crate::jacobian::above(b.ld(friction)?, b.constant(constant::ZERO)?) {
                for (k, axis) in ta.basis.into_iter().enumerate() {
                    let target = word(c.wrapping_add(0xcc + 8 * k as u32))?;
                    b.capped_angular(axis, friction, target)?;
                }
            }
        }
    }
    if b.sse.e.trapped {
        return Err(Float);
    }
    *cursor = b.cursor;
    if let Some(angle) = wrapped {
        j.put(c.wrapping_add(0xd4), angle).ok_or(Memory)?;
    }
    Ok(())
}
