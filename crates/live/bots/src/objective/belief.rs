//! What a bot believes about the flags and bombs it cannot see (docs/game/
//! bots.md, section 3.7, "Belief"). Perception keeps a loose flag or bomb,
//! or an enemy carrying one, only while it is in view (`perceive`); the
//! bases are public. So, like a player, the bot:
//!
//! - remembers where it last saw each flag or bomb, and who had it;
//! - takes a flag or bomb it has never seen to be at its spawn;
//! - forgets a remembered one when it looks at the spot and it is not
//!   there, and falls back on the spawn;
//! - knows a team's flag or bomb is missing when it looks at the spawn and
//!   it is not there, or when our flag's carrier gets away, until it sees
//!   it again or [`MISSING_TICKS`] pass;
//! - gives up for a while on a destination it cannot reach (the stuck
//!   machine repaths again and again on the way) or cannot take (it
//!   stands on it and nothing happens).
//!
//! CTF and Assault only; the other games plan from what is in view.

use crate::brain::{Me, Vec3};

use super::{Game, Goal, Item, Lean, Objective, Spot, kind};

/// A loose flag or bomb out of view is remembered this long (45 s).
pub const FORGET_LOOSE: u32 = 30 * 45;
/// An enemy carrier out of view is followed to where it was seen this long.
pub const FORGET_CARRIED: u32 = 30 * 4;
/// A flag or bomb known missing is presumed back at its spawn after this
/// long without a sighting (a capture or a return puts it there).
pub const MISSING_TICKS: u32 = 30 * 20;
/// A spot this near, in view, is checked: an item that is not seen there
/// is not there.
pub const CHECK_RANGE: f32 = 15.0;
/// The stuck machine repathing this often on the way to one destination
/// gives it up...
pub const GIVE_UP_REPATHS: u32 = 3;
/// ...as it does standing on an item this long without taking it...
pub const STAND_TICKS: u32 = 90;
/// ...for this long (15 s).
pub const UNREACHABLE_TICKS: u32 = 30 * 15;
/// Within this of a given-up point counts as the same point.
pub const SAME_SPOT: f32 = 1.5;

/// A presumed item's id: no object has it.
pub fn presumed_id(team: i16) -> u32 {
    u32::MAX - team as u16 as u32
}

/// What the plan knows besides the objective in view: the bot's leanings
/// and which teams' flags or bombs it knows are gone from their spawn
/// with no sighting since (bit per game team, 8 neutral).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Outlook {
    pub lean: Lean,
    pub missing: u16,
}

impl Outlook {
    pub fn missing(&self, team: i16) -> bool {
        (0..16).contains(&team) && self.missing & (1 << team) != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Known {
    item: Item,
    tick: u32,
}

/// The destination being pursued, the stuck machine's repath count when it
/// was adopted, and since when the bot has stood on it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pursuit {
    to: Vec3,
    repaths: u32,
    standing: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Belief {
    known: Vec<Known>,
    /// Teams whose flag or bomb is missing, and since when.
    missing: Vec<(i16, u32)>,
    /// Destinations given up, and until when.
    unreachable: Vec<(Vec3, u32)>,
    pursuit: Option<Pursuit>,
    last_tick: Option<u32>,
}

fn dist(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}
fn before(tick: u32, until: u32) -> bool {
    until.wrapping_sub(tick) as i32 > 0
}

impl Belief {
    /// The objective as the bot believes it: what it sees, plus what it
    /// remembers and presumes, less what it has given up on. `sees(at)`
    /// is whether the point is in the bot's view now. `None` outside CTF
    /// and Assault: plan from `o` as it is.
    pub fn know(
        &mut self,
        o: &Objective,
        me: &Me,
        tick: u32,
        lean: Lean,
        sees: &dyn Fn(Vec3) -> bool,
    ) -> Option<(Objective, Outlook)> {
        if !matches!(o.game, Game::Ctf | Game::Assault) {
            return None;
        }
        // A new match restarts the clock.
        if self.last_tick.is_some_and(|t| tick < t) {
            *self = Belief::default();
        }
        self.last_tick = Some(tick);
        self.unreachable.retain(|&(_, until)| before(tick, until));
        self.missing
            .retain(|&(_, since)| tick.wrapping_sub(since) < MISSING_TICKS);

        // What is in view is the truth: one flag or bomb a team.
        for i in &o.items {
            self.known.retain(|k| k.item.team != i.team);
            self.known.push(Known { item: *i, tick });
            self.missing.retain(|&(t, _)| t != i.team);
        }
        let near = |at: Vec3| dist(at, me.position) < CHECK_RANGE && sees(at);
        let mut gone = Vec::new();
        self.known.retain(|k| {
            if k.tick == tick {
                return true;
            }
            let age = tick.wrapping_sub(k.tick);
            // We carry their flag and our bomb (a neutral one either side).
            let ours_to_carry = match o.game {
                Game::Ctf => k.item.team != me.team,
                _ => k.item.team == me.team,
            };
            let keep = match k.item.carrier {
                // Its own: dropped or scored. A teammate's is marked on
                // the HUD: unmarked, it scored or died. Either way the
                // spawn is the first guess again.
                Some(c) if c == o.me || ours_to_carry => false,
                // An enemy carrier out of view: where it was, briefly.
                Some(_) => age < FORGET_CARRIED && !near(k.item.position),
                None => age < FORGET_LOOSE && !near(k.item.position),
            };
            // Our flag in an enemy's hands is not home when the trail
            // goes cold.
            if !keep && k.item.carrier.is_some() && o.game == Game::Ctf && k.item.team == me.team {
                gone.push(k.item.team);
            }
            keep
        });
        for team in gone {
            if !self.missing.iter().any(|&(t, _)| t == team) {
                self.missing.push((team, tick));
            }
        }

        let mut out = o.clone();
        for k in &self.known {
            if !out.items.iter().any(|i| i.team == k.item.team) {
                out.items.push(k.item);
            }
        }
        // Never seen: at its spawn, until a look there says otherwise.
        let spawn = match o.game {
            Game::Ctf => kind::CTF_SPAWN,
            _ => kind::BOMB_SPAWN,
        };
        for s in o.spots.iter().filter(|s| s.kind == spawn) {
            let team = match (o.game, s.team) {
                (_, Some(t)) => t,
                (Game::Assault, None) => 8,
                _ => continue,
            };
            if out.items.iter().any(|i| i.team == team)
                || self.missing.iter().any(|&(t, _)| t == team)
            {
                continue;
            }
            if near(s.position) {
                self.missing.push((team, tick));
                continue;
            }
            out.items.push(Item {
                id: presumed_id(team),
                team,
                position: s.position,
                carrier: None,
            });
        }

        // Less what it gave up on: loose items, and a scoring point while
        // another of its kind and team remains.
        let blocked = |at: Vec3| {
            self.unreachable
                .iter()
                .any(|&(p, _)| dist(p, at) < SAME_SPOT)
        };
        out.items
            .retain(|i| i.carrier.is_some() || !blocked(i.position));
        let spots = out.spots.clone();
        out.spots.retain(|s| {
            let scoring = matches!(s.kind, kind::CTF_RETURN | kind::BOMB_RETURN);
            !(scoring
                && blocked(s.position)
                && spots
                    .iter()
                    .any(|x: &Spot| x.kind == s.kind && x.team == s.team && !blocked(x.position)))
        });

        let mut missing = 0u16;
        for &(t, _) in &self.missing {
            if (0..16).contains(&t) && !out.items.iter().any(|i| i.team == t) {
                missing |= 1 << t;
            }
        }
        Some((out, Outlook { lean, missing }))
    }

    /// After the plan: follow the destination it asked for, and give it up
    /// when the stuck machine keeps repathing on the way (`repaths` is
    /// [`crate::stuck::Stuck::repaths`], a running count) or when the bot
    /// stands on an item it cannot take. `take` is whether the goal is an
    /// item to take or touch.
    pub fn pursue(&mut self, goal: Option<Goal>, take: bool, at: Vec3, repaths: u32, tick: u32) {
        let to = match goal {
            Some(Goal::Go(p) | Goal::Carry(p)) => p,
            _ => {
                self.pursuit = None;
                return;
            }
        };
        let take = take && matches!(goal, Some(Goal::Go(_)));
        let p = match self.pursuit {
            Some(p) if dist(p.to, to) < SAME_SPOT => p,
            _ => Pursuit {
                to,
                repaths,
                standing: None,
            },
        };
        let on = take && dist_xy(at, to) < 1.0 && (at[2] - to[2]).abs() < 1.5;
        let standing = if on {
            Some(p.standing.unwrap_or(tick))
        } else {
            None
        };
        let stood = standing.is_some_and(|s| tick.wrapping_sub(s) >= STAND_TICKS);
        // An arming carrier stands on the plate (`Carry(at)` where it is).
        let stuck = repaths.wrapping_sub(p.repaths) >= GIVE_UP_REPATHS && dist_xy(at, to) > 0.5;
        if stood || stuck {
            self.unreachable
                .push((to, tick.wrapping_add(UNREACHABLE_TICKS)));
            self.pursuit = None;
            return;
        }
        self.pursuit = Some(Pursuit { to, standing, ..p });
    }

    /// Whether `at` was given up at `tick`.
    pub fn unreachable(&self, at: Vec3, tick: u32) -> bool {
        self.unreachable
            .iter()
            .any(|&(p, until)| before(tick, until) && dist(p, at) < SAME_SPOT)
    }
}

#[cfg(test)]
mod tests;
