//! Diagnostics for one phantom brain. The controller never reads the counters or clock.
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use input::host::HostPadState;
use serde::Serialize;

use crate::arms::{Arms, Role};
use crate::brain::Perception;
use crate::mind::{Doing, MELEE, RELOAD, SWITCH};

const RECENT: usize = 8;
/// A left stick under this stands the bot still.
const STAND_STICK: f32 = 0.2;
/// A right stick over this asks for a turn the aim shows by the next
/// decision.
const TURN_STICK: f32 = 0.3;
/// Aims no component of which differs by this have not moved (under
/// 0.01°).
const SAME_AIM: f32 = 1e-4;

pub fn goal_name(doing: Doing) -> &'static str {
    match doing {
        Doing::Idle => "idle",
        Doing::Fight(_) => "fight",
        Doing::Chase(_) => "chase",
        Doing::HoldAngle(_) => "hold_angle",
        Doing::Roam => "roam",
        Doing::Support(_) => "support",
        Doing::Supply(_) => "supply",
        Doing::Vantage(_) => "vantage",
        Doing::Rearm(_) => "rearm",
        Doing::Recover(_) => "recover",
        Doing::Carry(_) => "carry",
        Doing::Keep => "keep",
        Doing::Play(_) => "play",
        Doing::Mount(_) => "mount",
        Doing::Drive(_) => "drive",
        Doing::Ride(_) => "ride",
    }
}

pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Unknown => "unknown",
        Role::Blade => "blade",
        Role::Scatter => "scatter",
        Role::Spray => "spray",
        Role::Precise => "precise",
        Role::Sniper => "sniper",
        Role::Blast => "blast",
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Decision {
    pub tick: u32,
    pub action: &'static str,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CombatStats {
    pub tick: Option<u32>,
    pub observation_tick: Option<u32>,
    pub goal: Option<&'static str>,
    /// Held weapon definition in this map.
    pub weapon: Option<u32>,
    pub weapon_role: Option<&'static str>,
    pub target: Option<u32>,
    /// Origin-to-origin distance in world units, from the observation used.
    pub range: Option<f32>,
    pub last_decisions: VecDeque<Decision>,
    pub melee_presses: u32,
    pub switch_presses: u32,
    pub grenade_requests: u32,
    pub grenades_used: u32,
    pub weapon_switches: u32,
    pub last_reaction_ticks: Option<u32>,
    pub total_reaction_ticks: u64,
    pub reactions: u32,
    pub think_ms: Option<f64>,
    pub total_think_ms: f64,
    pub think_ticks: u64,
    /// Decisions by goal ([`goal_name`]), for the whole match.
    pub goal_decisions: BTreeMap<&'static str, u32>,
    /// Of those, the ones whose left stick stood the bot still.
    pub standing_decisions: BTreeMap<&'static str, u32>,
    /// Decisions after one whose right stick asked for a turn.
    pub look_asked: u32,
    /// Of those, the ones whose observed aim had not moved since: the look
    /// did not reach the unit (docs/game/bots.md §5.1, "The facing release").
    pub look_stalled: u32,
}

#[derive(Default)]
pub(crate) struct Recorder {
    pub view: CombatStats,
    last_pad: HostPadState,
    acquired: Option<u32>,
    throwing: bool,
    last_arms: Option<Arms>,
    /// The observed aim at the last decision.
    last_aim: Option<crate::brain::Vec3>,
}

impl Recorder {
    fn note(&mut self, tick: u32, action: &'static str) {
        if self.view.last_decisions.len() == RECENT {
            self.view.last_decisions.pop_front();
        }
        self.view.last_decisions.push_back(Decision { tick, action });
    }

    /// Keep match totals through death. A new life has no target or pending input.
    pub fn release(&mut self) {
        self.last_pad = HostPadState::default();
        self.acquired = None;
        self.throwing = false;
        self.last_arms = None;
        self.last_aim = None;
        self.view.goal = None;
        self.view.weapon = None;
        self.view.weapon_role = None;
        self.view.target = None;
        self.view.range = None;
        self.view.observation_tick = None;
    }

    fn inventory(&mut self, arms: Option<Arms>) {
        let held = arms.and_then(|arms| arms.held);
        if let Some((old, new)) = self.last_arms.zip(arms) {
            let carried = |arms: Arms| Some((arms.held?.def, arms.other?.def));
            if let (Some((held, spare)), Some(current)) = (carried(old), carried(new))
                && held != spare
                && current == (spare, held)
            {
                self.view.weapon_switches += 1;
            }
            for (before, after) in old.grenades.into_iter().zip(new.grenades) {
                if after >= 0 && before > after {
                    self.view.grenades_used += (before - after) as u32;
                }
            }
        }
        self.last_arms = arms;
        self.view.weapon = held.map(|g| g.def);
        self.view.weapon_role = held.map(|g| role_name(g.role()));
    }

    pub fn decision(
        &mut self,
        now: u32,
        p: &Perception,
        doing: Doing,
        pad: HostPadState,
        throwing: bool,
    ) {
        self.inventory(p.me.and_then(|me| me.arms));
        self.view.observation_tick = Some(p.tick);
        let goal = goal_name(doing);
        if self.view.goal != Some(goal) {
            self.note(now, goal);
        }
        self.view.goal = Some(goal);
        *self.view.goal_decisions.entry(goal).or_default() += 1;
        if pad.left_stick.0.hypot(pad.left_stick.1) < STAND_STICK {
            *self.view.standing_decisions.entry(goal).or_default() += 1;
        }
        let aim = p.me.map(|me| me.aim);
        let (rx, ry) = self.last_pad.right_stick;
        if let (Some(before), Some(now)) = (self.last_aim, aim)
            && rx.hypot(ry) > TURN_STICK
        {
            self.view.look_asked += 1;
            if before.iter().zip(now).all(|(a, b)| (a - b).abs() < SAME_AIM) {
                self.view.look_stalled += 1;
            }
        }
        self.last_aim = aim;
        let target = match doing {
            Doing::Fight(id) => p.others.iter().find(|o| o.id == id),
            _ => None,
        };
        let id = target.map(|o| o.id);
        if self.view.target != id {
            self.acquired = id.map(|_| p.tick);
        }
        self.view.target = id;
        self.view.range = target
            .zip(p.me)
            .map(|(o, me)| crate::decisions::distance(o.position, me.position));

        let melee = pad.buttons.contains(MELEE);
        if melee && !self.last_pad.buttons.contains(MELEE) {
            self.view.melee_presses += 1;
            self.note(now, "melee");
        }
        if pad.buttons.contains(SWITCH) && !self.last_pad.buttons.contains(SWITCH) {
            self.view.switch_presses += 1;
            self.note(now, "switch");
        }
        if pad.buttons.contains(RELOAD) && !self.last_pad.buttons.contains(RELOAD) {
            self.note(now, "reload_or_interact");
        }
        if throwing && !self.throwing {
            self.view.grenade_requests += 1;
            self.note(now, "grenade");
        }
        self.throwing = throwing;
        let firing = pad.right_trigger > 0.5;
        if firing && self.last_pad.right_trigger <= 0.5 {
            self.note(now, "fire");
        }
        if (melee || firing || throwing)
            && let Some(start) = self.acquired.take()
        {
            let ticks = now.wrapping_sub(start);
            self.view.last_reaction_ticks = Some(ticks);
            self.view.reactions += 1;
            self.view.total_reaction_ticks += u64::from(ticks);
        }
        self.last_pad = pad;
    }

    pub fn thought(&mut self, tick: u32, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.;
        self.view.tick = Some(tick);
        self.view.think_ms = Some(ms);
        self.view.total_think_ms += ms;
        self.view.think_ticks += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Me, Seen};

    #[test]
    fn reaction_uses_observed_ticks_and_held_buttons_count_once() {
        let mut r = Recorder::default();
        let mut p = Perception {
            tick: 10,
            me: Some(Me::default()),
            others: vec![Seen {
                id: 2,
                team: 1,
                position: [3., 4., 0.],
            }],
            ..Default::default()
        };
        let mut pad = HostPadState::default();
        r.decision(13, &p, Doing::Fight(2), pad, false);
        assert_eq!(r.view.range, Some(5.));
        pad.buttons.set(MELEE, true);
        p.tick += 1;
        r.decision(14, &p, Doing::Fight(2), pad, false);
        p.tick += 1;
        r.decision(15, &p, Doing::Fight(2), pad, false);
        assert_eq!(r.view.melee_presses, 1);
        assert_eq!(r.view.last_reaction_ticks, Some(4));
        assert_eq!(r.view.reactions, 1);
        r.release();
        assert_eq!(r.view.range, None);
        assert_eq!(r.view.melee_presses, 1);
    }

    #[test]
    fn swaps_and_grenade_use_require_inventory_evidence_in_the_same_life() {
        use crate::arms::Gun;

        let mut r = Recorder::default();
        let mut arms = Arms {
            held: Some(Gun {
                def: 42,
                ..Default::default()
            }),
            other: Some(Gun {
                def: 8,
                ..Default::default()
            }),
            grenades: [2, 1],
            ..Default::default()
        };
        let sample = |r: &mut Recorder, tick, arms, throwing| {
            let p = Perception {
                tick,
                me: Some(Me { arms: Some(arms), ..Default::default() }),
                ..Default::default()
            };
            r.decision(tick, &p, Doing::Roam, HostPadState::default(), throwing);
        };
        sample(&mut r, 10, arms, true);
        std::mem::swap(&mut arms.held, &mut arms.other);
        arms.grenades[1] -= 1;
        sample(&mut r, 11, arms, true);
        r.release();
        arms.grenades = [0, 0];
        sample(&mut r, 12, arms, false);
        assert_eq!(r.view.weapon_switches, 1);
        assert_eq!(r.view.grenade_requests, 1);
        assert_eq!(r.view.grenades_used, 1);
    }

    #[test]
    fn standing_and_a_look_that_never_lands_are_counted() {
        let mut r = Recorder::default();
        let at = |tick, aim| Perception {
            tick,
            me: Some(Me { aim, ..Default::default() }),
            ..Default::default()
        };
        let pad = |left: (f32, f32), right: (f32, f32)| HostPadState {
            left_stick: left,
            right_stick: right,
            ..Default::default()
        };
        let x = [1.0, 0.0, 0.0];
        r.decision(1, &at(1, x), Doing::Roam, pad((0.0, 1.0), (0.8, 0.0)), false);
        // Asked to turn, the aim has not moved: stalled. It stands.
        r.decision(2, &at(2, x), Doing::Recover(x), pad((0.0, 0.1), (0.8, 0.0)), false);
        // Asked again, and this time it turned.
        r.decision(3, &at(3, [0.0, 1.0, 0.0]), Doing::Recover(x), pad((0.0, 0.0), (0.0, 0.0)), false);
        // Not asked: an unmoved aim is no stall.
        r.decision(4, &at(4, [0.0, 1.0, 0.0]), Doing::Recover(x), pad((0.5, 0.0), (0.0, 0.0)), false);
        assert_eq!(r.view.look_asked, 2);
        assert_eq!(r.view.look_stalled, 1);
        assert_eq!(r.view.goal_decisions.get("recover"), Some(&3));
        assert_eq!(r.view.goal_decisions.get("roam"), Some(&1));
        assert_eq!(r.view.standing_decisions.get("recover"), Some(&2));
        assert_eq!(r.view.standing_decisions.get("roam"), None);
        // A new life has no aim to compare against.
        r.decision(5, &at(5, x), Doing::Roam, pad((0.0, 1.0), (0.8, 0.0)), false);
        r.release();
        r.decision(6, &at(6, x), Doing::Roam, pad((0.0, 1.0), (0.8, 0.0)), false);
        assert_eq!((r.view.look_asked, r.view.look_stalled), (2, 1), "nothing across a death");
        assert_eq!(r.view.goal_decisions.get("roam"), Some(&3), "match totals");
    }
}
