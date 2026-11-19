//! A right thumb, driven at 30 Hz. The game handles local stick input.
//! The phantom path uses the nominal curve below because it submits an
//! action with absolute angles in place of an Xbox pad report.

use super::feel::{Rng, limit, reached};
use super::{Skill, TICKS, Vec3, angles, sub, wrap};
use crate::cache::MapData;
use crate::sound::ShotSound;

/// Nominal radians per second at full stick for the bots' sensitivity-3
/// profile. The curve is a tuning model for that profile.
pub(crate) const TURN_RATE: f32 = 3.0;
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
        Self {
            engage,
            overshoot,
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

#[derive(Clone, Debug)]
pub(super) struct Hand {
    rng: Rng,
    stick: [f32; 2],
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
        self.bearing = None;
        self.flick = None;
        self.drift = [0.0; 2];
        self.misread = None;
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
        let raw = angles(raw);
        let led = angles(led);
        let h = Handling::of(skill);
        let prior = self
            .bearing
            .filter(|b| b.id == id && tick.wrapping_sub(b.tick) <= 3);
        let mut error = angle_error(aim, led);
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
    pub(super) fn steer(&mut self, skill: Skill, error: Option<[f32; 2]>, dt: u32) -> (f32, f32) {
        let Some([yaw, pitch]) = error else {
            self.stick = [0.0; 2];
            return (0.0, 0.0);
        };
        let gain = skill.aim().gain;
        let want = deflection([-yaw * gain, pitch * gain]);
        let step = Handling::of(skill).engage * dt.clamp(1, 2) as f32;
        let delta = limit([want[0] - self.stick[0], want[1] - self.stick[1]], step);
        self.stick = limit([self.stick[0] + delta[0], self.stick[1] + delta[1]], 1.0);
        (self.stick[0], self.stick[1])
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

fn tracking_lag(rate: [f32; 2], base: f32) -> [f32; 2] {
    let seconds = base + 0.012 * rate[0].hypot(rate[1]).min(8.0);
    limit(rate.map(|v| v * seconds), 0.28)
}

#[cfg(test)]
mod tests;
