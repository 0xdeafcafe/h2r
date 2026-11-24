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

#[derive(Default)]
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

fn load(bits: u32) -> F80 {
    let mut x = F80::from_f32_bits(bits);
    if x.is_snan() {
        x.sig |= 1 << 62;
    }
    x
}

fn finite(value: u32) -> bool {
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
