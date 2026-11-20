//! Halo 1's ground vehicle velocity controller, in world units per second.
//! The H1 cache uses 30 ticks/sec. 0x39785 passes the raw movement input
//! as the target and 0x80c10 limits each increment and clamps it to the
//! directional speed cap. At 0x3988b a tank uses the same ramp at double
//! rate for the difference between the two track speeds.

/// An extension in the dormant inertia slot (+0x260). A zero redline
/// (+0x264) bypasses the native torque integrator at 0xf1981.
pub const H1_DIRECT_DRIVE: u32 = u32::from_le_bytes(*b"H1dd");
/// Bit in a converted tank contact's flags for powered group 1.
pub const H1_SECOND_TRACK: u32 = 1 << 31;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub forward: f32,
    pub reverse: f32,
    pub acceleration: f32,
    pub braking: f32,
}

impl Limits {
    /// Time left after braking through zero is available for acceleration
    /// in the other direction (H1 0x80c6a and 0x80cce).
    pub fn advance(self, speed: f32, target: f32, dt: f32) -> f32 {
        if ![
            self.forward,
            self.reverse,
            self.acceleration,
            self.braking,
            dt,
        ]
        .into_iter()
        .all(|v| v.is_finite() && v >= 0.0)
            || !speed.is_finite()
            || !target.is_finite()
        {
            return speed;
        }
        let target = target.clamp(-self.reverse, self.forward);
        if speed == target {
            return speed;
        }
        let sign = (target - speed).signum();
        let along = sign * speed;
        let brake = self.braking * dt;
        let step = if along >= 0.0 {
            along + self.acceleration * dt
        } else if -along >= brake {
            along + brake
        } else {
            (1.0 + along / brake) * self.acceleration * dt
        };
        sign * step.min(sign * target)
    }

    pub fn tread_turn(self, previous: f32, forward: f32, yaw: f32, dt: f32) -> f32 {
        // H1 0x3987d..0x398d6: centre at single rate when stopped;
        // otherwise a quarter-turn reaches the forward cap at double rate.
        let (target, scale) = if forward == 0.0 {
            (0.0, 1.0)
        } else {
            (
                (yaw * std::f32::consts::FRAC_2_PI).clamp(-1.0, 1.0) * self.forward,
                2.0,
            )
        };
        // 0x80c7c/0x80cde also scale the directional caps by the rate.
        Self {
            forward: self.forward * scale,
            reverse: self.reverse * scale,
            ..self
        }
        .advance(previous, target, dt * scale)
    }
}

/// H2 0xf1e15..0xf1e8b takes a signed common speed S and a turn
/// fraction t. Its outputs are S*(1-|t|) +/- |S|*t. These inputs
/// therefore deliver the H1 pair (forward-turn, forward+turn).
pub fn tread_inputs(forward: f32, turn: f32) -> (f32, f32) {
    let total = forward.abs() + turn.abs();
    if total == 0.0 {
        return (0.0, 0.0);
    }
    (total.copysign(forward), turn / total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversal_uses_only_the_time_left_after_braking() {
        let limits = Limits {
            forward: 6.0,
            reverse: 3.0,
            acceleration: 3.0,
            braking: 6.0,
        };
        assert_eq!(limits.advance(-1.5, 6.0, 0.5), 0.75);
        assert_eq!(limits.advance(1.5, -3.0, 0.5), -0.75);
        assert_eq!(limits.advance(5.9, 30.0, 1.0), 6.0);
        assert_eq!(limits.advance(-0.25, 0.0, 1.0), 0.0);
        assert_eq!(
            Limits {
                braking: 0.0,
                ..limits
            }
            .advance(-1.0, 6.0, 1.0),
            -1.0
        );
    }

    #[test]
    fn mixer_preserves_each_track_in_reverse_and_at_a_pivot() {
        for (v, turn) in [(2.0, -2.0), (-1.0, 3.0), (0.0, 1.0), (0.0, 0.0)] {
            let (s, t) = tread_inputs(v, turn);
            let common = s * (1.0 - t.abs());
            assert_eq!(
                [common - s.abs() * t, common + s.abs() * t],
                [v - turn, v + turn]
            );
        }
    }
}

/// The wheel phantom holds at most sixteen contacts (two inline shapes).
/// The loader at 0x205550 only touches the shapes counted at +0x38.
pub const CONTACT_DATA: u32 = 0x180;
pub const CONTACT_MAGIC: u32 = u32::from_le_bytes(*b"H1cf");
pub const CONTACT_HEADER: u32 = 8;
pub const WHEEL_BYTES: u32 = 16;
pub const SURFACE_BYTES: u32 = 24;
pub const POWER_DATA: u32 = 0x288;
pub const POWER_MAGIC: u32 = u32::from_le_bytes(*b"H1pw");
pub const HAS_SOURCE_GRAVITY: u32 = 1 << 8;
/// H1 0x1fb648 in units/tick², H2 0x4455d8 in units/second².
pub const SOURCE_GRAVITY: f32 = 0.0035651792 * 900.0;
pub const GRAVITY_RATIO: f32 = SOURCE_GRAVITY / 4.1712594;
pub const CONTACT_END: u32 = POWER_DATA;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelDrag {
    /// Low two bits select the friction axis. Bit two enables drive.
    pub mode: u32,
    /// Point's share of total mass times ground friction, in inverse seconds.
    pub rate: f32,
    pub parallel: f32,
    pub perpendicular: f32,
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// H1 0x12360 accepts either endpoint order. For equal endpoints the
/// descending branch returns one at the boundary.
fn grip(z: f32, cutoff: f32, full: f32) -> f32 {
    if full == cutoff {
        return if z <= full { 1.0 } else { 0.0 };
    }
    ((z - cutoff) / (full - cutoff)).clamp(0.0, 1.0)
}

impl WheelDrag {
    /// H1 0x83e30..0x83f9f and 0x80f80. Slope and alignment scale
    /// powered motion only. Neither friction axis has a slip threshold.
    pub fn force(
        self,
        velocity: [f32; 3],
        forward: [f32; 3],
        up: [f32; 3],
        normal: [f32; 3],
        speed: f32,
        full: f32,
        cutoff: f32,
        mass: f32,
    ) -> [f32; 3] {
        let drive = if self.mode & 4 != 0 {
            speed * (grip(normal[2], cutoff, full) * dot(up, normal).clamp(0.0, 1.0)).powi(2)
        } else {
            0.0
        };
        let slip = std::array::from_fn(|i| forward[i] * drive - velocity[i]);
        let vn = dot(slip, normal);
        let force = std::array::from_fn(|i| (slip[i] - normal[i] * vn) * self.rate * mass);
        let axis = match self.mode & 3 {
            0 => return force,
            1 => forward,
            2 => cross(up, forward),
            _ => up,
        };
        let along = dot(axis, force);
        std::array::from_fn(|i| {
            let parallel = along * axis[i];
            parallel * self.parallel + (force[i] - parallel) * self.perpendicular
        })
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::*;

    #[test]
    fn velocity_error_on_each_axis_and_no_force_at_wheel_speed() {
        let w = WheelDrag {
            mode: 5,
            rate: 2.0,
            parallel: 0.75,
            perpendicular: 0.5,
        };
        let (f, up) = ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let run = |v, speed| w.force(v, f, up, up, speed, 0.7, 0.5, 4.0);
        assert_eq!(run([1.0, 2.0, -8.0], 3.0), [12.0, -8.0, 0.0]);
        assert_eq!(run([3.0, 0.0, 9.0], 3.0), [0.0; 3]);
        assert_eq!(run([0.0, 2.0, 0.0], 0.0), [0.0, -8.0, 0.0]);
        assert_eq!(run([-10.0, -20.0, 0.0], 0.0), [60.0, 80.0, 0.0]);
    }

    #[test]
    fn slope_scales_only_drive_and_axis_zero_ignores_directional_coefficients() {
        let w = WheelDrag {
            mode: 4,
            rate: 1.0,
            parallel: 0.0,
            perpendicular: 0.0,
        };
        let normal = [0.0, 0.8, 0.6];
        let forward = [1.0, 0.0, 0.0];
        let up = [0.0, 0.0, 1.0];
        let drag = w.force(forward, forward, up, normal, 10.0, 0.7, 0.5, 1.0);
        // Half slope grip and 0.6 alignment give 0.09 of the drive speed.
        assert!((drag[0] + 0.1).abs() < 1e-5);
        assert_eq!(grip(0.5, 0.5, 0.5), 1.0);
        assert_eq!(grip(0.5f32.next_up(), 0.5, 0.5), 0.0);
        assert_eq!(grip(0.5, 1.0, 0.0), 0.5);
    }
}
