//! The brain: what a bot sees in, controller input out (docs/game/bots.md,
//! section 3). Pure: no guest memory, no clock but the tick it is handed.
//!
//! Stage 1 is the trivial brain. It turns toward the nearest living enemy,
//! walks at it, and holds the trigger while that enemy is near the
//! crosshair and in range. It has no line of sight, no path and no aim
//! model yet (stages 2 and 3).
//!
//! Coordinates are the game's world units, Z up. A unit's aim is a unit
//! vector in world space (unit `+0x168`, docs/live/live-gamestate.md).

use input::host::HostPadState;

use crate::arms::{Arms, Pickup};

pub type Vec3 = [f32; 3];

/// One player as the bot sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    /// Which player (its slot in the players array), to follow it from
    /// tick to tick.
    pub id: u32,
    /// The unit's origin.
    pub position: Vec3,
    pub team: i16,
}

/// The bot itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Me {
    pub position: Vec3,
    /// Where the bot aims: a unit vector.
    pub aim: Vec3,
    pub team: i16,
    /// Shield and body vitality, 0 to 1 (object `+0xf0`, `+0xec`). The
    /// shield can go over 1 with an overshield.
    pub shield: f32,
    pub health: f32,
    /// The shield a spawn gives it (`0x1588b0`, the spawn writes it at
    /// `0x14cc79`): the variant's shields times its handicap's factor. 0
    /// with no shields; over 1 with overshields.
    pub shield_full: f32,
    /// What it carries (docs/game/bots.md section 3.6); `None` when not
    /// read, and then the brain never acts on ammunition.
    pub arms: Option<Arms>,
}

impl Default for Me {
    fn default() -> Self {
        Me {
            position: [0.0; 3],
            aim: [1.0, 0.0, 0.0],
            team: 0,
            shield: 1.0,
            health: 1.0,
            shield_full: 1.0,
            arms: None,
        }
    }
}

/// Extra observations used by the decision layer. A driver without these
/// can still run the ordinary weapon, objective and targeting decisions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observations {
    /// Strictly leading on the public scoreboard.
    pub ahead: bool,
    /// Own player slot and team, including while waiting to respawn.
    pub player: Option<(u32, i16)>,
    /// Cues on living players. Enemy cues only inform decisions in view.
    pub contacts: Vec<Contact>,
    /// Approximate horizontal world positions of the hostile motion
    /// tracker dots, nearest first, at most four, each at the observer's
    /// own height. A dot carries no player identity and no height.
    pub radar: Vec<Vec3>,
    /// The tracker disc's horizontal radius while the tracker shows.
    pub radar_range: Option<f32>,
    /// The player slots (`perceive::player_bit`) moving faster than the
    /// motion tracker's threshold, and those that fired within
    /// `sound::HEARD_TICKS`: what shows through active camouflage
    /// (docs/game/bots.md section 3.1, "Visual range"). Read for every
    /// living player; perception keeps only what it sees.
    pub moving: u16,
    pub firing: u16,
    /// Loose frag/plasma equipment that the observer can see.
    pub ground_grenades: Vec<(Vec3, crate::grenades::Kind)>,
    /// An empty successful pickup scan can indicate that a weapon was taken.
    pub pickups_scanned: bool,
    /// Public simulation ticks per second for estimating placement service.
    pub service_rate: Option<u16>,
    /// Power weapon instances identified during the existing object scan.
    pub held_power: Vec<u32>,
    /// Recent successful pickup events, matched to witnessed removals.
    pub takes: Vec<crate::decisions::supplies::Take>,
    /// Living unit handles and their visible weapon objects, for recognizing
    /// a pickup without transferring a memory to a new life or player.
    pub supply_players: Vec<crate::decisions::supplies::Player>,
    /// Occupied power weapon and power-up placements. None means the host
    /// could not check the ground this tick. An empty scan is meaningful.
    pub supplies: Option<Vec<crate::decisions::supplies::Supply>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    /// Model of the weapon visibly in hand.
    pub weapon: Option<u32>,
    pub id: u32,
    pub aim: Vec3,
    pub shield: Option<f32>,
    /// A visible reload can create a window to push. The guest observer
    /// leaves this false until it can recognize the animation.
    pub empty_magazine: bool,
    /// The biped is requesting crouch this tick.
    pub crouching: bool,
}

/// An outside order (docs/game/rules-world-api.md, "Bots"): a hint a rules
/// script gives a phantom's brain on the host. The order is the bot's goal
/// while it stands, in place of the objective's; the brain fights from it
/// as it would from any goal. It returns nothing to whoever gave it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Order {
    /// `bot_go`: walk to this point.
    Go(Vec3),
    /// `bot_hold`: stay within the radius of this point and fight from it.
    Hold(Vec3, f32),
    /// `bot_follow`: stay near this player (its slot, as [`Seen::id`]).
    Follow(u32),
    /// `bot_attack`: kill this player (its slot, as [`Seen::id`]).
    Attack(u32),
}

impl Order {
    /// The goal the order asks ([`crate::objective::Goal`]), with a
    /// followed or attacked player's position from the roster. `None`
    /// while that player has no living unit: the order asks nothing then,
    /// and the bot plays on.
    pub fn goal(&self, others: &[Seen]) -> Option<crate::objective::Goal> {
        use crate::objective::Goal;
        let at = |id: u32| others.iter().find(|o| o.id == id).map(|o| o.position);
        match *self {
            Order::Go(at) => Some(Goal::Go(at)),
            Order::Hold(at, radius) => Some(Goal::Hold(at, radius)),
            Order::Follow(q) => at(q).map(Goal::Escort),
            Order::Attack(q) => at(q).map(|p| Goal::Hunt(q, p)),
        }
    }
}

/// What the bot knows this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Perception {
    pub observations: Observations,
    /// `None` while the bot has no living unit (dead, or not in a game).
    pub me: Option<Me>,
    /// The other players with living units, and in Firefight the AI's
    /// actors, with ids from [`crate::firefight::ACTOR_ID_BASE`].
    pub others: Vec<Seen>,
    /// A team game: same-team players are friends. In free-for-all
    /// everyone is an enemy.
    pub teams: bool,
    /// The game's tick count (30 a second).
    pub tick: u32,
    /// The weapons lying in the world.
    pub pickups: Vec<Pickup>,
    /// The gametype and its objective (docs/game/bots.md section 3.7):
    /// flags, balls, bombs and the hill. `None` in Slayer, or when it
    /// could not be read; the bot then plays as in Slayer.
    pub objective: Option<crate::objective::Objective>,
}

/// The trivial brain's tuning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Holds the trigger while the target is within this angle of the aim
    /// (radians).
    pub fire_cone: f32,
    /// ...and this close (world units; one is about 3 m).
    pub fire_range: f32,
    /// Stops walking this close.
    pub stop_range: f32,
    /// Does not walk while the target is further than this off the aim
    /// (radians): turn first.
    pub walk_cone: f32,
    /// Right stick deflection per radian of aim error.
    pub turn_gain: f32,
    /// The eye's height over the unit's origin, and the target point's.
    pub eye: f32,
    pub chest: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            fire_cone: 15f32.to_radians(),
            fire_range: 10.0,
            stop_range: 1.0,
            walk_cone: 60f32.to_radians(),
            turn_gain: 2.5,
            eye: 0.6,
            chest: 0.4,
        }
    }
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn len(a: Vec3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The angle in -pi..pi.
fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let a = (a + std::f32::consts::PI).rem_euclid(t) - std::f32::consts::PI;
    if a < -std::f32::consts::PI { a + t } else { a }
}

/// Yaw (counter-clockwise from +X, seen from above) and pitch (up
/// positive) of `v`.
fn angles(v: Vec3) -> (f32, f32) {
    let flat = (v[0] * v[0] + v[1] * v[1]).sqrt();
    (v[1].atan2(v[0]), v[2].atan2(flat))
}

/// The nearest enemy, as an index into `p.others`.
pub fn target(p: &Perception) -> Option<usize> {
    let me = p.me?;
    p.others
        .iter()
        .enumerate()
        .filter(|(_, o)| !p.teams || o.team != me.team)
        .map(|(i, o)| (i, len(sub(o.position, me.position))))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// This tick's pad.
pub fn step(p: &Perception, k: &Params) -> HostPadState {
    let mut pad = HostPadState::default();
    let (Some(me), Some(t)) = (p.me, target(p)) else {
        return pad;
    };
    let other = p.others[t];
    let eye = [me.position[0], me.position[1], me.position[2] + k.eye];
    let aim_at = [
        other.position[0],
        other.position[1],
        other.position[2] + k.chest,
    ];
    let to = sub(aim_at, eye);
    let dist = len(to);
    if dist < 1e-4 {
        return pad;
    }
    let (want_yaw, want_pitch) = angles(to);
    let (yaw, pitch) = angles(me.aim);
    let yaw_err = wrap(want_yaw - yaw);
    let pitch_err = want_pitch - pitch;
    // The right stick: +X turns right (clockwise from above), +Y looks up.
    pad.right_stick = (
        (-yaw_err * k.turn_gain).clamp(-1.0, 1.0),
        (pitch_err * k.turn_gain).clamp(-1.0, 1.0),
    );
    if dist > k.stop_range && yaw_err.abs() < k.walk_cone {
        pad.left_stick = (0.0, 1.0);
    }
    let off = dot(to, me.aim) / (dist * len(me.aim).max(1e-6));
    if off.clamp(-1.0, 1.0).acos() < k.fire_cone && dist < k.fire_range {
        pad.right_trigger = 1.0;
    }
    pad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(aim: Vec3) -> Me {
        Me {
            position: [0.0, 0.0, 0.0],
            aim,
            team: 0,
            ..Me::default()
        }
    }
    fn seen(x: f32, y: f32, team: i16) -> Seen {
        Seen {
            id: 0,
            position: [x, y, 0.2],
            team,
        }
    }
    fn look(me: Me, others: Vec<Seen>, teams: bool) -> HostPadState {
        step(
            &Perception {
                observations: Default::default(),
                me: Some(me),
                others,
                teams,
                tick: 0,
                pickups: vec![],
                objective: None,
            },
            &Params::default(),
        )
    }

    #[test]
    fn with_no_unit_or_no_enemy_the_pad_is_idle() {
        let idle = HostPadState::default();
        assert_eq!(step(&Perception::default(), &Params::default()), idle);
        assert_eq!(look(me([1.0, 0.0, 0.0]), vec![], false), idle);
        // A teammate is not a target in a team game.
        assert_eq!(
            look(me([1.0, 0.0, 0.0]), vec![seen(3.0, 0.0, 0)], true),
            idle
        );
    }

    #[test]
    fn an_enemy_dead_ahead_and_near_is_walked_at_and_shot() {
        let pad = look(me([1.0, 0.0, 0.0]), vec![seen(5.0, 0.0, 1)], true);
        assert_eq!(pad.left_stick, (0.0, 1.0));
        assert_eq!(pad.right_trigger, 1.0);
        assert!(pad.right_stick.0.abs() < 0.05, "{:?}", pad.right_stick);
    }

    #[test]
    fn everyone_is_an_enemy_in_free_for_all() {
        let pad = look(me([1.0, 0.0, 0.0]), vec![seen(5.0, 0.0, 0)], false);
        assert_eq!(pad.right_trigger, 1.0);
    }

    #[test]
    fn it_turns_toward_the_target_the_short_way() {
        // Target to the left (+Y with aim +X): counter-clockwise, so the
        // stick goes left.
        let left = look(me([1.0, 0.0, 0.0]), vec![seen(0.0, 5.0, 1)], false);
        assert!(left.right_stick.0 < -0.9, "{:?}", left.right_stick);
        let right = look(me([1.0, 0.0, 0.0]), vec![seen(0.0, -5.0, 1)], false);
        assert!(right.right_stick.0 > 0.9, "{:?}", right.right_stick);
        // Behind and a little left: still left, across the +-pi seam.
        let behind = look(me([-1.0, -0.05, 0.0]), vec![seen(5.0, 0.1, 1)], false);
        assert!(behind.right_stick.0.abs() > 0.9);
        // 90 degrees off: turns, but does not walk or shoot yet.
        assert_eq!(left.left_stick, (0.0, 0.0));
        assert_eq!(left.right_trigger, 0.0);
    }

    #[test]
    fn it_looks_up_at_a_target_above() {
        let up = step(
            &Perception {
                observations: Default::default(),
                me: Some(me([1.0, 0.0, 0.0])),
                others: vec![Seen {
                    id: 0,
                    position: [3.0, 0.0, 3.0],
                    team: 1,
                }],
                teams: false,
                tick: 0,
                pickups: vec![],
                objective: None,
            },
            &Params::default(),
        );
        assert!(up.right_stick.1 > 0.5, "{:?}", up.right_stick);
    }

    #[test]
    fn it_holds_fire_out_of_range_and_stops_when_close() {
        let far = look(me([1.0, 0.0, 0.0]), vec![seen(20.0, 0.0, 1)], false);
        assert_eq!((far.left_stick, far.right_trigger), ((0.0, 1.0), 0.0));
        let close = look(me([1.0, 0.0, 0.0]), vec![seen(0.5, 0.0, 1)], false);
        assert_eq!(close.left_stick, (0.0, 0.0));
    }

    #[test]
    fn the_nearest_enemy_is_the_target() {
        let p = Perception {
            observations: Default::default(),
            me: Some(me([1.0, 0.0, 0.0])),
            others: vec![seen(9.0, 0.0, 1), seen(0.0, 2.0, 0), seen(4.0, 0.0, 1)],
            teams: true,
            tick: 0,
            pickups: vec![],
            objective: None,
        };
        assert_eq!(target(&p), Some(2));
    }

    #[test]
    fn an_order_is_its_goal_with_the_rosters_positions() {
        use crate::objective::Goal;
        let others = vec![seen(7.0, 8.0, 0), seen(3.0, 4.0, 1)];
        assert_eq!(
            Order::Go([1.0, 2.0, 0.0]).goal(&others),
            Some(Goal::Go([1.0, 2.0, 0.0]))
        );
        assert_eq!(
            Order::Hold([3.0, 4.0, 0.0], 2.0).goal(&others),
            Some(Goal::Hold([3.0, 4.0, 0.0], 2.0))
        );
        // `seen` gives id 0: followed or attacked, its position is the goal's.
        assert_eq!(Order::Follow(0).goal(&others), Some(Goal::Escort([7.0, 8.0, 0.2])));
        assert_eq!(
            Order::Attack(0).goal(&others),
            Some(Goal::Hunt(0, [7.0, 8.0, 0.2]))
        );
        // A followed or attacked player with no living unit: nothing asked.
        assert_eq!(Order::Follow(9).goal(&others), None);
        assert_eq!(Order::Attack(9).goal(&others), None);
    }

    #[test]
    fn wrap_stays_in_range() {
        for a in [-10.0f32, -3.2, 0.0, 3.2, 10.0] {
            let w = wrap(a);
            assert!(
                (-std::f32::consts::PI..=std::f32::consts::PI).contains(&w),
                "{a} -> {w}"
            );
            assert!(
                ((w - a) / std::f32::consts::TAU).fract().abs() < 1e-4
                    || ((w - a) / std::f32::consts::TAU).fract().abs() > 0.9999
            );
        }
    }
}
