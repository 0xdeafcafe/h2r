//! The [`Game`] the VM reads and changes over guest memory at one moment
//! (docs/game/rules-engine.md, "The host runtime"): reads are made at
//! once, actions are kept as [`Call`]s for the next tick boundary.

use std::cell::OnceCell;

use ::rules::caps::id;
use ::rules::types::Type;
use ::rules::vm::{Arg, Call as VmCall, Game, Placed, ScriptError};
use guest_layout::GuestMemory;

use super::Call;
use crate::firefight::runtime::read_players;
use crate::rules::census::{self, Table};
use crate::rules::guest;
use crate::rules::world::map_tags::TagIndex;
use crate::rules::world::zones::{self, Zones};

/// The game, as the VM reads and changes it at one moment. Reads are made
/// at once; actions are kept in `calls` for the next tick boundary. Guest
/// memory does not change while one is held (an action is only kept), so
/// the roster and the objectives are read once, at the first read that
/// needs them, and kept for the rest of it.
pub struct GuestGame<'a> {
    pub m: &'a dyn GuestMemory,
    pub calls: &'a mut Vec<Call>,
    /// The round-start line a handler asked for, kept for the runtime.
    pub(super) start_line: Option<Type>,
    /// The script error the VM stopped on, if it did.
    pub error: Option<ScriptError>,
    /// The map's labelled things; none reads as a map with none.
    pub table: Option<&'a Table>,
    /// This run plays a film back: a bot order is not replayed (the seat's
    /// recorded pad plays), so none is made (docs/game/rules-world-api.md,
    /// "Films and Theater").
    film: bool,
    /// The map's zones, for `inside` and `ticks_held`; none reads as a map
    /// with none (`crate::rules::world::zones`).
    pub zones: Option<&'a Zones>,
    /// The map's tag paths, for `tag_int` and `tag_fixed`; none reads 0.
    pub tags: Option<&'a TagIndex>,
    /// The players' bodies at the last superbounce chain step, for
    /// `airborne`, `crouching` and `primed` (`crate::rules::world::physics`); none reads false.
    pub bodies: &'a [crate::superbounce_chain::Body],
    /// The roster (slot, alive, team), read once ([`GuestGame::roster`]).
    roster: OnceCell<Vec<(u8, bool, u8)>>,
    /// The objectives' datums, read once.
    objectives: OnceCell<Vec<u32>>,
}

impl<'a> GuestGame<'a> {
    pub fn new(m: &'a dyn GuestMemory, calls: &'a mut Vec<Call>) -> Self {
        Self {
            m,
            calls,
            start_line: None,
            error: None,
            table: None,
            film: false,
            zones: None,
            tags: None,
            bodies: &[],
            roster: OnceCell::new(),
            objectives: OnceCell::new(),
        }
    }

    /// The same game whose players' bodies are `bodies`.
    pub fn with_bodies(mut self, bodies: &'a [crate::superbounce_chain::Body]) -> Self {
        self.bodies = bodies;
        self
    }

    /// The same game whose tag paths are `tags`.
    pub fn with_tags(mut self, tags: &'a TagIndex) -> Self {
        self.tags = Some(tags);
        self
    }

    /// The same game whose zones are `zones`.
    pub fn with_zones(mut self, zones: &'a Zones) -> Self {
        self.zones = Some(zones);
        self
    }

    /// The same game whose labelled things are `table`.
    pub fn with_table(mut self, table: &'a Table) -> Self {
        self.table = Some(table);
        self
    }

    /// The same game for a run that plays a film back, or not.
    pub fn with_film(mut self, film: bool) -> Self {
        self.film = film;
        self
    }

    fn thing(&self, call: &VmCall) -> Option<&census::Thing> {
        match call.args.first() {
            Some(Arg::Object(Some(o))) => self.table.and_then(|t| t.get(*o)),
            _ => None,
        }
    }

    /// The roster's (slot, alive, team), read from guest memory at the
    /// first call and kept for the rest of this moment.
    fn roster(&self) -> &[(u8, bool, u8)] {
        self.roster.get_or_init(|| {
            read_players(self.m)
                .unwrap_or_default()
                .into_iter()
                .map(|(p, _)| (p.slot, p.alive, p.team))
                .collect()
        })
    }
}

fn player(call: &VmCall, n: usize) -> Option<u8> {
    match call.args.get(n) {
        Some(Arg::Player(Some(p))) => Some(*p),
        _ => None,
    }
}

impl Game for GuestGame<'_> {
    fn trace_sight(&mut self, _from: u32, _to: u32) -> Option<bool> {
        // The VM keeps the pair and its previous answer until run_work.
        None
    }

    fn start_line(&mut self, ty: Type) {
        self.start_line = Some(ty);
    }

    fn players(&self) -> Vec<u8> {
        self.roster().iter().map(|&(slot, _, _)| slot).collect()
    }

    fn teams(&self) -> Vec<u8> {
        let mut teams: Vec<u8> = self.roster().iter().map(|&(_, _, t)| t).collect();
        teams.sort_unstable();
        teams.dedup();
        teams
    }

    /// The roster slots seated on `team`, for a waypoint's audience
    /// (`mark(wp, at, team)`).
    fn players_of_team(&self, team: u8) -> Vec<u8> {
        self.roster()
            .iter()
            .filter(|&&(_, _, t)| t == team)
            .map(|&(slot, _, _)| slot)
            .collect()
    }

    /// The map's things with this label ([`Table::labelled`]).
    fn labelled(&self, label: &str) -> Vec<Placed> {
        self.table.map_or_else(Vec::new, |t| t.labelled(label))
    }

    fn objectives(&self) -> Vec<u32> {
        self.objectives
            .get_or_init(|| crate::rules::objectives::objects(self.m).unwrap_or_default())
            .clone()
    }

    fn variant(&self, field: &str) -> i32 {
        guest::variant(self.m, field).unwrap_or(0)
    }

    /// The one team whose living players alone are inside `zone`
    /// ([`Zones::holder`]).
    fn zone_holder(&self, zone: u32) -> Option<u8> {
        let zones = self.zones?;
        zones.holder(zone, &zones::bodies(self.m))
    }

    fn read(&mut self, call: &VmCall) -> i32 {
        let m = self.m;
        let p = player(call, 0);
        let on_player = |f: fn(&dyn GuestMemory, u8) -> Result<i32, guest_layout::MemFault>| {
            p.map_or(0, |p| f(m, p).unwrap_or(0))
        };
        match call.entry.id {
            id::PLAYER_SCORE => on_player(guest::player_score),
            id::KILLS => on_player(guest::kills),
            id::DEATHS => on_player(guest::deaths),
            id::LIVES => on_player(guest::lives),
            id::OBJECT_POS | id::OBJECT_VEL => crate::rules::world::objects::read(m, call),
            id::TEAM_SCORE => match call.args.first() {
                Some(Arg::Team(Some(t))) => guest::team_score(m, *t).unwrap_or(0),
                _ => 0,
            },
            id::ALIVE => {
                p.is_some_and(|p| self.roster().iter().any(|&(s, alive, _)| s == p && alive)) as i32
            }
            id::TEAM_OF => p
                .and_then(|p| self.roster().iter().copied().find(|&(s, _, _)| s == p))
                .map_or(::rules::ty::NONE, |(_, _, t)| i32::from(t)),
            id::SLOT_OF => p.map_or(::rules::ty::NONE, i32::from),
            id::SIDE_OF => match p {
                None => ::rules::ty::NONE,
                Some(p) if self.variant("team_play") == 0 => 16 + i32::from(p),
                Some(p) => self
                    .roster()
                    .iter()
                    .copied()
                    .find(|&(s, _, _)| s == p)
                    .map_or(::rules::ty::NONE, |(_, _, t)| i32::from(t)),
            },
            id::ROUND_TIME_LEFT => guest::round_time_left(m).unwrap_or(guest::NO_TIME_LIMIT),
            id::IS_OBJECTIVE => match call.args.first() {
                Some(Arg::Object(Some(o))) => guest::is_objective(m, *o).unwrap_or(false) as i32,
                _ => 0,
            },
            id::CARRIER_OF => match call.args.first() {
                Some(Arg::Object(Some(o))) => crate::rules::objectives::carrier(m, *o)
                    .ok()
                    .flatten()
                    .map_or(::rules::ty::NONE, i32::from),
                _ => ::rules::ty::NONE,
            },
            // An unnumbered thing, or one that is not labelled, reads 0.
            id::LABEL_NUMBER => self.thing(call).and_then(|t| t.number).unwrap_or(0),
            // A label the script does not declare is no label of its.
            id::LABEL_OF => self
                .thing(call)
                .and_then(|t| t.label)
                .map_or(::rules::ty::NONE, i32::from),
            id::ZONE_ID => self.thing(call).map_or(0, |t| t.id() as i32),
            id::OBJECT_TEAM => match call.args.first() {
                Some(Arg::Object(Some(o))) => {
                    let engine = guest::engine_index(m).ok().flatten();
                    census::object_team(m, self.table, engine, *o)
                        .ok()
                        .flatten()
                        .map_or(::rules::ty::NONE, i32::from)
                }
                _ => ::rules::ty::NONE,
            },
            // The player's living unit inside the zone.
            id::INSIDE => match (p, call.args.get(1), self.zones) {
                (Some(p), Some(Arg::Object(Some(zone))), Some(z)) => zones::bodies(m)
                    .iter()
                    .any(|b| b.slot == p && z.contains(*zone, b.at))
                    as i32,
                _ => 0,
            },
            // A fraction of full, 0 to 1; an object that is gone reads 0.
            id::OBJECT_HEALTH | id::OBJECT_SHIELD => match call.args.first() {
                Some(Arg::Object(Some(o))) => {
                    crate::rules::world::data::vitality(m, *o).map_or(0, |(health, shield)| {
                        if call.entry.id == id::OBJECT_HEALTH {
                            health
                        } else {
                            shield
                        }
                    })
                }
                _ => 0,
            },
            // A field of a tag the map holds, from the tag's data in guest
            // memory; a tag or field the map lacks reads 0 (the start gate
            // blocked a script that names one).
            id::TAG_INT | id::TAG_FIXED => match (call.args.first(), call.args.get(1), self.tags) {
                (Some(Arg::Tag(Some(tag))), Some(Arg::Text(field)), Some(index)) => {
                    crate::rules::world::map_tags::read(
                        m,
                        index,
                        tags::defs::Defs::builtin(),
                        call.entry.id,
                        tag,
                        field,
                    )
                    .unwrap_or(0)
                }
                _ => 0,
            },
            id::AIRBORNE | id::CROUCHING | id::PRIMED => p
                .is_some_and(|p| crate::rules::world::physics::read(self.bodies, call.entry.id, p))
                as i32,
            id::PLAYER_OF => match call.args.first() {
                Some(Arg::Object(Some(o))) => {
                    zones::player_of(m, *o).map_or(::rules::ty::NONE, i32::from)
                }
                _ => ::rules::ty::NONE,
            },
            _ => 0,
        }
    }

    /// A call through `none` does nothing, as a write through `none` does.
    fn act(&mut self, call: &VmCall) {
        let p = player(call, 0);
        let int = |n: usize| match call.args.get(n) {
            Some(Arg::Int(v)) => Some(*v),
            _ => None,
        };
        let made = match call.entry.id {
            id::SCORE => p
                .zip(int(1))
                .map(|(player, delta)| Call::Score { player, delta }),
            id::SET_LIVES => p
                .zip(int(1))
                .map(|(player, lives)| Call::SetLives { player, lives }),
            id::RESPAWN => p.map(Call::Respawn),
            // The VM holds the role on every record. The retail Juggernaut
            // setter also posts the announcement and updates its team mask.
            id::ROLE_TRAITS => match call.args.get(1) {
                Some(Arg::Bool(on))
                    if guest::engine_index(self.m).ok().flatten()
                        == Some(crate::rules::roles::JUGGERNAUT) =>
                {
                    p.map(|player| Call::RoleTraits { player, on: *on })
                }
                _ => None,
            },
            id::HOVER | id::UNLOAD => crate::rules::world::vehicles::plan(self.m, self.table, call),
            id::RESET_OBJECTIVE => match call.args.first() {
                Some(Arg::Object(Some(o))) => Some(Call::ResetObjective(*o)),
                _ => None,
            },
            // Shown and sent as a show cue at the boundary
            // (`crate::rules::world::show::publish`), not made as an engine call.
            id::MESSAGE | id::MESSAGE_TEAM | id::MESSAGE_ALL => {
                crate::rules::world::show::outgoing(call).map(Call::Message)
            }
            // Played and sent as a sound cue at the boundary
            // (`crate::rules::world::audio::publish`).
            id::SOUND_TO | id::SOUND_TO_TEAM | id::SOUND_TO_ALL => {
                crate::rules::world::audio::outgoing(call)
                    .map(|(to, path)| Call::SoundTo { to, path })
            }
            // The winner is not passed: naming one is not traced.
            id::END_ROUND => Some(Call::End),
            // The engine is not told the winner: the script scores it.
            id::NEXT_ROUND => Some(Call::NextRound),
            // The VM keeps the rule in its state, which the spawn chooser's
            // hooks read at each spawn (`crate::rules::world::spawns`).
            id::SPAWN_AREAS | id::SPAWN_NEAR => None,
            // The VM keeps the body in its state, which the spawn's hooks
            // read (`crate::rules::world::body`).
            id::SET_BODY => None,
            // The VM keeps the shown areas in its state; the walls are
            // drawn from it (`crate::rules::world::walls`).
            id::SHOW_AREA | id::HIDE_AREA => None,
            // The VM keeps the show state; the HUDs are drawn from it
            // (`crate::rules::world::hud`).
            id::HUD_VALUE
            | id::HUD_FILL
            | id::HUD_STYLE
            | id::HUD_TEXT
            | id::HUD_SHOW
            | id::MARK
            | id::MARK_PLAYER
            | id::MARK_TEAM
            | id::UNMARK => None,
            // The VM keeps the mover; each tick's step is `step_object`.
            id::MOVE_TO | id::STOP | id::SET_OPEN => None,
            // The VM keeps the offers; the boundary asks the players
            // (`crate::rules::world::pick::plan`).
            id::OFFER_PICK | id::PICK_ROW => None,
            // The VM keeps the loadout window; the boundary asks the player
            // (`crate::rules::world::choice::plan`).
            id::OFFER => None,
            // The VM keeps the objective HUD's state; the HUD is drawn from it
            // (`crate::objective_hud`).
            id::OBJECTIVE | id::OBJECTIVE_MARK | id::METER | id::COUNTDOWN => None,
            id::TELEPORT => crate::rules::world::objects::plan(call).map(Call::Put),
            id::SET_LOADOUT => {
                let tag = |n: usize| match call.args.get(n) {
                    Some(Arg::Tag(t)) => t.clone(),
                    _ => None,
                };
                let (primary, secondary) = (tag(1), tag(2));
                p.filter(|_| primary.is_some() || secondary.is_some())
                    .map(|player| Call::SetLoadout {
                        player,
                        primary,
                        secondary,
                    })
            }
            // A bot order is a hint to a phantom's brain on the host. It is
            // not replayed: a film plays each seat's recorded pad instead.
            id::BOT_GO | id::BOT_HOLD | id::BOT_FOLLOW | id::BOT_ATTACK | id::BOT_FREE => {
                if self.film {
                    None
                } else {
                    crate::rules::world::bots::plan(self.m, self.table, call)
                        .map(|(player, order)| Call::BotOrder { player, order })
                }
            }
            id::BOT_GOAL_ATTACK | id::BOT_GOAL_DEFEND | id::BOT_GOAL_HOLD | id::BOT_GOAL_CLEAR => {
                if self.film {
                    None
                } else {
                    crate::rules::world::bots::goal(self.m, self.table, call)
                        .map(|(team, goal)| Call::BotGoal { team, goal })
                }
            }
            _ => None,
        };
        if let Some(c) = made {
            self.calls.push(c);
        }
    }

    /// A mover's step is made at the boundary like a `teleport`.
    fn step_object(&mut self, object: u32, at: [i32; 3], _velocity: [i32; 3]) {
        self.calls
            .push(Call::Put(crate::rules::world::objects::Place::of_fixed(
                object, at,
            )));
    }

    fn script_error(&mut self, error: &ScriptError) {
        self.error = Some(*error);
        self.calls.push(Call::End);
    }
}
