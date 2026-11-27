//! Motion data on the two sides of an island solve. The accumulator's
//! last three vectors hold the transposed body basis while building the
//! Jacobians. During the solve that same space holds iteration sums.
use crate::fp::{Arith, Exact};
use crate::island_solve::Fallback;
use crate::mem::Journal;
use crate::motion::{self, ForceInput, Pose, PoseConstants, PoseInput, Vector};
use crate::solver::{finite, load};
use sim_core::det::{self, Chain, Checked};
use x87::{Cmp, F80, Mode};

type Basis = [Vector; 3];

pub struct Accumulator {
    pub linear: Vector,
    pub angular: Vector,
    pub inverse_mass_inertia: Vector,
    pub center: Vector,
    pub world_to_body: Basis,
}

/// Force integration precedes the snapshot into solver space. Both
/// dynamic motion types use this builder.
pub fn build(
    force: ForceInput,
    center: Vector,
    one: f32,
    zero: f32,
    control: u16,
) -> Option<(motion::Velocities, Accumulator)> {
    let velocities = motion::apply_forces(force, one, zero, control)?;
    let world_to_body =
        std::array::from_fn(|i| [force.basis[0][i], force.basis[1][i], force.basis[2][i], 0.0]);
    let angular = det::checked(|fp| Some(motion::rotate(fp, &world_to_body, velocities.angular)))?;
    let accumulator = Accumulator {
        linear: velocities.linear,
        angular,
        inverse_mass_inertia: force.inverse_inertia,
        center,
        world_to_body,
    };
    Some((velocities, accumulator))
}

/// A constraint class with a ported `vt[+0x24]` (docs/engine/havok-step.md,
/// "Constraints"), told apart by that slot's target: the contact class
/// and the joint classes [`crate::joint`] covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// `0x0031bdf0`, which calls [`contact`] (`0x0031f620`).
    Contact,
    /// A joint class ([`crate::joint`]).
    Joint(crate::joint::Joint),
}

/// The contact class's `vt[+0x24]`.
pub const CONTACT_BUILD: u32 = 0x0031_bdf0;
/// The contact constraint's points, their count, its point properties and
/// its friction block (`0x0031bdf0`).
pub const CONTACT_POINTS: u32 = 0x34;
pub const CONTACT_COUNT: u32 = 0x38;
pub const CONTACT_FRICTION: u32 = 0x50;
pub const CONTACT_PROPERTIES: u32 = 0x90;

impl Class {
    pub fn of(build: u32) -> Option<Self> {
        match build {
            CONTACT_BUILD => Some(Self::Contact),
            _ => crate::joint::Joint::of(build).map(Self::Joint),
        }
    }

    /// Append this constraint's rows and schemas at the cursor.
    pub fn build(
        self,
        j: &mut Journal,
        s: &mut Scratch,
        fpu: Fpu,
        constraint: u32,
        pairs: u32,
        cursor: &mut Cursor,
    ) -> Result<(), Fallback> {
        match self {
            Self::Contact => {
                let at = |off: u32| j.get(constraint.wrapping_add(off)).ok_or(Fallback::Memory);
                let args = ContactArgs {
                    points: at(CONTACT_POINTS)?,
                    count: at(CONTACT_COUNT)?,
                    properties: at(CONTACT_PROPERTIES)?,
                    friction: constraint.wrapping_add(CONTACT_FRICTION),
                    pairs,
                };
                contact(j, s, fpu, &args, cursor)
            }
            Self::Joint(joint) => crate::joint::build(j, s, fpu, joint, constraint, pairs, cursor),
        }
    }
}

/// The guest's float state for a solve: the MXCSR and the x87 control
/// word. Retail's step runs with every exception masked in both; anything
/// else declines ([`Fpu::masked`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fpu {
    pub mxcsr: u32,
    pub control: u16,
}

impl Fpu {
    pub fn masked(self) -> bool {
        self.mxcsr & 0x1f80 == 0x1f80 && self.control & 0x3f == 0x3f
    }
}

/// The solver buffer while one island solves (accumulators, rows,
/// impulses, schemas), addressed as the guest's but owned by Rust: the
/// guest's copy is scratch and is never written. An island that spills
/// into a LIFO block covers two ranges, the buffer's and the block's.
#[derive(Clone, Debug)]
pub struct Scratch {
    segments: Vec<(u32, Vec<u32>)>,
}

impl Scratch {
    /// Zeroed words covering `base..end`.
    pub fn new(base: u32, end: u32) -> Option<Self> {
        Self::of(&[(base, end)])
    }

    /// Zeroed words covering each `base..end`, 16-byte aligned bases,
    /// ranges apart.
    pub fn of(ranges: &[(u32, u32)]) -> Option<Self> {
        let mut total = 0u32;
        let mut segments = Vec::with_capacity(ranges.len());
        for (i, &(base, end)) in ranges.iter().enumerate() {
            let bytes = end.checked_sub(base)?;
            total = total.checked_add(bytes)?;
            if base & 15 != 0 || total > MAX_SCRATCH {
                return None;
            }
            if ranges[..i].iter().any(|&(b, e)| base < e && b < end) {
                return None;
            }
            segments.push((base, vec![0; bytes.div_ceil(4) as usize]));
        }
        Some(Self { segments })
    }

    fn index(&self, addr: u32) -> Option<(usize, usize)> {
        if addr & 3 != 0 {
            return None;
        }
        self.segments.iter().enumerate().find_map(|(k, (base, words))| {
            let i = (addr.checked_sub(*base)? / 4) as usize;
            (i < words.len()).then_some((k, i))
        })
    }

    pub fn get(&self, addr: u32) -> Option<u32> {
        let (k, i) = self.index(addr)?;
        Some(self.segments[k].1[i])
    }

    pub fn put(&mut self, addr: u32, value: u32) -> Option<()> {
        let (k, i) = self.index(addr)?;
        self.segments[k].1[i] = value;
        Some(())
    }

    /// A `movaps` operand: 16-byte aligned, as the guest requires.
    pub fn vector(&self, addr: u32) -> Option<u128> {
        if addr & 15 != 0 {
            return None;
        }
        (0..4).try_fold(0, |v, i| Some(v | u128::from(self.get(addr + 4 * i)?) << (32 * i)))
    }

    pub fn put_vector(&mut self, addr: u32, v: u128) -> Option<()> {
        if addr & 15 != 0 {
            return None;
        }
        (0..4).try_for_each(|i| self.put(addr + 4 * i, lane(v, i as usize)))
    }
}

/// Larger islands than this decline (the buffer itself is far smaller).
const MAX_SCRATCH: u32 = 1 << 22;

/// The Jacobian cursor `0x0031c100` passes to each `vt[+0x24]` (its
/// frame's `+0x1c..+0x2c`): the next row, the next schema, the row limit
/// and the overflow block. Before each constraint, a cursor at or past the
/// limit moves to the overflow block and the limit becomes `u32::MAX`
/// (`0x0031c36f`). The limit is the end of the row area; when the rows
/// spill into a LIFO block it is the buffer's end less `I+0x08`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub rows: u32,
    pub schemas: u32,
    pub limit: u32,
    pub overflow: u32,
}

impl Cursor {
    pub fn before_constraint(&mut self) {
        if self.rows >= self.limit {
            self.rows = self.overflow;
            self.limit = u32::MAX;
        }
    }
}

// ---- the contact builder `0x0031f620` ---------------------------------------
//
// cdecl, six arguments: the points (`0x20` each: position with the
// separation in `w`, normal), their count, the point properties (`0x1c`
// each), the friction block (`c+0x50`), the pair words and the cursor.
// It writes a header, one row per point, then two friction rows and, for
// more than one point, an angular twist row, each row with its schema.
// Every float operation below is one guest instruction, in the guest's
// operand order: x87 chains stay extended between memory stores, packed
// and scalar SSE run under the guest MXCSR, `rsqrtss` is the Pentium III
// estimate. The tail `0x00320280` is the exit with no friction.

/// Schema words the builder writes: id, size and correction pass.
pub const SCHEMA_CONTACT_HEADER: u32 = 0x0001_1801;
pub const SCHEMA_CONTACT: u32 = 0x0003_0402;
pub const SCHEMA_CONTACT_PAIR: u32 = 0x0005_0803;
pub const SCHEMA_FRICTION: u32 = 0x0005_1407;
pub const SCHEMA_FRICTION_TWIST: u32 = 0x0006_1808;
/// The header's output stride: one point property.
pub const PROPERTY_BYTES: u32 = 0x1c;
/// The friction schemas' output stride.
pub const FRICTION_STRIDE: u32 = 8;
/// More points than this decline; a manifold holds a handful.
pub const MAX_POINTS: u32 = 64;

/// Image constants the builder reads, by address.
pub mod constant {
    pub const ZERO: u32 = 0x0045_dbd8;
    pub const ONE: u32 = 0x0045_dbc0;
    /// Added to every row's effective mass.
    pub const SOFTNESS: u32 = 0x0041_559c;
    /// Scales the stored recovery `prop+0x14` before the cap `pair+0x10`.
    pub const RECOVERY: u32 = 0x0041_55a4;
    /// The penetration slack, and the friction axis length floor.
    pub const SLACK: u32 = 0x0045_dc70;
    /// Below this the position error is zero.
    pub const DEAD_ZONE: u32 = 0x0041_5068;
    /// The squared mean normal above which the first normal is the axis.
    pub const PARALLEL: u32 = 0x0045_dc78;
    /// The squared normal sum above which it is normalised.
    pub const SUM_FLOOR: u32 = 0x0045_dc68;
    /// The first normal's squared length below which the axis is `+y`.
    pub const NORMAL_FLOOR: u32 = 0x0045_dc84;
    /// The mean twist radius below which no twist row is built.
    pub const RADIUS_FLOOR: u32 = 0x0041_55a8;
    /// The four-lane sign mask.
    pub const SIGN: u32 = 0x0041_24a0;
}

/// One body's side as the builder reads its accumulator.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Side {
    /// `+0x0c`: nonzero when the basis is the identity.
    pub(crate) flag: u8,
    /// `+0x30`: inverse inertia, inverse mass in `w`.
    pub(crate) inertia: u128,
    pub(crate) center: u128,
    pub(crate) basis: [u128; 3],
}

impl Side {
    pub(crate) fn read(s: &Scratch, acc: u32) -> Option<Self> {
        Some(Self {
            flag: s.get(acc.checked_add(0x0c)?)? as u8,
            inertia: s.vector(acc + 0x30)?,
            center: s.vector(acc + 0x40)?,
            basis: [s.vector(acc + 0x50)?, s.vector(acc + 0x60)?, s.vector(acc + 0x70)?],
        })
    }

    pub(crate) fn inverse_mass(&self) -> u32 {
        lane(self.inertia, 3)
    }

    pub(crate) fn finite(&self) -> bool {
        [self.inertia, self.center, self.basis[0], self.basis[1], self.basis[2]]
            .into_iter()
            .all(|v| (0..4).all(|i| finite(lane(v, i))))
    }

    /// Into the body's frame unless the flag says it is the world's.
    pub(crate) fn local(&self, sse: &mut Sse, v: u128) -> u128 {
        if self.flag == 0 { sse.rotate(&self.basis, v) } else { v }
    }
}

pub(crate) fn lane(v: u128, i: usize) -> u32 {
    (v >> (32 * i)) as u32
}

pub(crate) fn splat(x: u32) -> u128 {
    u128::from(x) * (u128::MAX / u128::from(u32::MAX))
}

pub(crate) fn with_w(v: u128, w: u32) -> u128 {
    (v & (u128::MAX >> 32)) | u128::from(w) << 96
}

fn with_lane(v: u128, i: usize, x: u32) -> u128 {
    (v & !(u128::from(u32::MAX) << (32 * i))) | u128::from(x) << (32 * i)
}

/// `shufps x, x, imm`.
pub(crate) fn shuf(v: u128, imm: u8) -> u128 {
    (0..4).fold(0, |r, i| r | u128::from(lane(v, usize::from(imm >> (2 * i) & 3))) << (32 * i))
}

/// Packed and scalar SSE under one MXCSR.
pub(crate) struct Sse {
    pub(crate) e: Exact,
}

impl Sse {
    fn packed(&mut self, r: Result<u128, simd::Xm>) -> u128 {
        r.unwrap_or_else(|simd::Xm| {
            self.e.trapped = true;
            0
        })
    }
    pub(crate) fn mul(&mut self, a: u128, b: u128) -> u128 {
        let r = simd::mulps(&mut self.e.mxcsr, a, b);
        self.packed(r)
    }
    pub(crate) fn add(&mut self, a: u128, b: u128) -> u128 {
        let r = simd::addps(&mut self.e.mxcsr, a, b);
        self.packed(r)
    }
    pub(crate) fn sub(&mut self, a: u128, b: u128) -> u128 {
        let r = simd::subps(&mut self.e.mxcsr, a, b);
        self.packed(r)
    }

    /// `y + x` then `z + that`, the builder's horizontal sum.
    pub(crate) fn hsum(&mut self, v: u128) -> u32 {
        let xy = self.e.add(lane(v, 1), lane(v, 0));
        self.e.add(lane(v, 2), xy)
    }

    /// `x.yzx * y.zxy - x.zxy * y.yzx`, `w` included.
    fn cross(&mut self, x: u128, y: u128) -> u128 {
        let right = self.mul(shuf(x, 0xd2), shuf(y, 0xc9));
        let left = self.mul(shuf(x, 0xc9), shuf(y, 0xd2));
        self.sub(left, right)
    }

    /// `b0 x + b1 y + b2 z` over the accumulator's last three vectors.
    pub(crate) fn rotate(&mut self, basis: &[u128; 3], v: u128) -> u128 {
        let z = self.mul(basis[2], splat(lane(v, 2)));
        let y = self.mul(basis[1], splat(lane(v, 1)));
        let x = self.mul(basis[0], splat(lane(v, 0)));
        let xy = self.add(x, y);
        self.add(xy, z)
    }

    /// One Newton step on `rsqrtss` (`0x0031fa8a`, `0x0031fc67`).
    fn normalize(&mut self, v: u128) -> u128 {
        let squared = self.mul(v, v);
        let s = self.hsum(squared);
        let r = self.e.rsqrt(s);
        let sr = self.e.mul(s, r);
        let srr = self.e.mul(sr, r);
        let three = self.e.sub(3.0f32.to_bits(), srr);
        let half = self.e.mul(0.5f32.to_bits(), r);
        let k = self.e.mul(half, three);
        self.mul(splat(k), v)
    }

    /// `|a|^2 A + |b|^2 B` over the two inverse inertias.
    pub(crate) fn quad(&mut self, a: &Side, b: &Side, ang_a: u128, ang_b: u128) -> u32 {
        let bb = self.mul(ang_b, ang_b);
        let bb = self.mul(bb, b.inertia);
        let aa = self.mul(ang_a, ang_a);
        let aa = self.mul(aa, a.inertia);
        let sum = self.add(aa, bb);
        self.hsum(sum)
    }

    /// The off-diagonal term of two linear rows on one pair.
    fn coupling(&mut self, a: &Side, b: &Side, prev: &LinearRow, cur: &LinearRow) -> u32 {
        let nn = self.mul(cur.normal, prev.normal);
        let lb = self.mul(splat(b.inverse_mass()), nn);
        let la = self.mul(splat(a.inverse_mass()), nn);
        let linear = self.add(la, lb);
        let bb = self.mul(cur.ang_b, prev.ang_b);
        let bb = self.mul(bb, b.inertia);
        let aa = self.mul(cur.ang_a, prev.ang_a);
        let aa = self.mul(aa, a.inertia);
        let angular = self.add(aa, bb);
        let sum = self.add(angular, linear);
        self.hsum(sum)
    }
}

/// x87 arithmetic under the guest's control word.
pub(crate) struct Fx {
    pub(crate) mode: Mode,
}

impl Fx {
    pub(crate) fn add(&self, a: F80, b: F80) -> F80 {
        x87::add(a, b, self.mode).0
    }
    pub(crate) fn sub(&self, a: F80, b: F80) -> F80 {
        x87::sub(a, b, self.mode).0
    }
    pub(crate) fn mul(&self, a: F80, b: F80) -> F80 {
        x87::mul(a, b, self.mode).0
    }
    pub(crate) fn div(&self, a: F80, b: F80) -> F80 {
        x87::div(a, b, self.mode).0
    }
    pub(crate) fn st(&self, a: F80) -> u32 {
        a.to_f32_bits(self.mode).0
    }
}

pub(crate) fn below(a: F80, b: F80) -> bool {
    x87::compare(a, b, false).0 == Cmp::Less
}

pub(crate) fn above(a: F80, b: F80) -> bool {
    x87::compare(a, b, false).0 == Cmp::Greater
}

/// A linear row in the buffer: normal (`w` the right-hand side), the
/// angular parts for A (`w` the inverse effective mass) and B (`w` the
/// effective mass).
#[derive(Clone, Copy, Debug)]
struct LinearRow {
    normal: u128,
    ang_a: u128,
    ang_b: u128,
}

impl LinearRow {
    fn put(&self, s: &mut Scratch, at: u32) -> Option<()> {
        s.put_vector(at, self.normal)?;
        s.put_vector(at.checked_add(0x10)?, self.ang_a)?;
        s.put_vector(at + 0x20, self.ang_b)
    }

    fn get(s: &Scratch, at: u32) -> Option<Self> {
        Some(Self { normal: s.vector(at)?, ang_a: s.vector(at.checked_add(0x10)?)?, ang_b: s.vector(at + 0x20)? })
    }
}

struct Constants {
    zero: u32,
    one: u32,
    softness: u32,
    recovery: u32,
    slack: u32,
    dead_zone: u32,
    parallel: u32,
    sum_floor: u32,
    normal_floor: u32,
    radius_floor: u32,
    sign: u128,
}

impl Constants {
    fn read(j: &Journal) -> Option<Self> {
        use constant::*;
        Some(Self {
            zero: j.get(ZERO)?,
            one: j.get(ONE)?,
            softness: j.get(SOFTNESS)?,
            recovery: j.get(RECOVERY)?,
            slack: j.get(SLACK)?,
            dead_zone: j.get(DEAD_ZONE)?,
            parallel: j.get(PARALLEL)?,
            sum_floor: j.get(SUM_FLOOR)?,
            normal_floor: j.get(NORMAL_FLOOR)?,
            radius_floor: j.get(RADIUS_FLOOR)?,
            sign: vector(j, SIGN)?,
        })
    }

    fn finite(&self) -> bool {
        [
            self.zero,
            self.one,
            self.softness,
            self.recovery,
            self.slack,
            self.dead_zone,
            self.parallel,
            self.sum_floor,
            self.normal_floor,
            self.radius_floor,
        ]
        .into_iter()
        .all(finite)
    }
}

pub(crate) fn vector(j: &Journal, at: u32) -> Option<u128> {
    if at & 15 != 0 {
        return None;
    }
    (0..4).try_fold(0, |v, i| Some(v | u128::from(j.get(at.checked_add(4 * i)?)?) << (32 * i)))
}

pub(crate) fn put_vector(j: &mut Journal, at: u32, v: u128) -> Option<()> {
    (0..4).try_for_each(|i| j.put(at.checked_add(4 * i as u32)?, lane(v, i)))
}

/// The builder's six arguments less the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactArgs {
    pub points: u32,
    pub count: u32,
    pub properties: u32,
    pub friction: u32,
    pub pairs: u32,
}

/// One point and the property words the builder reads.
#[derive(Clone, Copy, Debug)]
struct Point {
    position: u128,
    normal: u128,
    impulse: u32,
    exported: u32,
    friction: u32,
    recovery: u32,
    paired: bool,
}

/// The friction block `c+0x50` (`0x0031fb38..0x0032025b`): flags (bit 2
/// asks for the twist radius), the solver's exported words from `+0x04`
/// (impulse and right-hand side, stride 8), the radius `+0x1c`, the two
/// tangent axes `+0x20` and `+0x30`.
pub const FRICTION_FLAGS: u32 = 0x00;
pub const FRICTION_OUT: u32 = 0x04;
pub const FRICTION_RADIUS: u32 = 0x1c;
pub const FRICTION_U: u32 = 0x20;
pub const FRICTION_V: u32 = 0x30;
/// Bit 2 of the flags: compute the twist radius from the points.
pub const FRICTION_MEASURE: u32 = 4;

/// `0x0031f620`. The rows and schemas go to `s` at the cursor, which
/// moves past them; the persistent words (each point's `+0x14`, the
/// friction block) go to the journal. Every float the x87 unit loads from
/// guest memory must be finite: a signalling NaN loads differently from
/// the register forms modelled here, so such a constraint declines.
pub fn contact(
    j: &mut Journal,
    s: &mut Scratch,
    fpu: Fpu,
    a: &ContactArgs,
    cursor: &mut Cursor,
) -> Result<(), Fallback> {
    use Fallback::{Constraint, Float, Memory};
    if !fpu.masked() {
        return Err(Float);
    }
    if a.count as i32 > MAX_POINTS as i32 || (a.count as i32) < 0 {
        return Err(Constraint);
    }
    let n = a.count;
    let k = Constants::read(j).ok_or(Memory)?;
    let word = |at: u32| j.get(at).ok_or(Memory);
    let acc_a = word(a.pairs)?;
    let acc_b = word(a.pairs.wrapping_add(4))?;
    let pair = |off: u32| word(a.pairs.wrapping_add(off));
    let (q_rhs, q_cap, q_pair, q_friction) = (pair(0x0c)?, pair(0x10)?, pair(0x18)?, pair(0x1c)?);
    let side_a = Side::read(s, acc_a).ok_or(Memory)?;
    let side_b = Side::read(s, acc_b).ok_or(Memory)?;
    let mut points = Vec::with_capacity(n as usize);
    for i in 0..n {
        let p = a.points.wrapping_add(0x20 * i);
        let r = a.properties.wrapping_add(PROPERTY_BYTES * i);
        points.push(Point {
            position: vector(j, p).ok_or(Memory)?,
            normal: vector(j, p.wrapping_add(0x10)).ok_or(Memory)?,
            impulse: word(r)?,
            exported: word(r.wrapping_add(4))?,
            friction: word(r.wrapping_add(8))?,
            recovery: word(r.wrapping_add(0x14))?,
            paired: word(r.wrapping_add(0x18))? & 0xffff != 0,
        });
    }
    let f = a.friction;
    let flags = word(f)?;
    let f_rhs = [word(f.wrapping_add(8))?, word(f.wrapping_add(0x10))?, word(f.wrapping_add(0x18))?];
    let radius = word(f.wrapping_add(FRICTION_RADIUS))?;
    let mut u_axis = vector(j, f.wrapping_add(FRICTION_U)).ok_or(Memory)?;
    let floats = points.iter().all(|p| {
        (0..4).all(|i| finite(lane(p.position, i)) && finite(lane(p.normal, i)))
            && [p.impulse, p.exported, p.friction, p.recovery].into_iter().all(finite)
    }) && [q_rhs, q_cap, q_pair, q_friction, radius].into_iter().all(finite)
        && f_rhs.into_iter().all(finite)
        && (0..4).all(|i| finite(lane(u_axis, i)))
        && k.finite()
        && side_a.finite()
        && side_b.finite();
    if !floats {
        return Err(Float);
    }
    // A paired point couples with the row before it; retail's first point
    // or a second pair in a row would overwrite the schema before it.
    if points.first().is_some_and(|p| p.paired) || points.windows(2).any(|w| w[0].paired && w[1].paired) {
        return Err(Constraint);
    }

    let x = Fx { mode: Mode::from_cw(fpu.control) };
    let mut sse = Sse { e: Exact::new(fpu.mxcsr) };
    let one = load(k.one);
    let zero = load(k.zero);
    // 0x0031f629: the scale 1/n, and the sums of the points' impulses
    // and friction coefficients.
    let inverse_n = x.st(x.div(one, F80::from_i64(i64::from(n as i32))));
    let mut impulses = zero;
    let mut frictions = zero;
    let header = cursor.schemas;
    let schema = |s: &mut Scratch, at: u32, v: u32| s.put(at, v).ok_or(Memory);
    schema(s, header, SCHEMA_CONTACT_HEADER)?;
    schema(s, header.wrapping_add(4), cursor.rows)?;
    schema(s, header.wrapping_add(8), a.properties)?;
    schema(s, header.wrapping_add(0x0c), acc_a)?;
    schema(s, header.wrapping_add(0x10), acc_b)?;
    schema(s, header.wrapping_add(0x14), PROPERTY_BYTES)?;
    let mut sc = header.wrapping_add(0x18);
    let mut row_at = cursor.rows;
    let mut position_sum = 0u128;
    let mut normal_sum = 0u128;
    let mut recoveries = Vec::with_capacity(n as usize);

    // Each row's effective mass: the inverse masses, the softness and the
    // angular terms, extended; the row keeps it and its inverse.
    let effective = |sse: &mut Sse, ang_a: u128, ang_b: u128, linear: bool| {
        let q = load(sse.quad(&side_a, &side_b, ang_a, ang_b));
        let t = if linear {
            let masses = x.add(load(side_a.inverse_mass()), load(side_b.inverse_mass()));
            x.add(x.add(masses, load(k.softness)), q)
        } else {
            x.add(q, load(k.softness))
        };
        (x.st(t), x.st(x.div(one, t)))
    };
    let jacobian = |sse: &mut Sse, point: u128, normal: u128| {
        let ra = sse.sub(point, side_a.center);
        let rb = sse.sub(point, side_b.center);
        let ca = sse.cross(ra, normal);
        let ang_a = side_a.local(sse, ca);
        let cb = sse.cross(normal, rb);
        let ang_b = side_b.local(sse, cb);
        (ang_a, ang_b)
    };

    for p in &points {
        // 0x0031f6b0
        let (ang_a, ang_b) = jacobian(&mut sse, p.position, p.normal);
        let (mass, inverse) = effective(&mut sse, ang_a, ang_b, true);
        frictions = x.add(frictions, load(p.friction));
        impulses = x.add(impulses, load(p.impulse));
        // 0x0031f80e: the position error, less what the last solve
        // recovered, inside the slack.
        let stored = load(p.recovery);
        let depth = x.st(x.sub(load(lane(p.position, 3)), stored));
        let excess = x.st(x.sub(load(p.exported).neg(), load(depth)));
        let cap = load(q_cap);
        let scaled = x.mul(stored, load(k.recovery));
        let h = if below(scaled, cap) { x.st(scaled) } else { x.st(cap) };
        let mut next = x.st(x.add(load(h), stored));
        let mut error = x.sub(load(depth), load(h));
        if below(x.add(x.add(load(h), load(h)), load(k.slack)), load(excess)) {
            next = x.st(x.sub(load(next), load(excess)));
            error = x.add(error, load(excess));
        }
        if below(error.abs(), load(k.dead_zone)) {
            error = zero;
        }
        recoveries.push(if below(load(next), zero) { x.st(load(next)) } else { x.st(zero) });
        position_sum = sse.add(position_sum, p.position);
        normal_sum = sse.add(normal_sum, p.normal);
        let rhs = x.st(x.mul(error, load(q_rhs)).neg());
        let row = LinearRow { normal: with_w(p.normal, rhs), ang_a: with_w(ang_a, inverse), ang_b: with_w(ang_b, mass) };
        row.put(s, row_at).ok_or(Memory)?;
        if !p.paired {
            schema(s, sc, SCHEMA_CONTACT)?;
        } else {
            // 0x0031f8fa: this point and the one before solve as a pair.
            let prev_at = row_at.wrapping_sub(0x30);
            let mut prev = LinearRow::get(s, prev_at).ok_or(Memory)?;
            let m = x.mul(load(sse.coupling(&side_a, &side_b, &prev, &row)), load(q_pair));
            schema(s, sc.wrapping_sub(4), SCHEMA_CONTACT_PAIR)?;
            let (kp, kc) = (load(lane(prev.ang_b, 3)), load(mass));
            let det = x.sub(x.mul(kp, kc), x.mul(m, m));
            let inverse_det = x.div(one, det);
            let d = load(x.st(inverse_det));
            prev.ang_b = with_w(prev.ang_b, x.st(x.mul(inverse_det, kc)));
            let cur_b = with_w(row.ang_b, x.st(x.mul(d, kp)));
            s.put_vector(prev_at + 0x20, prev.ang_b).ok_or(Memory)?;
            s.put_vector(row_at + 0x20, cur_b).ok_or(Memory)?;
            schema(s, sc, x.st(x.mul(d, m).neg()))?;
        }
        sc = sc.wrapping_add(4);
        row_at = row_at.wrapping_add(0x30);
    }
    for (i, r) in recoveries.iter().enumerate() {
        j.put(a.properties.wrapping_add(PROPERTY_BYTES * i as u32 + 0x14), *r).ok_or(Memory)?;
    }

    // 0x0031f9f6: the friction limit, the summed impulse times the mean
    // coefficient. None at or below zero: the tail `0x00320280`.
    let mut limit = x.mul(impulses, x.mul(frictions, load(inverse_n)));
    if !above(limit, zero) {
        cursor.rows = row_at;
        cursor.schemas = sc;
        return if sse.e.trapped { Err(Float) } else { Ok(()) };
    }
    // 0x0031fa0d: the friction plane's normal.
    let squared = sse.mul(normal_sum, normal_sum);
    let length = sse.hsum(squared);
    let first = points[0].normal;
    let t = if above(x.mul(x.mul(load(inverse_n), load(inverse_n)), load(length)), load(k.parallel)) {
        first
    } else if above(load(length), load(k.sum_floor)) {
        sse.normalize(normal_sum)
    } else {
        let fs = sse.mul(first, first);
        if below(load(sse.hsum(fs)), load(k.normal_floor)) {
            limit = zero;
            u128::from(1.0f32.to_bits()) << 32
        } else {
            first
        }
    };
    // 0x0031fb50: the second tangent, the first one's cross with the
    // normal; a short one takes a new first tangent from the normal.
    let mut reset = false;
    let v = loop {
        let v = sse.cross(u_axis, t);
        let vv = sse.mul(v, v);
        if above(load(sse.hsum(vv)), load(k.slack)) {
            break v;
        }
        if reset {
            // Retail would spin here forever.
            return Err(Constraint);
        }
        reset = true;
        limit = zero;
        let abs = |i: usize| load(lane(t, i)).abs();
        let (mut i, mut jj, mut kk) = (0, 1, 2);
        let mut min = abs(0);
        if below(abs(1), min) {
            min = load(x.st(abs(1)));
            jj = 0;
            i = 1;
        }
        if below(load(x.st(abs(2))), min) {
            kk = i;
            i = 2;
        }
        u_axis = with_lane(u_axis, i, 0);
        u_axis = with_lane(u_axis, jj, lane(t, kk));
        u_axis = with_lane(u_axis, kk, x.st(load(lane(t, jj)).neg()));
    };
    let v = sse.normalize(v);
    let u = sse.cross(t, v);
    put_vector(j, f.wrapping_add(FRICTION_V), v).ok_or(Memory)?;
    put_vector(j, f.wrapping_add(FRICTION_U), u).ok_or(Memory)?;
    // 0x0031fcd3: two linear rows at the mean point.
    let mean = sse.mul(splat(inverse_n), position_sum);
    let mut rows = [u, v].map(|axis| (axis, 0u128, 0u128));
    for (r, &rhs) in rows.iter_mut().zip(&f_rhs[..2]) {
        let (ang_a, ang_b) = jacobian(&mut sse, mean, r.0);
        let (mass, inverse) = effective(&mut sse, ang_a, ang_b, true);
        let rhs = x.st(x.mul(load(rhs), load(q_rhs)));
        *r = (with_w(r.0, rhs), with_w(ang_a, inverse), with_w(ang_b, mass));
    }
    let [r0, r1] = rows.map(|(normal, ang_a, ang_b)| LinearRow { normal, ang_a, ang_b });
    let m = sse.coupling(&side_a, &side_b, &r0, &r1);
    let limit = x.mul(limit, load(q_friction));
    let (k0, k1) = (load(lane(r0.ang_b, 3)), load(lane(r1.ang_b, 3)));
    let det = x.sub(x.mul(k1, k0), x.mul(load(m), load(m)));
    let inverse_det = x.div(one, det);
    let d = load(x.st(inverse_det));
    let r0 = LinearRow { ang_b: with_w(r0.ang_b, x.st(x.mul(inverse_det, k1))), ..r0 };
    let r1 = LinearRow { ang_b: with_w(r1.ang_b, x.st(x.mul(d, k0))), ..r1 };
    r0.put(s, row_at).ok_or(Memory)?;
    r1.put(s, row_at.wrapping_add(0x30)).ok_or(Memory)?;
    row_at = row_at.wrapping_add(0x60);
    schema(s, sc, SCHEMA_FRICTION)?;
    schema(s, sc.wrapping_add(4), f.wrapping_add(FRICTION_OUT))?;
    schema(s, sc.wrapping_add(8), x.st(x.mul(d, load(m)).neg()))?;
    schema(s, sc.wrapping_add(0x0c), x.st(limit))?;
    schema(s, sc.wrapping_add(0x10), FRICTION_STRIDE)?;
    let done = |cursor: &mut Cursor, rows: u32, schemas: u32, sse: &Sse| {
        cursor.rows = rows;
        cursor.schemas = schemas;
        if sse.e.trapped { Err(Float) } else { Ok(()) }
    };
    if n as i32 <= 1 {
        return done(cursor, row_at, sc.wrapping_add(0x14), &sse);
    }
    // 0x0032009a: the twist radius, the points' mean distance from their
    // mean, measured when the flag asks and kept in the block.
    let mut radius = radius;
    if flags as u8 & FRICTION_MEASURE as u8 != 0 {
        radius = 0;
        for p in &points {
            let d = sse.sub(p.position, mean);
            let dd = sse.mul(d, d);
            let r = sse.hsum(dd);
            let r = sse.e.sqrt(r);
            radius = x.st(x.add(load(r), load(radius)));
        }
        let scaled = x.mul(load(inverse_n), load(radius));
        radius = x.st(scaled);
        j.put(f.wrapping_add(FRICTION_RADIUS), radius).ok_or(Memory)?;
        if below(scaled, load(k.radius_floor)) {
            return done(cursor, row_at, sc.wrapping_add(0x14), &sse);
        }
        j.put(f, flags & !FRICTION_MEASURE).ok_or(Memory)?;
    }
    // 0x0032014f: the angular row about the normal.
    let ang_a = side_a.local(&mut sse, t);
    let negated = t ^ k.sign;
    let ang_b = side_b.local(&mut sse, negated);
    let (_, inverse) = effective(&mut sse, ang_a, ang_b, false);
    let scale = x.div(one, load(radius));
    schema(s, sc.wrapping_add(0x14), x.st(load(radius)))?;
    schema(s, sc, SCHEMA_FRICTION_TWIST)?;
    let ang_a = with_w(ang_a, x.st(x.mul(scale, load(inverse))));
    let ang_b = with_w(ang_b, x.st(x.mul(load(f_rhs[2]), load(q_rhs))));
    s.put_vector(row_at, ang_a).ok_or(Memory)?;
    s.put_vector(row_at.wrapping_add(0x10), ang_b).ok_or(Memory)?;
    done(cursor, row_at.wrapping_add(0x20), sc.wrapping_add(0x18), &sse)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Export {
    /// Integrate the velocities already in the motion.
    Keyframed,
    /// Integrate the masked iteration sums and keep those as velocity.
    Spherical,
    /// Integrate the iteration sums, then restore the solved velocity
    /// using the basis at the end of the step.
    Dynamic,
}

#[derive(Clone, Copy)]
pub struct Solved {
    pub linear_sum: Vector,
    pub angular_sum: Vector,
    pub linear: Vector,
    pub angular: Vector,
}

pub struct Writeback {
    pub pose: Pose,
    pub linear: Vector,
    pub angular: Vector,
}

fn masked(v: Vector, mask: [u32; 4]) -> Vector {
    std::array::from_fn(|i| f32::from_bits(v[i].to_bits() & mask[i]))
}

/// Two motion classes compute the translation directly from the
/// quaternion (0x2db7a0). Using the rounded basis would change the answer.
fn quaternion_offset(fp: &Checked, control: u16, q: Vector, v: Vector, one: f32) -> Option<Vector> {
    let load = |x| Chain::load(fp, control, x);
    let w = load(q[3])?;
    let square = w * w;
    let scale = (square + square - load(one)?).store();
    let q = [q[0], q[1], q[2], 0.0];
    let dot = fp.add(
        fp.mul(q[2], v[2]),
        fp.add(fp.mul(q[1], v[1]), fp.mul(q[0], v[0])),
    );
    let dot = load(dot)?;
    let dot2 = (dot + dot).store();
    let w2 = (w + w).store();
    Some(std::array::from_fn(|i| {
        let (j, k) = if i == 3 {
            (3, 3)
        } else {
            ((i + 1) % 3, (i + 2) % 3)
        };
        let cross = fp.sub(fp.mul(q[j], v[k]), fp.mul(q[k], v[j]));
        fp.add(
            fp.add(fp.mul(scale, v[i]), fp.mul(dot2, q[i])),
            fp.mul(w2, cross),
        )
    }))
}

pub fn export(
    kind: Export,
    mut input: PoseInput,
    basis: Basis,
    solved: Solved,
    scale: f32,
    constants: PoseConstants,
    control: u16,
) -> Option<Writeback> {
    if kind != Export::Keyframed {
        (input.linear, input.angular) = det::checked(|fp| {
            let angular = motion::rotate(fp, &basis, solved.angular_sum);
            Some((
                solved.linear_sum.map(|v| fp.mul(scale, v)),
                angular.map(|v| fp.mul(scale, v)),
            ))
        })?;
        if kind == Export::Spherical {
            input.linear = masked(input.linear, constants.angular_mask);
            input.angular = masked(input.angular, constants.angular_mask);
        }
    }
    let mut pose = motion::integrate_pose(input, constants, control)?;
    if kind != Export::Dynamic {
        pose.translation = det::checked(|fp| {
            let offset = quaternion_offset(
                fp,
                control,
                pose.orientation,
                input.local_center,
                constants.identity,
            )?;
            // The motion overwrites center.w with 1 after subtracting the offset.
            let mut center = pose.center;
            center[3] = fp.add(input.center[3], fp.mul(input.dt, input.linear[3]));
            Some(std::array::from_fn(|i| fp.sub(center[i], offset[i])))
        })?;
    }
    let (linear, angular) = if kind == Export::Dynamic {
        let angular = det::checked(|fp| Some(motion::rotate(fp, &pose.basis, solved.angular)))?;
        (
            masked(solved.linear, constants.angular_mask),
            masked(angular, constants.angular_mask),
        )
    } else {
        (input.linear, input.angular)
    };
    Some(Writeback {
        pose,
        linear,
        angular,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_moves_to_the_overflow_block_at_its_limit() {
        let mut c = Cursor { rows: 0x100, schemas: 0x800, limit: 0x100, overflow: 0x4000 };
        c.before_constraint();
        assert_eq!(c, Cursor { rows: 0x4000, schemas: 0x800, limit: u32::MAX, overflow: 0x4000 });
        let mut c = Cursor { rows: 0xff, schemas: 0x800, limit: 0x100, overflow: 0x4000 };
        c.before_constraint();
        assert_eq!(c.rows, 0xff);
        assert_eq!(Class::of(CONTACT_BUILD), Some(Class::Contact));
        assert_eq!(Class::of(0x0031_be20), None);
    }

    #[test]
    fn offset_preserves_the_fourth_lane_before_subtraction() {
        let result = det::checked(|fp| {
            quaternion_offset(fp, 0x027f, [0.5, 0.5, 0.5, 0.5], [1.0, 2.0, 3.0, 4.0], 1.0)
        })
        .unwrap();
        assert_eq!(result, [3.0, 1.0, 2.0, -2.0]);
    }
}
