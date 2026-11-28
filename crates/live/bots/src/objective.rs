//! Objectives (docs/game/bots.md, section 3.7): the gametype, the flags,
//! balls and bombs and who carries them, the scenario's netgame flags
//! (flag spawns and returns, ball spawns, bomb plates, hill markers), the
//! live hill, and the pure decision of what to do about them ([`plan`]).
//!
//! The fields were traced offline from the XBE (no game run); section 3.7
//! has the addresses, the evidence and how sure each is. In short:
//!
//! - the game-engine globals `E = [0x4e9ae8]`: `+0xc14` the engine index
//!   (1 CTF, 3 Oddball, 4 King of the Hill, 7 Juggernaut, 8 Territories, 9 Assault),
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

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use guest_layout::view::objects;

use crate::brain::{Me, Seen, Vec3};

pub mod belief;
mod lean;
pub mod script;

pub use belief::{Belief, Outlook};
pub use lean::Lean;
mod zones;
pub use zones::{
    BALL_COVER, EDGE_CAUTION, FOOTWORK_ROOM, HILL_KEEP, HILL_KEEP_CAUTION, RUSHERS, SCREEN,
    STAGE, TERRITORY_KEEP, TERRITORY_REACH, Temper,
};
pub(crate) use zones::within;

type R<T> = Result<T, MemFault>;

// ---- where it lives ----

pub use engine::objectives::{
    DESIGNATOR_MASK, DESIGNATOR_TEAMS, DESIGNATORS, ENGINE_INDEX, FLAG_TEAM, FLAG_TYPE, GAME_ENGINE,
    KOTH_GLOBALS, KOTH_HILL, NETGAME_FLAG_LIMIT, NETGAME_FLAG_STRIDE, NETGAME_FLAGS, OBJECTIVE_COUNT,
    OBJECTIVE_MAX, OBJECTIVE_STRIDE, OBJECTIVES, RECORD_OBJECT, RECORD_PLAYER, RECORD_TEAM, RECORD_UNIT,
    kind, netgame_flags,
};

// ---- what the bot knows ----

/// Game variants with a role, item or scoring zone to play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    Ctf,
    Oddball,
    Koth,
    Juggernaut,
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
            7 => Game::Juggernaut,
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

/// The hill: its corners, in the scenario's order, or the area of its one
/// flag (docs/game/gametype-markers.md, "In a script"): the zone a rules
/// script scores.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Hill {
    pub corners: Vec<Vec3>,
    /// The area round the hill's flag, the flag's place, when the hill is
    /// shaped as one: it then decides [`Hill::contains`], [`Hill::center`]
    /// and [`Hill::radius`].
    pub area: Option<(Vec3, game_markers::Area)>,
}

impl Hill {
    pub fn center(&self) -> Vec3 {
        if let Some((at, area)) = &self.area {
            return area.middle(*at);
        }
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
        if let Some((at, area)) = &self.area {
            let outline = area.outline();
            let n = outline.len().max(1) as f32;
            return outline
                .iter()
                .map(|o| dist_xy([at[0] + o[0], at[1] + o[1], c[2]], c))
                .sum::<f32>()
                / n;
        }
        let n = self.corners.len().max(1) as f32;
        self.corners.iter().map(|p| dist_xy(*p, c)).sum::<f32>() / n
    }
    /// Whether `p` stands in the hill: inside the corners (taken round
    /// the centre by angle) and within [`HILL_HEIGHT`] of it.
    pub fn contains(&self, p: Vec3) -> bool {
        if let Some((at, area)) = &self.area {
            return area.contains(*at, p);
        }
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

/// The areas of the scenario's netgame flags, by flag index, as the host's
/// census has them (`game::rules::world::zones`): the document's data,
/// which no guest memory holds.
static FLAG_AREAS: std::sync::Mutex<Vec<(u32, game_markers::Area)>> = std::sync::Mutex::new(Vec::new());

/// Sets the flag areas the bots read a rules script's hill from: the host
/// sets them with the census when a game starts, and they replace the last.
pub fn set_flag_areas(areas: Vec<(u32, game_markers::Area)>) {
    *FLAG_AREAS.lock().unwrap_or_else(|e| e.into_inner()) = areas;
}

fn flag_areas() -> Vec<(u32, game_markers::Area)> {
    FLAG_AREAS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// How far above or below a hill's centre still counts as in it.
pub const HILL_HEIGHT: f32 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Territory {
    pub at: Vec3,
    pub owner: Option<u32>,
    pub owner_team: Option<i16>,
    pub occupants: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Unknown,
    Hill {
        occupants: u16,
    },
    Territories(Vec<Territory>),
    /// Territories without the engine's globals (a rules script that
    /// keeps none): the markers alone. Who stands in one is judged by
    /// distance; no owner is known.
    Markers(Vec<Vec3>),
    Juggernauts(u16),
    Assault {
        defending: Option<i16>,
        armed: u16,
        arming: u16,
        planted: u16,
    },
}

fn member(mask: u16, player: u32) -> bool {
    player < 16 && mask & (1 << player) != 0
}

/// The objective as the bot sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Objective {
    pub game: Game,
    pub state: State,
    /// The bot's own player index (as [`Seen::id`]): to know what it
    /// carries and its role.
    pub me: u32,
    pub items: Vec<Item>,
    pub spots: Vec<Spot>,
    /// King of the Hill's hill in play.
    pub hill: Option<Hill>,
}

impl Objective {
    /// Hold the primary use action only for a loose item this goal can take.
    pub(crate) fn take_at(&self, goal: Option<Goal>, me: Me) -> bool {
        let Some(Goal::Go(at)) = goal else {
            return false;
        };
        // Across a step or a lip, the object sits higher or lower than
        // the unit's origin: in reach by XY with a little height.
        dist_xy(me.position, at) < crate::mind::TAKE_RANGE
            && (me.position[2] - at[2]).abs() < TAKE_HEIGHT
            && self.items.iter().any(|i|{
            i.position==at && i.carrier.is_none() && match self.game {
                Game::Oddball => true,
                Game::Ctf => i.team!=me.team,
                Game::Assault => (i.team==me.team || i.team==8)
                    && !matches!(self.state,State::Assault {planted,..} if member(planted,i.team as u32)),
                _ => false,
            }
        })
    }

    pub(crate) fn in_hill(&self, id: u32, at: Vec3) -> bool {
        match self.state {
            State::Hill { occupants } => member(occupants, id),
            _ => self.hill.as_ref().is_some_and(|h| h.contains(at)),
        }
    }
    pub(crate) fn am_juggernaut(&self) -> bool {
        matches!(self.state, State::Juggernauts(mask) if member(mask,self.me))
    }
    /// The games whose held zones keep the fight's footwork inside them
    /// (`zones::within`): the ball, the hill and the territories.
    pub(crate) fn zone_footwork(&self) -> bool {
        matches!(self.game, Game::Oddball | Game::Koth | Game::Territories)
    }
    /// A ball carrier cannot shoot (it melees only): it is no gun in a
    /// fight's count.
    pub(crate) fn unarmed(&self, id: u32) -> bool {
        self.game == Game::Oddball && self.items.iter().any(|i| i.carrier == Some(id))
    }
    pub(crate) fn juggernaut_opponent(&self, id: u32) -> Option<bool> {
        match self.state {
            State::Juggernauts(mask) => Some(member(mask, id) != member(mask, self.me)),
            _ => None,
        }
    }
}

/// A loose flag, ball or bomb is in reach this far above or below the
/// unit's origin.
pub const TAKE_HEIGHT: f32 = 1.0;

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
        state: State::Unknown,
        items: match game {
            Game::Assault | Game::Ctf | Game::Oddball => read_items(m, e)?,
            _ => Vec::new(),
        },
        spots: Vec::new(),
        hill: None,
    };
    match game {
        Game::Juggernaut => {
            let j = m.u32(points::JUGGERNAUT_GLOBALS.base)?;
            if j != 0 {
                out.state = State::Juggernauts(m.u16(j)?);
            }
        }
        Game::Territories => {
            out.state = if m.u32(points::TERRITORIES_GLOBALS.base)? == 0 {
                State::Markers(zones::territory_markers(m)?)
            } else {
                State::Territories(territories(m, &netgame_flags(m)?)?)
            }
        }
        Game::Koth => read_hill(m, &netgame_flags(m)?, &mut out)?,
        Game::Oddball => {
            let balls = m.u32(points::ODDBALL_GLOBALS.base)?;
            if balls == 0 {
                // No Oddball globals (a rules script): the neutral spawns.
                for position in zones::ball_spawns(&netgame_flags(m)?) {
                    out.spots.push(Spot {
                        kind: kind::BALL_SPAWN,
                        team: None,
                        position,
                    });
                }
            } else {
                let flags = netgame_flags(m)?;
                for i in 0..3 {
                    let index = m.u32(balls + 0xc + 4 * i)?;
                    if let Some(&(position, kind::BALL_SPAWN, 8)) = flags.get(index as usize) {
                        out.spots.push(Spot {
                            kind: kind::BALL_SPAWN,
                            team: None,
                            position,
                        });
                    }
                }
            }
        }
        Game::Ctf | Game::Assault => {
            let flags = netgame_flags(m)?;
            let teams = designators(m, e)?;
            let (plates, active_teams) = if game == Game::Assault {
                (read_assault(m, &mut out)?, m.u16(e + 0xc)?)
            } else {
                (Vec::new(), 0)
            };
            for (index, (position, kind, d)) in flags.into_iter().enumerate() {
                let team = teams.get(usize::from(d)).copied().flatten();
                let wanted = match game {
                    Game::Ctf => matches!(kind, kind::CTF_SPAWN | kind::CTF_RETURN),
                    _ => {
                        kind == kind::BOMB_SPAWN
                            || kind == kind::BOMB_RETURN
                                && team.is_some_and(|t| member(active_teams, t as u32))
                                && plates.contains(&(d, index as u16))
                    }
                };
                if wanted {
                    out.spots.push(Spot {
                        kind,
                        team,
                        position,
                    });
                }
            }
        }
    }
    Ok(Some(out))
}

/// The arming and plant routines address both plates by designator.
fn read_assault(m: &dyn GuestMemory, out: &mut Objective) -> R<Vec<(u16, u16)>> {
    let mut plates = Vec::new();
    let a = m.u32(points::CTF_ASSAULT_GLOBALS.base)?;
    if a == 0 {
        return Ok(plates);
    }
    let g = m.u32(points::GAME_GLOBALS.base)?;
    let defending = if g != 0 && m.i32(g + 0x240)? == 1 {
        let team = m.i32(a + 0x1f4)?;
        (0..8).contains(&team).then_some(team as i16)
    } else {
        None
    };
    let mut armed = 0;
    let mut arming = 0;
    let mut planted = 0;
    for team in 0..9 {
        let state = engine::objectives::bomb_state(m, a, team)?;
        if state.planted {
            planted |= 1 << team;
        }
        if state.armed {
            armed |= 1 << team;
        }
        if state.arming {
            arming |= 1 << team;
        }
    }
    for d in 0..9 {
        for offset in [0x14, 0x26] {
            let index = m.u16(a + offset + 2 * d)?;
            if index != u16::MAX {
                plates.push((d as u16, index));
            }
        }
    }
    out.state = State::Assault {
        defending,
        armed,
        arming,
        planted,
    };
    Ok(plates)
}

fn read_hill(m: &dyn GuestMemory, flags: &[(Vec3, u16, u16)], out: &mut Objective) -> R<()> {
    let k = m.u32(KOTH_GLOBALS)?;
    if k == 0 {
        // No King globals (a rules script): the first hill's corners, and
        // who stands in it judged by them.
        out.hill = zones::first_hill(flags, &flag_areas());
        return Ok(());
    }
    out.state = State::Hill {
        occupants: m.u16(k + engine::objectives::HILL_OCCUPANTS)?,
    };
    out.hill = engine::objectives::hill_corners(m, k, flags)?.map(|(_, corners)| Hill {
        corners,
        area: None,
    });
    Ok(())
}

fn territories(m: &dyn GuestMemory, flags: &[(Vec3, u16, u16)]) -> R<Vec<Territory>> {
    let t = m.u32(points::TERRITORIES_GLOBALS.base)?;
    let mut out = Vec::new();
    if t == 0 {
        return Ok(out);
    }
    for site in engine::objectives::territories(m, t)? {
        let Some(&(at, kind::TERRITORY, _)) = flags.get(usize::from(site.flag)) else {
            continue;
        };
        out.push(Territory {
            at,
            owner: site.owner.map(|(h, _)| u32::from(h.index())),
            owner_team: site.owner.map(|(_, team)| team),
            occupants: site.occupants,
        });
    }
    Ok(out)
}

/// The registered flags, balls and bombs.
fn read_items(m: &dyn GuestMemory, e: u32) -> R<Vec<Item>> {
    let mut out = Vec::new();
    for record in engine::objectives::registered(m, e)? {
        let engine::objectives::Registered {
            object,
            unit,
            player,
            designator: team,
        } = record;
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

/// Each bound designator's game team.
fn designators(m: &dyn GuestMemory, e: u32) -> R<[Option<i16>; 9]> {
    let mask = m.u16(e + DESIGNATOR_MASK)?;
    let mut t = [None; 9];
    for d in 0..DESIGNATORS {
        if mask & 1 << d != 0 {
            let team = m.u16(e + DESIGNATOR_TEAMS + 2 * u32::from(d))? as i16;
            t[usize::from(d)] = (0..8).contains(&team).then_some(team);
        }
    }
    Ok(t)
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
    plan_with(o, me, others, teams, &Outlook::default(), Temper::default())
}

/// [`plan`] for a bot of this temper (its name's traits), with what it
/// believes besides what it sees: the flags or bombs it knows are missing
/// ([`Belief`]).
pub fn plan_with(
    o: &Objective,
    me: &Me,
    others: &[Seen],
    teams: bool,
    outlook: &Outlook,
    temper: Temper,
) -> Option<Goal> {
    let mates: Vec<u32> = others
        .iter()
        .filter(|s| teams && s.team == me.team)
        .map(|s| s.id)
        .collect();
    let friend = |id: u32| id == o.me || mates.contains(&id);
    match o.game {
        Game::Ctf => flag_plan(o, me, others, &mates, &friend, outlook),
        Game::Assault => bomb_plan(o, me, others, &mates, &friend, outlook),
        Game::Oddball => zones::ball(o, me, others, &mates, &friend, temper),
        Game::Koth => Some(zones::hill(o, o.hill.as_ref()?, me, others, &friend, temper)),
        Game::Territories => match &o.state {
            State::Territories(sites) => {
                zones::territory(o, sites, me, others, &mates, teams, &friend)
            }
            State::Markers(markers) => {
                let sites = zones::sites_from_markers(markers, o.me, me.position, others);
                zones::territory(o, &sites, me, others, &mates, teams, &friend)
            }
            _ => None,
        },
        Game::Juggernaut => {
            let State::Juggernauts(mask) = o.state else {
                return None;
            };
            let target = nearest(
                me.position,
                others
                    .iter()
                    .filter(|p| member(mask, o.me) != member(mask, p.id))
                    .map(|p| (p, p.position)),
            )?;
            Some(Goal::Hunt(target.id, target.position))
        }
    }
}

fn nearest<'a, T: 'a>(at: Vec3, it: impl Iterator<Item = (&'a T, Vec3)>) -> Option<&'a T> {
    it.map(|(t, p)| (t, dist(p, at)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
}

fn flag_plan(
    o: &Objective,
    me: &Me,
    others: &[Seen],
    mates: &[u32],
    friend: &dyn Fn(u32) -> bool,
    outlook: &Outlook,
) -> Option<Goal> {
    let lean = outlook.lean;
    let mine = |s: &&Spot| s.team == Some(me.team);
    let at = me.position;
    let spot = |ty: u16, ours: bool| {
        nearest(
            at,
            o.spots
                .iter()
                .filter(|s| s.kind == ty && s.team.is_some() && mine(s) == ours)
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
    let guard = lean.guard_radius();
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
            // attackers when the carrier is nearer than their target (a
            // focused attacker keeps on for the flag longer).
            Some(c) if !friend(c) => {
                let nearer = target.is_none_or(|t| {
                    dist(f.position, at) < dist(t.position, at) * lean.carrier_pull()
                });
                if role == Role::Defend || nearer {
                    return Some(Goal::Hunt(c, f.position));
                }
            }
            Some(_) => {}
            // 3. Our flag lies away from home: return it (touch it).
            None => {
                let away = home.is_some_and(|h| dist(h, f.position) > HOME);
                if away && (role == Role::Defend || dist(f.position, at) < lean.return_range()) {
                    return Some(Goal::Go(f.position));
                }
            }
        }
    } else if outlook.missing(me.team) && role == Role::Defend {
        // Our flag is gone and its carrier out of sight: cut it off on
        // its way to their base.
        if let Some(p) = spot(kind::CTF_RETURN, false).or(spot(kind::CTF_SPAWN, false)) {
            return Some(Goal::Go(p));
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
            (Role::Defend, Some(base)) => Goal::Hold(base, guard),
            _ => Goal::Escort(e.position),
        });
    }
    if role == Role::Attack {
        if let Some(t) = target {
            return Some(match t.carrier {
                Some(c) => Goal::Hunt(c, t.position),
                None => grab_or_escort(o, at, t.position, others, mates, lean),
            });
        }
        // Nothing known to take (their flag is missing): play as in
        // Slayer until it turns up, rather than walking home to wait.
        if home.is_some() {
            return None;
        }
    }
    // 5. Guard our flag's spawn (or the flag itself).
    let base = home.or(own.filter(|f| f.carrier.is_none()).map(|f| f.position));
    base.map(|g| Goal::Hold(g, guard))
        .or_else(|| target.map(|t| Goal::Go(t.position)))
}

/// An attacker takes the loose flag or bomb at `item`, unless it leans to
/// escorting ([`Lean::escorts`]) and another attacker is nearer to it:
/// then it covers them on the way. The nearest attacker always goes.
fn grab_or_escort(
    o: &Objective,
    at: Vec3,
    item: Vec3,
    others: &[Seen],
    mates: &[u32],
    lean: Lean,
) -> Goal {
    if lean.escorts() {
        let team: Vec<u32> = mates.iter().copied().chain([o.me]).collect();
        let attacker = |id: u32| {
            let rest: Vec<u32> = team.iter().copied().filter(|&m| m != id).collect();
            role_of(id, &rest) == Role::Attack
        };
        let mine = (dist(at, item), o.me);
        if let Some(m) = nearest(
            item,
            others
                .iter()
                .filter(|s| mates.contains(&s.id) && attacker(s.id))
                .filter(|s| (dist(s.position, item), s.id) < mine)
                .map(|s| (s, s.position)),
        ) {
            return Goal::Escort(m.position);
        }
    }
    Goal::Go(item)
}

/// A planted bomb goes off where it lies: nobody stands within this of it
/// (world units, XY).
pub const BLAST_CLEAR: f32 = 6.0;
/// Clear of the blast, a bot holds within this of its spot.
pub const CLEAR_HOLD: f32 = 1.5;

/// A spot clear of the blast of the bomb planted at `bomb`: out along the
/// line from the bomb to the bot, or towards the nearest public spot
/// outside the blast when the bot stands on the bomb.
fn clear_of(bomb: Vec3, at: Vec3, spots: &[Spot]) -> Vec3 {
    let unit = |p: Vec3| {
        let d = [p[0] - bomb[0], p[1] - bomb[1]];
        let l = d[0].hypot(d[1]);
        (l >= 0.5).then(|| [d[0] / l, d[1] / l])
    };
    let away = nearest(
        at,
        spots
            .iter()
            .filter(|s| dist_xy(s.position, bomb) > BLAST_CLEAR)
            .map(|s| (s, s.position)),
    );
    let dir = unit(at)
        .or_else(|| away.and_then(|s| unit(s.position)))
        .unwrap_or([-1.0, 0.0]);
    let r = BLAST_CLEAR + CLEAR_HOLD;
    [bomb[0] + dir[0] * r, bomb[1] + dir[1] * r, at[2]]
}

/// In Assault a team takes its own (or a neutral) bomb to the opposing
/// plate. A dropped enemy bomb is not a flag we can touch to return. A
/// planted bomb is left alone and its blast kept clear of.
fn bomb_plan(
    o: &Objective,
    me: &Me,
    others: &[Seen],
    mates: &[u32],
    friend: &dyn Fn(u32) -> bool,
    outlook: &Outlook,
) -> Option<Goal> {
    let at = me.position;
    let planted: Vec<Vec3> = match o.state {
        State::Assault { planted, .. } => o
            .items
            .iter()
            .filter(|i| i.carrier.is_none() && member(planted, i.team as u32))
            .map(|i| i.position)
            .collect(),
        _ => Vec::new(),
    };
    let goal = bomb_goal(o, me, others, mates, friend, outlook);
    let Some(bomb) = nearest(at, planted.iter().map(|p| (p, *p))).copied() else {
        return goal;
    };
    let inside = |p: Vec3| dist_xy(p, bomb) < BLAST_CLEAR;
    let to = match goal {
        Some(Goal::Carry(_) | Goal::Keep) => return goal,
        Some(Goal::Go(p) | Goal::Hold(p, _) | Goal::Escort(p) | Goal::Hunt(_, p)) => Some(p),
        None => None,
    };
    if to.map_or(inside(at), inside) {
        return Some(Goal::Hold(clear_of(bomb, at, &o.spots), CLEAR_HOLD));
    }
    goal
}

fn bomb_goal(
    o: &Objective,
    me: &Me,
    others: &[Seen],
    mates: &[u32],
    friend: &dyn Fn(u32) -> bool,
    outlook: &Outlook,
) -> Option<Goal> {
    let at = me.position;
    let lean = outlook.lean;
    let defending = match o.state {
        State::Assault { defending, .. } => defending,
        _ => None,
    };
    let is_planted =
        |i: &Item| matches!(o.state, State::Assault { planted, .. } if member(planted, i.team as u32));
    let plate = |ours| {
        nearest(
            at,
            o.spots
                .iter()
                .filter(|s| {
                    let Some(team) = s.team else { return false };
                    s.kind == kind::BOMB_RETURN
                        && (0..8).contains(&team)
                        && if ours {
                            team == me.team
                        } else {
                            team != me.team && defending.is_none_or(|d| d == team)
                        }
                })
                .map(|s| (s, s.position)),
        )
        .map(|s| s.position)
    };
    let theirs = plate(false);
    if let Some(bomb) = o.items.iter().find(|i| i.carrier == Some(o.me)) {
        if let State::Assault { armed, arming, .. } = o.state
            && member(arming, bomb.team as u32)
            && !member(armed, bomb.team as u32)
            && theirs.is_some_and(|p| dist(at, p) < 0.5)
        {
            return Some(Goal::Carry(at));
        }
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
    let bomb = closest(&|i| {
        i.carrier.is_none() && (i.team == me.team || i.team == 8) && !is_planted(i)
    });
    let defender = role_of(o.me, mates) == Role::Defend;
    let home = plate(true);
    let guard = lean.guard_radius();
    if defending == Some(me.team) {
        return incoming
            .and_then(|b| b.carrier.map(|id| Goal::Hunt(id, b.position)))
            .or_else(|| home.map(|p| Goal::Hold(p, guard)));
    }
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
        if !defender || home.is_none() || defending.is_some() {
            return Some(grab_or_escort(o, at, b.position, others, mates, lean));
        }
    }
    home.map(|p| Goal::Hold(p, guard))
        .or_else(|| bomb.map(|b| Goal::Go(b.position)))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod team_tests;
