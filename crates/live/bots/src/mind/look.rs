//! A right thumb, driven at 30 Hz. The game handles local stick input.
//! The hand requests stick travel using its nominal sensitivity model.

use super::feel::{Rng, limit, reached};
use super::{Skill, TICKS, Vec3, angles, len, sub, wrap};
use crate::aim_net::{self, Features};
use crate::arms::Role;
use crate::cache::MapData;
use crate::sound::ShotSound;
use crate::tuning::AimModel;

/// Nominal radians per second at full stick for the bots' sensitivity-3
/// profile. The curve is a tuning model for that profile.
const TURN_RATE: f32 = 3.0;
const DEADZONE: f32 = 0.13;
const LOW_GAIN: f32 = 0.55;
const HIGH_GAIN: f32 = 1.0 - LOW_GAIN;

pub(crate) fn turn_rate(stick: (f32, f32)) -> (f32, f32) {
    let stick = limit([stick.0, stick.1], 1.0);
    let length = stick[0].hypot(stick[1]);
    if length <= DEADZONE {
        return (0.0, 0.0);
    }
    let live = (length - DEADZONE) / (1.0 - DEADZONE);
    let rate = TURN_RATE * live * (LOW_GAIN + HIGH_GAIN * live) / length;
    (stick[0] * rate, stick[1] * rate)
}

/// Translate a requested fraction of full turn rate into stick travel.
/// A small quiet band near the centre keeps a rested thumb centred.
fn deflection(speed: [f32; 2]) -> [f32; 2] {
    let speed = limit(speed, 1.0);
    let mag = speed[0].hypot(speed[1]);
    if mag < 0.004 {
        return [0.0; 2];
    }
    let live = 2.0 * mag / (LOW_GAIN + (LOW_GAIN * LOW_GAIN + 4.0 * HIGH_GAIN * mag).sqrt());
    speed.map(|v| v * ((DEADZONE + (1.0 - DEADZONE) * live) / mag))
}

#[derive(Clone, Copy)]
struct Handling {
    engage: f32,
    overshoot: f32,
    lag: f32,
    misread: f32,
}

impl Handling {
    fn of(skill: Skill) -> Self {
        let (engage, overshoot, lag, misread) = match skill {
            Skill::Easy => (0.15, 0.16, 0.12, 0.012),
            Skill::Normal => (0.19, 0.11, 0.095, 0.007),
            Skill::Heroic => (0.22, 0.08, 0.073, 0.004),
            Skill::Legendary => (0.25, 0.065, 0.055, 0.002),
        };
        let twitchy = crate::tuning::current().is_some_and(|t| t.twitchy);
        Self {
            engage: if twitchy { 1.45 * engage } else { engage },
            overshoot: if twitchy { 1.35 * overshoot } else { overshoot },
            lag,
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

/// How many of its own stick reports the hand remembers, for the turn the
/// view does not show yet.
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
    /// acquired, the last error, this tick's inputs with the error `track`
    /// returned for them, and the nominal turn of each recent report, the
    /// newest first, in the turn frame.
    acquired: Option<u32>,
    last_error: Option<[f32; 2]>,
    net: Option<([f32; 2], Features)>,
    turns: [[f32; 2]; REMEMBERED],
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
}

impl Hand {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed ^ 0x7374_6963_6b21),
            stick: [0.0; 2],
            acquired: None,
            last_error: None,
            net: None,
            turns: [[0.0; 2]; REMEMBERED],
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
        }
    }

    pub(super) fn release(&mut self) {
        self.stick = [0.0; 2];
        self.acquired = None;
        self.last_error = None;
        self.net = None;
        self.turns = [[0.0; 2]; REMEMBERED];
        self.trace = None;
        self.bearing = None;
        self.flick = None;
        self.drift = [0.0; 2];
        self.misread = None;
        self.glance = None;
        self.listen_after = None;
        self.corner = None;
        self.corner_after = None;
    }

    /// Visibility is required for consecutive bearing samples. On the
    /// first tick back in sight, it has no angular velocity estimate.
    pub(super) fn lose_target(&mut self) {
        self.acquired = None;
        self.last_error = None;
        self.net = None;
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

    /// Angles from the raw visible target and from the projectile lead.
    /// Target motion and shot lead have separate effects on the hand.
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
        let mut error = angle_error(aim, led);
        self.trace = Some(AimTrace {
            target: id,
            tick,
            error,
        });
        if crate::tuning::aim_model() == AimModel::Net {
            return self.track_net(skill, h, id, tick, raw, error, distance, prior, noise_extra);
        }
        if let Some(b) = prior {
            let dt = tick.wrapping_sub(b.tick).max(1) as f32 / TICKS;
            let v = angle_error(b.angle, raw).map(|a| (a / dt).clamp(-8.0, 8.0));
            let lag = tracking_lag(v, h.lag);
            error[0] -= lag[0];
            error[1] -= lag[1];
        } else {
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
        }
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
        let noise = crate::human_input::tracking_noise(self.error(skill, h, tick), noise_extra);
        [error[0] + noise[0], error[1] + noise[1]]
    }

    /// The aim net's side of [`track`](Self::track): the same view and the
    /// same wandering error, laid out as the network's inputs. The error
    /// returned is what the hand believes; [`steer`](Self::steer) runs the
    /// network when it is handed that error back.
    #[allow(clippy::too_many_arguments)]
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
        let noise = crate::human_input::tracking_noise(self.error(skill, h, tick), noise_extra);
        let aim = skill.aim();
        let late = latency(skill);
        let mut pending = [0.0; 2];
        for turn in &self.turns[..late] {
            pending[0] += turn[0];
            pending[1] += turn[1];
        }
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

    fn error(&mut self, skill: Skill, handling: Handling, tick: u32) -> [f32; 2] {
        let spread = skill.aim().error;
        for (axis, drift) in self.drift.iter_mut().enumerate() {
            let scale = if axis == 0 { 1.0 } else { 0.5 };
            *drift = 0.86 * *drift
                + 0.51 * spread * scale * (self.rng.signed() + self.rng.signed()) * 1.225;
        }
        if self.misread.is_some_and(|(_, until)| reached(tick, until)) {
            self.misread = None;
        }
        if self.misread.is_none() && self.rng.unit() < handling.misread {
            let offset = (0.025 + 1.4 * spread) * self.rng.signed().signum();
            self.misread = Some((offset, tick.wrapping_add(self.rng.ticks(3, 8))));
        }
        [
            self.drift[0] + 0.08 * spread * self.rng.signed() + self.misread.map_or(0.0, |m| m.0),
            self.drift[1] + 0.04 * spread * self.rng.signed(),
        ]
    }

    /// Slew the stick toward the requested turn. Passing through the centre
    /// costs time, including when correcting an overshoot.
    ///
    /// The aim net asks for the stick only on a tracked target, for the
    /// error `track` returned this tick. Any other look is the code's.
    pub(super) fn steer(&mut self, skill: Skill, error: Option<[f32; 2]>, dt: u32) -> (f32, f32) {
        let net = self.net.take();
        let Some([yaw, pitch]) = error else {
            self.stick = [0.0; 2];
            self.remember(dt);
            return (0.0, 0.0);
        };
        let want = match net {
            Some((seen, features)) if seen == [yaw, pitch] => {
                let [yaw, pitch] = aim_net::turn(&features);
                limit([-yaw, pitch], 1.0)
            }
            _ => {
                let gain = skill.aim().gain;
                deflection([-yaw * gain, pitch * gain])
            }
        };
        let step = Handling::of(skill).engage * dt.clamp(1, 2) as f32;
        let delta = limit([want[0] - self.stick[0], want[1] - self.stick[1]], step);
        self.stick = limit([self.stick[0] + delta[0], self.stick[1] + delta[1]], 1.0);
        self.remember(dt);
        (self.stick[0], self.stick[1])
    }

    /// The aim net's memory of what its reports asked for: the nominal
    /// turn of this tick's stick, once a tick it stands for.
    fn remember(&mut self, dt: u32) {
        if crate::tuning::aim_model() != AimModel::Net {
            return;
        }
        let (x, y) = turn_rate((self.stick[0], self.stick[1]));
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

    pub(super) fn travel(
        &mut self,
        eye: Vec3,
        dir: [f32; 2],
        tick: u32,
        map: Option<&MapData>,
        corner: Option<Vec3>,
    ) -> (f32, f32) {
        let forward = dir[1].atan2(dir[0]);
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
                return (yaw, 0.0);
            }
        }
        (forward, 0.0)
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

fn tracking_lag(rate: [f32; 2], base: f32) -> [f32; 2] {
    let seconds = base + 0.012 * rate[0].hypot(rate[1]).min(8.0);
    limit(rate.map(|v| v * seconds), 0.28)
}

#[cfg(test)]
mod tests;
