//! The aim net: a small network with weights written by hand, as an
//! alternative to the hand-written tracking in `mind::look`
//! (docs/game/bots.md §3.4.1). `aim_model = "net"` selects it.
//!
//! 18 inputs, one hidden layer of 24 ReLU units, 2 outputs clamped to
//! -1..1. No training and no dependency: every weight below is built from
//! the named constants, so each hidden unit has one job that can be read
//! off its row. It keeps no state and allocates nothing; the same inputs
//! give the same bits.
//!
//! Angles are yaw then pitch, in radians, with yaw positive to the left as
//! `mind::angles` gives it. The outputs are a turn request in the same
//! frame, in stick units: the hand maps `[yaw, pitch]` to the right stick
//! `(-yaw, pitch)` and moves its thumb there at the skill's speed.
//!
//! The units come in mirrored pairs, one per direction. A pair
//! `relu(x) - relu(-x)` is the linear term `x`. A pair with a negative
//! bias is a dead band, and a pair subtracted from a steeper one is a
//! clamp. Skill enters only through biases, so a better bot has a narrower
//! dead band and wider clamps.
//!
//! Per axis the network computes
//!
//! ```text
//! seen      = error + noise
//! predicted = seen + T_RATE * error_rate + T_LEAD * target_rate - W_PENDING * pending
//! tolerance = TOL_SPREAD * spread + TOL_SIZE * size + TOL_LOOSE * looseness
//! u         = max(0, |predicted| - tolerance)
//! m         = max(0, |seen| - tolerance)
//! turn      = sign(predicted) * (G_BASE * u + min(G_BOOST * u, BOOST_CAP * gain))
//!           + sign(seen) * min(K_BREAKOUT * m, BREAKOUT)
//!           + sign(target_rate) * min(K_LEAD * |target_rate|, LEAD_CAP * lead)   (yaw)
//!           + sign(stick) * W_FOLLOW * max(0, |stick| - FOLLOW_AT - FOLLOW_FADE * since + FOLLOW_SPREAD * spread)   (yaw)
//! ```
//!
//! and clamps `turn` to -1..1.

/// Network sizes.
pub const INPUTS: usize = 18;
pub const HIDDEN: usize = 24;
pub const OUTPUTS: usize = 2;

/// Where each input sits in the input vector.
pub mod input {
    /// Angle from the aim to the aim point (the led target), radians.
    pub const ERROR: usize = 0;
    /// Its rate of change, radians a second.
    pub const ERROR_RATE: usize = 2;
    /// The target's bearing rate from the eye, radians a second.
    pub const TARGET_RATE: usize = 4;
    /// The thumb now, in the turn frame, -1..1.
    pub const STICK: usize = 6;
    /// The turn already asked for that the view does not show yet, radians.
    pub const PENDING: usize = 8;
    /// Seconds since this target was acquired, 0..2.
    pub const SINCE: usize = 10;
    /// The target's apparent half width, radians, 0..0.25.
    pub const SIZE: usize = 11;
    /// How forgiving the weapon is: 0 a sniper rifle, 1 a shotgun.
    pub const LOOSENESS: usize = 12;
    /// The skill's gain, 0 at easy's and 1 at legendary's.
    pub const GAIN: usize = 13;
    /// The skill's tracking error spread, 1 at easy's.
    pub const SPREAD: usize = 14;
    /// The skill's lead fraction, 0..1.
    pub const LEAD: usize = 15;
    /// The hand's own wandering error this tick, radians.
    pub const NOISE: usize = 16;
}

/// Where each hidden unit sits. Yaw's fourteen, then pitch's ten. In each
/// pair the first unit handles a turn to the positive side.
pub mod unit {
    /// Proportional pull on the predicted error, past the tolerance.
    pub const YAW_PULL: usize = 0;
    /// Takes the skill's extra gain away again beyond its band.
    pub const YAW_BOOST_CAP: usize = 2;
    /// Steep pull on the seen error: a small correction near the target.
    pub const YAW_BREAKOUT: usize = 4;
    /// Stops that correction at a stick just past its dead zone.
    pub const YAW_BREAKOUT_CAP: usize = 6;
    /// Feeds the target's bearing rate forward.
    pub const YAW_LEAD: usize = 8;
    /// Stops the feed-forward at the rate the skill can match.
    pub const YAW_LEAD_CAP: usize = 10;
    /// Keeps a hard-over thumb going early in a flick: the overshoot.
    pub const YAW_FOLLOW: usize = 12;
    pub const PITCH_PULL: usize = 14;
    pub const PITCH_BOOST_CAP: usize = 16;
    pub const PITCH_BREAKOUT: usize = 18;
    pub const PITCH_BREAKOUT_CAP: usize = 20;
    /// Pitch's feed-forward. Targets cross slowly in pitch, so no cap.
    pub const PITCH_LEAD: usize = 22;
}

/// What each hidden unit is for, by index (two rows a pair).
pub const UNIT_ROLES: [&str; HIDDEN] = [
    "yaw pull +",
    "yaw pull -",
    "yaw boost cap +",
    "yaw boost cap -",
    "yaw breakout +",
    "yaw breakout -",
    "yaw breakout cap +",
    "yaw breakout cap -",
    "yaw lead +",
    "yaw lead -",
    "yaw lead cap +",
    "yaw lead cap -",
    "yaw follow-through +",
    "yaw follow-through -",
    "pitch pull +",
    "pitch pull -",
    "pitch boost cap +",
    "pitch boost cap -",
    "pitch breakout +",
    "pitch breakout -",
    "pitch breakout cap +",
    "pitch breakout cap -",
    "pitch lead +",
    "pitch lead -",
];

// ---- the design constants every weight is made of ----

/// Seconds of error rate added to the error: the D of the PD.
pub const T_RATE: f32 = 0.02;
/// Seconds of target motion added to the error. The view is about this old
/// by the time a stick report lands (perception plus motor delay).
pub const T_LEAD: f32 = 0.12;
/// How much of the pending turn is taken off the error. Under 1, so a game
/// that turns less than the nominal stick curve is not braked too early.
pub const W_PENDING: f32 = 0.9;
/// The dead band, radians: per unit of spread, of apparent size and of
/// looseness.
pub const TOL_SPREAD: f32 = 0.006;
pub const TOL_SIZE: f32 = 0.06;
pub const TOL_LOOSE: f32 = 0.004;
/// Stick per radian every skill has.
pub const G_BASE: f32 = 1.9;
/// More stick per radian near the target, for as far as the skill's gain
/// reaches: `BOOST_CAP` of stick at legendary's gain, none at easy's.
pub const G_BOOST: f32 = 2.0;
pub const BOOST_CAP: f32 = 0.43;
/// The micro-correction: steep, and capped just past the stick's dead
/// zone (0.13), where the view creeps.
pub const K_BREAKOUT: f32 = 8.0;
pub const BREAKOUT: f32 = 0.17;
/// Stick per radian a second of target motion, and the most of it a full
/// lead gives.
pub const K_LEAD: f32 = 0.35;
pub const LEAD_CAP: f32 = 0.6;
pub const K_LEAD_PITCH: f32 = 0.25;
/// The follow-through holds on while the thumb is past `FOLLOW_AT`, fades
/// over `1 / FOLLOW_FADE` seconds, and is wider for a wider spread.
pub const FOLLOW_AT: f32 = 0.5;
pub const FOLLOW_FADE: f32 = 1.2;
pub const FOLLOW_SPREAD: f32 = 0.3;
pub const W_FOLLOW: f32 = 0.8;

/// The skill scalars' scales: easy's and legendary's gain, easy's spread.
const GAIN_LOW: f32 = 1.6;
const GAIN_SPAN: f32 = 2.9;
const SPREAD_UNIT: f32 = 0.10;
/// Half a biped's width, world units.
const TARGET_HALF_WIDTH: f32 = 0.25;

const YAW: usize = 0;
const PITCH: usize = 1;

/// A unit's input weights, then its bias.
pub type Row = [f32; INPUTS + 1];
const BIAS: usize = INPUTS;
const NONE: Row = [0.0; INPUTS + 1];

const fn with(mut r: Row, i: usize, w: f32) -> Row {
    r[i] += w;
    r
}

const fn times(mut r: Row, k: f32) -> Row {
    let mut i = 0;
    while i < r.len() {
        r[i] *= k;
        i += 1;
    }
    r
}

/// Less the tolerance: the dead band every pull starts after.
const fn past_tolerance(r: Row) -> Row {
    let r = with(r, input::SPREAD, -TOL_SPREAD);
    let r = with(r, input::SIZE, -TOL_SIZE);
    with(r, input::LOOSENESS, -TOL_LOOSE)
}

/// `sign * predicted - tolerance`.
const fn pull(axis: usize, sign: f32) -> Row {
    let r = with(NONE, input::ERROR + axis, sign);
    let r = with(r, input::NOISE + axis, sign);
    let r = with(r, input::ERROR_RATE + axis, sign * T_RATE);
    let r = with(r, input::TARGET_RATE + axis, sign * T_LEAD);
    let r = with(r, input::PENDING + axis, -sign * W_PENDING);
    past_tolerance(r)
}

/// `G_BOOST * pull - BOOST_CAP * gain`: what the boost is over its cap.
const fn boost_cap(axis: usize, sign: f32) -> Row {
    with(times(pull(axis, sign), G_BOOST), input::GAIN, -BOOST_CAP)
}

/// `sign * seen - tolerance`.
const fn breakout(axis: usize, sign: f32) -> Row {
    let r = with(NONE, input::ERROR + axis, sign);
    past_tolerance(with(r, input::NOISE + axis, sign))
}

/// `K_BREAKOUT * breakout - BREAKOUT`.
const fn breakout_cap(axis: usize, sign: f32) -> Row {
    with(times(breakout(axis, sign), K_BREAKOUT), BIAS, -BREAKOUT)
}

/// `sign * target_rate`.
const fn lead(axis: usize, sign: f32) -> Row {
    with(NONE, input::TARGET_RATE + axis, sign)
}

/// `K_LEAD * sign * target_rate - LEAD_CAP * lead` (yaw).
const fn lead_cap(sign: f32) -> Row {
    with(times(lead(YAW, sign), K_LEAD), input::LEAD, -LEAD_CAP)
}

/// `sign * stick - FOLLOW_AT - FOLLOW_FADE * since + FOLLOW_SPREAD * spread`
/// (yaw).
const fn follow(sign: f32) -> Row {
    let r = with(NONE, input::STICK + YAW, sign);
    let r = with(r, input::SINCE, -FOLLOW_FADE);
    let r = with(r, input::SPREAD, FOLLOW_SPREAD);
    with(r, BIAS, -FOLLOW_AT)
}

/// The hidden layer: one row a unit, in [`unit`]'s order.
pub const LAYER1: [Row; HIDDEN] = [
    pull(YAW, 1.0),
    pull(YAW, -1.0),
    boost_cap(YAW, 1.0),
    boost_cap(YAW, -1.0),
    breakout(YAW, 1.0),
    breakout(YAW, -1.0),
    breakout_cap(YAW, 1.0),
    breakout_cap(YAW, -1.0),
    lead(YAW, 1.0),
    lead(YAW, -1.0),
    lead_cap(1.0),
    lead_cap(-1.0),
    follow(1.0),
    follow(-1.0),
    pull(PITCH, 1.0),
    pull(PITCH, -1.0),
    boost_cap(PITCH, 1.0),
    boost_cap(PITCH, -1.0),
    breakout(PITCH, 1.0),
    breakout(PITCH, -1.0),
    breakout_cap(PITCH, 1.0),
    breakout_cap(PITCH, -1.0),
    lead(PITCH, 1.0),
    lead(PITCH, -1.0),
];

/// An output's weight on each hidden unit, then its bias.
pub type Out = [f32; HIDDEN + 1];

/// `w` on a pair's positive unit and `-w` on its negative one.
const fn pair(mut o: Out, first: usize, w: f32) -> Out {
    o[first] = w;
    o[first + 1] = -w;
    o
}

const fn yaw_out() -> Out {
    let o = pair([0.0; HIDDEN + 1], unit::YAW_PULL, G_BASE + G_BOOST);
    let o = pair(o, unit::YAW_BOOST_CAP, -1.0);
    let o = pair(o, unit::YAW_BREAKOUT, K_BREAKOUT);
    let o = pair(o, unit::YAW_BREAKOUT_CAP, -1.0);
    let o = pair(o, unit::YAW_LEAD, K_LEAD);
    let o = pair(o, unit::YAW_LEAD_CAP, -1.0);
    pair(o, unit::YAW_FOLLOW, W_FOLLOW)
}

const fn pitch_out() -> Out {
    let o = pair([0.0; HIDDEN + 1], unit::PITCH_PULL, G_BASE + G_BOOST);
    let o = pair(o, unit::PITCH_BOOST_CAP, -1.0);
    let o = pair(o, unit::PITCH_BREAKOUT, K_BREAKOUT);
    let o = pair(o, unit::PITCH_BREAKOUT_CAP, -1.0);
    pair(o, unit::PITCH_LEAD, K_LEAD_PITCH)
}

/// The output layer: the yaw turn, then the pitch turn.
pub const LAYER2: [Out; OUTPUTS] = [yaw_out(), pitch_out()];

/// What the hand knows about its target on one tick, in plain units.
/// [`Features::encode`] scales and bounds it for the network.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Features {
    /// From the aim to the aim point, radians.
    pub error: [f32; 2],
    /// Radians a second.
    pub error_rate: [f32; 2],
    /// The target's bearing rate, radians a second.
    pub target_rate: [f32; 2],
    /// The thumb in the turn frame: `[-right_stick.x, right_stick.y]`.
    pub stick: [f32; 2],
    /// The turn the last reports asked for that the view is too old to
    /// show, radians.
    pub pending: [f32; 2],
    /// Seconds since the target was acquired.
    pub since_acquired: f32,
    /// World units to the target.
    pub distance: f32,
    /// 0 for a weapon that needs the exact point, 1 for one that does not.
    pub looseness: f32,
    /// The skill's (tuned) [`Aim`](crate::mind::Aim) numbers.
    pub gain: f32,
    pub spread: f32,
    pub lead: f32,
    /// The hand's wandering error this tick, radians.
    pub noise: [f32; 2],
}

fn bounded(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { 0.0 }
}

impl Features {
    /// The network's input vector. Every input is bounded, and a value
    /// that is not a number reads as zero.
    pub fn encode(&self) -> [f32; INPUTS] {
        use std::f32::consts::{FRAC_PI_2, PI};
        let mut x = [0.0; INPUTS];
        x[input::ERROR] = bounded(self.error[0], -PI, PI);
        x[input::ERROR + 1] = bounded(self.error[1], -FRAC_PI_2, FRAC_PI_2);
        for axis in 0..2 {
            x[input::ERROR_RATE + axis] = bounded(self.error_rate[axis], -8.0, 8.0);
            x[input::TARGET_RATE + axis] = bounded(self.target_rate[axis], -8.0, 8.0);
            x[input::STICK + axis] = bounded(self.stick[axis], -1.0, 1.0);
            x[input::PENDING + axis] = bounded(self.pending[axis], -1.5, 1.5);
            x[input::NOISE + axis] = bounded(self.noise[axis], -0.5, 0.5);
        }
        x[input::SINCE] = bounded(self.since_acquired, 0.0, 2.0);
        let distance = bounded(self.distance, 0.0, f32::MAX).max(1.0);
        x[input::SIZE] = TARGET_HALF_WIDTH / distance;
        x[input::LOOSENESS] = bounded(self.looseness, 0.0, 1.0);
        x[input::GAIN] = bounded((self.gain - GAIN_LOW) / GAIN_SPAN, 0.0, 1.5);
        // Capped so the follow-through never opens on a centred thumb.
        x[input::SPREAD] = bounded(self.spread / SPREAD_UNIT, 0.0, 1.5);
        x[input::LEAD] = bounded(self.lead, 0.0, 1.0);
        x
    }
}

/// The hidden layer's activations.
pub fn hidden(x: &[f32; INPUTS]) -> [f32; HIDDEN] {
    let mut h = [0.0; HIDDEN];
    for (out, row) in h.iter_mut().zip(&LAYER1) {
        let mut sum = row[BIAS];
        for (w, v) in row[..INPUTS].iter().zip(x) {
            sum += w * v;
        }
        *out = sum.max(0.0);
    }
    h
}

/// The network: inputs to the turn request `[yaw, pitch]`, each -1..1.
pub fn forward(x: &[f32; INPUTS]) -> [f32; OUTPUTS] {
    let h = hidden(x);
    let mut out = [0.0; OUTPUTS];
    for (o, row) in out.iter_mut().zip(&LAYER2) {
        let mut sum = row[HIDDEN];
        for (w, v) in row[..HIDDEN].iter().zip(&h) {
            sum += w * v;
        }
        *o = sum.clamp(-1.0, 1.0);
    }
    out
}

/// The turn request for what the hand knows.
pub fn turn(f: &Features) -> [f32; OUTPUTS] {
    forward(&f.encode())
}

#[cfg(test)]
mod tests;
