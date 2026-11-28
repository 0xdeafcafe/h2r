//! Clankers in Firefight (docs/game/bots.md, "Firefight"; docs/game/firefight.md).
//!
//! A bot is always on a player team, never on the AI's. The AI's actors
//! are its enemies, read from the actor pool ([`read_actors`]) and put in
//! [`Perception::others`] with ids from [`ACTOR_ID_BASE`], so they go
//! through the same sight, target and aim path as players. With two
//! player teams the other team's players are enemies too; a teammate
//! never is.
//!
//! The game describes the match in a [`Context`] ([`set_context`]): the
//! player teams, the AI's team, each team's home base, its lives and the
//! wave phase. From it the brain holds near its home base
//! ([`State::plan`]), falls back to it when hurt, regroups there when the
//! team's lives run low, rearms between waves within an errand radius,
//! and never walks alone to a drop zone.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, RwLock};

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::{Datums, objects};
use guest_layout::actor::Actor;
use guest_layout::global::ACTORS;

use crate::brain::{Me, Perception, Seen, Vec3};
use crate::name_profile::Weights;
use crate::objective::Goal;

/// A [`Seen::id`] at or over this is an actor: [`ACTOR_ID_BASE`] plus
/// the actor's datum index. Player slots are 0 to 15.
pub const ACTOR_ID_BASE: u32 = 0x100;

/// The team the bots give actors when the AI's team is a player team
/// (a misassignment): an actor never shares a team with a player.
pub const ACTOR_TEAM: i16 = i16::MIN;

/// The hold radius around home (world units) at the middle of the
/// name's aggression; [`Traits::hold`] scales it from 0.75 to 1.25.
pub const HOLD_RADIUS: f32 = 8.0;
/// Between waves a bot is free within this many hold radii of home.
pub const ERRAND: f32 = 2.0;
/// The radius it keeps when the team's lives are low.
pub const REGROUP_RADIUS: f32 = 3.0;
/// The radius it falls back to when hurt.
pub const FALLBACK_RADIUS: f32 = 2.5;
/// A point this near a drop zone outside the hold radius is the drop
/// zone's.
pub const DROP_NEAR: f32 = 8.0;
/// A teammate this near is company: the bot is not alone.
pub const COMPANY: f32 = 5.0;

/// Whether `id` names an actor ([`ACTOR_ID_BASE`]).
pub fn is_actor(id: u32) -> bool {
    id >= ACTOR_ID_BASE
}

/// The wave phase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// A wave is on.
    #[default]
    Fighting,
    /// Between waves (and before the first): time to rearm.
    Between,
}

/// One Firefight match as the game describes it to the bots. The
/// Firefight runtime fills it (`game::firefight`) and sets it with
/// [`set_context`]; `None` outside Firefight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    /// The player teams' engine team indices, Firefight team 0 first: one
    /// or two.
    pub player_teams: Vec<i16>,
    /// The engine team the actors are on.
    pub ai_team: i16,
    /// Each player team's home base point, by Firefight team: the kit's
    /// zones `ff_base_00` and `ff_base_01` (the host's `Kit.bases`).
    pub bases: Vec<Vec3>,
    /// Each player team's lives left in its pool, by Firefight team;
    /// `None` for unlimited or unknown.
    pub lives: Vec<Option<u16>>,
    pub phase: Phase,
    /// The kit's drop zones, where waves arrive.
    pub drop_zones: Vec<Vec3>,
}

impl Context {
    /// The Firefight team (0 or 1) of engine team `team`; `None` when it
    /// is not a player team (the AI's, or one the match does not use).
    pub fn team_index(&self, team: i16) -> Option<usize> {
        if team == self.ai_team && !self.player_teams.contains(&team) {
            return None;
        }
        self.player_teams.iter().position(|&t| t == team)
    }

    /// The team actors are given in the bots' perception: the AI's,
    /// unless a player team (or `me`) has it ([`ACTOR_TEAM`] then).
    pub fn actor_team(&self, me: Option<i16>) -> i16 {
        if self.player_teams.contains(&self.ai_team) || me == Some(self.ai_team) {
            ACTOR_TEAM
        } else {
            self.ai_team
        }
    }
}

static CONTEXT: RwLock<Option<Arc<Context>>> = RwLock::new(None);

/// Sets the running Firefight match, or `None` when none runs.
pub fn set_context(ctx: Option<Context>) {
    *CONTEXT.write().unwrap_or_else(|e| e.into_inner()) = ctx.map(Arc::new);
}

/// The running Firefight match ([`set_context`]).
pub fn current() -> Option<Arc<Context>> {
    CONTEXT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

static SEATS: AtomicU8 = AtomicU8::new(0);

/// The lobby's Firefight player teams (1 or 2), or `None` when the lobby
/// is not set up for Firefight. A phantom's lobby team is then one of
/// the player teams ([`seat_team`], `phantom::session::lobby_team`).
pub fn set_seat_teams(teams: Option<u8>) {
    SEATS.store(teams.map_or(0, |n| n.clamp(1, 2)), Ordering::Relaxed);
}

/// [`set_seat_teams`]'s value.
pub fn seat_teams() -> Option<u8> {
    match SEATS.load(Ordering::Relaxed) {
        0 => None,
        n => Some(n),
    }
}

/// The lobby team for a new Clanker in Firefight with `teams` player
/// teams, given the teams the lobby's players hold: team 0 with one
/// player team; with two, the one with fewer players (0 on a tie). Never
/// any other team, so never the AI's.
pub fn seat_team(held: impl IntoIterator<Item = u8>, teams: u8) -> u8 {
    let n = teams.clamp(1, 2);
    let mut count = [0usize; 2];
    for t in held.into_iter().filter(|&t| t < n) {
        count[usize::from(t)] += 1;
    }
    (0..n).min_by_key(|&t| (count[usize::from(t)], t)).unwrap_or(0)
}

/// Appends the living actors to `out` as [`Seen`]s: id [`ACTOR_ID_BASE`]
/// plus the actor's datum index, its unit's origin (a rider's is its
/// vehicle's), and its own team (the perception's [`normalise`] decides
/// the team the bot uses). An actor with no unit, or whose body is
/// depleted, is left out.
pub fn read_actors(m: &dyn GuestMemory, out: &mut Vec<Seen>) -> Result<(), MemFault> {
    let Some(pool) = Datums::<Actor>::from_global(m, ACTORS.base)? else {
        return Ok(());
    };
    for actor in pool.live_iter(m, guest_layout::actor::CAPACITY)? {
        let (handle, a) = actor?;
        let Some(unit) = a.unit(m)? else { continue };
        let Some((kind, body)) = objects::get(m, unit)? else {
            continue;
        };
        if !matches!(kind, objects::BIPED | objects::VEHICLE)
            || body.damage_flags(m)? & crate::guest::BODY_DEPLETED != 0
        {
            continue;
        }
        out.push(Seen {
            id: ACTOR_ID_BASE + u32::from(handle.index()),
            position: crate::vehicle::read::unit_position(m, body)?,
            team: a.team(m)?,
        });
    }
    Ok(())
}

/// Puts a guest-read perception in Firefight's terms before the sight
/// filter: a team game; with one player team every player is a teammate
/// whatever team byte the engine gave it; and actors carry
/// [`Context::actor_team`], never a player's team.
pub fn normalise(p: &mut Perception, ctx: &Context) {
    p.teams = true;
    drop_allies(p, ctx);
    if let [team] = ctx.player_teams[..] {
        if let Some(me) = &mut p.me {
            me.team = team;
        }
        if let Some((_, t)) = &mut p.observations.player {
            *t = team;
        }
        for o in p.others.iter_mut().filter(|o| !is_actor(o.id)) {
            o.team = team;
        }
    }
    let actors = ctx.actor_team(p.me.map(|m| m.team));
    for o in p.others.iter_mut().filter(|o| is_actor(o.id)) {
        o.team = actors;
    }
}

/// Removes the actors that serve the bot's own side: an ally squad's actors
/// are on a player team (`Command::Ally`), the one the bot is on, or with
/// one player team the only one. They are never a target, never in the
/// sight pass. With two teams the other team's ally stays an actor, an
/// enemy as the engine has it. When the AI's own team is a player team
/// (a misassignment) no actor can be told from an ally, and none is dropped.
fn drop_allies(p: &mut Perception, ctx: &Context) {
    if ctx.player_teams.contains(&ctx.ai_team) {
        return;
    }
    let single = ctx.player_teams.len() == 1;
    let mine = p.me.map(|m| m.team);
    p.others.retain(|o| {
        !(is_actor(o.id)
            && ctx.player_teams.contains(&o.team)
            && (single || mine == Some(o.team)))
    });
}

/// Whether `o` is an enemy of a bot on `team` in perception `p`: an
/// actor always; a player on another team in a team game, anyone in
/// free-for-all.
pub fn hostile(p: &Perception, team: i16, o: &Seen) -> bool {
    is_actor(o.id) || !p.teams || o.team != team
}

/// The name's Firefight numbers ([`crate::name_profile`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Traits {
    /// The hold radius around home: [`HOLD_RADIUS`] scaled by aggression.
    pub hold: f32,
    /// Falls back home under this vitality (shield, at most 1, plus
    /// health): higher with the name's retreat.
    pub hurt: f32,
}

impl Traits {
    pub fn of(weights: Option<Weights>) -> Traits {
        let (aggression, retreat) = weights.map_or((0.5, 0.5), |w| (w.aggression, w.retreat));
        Traits {
            hold: HOLD_RADIUS * (0.75 + 0.5 * aggression),
            hurt: 0.4 + 0.6 * retreat,
        }
    }
}

fn dist(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The Firefight part of a bot's mind: the match and whether it is on
/// its way home.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub(crate) ctx: Option<Arc<Context>>,
    /// Walking back inside the hold radius: between waves it is not free
    /// again until there.
    returning: bool,
}

impl State {
    pub fn set(&mut self, ctx: Option<Arc<Context>>) {
        if ctx.is_none() {
            self.returning = false;
        }
        self.ctx = ctx;
    }

    pub fn context(&self) -> Option<&Context> {
        self.ctx.as_deref()
    }

    pub(crate) fn release(&mut self) {
        self.returning = false;
    }

    /// The bot's team's home and Firefight team, when it is on a player
    /// team with a base.
    fn home(&self, me: &Me) -> Option<(&Context, usize, Vec3)> {
        let ctx = self.ctx.as_deref()?;
        let t = ctx.team_index(me.team)?;
        Some((ctx, t, *ctx.bases.get(t)?))
    }

    /// Whether `at` is somewhere a bot may go on its own: within the
    /// errand radius of home, and not at a drop zone unless a teammate
    /// is with it.
    fn free(ctx: &Context, home: Vec3, hold: f32, at: Vec3, alone: bool) -> bool {
        dist(at, home) <= hold * ERRAND
            && !(alone
                && ctx
                    .drop_zones
                    .iter()
                    .any(|&z| dist(z, at) < DROP_NEAR && dist(z, home) > hold))
    }

    fn alone(p: &Perception, me: &Me) -> bool {
        !p.others
            .iter()
            .any(|o| !is_actor(o.id) && o.team == me.team && dist(o.position, me.position) < COMPANY)
    }

    /// The goal Firefight asks this tick, `None` when the bot is free to
    /// play as it would (between waves, near home), or outside Firefight:
    ///
    /// - the team's lives at or under its living players: regroup on home
    ///   ([`REGROUP_RADIUS`]);
    /// - hurt (under [`Traits::hurt`]): fall back to home
    ///   ([`FALLBACK_RADIUS`]);
    /// - between waves, within the errand radius and away from the drop
    ///   zones (or with company): free;
    /// - otherwise hold within [`Traits::hold`] of home. Once outside the
    ///   free area it walks back inside the hold radius before it is free
    ///   again.
    pub fn plan(&mut self, p: &Perception, me: &Me, weights: Option<Weights>) -> Option<Goal> {
        let (ctx, t, home) = self.home(me)?;
        let traits = Traits::of(weights);
        let living = 1 + p
            .others
            .iter()
            .filter(|o| !is_actor(o.id) && o.team == me.team)
            .count();
        if ctx
            .lives
            .get(t)
            .copied()
            .flatten()
            .is_some_and(|n| usize::from(n) <= living)
        {
            return Some(Goal::Hold(home, REGROUP_RADIUS));
        }
        if me.shield.min(1.0) + me.health < traits.hurt {
            return Some(Goal::Hold(home, FALLBACK_RADIUS));
        }
        let d = dist(me.position, home);
        let between = ctx.phase == Phase::Between;
        let free = between && Self::free(ctx, home, traits.hold, me.position, Self::alone(p, me));
        if d <= traits.hold {
            self.returning = false;
        } else if !free {
            self.returning = true;
        }
        if free && !self.returning {
            return None;
        }
        Some(Goal::Hold(home, traits.hold))
    }

    /// Whether a weapon or ammunition at `at` is off limits: outside the
    /// errand radius of home, or at a drop zone with no teammate along.
    pub fn blocks(&self, at: Vec3, p: &Perception, me: &Me, weights: Option<Weights>) -> bool {
        let Some((ctx, _, home)) = self.home(me) else {
            return false;
        };
        !Self::free(ctx, home, Traits::of(weights).hold, at, Self::alone(p, me))
    }
}

#[cfg(test)]
mod tests;
