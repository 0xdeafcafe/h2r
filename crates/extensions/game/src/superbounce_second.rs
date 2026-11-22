//! The second bounce's targets (docs/game/superbounce.md, "The second
//! bounce"): in the flight after a bounce, which seams that flight can
//! come down on, the best first.
//!
//! - The flight is the local player's while their chain is live and they
//!   are in the air ([`window`]): from the launch to the next touchdown.
//! - A seam is reached ([`judge`]) when the flight comes down to its
//!   height at the speed it needs or faster, the stick can move the
//!   coasting landing onto it in the time left (the fall assist's air
//!   acceleration, `superbounce_assist::AIR_ACCEL`), and no floor is
//!   over it. A V's arm counts only from across its bisector.
//! - The reached seams are ranked by a cost ([`Ranked::cost`]): how much
//!   of the stick's reach the steer takes, and how little speed there is
//!   to spare. The lowest are kept ([`rank`]), no two on one place.
//!
//! The readout draws them (`superbounce_readout::view`): the target
//! navpoints go on the ranked seams, the ranked line lists them, and the
//! fall row is the best one's line-up. The variant's `second_bounce`
//! option turns it off (`superbounce_options`).

use superbounce::{Spot, Vec3};

use crate::superbounce_assist::{
    AIR_ACCEL, Body, GRAVITY, Ground, HOLD_SLACK, across, aim_at, covered, drift_unit, fall_time,
    predicted_landing, stand_near,
};
use crate::superbounce_chain as chain;
use crate::superbounce_charge::{need_of, slot_miss};
use crate::superbounce_targets::{Mark, Marks, NEAREST, Target, faces};

/// How many seams are ranked: one for each target navpoint.
pub const RANKED: usize = NEAREST;
/// A flight longer than this (seconds) to a seam's height is not ranked:
/// a fall out of the map.
pub const MAX_TIME: f32 = 8.0;
/// What the spare speed weighs in the cost against the steer's share of
/// the reach.
pub const SPEED_WEIGHT: f32 = 0.5;
/// A seam this near a better one's point (world units) is the same place
/// to the eye: one of them is ranked.
pub const APART: f32 = 1.0;

/// A seam the flight can come down on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ranked {
    /// The point of the seam to land on (`superbounce_assist::stand_near`
    /// the coasting landing).
    pub at: Vec3,
    /// World units from the player to it.
    pub distance: f32,
    /// Seconds to touchdown at its height.
    pub time: f32,
    /// From the coasting landing to the point to come down on
    /// (`superbounce_assist::aim_at`: `at`, or a scan's stand led on its
    /// approach side), world units: the way to steer and how far.
    pub steer: [f32; 2],
    /// The coasting landing is within the seam's catch already.
    pub caught: bool,
    /// The seam's drift, a unit vector (zero for none), and the speed
    /// along it the coasting landing has (units/s).
    pub drift: [f32; 2],
    pub across: f32,
    /// How far the stick can move the landing in the time left.
    pub reach: f32,
    /// The touchdown speed the flight gives there, and the speed the seam
    /// needs (units/s).
    pub speed: f32,
    pub need: f32,
    /// Lower is better: the steer's share of the reach (0 to 1), and
    /// [`SPEED_WEIGHT`] times the need over the speed (so 0.5 with none
    /// to spare, less with more).
    pub cost: f32,
}

/// Whether the second bounce's flight is on: the player in slot `local`
/// is in the air and their chain is live, so the next bounce adds to it.
pub fn window(airborne: bool, local: Option<u8>, medals: &chain::View) -> bool {
    airborne
        && local.is_some_and(|slot| {
            medals
                .players
                .iter()
                .any(|(s, r)| *s == slot && r.live && r.chain >= 1)
        })
}

/// The highest the flight gets: the body's height, and while it rises the
/// apex.
pub fn apex(body: &Body) -> f32 {
    let vz = body.velocity[2].max(0.0);
    body.position[2] + vz * vz / (2.0 * GRAVITY)
}

/// `spot` as the flight of `body` reaches it, or `None`: it comes down to
/// the seam's height within [`MAX_TIME`], at the seam's need or faster;
/// the point of the seam nearest the coasting landing is within what the
/// stick can move it in that time (half the air acceleration times the
/// time squared, plus the assist's slack); and no floor under the flight's
/// apex covers the point.
pub fn judge(body: &Body, spot: &Spot, ground: &Ground<'_>) -> Option<Ranked> {
    let [x, y, z] = body.position;
    let vz = body.velocity[2];
    let height = z - spot.stand[2];
    let time = fall_time(height, vz).filter(|t| *t <= MAX_TIME)?;
    let speed = (vz * vz + 2.0 * GRAVITY * height).max(0.0).sqrt();
    let need = need_of(spot);
    if !speed.is_finite() || speed < need {
        return None;
    }
    let landing = predicted_landing([x, y], [body.velocity[0], body.velocity[1]], time);
    let at = stand_near(spot, landing);
    let aim = aim_at(spot, at);
    let steer = [aim[0] - landing[0], aim[1] - landing[1]];
    let miss = steer[0].hypot(steer[1]);
    let reach = 0.5 * AIR_ACCEL * time * time;
    if miss.is_nan() || miss > reach + HOLD_SLACK {
        return None;
    }
    if covered(apex(body), at, ground) {
        return None;
    }
    let share = (miss / (reach + HOLD_SLACK)).clamp(0.0, 1.0);
    let spare = if speed > 0.0 { need / speed } else { 1.0 };
    Some(Ranked {
        at,
        distance: ((at[0] - x).powi(2) + (at[1] - y).powi(2) + (at[2] - z).powi(2)).sqrt(),
        time,
        steer,
        caught: slot_miss(spot, landing) <= 0.0,
        drift: drift_unit(spot.drift),
        across: across(spot, [body.velocity[0], body.velocity[1]]),
        reach,
        speed,
        need,
        cost: share + SPEED_WEIGHT * spare,
    })
}

/// The [`RANKED`] seams of `targets` the flight of `body` reaches at the
/// lowest cost, best first; a V's arm only from across its bisector, and
/// of seams within [`APART`] of each other the better one.
pub fn rank(targets: &[Target], body: &Body, ground: &Ground<'_>) -> [Option<Ranked>; RANKED] {
    let mut reached: Vec<Ranked> = targets
        .iter()
        .filter(|t| faces(t, body.position))
        .filter_map(|t| judge(body, &t.spot, ground))
        .collect();
    reached.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    let mut out = [None; RANKED];
    let mut n = 0;
    for r in reached {
        let near = |o: &Option<Ranked>| {
            o.is_some_and(|o| {
                let d = [o.at[0] - r.at[0], o.at[1] - r.at[1], o.at[2] - r.at[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < APART
            })
        };
        if n == RANKED {
            break;
        }
        if !out.iter().any(near) {
            out[n] = Some(r);
            n += 1;
        }
    }
    out
}

/// `marks` with the ranked seams on the target navpoints, the best first:
/// in the flight they are the targets. With none ranked, `marks` as it
/// is. The tag keeps its navpoint unless a ranked seam is on it.
pub fn over(marks: Marks, ranked: &[Option<Ranked>; RANKED]) -> Marks {
    if ranked.iter().all(Option::is_none) {
        return marks;
    }
    let on_ranked = |m: &Mark| {
        ranked.iter().flatten().any(|r| {
            let d = [r.at[0] - m.at[0], r.at[1] - m.at[1], r.at[2] - m.at[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < APART
        })
    };
    Marks {
        tag: marks.tag.filter(|m| !on_ranked(m)),
        targets: ranked.map(|r| {
            r.map(|r| Mark {
                at: r.at,
                distance: r.distance,
            })
        }),
        tagged: marks.tagged,
    }
}

#[cfg(test)]
mod tests {
    use superbounce::{Arm, Kind};

    use super::*;
    use crate::superbounce_targets::Class;

    fn seam(stand: Vec3) -> Target {
        Target {
            spot: Spot {
                kind: Kind::Bounce,
                edge: [
                    [stand[0], stand[1] - 1.0, stand[2]],
                    [stand[0], stand[1] + 1.0, stand[2]],
                ],
                stand,
                fall: 2.43,
                need: 5.4,
                ..Spot::default()
            },
            class: Class::Slot,
        }
    }

    fn body(position: Vec3, velocity: Vec3) -> Body {
        Body {
            position,
            velocity,
            yaw: 0.0,
            airborne: true,
        }
    }

    fn open() -> Ground<'static> {
        Ground::open()
    }

    #[test]
    fn the_window_is_the_local_players_live_chain_in_the_air() {
        let view = |live, chain| chain::View {
            players: vec![(
                3,
                chain::Record {
                    chain,
                    live,
                    ..chain::Record::default()
                },
            )],
            recent: Vec::new(),
        };
        assert!(window(true, Some(3), &view(true, 1)));
        assert!(window(true, Some(3), &view(true, 4)));
        assert!(!window(false, Some(3), &view(true, 1)), "on the ground");
        assert!(!window(true, Some(3), &view(false, 1)), "the chain ended");
        assert!(!window(true, Some(3), &view(true, 0)));
        assert!(!window(true, Some(2), &view(true, 1)), "another player's");
        assert!(!window(true, None, &view(true, 1)));
        assert!(!window(true, Some(3), &chain::View::EMPTY));
    }

    #[test]
    fn a_seam_is_reached_fast_enough_and_within_the_sticks_reach() {
        // Rising at 8 from the floor: the apex is 8²/(2g) up, and the
        // flight comes back to the floor at 8 units/s, over the need.
        let b = body([0.0, 0.0, 0.0], [1.0, 0.0, 8.0]);
        let time = 2.0 * 8.0 / GRAVITY;
        assert!((apex(&b) - 64.0 / (2.0 * GRAVITY)).abs() < 1e-4);
        // A seam where it comes down coasting: no steer, caught.
        let on = judge(&b, &seam([time, 0.0, 0.0]).spot, &open()).unwrap();
        assert!((on.time - time).abs() < 1e-3 && (on.speed - 8.0).abs() < 1e-3);
        assert!(on.caught && on.steer[0].hypot(on.steer[1]) < 1e-3);
        assert!((on.cost - SPEED_WEIGHT * 5.4 / 8.0).abs() < 1e-3);
        assert!((on.distance - time).abs() < 1e-3);
        // One 2 units further on: steered onto, at a higher cost.
        let off = judge(&b, &seam([time + 2.0, 0.0, 0.0]).spot, &open()).unwrap();
        assert!(!off.caught && (off.steer[0] - 2.0).abs() < 1e-3);
        assert!(off.cost > on.cost);
        // A seam with a drift and no measured landing is steered to on
        // its approach side, and the flight's speed along the drift is
        // kept for the line-up; a measured landing is steered to as it is.
        let mut led = seam([time + 2.0, 0.0, 0.0]).spot;
        led.drift = [2.0, 0.0];
        let l = judge(&b, &led, &open()).unwrap();
        assert!((l.steer[0] - (2.0 - crate::superbounce_assist::AIM_BACK)).abs() < 1e-3);
        assert_eq!(l.at, off.at);
        assert_eq!(l.drift, [1.0, 0.0]);
        assert!((l.across - 1.0).abs() < 1e-6);
        led.landed = true;
        assert!((judge(&b, &led, &open()).unwrap().steer[0] - 2.0).abs() < 1e-3);
        assert_eq!((off.drift, off.across), ([0.0; 2], 0.0));
        assert!((off.reach - 0.5 * AIR_ACCEL * time * time).abs() < 1e-3);
        // One past the stick's reach is not.
        let far = on.reach + HOLD_SLACK + 1.0;
        assert_eq!(judge(&b, &seam([time + far, 0.0, 0.0]).spot, &open()), None);
        // A seam over the apex is never come down on.
        assert_eq!(judge(&b, &seam([1.0, 0.0, 9.0]).spot, &open()), None);
        // A seam just under the apex is, too slowly to bounce.
        let high = apex(&b) - 0.5;
        assert_eq!(judge(&b, &seam([1.0, 0.0, high]).spot, &open()), None);
        // A lower seam is reached faster than the launch.
        let low = judge(&b, &seam([time, 0.0, -3.0]).spot, &open()).unwrap();
        assert!(low.speed > 8.0 && low.time > time);
    }

    #[test]
    fn a_covered_seam_and_a_long_fall_are_not_ranked() {
        let b = body([0.0, 0.0, 6.0], [0.0, 0.0, -1.0]);
        let s = seam([0.0, 0.0, 0.0]).spot;
        assert!(judge(&b, &s, &open()).is_some());
        // A floor 3 over the seam, under the player.
        let slab = |_: [f32; 2], _: f32| Some(3.0);
        let under = Ground {
            floor: &slab,
            ..Ground::open()
        };
        assert_eq!(judge(&b, &s, &under), None);
        // A fall of more than MAX_TIME.
        let deep = seam([0.0, 0.0, -400.0]).spot;
        assert_eq!(judge(&b, &deep, &open()), None);
    }

    #[test]
    fn the_seams_are_ranked_by_cost_and_one_a_place() {
        let b = body([0.0, 0.0, 0.0], [1.0, 0.0, 8.0]);
        let time = 2.0 * 8.0 / GRAVITY;
        let list = [
            seam([time + 3.0, 0.0, 0.0]),
            seam([time, 0.0, 0.0]),
            // The same place as the best: not ranked beside it.
            seam([time + 0.3, 0.0, 0.0]),
            seam([time + 1.5, 0.0, 0.0]),
            // Out of reach.
            seam([time + 400.0, 0.0, 0.0]),
        ];
        let r = rank(&list, &b, &open());
        let xs = r.map(|r| r.map(|r| r.at[0]));
        assert!((xs[0].unwrap() - time).abs() < 1e-3, "{xs:?}");
        assert!((xs[1].unwrap() - (time + 1.5)).abs() < 1e-3, "{xs:?}");
        assert!(r[0].unwrap().cost <= r[1].unwrap().cost);
        // One reached seam leaves the second place empty; none, both.
        let one = rank(&list[1..2], &b, &open());
        assert!(one[0].is_some() && one[1].is_none());
        assert_eq!(rank(&list[4..], &b, &open()), [None, None]);
    }

    #[test]
    fn a_vs_arm_is_ranked_only_from_across_its_bisector() {
        let time = 2.0 * 8.0 / GRAVITY;
        let mut arm = seam([0.0, 0.5, 0.0]);
        arm.spot.arm = Some(Arm {
            pivot: [0.0, 0.0, 0.0],
            bisector: [1.0, 0.0],
            from: [0.0, -1.0],
        });
        let across = body([0.0, -1.0, 0.0], [0.0, 1.5 / time, 8.0]);
        let own = body([0.0, 2.0, 0.0], [0.0, -1.5 / time, 8.0]);
        assert!(rank(&[arm], &across, &open())[0].is_some());
        assert_eq!(rank(&[arm], &own, &open()), [None, None]);
    }

    #[test]
    fn the_ranked_seams_take_the_target_navpoints() {
        let mark = |x: f32| Mark {
            at: [x, 0.0, 0.0],
            distance: x,
        };
        let marks = Marks {
            tag: Some(mark(50.0)),
            targets: [Some(mark(4.0)), Some(mark(9.0))],
            tagged: Some([50.0, 0.0, 0.0]),
        };
        // None ranked: the marks as they are.
        assert_eq!(over(marks, &[None, None]), marks);
        let b = body([0.0, 0.0, 0.0], [1.0, 0.0, 8.0]);
        let time = 2.0 * 8.0 / GRAVITY;
        let r = rank(&[seam([time, 0.0, 0.0])], &b, &open());
        let got = over(marks, &r);
        assert!((got.targets[0].unwrap().at[0] - time).abs() < 1e-3);
        assert_eq!(got.targets[1], None);
        assert_eq!((got.tag, got.tagged), (marks.tag, marks.tagged));
        // A ranked seam on the tag: one navpoint there, the ranked one.
        let tagged = Marks {
            tag: Some(mark(time)),
            ..marks
        };
        assert_eq!(over(tagged, &r).tag, None);
    }
}
