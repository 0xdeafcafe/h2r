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
