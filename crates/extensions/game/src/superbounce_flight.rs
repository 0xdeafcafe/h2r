//! The superbounce flight model (docs/game/superbounce.md, "The flight
//! model"): a body in free flight, stepped a game tick at a time the way
//! the engine steps it. The model is `superbounce::flight`, shared with
//! the bots' routes and flight net (`bots::bounce_route`), so a bot and
//! the HUD agree on where a jump comes down; this module is its name in
//! the game. Everything here that predicts a landing flies it: the
//! approach guide (`superbounce_guide`), the fall assist and the readout's
//! landing (`superbounce_assist`), and the second bounce
//! (`superbounce_second`).
//!
//! The tests below hold the model to the sim's fall model
//! (`sim_bench::drops`) and to the bots' jump (`bots::bounce_route`).

pub use superbounce::flight::*;

#[cfg(test)]
mod tests {
    use sim_bench::drops::{FALL_CLEARANCE, FALL_OFFSET, FALL_STILL_ROWS, FALL_V0, fall_model};

    use super::*;

    fn near(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() <= eps, "{a} is not within {eps} of {b}");
    }

    fn flat(h: f32) -> impl Fn([f32; 2]) -> f32 {
        move |_| h
    }

    /// The numbers are the measured ones.
    #[test]
    fn the_model_is_the_measured_one() {
        near(AIR_ACCEL * TICK, 0.035, 1e-6);
        assert_eq!((AIR_TOP, DEAD_ZONE), (1.2, 0.2));
        near(GRAVITY * TICK, 0.13904, 1e-4);
        // One gravity: the sim's fall model's, which the bots fly too.
        assert_eq!(GRAVITY, sim_bench::drops::FALL_GRAVITY);
        assert_eq!(bots::bounce_route::FALL_GRAVITY, GRAVITY);
        assert_eq!(
            (bots::bounce_route::AIR_ACCEL, bots::bounce_route::AIR_TOP, bots::bounce_route::MIN_STICK),
            (AIR_ACCEL, AIR_TOP, DEAD_ZONE)
        );
    }

    /// The video's run-jump: off the floor at 2.0 units/s with the stick
    /// held forward, the speed drops 0.035 a tick to 1.2 and holds there;
    /// with the stick let go it drops the same way to a stop and stays.
    #[test]
    fn a_run_jump_brakes_to_the_top_with_the_stick_and_to_a_stop_without() {
        let mut v = [2.0, 0.0];
        for k in 1..=60 {
            v = air_step(v, [1.0, 0.0]);
            let want = (2.0 - 0.035 * k as f32).max(AIR_TOP);
            near(v[0], want, 1e-4);
            assert_eq!(v[1], 0.0);
        }
        let mut v = [2.0, 0.0];
        for k in 1..=80 {
            v = air_step(v, [0.0, 0.0]);
            near(v[0], (2.0 - 0.035 * k as f32).max(0.0), 1e-4);
        }
        assert_eq!(v, [0.0, 0.0]);
        // From rest a full stick pushes at the same rate, toward the top.
        near(air_step([0.0; 2], [0.0, -1.0])[1], -0.035, 1e-6);
        // A stick past the unit disc is the full stick that way.
        assert_eq!(air_step([0.0; 2], [3.0, 0.0]), air_step([0.0; 2], [1.0, 0.0]));
    }

    /// A stick inside the dead zone is none: the body brakes.
    #[test]
    fn a_dead_zone_stick_is_no_stick() {
        assert_eq!(effective([0.15, 0.0]), [0.0, 0.0]);
        assert_eq!(effective([0.0, 0.199]), [0.0, 0.0]);
        assert_eq!(effective([0.2, 0.0]), [0.2, 0.0]);
        assert_eq!(effective([f32::NAN, 0.0]), [0.0, 0.0]);
        assert_eq!(air_step([0.5, 0.0], [0.1, 0.0]), air_step([0.5, 0.0], [0.0; 2]));
        near(air_step([0.5, 0.0], [0.1, 0.0])[0], 0.465, 1e-6);
    }

    /// The throttle's forward is the facing and its left is the facing
    /// turned a quarter left; the keeping stick is the speed over the top.
    #[test]
    fn the_stick_is_in_world_axes() {
        use std::f32::consts::FRAC_PI_2;
        let same = |a: [f32; 2], b: [f32; 2]| {
            near(a[0], b[0], 1e-6);
            near(a[1], b[1], 1e-6);
        };
        same(held([1.0, 0.0], 0.0), [1.0, 0.0]);
        same(held([0.0, 1.0], 0.0), [0.0, 1.0]);
        same(held([1.0, 0.0], FRAC_PI_2), [0.0, 1.0]);
        same(held([0.0, 1.0], FRAC_PI_2), [-1.0, 0.0]);
        same(held([1.0, 1.0], 0.0), [std::f32::consts::FRAC_1_SQRT_2; 2]);
        assert_eq!(held([f32::NAN, 0.0], 0.0), [0.0, 0.0]);
        assert_eq!(held([1.0, 0.0], f32::INFINITY), [0.0, 0.0]);
        same(keep([0.6, 0.0]), [0.5, 0.0]);
        same(keep([2.4, 0.0]), [1.0, 0.0]);
        assert_eq!(keep([f32::NAN, 0.0]), [0.0, 0.0]);
        // Under the keeping stick a body at or under the top keeps its
        // speed; one over it slows.
        near(air_step([0.6, 0.0], keep([0.6, 0.0]))[0], 0.6, 1e-6);
        near(air_step([2.0, 0.0], keep([2.0, 0.0]))[0], 1.965, 1e-6);
    }

    /// The video's run-jump, landed: off a floor at 2.0 units/s and the
    /// jump's 3.08 up, onto the same height. The vertical comes back down
    /// on row 43 either way. With the stick held forward the speed falls
    /// 0.035 a tick for 22 ticks (35.145 / 30 units) and holds 1.2 for 21
    /// (25.2 / 30): it lands 2.0115 out. Let go, it brakes all 43 ticks
    /// (52.89 / 30): 1.763 out. Coasting says 2.0 × 43 / 30 = 2.867, 0.86
    /// too far.
    #[test]
    fn a_braking_run_jump_lands_well_short_of_the_coasting_one() {
        let v = [2.0, 0.0, bots::bounce_route::JUMP_VZ];
        let on = hold([0.0; 3], v, [1.0, 0.0], flat(0.0)).unwrap();
        assert_eq!(on.ticks, 43);
        near(on.row[0], 2.0115, 2e-3);
        near(on.velocity[0], AIR_TOP, 1e-4);
        near(on.speed, 2.898, 2e-3);
        let off = hold([0.0; 3], v, [0.0, 0.0], flat(0.0)).unwrap();
        assert_eq!(off.ticks, 43);
        near(off.row[0], 1.763, 2e-3);
        near(off.velocity[0], 0.495, 1e-3);
        let coasting = 2.0 * 43.0 * TICK;
        assert!(coasting - on.row[0] > 0.8, "{on:?}");
    }

    /// A plumb drop is the sim's fall model: a drop from each of its
    /// heights, taken up where its fall starts (the still rows over, at
    /// its first speed), touches down from the model's row at its speed.
    #[test]
    fn a_plumb_drop_is_the_sims_fall_model() {
        for height in [2.43, 3.45, 3.65, 3.82, 4.5, 5.5] {
            let (n, v) = fall_model(height);
            let l = hold(
                [0.0, 0.0, height + FALL_OFFSET],
                [0.0, 0.0, -FALL_V0],
                [0.0; 2],
                flat(FALL_CLEARANCE),
            )
            .unwrap();
            assert_eq!(l.ticks, n - FALL_STILL_ROWS, "{height}");
            near(l.speed, v, 1e-3);
            assert_eq!((l.row[0], l.row[1], l.velocity), (0.0, 0.0, [0.0; 2]));
            let f = fall(height + FALL_OFFSET, -FALL_V0, FALL_CLEARANCE).unwrap();
            assert_eq!((f.ticks, f.speed), (l.ticks, l.speed));
        }
    }

    /// A rising flight lands only on its way down: onto a surface over
    /// the start and under the apex (1.137 up for a jump's 3.08), after
    /// the top; never onto one over the apex, nor one it starts under.
    #[test]
    fn a_rising_flight_lands_on_the_way_down() {
        let up = [0.0, 0.0, bots::bounce_route::JUMP_VZ];
        let l = hold([0.0; 3], up, [0.0; 2], flat(0.5)).unwrap();
        // The top is on row 22 (3.08 / 0.139).
        assert!(l.ticks > 22 && l.row[2] > 0.5, "{l:?}");
        assert!(l.row[2] - (l.speed + GRAVITY * TICK) * TICK <= 0.5);
        assert_eq!(hold([0.0; 3], up, [0.0; 2], flat(1.5)), None);
        assert_eq!(hold([0.0; 3], [0.0; 3], [0.0; 2], flat(1.0)), None);
        // Falling from rest onto the floor it stands a hair over.
        assert_eq!(hold([0.0, 0.0, 0.001], [0.0; 3], [0.0; 2], flat(0.0)).unwrap().ticks, 0);
    }

    /// The pursuit gets to a point inside what the stick can do, and says
    /// how far short it ends of one outside it. A drop of 4.0 from rest
    /// takes 41 ticks; a full stick from rest covers 0.974 in them.
    #[test]
    fn the_pursuit_reaches_inside_the_envelope_and_misses_outside_it() {
        for aim in [[0.5, 0.0], [0.9, 0.0], [0.3, 0.4], [-0.6, -0.2]] {
            let p = pursue([0.0, 0.0, 4.0], [0.0; 3], aim, flat(0.0)).unwrap();
            assert!(p.miss < 0.02, "{aim:?}: {p:?}");
            assert_eq!(p.landing.ticks, 41);
            // The first stick points at the aim.
            let s = p.stick_now;
            near(s[0] * aim[1] - s[1] * aim[0], 0.0, 1e-4);
            assert!(s[0] * aim[0] + s[1] * aim[1] > 0.0);
        }
        let out = pursue([0.0, 0.0, 4.0], [0.0; 3], [3.0, 0.0], flat(0.0)).unwrap();
        near(out.miss, 3.0 - 0.974, 5e-3);
        assert_eq!(out.stick_now, [1.0, 0.0]);
        // Moving away from the aim, the reach is smaller.
        let away = pursue([0.0, 0.0, 4.0], [-1.0, 0.0, 0.0], [0.9, 0.0], flat(0.0)).unwrap();
        assert!(away.miss > 0.5, "{away:?}");
        // Onto nothing, or toward no number: none.
        assert_eq!(pursue([0.0, 0.0, -1.0], [0.0; 3], [0.5, 0.0], flat(0.0)), None);
        assert_eq!(pursue([0.0, 0.0, 4.0], [0.0; 3], [f32::NAN, 0.0], flat(0.0)), None);
    }

    /// A number that is not one has no landing.
    #[test]
    fn nan_in_is_no_landing() {
        assert_eq!(hold([f32::NAN, 0.0, 2.0], [0.0; 3], [0.0; 2], flat(0.0)), None);
        assert_eq!(hold([0.0, 0.0, 2.0], [0.0, f32::INFINITY, 0.0], [0.0; 2], flat(0.0)), None);
        assert_eq!(fall(f32::NAN, 0.0, 0.0), None);
        // A NaN stick is no stick.
        let l = hold([0.0, 0.0, 2.0], [0.0; 3], [f32::NAN, 1.0], flat(0.0)).unwrap();
        assert_eq!((l.row[0], l.row[1]), (0.0, 0.0));
    }
}
