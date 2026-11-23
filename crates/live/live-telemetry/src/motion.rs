//! A unit's frame `stance`, `speed`, one-shot `actions` and `grenade`,
//! worked out by comparing what the unit's memory says now with what it
//! said at the previous frame.
//!
//! Actions come from state changes, not hooks: a weapon datum new to the
//! unit's four slots is a pickup, the held slot moving to a weapon it
//! already carried is a switch, the held weapon's loaded rounds going up
//! while its reserve goes down is a reload, fewer grenades is a throw (more
//! is a pickup), the biped's physics mode going from ground to flying while
//! moving up is a jump, and entering the melee mode is a melee (one mark a
//! swing: dealing the swing's damage marks it only when no frame saw the
//! entry). A unit seen for the first time (a new game, a respawn) has no
//! actions.

use live_proto::game::MAX_FRAME_ACTIONS;

use crate::event::{Motion, ObjectState};
use crate::layout;

/// What a unit looked like at its previous frame.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitMemory {
    pub tick: u64,
    pub pos: [f64; 3],
    pub held: Option<u32>,
    pub motion: Motion,
    /// The tick of the last `melee` action, so one swing is marked once:
    /// its damage lands a few ticks after the entry into the melee mode,
    /// and a hit that soon after a marked entry is the same swing.
    pub melee_tick: Option<u64>,
}

/// A unit's motion fields for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Observed {
    pub stance: Option<String>,
    pub speed: Option<f64>,
    pub actions: Option<Vec<String>>,
    pub grenade: Option<String>,
}

/// Faster than this between two frames is a teleport or a respawn, not
/// movement (game units a second; a banshee boosts at about 30).
const MAX_PLAUSIBLE_SPEED: f64 = 100.0;
/// Frames further apart than this give no speed (game seconds).
const MAX_SPEED_GAP: f64 = 1.0;
/// A swing's damage lands within this many ticks of the entry into the
/// melee mode (seen: at most 8 at the 15 Hz frame rate). Two swings are
/// further apart than this (the melee recovery), so a hit this soon after a
/// marked entry belongs to the swing already marked.
const MELEE_HIT_MARK_TICKS: u64 = 15;

/// The unit's motion fields this frame, and what to remember for the next.
/// `melee_hit` is set when the unit dealt melee damage since its previous
/// frame (a melee that missed shows up only through the physics mode). A
/// landed swing is one `melee` action: the damage-dealt mark stands in for
/// a swing whose entry into the melee mode no frame saw, and is skipped
/// when the entry was already marked within `MELEE_HIT_MARK_TICKS`.
pub fn observe(
    previous: Option<&UnitMemory>,
    object: &ObjectState,
    alive: bool,
    tick: u64,
    tick_rate: u64,
    melee_hit: bool,
) -> (Observed, UnitMemory) {
    let m = &object.motion;
    let held = object.weapon.map(|w| w.datum);
    let stance = if !alive {
        Some("dead")
    } else if object.vehicle.is_some() {
        Some("vehicle")
    } else if m.physics_mode == Some(layout::BIPED_MODE_FLYING) {
        Some("airborne")
    } else if let Some(crouch) = m.crouch {
        Some(if crouch >= 0.5 { "crouch" } else { "stand" })
    } else {
        m.physics_mode.map(|_| "stand")
    };
    let speed = previous.and_then(|p| {
        let dt = tick.checked_sub(p.tick)? as f64 / tick_rate.max(1) as f64;
        if dt <= 0.0 || dt > MAX_SPEED_GAP {
            return None;
        }
        let d = ((object.pos[0] - p.pos[0]).powi(2) + (object.pos[1] - p.pos[1]).powi(2)).sqrt();
        let speed = d / dt;
        (speed.is_finite() && speed <= MAX_PLAUSIBLE_SPEED).then(|| (speed * 100.0).round() / 100.0)
    });
    let mut actions = Vec::new();
    let mut melee_tick = None;
    if let Some(p) = previous.filter(|_| alive) {
        melee_tick = p.melee_tick;
        let pm = &p.motion;
        let mut push = |a: &str| {
            if !actions.iter().any(|x| x == a) {
                actions.push(a.to_string());
            }
        };
        if let (Some(before), Some(now)) = (pm.weapons, m.weapons) {
            let carried = |d: &u32| *d != u32::MAX;
            if let (Some(a), Some(b)) = (p.held, held)
                && a != b
                && before.contains(&b)
            {
                push("switch");
            }
            if now
                .iter()
                .filter(|d| carried(d))
                .any(|d| !before.contains(d))
            {
                push("pickup");
            }
        }
        if let (Some(a), Some(b)) = (p.held, held)
            && a == b
            && let (Some((reserve_before, loaded_before)), Some((reserve, loaded))) =
                (pm.magazine, m.magazine)
            && loaded > loaded_before
            && reserve < reserve_before
        {
            push("reload");
        }
        if let (Some(before), Some(now)) = (pm.grenades, m.grenades) {
            if now < before {
                push("grenade");
            } else if now > before {
                push("pickup");
            }
        }
        let entered = |mode: u8| m.physics_mode == Some(mode) && pm.physics_mode != Some(mode);
        let marked_swing = p
            .melee_tick
            .is_some_and(|t| tick.saturating_sub(t) <= MELEE_HIT_MARK_TICKS);
        if entered(layout::BIPED_MODE_MELEE) || (melee_hit && !marked_swing) {
            push("melee");
            melee_tick = Some(tick);
        }
        if entered(layout::BIPED_MODE_FLYING)
            && pm.physics_mode == Some(layout::BIPED_MODE_GROUND)
            && m.velocity[2] > 0.0
        {
            push("jump");
        }
    }
    actions.truncate(MAX_FRAME_ACTIONS);
    let observed = Observed {
        stance: stance.map(str::to_owned),
        speed,
        actions: (!actions.is_empty()).then_some(actions),
        grenade: m.grenade.and_then(layout::grenade_name).map(str::to_owned),
    };
    let memory = UnitMemory {
        tick,
        pos: object.pos,
        held,
        motion: m.clone(),
        melee_tick,
    };
    (observed, memory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Weapon;

    const RIFLE: u32 = 0x9001_0010;
    const PISTOL: u32 = 0x9002_0011;
    const NEEDLER: u32 = 0x9003_0012;

    fn unit(held: Option<u32>, slots: [u32; 4]) -> ObjectState {
        ObjectState {
            forward: [1.0, 0.0, 0.0],
            weapon: held.map(|datum| Weapon {
                datum,
                definition: 1,
            }),
            motion: Motion {
                crouch: Some(0.0),
                physics_mode: Some(layout::BIPED_MODE_GROUND),
                grenade: Some(0),
                grenades: Some(4),
                weapons: Some(slots),
                magazine: Some((96, 32)),
                ..Motion::default()
            },
            ..ObjectState::default()
        }
    }

    fn step(before: &ObjectState, after: &ObjectState) -> Observed {
        let (_, memory) = observe(None, before, true, 10, 30, false);
        observe(Some(&memory), after, true, 12, 30, false).0
    }

    fn actions(o: &Observed) -> Vec<&str> {
        o.actions.iter().flatten().map(String::as_str).collect()
    }

    const NONE: u32 = u32::MAX;

    #[test]
    fn stances() {
        let mut o = unit(Some(RIFLE), [RIFLE, NONE, NONE, NONE]);
        let stance = |o: &ObjectState, alive| observe(None, o, alive, 0, 30, false).0.stance;
        assert_eq!(stance(&o, true).as_deref(), Some("stand"));
        o.motion.crouch = Some(0.8);
        assert_eq!(stance(&o, true).as_deref(), Some("crouch"));
        o.motion.physics_mode = Some(layout::BIPED_MODE_FLYING);
        assert_eq!(stance(&o, true).as_deref(), Some("airborne"));
        o.vehicle = Some(0x9004_0004);
        assert_eq!(stance(&o, true).as_deref(), Some("vehicle"));
        assert_eq!(stance(&o, false).as_deref(), Some("dead"));
        let unknown = ObjectState::default();
        assert_eq!(stance(&unknown, true), None, "nothing read");
    }

    #[test]
    fn speed_is_horizontal_distance_over_game_time() {
        let a = unit(None, [NONE; 4]);
        let mut b = a.clone();
        b.pos = [0.3, 0.4, 9.0];
        // 0.5 units in 2 ticks at 30 Hz: 7.5 units a second; height ignored.
        assert_eq!(step(&a, &b).speed, Some(7.5));
        b.pos = [50.0, 0.0, 0.0];
        assert_eq!(step(&a, &b).speed, None, "a teleport is not movement");
        assert_eq!(observe(None, &a, true, 0, 30, false).0.speed, None);
    }

    #[test]
    fn weapon_actions() {
        let a = unit(Some(RIFLE), [RIFLE, PISTOL, NONE, NONE]);
        let b = unit(Some(PISTOL), [RIFLE, PISTOL, NONE, NONE]);
        assert_eq!(actions(&step(&a, &b)), ["switch"]);
        // Picking up a needler in place of the rifle, and holding it.
        let c = unit(Some(NEEDLER), [NEEDLER, PISTOL, NONE, NONE]);
        assert_eq!(actions(&step(&a, &c)), ["pickup"]);
        let mut d = a.clone();
        d.motion.magazine = Some((64, 64));
        assert_eq!(actions(&step(&a, &d)), ["reload"]);
        let mut e = a.clone();
        e.motion.magazine = Some((96, 31));
        assert!(step(&a, &e).actions.is_none(), "firing is not an action");
    }

    #[test]
    fn grenade_melee_and_jump() {
        let a = unit(Some(RIFLE), [RIFLE, NONE, NONE, NONE]);
        let mut b = a.clone();
        b.motion.grenades = Some(3);
        assert_eq!(actions(&step(&a, &b)), ["grenade"]);
        b.motion.grenades = Some(5);
        assert_eq!(actions(&step(&a, &b)), ["pickup"]);
        let mut c = a.clone();
        c.motion.physics_mode = Some(layout::BIPED_MODE_MELEE);
        assert_eq!(actions(&step(&a, &c)), ["melee"]);
        let mut d = a.clone();
        d.motion.physics_mode = Some(layout::BIPED_MODE_FLYING);
        d.motion.velocity = [0.0, 0.0, 0.1];
        assert_eq!(actions(&step(&a, &d)), ["jump"]);
        d.motion.velocity = [0.0, 0.0, -0.1];
        assert!(step(&a, &d).actions.is_none(), "walking off a ledge");
        let (_, memory) = observe(None, &a, true, 10, 30, false);
        let hit = observe(Some(&memory), &a, true, 12, 30, true).0;
        assert_eq!(actions(&hit), ["melee"]);
        assert_eq!(hit.grenade.as_deref(), Some("frag"));
    }

    #[test]
    fn a_landed_swing_marks_melee_once() {
        let a = unit(Some(RIFLE), [RIFLE, NONE, NONE, NONE]);
        let mut swing = a.clone();
        swing.motion.physics_mode = Some(layout::BIPED_MODE_MELEE);
        let mut recovered = a.clone();
        recovered.motion.physics_mode = Some(layout::BIPED_MODE_GROUND);
        let (_, memory) = observe(None, &a, true, 10, 30, false);
        // The entry into the melee mode marks the swing.
        let (entered, memory) = observe(Some(&memory), &swing, true, 12, 30, false);
        assert_eq!(actions(&entered), ["melee"]);
        // Its damage landing a few ticks later is the same swing: no
        // second action.
        let hit = observe(Some(&memory), &recovered, true, 20, 30, true).0;
        assert!(hit.actions.is_none(), "the hit is the swing already marked");
        // A hit with no marked swing in reach still marks (the swing's
        // entry went unseen), as does one long after the last mark.
        let late = observe(Some(&memory), &recovered, true, 40, 30, true).0;
        assert_eq!(actions(&late), ["melee"]);
        let (_, memory) = observe(None, &a, true, 50, 30, false);
        let unseen = observe(Some(&memory), &recovered, true, 52, 30, true).0;
        assert_eq!(actions(&unseen), ["melee"]);
        // A second swing after the first marks again at its own entry.
        let (_, memory) = observe(Some(&memory), &recovered, true, 60, 30, false);
        let second = observe(Some(&memory), &swing, true, 62, 30, false).0;
        assert_eq!(actions(&second), ["melee"]);
    }

    #[test]
    fn a_dead_or_new_unit_has_no_actions() {
        let a = unit(Some(RIFLE), [RIFLE, NONE, NONE, NONE]);
        let mut b = a.clone();
        b.motion.grenades = Some(0);
        let (_, memory) = observe(None, &a, true, 10, 30, false);
        assert!(
            observe(Some(&memory), &b, false, 12, 30, false)
                .0
                .actions
                .is_none()
        );
        assert!(observe(None, &b, true, 12, 30, true).0.actions.is_none());
    }
}
