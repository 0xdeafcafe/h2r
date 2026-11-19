//! Objectives (docs/game/bots.md, section 3.7): the gametype, the flags,
//! balls and bombs and who carries them, the scenario's netgame flags
//! (flag spawns and returns, ball spawns, bomb plates, hill markers), the
//! live hill, and the pure decision of what to do about them ([`plan`]).
//!
//! The fields were traced offline from the XBE (no game run); section 3.7
//! has the addresses, the evidence and how sure each is. In short:
//!
//! - the game-engine globals `E = [0x4e9ae8]`: `+0xc14` the engine index
//!   (1 CTF, 3 Oddball, 4 King of the Hill, 8 Territories, 9 Assault),
//!   `+0xc18` how many objective objects there are (at most 8) and
//!   `+0xc1c` their records, `0x10` each: the object, its team (in
//!   Oddball the ball's number), the unit carrying it and that unit's
//!   player (`0x15e050` registers one, `0x15e250` and `0x15e460` take it
//!   up and drop it);
//! - `E+8` a mask of the netgame team designators in use and `E+0x10`
//!   each designator's game team (`i16`, `0x158990`);
//! - the scenario's netgame flags (`scnr+0x118`, `0x20` each: position,
//!   `+0x10` type, `+0x12` team designator), as `0x19ec40` matches them;
//! - King of the Hill's globals `[0x51ecc8]`: `+0x1a0` the hill in play,
//!   whose corners are the netgame flags of type `11 +` it (`0x2bdc70`).

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::view::objects;

use crate::brain::{Me, Seen, Vec3};

type R<T> = Result<T, MemFault>;

// ---- where it lives ----

/// Holds the game-engine globals' address (`0xc9c` bytes).
pub const GAME_ENGINE: u32 = 0x004e_9ae8;
/// `E+0xc14`: the engine index in play (`i32`; `0x1574c0` copies the
/// variant's `+0x44` there and sets 0 when that engine has no globals).
pub const ENGINE_INDEX: u32 = 0xc14;
/// `E+0xc18`: how many objective objects are registered (`i32`, at most
/// [`OBJECTIVE_MAX`]).
pub const OBJECTIVE_COUNT: u32 = 0xc18;
/// `E+0xc1c`: the objective records, [`OBJECTIVE_STRIDE`] each.
pub const OBJECTIVES: u32 = 0xc1c;
pub const OBJECTIVE_STRIDE: u32 = 0x10;
pub const OBJECTIVE_MAX: u32 = 8;
/// In a record: the object (`u32` datum), its team (`i16`; a ball's
/// number in Oddball), the unit carrying it and that unit's player (`u32`
/// datums, -1 for none).
pub const RECORD_OBJECT: u32 = 0;
pub const RECORD_TEAM: u32 = 4;
pub const RECORD_UNIT: u32 = 8;
pub const RECORD_PLAYER: u32 = 0xc;
/// `E+8`: the netgame team designators in use (`u16` mask, bit per
/// designator); `E+0x10`: each designator's game team (`i16`, 9 of them).
pub const DESIGNATOR_MASK: u32 = 8;
pub const DESIGNATOR_TEAMS: u32 = 0x10;
pub const DESIGNATORS: u16 = 9;

/// Holds King of the Hill's globals' address (`E+0xfc` in a KotH game).
pub const KOTH_GLOBALS: u32 = 0x0051_ecc8;
/// In them: the hill in play (`i32`, 0 to 7).
pub const KOTH_HILL: u32 = 0x1a0;

/// `scnr+0x118`: the netgame flags block (count, then address), `0x20`
/// each.
pub const NETGAME_FLAGS: u32 = 0x118;
pub const NETGAME_FLAG_STRIDE: u32 = 0x20;
pub const FLAG_TYPE: u32 = 0x10;
pub const FLAG_TEAM: u32 = 0x12;
/// Netgame flags looked at, at most.
pub const NETGAME_FLAG_LIMIT: i32 = 512;

/// Netgame flag types the engine asks for (`0x243b10`, `0x2bd164`).
pub mod kind {
    pub const CTF_SPAWN: u16 = 0;
    pub const CTF_RETURN: u16 = 1;
    pub const BOMB_SPAWN: u16 = 2;
    pub const BOMB_RETURN: u16 = 3;
    pub const BALL_SPAWN: u16 = 4;
    /// Hill `n`'s corners are type `HILL + n`, `n` 0 to 7.
    pub const HILL: u16 = 11;
    pub const HILLS: u16 = 8;
}

// ---- what the bot knows ----

/// The objective gametypes (Slayer and Juggernaut have none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    Ctf,
    Oddball,
    Koth,
    Territories,
    Assault,
}

impl Game {
    /// The engine index's gametype (docs/live/live-gamestate.md, section 3).
    pub fn from_engine(index: i32) -> Option<Game> {
        Some(match index {
            1 => Game::Ctf,
            3 => Game::Oddball,
            4 => Game::Koth,
            8 => Game::Territories,
            9 => Game::Assault,
            _ => return None,
        })
    }
}

/// A flag, ball or bomb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    /// The weapon object's datum.
    pub id: u32,
    /// Its team (CTF, Assault: 8 for neutral), or in Oddball the ball's
    /// number.
    pub team: i16,
    /// Where it is: its carrier's position while carried.
    pub position: Vec3,
    /// The carrier's player index (as [`Seen::id`]).
    pub carrier: Option<u32>,
}

/// A netgame flag: a flag or bomb spawn, a return point or plate, a ball
/// spawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub kind: u16,
    /// The game team its designator stands for, if any.
    pub team: Option<i16>,
    pub position: Vec3,
}

/// The hill: its corners, in the scenario's order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Hill {
    pub corners: Vec<Vec3>,
}

impl Hill {
    pub fn center(&self) -> Vec3 {
        let n = self.corners.len().max(1) as f32;
        let mut c = [0.0; 3];
        for p in &self.corners {
            for (ck, pk) in c.iter_mut().zip(p) {
                *ck += pk / n;
            }
        }
        c
    }
    /// The corners' mean distance from the centre (XY).
    pub fn radius(&self) -> f32 {
        let c = self.center();
        let n = self.corners.len().max(1) as f32;
        self.corners.iter().map(|p| dist_xy(*p, c)).sum::<f32>() / n
    }
    /// Whether `p` stands in the hill: inside the corners (taken round
    /// the centre by angle) and within [`HILL_HEIGHT`] of it.
    pub fn contains(&self, p: Vec3) -> bool {
        if self.corners.len() < 3 {
            return false;
        }
        let c = self.center();
        if (p[2] - c[2]).abs() > HILL_HEIGHT {
            return false;
        }
        let mut ring: Vec<Vec3> = self.corners.clone();
        ring.sort_by(|a, b| {
            let ka = (a[1] - c[1]).atan2(a[0] - c[0]);
            let kb = (b[1] - c[1]).atan2(b[0] - c[0]);
            ka.total_cmp(&kb)
        });
        // Even-odd rule.
        let mut inside = false;
        let mut b = ring[ring.len() - 1];
        for &a in &ring {
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                inside = !inside;
            }
            b = a;
        }
        inside
    }
}

/// How far above or below a hill's centre still counts as in it.
pub const HILL_HEIGHT: f32 = 2.0;

/// The objective as the bot sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Objective {
    pub game: Game,
    /// The bot's own player index (as [`Seen::id`]): to know what it
    /// carries and its role.
    pub me: u32,
    pub items: Vec<Item>,
    pub spots: Vec<Spot>,
    /// King of the Hill's hill in play.
    pub hill: Option<Hill>,
}

// ---- reading it ----

/// The objective in play for the player at index `me`, or `None` outside
/// an objective game.
pub fn read(m: &dyn GuestMemory, me: u32) -> R<Option<Objective>> {
    let e = m.u32(GAME_ENGINE)?;
    if e == 0 {
        return Ok(None);
    }
    let Some(game) = Game::from_engine(m.i32(e + ENGINE_INDEX)?) else {
        return Ok(None);
    };
    let mut out = Objective {
        game,
        me,
        items: read_items(m, e)?,
        spots: Vec::new(),
        hill: None,
    };
    let flags = netgame_flags(m)?;
    let teams = designators(m, e)?;
    let team_of = |d: u16| -> Option<i16> {
        match teams {
            // No designator bound: take the designator for the team.
            None => (d < 8).then_some(d as i16),
            Some(t) => t.get(usize::from(d)).copied().flatten(),
        }
    };
    let wanted: &[u16] = match game {
        Game::Ctf => &[kind::CTF_SPAWN, kind::CTF_RETURN],
        Game::Assault => &[kind::BOMB_SPAWN, kind::BOMB_RETURN],
        Game::Oddball => &[kind::BALL_SPAWN],
        Game::Koth | Game::Territories => &[],
    };
    for &(position, ty, d) in &flags {
        if wanted.contains(&ty) {
            out.spots.push(Spot {
                kind: ty,
                team: team_of(d),
                position,
            });
        }
    }
    if game == Game::Koth {
        let k = m.u32(KOTH_GLOBALS)?;
        if k != 0 {
            let n = m.i32(k + KOTH_HILL)?;
            if (0..i32::from(kind::HILLS)).contains(&n) {
                let ty = kind::HILL + n as u16;
                let corners: Vec<Vec3> = flags.iter().filter(|f| f.1 == ty).map(|f| f.0).collect();
                if !corners.is_empty() {
                    out.hill = Some(Hill { corners });
                }
            }
        }
    }
    Ok(Some(out))
}

/// The registered flags, balls and bombs.
fn read_items(m: &dyn GuestMemory, e: u32) -> R<Vec<Item>> {
    let n = m.i32(e + OBJECTIVE_COUNT)?.clamp(0, OBJECTIVE_MAX as i32) as u32;
    let mut out = Vec::new();
    for i in 0..n {
        let r = e + OBJECTIVES + i * OBJECTIVE_STRIDE;
        let Some(object) = DatumHandle::from_raw(m.u32(r + RECORD_OBJECT)?) else {
            continue;
        };
        let team = m.u16(r + RECORD_TEAM)? as i16;
        let unit = DatumHandle::from_raw(m.u32(r + RECORD_UNIT)?);
        let player = DatumHandle::from_raw(m.u32(r + RECORD_PLAYER)?);
        // Carried: where the carrier is. Else where it lies.
        let carrier_at = match unit {
            Some(u) => objects::get(m, u)?
                .map(|(_, b)| b.position(m))
                .transpose()?,
            None => None,
        };
        let position = match carrier_at {
            Some(p) => p,
            None => match objects::get(m, object)? {
                Some((_, b)) => b.position(m)?,
                None => continue,
            },
        };
        out.push(Item {
            id: object.raw(),
            team,
            position,
            carrier: player.map(|p| u32::from(p.index())),
        });
    }
    Ok(out)
}

/// Each designator's game team, or `None` when no designator is bound.
fn designators(m: &dyn GuestMemory, e: u32) -> R<Option<[Option<i16>; 9]>> {
    let mask = m.u16(e + DESIGNATOR_MASK)?;
    if mask == 0 {
        return Ok(None);
    }
    let mut t = [None; 9];
    for d in 0..DESIGNATORS {
        if mask & 1 << d != 0 {
            let team = m.u16(e + DESIGNATOR_TEAMS + 2 * u32::from(d))? as i16;
            t[usize::from(d)] = (team >= 0).then_some(team);
        }
    }
    Ok(Some(t))
}

/// The scenario's netgame flags: (position, type, team designator).
pub fn netgame_flags(m: &dyn GuestMemory) -> R<Vec<(Vec3, u16, u16)>> {
    let s = m.u32(points::SCENARIO.base)?;
    if s == 0 {
        return Ok(Vec::new());
    }
    let n = m.i32(s + NETGAME_FLAGS)?.clamp(0, NETGAME_FLAG_LIMIT) as u32;
    let at = m.u32(s + NETGAME_FLAGS + 4)?;
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let f = at + i * NETGAME_FLAG_STRIDE;
        out.push((m.vec3(f)?, m.u16(f + FLAG_TYPE)?, m.u16(f + FLAG_TEAM)?));
    }
    Ok(out)
}

// ---- the decision ----

/// A bot's part in its team's play (CTF and Assault).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Takes the enemy flag home, escorts a teammate who has it.
    Attack,
    /// Guards our flag's spawn.
    Defend,
}

/// Roles across a team, by rank: the team's living players sorted by
/// index, the same in every bot's mind with nothing said between them.
/// One alone attacks; two split; from three a third (rounded down)
/// defend, the highest ranks.
pub fn role(rank: usize, size: usize) -> Role {
    let defenders = match size {
        0 | 1 => 0,
        2 => 1,
        n => n / 3,
    };
    if rank >= size.saturating_sub(defenders) {
        Role::Defend
    } else {
        Role::Attack
    }
}

/// The role of player `me` on a team whose other living players are
/// `mates` (indices).
pub fn role_of(me: u32, mates: &[u32]) -> Role {
    let mut ids: Vec<u32> = mates.iter().copied().filter(|&i| i != me).collect();
    ids.push(me);
    ids.sort_unstable();
    let rank = ids.iter().position(|&i| i == me).unwrap_or(0);
    role(rank, ids.len())
}

/// What the objective asks of the bot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Goal {
    /// It carries the flag or bomb: run there, along the nav mesh, and
    /// nothing else matters (no rearming, no backing off).
    Carry(Vec3),
    /// It carries the ball: keep it, away from the enemy, meleeing who
    /// comes close.
    Keep,
    /// Walk there: take the enemy flag or the ball, return our flag, get
    /// to the hill.
    Go(Vec3),
    /// Stay within the radius of there and fight from it: guard our flag,
    /// hold the hill.
    Hold(Vec3, f32),
    /// Kill the carrier: this player, here.
    Hunt(u32, Vec3),
    /// Stay near a teammate who carries it, here.
    Escort(Vec3),
}

/// Our flag is home when within this of its spawn.
pub const HOME: f32 = 2.0;
/// An attacker turns back for our dropped flag within this.
pub const RETURN_RANGE: f32 = 25.0;
/// A defender keeps within this of our flag's spawn.
pub const DEFEND_RADIUS: f32 = 4.0;
/// A bot in the hill keeps within this part of its radius.
pub const HILL_KEEP: f32 = 0.6;

fn dist(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}
fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The decision, pure: what the objective asks of the bot `me`, given the
/// other living players and whether teams play. `None` when it asks
/// nothing (no flag read, no hill): the bot plays as in Slayer.
pub fn plan(o: &Objective, me: &Me, others: &[Seen], teams: bool) -> Option<Goal> {
    let mates: Vec<u32> = others
        .iter()
        .filter(|s| teams && s.team == me.team)
        .map(|s| s.id)
        .collect();
    let friend = |id: u32| id == o.me || mates.contains(&id);
    match o.game {
        Game::Ctf => flag_plan(o, me, &mates, &friend),
        Game::Assault => bomb_plan(o, me, others, &mates, &friend),
        Game::Oddball => ball_plan(o, me, &friend),
        Game::Koth => {
            let h = o.hill.as_ref()?;
            let c = h.center();
            if h.contains(me.position) {
                Some(Goal::Hold(c, (h.radius() * HILL_KEEP).max(0.5)))
            } else {
                Some(Goal::Go(c))
            }
        }
        // Next (docs/game/bots.md section 3.7): the territories' owners.
        Game::Territories => None,
    }
}

fn nearest<'a, T: 'a>(at: Vec3, it: impl Iterator<Item = (&'a T, Vec3)>) -> Option<&'a T> {
    it.map(|(t, p)| (t, dist(p, at)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
}

fn flag_plan(o: &Objective, me: &Me, mates: &[u32], friend: &dyn Fn(u32) -> bool) -> Option<Goal> {
    let mine = |s: &&Spot| s.team == Some(me.team);
    let at = me.position;
    let spot = |ty: u16, ours: bool| {
        nearest(
            at,
            o.spots
                .iter()
                .filter(|s| s.kind == ty && mine(s) == ours)
                .map(|s| (s, s.position)),
        )
        .map(|s| s.position)
    };
    let home = spot(kind::CTF_SPAWN, true);
    let goal = spot(kind::CTF_RETURN, true).or(home);

    // 1. It carries one: take it there.
    if o.items.iter().any(|i| i.carrier == Some(o.me)) {
        return Some(Goal::Carry(goal.unwrap_or(at)));
    }

    let role = role_of(o.me, mates);
    let ours = |i: &&Item| i.team == me.team;
    let own = o.items.iter().find(ours);
    let theirs: Vec<&Item> = o.items.iter().filter(|i| !ours(i)).collect();
    let target = nearest(
        at,
        theirs
            .iter()
            .filter(|i| i.carrier.is_none_or(|c| !friend(c)))
            .map(|i| (*i, i.position)),
    );

    if let Some(f) = own {
        match f.carrier {
            // 2. An enemy has our flag: kill the carrier. Defenders always;
            // attackers when the carrier is nearer than their target.
            Some(c) if !friend(c) => {
                let nearer = target.is_none_or(|t| dist(f.position, at) < dist(t.position, at));
                if role == Role::Defend || nearer {
                    return Some(Goal::Hunt(c, f.position));
                }
            }
            Some(_) => {}
            // 3. Our flag lies away from home: return it (touch it).
            None => {
                let away = home.is_some_and(|h| dist(h, f.position) > HOME);
                if away && (role == Role::Defend || dist(f.position, at) < RETURN_RANGE) {
                    return Some(Goal::Go(f.position));
                }
            }
        }
    }
    // 4. A teammate has theirs: attackers escort, defenders guard.
    let escort = theirs
        .iter()
        .find(|i| i.carrier.is_some_and(|c| c != o.me && friend(c)));
    // The carrier is occupied. Allocate the remaining players between
    // escort and home guard even if the carrier used to be the defender.
    if let Some(e) = escort {
        let free: Vec<_> = mates
            .iter()
            .copied()
            .filter(|&id| Some(id) != e.carrier)
            .collect();
        return Some(match (role_of(o.me, &free), home) {
            (Role::Defend, Some(base)) => Goal::Hold(base, DEFEND_RADIUS),
            _ => Goal::Escort(e.position),
        });
    }
    if role == Role::Attack {
        if let Some(t) = target {
            return Some(match t.carrier {
                Some(c) => Goal::Hunt(c, t.position),
                None => Goal::Go(t.position),
            });
        }
    }
    // 5. Guard our flag's spawn (or the flag itself).
    let guard = home.or(own.filter(|f| f.carrier.is_none()).map(|f| f.position));
    guard
        .map(|g| Goal::Hold(g, DEFEND_RADIUS))
        .or_else(|| target.map(|t| Goal::Go(t.position)))
}

/// In Assault a team takes its own (or a neutral) bomb to the opposing
/// plate. A dropped enemy bomb is not a flag we can touch to return.
fn bomb_plan(
    o: &Objective,
    me: &Me,
    others: &[Seen],
    mates: &[u32],
    friend: &dyn Fn(u32) -> bool,
) -> Option<Goal> {
    let at = me.position;
    let plate = |ours| {
        let find = |kind| {
            nearest(
                at,
                o.spots
                    .iter()
                    .filter(|s| {
                        s.kind == kind
                            && s.team
                                .is_some_and(|t| (0..8).contains(&t) && (t == me.team) == ours)
                    })
                    .map(|s| (s, s.position)),
            )
            .map(|s| s.position)
        };
        find(kind::BOMB_RETURN).or_else(|| find(kind::BOMB_SPAWN))
    };
    let theirs = plate(false);
    if o.items.iter().any(|i| i.carrier == Some(o.me)) {
        return Some(Goal::Carry(theirs.unwrap_or(at)));
    }
    let closest = |want: &dyn Fn(&Item) -> bool| {
        nearest(
            at,
            o.items.iter().filter(|i| want(i)).map(|i| (i, i.position)),
        )
    };
    let incoming = closest(&|i| i.carrier.is_some_and(|id| !friend(id)));
    let escort = closest(&|i| i.carrier.is_some_and(friend));
    let bomb = closest(&|i| i.carrier.is_none() && (i.team == me.team || i.team == 8));
    let defender = role_of(o.me, mates) == Role::Defend;
    let home = plate(true);
    if let Some(b) = incoming
        && (defender
            || (escort.is_none() && bomb.is_none())
            || home.is_some_and(|p| dist(p, b.position) <= 10.0))
        && let Some(id) = b.carrier
    {
        return Some(Goal::Hunt(id, b.position));
    }
    if let Some(b) = escort {
        if let Some(p) = theirs
            && dist(p, b.position) < DEFEND_RADIUS
        {
            return Some(Goal::Hold(p, DEFEND_RADIUS));
        }
        return Some(Goal::Escort(b.position));
    }
    if let Some(b) = bomb {
        if let Some(plate) = theirs
            && dist(plate, b.position) <= HOME
        {
            // A loose bomb on the plate might still need picking up. Send
            // the nearest teammate to it and cover the plate with the rest.
            let ours = (dist(at, b.position), o.me);
            let closer = others
                .iter()
                .filter(|s| mates.contains(&s.id))
                .any(|s| (dist(s.position, b.position), s.id) < ours);
            return Some(if closer {
                Goal::Hold(plate, DEFEND_RADIUS)
            } else {
                Goal::Go(b.position)
            });
        }
        if !defender || home.is_none() {
            return Some(Goal::Go(b.position));
        }
    }
    home.map(|p| Goal::Hold(p, DEFEND_RADIUS))
        .or_else(|| bomb.map(|b| Goal::Go(b.position)))
}

fn ball_plan(o: &Objective, me: &Me, friend: &dyn Fn(u32) -> bool) -> Option<Goal> {
    if o.items.iter().any(|i| i.carrier == Some(o.me)) {
        return Some(Goal::Keep);
    }
    let at = me.position;
    // A free ball first, then an enemy carrier, then a teammate's.
    if let Some(b) = nearest(
        at,
        o.items
            .iter()
            .filter(|i| i.carrier.is_none())
            .map(|i| (i, i.position)),
    ) {
        return Some(Goal::Go(b.position));
    }
    if let Some(b) = nearest(
        at,
        o.items
            .iter()
            .filter(|i| i.carrier.is_some_and(|c| !friend(c)))
            .map(|i| (i, i.position)),
    ) && let Some(c) = b.carrier
    {
        return Some(Goal::Hunt(c, b.position));
    }
    o.items
        .iter()
        .find(|i| i.carrier.is_some_and(friend))
        .map(|i| Goal::Escort(i.position))
}

#[cfg(test)]
mod tests;
