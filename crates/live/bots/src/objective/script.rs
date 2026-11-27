//! A rules script's objectives (docs/game/bots.md, section 3.7, "A
//! script's goals"; docs/game/rules-world-api.md, "Bots"): the places a
//! script says matter to a team, which the retail engine's globals do not
//! hold. A script sets them with `bot_goal_attack`, `bot_goal_defend` and
//! `bot_goal_hold` and clears a team's with `bot_goal_clear`; the host
//! keeps the table here ([`set`], [`clear`]) and every phantom's
//! perception carries a copy ([`ScriptGoals`]). [`plan`] is pure: it
//! turns the table into the same [`Goal`] a retail objective's plan gives,
//! with the same roles ([`super::role_of`]) and the hill's keep
//! ([`super::zones::hill`]).

use super::{Goal, Hill, Role, Temper, dist, nearest, role_of};
use crate::brain::{Me, Seen, Vec3};

/// The goals one team holds at most: the engine's objective records'
/// count (`E+0xc18`, section 3.7), so a script names no more places for a
/// side than a retail game does.
pub const TEAM_GOALS: usize = 8;
/// Teams a goal may name (the VM's team indices).
pub const TEAMS: i16 = 8;
/// A defender hunts an enemy this many times the place's reach from its
/// middle.
pub const DEFEND_REACH: f32 = 2.0;

/// What a script asks a team to do about a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aim {
    /// Take it: the team's attackers go and stand in it, as on a hill.
    Attack,
    /// Guard it: stand in it and kill who comes in.
    Defend,
    /// Stand in it, the whole team, with no split into roles.
    Hold,
}

impl Aim {
    /// The script entry that sets it.
    pub fn entry(self) -> &'static str {
        match self {
            Aim::Attack => "bot_goal_attack",
            Aim::Defend => "bot_goal_defend",
            Aim::Hold => "bot_goal_hold",
        }
    }
}

/// One place a script names for a team: the zone's marker and its area,
/// the shape the script's `inside` tests (`game::rules::world::zones`).
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptGoal {
    pub team: i16,
    pub aim: Aim,
    /// The zone's handle in the script: setting the same zone for the same
    /// team again replaces its aim.
    pub zone: u32,
    /// The marker's place and facing.
    pub at: Vec3,
    pub yaw: f32,
    pub area: game_markers::Area,
}

impl ScriptGoal {
    fn hill(&self) -> Hill {
        Hill {
            corners: Vec::new(),
            area: Some((self.at, self.area.clone())),
        }
    }
    pub fn middle(&self) -> Vec3 {
        self.area.middle(self.at)
    }
    pub fn contains(&self, p: Vec3) -> bool {
        self.area.contains_turned(self.at, self.yaw, p)
    }
    /// The area's mean reach from its middle (XY), as a hill's.
    pub fn radius(&self) -> f32 {
        self.hill().radius()
    }
}

/// Why the host refused a goal call: the call is recorded as refused with
/// this text, and no goal changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The team holds [`TEAM_GOALS`] other places already.
    Full { team: i16 },
    /// The team is not one of the VM's.
    NoTeam { team: i16 },
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Refused::Full { team } => write!(
                f,
                "bot goal refused: team {team} has {TEAM_GOALS} goals already, the most a team holds; clear them first (bot_goal_clear)"
            ),
            Refused::NoTeam { team } => write!(f, "bot goal refused: team {team} is not a team (0 to 7)"),
        }
    }
}

impl std::error::Error for Refused {}

/// The goals of the game in play, in the order the script set them.
static GOALS: std::sync::Mutex<Vec<ScriptGoal>> = std::sync::Mutex::new(Vec::new());

fn table() -> std::sync::MutexGuard<'static, Vec<ScriptGoal>> {
    GOALS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Adds `goal` to its team's goals, or changes the aim of the same zone's.
pub fn set(goal: ScriptGoal) -> Result<(), Refused> {
    put(&mut table(), goal)
}

/// [`set`] on a table of its own.
pub fn put(goals: &mut Vec<ScriptGoal>, goal: ScriptGoal) -> Result<(), Refused> {
    if !(0..TEAMS).contains(&goal.team) {
        return Err(Refused::NoTeam { team: goal.team });
    }
    if let Some(g) = goals.iter_mut().find(|g| g.team == goal.team && g.zone == goal.zone) {
        *g = goal;
        return Ok(());
    }
    if goals.iter().filter(|g| g.team == goal.team).count() >= TEAM_GOALS {
        return Err(Refused::Full { team: goal.team });
    }
    goals.push(goal);
    Ok(())
}

/// Clears `team`'s goals, or every team's (`None`: a game's start and end).
pub fn clear(team: Option<i16>) {
    table().retain(|g| team.is_some_and(|t| g.team != t));
}

/// Every team's goals, for a perception.
pub fn goals() -> Vec<ScriptGoal> {
    table().clone()
}

/// The script's goals as one bot sees them: its own player index (to rank
/// it in its team) and the table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptGoals {
    pub me: u32,
    pub list: Vec<ScriptGoal>,
}

/// The decision, pure: what the script's goals ask of the bot, or `None`
/// when its team has none (the retail objective, or Slayer, then plays).
///
/// - A `hold` place takes the whole team.
/// - With `attack` and `defend` places both, the team splits by role
///   ([`role_of`]): attackers take, defenders guard. With one kind only,
///   the whole team plays it.
/// - Attackers all take the first `attack` place the script set (a place
///   is taken by standing in it, so they go together).
/// - Defenders spread over the `defend` places by rank.
/// - Taking or guarding, a bot goes to the place and, once in it, keeps
///   about its middle as on a hill; a guard first hunts the enemy nearest
///   the middle within [`DEFEND_REACH`] reaches of it.
pub fn plan(goals: &ScriptGoals, me: &Me, others: &[Seen], teams: bool, temper: Temper) -> Option<Goal> {
    let mine: Vec<&ScriptGoal> = goals.list.iter().filter(|g| g.team == me.team).collect();
    if mine.is_empty() {
        return None;
    }
    let of = |aim: Aim| mine.iter().copied().filter(move |g| g.aim == aim);
    if let Some(g) = of(Aim::Hold).next() {
        return Some(take(g, me, temper));
    }
    let friend = |s: &Seen| teams && s.team == me.team;
    let mut ids: Vec<u32> = others.iter().filter(|s| friend(s)).map(|s| s.id).collect();
    let role = role_of(goals.me, &ids);
    let attack = of(Aim::Attack).next();
    let defend: Vec<&ScriptGoal> = of(Aim::Defend).collect();
    match (attack, role) {
        (Some(g), Role::Attack) => Some(take(g, me, temper)),
        (Some(g), Role::Defend) if defend.is_empty() => Some(take(g, me, temper)),
        _ => {
            ids.push(goals.me);
            ids.sort_unstable();
            ids.dedup();
            let rank = ids.iter().position(|&i| i == goals.me).unwrap_or(0);
            let g = *defend.get(rank % defend.len().max(1))?;
            Some(guard(g, me, others, &friend, temper))
        }
    }
}

/// In the place: keep about its middle (the cautious wider); out: go in.
fn take(g: &ScriptGoal, me: &Me, t: Temper) -> Goal {
    let c = g.middle();
    if g.contains(me.position) {
        let keep = super::HILL_KEEP + super::HILL_KEEP_CAUTION * t.caution.clamp(0.0, 1.0);
        Goal::Hold(c, (g.radius() * keep).max(0.5))
    } else {
        Goal::Go(c)
    }
}

/// Hunt the enemy nearest the middle within reach, else take the place.
fn guard(g: &ScriptGoal, me: &Me, others: &[Seen], friend: &dyn Fn(&Seen) -> bool, t: Temper) -> Goal {
    let c = g.middle();
    let reach = g.radius().max(super::DEFEND_RADIUS) * DEFEND_REACH;
    let intruder = nearest(
        c,
        others
            .iter()
            .filter(|s| !friend(s) && (g.contains(s.position) || dist(s.position, c) < reach))
            .map(|s| (s, s.position)),
    );
    match intruder {
        Some(s) => Goal::Hunt(s.id, s.position),
        None => take(g, me, t),
    }
}

#[cfg(test)]
mod tests;
