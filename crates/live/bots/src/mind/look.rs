//! A right thumb, driven at 30 Hz. The game turns the view by retail's
//! look response; the hand steers by a model of it ([`RESPONSE`]) and
//! counts the turn its last reports will still make before it lands
//! (docs/game/bots.md §3.4).

use super::feel::{Rng, limit, reached};
use super::{Skill, TICKS, Vec3, angles, len, sub, wrap};
use crate::aim_net::{self, Features};
use crate::arms::Role;
use crate::cache::MapData;
use crate::sound::ShotSound;
use crate::tuning::AimModel;

/// Radians a second at full yaw stick for the bots' sensitivity-3
/// profile, the nominal curve's and the response table's full-stick end.
const TURN_RATE: f32 = 3.0;
const DEADZONE: f32 = 0.13;
const LOW_GAIN: f32 = 0.55;
const HIGH_GAIN: f32 = 1.0 - LOW_GAIN;
/// The local Midship table's turn at half stick before aim assist: about
/// 9°/s in yaw and 5°/s in pitch (docs/game/bots.md §3.4, §5.1).
const HALF_YAW: f32 = 9.0 * std::f32::consts::PI / 180.0;
const HALF_PITCH: f32 = 5.0 * std::f32::consts::PI / 180.0;

/// The look response the hand steers by: stick travel on one axis against
/// radians a second, piecewise linear from the dead zone to full stick.
/// Seeded from the one measured half-stick point on each axis and the
/// nominal full-stick rate, pitch scaled down with yaw; a stick sweep read
/// from `sim.player.list`'s `aim` replaces the rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Response {
    pub yaw: &'static [(f32, f32)],
    pub pitch: &'static [(f32, f32)],
}

pub(crate) const RESPONSE: Response = Response {
    yaw: &[(DEADZONE, 0.0), (0.5, HALF_YAW), (1.0, TURN_RATE)],
    pitch: &[
        (DEADZONE, 0.0),
        (0.5, HALF_PITCH),
        (1.0, TURN_RATE * HALF_PITCH / HALF_YAW),
    ],
};

/// A requested turn under this (radians a second) leaves the thumb
/// centred, so a rested hand does not twitch past the dead zone.
const QUIET: f32 = 0.004 * TURN_RATE;

fn curve(table: &[(f32, f32)], stick: f32) -> f32 {
    let stick = stick.min(1.0);
    if stick <= table[0].0 {
        return 0.0;
    }
    for pair in table.windows(2) {
        let ((s0, r0), (s1, r1)) = (pair[0], pair[1]);
        if stick <= s1 {
            return r0 + (r1 - r0) * (stick - s0) / (s1 - s0);
        }
    }
    table[table.len() - 1].1
}

fn inverse(table: &[(f32, f32)], rate: f32) -> f32 {
    if rate < QUIET {
        return 0.0;
    }
    for pair in table.windows(2) {
        let ((s0, r0), (s1, r1)) = (pair[0], pair[1]);
        if rate <= r1 {
            return s0 + (s1 - s0) * (rate - r0) / (r1 - r0);
        }
    }
    1.0
}

impl Response {
    /// The turn a stick makes, radians a second on each axis: stick x
    /// positive turns right (yaw down), y positive looks up.
    pub(crate) fn turn(&self, stick: (f32, f32)) -> (f32, f32) {
        let [x, y] = limit([stick.0, stick.1], 1.0);
        (
            x.signum() * curve(self.yaw, x.abs()),
            y.signum() * curve(self.pitch, y.abs()),
        )
    }

    /// The stick for a turn of `rate` (stick-signed, radians a second),
    /// each axis on its own curve, kept in the stick's circle.
    pub(crate) fn stick(&self, rate: [f32; 2]) -> [f32; 2] {
        limit(
            [
                rate[0].signum() * inverse(self.yaw, rate[0].abs()),
                rate[1].signum() * inverse(self.pitch, rate[1].abs()),
            ],
            1.0,
        )
    }
}

/// The game's turn for a stick, as the hand models it ([`RESPONSE`]).
pub(crate) fn turn_rate(stick: (f32, f32)) -> (f32, f32) {
    RESPONSE.turn(stick)
}

/// The nominal radial curve the aim net's weights were designed against
/// (docs/game/bots.md §3.4.1): the net's own count of its pending turn.
pub(crate) fn nominal_turn_rate(stick: (f32, f32)) -> (f32, f32) {
    let stick = limit([stick.0, stick.1], 1.0);
    let length = stick[0].hypot(stick[1]);
    if length <= DEADZONE {
        return (0.0, 0.0);
    }
    let live = (length - DEADZONE) / (1.0 - DEADZONE);
    let rate = TURN_RATE * live * (LOW_GAIN + HIGH_GAIN * live) / length;
    (stick[0] * rate, stick[1] * rate)
}

/// The integral term: it gathers the error the hand sees at this rate
/// (per second), only while that error is under [`INTEGRAL_BAND`] (no
/// wind-up through a flick), and adds at most [`INTEGRAL_LIMIT`] to it.
const INTEGRAL_RATE: f32 = 1.0;
const INTEGRAL_BAND: f32 = 0.1;
const INTEGRAL_LIMIT: f32 = 0.05;

/// The thumb rests on a still target: once the error the hand sees is under
/// `REST_AT / distance` radians (between [`REST_FLOOR`] and [`REST_CEIL`])
/// it lets the stick go, and takes it up again only past [`REST_EXIT`]
/// times that. A target whose bearing moves faster than [`REST_DRIFT`]
/// (radians a second) is tracked without a rest.
const REST_AT: f32 = 0.2;
const REST_FLOOR: f32 = 0.004;
const REST_CEIL: f32 = 0.04;
const REST_EXIT: f32 = 2.0;
const REST_DRIFT: f32 = 0.05;

/// A flick: with a tracked target more than [`FLICK_ERROR`] radians off,
/// the thumb travels up to [`FLICK_STEP`] a tick, under the pad's own
/// 0.45 (`tuning::InputLimits::stick_step`), at every skill.
pub(crate) const FLICK_ERROR: f32 = 0.3;
pub(crate) const FLICK_STEP: f32 = 0.4;

/// The radius the thumb rests within on a still target `distance` away.
fn rest_band(distance: f32) -> f32 {
    (REST_AT / distance.max(1e-3)).clamp(REST_FLOOR, REST_CEIL)
}

/// `Aim.error` is the tracking spread at this distance (world units)...
pub(crate) const SPREAD_AT: f32 = 5.0;
/// ...and this much of it stays however far the target is.
const SPREAD_FLOOR: f32 = 0.2;

/// The spread at `distance` as a fraction of `Aim.error`: the whole of it
/// within [`SPREAD_AT`], down to [`SPREAD_FLOOR`] far away. A hand misses
/// a far target by a smaller angle than a near one.
pub(crate) fn spread_scale(distance: f32) -> f32 {
    SPREAD_FLOOR + (1.0 - SPREAD_FLOOR) * (SPREAD_AT / distance.max(1e-3)).min(1.0)
}

/// Half the body the trigger is pulled on (world units), before the
/// weapon's looseness widens it.
pub(crate) const BODY_WIDTH: f32 = 0.35;
/// The narrowest fire cone (radians), for a target far away.
pub(crate) const CONE_FLOOR: f32 = 0.012;

/// How far off the aim point the trigger is pulled at `distance` with a
/// `role` gun: the body's width as an angle, wider for a forgiving gun,
/// from [`CONE_FLOOR`] up to the skill's `fire_cone` plus 0.3 rad at
/// point blank.
pub(crate) fn fire_cone(skill_cone: f32, distance: f32, role: Role) -> f32 {
    let width = BODY_WIDTH * (1.0 + looseness(role));
    (width / distance.max(0.1)).clamp(CONE_FLOOR, skill_cone + 0.3)
}

/// Seconds of a target's motion the aim point leads it by: the shot's
/// flight (`shot_speed` world units a second, 0 for unknown) and the
/// hand's own delay from view to report, times the skill's `lead`.
pub(crate) fn lead_time(skill: Skill, distance: f32, shot_speed: f32) -> f32 {
    let flight = if shot_speed > 0.0 {
        distance / shot_speed
    } else {
        0.0
    };
    skill.aim().lead * (flight + latency(skill) as f32 / TICKS)
}

#[derive(Clone, Copy)]
struct Handling {
    engage: f32,
    overshoot: f32,
    misread: f32,
}

impl Handling {
    fn of(skill: Skill) -> Self {
        let (engage, overshoot, misread) = match skill {
            Skill::Easy => (0.15, 0.16, 0.012),
            Skill::Normal => (0.19, 0.11, 0.007),
            Skill::Heroic => (0.22, 0.08, 0.004),
            Skill::Legendary => (0.25, 0.065, 0.002),
        };
        let twitchy = crate::tuning::current().is_some_and(|t| t.twitchy);
        Self {
            engage: if twitchy { 1.45 * engage } else { engage },
            overshoot: if twitchy { 1.35 * overshoot } else { overshoot },
            misread,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Bearing {
    id: u32,
    tick: u32,
    angle: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
struct Flick {
    direction: [f32; 2],
    offset: [f32; 2],
    until: u32,
}

#[derive(Clone, Copy, Debug)]
struct Glance {
    yaw: f32,
    from: u32,
    until: u32,
}

/// The hand's view of its target on the last tick it tracked one: the
/// angle from the aim to the aim point, before the hand's own error. The
/// sim's aim-motion metrics read it; nothing in the brain does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimTrace {
    pub target: u32,
    /// The tick of the view it was measured on.
    pub tick: u32,
    /// Yaw then pitch, radians.
    pub error: [f32; 2],
}

/// Walking, the pitch follows the slope to the next waypoint up to this
/// (radians).
const SLOPE_PITCH: f32 = 0.35;

/// How many of its own stick reports the hand remembers, for the turn the
/// view does not show yet (under either aim model).
const REMEMBERED: usize = 16;

/// How forgiving a weapon is of a near miss (`aim_net::input::LOOSENESS`).
fn looseness(role: Role) -> f32 {
    match role {
        Role::Sniper => 0.0,
        Role::Precise => 0.15,
        Role::Blast | Role::Unknown => 0.5,
        Role::Spray => 0.7,
        Role::Scatter | Role::Blade => 1.0,
    }
}

#[derive(Clone, Debug)]
pub(super) struct Hand {
    rng: Rng,
    stick: [f32; 2],
    /// The aim net's state (`aim_model = "net"`): when the target was
    /// acquired, the last error, and this tick's inputs with the error
    /// `track` returned for them.
    acquired: Option<u32>,
    last_error: Option<[f32; 2]>,
    net: Option<([f32; 2], Features)>,
    /// The modelled turn of each recent report (radians, yaw then pitch),
    /// the newest first: on the response table, or the nominal curve under
    /// the net.
    turns: [[f32; 2]; REMEMBERED],
    /// The code aim's integral of the error it has seen; the error `track`
    /// returned this tick with the band the thumb may rest within (0 for a
    /// moving target), so `steer` applies both only to that; and whether
    /// the thumb is resting.
    integral: [f32; 2],
    tracked: Option<([f32; 2], f32)>,
    resting: bool,
    looseness: f32,
    trace: Option<AimTrace>,
    bearing: Option<Bearing>,
    drift: [f32; 2],
    misread: Option<(f32, u32)>,
    flick: Option<Flick>,
    glance: Option<Glance>,
    listen_after: Option<u32>,
    corner: Option<Glance>,
    corner_after: Option<u32>,
    /// The target this tick is under active camouflage: its tracking
    /// error is this many times the skill's ([`camo_aim`]), and the thumb
    /// engages it at [`CAMO_ENGAGE`] of its pace. 1 for a plain target.
    obscured: f32,
}

/// A camouflaged target's tracking error, in times the skill's: a shimmer
/// is hard to hold the crosshair on, the better hand less so.
pub(crate) fn camo_aim(skill: Skill) -> f32 {
    match skill {
        Skill::Easy => 2.5,
        Skill::Normal => 2.2,
        Skill::Heroic => 1.9,
        Skill::Legendary => 1.6,
    }
}
/// The thumb's pace onto a camouflaged target, as a fraction of its own:
/// a slower lock.
pub(crate) const CAMO_ENGAGE: f32 = 0.6;

impl Hand {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed ^ 0x7374_6963_6b21),
            stick: [0.0; 2],
            acquired: None,
            last_error: None,
            net: None,
            turns: [[0.0; 2]; REMEMBERED],
            integral: [0.0; 2],
            tracked: None,
            resting: false,
            looseness: looseness(Role::Unknown),
            trace: None,
            bearing: None,
            drift: [0.0; 2],
            misread: None,
            flick: None,
            glance: None,
            listen_after: None,
            corner: None,
            corner_after: None,
            obscured: 1.0,
        }
    }

    pub(super) fn release(&mut self) {
        self.stick = [0.0; 2];
        self.acquired = None;
        self.last_error = None;
        self.net = None;
        self.turns = [[0.0; 2]; REMEMBERED];
        self.integral = [0.0; 2];
        self.tracked = None;
        self.resting = false;
        self.trace = None;
        self.bearing = None;
        self.flick = None;
        self.drift = [0.0; 2];
        self.misread = None;
        self.glance = None;
        self.listen_after = None;
        self.corner = None;
        self.corner_after = None;
        self.obscured = 1.0;
    }

    /// Whether the target the hand tracks this tick is camouflaged
    /// (`perceive::Cues::cloaked`) at `skill`: set before each
    /// [`track`](Self::track).
    pub(super) fn obscure(&mut self, skill: Skill, cloaked: bool) {
        self.obscured = if cloaked { camo_aim(skill) } else { 1.0 };
    }

    /// Visibility is required for consecutive bearing samples. On the
    /// first tick back in sight, it has no angular velocity estimate.
    pub(super) fn lose_target(&mut self) {
        self.acquired = None;
        self.last_error = None;
        self.net = None;
        self.integral = [0.0; 2];
        self.tracked = None;
        self.resting = false;
        self.bearing = None;
        self.flick = None;
        self.drift = [0.0; 2];
        self.misread = None;
    }

    /// The weapon in hand, for how exact the aim has to be.
    pub(super) fn wield(&mut self, role: Role) {
        self.looseness = looseness(role);
    }

    /// What the hand last saw of a tracked target.
    pub(super) fn trace(&self) -> Option<AimTrace> {
        self.trace
    }

    /// The facing the hand's reports will have made by the time a report
    /// sent now lands: `aim` as the view shows it plus the modelled turn of
    /// the last `perception_ticks + motor_ticks` reports, which the view
    /// does not show yet.
    pub(super) fn ahead(&self, skill: Skill, aim: (f32, f32)) -> (f32, f32) {
        let pending = self.pending(latency(skill));
        (wrap(aim.0 + pending[0]), aim.1 + pending[1])
    }

    fn pending(&self, late: usize) -> [f32; 2] {
        self.turns[..late]
            .iter()
            .fold([0.0; 2], |sum, turn| [sum[0] + turn[0], sum[1] + turn[1]])
    }

    /// Angles from the raw visible target and from the aim point (the lead
    /// for the shot's flight and the hand's own delay). The code aim
    /// steers from the facing it predicts ([`ahead`](Self::ahead)); the
    /// trace and the net read the view's.
    pub(super) fn track(
        &mut self,
        skill: Skill,
        id: u32,
        tick: u32,
        aim: (f32, f32),
        [raw, led]: [Vec3; 2],
        noise_extra: f32,
    ) -> [f32; 2] {
        let distance = len(raw);
        let raw = angles(raw);
        let led = angles(led);
        let h = Handling::of(skill);
        let prior = self
            .bearing
            .filter(|b| b.id == id && tick.wrapping_sub(b.tick) <= 3);
        let viewed = angle_error(aim, led);
        self.trace = Some(AimTrace {
            target: id,
            tick,
            error: viewed,
        });
        if crate::tuning::aim_model() == AimModel::Net {
            return self.track_net(
                skill,
                h,
                id,
                tick,
                raw,
                viewed,
                distance,
                prior,
                noise_extra,
            );
        }
        let mut error = angle_error(self.ahead(skill, aim), led);
        let mut rest = 0.0;
        let dt = match prior {
            Some(b) => {
                let dt = tick.wrapping_sub(b.tick).max(1);
                let [x, y] = angle_error(b.angle, raw);
                if x.hypot(y) * TICKS / (dt as f32) < REST_DRIFT {
                    rest = rest_band(distance);
                }
                dt
            }
            None => {
                self.lose_target();
                let magnitude = error[0].hypot(error[1]);
                if magnitude > 0.55 {
                    let direction = error.map(|a| a / magnitude);
                    let overshoot = h.overshoot * (0.8 + 0.4 * self.rng.unit());
                    self.flick = Some(Flick {
                        direction,
                        offset: direction.map(|a| a * overshoot),
                        until: tick.wrapping_add(45),
                    });
                }
                1
            }
        };
        self.bearing = Some(Bearing {
            id,
            tick,
            angle: raw,
        });
        self.glance = None;
        self.corner = None;

        if let Some(f) = self.flick {
            let past = error[0] * f.direction[0] + error[1] * f.direction[1] < -0.012;
            if past || reached(tick, f.until) {
                self.flick = None;
            } else {
                error[0] += f.offset[0];
                error[1] += f.offset[1];
            }
        }
        let noise =
            crate::human_input::tracking_noise(self.error(skill, h, tick, distance), noise_extra);
        let seen = [error[0] + noise[0], error[1] + noise[1]];
        if !self.resting && seen[0].hypot(seen[1]) < INTEGRAL_BAND {
            let gather = INTEGRAL_RATE * dt as f32 / TICKS;
            for (sum, e) in self.integral.iter_mut().zip(seen) {
                *sum = (*sum + gather * e).clamp(-INTEGRAL_LIMIT, INTEGRAL_LIMIT);
            }
        }
        self.tracked = Some((seen, rest));
        seen
    }

    /// The aim net's side of [`track`](Self::track): the same view and the
    /// same wandering error, laid out as the network's inputs. The error
    /// returned is what the hand believes; [`steer`](Self::steer) runs the
    /// network when it is handed that error back.
    fn track_net(
        &mut self,
        skill: Skill,
        h: Handling,
        id: u32,
        tick: u32,
        raw: (f32, f32),
        error: [f32; 2],
        distance: f32,
        prior: Option<Bearing>,
        noise_extra: f32,
    ) -> [f32; 2] {
        let mut error_rate = [0.0; 2];
        let mut target_rate = [0.0; 2];
        if let Some(b) = prior {
            let dt = tick.wrapping_sub(b.tick).max(1) as f32 / TICKS;
            target_rate = angle_error(b.angle, raw).map(|a| a / dt);
            if let Some(last) = self.last_error {
                error_rate = [wrap(error[0] - last[0]) / dt, (error[1] - last[1]) / dt];
            }
        } else {
            self.lose_target();
            self.acquired = Some(tick);
        }
        self.bearing = Some(Bearing {
            id,
            tick,
            angle: raw,
        });
        self.glance = None;
        self.corner = None;
        self.last_error = Some(error);
        let noise =
            crate::human_input::tracking_noise(self.error(skill, h, tick, distance), noise_extra);
        let aim = skill.aim();
        let pending = self.pending(latency(skill));
        let seen = [error[0] + noise[0], error[1] + noise[1]];
        let features = Features {
            error,
            error_rate,
            target_rate,
            stick: [-self.stick[0], self.stick[1]],
            pending,
            since_acquired: self
                .acquired
                .map_or(0.0, |t| tick.wrapping_sub(t) as f32 / TICKS),
            distance,
            looseness: self.looseness,
            gain: aim.gain,
            spread: aim.error,
            lead: aim.lead,
            noise,
        };
        self.net = Some((seen, features));
        seen
    }

    /// The hand's own error this tick: a wandering drift (in units of the
    /// spread), a little white noise and the occasional misread, all
    /// scaled down with range ([`spread_scale`]).
    fn error(&mut self, skill: Skill, handling: Handling, tick: u32, distance: f32) -> [f32; 2] {
        let base = skill.aim().error * self.obscured;
        let range = spread_scale(distance);
        let spread = base * range;
        for (axis, drift) in self.drift.iter_mut().enumerate() {
            let scale = if axis == 0 { 1.0 } else { 0.5 };
            *drift = 0.86 * *drift + 0.51 * scale * (self.rng.signed() + self.rng.signed()) * 1.225;
        }
        if self.misread.is_some_and(|(_, until)| reached(tick, until)) {
            self.misread = None;
        }
        if self.misread.is_none() && self.rng.unit() < handling.misread {
            let offset = (0.025 + 1.4 * base) * self.rng.signed().signum();
            self.misread = Some((offset, tick.wrapping_add(self.rng.ticks(3, 8))));
        }
        [
            spread * (self.drift[0] + 0.08 * self.rng.signed())
                + range * self.misread.map_or(0.0, |m| m.0),
            spread * (self.drift[1] + 0.04 * self.rng.signed()),
        ]
    }

    /// Slew the stick toward the requested turn. Passing through the centre
    /// costs time, including when correcting an overshoot. On the tracked
    /// target the code aim flicks faster ([`FLICK_STEP`]) and rests on a
    /// still one ([`REST_AT`]).
    ///
    /// The aim net asks for the stick only on a tracked target, for the
    /// error `track` returned this tick. Any other look is the code's.
    pub(super) fn steer(&mut self, skill: Skill, error: Option<[f32; 2]>, dt: u32) -> (f32, f32) {
        let net = self.net.take();
        let tracked = self.tracked.take();
        let resting = std::mem::take(&mut self.resting);
        let Some([yaw, pitch]) = error else {
            self.stick = [0.0; 2];
            self.remember(dt);
            return (0.0, 0.0);
        };
        let ticks = dt.clamp(1, 2) as f32;
        let pace = if self.obscured > 1.0 { CAMO_ENGAGE } else { 1.0 };
        let mut step = Handling::of(skill).engage * ticks * pace;
        let want = match net {
            Some((seen, features)) if seen == [yaw, pitch] => {
                let [yaw, pitch] = aim_net::turn(&features);
                limit([-yaw, pitch], 1.0)
            }
            _ => {
                // The integral, the rest and the flick only on the target
                // `track` saw this tick: a glance or a walk's look is
                // proportional.
                let (i, band, on) = match tracked {
                    Some((seen, band)) if seen == [yaw, pitch] => (self.integral, band, true),
                    _ => ([0.0; 2], 0.0, false),
                };
                let size = yaw.hypot(pitch);
                let edge = if resting { REST_EXIT * band } else { band };
                self.resting = size < edge;
                if on && size > FLICK_ERROR {
                    step = step.max(FLICK_STEP * ticks * pace);
                }
                if self.resting {
                    [0.0; 2]
                } else {
                    let rate = skill.aim().gain * TURN_RATE;
                    RESPONSE.stick([-(yaw + i[0]) * rate, (pitch + i[1]) * rate])
                }
            }
        };
        let delta = limit([want[0] - self.stick[0], want[1] - self.stick[1]], step);
        self.stick = limit([self.stick[0] + delta[0], self.stick[1] + delta[1]], 1.0);
        self.remember(dt);
        (self.stick[0], self.stick[1])
    }

    /// The hand's memory of what its reports asked for: the modelled turn
    /// of this tick's stick, once a tick it stands for. The net counts on
    /// the nominal curve it was designed against.
    fn remember(&mut self, dt: u32) {
        let stick = (self.stick[0], self.stick[1]);
        let (x, y) = if crate::tuning::aim_model() == AimModel::Net {
            nominal_turn_rate(stick)
        } else {
            turn_rate(stick)
        };
        for _ in 0..dt.clamp(1, 2) {
            self.turns.copy_within(..REMEMBERED - 1, 1);
            self.turns[0] = [-x / TICKS, y / TICKS];
        }
    }

    /// Once per glance, sample a coarse bearing to the closest audible
    /// muzzle report. The point is never used to pick a target or fire.
    pub(super) fn sound(
        &mut self,
        ear: Vec3,
        tick: u32,
        map: Option<&MapData>,
        sounds: &[ShotSound],
    ) -> Option<f32> {
        if self.glance.is_some_and(|g| reached(tick, g.until)) {
            self.glance = None;
        }
        if self.listen_after.is_none_or(|t| reached(tick, t)) {
            let nearest = sounds
                .iter()
                .filter(|s| tick.wrapping_sub(s.tick) <= 6)
                .filter_map(|s| {
                    let to = sub(s.at, ear);
                    let distance = to[0].hypot(to[1]).hypot(to[2]);
                    // Own fire and point-blank threats are already handled
                    // by the combat loop. Walls muffle distant reports.
                    if !(1.0..=18.0).contains(&distance) {
                        return None;
                    }
                    let muffled = map.is_some_and(|m| !m.walls.clear(ear, s.at));
                    if muffled && distance > 8.0 {
                        return None;
                    }
                    Some((
                        distance * if muffled { 2.0 } else { 1.0 },
                        angles(to).0,
                        muffled,
                    ))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, yaw, muffled)) = nearest {
                let from = tick.wrapping_add(self.rng.ticks(5, 8));
                let uncertainty = if muffled { 0.32 } else { 0.16 };
                self.glance = Some(Glance {
                    yaw: {
                        let sector = std::f32::consts::FRAC_PI_8;
                        (yaw / sector).round() * sector + uncertainty * self.rng.signed()
                    },
                    from,
                    until: from.wrapping_add(self.rng.ticks(10, 18)),
                });
                self.listen_after = Some(tick.wrapping_add(50));
            }
        }
        self.glance.filter(|g| reached(tick, g.from)).map(|g| g.yaw)
    }

    /// Where to look while walking `dir`: into the route's next turn
    /// (`corner`) when it is in sight, else along the walk, glancing into
    /// a doorway it passes. Off a turn the pitch follows the slope to the
    /// next waypoint (`ahead`), so a ramp is not climbed staring level.
    pub(super) fn travel(
        &mut self,
        eye: Vec3,
        dir: [f32; 2],
        tick: u32,
        map: Option<&MapData>,
        corner: Option<Vec3>,
        ahead: Option<Vec3>,
    ) -> (f32, f32) {
        let forward = dir[1].atan2(dir[0]);
        let slope = ahead
            .map(|at| sub(super::up(at, super::EYE), eye))
            .filter(|to| to[0].hypot(to[1]) > 0.5)
            .map_or(0.0, |to| angles(to).1.clamp(-SLOPE_PITCH, SLOPE_PITCH));
        if let Some(map) = map {
            if let Some(at) = corner {
                let at = super::up(at, super::EYE);
                if map.walls.clear(eye, at) {
                    let (yaw, pitch) = angles(sub(at, eye));
                    return (
                        forward + wrap(yaw - forward).clamp(-0.65, 0.65),
                        pitch.clamp(-0.25, 0.25),
                    );
                }
            }
            if let Some(yaw) = self.doorway(eye, dir, tick, map) {
                return (yaw, slope);
            }
        }
        (forward, slope)
    }

    /// When approaching a doorway edge, check the side that opens up.
    /// A fixed short glance is shared across ticks so a threshold does not
    /// make the view wag back and forth.
    pub(super) fn doorway(
        &mut self,
        eye: Vec3,
        dir: [f32; 2],
        tick: u32,
        map: &MapData,
    ) -> Option<f32> {
        if self.corner.is_some_and(|g| reached(tick, g.until)) {
            self.corner = None;
        }
        if self.corner_after.is_none_or(|t| reached(tick, t)) {
            self.corner_after = Some(tick.wrapping_add(6));
            let ahead = [eye[0] + 1.2 * dir[0], eye[1] + 1.2 * dir[1], eye[2]];
            for sign in [-1.0, 1.0] {
                let side = [-sign * 1.25 * dir[1], sign * 1.25 * dir[0]];
                let here = [eye[0] + side[0], eye[1] + side[1], eye[2]];
                let there = [ahead[0] + side[0], ahead[1] + side[1], eye[2]];
                if !map.walls.clear(eye, here)
                    && map.walls.clear(ahead, there)
                    && map.walls.clear(eye, there)
                {
                    let yaw = dir[1].atan2(dir[0]) + sign * 0.45;
                    self.corner = Some(Glance {
                        yaw,
                        from: tick,
                        until: tick.wrapping_add(9),
                    });
                    self.corner_after = Some(tick.wrapping_add(24));
                    break;
                }
            }
        }
        self.corner.map(|g| g.yaw)
    }
}

pub(super) fn angle_error(from: (f32, f32), to: (f32, f32)) -> [f32; 2] {
    [wrap(to.0 - from.0), to.1 - from.1]
}

/// Ticks from a view to the stick report made on it landing: the brain's
/// perception and motor delays (`tuning::InputLimits`).
fn latency(skill: Skill) -> usize {
    let limits = crate::tuning::current()
        .unwrap_or_default()
        .input_limits(skill);
    ((limits.perception_ticks + limits.motor_ticks) as usize).min(REMEMBERED)
}

#[cfg(test)]
mod tests;
