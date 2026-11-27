//! The solver-side motion virtuals (docs/engine/havok-step.md, "Motions"
//! and "The island solve"): `vt[+0x0c]` integrates the pending forces and
//! fills the body's accumulator, `vt[+0x10]` takes the solved velocities
//! back and integrates the pose. Both are `thiscall` on the motion with
//! three stack arguments (`ret 0xc`) and return the next accumulator.
//!
//! Every float operation is one guest instruction in the guest's operand
//! order: packed and scalar SSE under the guest MXCSR ([`Sse`]), x87
//! chains in working precision under the control word ([`Fx`]), `rsqrtss`
//! the architectural estimate. A non-finite float the x87 code would load
//! from memory declines ([`Fallback::Float`]), as in the contact builder.
//! Motion writes go to the [`Journal`]; the accumulator is returned.
use crate::fp::{Arith, Exact};
use crate::island_solve::Fallback;
use crate::jacobian::{below, lane, put_vector, shuf, splat, vector, with_w, Fpu, Fx, Sse};
use crate::mem::Journal;
use crate::motion::{Accumulator, SolverClass};
use crate::solver::{finite, load};
use x87::{F80, Mode};

/// Image constants the virtuals read, by address.
pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    pub const ZERO: u32 = 0x0045_dbd8;
    /// The Newton step's `3` and `0.5` in the quaternion normalisation
    /// `0x001d8d60`.
    pub const NEWTON_BIAS: u32 = 0x0045_dbd4;
    pub const NEWTON_SCALE: u32 = 0x0045_dbbc;
    /// The four-lane mask applied to velocities (`w` cleared).
    pub const VELOCITY_MASK: u32 = 0x0041_5120;
}

/// Motion fields, by byte offset. The dynamic types share the pose block
/// `+0x0c..+0xc0`; the force and saved-velocity slots differ between the
/// sphere (2, 3) and box (4, 5) layouts.
pub mod field {
    /// Scales `dt |angular|` into the previous centre's `w`.
    pub const SWEEP: u32 = 0x0c;
    /// Sphere types: the scalar inverse inertia.
    pub const INVERSE_INERTIA: u32 = 0x10;
    pub const LINEAR_DAMPING: u32 = 0x14;
    pub const ANGULAR_DAMPING: u32 = 0x18;
    pub const LOCAL_CENTER: u32 = 0x20;
    pub const INVERSE_MASS: u32 = 0x2c;
    pub const ORIENTATION: u32 = 0x30;
    pub const LINEAR: u32 = 0x40;
    pub const ANGULAR: u32 = 0x50;
    /// The previous centre, its `w` the swept radius.
    pub const PREVIOUS_CENTER: u32 = 0x60;
    pub const CENTER: u32 = 0x70;
    /// Three columns, `+0x80`, `+0x90`, `+0xa0`.
    pub const BASIS: u32 = 0x80;
    pub const TRANSLATION: u32 = 0xb0;
    /// Sphere types: force and torque; box types: the saved velocities.
    pub const SLOT_C0: u32 = 0xc0;
    pub const SLOT_D0: u32 = 0xd0;
    /// Sphere types: the saved velocities; box types: the inverse inertia
    /// (inverse mass in `w`) and the force.
    pub const SLOT_E0: u32 = 0xe0;
    pub const SLOT_F0: u32 = 0xf0;
    /// Box types: the torque, its `w` nonzero while one is pending.
    pub const BOX_TORQUE: u32 = 0x100;
}

const ONE_F32: u32 = 0x3f80_0000;
const HALF_F32: u32 = 0x3f00_0000;

/// The float units and the image constants of one call.
struct Units {
    sse: Sse,
    x: Fx,
    one: u32,
    zero: u32,
    mask: u128,
    newton_bias: u32,
    newton_scale: u32,
}

impl Units {
    fn new(j: &Journal, fpu: Fpu) -> Result<Self, Fallback> {
        if !fpu.masked() {
            return Err(Fallback::Float);
        }
        let word = |at| j.get(at).ok_or(Fallback::Memory);
        Ok(Self {
            sse: Sse { e: Exact::new(fpu.mxcsr) },
            x: Fx { mode: Mode::from_cw(fpu.control) },
            one: word(constant::ONE)?,
            zero: word(constant::ZERO)?,
            mask: vector(j, constant::VELOCITY_MASK).ok_or(Fallback::Memory)?,
            newton_bias: word(constant::NEWTON_BIAS)?,
            newton_scale: word(constant::NEWTON_SCALE)?,
        })
    }

    /// An x87 load from memory; a non-finite value declines.
    fn ld(&self, bits: u32) -> Result<F80, Fallback> {
        if finite(bits) { Ok(load(bits)) } else { Err(Fallback::Float) }
    }

    fn done(&self) -> Result<(), Fallback> {
        if self.sse.e.trapped { Err(Fallback::Float) } else { Ok(()) }
    }

    /// `1 - dt * damping`, raised to zero when below it (`fcomp` against
    /// the zero constant, `test ah, 41h`), stored.
    fn damping(&self, dt: F80, damping: u32) -> Result<u32, Fallback> {
        let x = &self.x;
        let factor = x.sub(self.ld(self.one)?, x.mul(dt, self.ld(damping)?));
        let zero = self.ld(self.zero)?;
        Ok(x.st(if below(factor, zero) { zero } else { factor }))
    }

    /// `0x00319300`: the quaternion rate `omega * q`, its `w` on the x87.
    fn rate(&mut self, omega: u128, q: u128) -> Result<u128, Fallback> {
        let s = &mut self.sse;
        let c1 = s.mul(shuf(omega, 0xd2), shuf(q, 0xc9));
        let c2 = s.mul(shuf(omega, 0xc9), shuf(q, 0xd2));
        let r = s.sub(c2, c1);
        let t = s.mul(splat(lane(omega, 3)), q);
        let r = s.add(r, t);
        let t = s.mul(splat(lane(q, 3)), omega);
        let r = s.add(r, t);
        let ww = self.x.mul(self.ld(lane(omega, 3))?, self.ld(lane(q, 3))?);
        let p = self.sse.mul(omega, q);
        let d = self.sse.hsum(p);
        let w = self.x.st(self.x.sub(ww, self.ld(d)?));
        Ok(with_w(r, w))
    }

    /// `0x001d8d60`: one Newton step on `rsqrtss` of the four-lane norm.
    fn normalise(&mut self, q: u128) -> u128 {
        let s = &mut self.sse;
        let sq = s.mul(q, q);
        let a = s.add(shuf(sq, 0x4e), sq);
        let b = s.add(a, shuf(a, 0xb1));
        let n = lane(b, 0);
        let r = s.e.rsqrt(n);
        let t = s.e.mul(n, r);
        let t = s.e.mul(t, r);
        let u = s.e.sub(self.newton_bias, t);
        let k = s.e.mul(self.newton_scale, r);
        let k = s.e.mul(k, u);
        s.mul(splat(k), q)
    }

    /// `0x00372500`: the rotation's columns from a unit quaternion, on the
    /// x87, most cross terms stored before the columns are composed.
    fn basis(&self, q: u128) -> Result<[u128; 3], Fallback> {
        let x = &self.x;
        let [qx, qy, qz, qw] = [0, 1, 2, 3].map(|i| lane(q, i));
        let (lx, ly, lz, lw) = (self.ld(qx)?, self.ld(qy)?, self.ld(qz)?, self.ld(qw)?);
        let x2 = x.add(lx, lx);
        let y2 = x.add(ly, ly);
        let z2 = load(x.st(x.add(lz, lz)));
        let xx = load(x.st(x.mul(x2, lx)));
        let xy = load(x.st(x.mul(y2, lx)));
        let xz = load(x.st(x.mul(z2, lx)));
        let yy = load(x.st(x.mul(y2, ly)));
        let yz = load(x.st(x.mul(z2, ly)));
        let zz = load(x.st(x.mul(z2, lz)));
        let xw = load(x.st(x.mul(x2, lw)));
        let yw = x.mul(y2, lw);
        let zw = x.mul(z2, lw);
        let one = self.ld(self.one)?;
        let col = |a: F80, b: F80, c: F80| {
            u128::from(x.st(a)) | u128::from(x.st(b)) << 32 | u128::from(x.st(c)) << 64
        };
        Ok([
            col(x.sub(one, x.add(zz, yy)), x.add(xy, zw), x.sub(xz, yw)),
            col(x.sub(xy, zw), x.sub(one, x.add(zz, xx)), x.add(xw, yz)),
            col(x.add(yw, xz), x.sub(yz, xw), x.sub(one, x.add(yy, xx))),
        ])
    }

    /// `0x002db7a0`: `v` rotated by the quaternion directly,
    /// `(2w^2 - 1) v + 2 (q.v) q + 2w (q x v)`.
    fn rotate_by_quaternion(&mut self, q: u128, v: u128) -> Result<u128, Fallback> {
        let x = &self.x;
        let w = self.ld(lane(q, 3))?;
        let ww = x.mul(w, w);
        let scale = x.st(x.sub(x.add(ww, ww), self.ld(self.one)?));
        let w2 = x.st(x.add(w, w));
        let s = &mut self.sse;
        let out = s.mul(splat(scale), v);
        let qv = with_w(q, 0);
        let p = s.mul(qv, v);
        let d = s.hsum(p);
        let d = self.ld(d)?;
        let d2 = self.x.st(self.x.add(d, d));
        let s = &mut self.sse;
        let right = s.mul(shuf(qv, 0xd2), shuf(v, 0xc9));
        let left = s.mul(shuf(qv, 0xc9), shuf(v, 0xd2));
        let cross = s.sub(left, right);
        let cross = s.mul(splat(w2), cross);
        let along = s.mul(splat(d2), qv);
        let out = s.add(out, along);
        Ok(s.add(out, cross))
    }
}

/// A motion's words through the journal.
struct Motion<'j, 'a> {
    j: &'j mut Journal<'a>,
    at: u32,
}

impl Motion<'_, '_> {
    fn word(&self, off: u32) -> Result<u32, Fallback> {
        self.j.get(self.at.wrapping_add(off)).ok_or(Fallback::Memory)
    }
    fn vector(&self, off: u32) -> Result<u128, Fallback> {
        vector(self.j, self.at.wrapping_add(off)).ok_or(Fallback::Memory)
    }
    fn put(&mut self, off: u32, v: u32) -> Result<(), Fallback> {
        self.j.put(self.at.wrapping_add(off), v).ok_or(Fallback::Memory)
    }
    fn put_vector(&mut self, off: u32, v: u128) -> Result<(), Fallback> {
        put_vector(self.j, self.at.wrapping_add(off), v).ok_or(Fallback::Memory)
    }
    fn basis(&self) -> Result<[u128; 3], Fallback> {
        Ok([self.vector(field::BASIS)?, self.vector(field::BASIS + 0x10)?, self.vector(field::BASIS + 0x20)?])
    }
}

fn identity() -> [u128; 3] {
    let one = u128::from(ONE_F32);
    [one, one << 32, one << 64]
}

/// The basis transposed into rows (`0x002da7a0`), `w` zero.
fn transpose(b: &[u128; 3]) -> [u128; 3] {
    std::array::from_fn(|r| (0..3).fold(0, |v, c| v | u128::from(lane(b[c], r)) << (32 * c)))
}

/// `vt[+0x0c]` of `class` on the motion at `motion`: the pending force and
/// torque integrated into the velocities (types 2 to 5), then the
/// accumulator. `step` holds `dt`; `gravity` is a vector.
pub fn accumulate(
    j: &mut Journal,
    class: SolverClass,
    motion: u32,
    step: u32,
    gravity: u32,
    fpu: Fpu,
) -> Result<Accumulator, Fallback> {
    use field::*;
    use SolverClass::*;
    match class {
        Fixed => return Ok(crate::motion::fixed_accumulator()),
        Keyframed => {
            let m = Motion { j, at: motion };
            return Ok(crate::motion::keyframed_accumulator(m.vector(LINEAR)?, m.vector(ANGULAR)?, m.vector(CENTER)?));
        }
        Sphere | StabilizedSphere | Box | StabilizedBox => {}
    }
    let mut u = Units::new(j, fpu)?;
    let sphere = matches!(class, Sphere | StabilizedSphere);
    let dt_bits = j.get(step).ok_or(Fallback::Memory)?;
    let g = vector(j, gravity).ok_or(Fallback::Memory)?;
    let mut m = Motion { j, at: motion };
    let (linear, angular) = integrate_forces(&mut u, &mut m, sphere, dt_bits, g)?;
    let center = m.vector(CENTER)?;
    let acc = if sphere {
        let [r0, r1, r2] = identity();
        let inertia = with_w(splat(m.word(INVERSE_INERTIA)?), m.word(INVERSE_MASS)?);
        Accumulator { flag: Some(1), vectors: [linear, angular, inertia, center, r0, r1, r2] }
    } else {
        let rows = transpose(&m.basis()?);
        let local = u.sse.rotate(&rows, angular);
        let [r0, r1, r2] = rows;
        Accumulator { flag: Some(0), vectors: [linear, local, m.vector(SLOT_E0)?, center, r0, r1, r2] }
    };
    u.done()?;
    Ok(acc)
}

/// The pending force and torque integrated into the velocities, damped:
/// the first half of the dynamic types' `vt[+0x0c]` and the whole of their
/// `vt[+0x08]` (`0x00319cd0` for 2 and 3, `0x0031b1c0` for 4 and 5),
/// instruction for instruction. Returns the stored velocities.
fn integrate_forces(u: &mut Units, m: &mut Motion, sphere: bool, dt_bits: u32, g: u128) -> Result<(u128, u128), Fallback> {
    use field::*;
    let (force_at, torque_at) = if sphere { (SLOT_C0, SLOT_D0) } else { (SLOT_F0, BOX_TORQUE) };
    let dt = u.ld(dt_bits)?;
    let x = Fx { mode: u.x.mode };

    // Linear: force times dt / m, gravity times dt, damped.
    let weight = x.st(x.mul(dt, u.ld(m.word(INVERSE_MASS)?)?));
    let force = m.vector(force_at)?;
    let t = u.sse.mul(splat(weight), force);
    let linear = u.sse.add(m.vector(LINEAR)?, t);
    let dt_stored = x.st(dt);
    let t = u.sse.mul(splat(dt_stored), g);
    let linear = u.sse.add(linear, t);
    let damp = u.damping(dt, m.word(LINEAR_DAMPING)?)?;
    let linear = u.sse.mul(splat(damp), linear);
    m.put_vector(LINEAR, linear)?;

    // Angular: the torque, then damped.
    let mut angular = m.vector(ANGULAR)?;
    let torque = m.vector(torque_at)?;
    if sphere {
        let weight = x.st(x.mul(dt, u.ld(m.word(INVERSE_INERTIA)?)?));
        m.put_vector(force_at, 0)?;
        let t = u.sse.mul(splat(weight), torque);
        angular = u.sse.add(angular, t);
        m.put_vector(torque_at, 0)?;
    } else {
        // `fucompp` of the torque's `w` against zero; equal skips.
        let pending = u.ld(lane(torque, 3))?;
        m.put_vector(force_at, 0)?;
        if x87::compare(pending, u.ld(u.zero)?, true).0 != x87::Cmp::Equal {
            let impulse = u.sse.mul(splat(dt_bits), torque);
            let world = apply_torque(&mut u.sse, &m.basis()?, m.vector(SLOT_E0)?, impulse);
            angular = u.sse.add(angular, world);
            m.put_vector(torque_at, 0)?;
        }
    }
    let damp = u.damping(dt, m.word(ANGULAR_DAMPING)?)?;
    let angular = u.sse.mul(splat(damp), angular);
    m.put_vector(ANGULAR, angular)?;
    Ok((linear, angular))
}

/// `0x0031af90`: a world impulse into the body frame (the basis's rows,
/// `w` the third row's), times the inverse inertia, and back out.
fn apply_torque(s: &mut Sse, b: &[u128; 3], inertia: u128, t: u128) -> u128 {
    let row = |i: usize| {
        let z = lane(b[2], i);
        u128::from(lane(b[0], i)) | u128::from(lane(b[1], i)) << 32 | u128::from(z) << 64 | u128::from(z) << 96
    };
    let y = s.mul(row(1), splat(lane(t, 1)));
    let x = s.mul(row(0), splat(lane(t, 0)));
    let local = s.add(x, y);
    let z = s.mul(row(2), splat(lane(t, 2)));
    let local = s.add(local, z);
    let local = s.mul(inertia, local);
    s.rotate(b, local)
}

/// The accumulator words `vt[+0x10]` reads after the solve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Solved {
    /// `+0x10`, `+0x20`: the solved velocities (body frame for box types).
    pub linear: u128,
    pub angular: u128,
    /// `+0x40`, `+0x50`: the iteration sums.
    pub linear_sum: u128,
    pub angular_sum: u128,
}

impl Solved {
    pub fn read(word: impl Fn(u32) -> Option<u32>, acc: u32) -> Option<Self> {
        let v = |off: u32| (0..4).try_fold(0u128, |v, i| Some(v | u128::from(word(acc.checked_add(off + 4 * i)?)?) << (32 * i)));
        Some(Self { linear: v(0x10)?, angular: v(0x20)?, linear_sum: v(0x40)?, angular_sum: v(0x50)? })
    }
}

/// `vt[+0x10]` of `class`: the velocities from the solve (types 2 to 5,
/// scaled by `scale`, `[info+0x20]`), then the pose integrated over `dt`
/// (types 2 to 6). Types 2 and 4 restore the solved velocities after;
/// 3 and 5 keep the masked sums. The fixed type writes nothing.
pub fn export(
    j: &mut Journal,
    class: SolverClass,
    motion: u32,
    step: u32,
    scale: u32,
    solved: &Solved,
    fpu: Fpu,
) -> Result<(), Fallback> {
    use field::*;
    use SolverClass::*;
    if class == Fixed {
        return Ok(());
    }
    let mut u = Units::new(j, fpu)?;
    let dt_bits = j.get(step).ok_or(Fallback::Memory)?;
    let mut m = Motion { j, at: motion };
    let stabilized = matches!(class, StabilizedSphere | StabilizedBox);
    let sphere = matches!(class, Sphere | StabilizedSphere);
    if class != Keyframed {
        let mut linear = u.sse.mul(splat(scale), solved.linear_sum);
        let angular = if sphere { solved.angular_sum } else { u.sse.rotate(&m.basis()?, solved.angular_sum) };
        let mut angular = u.sse.mul(splat(scale), angular);
        if stabilized {
            linear &= u.mask;
            angular &= u.mask;
        }
        m.put_vector(LINEAR, linear)?;
        m.put_vector(ANGULAR, angular)?;
        let (saved_linear, saved_angular) = if sphere { (SLOT_E0, SLOT_F0) } else { (SLOT_C0, SLOT_D0) };
        m.put_vector(saved_linear, linear)?;
        m.put_vector(saved_angular, angular)?;
    }
    integrate(&mut u, &mut m, dt_bits, matches!(class, Sphere | Box))?;
    match class {
        Sphere => {
            m.put_vector(LINEAR, solved.linear & u.mask)?;
            m.put_vector(ANGULAR, solved.angular & u.mask)?;
        }
        Box => {
            let angular = u.sse.rotate(&m.basis()?, solved.angular);
            m.put_vector(LINEAR, solved.linear & u.mask)?;
            m.put_vector(ANGULAR, angular & u.mask)?;
        }
        _ => {}
    }
    u.done()
}

/// A motion's unconstrained virtuals, which `0x00316a20` calls for each
/// entity of an island with no constraint: `vt[+0x08]` (step, gravity;
/// `ret 8`) integrates the pending forces, `vt[+0x04]` (step; `ret 4`)
/// the pose. Types 2 and 3 share both targets, as do 4 and 5; the
/// stabilised types differ only in their solver export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeClass {
    /// Types 2 and 3: `0x00319cd0`, `0x00319f70`.
    Sphere,
    /// Types 4 and 5: `0x0031b1c0`, `0x0031b060`.
    Box,
    /// Type 6: no forces (`0x0024dc50`, `ret 8`), the pose `0x003195d0`.
    Keyframed,
    /// Type 7: `0x0024dc50` and `0x00072c70`, both a bare `ret`.
    Fixed,
}

pub const SPHERE_FORCES: u32 = 0x0031_9cd0;
pub const SPHERE_POSE: u32 = 0x0031_9f70;
pub const BOX_FORCES: u32 = 0x0031_b1c0;
pub const BOX_POSE: u32 = 0x0031_b060;
pub const NO_FORCES: u32 = 0x0024_dc50;
pub const KEYFRAMED_POSE: u32 = 0x0031_95d0;
pub const NO_POSE: u32 = 0x0007_2c70;

impl FreeClass {
    pub const ALL: [Self; 4] = [Self::Sphere, Self::Box, Self::Keyframed, Self::Fixed];

    /// The `vt[+0x08]` (forces) and `vt[+0x04]` (pose) targets.
    pub fn slots(self) -> (u32, u32) {
        match self {
            Self::Sphere => (SPHERE_FORCES, SPHERE_POSE),
            Self::Box => (BOX_FORCES, BOX_POSE),
            Self::Keyframed => (NO_FORCES, KEYFRAMED_POSE),
            Self::Fixed => (NO_FORCES, NO_POSE),
        }
    }

    pub fn of(forces: u32, pose: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.slots() == (forces, pose))
    }
}

/// `vt[+0x08]` of `class`: the dynamic types integrate the pending force
/// and torque into the velocities (the first half of their `vt[+0x0c]`);
/// types 6 and 7 write nothing.
pub fn forces(j: &mut Journal, class: FreeClass, motion: u32, step: u32, gravity: u32, fpu: Fpu) -> Result<(), Fallback> {
    let sphere = match class {
        FreeClass::Sphere => true,
        FreeClass::Box => false,
        FreeClass::Keyframed | FreeClass::Fixed => return Ok(()),
    };
    let mut u = Units::new(j, fpu)?;
    let dt_bits = j.get(step).ok_or(Fallback::Memory)?;
    let g = vector(j, gravity).ok_or(Fallback::Memory)?;
    let mut m = Motion { j, at: motion };
    integrate_forces(&mut u, &mut m, sphere, dt_bits, g)?;
    u.done()
}

/// `vt[+0x04]` of `class`: the dynamic types save the velocities (`+0xe0`,
/// `+0xf0` for 2 and 3; `+0xc0`, `+0xd0` for 4 and 5) and integrate the
/// pose, the local centre taken off through the new basis; type 6
/// integrates the pose through the quaternion; type 7 writes nothing.
pub fn pose(j: &mut Journal, class: FreeClass, motion: u32, step: u32, fpu: Fpu) -> Result<(), Fallback> {
    use field::*;
    let saved = match class {
        FreeClass::Sphere => Some((SLOT_E0, SLOT_F0)),
        FreeClass::Box => Some((SLOT_C0, SLOT_D0)),
        FreeClass::Keyframed => None,
        FreeClass::Fixed => return Ok(()),
    };
    let mut u = Units::new(j, fpu)?;
    let dt_bits = j.get(step).ok_or(Fallback::Memory)?;
    let mut m = Motion { j, at: motion };
    if let Some((linear_at, angular_at)) = saved {
        let (linear, angular) = (m.vector(LINEAR)?, m.vector(ANGULAR)?);
        m.put_vector(linear_at, linear)?;
        m.put_vector(angular_at, angular)?;
    }
    integrate(&mut u, &mut m, dt_bits, saved.is_some())?;
    u.done()
}

/// The pose step every exporting type shares: the previous centre and its
/// swept radius, the centre, the orientation (rate, Newton-normalised),
/// its basis, and the translation, the local centre taken off through the
/// new basis (`by_basis`, types 2 and 4) or the quaternion (3, 5, 6).
fn integrate(u: &mut Units, m: &mut Motion, dt_bits: u32, by_basis: bool) -> Result<(), Fallback> {
    use field::*;
    let linear = m.vector(LINEAR)?;
    let center = m.vector(CENTER)?;
    let t = u.sse.mul(splat(dt_bits), linear);
    let center_next = u.sse.add(center, t);
    m.put_vector(CENTER, center_next)?;
    let angular = m.vector(ANGULAR)?;
    let sq = u.sse.mul(angular, angular);
    let n = u.sse.hsum(sq);
    let r = u.sse.e.sqrt(n);
    let x = Fx { mode: u.x.mode };
    let swept = x.st(x.mul(x.mul(u.ld(m.word(SWEEP)?)?, u.ld(dt_bits)?), u.ld(r)?));
    m.put_vector(PREVIOUS_CENTER, with_w(center, swept))?;
    let omega = u.sse.mul(splat(HALF_F32), angular) & u.mask;
    let q = m.vector(ORIENTATION)?;
    let rate = u.rate(omega, q)?;
    let t = u.sse.mul(splat(dt_bits), rate);
    let q = u.sse.add(q, t);
    let q = u.normalise(q);
    m.put_vector(ORIENTATION, q)?;
    let basis = u.basis(q)?;
    for (i, c) in basis.iter().enumerate() {
        m.put_vector(BASIS + 0x10 * i as u32, *c)?;
    }
    let local = m.vector(LOCAL_CENTER)?;
    let offset = if by_basis { u.sse.rotate(&basis, local) } else { u.rotate_by_quaternion(q, local)? };
    let translation = u.sse.sub(center_next, offset);
    m.put_vector(TRANSLATION, translation)?;
    m.put(CENTER + 0x0c, ONE_F32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_free_classes_are_told_apart_by_both_slots() {
        for c in FreeClass::ALL {
            let (f, p) = c.slots();
            assert_eq!(FreeClass::of(f, p), Some(c));
        }
        assert_eq!(FreeClass::of(NO_FORCES, SPHERE_POSE), None);
        assert_eq!(FreeClass::of(BOX_FORCES, SPHERE_POSE), None);
    }

    #[test]
    fn the_transpose_moves_columns_to_rows_with_w_zero() {
        let b = [0x0403_0201u128 | 0x99 << 96, 0x0807_0605 | 0x99 << 96, 0x0c0b_0a09 | 0x99 << 96];
        let b = b.map(|v| (0..4).fold(0u128, |a, i| a | ((v >> (8 * i)) & 0xff) << (32 * i)));
        let rows = transpose(&b);
        assert_eq!(rows.map(|r| [0, 1, 2, 3].map(|i| lane(r, i))), [[1, 5, 9, 0], [2, 6, 10, 0], [3, 7, 11, 0]]);
    }
}
