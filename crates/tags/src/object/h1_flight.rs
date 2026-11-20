//! Halo 1 aircraft controllers. Linear and angular velocities here are per
//! second. The source XBE integrates its force and torque once per 30 Hz tick.

use super::direct_drive::{cross, dot};

pub const DATA: u32 = 0x180;
pub const MAGIC: u32 = u32::from_le_bytes(*b"H1fl");
pub const DROPSHIP: u32 = 1;
pub const LIFEPOD: u32 = 2;
pub const BANSHEE: u32 = 3;
pub const PASSIVE: u32 = 4;

type V = [f32; 3];

fn norm(v: V) -> f32 {
    dot(v, v).sqrt()
}
fn times(v: V, s: f32) -> V {
    v.map(|x| x * s)
}
fn plus(a: V, b: V) -> V {
    std::array::from_fn(|i| a[i] + b[i])
}

/// World-space shortest rotation from the body's basis to the desired basis
/// (H1 0xb42c0). Recover the axis from the diagonal at a half turn.
fn attitude(forward: V, up: V, look: V, target_up: V) -> V {
    let a = [forward, cross(up, forward), up];
    let b = [look, cross(target_up, look), target_up];
    let r = |i: usize, j: usize| (0..3).map(|k| b[k][i] * a[k][j]).sum::<f32>();
    let axis = [r(2, 1) - r(1, 2), r(0, 2) - r(2, 0), r(1, 0) - r(0, 1)];
    let sin2 = norm(axis);
    let cos2 = r(0, 0) + r(1, 1) + r(2, 2) - 1.0;
    if sin2 > 1e-6 {
        return times(axis, sin2.atan2(cos2) / sin2);
    }
    if cos2 >= 0.0 {
        return times(axis, 0.5);
    }
    let mut i = 0;
    for j in 1..3 {
        if r(j, j) > r(i, i) {
            i = j;
        }
    }
    let d = ((1.0 + r(i, i)) * 0.5).sqrt();
    std::array::from_fn(|j| {
        let component = if i == j {
            d
        } else {
            (r(i, j) + r(j, i)) / (4.0 * d)
        };
        component * std::f32::consts::PI
    })
}

#[derive(Clone, Copy)]
pub struct Aircraft {
    pub kind: u32,
    pub radius: f32,
    pub mean_moment_per_mass: f32,
    pub gun_pitch: f32,
    pub bank: f32,
    pub turn: f32,
    pub limits: super::direct_drive::Limits,
}

pub struct Motion {
    pub forward: V,
    pub up: V,
    pub look: V,
    pub velocity: V,
    pub angular: V,
    pub mass: f32,
    pub gravity: f32,
    pub power: f32,
    pub driver_is_player: bool,
    pub scripted_hover: bool,
    pub crouch: bool,
    pub brake: bool,
}

impl Aircraft {
    /// H1 0x38ac0 (dropship), 0x37760 (positive physics radius) and
    /// 0x37ab0 (Banshee). Rotor power belongs to the individual vehicle.
    pub fn forces(self, motion: &Motion, speed: f32, rotor: &mut f32, dt: f32) -> (V, V) {
        use std::f32::consts::{FRAC_1_PI, FRAC_PI_2};
        let m = motion;
        // H1 0x38b14 clears the dropship motor during a scripted hover
        // and skips the rotor update. Recording playback owns the hold.
        if self.kind == PASSIVE
            || (self.kind == DROPSHIP && m.scripted_hover)
            || self.limits.forward <= 0.0
        {
            return ([0.0; 3], [0.0; 3]);
        }
        let mut look = m.look;
        let level = plus([0.0, 0.0, 1.0], times(look, -look[2]));
        let length = norm(level);
        let mut target_up = if length > 0.0001 {
            times(level, 1.0 / length)
        } else {
            [1.0, 0.0, 0.0]
        };
        if self.kind == BANSHEE && !m.driver_is_player {
            let (sn, cs) = self.gun_pitch.sin_cos();
            (look, target_up) = (
                plus(times(look, cs), times(target_up, sn)),
                plus(times(target_up, cs), times(look, -sn)),
            );
        }
        let max = self.limits.forward;
        let bank_gain = if self.kind == BANSHEE {
            self.bank
        } else {
            FRAC_PI_2
        };
        let bank = (look[0] * m.velocity[1] - look[1] * m.velocity[0]) * bank_gain / max;
        let (sn, cs) = bank.sin_cos();
        target_up = plus(times(target_up, cs), times(cross(look, target_up), sn));
        let error = attitude(m.forward, m.up, look, target_up);
        let (acceleration, steering, damping) = if self.kind == BANSHEE {
            // 0x35fb0 adds gravity to velocity error, then interpolates the
            // acceleration bound by the squared cosine with desired velocity.
            let desired = times(m.forward, speed);
            let delta = plus(
                plus(desired, times(m.velocity, -1.0)),
                [0.0, 0.0, m.gravity / 30.0],
            );
            let proj = dot(desired, delta);
            let weight = if proj > 0.0001 * 900.0 {
                proj * proj / (dot(desired, desired) * dot(delta, delta))
            } else {
                0.0
            };
            let fraction = if speed > 0.0 {
                speed / max
            } else if self.limits.reverse > 0.0 {
                -speed / self.limits.reverse
            } else {
                0.0
            };
            let bound = fraction
                * (self.limits.braking + weight * (self.limits.acceleration - self.limits.braking));
            let wanted = times(delta, 30.0);
            let magnitude = norm(wanted);
            let accel = if magnitude > bound && magnitude > 0.0 {
                times(wanted, bound / magnitude)
            } else {
                wanted
            };
            (
                accel,
                self.turn * 30.0 * FRAC_1_PI,
                self.mean_moment_per_mass * 30.0,
            )
        } else {
            let along = dot(m.velocity, m.forward);
            let mut thrust = (speed - along) * (0.05 * 30.0);
            let mut lift = (along / max).abs() * 1.05;
            let steering = if self.kind == DROPSHIP {
                let cruise = (speed / max).clamp(0.0, 1.0).powi(2);
                let collective = if m.crouch {
                    0.25
                } else if m.brake {
                    1.0
                } else {
                    0.75
                };
                let step = 0.05 * 30.0 * dt;
                *rotor += ((1.0 - cruise) * m.power * collective - *rotor).clamp(-step, step);
                thrust *= cruise * m.power;
                lift += *rotor * 1.3;
                1.0
            } else {
                4.0
            };
            (
                plus(times(m.forward, thrust), times(m.up, lift * m.gravity)),
                steering,
                self.radius.powi(2) * 1.5,
            )
        };
        (
            times(acceleration, m.mass * m.power),
            times(
                plus(times(error, steering), times(m.angular, -1.0)),
                damping * m.mass * m.power,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::direct_drive::Limits;

    fn motion() -> Motion {
        Motion {
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            look: [1.0, 0.0, 0.0],
            velocity: [0.0; 3],
            angular: [0.0; 3],
            mass: 4000.0,
            gravity: crate::object::direct_drive::SOURCE_GRAVITY,
            power: 1.0,
            driver_is_player: true,
            scripted_hover: false,
            crouch: false,
            brake: false,
        }
    }

    fn aircraft(kind: u32) -> Aircraft {
        Aircraft {
            kind,
            radius: 4.0,
            mean_moment_per_mass: 0.25,
            gun_pitch: 0.0,
            bank: 1.0,
            turn: 0.2,
            limits: Limits {
                forward: 6.6,
                reverse: 0.9,
                acceleration: 7.2,
                braking: 14.4,
            },
        }
    }

    #[test]
    fn banked_aircraft_at_speed_holds_altitude_and_slews_toward_the_look_direction() {
        let a = aircraft(BANSHEE);
        let mut m = motion();
        m.velocity = times(m.forward, a.limits.forward);
        let (force, torque) = a.forces(&m, a.limits.forward, &mut 0.0, 1.0 / 30.0);
        assert!((force[2] / m.mass - 0.0035651792 * 900.0).abs() < 1e-5);
        assert_eq!((force[0], force[1], torque), (0.0, 0.0, [0.0; 3]));
        m.velocity = [0.0; 3];
        m.look = [0.0, 1.0, 0.0];
        let (_, torque) = a.forces(&m, 0.0, &mut 0.0, 1.0 / 30.0);
        // A quarter turn at 0.2 radians/tick/pi requests 3 radians/sec.
        assert!((torque[2] / (m.mass * 0.25 * 30.0) - 3.0).abs() < 1e-5);
        m.look = [-1.0, 0.0, 0.0];
        let (_, torque) = a.forces(&m, 0.0, &mut 0.0, 1.0 / 30.0);
        assert!((torque[2].abs() / (m.mass * 0.25 * 30.0) - 6.0).abs() < 1e-5);
    }

    #[test]
    fn lifepod_thrust_and_turn_rate_follow_the_positive_radius_controller() {
        let a = aircraft(LIFEPOD);
        let mut m = motion();
        m.look = [0.0, 1.0, 0.0];
        let (force, torque) = a.forces(&m, 2.0, &mut 0.0, 1.0 / 30.0);
        assert_eq!(force, [3.0 * m.mass, 0.0, 0.0]);
        let yaw_rate = torque[2] / (1.5 * a.radius.powi(2) * m.mass);
        assert!((yaw_rate - 2.0 * std::f32::consts::PI).abs() < 1e-5);
    }

    #[test]
    fn dropship_collective_slews_and_passive_lifepod_has_no_motor() {
        let mut m = motion();
        let mut rotor = 0.0;
        let a = aircraft(DROPSHIP);
        let (first, _) = a.forces(&m, 0.0, &mut rotor, 1.0 / 30.0);
        assert!((rotor - 0.05).abs() < 1e-6);
        for _ in 0..30 {
            a.forces(&m, 0.0, &mut rotor, 1.0 / 30.0);
        }
        assert_eq!(rotor, 0.75);
        m.crouch = true;
        for _ in 0..30 {
            a.forces(&m, 0.0, &mut rotor, 1.0 / 30.0);
        }
        assert_eq!(rotor, 0.25);
        assert!(first[2] > 0.0);
        m.scripted_hover = true;
        assert_eq!(
            a.forces(&m, 6.6, &mut rotor, 1.0 / 30.0),
            ([0.0; 3], [0.0; 3])
        );
        assert_eq!(rotor, 0.25);
        assert_eq!(
            aircraft(PASSIVE).forces(&m, 5.0, &mut rotor, 1.0 / 30.0),
            ([0.0; 3], [0.0; 3])
        );
    }
}
