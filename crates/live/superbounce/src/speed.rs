//! The one speed model every seam shares (docs/game/superbounce.md,
//! "Speed"). A primed landing on a seam bounces when it touches down at
//! [`BOUNCE_SPEED`] or faster, the same threshold for every seam; the speed
//! over it changes how high the launch goes, not whether it bounces
//! ([`launch`]). Nothing here is per spot: a spot carries a `need` or a
//! `fall` only when a game run measured one ([`need`], [`fall_of`]).
//!
//! The one place to tune the model is this file's constants.

use crate::{Kind, STANDING, Spot};

/// The touchdown speed (world units a second, downward) every seam needs
/// for a primed landing to bounce.
// TODO(sim): measure the threshold with the sim's training (`h2 sim
// train`, drops from a range of heights onto one seam) and replace this.
// 5.4 is the slowest touchdown measured to bounce: the recipe's 2.43 drop
// at Ascension, stepped run tick 6753 (docs/game/superbounce.md, "The
// touchdown tick"). A standing jump lands at 3 and never bounced, so the
// real threshold is between the two.
pub const BOUNCE_SPEED: f32 = 5.4;
/// Gravity (world units a second squared), fitted from the recipe: a
/// standing jump lands at 3, a 2.43 drop at 5.4.
pub const GRAVITY: f32 = 4.15;
/// A standing jump's touchdown speed on the floor it left (units/s).
pub const JUMP_LAND: f32 = 3.0;
/// Launch (units/s up) gained per unit/s of touchdown speed over
/// [`BOUNCE_SPEED`].
// TODO(sim): unmeasured. One measured bounce gives no slope; 1 is the
// plain assumption that the excess comes back out.
pub const EXCESS_GAIN: f32 = 1.0;
/// A typical bounce's launch (units/s up): the measured ones are 4 to 6
/// at most spots, 13.6 at the best Ascension landing (`ascension.verified`
/// and `ascension.learned`, `best=13.60`). The top of the typical range
/// sets how much open air a seam needs over it ([`cover`]).
pub const TYPICAL_LAUNCH: f32 = 6.0;

/// The touchdown speed `spot` needs: its measured `need`, else the one
/// threshold, [`BOUNCE_SPEED`].
pub fn need(spot: &Spot) -> f32 {
    if spot.need > 0.0 {
        spot.need
    } else {
        BOUNCE_SPEED
    }
}

/// The touchdown speed after a drop of `fall` (world units) from a
/// standing jump: `√(3² + 2 × 4.15 × fall)`.
pub fn touchdown(fall: f32) -> f32 {
    (JUMP_LAND * JUMP_LAND + 2.0 * GRAVITY * fall.max(0.0)).sqrt()
}

/// The drop (world units) over a seam that touches down at
/// [`BOUNCE_SPEED`]: 2.43, the recipe's drop at Ascension.
pub fn fall() -> f32 {
    ((BOUNCE_SPEED * BOUNCE_SPEED - JUMP_LAND * JUMP_LAND) / (2.0 * GRAVITY)).max(0.0)
}

/// The drop `spot` needs: its measured `fall`, else [`fall`].
pub fn fall_of(spot: &Spot) -> f32 {
    if spot.fall > 0.0 { spot.fall } else { fall() }
}

/// The launch (units/s up) from a primed landing at `speed_in` on a spot
/// whose launch at the threshold is `base` (the scan's estimate from its
/// crease): `base` plus [`EXCESS_GAIN`] × the speed over [`BOUNCE_SPEED`].
/// `None` under the threshold: no bounce.
pub fn launch(base: f32, speed_in: f32) -> Option<f32> {
    (speed_in >= BOUNCE_SPEED).then(|| base + EXCESS_GAIN * (speed_in - BOUNCE_SPEED))
}

/// How high (world units) a launch of `v` units/s climbs.
pub fn climb(v: f32) -> f32 {
    v.max(0.0).powi(2) / (2.0 * GRAVITY)
}

/// The open air (world units) a seam needs over its floor to launch
/// anyone: the climb of a [`TYPICAL_LAUNCH`] (4.34) plus the standing
/// body (0.725), 5.06. A seam with structure overhead nearer than this (a
/// low ceiling, indoors) is no bounce spot (`bounce::covered`). The best
/// measured launch, 13.6, climbs 22 units and would drop seams under any
/// roof; the typical one is the bounce a player gets.
pub fn cover() -> f32 {
    climb(TYPICAL_LAUNCH) + STANDING
}

/// `spots` with the model filled in where nothing was measured: each
/// bounce spot's `need` ([`need`]) and `fall` ([`fall_of`]). What the
/// game's HUD, marks and assist read (`superbounce_mode`).
pub fn apply(spots: &mut [Spot]) {
    for s in spots.iter_mut().filter(|s| s.kind == Kind::Bounce) {
        s.need = need(s);
        s.fall = fall_of(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_threshold_for_every_seam_and_the_excess_sets_the_height() {
        let scanned = Spot {
            kind: Kind::Bounce,
            launch: 17.85,
            ..Spot::default()
        };
        assert_eq!(need(&scanned), BOUNCE_SPEED);
        // The recipe's drop touches down at the threshold.
        assert!((fall() - 2.43).abs() < 0.01, "{}", fall());
        assert!((touchdown(fall()) - BOUNCE_SPEED).abs() < 1e-4);
        assert!((touchdown(0.0) - JUMP_LAND).abs() < 1e-6);
        // Under it, no bounce; over it, a higher launch, not a different
        // answer.
        assert_eq!(launch(scanned.launch, 5.0), None);
        let at = launch(scanned.launch, BOUNCE_SPEED).unwrap();
        let over = launch(scanned.launch, BOUNCE_SPEED + 2.0).unwrap();
        assert!((at - 17.85).abs() < 1e-5 && over > at);
        assert!(climb(over) > climb(at));
        // A measured need or fall is the spot's own.
        let measured = Spot {
            need: 4.0,
            fall: 1.0,
            ..scanned
        };
        assert_eq!((need(&measured), fall_of(&measured)), (4.0, 1.0));
        // The game's list gets the model; crouch spots are left alone.
        let crouch = Spot {
            kind: Kind::Crouch,
            ..Spot::default()
        };
        let mut v = [scanned, measured, crouch];
        apply(&mut v);
        assert_eq!((v[0].need, v[0].fall), (BOUNCE_SPEED, fall()));
        assert_eq!((v[1].need, v[1].fall), (4.0, 1.0));
        assert_eq!(v[2], crouch);
        // The open air a seam needs: a typical launch's climb and the body.
        assert!((cover() - 5.06).abs() < 0.01, "{}", cover());
    }
}
