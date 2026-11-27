//! Sequential impulse solver. All indices refer to the working set of this
//! call. Packed projections round each lane before reduction. The x87 impulse
//! stays extended through accumulation, with a separate m32 store for the
//! factor applied to the velocities.
use x87::{Cmp, F80, Mode};

pub type Vector = u128;

#[derive(Clone, Copy, Debug, Default)]
pub struct Body {
    pub linear: Vector,
    pub angular: Vector,
    pub inverse_mass: Vector,
    pub linear_sum: Vector,
    pub angular_sum: Vector,
}

#[derive(Clone, Copy, Debug)]
pub struct Row {
    /// Absent for a purely angular constraint.
    pub normal: Option<Vector>,
    pub angular_a: Vector,
    pub angular_b: Vector,
    pub impulse: u32,
}

fn w(v: Vector) -> u32 {
    (v >> 96) as u32
}

impl Row {
    pub fn rhs(&self) -> u32 {
        w(self.normal.unwrap_or(self.angular_b))
    }

    fn set_rhs(&mut self, value: u32) {
        let v = self.normal.as_mut().unwrap_or(&mut self.angular_b);
        *v = (*v & (u128::MAX >> 32)) | (u128::from(value) << 96);
    }

    fn inverse(&self) -> u32 {
        w(self.angular_a)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Limit {
    Positive,
    Free,
    Capped {
        magnitude: u32,
    },
    Bounded {
        high: u32,
        low: u32,
    },
    Range {
        high: u32,
        low: u32,
        angular_scale: u32,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum Constraint {
    Single(Limit),
    PositivePair {
        cross: u32,
    },
    Friction {
        cross: u32,
        limit: u32,
        twist_scale: Option<u32>,
    },
}

impl Constraint {
    pub fn rows(self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Friction {
                twist_scale: Some(_),
                ..
            } => 3,
            _ => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub constraint: Constraint,
    pub first_row: usize,
    pub bodies: [usize; 2],
    pub rhs_scale: u32,
    pub velocity_scale: u32,
}

#[derive(Clone, Default)]
pub struct Stream {
    pub bodies: Vec<Body>,
    pub rows: Vec<Row>,
    pub commands: Vec<Command>,
}

struct Math {
    mxcsr: u32,
    mode: Mode,
}

fn splat(value: u32) -> Vector {
    u128::from(value) * (u128::MAX / u128::from(u32::MAX))
}

impl Math {
    fn project(&mut self, row: &Row, a: Body, b: Body) -> Option<u32> {
        let csr = &mut self.mxcsr;
        let wa = simd::mulps(csr, a.angular, row.angular_a).ok()?;
        let wb = simd::mulps(csr, b.angular, row.angular_b).ok()?;
        let mut sum = simd::addps(csr, wa, wb).ok()?;
        if let Some(normal) = row.normal {
            let dv = simd::subps(csr, a.linear, b.linear).ok()?;
            let linear = simd::mulps(csr, dv, normal).ok()?;
            sum = simd::addps(csr, sum, linear).ok()?;
        }
        let xy = simd::addss(csr, sum >> 32, sum as u32).ok()?;
        Some(simd::addss(csr, sum >> 64, xy as u32).ok()? as u32)
    }

    fn add(&self, a: F80, b: F80) -> F80 {
        x87::add(a, b, self.mode).0
    }
    fn sub(&self, a: F80, b: F80) -> F80 {
        x87::sub(a, b, self.mode).0
    }
    fn mul(&self, a: F80, b: F80) -> F80 {
        x87::mul(a, b, self.mode).0
    }
    fn div(&self, a: F80, b: F80) -> F80 {
        x87::div(a, b, self.mode).0
    }
    fn store(&self, a: F80) -> u32 {
        a.to_f32_bits(self.mode).0
    }
    fn spill(&self, a: F80) -> F80 {
        load(self.store(a))
    }
}

pub(crate) fn load(bits: u32) -> F80 {
    let mut x = F80::from_f32_bits(bits);
    if x.is_snan() {
        x.sig |= 1 << 62;
    }
    x
}

pub(crate) fn finite(value: u32) -> bool {
    value & 0x7f80_0000 != 0x7f80_0000
}

fn finite_vector(v: Vector) -> bool {
    (0..4).all(|lane| finite((v >> (32 * lane)) as u32))
}

fn above(a: F80, b: F80) -> bool {
    x87::compare(a, b, false).0 == Cmp::Greater
}
fn below(a: F80, b: F80) -> bool {
    x87::compare(a, b, false).0 == Cmp::Less
}

fn apply(
    m: &mut Math,
    ids: [usize; 2],
    bodies: &mut [Body],
    row: &mut Row,
    gain: F80,
) -> Option<()> {
    let [a, b] = ids;
    let factor = splat(m.store(gain));
    let csr = &mut m.mxcsr;
    let pa = simd::mulps(csr, factor, bodies[a].inverse_mass).ok()?;
    let pb = simd::mulps(csr, factor, bodies[b].inverse_mass).ok()?;
    if let Some(normal) = row.normal {
        let da = simd::mulps(csr, splat(w(pa)), normal).ok()?;
        let db = simd::mulps(csr, splat(w(pb)), normal).ok()?;
        bodies[a].linear = simd::addps(csr, bodies[a].linear, da).ok()?;
        // A and B may select the same accumulator.
        bodies[b].linear = simd::subps(csr, bodies[b].linear, db).ok()?;
    }
    let da = simd::mulps(csr, pa, row.angular_a).ok()?;
    let db = simd::mulps(csr, pb, row.angular_b).ok()?;
    bodies[a].angular = simd::addps(csr, bodies[a].angular, da).ok()?;
    bodies[b].angular = simd::addps(csr, bodies[b].angular, db).ok()?;
    row.impulse = m.store(m.add(gain, load(row.impulse)));
    Some(())
}

fn single(
    m: &mut Math,
    bodies: &mut [Body],
    row: &mut Row,
    cmd: Command,
    limit: Limit,
    bias: u32,
    zero: F80,
) -> Option<()> {
    let [a, b] = cmd.bodies;
    let dot = load(m.project(row, bodies[a], bodies[b])?);
    let rhs = load(row.rhs());
    let inv = load(row.inverse());
    let velocity = m.mul(dot, load(cmd.velocity_scale));
    let residual = |m: &Math, scale| m.sub(m.mul(rhs, load(scale)), velocity);
    let gain = match limit {
        Limit::Positive => {
            let r = residual(m, cmd.rhs_scale);
            if !above(r, zero) {
                return Some(());
            }
            m.mul(r, inv)
        }
        Limit::Free => m.mul(inv, residual(m, cmd.rhs_scale)),
        Limit::Capped { magnitude } => {
            let r = m.mul(residual(m, bias), inv);
            if above(r.abs(), load(magnitude)) {
                let scale = m.div(load(magnitude), r.abs());
                row.set_rhs(m.store(m.mul(scale, rhs)));
                m.mul(r, scale)
            } else {
                r
            }
        }
        Limit::Bounded { high, low } => {
            let r = m.mul(m.mul(m.sub(rhs, dot), inv), load(cmd.rhs_scale));
            let limit = if above(r, load(high)) {
                Some(high)
            } else if below(r, load(low)) {
                Some(low)
            } else {
                None
            };
            limit.map_or(r, |bound| m.mul(r, m.div(load(bound), r)))
        }
        Limit::Range {
            high,
            low,
            angular_scale,
        } => {
            let angular = row.normal.is_none();
            let scale = load(if angular {
                angular_scale
            } else {
                cmd.rhs_scale
            });
            let upper = m.sub(m.mul(m.sub(rhs, load(high)), scale), velocity);
            if above(upper, zero) {
                let r = if angular { m.spill(upper) } else { upper };
                let gain = m.mul(r, inv);
                apply(m, cmd.bodies, bodies, row, gain)?;
                if angular {
                    return Some(());
                }
            }
            let lower = m.sub(m.mul(m.sub(rhs, load(low)), scale), velocity);
            if !below(lower, zero) {
                return Some(());
            }
            m.mul(lower, inv)
        }
    };
    apply(m, cmd.bodies, bodies, row, gain)
}

fn coupled(
    m: &mut Math,
    bodies: &mut [Body],
    rows: &mut [Row],
    cmd: Command,
    bias: u32,
    zero: F80,
) -> Option<()> {
    let [a, b] = cmd.bodies;
    let positive = matches!(cmd.constraint, Constraint::PositivePair { .. });
    let scale = load(if positive { cmd.rhs_scale } else { bias });
    let mut residual = [F80::ZERO; 3];
    for (i, row) in rows.iter().enumerate() {
        let dot = load(m.project(row, bodies[a], bodies[b])?);
        residual[i] = m.sub(
            m.mul(load(row.rhs()), scale),
            m.mul(dot, load(cmd.velocity_scale)),
        );
    }
    let r0 = m.spill(residual[0]);
    let (cross, limit, twist) = match cmd.constraint {
        Constraint::PositivePair { cross } => (load(cross), None, None),
        Constraint::Friction {
            cross,
            limit,
            twist_scale,
        } => (load(cross), Some(load(limit)), twist_scale),
        Constraint::Single(_) => return None,
    };
    let r1 = if positive {
        residual[1]
    } else {
        m.spill(residual[1])
    };
    let mut gains = [
        m.add(m.mul(load(w(rows[0].angular_b)), r0), m.mul(r1, cross)),
        m.add(m.mul(load(w(rows[1].angular_b)), r1), m.mul(r0, cross)),
        F80::ZERO,
    ];
    // Multiplication operand order differs across the guest's unrolled paths
    // only for NaNs. Decline those pairs before mutating the working set.
    if gains[..2].iter().any(|g| g.is_nan()) {
        return None;
    }
    if let Some(limit) = limit {
        if let Some(scale) = twist {
            gains[2] = m.spill(m.mul(residual[2], load(rows[2].inverse())));
            let mut squared = m.mul(gains[2], gains[2]);
            squared = m.add(squared, m.mul(gains[1], gains[1]));
            squared = m.add(squared, m.mul(gains[0], gains[0]));
            if above(squared, m.mul(limit, limit)) {
                let factor = m.div(limit, x87::sqrt(squared, m.mode).0);
                for (row, gain) in rows.iter_mut().zip(&mut gains) {
                    *gain = m.mul(*gain, factor);
                    row.set_rhs(m.store(m.mul(factor, load(row.rhs()))));
                }
            }
            gains[2] = m.spill(m.mul(gains[2], load(scale)));
        } else {
            let limit2 = m.spill(m.mul(limit, limit));
            let squared = m.add(m.mul(gains[0], gains[0]), m.mul(gains[1], gains[1]));
            if above(squared, limit2) {
                let factor = x87::sqrt(m.div(limit2, squared), m.mode).0;
                for (row, gain) in rows.iter_mut().zip(&mut gains) {
                    row.set_rhs(m.store(m.mul(factor, load(row.rhs()))));
                    *gain = m.mul(*gain, factor);
                }
            }
        }
    } else {
        gains[0] = m.spill(gains[0]);
        gains[1] = m.spill(gains[1]);
        if !above(gains[0], zero) {
            let second = m.mul(r1, load(rows[1].inverse()));
            // test AH,41h; JP accepts greater and unordered.
            if above(second, zero) || second.is_nan() {
                return apply(m, cmd.bodies, bodies, &mut rows[1], second);
            }
        }
        if !above(gains[0], zero) || !above(gains[1], zero) {
            let first = m.mul(r0, load(rows[0].inverse()));
            return if above(first, zero) {
                apply(m, cmd.bodies, bodies, &mut rows[0], first)
            } else {
                Some(())
            };
        }
    }
    for (row, gain) in rows.iter_mut().zip(gains) {
        apply(m, cmd.bodies, bodies, row, gain)?;
    }
    Some(())
}

impl Stream {
    /// Consume a gathered stream so an unsupported operation can discard all
    /// partial results. Correction and exports use the same ordered rows.
    pub fn solve(
        mut self,
        iterations: usize,
        bias: u32,
        zero: u32,
        mxcsr: u32,
        control: u16,
    ) -> Option<Self> {
        // The NaN source preference of x87 memory operands differs from FLD
        // when an operand signals. Those streams stay with the guest.
        if !finite(bias)
            || !finite(zero)
            || self.bodies.iter().any(|b| {
                ![b.linear, b.angular, b.inverse_mass]
                    .into_iter()
                    .all(finite_vector)
            })
            || self.rows.iter().any(|r| {
                !finite(r.impulse)
                    || !finite_vector(r.angular_a)
                    || !finite_vector(r.angular_b)
                    || !r.normal.is_none_or(finite_vector)
            })
        {
            return None;
        }
        let mut math = Math {
            mxcsr,
            mode: Mode::from_cw(control),
        };
        for iteration in 0..iterations {
            for &cmd in &self.commands {
                let count = cmd.constraint.rows();
                let operands: &[u32] = match &cmd.constraint {
                    Constraint::Single(Limit::Capped { magnitude }) => {
                        std::slice::from_ref(magnitude)
                    }
                    Constraint::Single(Limit::Range {
                        high,
                        low,
                        angular_scale,
                    }) => &[*high, *low, *angular_scale],
                    Constraint::Single(Limit::Bounded { high, low }) => &[*high, *low],
                    Constraint::PositivePair { cross } => std::slice::from_ref(cross),
                    Constraint::Friction {
                        cross,
                        limit,
                        twist_scale,
                    } => &[*cross, *limit, twist_scale.unwrap_or(0)],
                    Constraint::Single(_) => &[],
                };
                if !finite(cmd.rhs_scale)
                    || !finite(cmd.velocity_scale)
                    || !operands.iter().copied().all(finite)
                {
                    return None;
                }
                let rows = self
                    .rows
                    .get_mut(cmd.first_row..cmd.first_row.checked_add(count)?)?;
                if cmd.bodies.iter().any(|&i| i >= self.bodies.len()) {
                    return None;
                }
                match cmd.constraint {
                    Constraint::Single(limit) => single(
                        &mut math,
                        &mut self.bodies,
                        &mut rows[0],
                        cmd,
                        limit,
                        bias,
                        load(zero),
                    )?,
                    _ => coupled(&mut math, &mut self.bodies, rows, cmd, bias, load(zero))?,
                }
            }
            for body in &mut self.bodies {
                if iteration == 0 {
                    body.linear_sum = body.linear;
                    body.angular_sum = body.angular;
                } else {
                    body.linear_sum =
                        simd::addps(&mut math.mxcsr, body.linear_sum, body.linear).ok()?;
                    body.angular_sum =
                        simd::addps(&mut math.mxcsr, body.angular_sum, body.angular).ok()?;
                }
            }
            for cmd in &self.commands {
                let [a, b] = cmd.bodies;
                for row in &mut self.rows[cmd.first_row..cmd.first_row + cmd.constraint.rows()] {
                    let dot = math.project(row, self.bodies[a], self.bodies[b])?;
                    row.set_rhs(math.store(math.sub(load(row.rhs()), load(dot))));
                }
            }
        }
        Some(self)
    }
}

/// Contact-property export multiplies the corrected RHS by the caller's
/// scale in x87, rounding only when it reaches the property word.
pub fn export(rhs: u32, scale: u32, control: u16) -> u32 {
    let mode = Mode::from_cw(control);
    x87::mul(load(rhs), load(scale), mode).0.to_f32_bits(mode).0
}

// ---- the schema stream `0x0031c940` reads -----------------------------------
//
// A contact island's schemas, as `jacobian::contact` writes them, decoded
// into a [`Stream`]: each header names the rows that follow and the two
// accumulators; each command its rows. The write-back pass after the
// iterations (`0x0031e4da` to `0x0031e696`) stores each row's impulse and
// its corrected right-hand side times the export scale, in schema order:
// implicit forms at the header's output pointer, stepping by its stride;
// the friction forms at their own pointer and stride.

use crate::jacobian::{
    SCHEMA_CONTACT, SCHEMA_CONTACT_HEADER, SCHEMA_CONTACT_PAIR, SCHEMA_FRICTION, SCHEMA_FRICTION_TWIST,
    Scratch,
};

/// The end schema (id 0, size 4, pass 0), after the last constraint's.
pub const SCHEMA_END: u32 = 0x0000_0400;
const ACCUMULATOR_BYTES: u32 = 0x80;

/// The solver parameters `W+0x160` the solve reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// `+0x04` and `+0x08`, every command's scales (no contact schema
    /// overrides them).
    pub rhs_scale: u32,
    pub velocity_scale: u32,
    /// `+0x0c`.
    pub bias: u32,
    /// `+0x10`: the write-back pass multiplies each right-hand side by it.
    pub export: u32,
    /// `+0x1c`, signed: none at or below zero.
    pub iterations: u32,
    /// `+0x20`, passed to each motion's export.
    pub scale: u32,
}

impl Params {
    pub fn read(read: impl Fn(u32) -> Option<u32>, at: u32) -> Option<Self> {
        let w = |off: u32| read(at.wrapping_add(off));
        Some(Self {
            rhs_scale: w(0x04)?,
            velocity_scale: w(0x08)?,
            bias: w(0x0c)?,
            export: w(0x10)?,
            iterations: w(0x1c)?,
            scale: w(0x20)?,
        })
    }
}

/// Where the write-back pass stores one command's rows: the first row's
/// impulse word, then `stride` on for each further row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    pub dest: u32,
    pub stride: u32,
}

/// A decoded stream: body `i` is the accumulator at `accumulators[i]`,
/// command `i` writes back through `outputs[i]`.
#[derive(Clone)]
pub struct Decoded {
    pub stream: Stream,
    pub accumulators: Vec<u32>,
    pub outputs: Vec<Output>,
}

/// The island's areas as the decode checks them.
#[derive(Clone, Copy, Debug)]
pub struct Areas {
    /// The leading accumulator; the island's are the `count` after it.
    pub accumulators: u32,
    pub count: u32,
    /// The row area: the first header's rows start here and every row
    /// ends at or before `rows_end`.
    pub rows: u32,
    pub rows_end: u32,
    /// How many impulse words the frame zeroes (`I+0x18`).
    pub impulse_words: u32,
}

/// Decode the schemas from `start`. `None` for a stream the kernel does
/// not cover: an id other than the contact builder's, rows that are not
/// one run from the row area's start, an accumulator outside the island,
/// or more rows than zeroed impulse words.
pub fn decode(s: &Scratch, start: u32, areas: Areas, params: Params) -> Option<Decoded> {
    let mut d = Decoded { stream: Stream::default(), accumulators: Vec::new(), outputs: Vec::new() };
    let body = |d: &mut Decoded, addr: u32| -> Option<usize> {
        let off = addr.checked_sub(areas.accumulators)?;
        if off % ACCUMULATOR_BYTES != 0 || off / ACCUMULATOR_BYTES > areas.count {
            return None;
        }
        if let Some(i) = d.accumulators.iter().position(|&a| a == addr) {
            return Some(i);
        }
        d.stream.bodies.push(Body {
            linear: s.vector(addr + 0x10)?,
            angular: s.vector(addr + 0x20)?,
            inverse_mass: s.vector(addr + 0x30)?,
            linear_sum: s.vector(addr + 0x40)?,
            angular_sum: s.vector(addr + 0x50)?,
        });
        d.accumulators.push(addr);
        Some(d.accumulators.len() - 1)
    };
    let mut p = start;
    let mut row_at = areas.rows;
    let mut bodies = [0; 2];
    let mut out = Output { dest: 0, stride: 0 };
    // The first schema is read as a header whatever its id (`0x0031c9a0`
    // is entered directly); the builder always writes one there.
    if s.get(p)? != SCHEMA_CONTACT_HEADER {
        return None;
    }
    loop {
        let tag = s.get(p)?;
        let (constraint, linear, angular, size) = match tag {
            SCHEMA_END => break,
            SCHEMA_CONTACT_HEADER => {
                if s.get(p.checked_add(4)?)? != row_at {
                    return None;
                }
                bodies = [body(&mut d, s.get(p + 0x0c)?)?, body(&mut d, s.get(p + 0x10)?)?];
                out = Output { dest: s.get(p + 8)?, stride: s.get(p + 0x14)? };
                p += 0x18;
                continue;
            }
            SCHEMA_CONTACT => (Constraint::Single(Limit::Positive), 1, 0, 4),
            SCHEMA_CONTACT_PAIR => (Constraint::PositivePair { cross: s.get(p.checked_add(4)?)? }, 2, 0, 8),
            SCHEMA_FRICTION | SCHEMA_FRICTION_TWIST => {
                let w = |off: u32| s.get(p.checked_add(off)?);
                let twist = tag == SCHEMA_FRICTION_TWIST;
                (
                    Constraint::Friction {
                        cross: w(8)?,
                        limit: w(0x0c)?,
                        twist_scale: if twist { Some(w(0x14)?) } else { None },
                    },
                    2,
                    u32::from(twist),
                    if twist { 0x18 } else { 0x14 },
                )
            }
            _ => return None,
        };
        let output = match constraint {
            Constraint::Friction { .. } => Output { dest: s.get(p + 4)?, stride: s.get(p + 0x10)? },
            _ => {
                let o = out;
                out.dest = out.dest.wrapping_add(out.stride.wrapping_mul(linear));
                o
            }
        };
        if angular == 1 {
            // The three-row write-back leaves the header's pointer zero
            // (`0x0031e631`).
            out.dest = 0;
        }
        let first_row = d.stream.rows.len();
        for i in 0..linear + angular {
            let row = if i < linear {
                let r = Row {
                    normal: Some(s.vector(row_at)?),
                    angular_a: s.vector(row_at.checked_add(0x10)?)?,
                    angular_b: s.vector(row_at + 0x20)?,
                    impulse: 0,
                };
                row_at += 0x30;
                r
            } else {
                let r = Row {
                    normal: None,
                    angular_a: s.vector(row_at)?,
                    angular_b: s.vector(row_at.checked_add(0x10)?)?,
                    impulse: 0,
                };
                row_at += 0x20;
                r
            };
            if row_at > areas.rows_end {
                return None;
            }
            d.stream.rows.push(row);
        }
        d.stream.commands.push(Command {
            constraint,
            first_row,
            bodies,
            rhs_scale: params.rhs_scale,
            velocity_scale: params.velocity_scale,
        });
        d.outputs.push(output);
        p = p.checked_add(size)?;
    }
    // Impulses start zeroed (`0x0031c3c0`) for `I+0x18` words; the rows'
    // read past that would be the guest's padding and schemas.
    (d.stream.rows.len() <= areas.impulse_words as usize).then_some(d)
}

impl Decoded {
    /// The write-back pass: `(address, word)` in store order.
    pub fn writeback(&self, solved: &Stream, export_scale: u32, control: u16) -> Vec<(u32, u32)> {
        let mut writes = Vec::new();
        for (cmd, out) in solved.commands.iter().zip(&self.outputs) {
            let mut dest = out.dest;
            for row in &solved.rows[cmd.first_row..cmd.first_row + cmd.constraint.rows()] {
                writes.push((dest, row.impulse));
                writes.push((dest.wrapping_add(4), export(row.rhs(), export_scale, control)));
                dest = dest.wrapping_add(out.stride);
            }
        }
        writes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(lanes: [f32; 4]) -> Vector {
        lanes.iter().map(|f| f.to_bits()).rev().fold(0, |p, w| p << 32 | u128::from(w))
    }

    fn lanes(v: Vector) -> [u32; 4] {
        std::array::from_fn(|i| (v >> (32 * i)) as u32)
    }

    /// Two equal bodies against one contact row: the contact's velocity
    /// error is 2, the right-hand side 4, the row's inverse 1/2.
    fn stream(rhs: f32) -> Stream {
        let body = Body {
            linear: v4([0.0; 4]),
            angular: v4([0.0; 4]),
            inverse_mass: v4([0.5, 0.5, 0.5, 0.5]),
            linear_sum: v4([0.0; 4]),
            angular_sum: v4([0.0; 4]),
        };
        Stream {
            bodies: vec![
                Body { linear: v4([1.0, 0.0, 0.0, 0.0]), ..body },
                Body { linear: v4([-1.0, 0.0, 0.0, 0.0]), ..body },
            ],
            rows: vec![Row {
                normal: Some(v4([1.0, 0.0, 0.0, rhs])),
                angular_a: v4([0.0, 0.0, 0.0, 0.5]),
                angular_b: v4([0.0; 4]),
                impulse: 0.0f32.to_bits(),
            }],
            commands: vec![Command {
                constraint: Constraint::Single(Limit::Free),
                first_row: 0,
                bodies: [0, 1],
                rhs_scale: 1.0f32.to_bits(),
                velocity_scale: 1.0f32.to_bits(),
            }],
        }
    }

    #[test]
    fn a_free_row_pushes_both_bodies_apart_and_corrects_the_rhs() {
        let solved = stream(4.0)
            .solve(1, 1.0f32.to_bits(), 0.0f32.to_bits(), 0x1f80, 0x027f)
            .unwrap();
        // The gain is (4 - 2) x 1/2 = 1; each body takes half of it
        // along the normal, in opposite directions. The packed multiply
        // also scales the normal's w (the right-hand side) into the
        // velocities' and the angular accumulator's w lanes, as retail's
        // does; the horizontal reduction never reads them.
        assert_eq!(lanes(solved.bodies[0].linear), [1.5, 0.0, 0.0, 2.0].map(f32::to_bits));
        assert_eq!(lanes(solved.bodies[1].linear), [-1.5, 0.0, 0.0, -2.0].map(f32::to_bits));
        assert_eq!(lanes(solved.bodies[0].angular), [0.0, 0.0, 0.0, 0.25].map(f32::to_bits));
        assert_eq!(lanes(solved.bodies[1].angular), [0.0; 4].map(f32::to_bits));
        assert_eq!(solved.rows[0].impulse, 1.0f32.to_bits());
        // One iteration: the sums snapshot the post-command velocities and
        // the correction subtracts the new velocity error from the rhs.
        assert_eq!(lanes(solved.bodies[0].linear_sum), lanes(solved.bodies[0].linear));
        assert_eq!(solved.rows[0].rhs(), 1.0f32.to_bits());
    }

    #[test]
    fn a_positive_row_below_the_error_applies_nothing() {
        // The velocity error (2) already exceeds the right-hand side (1):
        // the row is separating, and the impulse stays zero. Only a
        // positive-limited row skips it; a free row pulls back (-0.5).
        let mut s = stream(1.0);
        s.commands[0].constraint = Constraint::Single(Limit::Positive);
        let solved = s
            .solve(1, 1.0f32.to_bits(), 0.0f32.to_bits(), 0x1f80, 0x027f)
            .unwrap();
        assert_eq!(lanes(solved.bodies[0].linear), [1.0, 0.0, 0.0, 0.0].map(f32::to_bits));
        assert_eq!(lanes(solved.bodies[1].linear), [-1.0, 0.0, 0.0, 0.0].map(f32::to_bits));
        assert_eq!(solved.rows[0].impulse, 0.0f32.to_bits());
    }

    #[test]
    fn non_finite_operands_decline_whole_stream() {
        let mut s = stream(4.0);
        s.bodies[0].inverse_mass = v4([f32::NAN, 0.5, 0.5, 0.5]);
        assert!(s.solve(1, 1.0f32.to_bits(), 0.0f32.to_bits(), 0x1f80, 0x027f).is_none());
        let mut s = stream(4.0);
        s.rows[0].impulse = f32::INFINITY.to_bits();
        assert!(s.solve(1, 1.0f32.to_bits(), 0.0f32.to_bits(), 0x1f80, 0x027f).is_none());
        let mut s = stream(4.0);
        s.commands[0].rhs_scale = f32::NAN.to_bits();
        assert!(s.solve(1, 1.0f32.to_bits(), 0.0f32.to_bits(), 0x1f80, 0x027f).is_none());
    }

    #[test]
    fn export_rounds_only_at_the_property_word() {
        // One x87 multiply, extended, with the single binary32 store.
        let mode = Mode::from_cw(0x027f);
        let word = export(0.1f32.to_bits(), 0.7f32.to_bits(), 0x027f);
        let extended = x87::mul(load(0.1f32.to_bits()), load(0.7f32.to_bits()), mode).0;
        assert_eq!(word, extended.to_f32_bits(mode).0);
    }
}
