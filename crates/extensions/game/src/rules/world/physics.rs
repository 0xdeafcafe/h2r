//! Physics events on the host (docs/game/rules-world-api.md, "Physics"):
//! what the superbounce medal chain saw of each player's physics this
//! tick (`superbounce_chain::Physics`, from `Medals::step`), as the VM's
//! inputs.
//!
//! - A touchdown is `Input::Touchdown`, the fall before and the vertical
//!   speed after rounded to the nearest `fixed`.
//! - A landing judged a bounce is `Input::Launch`, with its strip and its
//!   launch rounded **up** to the next `fixed` step, so a script's
//!   `speed > 16.0` is true exactly when the host's `speed > 16.0` is.
//! - The end of a run of bounces is `Input::Grounded`.
//!
//! A chain step sees at most one touchdown, one launch and one end of a
//! run for its player, so a tick has at most 16 of each: the events' cap
//! (`::rules::Event::worst_per_tick`). They are delivered while the
//! superbounce mode steps the chain, which is while a script that
//! requires `hud.superbounce` runs.

use ::rules::Input;

use super::objects::to_fixed;
use crate::superbounce_chain::Physics;

/// A speed (units/s) as a `fixed` rounded up to the next 1/65536: for any
/// constant `c` a script can write, `speed > c` reads as `v > c` does. A
/// speed that is not finite reads 0.
pub fn to_fixed_up(v: f32) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    (f64::from(v) * 65536.0)
        .ceil()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// The VM's input for player `slot`'s physics event `p`.
pub fn input(slot: u8, p: Physics) -> Input {
    match p {
        Physics::Touchdown { down, up } => Input::Touchdown {
            player: slot,
            down: to_fixed(down),
            up: to_fixed(up),
        },
        Physics::Launch { spot, speed } => Input::Launch {
            player: slot,
            spot,
            speed: to_fixed_up(speed),
        },
        Physics::Grounded => Input::Grounded(slot),
    }
}

/// One tick's physics events, in the chain's order (slot by slot, in
/// order within a slot).
pub fn inputs(physics: &[(u8, Physics)]) -> Vec<Input> {
    physics.iter().map(|(slot, p)| input(*slot, *p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::ty::fixed;

    #[test]
    fn a_launch_rounds_up_so_a_strict_test_against_a_constant_holds() {
        let sixteen = fixed::from_int(16);
        assert_eq!(to_fixed_up(16.0), sixteen, "at the speed is not over it");
        assert!(to_fixed_up(16.000_002) > sixteen, "just over is over");
        assert!(to_fixed_up(15.999_998) <= sixteen);
        assert_eq!(to_fixed_up(f32::NAN), 0);
        assert_eq!(to_fixed_up(f32::INFINITY), 0);
    }

    #[test]
    fn each_physics_event_is_its_input_for_its_slot() {
        let got = inputs(&[
            (
                3,
                Physics::Touchdown {
                    down: 6.0,
                    up: 12.0,
                },
            ),
            (
                3,
                Physics::Launch {
                    spot: Some(77),
                    speed: 12.5,
                },
            ),
            (
                5,
                Physics::Launch {
                    spot: None,
                    speed: 9.0,
                },
            ),
            (3, Physics::Grounded),
        ]);
        assert_eq!(
            got,
            [
                Input::Touchdown {
                    player: 3,
                    down: fixed::from_int(6),
                    up: fixed::from_int(12)
                },
                Input::Launch {
                    player: 3,
                    spot: Some(77),
                    speed: fixed::from_int(12) + fixed::ONE / 2
                },
                Input::Launch {
                    player: 5,
                    spot: None,
                    speed: fixed::from_int(9)
                },
                Input::Grounded(3),
            ]
        );
    }
}
