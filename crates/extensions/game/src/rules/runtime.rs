//! The rules VM in the running game: the [`Game`] over guest memory, one
//! game's script and state, and what each moment of the game does with it
//! (docs/game/rules-engine.md, "The host runtime").
//!
//! The feature (`super::feature`) and the superbounce mode call in at eight
//! moments:
//!
//! - [`Runtime::start`] after a successful `GAME_ACTIVATE`: read the
//!   record's script reference, find the blob, verify it, refuse what no
//!   host can run, check the map's labels (the start gate), refuse what
//!   this host cannot run yet by name, start the VM, run `on init` and the
//!   first round's `on round_start`, and set the `RULES` record. A
//!   refusal is kept with its reason and logged; the game then plays as
//!   record's engine's lifted rules. A map the gate
//!   blocks plays no rules at all: [`Status::Blocked`] keeps every problem,
//!   no VM is made, and the first tick boundary ends the game
//!   (docs/game/rules-start-gate.md, "What a blocked start shows"). A
//!   script with a `uses` line, after the policy check, runs that
//!   type's built-in script under the record ([`used_builtin`],
//!   [`Carried`]): every step after that is the built-in's, and a
//!   refusal names both.
//! - [`Runtime::pre_tick`] before each game tick: the VM's tick begins and
//!   its queued calls join the calls the handlers made since the last
//!   boundary. The feature makes them, in order, in an async body
//!   ([`run_work`]) and then runs the tick. At the first boundary the body
//!   also asks `GAME_ENGINE_AUTHORITATIVE`; a machine that is not the
//!   authority drops the script and its record.
//! - [`Runtime::kill`] at `PLAYER_KILLED`'s entry: `on kill`.
//! - [`Runtime::round_start`] at the engine's round reset
//!   (`ENGINE_ROUND_RESET`) as each later round begins: `on round_start`.
//! - [`Runtime::quit`] at the engine's player-left notice
//!   (`ENGINE_PLAYER_LEFT`): `on quit`.
//! - [`Runtime::physics`] and then [`Runtime::bounces`] after each game
//!   tick, from the superbounce mode's medal chain: `on touchdown`, `on
//!   launch` and `on grounded` for what the chain saw of each player's
//!   physics (`super::world::physics`), then `on bounce` and `on bounced`,
//!   each bounce and Bounced on the tick its chain files it.
//! - [`Runtime::post_tick`] after each game tick: the roster's and the
//!   zones' changes (`on death`, `on spawn`, `on leave`, `on enter`;
//!   `super::world::zones`), then timers, then `on tick`.
//! - [`Runtime::ended`] after `GAME_END`: the record is removed.
//!
//! The agent API's `rules.reload` (`super::reload`) calls
//! [`Runtime::swap`] at a tick boundary for an in-place reload.
//!
//! **Calls wait for the tick boundary.** `PLAYER_KILLED` is opaque, so a
//! hook there can only observe, and the per-tick group runs after
//! `GAME_TICK`, not in place of the engine update `0x15c150`. So every
//! call a handler makes is kept and made by the `before` on the next
//! `GAME_TICK`, through the mechanism Firefight's tick hook uses, ahead of
//! the VM's queued calls. A score therefore lands at the next tick
//! boundary rather than inside the kill, and a read in the same handler
//! does not see it. On the script index no retail engine scores beside
//! it: the engine object's kill slot is an empty body
//! (`super::engine_object`).
//!
//! **State.** The VM's state is the `RULES` record of the `SimExt` blob
//! ([`Store`]), written after every run that changed it. Before each run
//! the record is compared with what was last written; a checkpoint revert
//! or a snapshot load that changed it resumes the VM from it, and a record
//! that is gone (a full restore) stops the script with a refusal.

use std::collections::VecDeque;
use std::sync::Arc;

use ::rules::caps::{self, Kind, Status as EntryStatus, id};
use ::rules::ir::{Event, Ir, Op, TagGroup};
use ::rules::vm::{Fired, Game, Input, RulesExt, ScriptError, TICK_BUDGET, Vm};
use ::rules::policy::{self, Context};
use ::rules::{Sha256, verify};
use ::rules::types::Type;
use agent_proto::rules as wire;
use guest_layout::GuestMemory;
use variants::ext::{Ext, Mode};

use super::census::{self, Table};
use super::guest;
use super::world::data;
use super::world::map_tags::{self, TagIndex};
use super::world::zones::{self, Change, Seen, Zones};
use sim_state::SimEvent;
use crate::firefight::runtime::read_players;
use forge_doc::LabelledThing;

/// The compiled-in scripts.
mod builtins;
/// The game the VM reads and changes at one moment.
mod guest_game;
/// What the authority's script publishes after each tick.
mod publish;
/// The tick boundary's async body.
mod work;

pub use self::builtins::{Builtin, builtins, compiled_in, examples, listed, matchmaking};
pub use guest_game::GuestGame;
use publish::ShowCue;
pub use work::{Refused, make_calls, run_work};

/// The match seed until the session data carries one (as Firefight's).
pub const SEED: u64 = 0;
/// Engine indices whose built-in script runs for a record with no script
/// mode (rules-engine.md, "A record with no mode"): Juggernaut (7), whose
/// events and entries this host all delivers and makes.
pub const PORTED: &[u8] = &[7];
/// The engine refusals `rules.state` keeps.
pub const ERRORS_KEPT: usize = 16;
/// The options mode of a multiplayer game (`[GAME_GLOBALS] + 8`).
pub const MODE_MULTIPLAYER: u32 = 2;

/// The events this runtime delivers. A script answering any other is
/// refused at load by the event's name.
pub const DELIVERED: &[Event] = &[
    Event::Init,
    Event::RoundStart,
    Event::Kill,
    Event::Quit,
    Event::Timer,
    Event::Tick,
    Event::Bounce,
    Event::Bounced,
    // From the superbounce mode's medal chain, while it runs
    // (`super::world::physics`).
    Event::Touchdown,
    Event::Launch,
    Event::Grounded,
    // From the roster and the zones at each tick's end
    // (`super::world::zones`).
    Event::Spawn,
    Event::Death,
    Event::Enter,
    Event::Leave,
    // The VM's own, when a mover ends (`Vm::update`).
    Event::Arrive,
    // From the tick's `SimEvent::Damage` events (`Runtime::post_tick`,
    // `super::world::data::damage_inputs`).
    Event::Damage,
];
/// The read entries this runtime answers.
pub const READS: &[u16] = &[
    id::PLAYER_SCORE,
    id::TEAM_SCORE,
    id::KILLS,
    id::DEATHS,
    id::ALIVE,
    id::TEAM_OF,
    // A HUD widget's side and slot (`super::world::hud`).
    id::SIDE_OF,
    id::SLOT_OF,
    id::LIVES,
    id::ROUND_TIME_LEFT,
    id::IS_OBJECTIVE,
    id::CARRIER_OF,
    // The labelled things' reads (`super::census::Table`).
    id::LABEL_NUMBER,
    id::LABEL_OF,
    id::ZONE_ID,
    id::OBJECT_TEAM,
    // The zone test and the roster's units (`super::world::zones`).
    id::INSIDE,
    id::PLAYER_OF,
    // An object's position and velocity (`super::world::objects`).
    id::OBJECT_POS,
    id::OBJECT_VEL,
    // A unit's vitality (`super::world::data::vitality`).
    id::OBJECT_HEALTH,
    id::OBJECT_SHIELD,
    // A field of a tag the map holds (`super::world::map_tags::read`).
    id::TAG_INT,
    id::TAG_FIXED,
    // The superbounce chain's biped snapshot (super::world::physics).
    id::AIRBORNE,
    id::CROUCHING,
    id::PRIMED,
];
/// The action entries this runtime makes.
pub const ACTIONS: &[u16] = &[
    id::SCORE,
    id::END_ROUND,
    id::NEXT_ROUND,
    id::SET_LIVES,
    id::RESPAWN,
    id::ROLE_TRAITS,
    id::RESET_OBJECTIVE,
    id::HOVER,
    id::UNLOAD,
    id::BOT_GO,
    id::BOT_HOLD,
    id::BOT_FOLLOW,
    id::BOT_ATTACK,
    id::BOT_FREE,
    id::BOT_GOAL_ATTACK,
    id::BOT_GOAL_DEFEND,
    id::BOT_GOAL_HOLD,
    id::BOT_GOAL_CLEAR,
    // Shown to this machine's local users and sent as show cues
    // (`super::world::show`).
    id::MESSAGE,
    id::MESSAGE_TEAM,
    id::MESSAGE_ALL,
    // The multiplayer list's weapons (`super::world::loadout`).
    id::SET_LOADOUT,
    // Played at the audience's units and sent as sound cues
    // (`super::world::audio`).
    id::SOUND_TO,
    id::SOUND_TO_TEAM,
    id::SOUND_TO_ALL,
    // Kept in the VM's state; the spawn chooser's hooks read it
    // (`super::world::spawns`), so no call is made at the boundary.
    id::SPAWN_AREAS,
    id::SPAWN_NEAR,
    // Kept in the VM's state; the spawn's hooks read it
    // (`super::world::body`).
    id::SET_BODY,
    // Kept in the VM's state; the walls are drawn from it every tick
    // (`super::world::walls`), so no call is made at the boundary.
    id::SHOW_AREA,
    id::HIDE_AREA,
    // Kept in the VM's state (`RulesExt::show`); the objective HUDs are
    // fed from it every tick (`super::world::hud`), so no call is made at
    // the boundary.
    id::HUD_VALUE,
    id::HUD_VALUE_FOR,
    id::HUD_FILL,
    id::HUD_STYLE,
    id::HUD_TEXT,
    id::HUD_SHOW,
    id::MARK,
    id::MARK_PLAYER,
    id::MARK_TEAM,
    id::UNMARK,
    // Kept in the VM's state (`RulesExt::objectives`); the objective HUD is
    // fed from it every tick (`crate::objective_hud`), so no call is made at
    // the boundary.
    id::OBJECTIVE,
    id::OBJECTIVE_MARK,
    id::METER,
    id::COUNTDOWN,
    // Put at the boundary (`super::world::objects`); the VM keeps a mover
    // and tells the game each tick's step.
    id::TELEPORT,
    id::MOVE_TO,
    id::STOP,
    // The VM keeps the open barriers and steps each to its place
    // ([`step_object`](Game::step_object)), so no call is made of its own.
    id::SET_OPEN,
    // Kept in the VM's state (`RulesExt::picks`); the boundary opens the
    // picker for each local player from it (`super::world::pick`), so no
    // call is made of the entry's own.
    id::OFFER_PICK,
    id::PICK_ROW,
    // Kept in the VM's state (`RulesExt::loadout`); the boundary asks the
    // player from it (`super::world::choice`), so no call is made of the
    // entry's own.
    id::OFFER,
];

/// Where the `RULES` record lives: the process's `SimExt` blob in the
/// game ([`SimExtStore`]), a local container in tests.
pub trait Store: Send {
    /// A copy of the record; none when there is none.
    fn get(&self) -> Option<Vec<u8>>;
    /// Whether the record's bytes are `written`, compared in place; none
    /// when there is no record. The check before every run, so it copies
    /// nothing.
    fn same(&self, written: &[u8]) -> Option<bool>;
    fn set(&mut self, bytes: Vec<u8>) -> Result<(), String>;
    fn remove(&mut self);
}

/// The process's `SimExt` blob (`sim_world::ext::global`), which checkpoints,
/// snapshots, film keyframes and the sim hash carry.
pub struct SimExtStore;

impl Store for SimExtStore {
    fn get(&self) -> Option<Vec<u8>> {
        sim_world::ext::global().ext.get(&sim_world::ext::RULES).map(<[u8]>::to_vec)
    }
    fn same(&self, written: &[u8]) -> Option<bool> {
        sim_world::ext::global().ext.get(&sim_world::ext::RULES).map(|now| now == written)
    }
    fn set(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        sim_world::ext::global()
            .ext
            .set(&sim_world::ext::RULES, bytes)
            .map_err(|e| e.to_string())
    }
    fn remove(&mut self) {
        sim_world::ext::global().ext.remove(&sim_world::ext::RULES);
    }
}

/// Fetches a blob that is neither built in nor on this machine, by the
/// hash prefix the record carries. In the game it is
/// [`crate::game_variants::GameScripts::fetched`]: the outcome of the Live
/// fetch the start hook made with no guest memory held, so it never waits.
pub type Fetch = dyn Fn([u8; 8]) -> Result<Vec<u8>, String> + Send + Sync;

/// The blob on this machine (the game-variant store) whose SHA-256 starts
/// with the prefix, if any.
pub type LocalScripts = dyn Fn([u8; 8]) -> Option<Vec<u8>> + Send + Sync;

/// The loaded game's labelled things, for the start gate's census and the
/// script's label reads. In the game it is [`census::things_for`]: the loaded
/// scenario's netgame flags and starting locations in guest memory, with the
/// built-in layout of the script (`census::layout_script`) on the loaded map.
pub type CensusSource = dyn Fn(&dyn GuestMemory, Option<&str>) -> Result<Vec<LabelledThing>, String> + Send + Sync;

/// The loaded map's tag paths, for the start gate's tag reads and the
/// script's `tag_int` and `tag_fixed`. In the game it is
/// [`map_tags::current`]: the index read from the map file at
/// `GAME_ACTIVATE`.
pub type TagSource = dyn Fn() -> Arc<TagIndex> + Send + Sync;

/// No fetch: refused by name.
pub fn no_fetch(prefix: [u8; 8]) -> Result<Vec<u8>, String> {
    Err(format!(
        "no built-in or stored script has hash prefix {}, and this runtime has no Live client to fetch it",
        hex(&prefix)
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Where a running blob came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin,
    /// The game-variant store on this machine.
    Local,
    Live,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Builtin => "builtin",
            Source::Local => "local",
            Source::Live => "live",
        }
    }
}

/// One engine call a handler made, made at the next tick boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    StartLine(Type),
    Score { player: u8, delta: i32 },
    SetLives { player: u8, lives: i32 },
    Respawn(u8),
    /// `role_traits`: the carrier's role given or taken (`super::roles`).
    RoleTraits { player: u8, on: bool },
    /// `reset_objective`: the objective (a salted datum) returned
    /// (`super::objectives`).
    ResetObjective(u32),
    Vehicle(super::world::vehicles::Action, Option<u32>),
    /// `bot_go`, `bot_hold`, `bot_follow`, `bot_attack` and `bot_free`: an
    /// outside order to the phantom in `player`'s seat, or `None` to free
    /// it (`super::world::bots`).
    BotOrder {
        player: u8,
        order: Option<bots::brain::Order>,
    },
    /// `bot_goal_attack`, `bot_goal_defend`, `bot_goal_hold` and
    /// `bot_goal_clear`: a place set in a team's bot goals, or the team's
    /// cleared (`None`) (`super::world::bots::goal`).
    BotGoal {
        team: u8,
        goal: Option<bots::objective::script::ScriptGoal>,
    },
    /// `end_round`, or the round end of a script error.
    End,
    /// `next_round`: the round ends and the game goes on
    /// (`guest::next_round`).
    NextRound,
    /// `message`, `message_team` or `message_all`: not an engine call but
    /// a show cue, made at the boundary by [`Runtime::pre_tick`]
    /// (`super::world::show`).
    Message(super::world::show::Outgoing),
    /// `sound_to`, `sound_to_team` or `sound_to_all`: a sound cue and a play
    /// for this machine's players, made at the boundary by
    /// [`Runtime::pre_tick`] (`super::world::audio`).
    SoundTo { to: ::rules::show::To, path: String },
    /// The play of a sound the boundary published: the sound's tag path to
    /// this machine's own players in the audience.
    Play {
        audience: ::rules::show::Audience,
        path: String,
    },
    /// `set_loadout`: the player's living unit gets the weapons, by tag
    /// path (`super::world::loadout`).
    SetLoadout {
        player: u8,
        primary: Option<String>,
        secondary: Option<String>,
    },
    /// `teleport`, or a mover's step: the object put at a place
    /// (`super::world::objects`).
    Put(super::world::objects::Place),
    /// `offer_pick`: the value picker opened for one local player with the
    /// offer's rows (`super::world::pick`).
    OfferPick(super::world::pick::Ask),
    /// `offer`: the loadout picker opened for one local player with the
    /// rows their window holds (`super::world::choice`).
    OfferChoice(super::world::pick::Ask),
}

impl Call {
    /// The API entry's name.
    pub fn name(&self) -> &'static str {
        match self {
            Call::StartLine(_) => "start_line",
            Call::Score { .. } => "score",
            Call::SetLives { .. } => "set_lives",
            Call::Respawn(_) => "respawn",
            Call::RoleTraits { .. } => "role_traits",
            Call::Vehicle(action, _) => action.name(),
            Call::ResetObjective(_) => "reset_objective",
            Call::BotOrder { order, .. } => match order {
                Some(bots::brain::Order::Go(_)) => "bot_go",
                Some(bots::brain::Order::Hold(..)) => "bot_hold",
                Some(bots::brain::Order::Follow(_)) => "bot_follow",
                Some(bots::brain::Order::Attack(_)) => "bot_attack",
                None => "bot_free",
            },
            Call::BotGoal { goal, .. } => match goal {
                Some(g) => g.aim.entry(),
                None => "bot_goal_clear",
            },
            Call::End => "end_round",
            Call::NextRound => "next_round",
            Call::Message(_) => "message",
            Call::SoundTo { .. } | Call::Play { .. } => "sound",
            Call::SetLoadout { .. } => "set_loadout",
            Call::Put(_) => "put_object",
            Call::OfferPick(_) => "offer_pick",
            Call::OfferChoice(_) => "offer",
        }
    }
}

/// What the async body before a tick does.
#[derive(Clone, Debug, PartialEq)]
pub struct Work {
    /// The tick about to run.
    pub tick: u32,
    /// Ask `GAME_ENGINE_AUTHORITATIVE` first, and make the calls only when
    /// it says yes.
    pub ask_authority: bool,
    pub calls: Vec<Call>,
    /// Ordered object pairs to trace after the calls, before the tick.
    pub sight: Vec<super::world::physics::SightQuery>,
}

/// Why a script is not hostable here, by the first thing this runtime
/// cannot do; `None` when it can run it all. Entries no host can make are
/// named first ([`unhostable_refusal`]), then what this runtime does not
/// do yet: an event it does not deliver, an entry it does not answer or
/// make, a variant field whose offset is not traced, a grant.
pub fn host_refusal(ir: &Ir) -> Option<String> {
    unhostable_refusal(ir).or_else(|| runtime_refusal(ir))
}

/// An unavailable script entry, in words.
pub fn unhostable_refusal(ir: &Ir) -> Option<String> {
    ir.handlers.iter().flat_map(|h| &h.code).find_map(|op| {
        let Op::Call { entry, .. } = op else { return None };
        let e = caps::entry(*entry, ir.header.api_version)?;
        let status = match e.status {
            EntryStatus::Traced => "traced but not a named point",
            EntryStatus::Untraced => "untraced",
            EntryStatus::NotBuilt => "not built",
            EntryStatus::Named | EntryStatus::VmOnly => return None,
        };
        Some(format!(
            "the script calls {}, whose engine entry is {status}",
            caps::spelled(e)
        ))
    })
}

/// The retail ball return looks up a spawn in Oddball's engine globals.
pub(super) fn record_refusal(ir: &Ir, engine: i32) -> Option<String> {
    if matches!(variants::GameType::from_engine(engine), variants::GameType::Other(_)) {
        return Some(format!("the record has unsupported engine index {engine}"));
    }
    if engine != variants::GameType::Oddball.engine()
        && ir.handlers.iter().flat_map(|h| &h.code)
            .any(|op| matches!(op, Op::Call { entry, .. } if *entry == id::RESET_OBJECTIVE))
    {
        return Some("reset_objective needs the active Oddball record's ball spawns".into());
    }
    None
}

/// What this runtime does not do yet, of a script every host could run.
fn runtime_refusal(ir: &Ir) -> Option<String> {
    // `set_loadout` gives only the multiplayer list's weapons
    // (`super::world::loadout`): a script that calls it and names another
    // weapon is refused at the start, by the weapon's path.
    let loads = ir
        .handlers
        .iter()
        .flat_map(|h| &h.code)
        .any(|op| matches!(op, Op::Call { entry, .. } if *entry == id::SET_LOADOUT));
    if loads
        && let Some(t) = ir
            .tags
            .iter()
            .find(|t| t.group == TagGroup::Weapon && ::rules::weapons::of_path(&t.path).is_none())
    {
        return Some(format!(
            "the script names the weapon {}, and set_loadout gives only the multiplayer list's weapons",
            t.path
        ));
    }
    // A sound is found by its tag path in the loaded map's file: a host with
    // no lookup, or a map without the sound, refuses the script by its path.
    if let Some(why) = super::world::audio::refusal(ir) {
        return Some(why);
    }
    let superbounce = super::header::asks_superbounce(ir);
    for hd in &ir.handlers {
        if !DELIVERED.contains(&hd.event) {
            return Some(format!(
                "the script answers on {}, which this host runtime does not deliver yet",
                hd.event.name()
            ));
        }
        for op in &hd.code {
            match op {
                Op::Call { entry, .. } => {
                    let Some(e) = caps::entry(*entry, ir.header.api_version) else {
                        continue;
                    };
                    match e.kind {
                        Kind::Read if matches!(e.id, id::AIRBORNE | id::CROUCHING | id::PRIMED)
                            && !superbounce =>
                        {
                            return Some(format!(
                                "the script reads {}, which needs the superbounce mode (add `requires {}`)",
                                e.name, super::header::SUPERBOUNCE
                            ));
                        }
                        Kind::Read if !READS.contains(&e.id) => {
                            return Some(format!(
                                "the script reads {}, which this host runtime does not answer yet",
                                e.name
                            ));
                        }
                        Kind::Action if !ACTIONS.contains(&e.id) => {
                            return Some(format!(
                                "the script calls {}, which this host runtime does not make yet",
                                e.name
                            ));
                        }
                        _ => {}
                    }
                }
                Op::Variant { field, .. } => {
                    let name = caps::VARIANT_FIELDS
                        .get(*field as usize)
                        .map_or("?", |&(n, _)| n);
                    if !guest::VARIANT_FIELDS_READ.contains(&name) {
                        return Some(format!(
                            "the script reads variant.{name}, whose record offset is not traced"
                        ));
                    }
                }
                Op::Grant { cap, .. } => {
                    let name = ir.header.requires.get(*cap as usize).map_or("?", |s| s.as_str());
                    return Some(format!(
                        "the script grants {name}, and grants need the match edit path, which is not built"
                    ));
                }
                _ => {}
            }
        }
    }
    None
}

/// One game's script.
pub struct Running {
    /// The physical record, including the script index.
    pub(super) record_engine: i32,
    /// The identity peers see in the record, also across in-place reloads.
    pub(super) record_prefix: [u8; 8],
    pub vm: Vm,
    /// The type of the record's script, including a settings-only variant.
    game_type: Type,
    /// Wait for the players' machines to enter the round before posting it.
    start_line: Option<Type>,
    pub hash: Sha256,
    pub source: Source,
    pub options: u32,
    /// The record's bytes as this runtime last wrote them.
    written: Vec<u8>,
    /// Calls made since the last tick boundary, in order.
    pending: Vec<Call>,
    /// What `GAME_ENGINE_AUTHORITATIVE` answered; none before it was asked.
    pub authority: Option<bool>,
    /// The tick the last [`Runtime::pre_tick`] and [`Runtime::post_tick`]
    /// ran for.
    pre: Option<u32>,
    post: Option<u32>,
    /// The map's labelled things the script reads.
    pub table: Table,
    /// The zones of the map's flags (`super::world::zones`).
    pub zones: Zones,
    /// What the VM was last told of the roster and the zones.
    seen: Seen,
    /// What the start gate checked, and for which teams: a swap is checked
    /// against them again.
    census: ::rules::labels::Census,
    teams: Vec<u8>,
    /// The map's tag paths at the start: a swapped-in script's tag reads
    /// are checked against them, and the VM's reads go through them.
    tags: Arc<TagIndex>,
    /// The `uses` script the record names, when `vm` runs the built-in it
    /// uses.
    pub carried: Option<Carried>,
    /// The program answers `on damage` ([`wants_damage`]): worked out at
    /// the start and at a swap, not at every tick.
    wants_damage: bool,
}

impl Running {
    /// Calls waiting for the next tick boundary.
    pub fn pending(&self) -> &[Call] {
        &self.pending
    }
}

/// The `uses` script a game's record names
/// (docs/game/rules-variants.md, "A script that keeps its carrier's
/// rules"): settings and a header, no handlers. The game runs the built-in
/// script of the type it uses ([`used_builtin`]) under the record, and
/// this keeps what the record named. The header's `requires` turn their
/// modes on through the record's script reference (`super::header`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carried {
    pub hash: Sha256,
    pub source: Source,
    pub name: String,
}

/// Whether the program answers `on damage`, so a tick's damage events are
/// worked out for it.
fn wants_damage(ir: &Ir) -> bool {
    ir.handlers.iter().any(|h| h.event == Event::Damage)
}

/// Whether `hash` is one of the built-in ports ([`builtins`]).
fn is_builtin(hash: &Sha256) -> bool {
    builtins().iter().any(|b| b.id == *hash)
}

/// The built-in script a `uses` script's game runs: the one of the type
/// it uses ([`builtins`], `rules::builtin::of_type`). A type with none is
/// refused by name.
pub fn used_builtin(ir: &Ir) -> Result<Ir, String> {
    let Some(uses) = ir.header.uses else {
        return Err(format!("{} uses no built-in game", ir.header.name));
    };
    let engine = uses.engine();
    let Some(b) = builtins().iter().find(|b| b.engine == engine).filter(|_| uses.is_retail()) else {
        return Err(format!(
            "{} uses {}, which has no built-in script",
            ir.header.name,
            uses.name()
        ));
    };
    verify::load(&b.blob)
        .map(|(ir, _)| ir)
        .map_err(|e| format!("the built-in {} script does not verify: {e}", uses.name()))
}

/// A refusal of the script that runs, in words that name the `uses`
/// script it runs for, if any.
fn for_carried(carried: Option<&Carried>, running: &Ir, why: String) -> String {
    match carried {
        Some(c) => format!(
            "{} uses the built-in {} script, which this host cannot run yet: {why}",
            c.name, running.header.name
        ),
        None => why,
    }
}

/// The problems the map's tags give a script's `tag_int` and `tag_fixed`
/// reads (`::rules::fields::check`): a tag the map lacks, a group or field
/// our definitions lack, a field of the wrong kind. A map whose tag paths
/// could not be read is one problem for a script that reads any.
fn tag_problems(ir: &Ir, index: &TagIndex) -> Vec<String> {
    if ::rules::fields::uses(ir).is_empty() {
        return Vec::new();
    }
    if index.is_empty() {
        return vec![format!(
            "the map's tag paths could not be read, and {} reads tag fields",
            ir.header.name
        )];
    }
    let defs = tags::defs::Defs::builtin();
    let schema = data::DefsSchema { group: |tag: &str| index.definition(defs, tag) };
    ::rules::fields::check(ir, &schema).into_iter().map(|p| p.text).collect()
}

/// Whether a script runs in this game, and why not.
pub enum Status {
    Idle(&'static str),
    /// `hash`: the full hash, or the record's prefix when the blob was not
    /// found.
    Refused { hash: Option<String>, why: String },
    /// The start gate found the map lacks or breaks what the script needs
    /// (docs/game/rules-start-gate.md): no VM, and the carrier's rules do
    /// not play either. `end`: the game is still to be ended at the next
    /// tick boundary; a Forge session (`forge`) is never ended.
    Blocked {
        hash: String,
        problems: Vec<String>,
        forge: bool,
        end: bool,
        table: Table,
    },
    Running(Box<Running>),
}

/// The runtime for the whole run: one game at a time.
pub struct Runtime {
    pub status: Status,
    errors: VecDeque<wire::Refusal>,
    store: Box<dyn Store>,
    local: Box<LocalScripts>,
    fetch: Box<Fetch>,
    /// The kind of game the next start plays, for the script policy
    /// (`rules::policy`): custom until the feature says otherwise.
    context: Context<'static>,
    /// The map's labelled things at a start.
    census: Box<CensusSource>,
    /// The map's tag paths at a start.
    tags: Box<TagSource>,
    /// This run plays a film back (Theater): the start gate does not run,
    /// since the film's host passed it.
    film: bool,
    /// The next start is a Forge session's (`lobby_start::Mode::Forge`):
    /// a block refuses the script and does not end the game.
    forge: bool,
    /// The last tick published walls: the next tick with none clears them.
    walled: bool,
    /// The last tick fed the objective HUDs: the next tick with no script
    /// feeding them hides them.
    hud_on: bool,
    /// The same for the scripted objective HUD (`crate::objective_hud`).
    objective_on: bool,
    /// The walls cue last sent on the actor channel, and the tick it was
    /// sent: the list is sent when it changes and again every
    /// `::rules::show::WALLS_REPEAT` ticks while it is not empty, so a
    /// machine that joins late has it (`super::world::show::walls`).
    wall_cue: Option<(Vec<(u32, Option<u8>)>, u32)>,
    /// The pickers' offers last sent as `Pick` cues on the actor channel,
    /// and the tick they were sent: sent when they change and again every
    /// `::rules::show::PICK_REPEAT` ticks while any stands, so a machine
    /// that joins late has them (`super::world::show::pick_offer`).
    pick_cue: Option<(Vec<::rules::vm::PickOffer>, u32)>,
    /// The show state last offered to the actor channel and its bytes:
    /// encoded again only when the state changes ([`ShowCue`]).
    show_cue: ShowCue,
    /// The players' bodies at the superbounce chain's last step: what
    /// `airborne` and `primed` read (`super::world::physics`).
    bodies: Vec<crate::superbounce_chain::Body>,
}

fn describe(e: &ScriptError) -> String {
    format!(
        "script error {:?} at tick {}, handler {}, op {}",
        e.kind, e.tick, e.handler, e.pc
    )
}

/// The record's bytes changed since `r` wrote them (a revert or a load):
/// resume from them. A record that is gone stops the script. The record
/// is compared in place and copied only when it changed.
fn sync(r: &mut Running, store: &dyn Store) -> Result<(), String> {
    const CLEARED: &str = "the RULES record was cleared (a full restore); the script stops";
    match store.same(&r.written) {
        None => return Err(CLEARED.into()),
        Some(true) => return Ok(()),
        Some(false) => {}
    }
    let Some(now) = store.get() else {
        return Err(CLEARED.into());
    };
    let ext = RulesExt::decode(&now).map_err(|e| format!("the RULES record: {e}"))?;
    r.vm = Vm::resume(r.vm.ir().clone(), r.options, ext)
        .map_err(|e| format!("resuming from the RULES record: {e}"))?;
    r.written = now;
    Ok(())
}

/// Writes the VM's state when it changed. The store takes the encoded
/// bytes and `written` keeps its copy in its own buffer. A write the store
/// refuses stops the script, so `written` is not read after it.
fn save(r: &mut Running, store: &mut dyn Store) -> Result<(), String> {
    let bytes = r.vm.ext().encode();
    if bytes != r.written {
        r.written.clear();
        r.written.extend_from_slice(&bytes);
        store.set(bytes)?;
    }
    Ok(())
}

impl Runtime {
    pub fn new(store: Box<dyn Store>, fetch: Box<Fetch>) -> Self {
        Self {
            status: Status::Idle("no game has started"),
            errors: VecDeque::new(),
            store,
            local: Box::new(|_: [u8; 8]| None),
            fetch,
            context: Context::Custom,
            census: Box::new(census::things_for),
            tags: Box::new(map_tags::current),
            film: false,
            forge: false,
            walled: false,
            hud_on: false,
            objective_on: false,
            wall_cue: None,
            pick_cue: None,
            show_cue: ShowCue::default(),
            bodies: Vec::new(),
        }
    }

    /// The same runtime for a run that plays a film back, or not.
    pub fn with_film(mut self, film: bool) -> Self {
        self.film = film;
        self
    }

    /// The same runtime with `census` for the map's labelled things.
    pub fn with_census(mut self, census: Box<CensusSource>) -> Self {
        self.census = census;
        self
    }

    /// The same runtime with `tags` for the map's tag paths.
    pub fn with_tags(mut self, tags: Box<TagSource>) -> Self {
        self.tags = tags;
        self
    }

    /// Whether the next [`Runtime::start`] is a Forge session's.
    pub fn set_forge_session(&mut self, forge: bool) {
        self.forge = forge;
    }

    /// The problems the start gate blocked this game on, while it holds
    /// the block: the seam the lobby's verdict reads
    /// (docs/game/rules-start-gate.md, "What a blocked start shows").
    pub fn blocked(&self) -> Option<&[String]> {
        match &self.status {
            Status::Blocked { problems, .. } => Some(problems),
            _ => None,
        }
    }

    /// The start gate (docs/game/rules-start-gate.md, "The check"): the
    /// map's labelled things, and every problem the script's label and tag
    /// tables have with them for `teams`. A census that cannot be read is a
    /// problem for a script with either table: no script runs on a map the
    /// check has not passed. A script with neither has nothing to check.
    fn gate(&self, m: &dyn GuestMemory, ir: &Ir, teams: &[u8]) -> (Vec<LabelledThing>, Vec<String>) {
        let unchecked = self.film || (ir.labels.is_empty() && ir.tags.is_empty());
        match (self.census)(m, census::layout_script(ir)) {
            Ok(things) if unchecked => (things, Vec::new()),
            Ok(things) => {
                let census = census::census(&things);
                let problems = ::rules::labels::check(ir, &census, teams)
                    .into_iter()
                    .map(|p| p.text)
                    .collect();
                (things, problems)
            }
            Err(_) if unchecked => (Vec::new(), Vec::new()),
            Err(e) => (Vec::new(), vec![format!("the map's labels could not be read: {e}")]),
        }
    }

    /// Blocks the game on `problems`: no VM, no record, and (outside a
    /// Forge session) one `end_round` at the next tick boundary.
    fn block(&mut self, hash: Sha256, problems: Vec<String>, table: Table) -> String {
        self.store.remove();
        let forge = self.forge;
        let line = if forge {
            format!(
                "rules: blocked in a Forge session, the script does not run until the map has what it needs: {}",
                problems.join("; ")
            )
        } else {
            format!("rules: blocked, the game does not start: {}", problems.join("; "))
        };
        self.status = Status::Blocked {
            hash: hash.to_string(),
            problems,
            forge,
            end: !forge,
            table,
        };
        line
    }

    /// The kind of game the next [`Runtime::start`] plays. The feature sets
    /// a hopper for a matchmade game and custom otherwise; a ranked
    /// session's script is held to the ranked policy by Live, which knows
    /// the session's flags.
    pub fn set_context(&mut self, context: Context<'static>) {
        self.context = context;
    }

    pub fn context(&self) -> Context<'static> {
        self.context
    }

    /// The same runtime with `local` for the blobs on this machine.
    pub fn with_local(mut self, local: Box<LocalScripts>) -> Self {
        self.local = local;
        self
    }

    /// The runtime the game uses: the process's `SimExt` blob, and the
    /// game-variant store and Live through `scripts`.
    pub fn in_game(scripts: std::sync::Arc<crate::game_variants::GameScripts>) -> Self {
        let (l, f) = (std::sync::Arc::clone(&scripts), scripts);
        Self::new(Box::new(SimExtStore), Box::new(move |p: [u8; 8]| f.fetched(p)))
            .with_local(Box::new(move |p: [u8; 8]| l.local(p)))
    }

    fn error(&mut self, tick: u32, call: &str, error: impl std::fmt::Display) {
        let row = wire::Refusal {
            tick,
            call: call.into(),
            error: error.to_string(),
        };
        eprintln!("rules: {} at tick {tick}: {}", row.call, row.error);
        if self.errors.len() == ERRORS_KEPT {
            self.errors.pop_front();
        }
        self.errors.push_back(row);
    }

    pub fn running(&self) -> Option<&Running> {
        match &self.status {
            Status::Running(r) => Some(r),
            _ => None,
        }
    }

    /// The starting locations the spawn of player `handle` (the chooser's
    /// ECX) may pick, by its team's spawn rule (`super::world::spawns`):
    /// none while no script makes the game's rules here, when the team has
    /// no rule, or when the map has none of the rule's locations.
    pub fn spawn_filter(&self, m: &dyn GuestMemory, handle: u32) -> Option<super::world::spawns::Filter> {
        let r = self.running().filter(|r| r.authority == Some(true))?;
        let slot = (handle & 0xffff) as u8;
        let team = read_players(m)
            .ok()?
            .into_iter()
            .find(|(p, _)| p.slot == slot)?
            .0
            .team;
        let rule = r.vm.spawn_rule(team)?;
        let starts = super::world::spawns::starts(m).ok()?;
        super::world::spawns::filter(Some(rule), &starts, &r.table, &r.zones)
    }

    /// The body player `handle` (`PLAYER_SPAWN`'s argument) spawns in, by
    /// its team's `set_body` (`super::world::body`), with its roster slot:
    /// none while no script makes the game's rules here, or when the team
    /// has no body.
    pub fn spawn_body(&self, m: &dyn GuestMemory, handle: u32) -> Option<(u8, ::rules::bodies::Body)> {
        let r = self.running().filter(|r| r.authority == Some(true))?;
        let slot = (handle & 0xffff) as u8;
        let team = read_players(m)
            .ok()?
            .into_iter()
            .find(|(p, _)| p.slot == slot)?
            .0
            .team;
        Some((slot, r.vm.body(team)?))
    }

    /// Whether a script runs here and this machine is the game's
    /// authority, so the carrier's round-start line is the script's to
    /// make (`super::start_line::replace_carrier`). Before the first tick
    /// boundary asks for the authority, the engine keeps its line.
    pub fn has_authority(&self) -> bool {
        matches!(&self.status, Status::Running(r) if r.authority == Some(true))
    }

    /// The running blob's full hash, for the Live session's `rules` field.
    /// A `uses` script's game names the built-in it uses, which every
    /// build has.
    pub fn hash(&self) -> Option<String> {
        self.running().map(|r| r.hash.to_string())
    }

    fn refuse(&mut self, hash: Option<String>, why: String) -> String {
        let line = format!("rules: refused, the game plays the record engine's own rules: {why}");
        self.store.remove();
        self.status = Status::Refused { hash, why };
        line
    }

    /// A game started. `system_link`: the squad session is System Link,
    /// which runs only the built-in scripts. Returns the line to log.
    pub fn start(&mut self, m: &dyn GuestMemory, system_link: bool) -> String {
        self.errors.clear();
        self.bodies.clear();
        self.store.remove();
        if bots::guest::game_mode(m) != Some(MODE_MULTIPLAYER) {
            self.status = Status::Idle("not a multiplayer game");
            return "rules: idle: not a multiplayer game".into();
        }
        let (Ok(Some(block)), Ok(Some(engine))) = (guest::block(m), guest::engine_index(m)) else {
            return self.refuse(None, "the active variant record could not be read".into());
        };
        let script = Ext::in_effect(&block).script;
        let found = match script.mode {
            Mode::Rules => {
                match compiled_in(script.prefix) {
                    Some(b) => Ok((b.blob.clone(), Source::Builtin)),
                    None => match (self.local)(script.prefix) {
                        Some(blob) => Ok((blob, Source::Local)),
                        None if system_link => Err(format!(
                            "a System Link game runs only the scripts on this machine (built in or in its game-variant store), and none has hash prefix {}",
                            hex(&script.prefix)
                        )),
                        None => (self.fetch)(script.prefix).map(|blob| (blob, Source::Live)),
                    },
                }
            }
            Mode::None => {
                let ported = u8::try_from(engine).ok().filter(|e| PORTED.contains(e));
                match ported.and_then(|e| builtins().iter().find(|b| b.engine == e)) {
                    Some(b) => Ok((b.blob.clone(), Source::Builtin)),
                    None => {
                        self.status = Status::Idle("the record names no script");
                        return "rules: idle: the record names no script".into();
                    }
                }
            }
            Mode::Firefight => {
                self.status = Status::Idle("the record names Firefight, which its own feature runs");
                return "rules: idle: the record names Firefight".into();
            }
            Mode::Unknown(n) => Err(format!("script mode {n} is unknown to this build")),
        };
        let prefix = Some(hex(&script.prefix));
        let (blob, source) = match found {
            Ok(found) => found,
            Err(why) => return self.refuse(prefix, why),
        };
        let (ir, _) = match verify::load(&blob) {
            Ok(loaded) => loaded,
            Err(e) => return self.refuse(prefix, format!("the blob does not verify: {e}")),
        };
        let hash = ir.id();
        if hash.0[..8] != script.prefix && script.mode == Mode::Rules {
            return self.refuse(
                prefix,
                format!("the blob's hash {hash} does not start with the record's prefix"),
            );
        }
        let full = Some(hash.to_string());
        // The policy first: a refusal by kind of game names the capability.
        if let Some(d) = policy::check(self.context, &ir.header.requires, ir.header.api_version)
            .into_iter()
            .next()
        {
            return self.refuse(full, d.to_string());
        }
        // `uses`: the record's script is settings only, so the game runs
        // the built-in script of the type it uses under the record
        // (its settings, its score limit). From here on `ir` is the
        // built-in's; every refusal names the carried script too.
        let game_type = ir.header.game_type;
        let (ir, hash, source, carried) = match ir.header.uses {
            None => (ir, hash, source, None),
            Some(_) => match used_builtin(&ir) {
                Ok(builtin) => {
                    let carried = Carried {
                        hash,
                        source,
                        name: ir.header.name.clone(),
                    };
                    let id = builtin.id();
                    (builtin, id, Source::Builtin, Some(carried))
                }
                Err(why) => return self.refuse(full, why),
            },
        };
        if let Some(why) = record_refusal(&ir, engine).or_else(|| unhostable_refusal(&ir)) {
            return self.refuse(full, for_carried(carried.as_ref(), &ir, why));
        }
        // A carried script's built-in is refused for what this host cannot
        // run yet before the gate: the variant names no labels, so a map
        // that lacks the built-in's must not end a game the host would
        // refuse anyway.
        if carried.is_some()
            && let Some(why) = runtime_refusal(&ir)
        {
            return self.refuse(full, for_carried(carried.as_ref(), &ir, why));
        }
        // The start gate: the final authority on the map's labels. Theater
        // skips it (`film`): the film's host passed it.
        let teams = if guest::variant(m, "team_play").unwrap_or(0) != 0 {
            GuestGame::new(m, &mut Vec::new()).teams()
        } else {
            Vec::new()
        };
        let tags = (self.tags)();
        let (things, mut problems) = self.gate(m, &ir, &teams);
        if !self.film {
            problems.extend(tag_problems(&ir, &tags));
        }
        let table = Table::new(&things, &ir);
        if !problems.is_empty() {
            return self.block(hash, problems, table);
        }
        if let Some(why) = runtime_refusal(&ir) {
            return self.refuse(full, for_carried(carried.as_ref(), &ir, why));
        }
        let census = census::census(&things);
        let zones = Zones::read(m, &table);
        // A new game's script starts with no bot goals.
        ::bots::objective::script::clear(None);
        // The bots read a hill from the same areas (`bots::objective`),
        // a box turned onto the world's axes with its flag's facing, as
        // the zone scores it.
        let facings = engine::objectives::netgame_flag_facings(m).unwrap_or_default();
        ::bots::objective::set_flag_areas(
            table
                .flag_areas()
                .into_iter()
                .map(|(i, area)| {
                    let yaw = facings.get(i as usize).copied().unwrap_or(0.0);
                    (i, area.turned(yaw))
                })
                .collect(),
        );
        // The record's option bits are its script's; a carried script has
        // no options, so the built-in plays its own defaults. A built-in under a retail record (no script, or a `uses` script's
        // settings) takes its type-area settings as options
        // ([`guest::retail_options`]); a record on the script index has no
        // type area and keeps the record's options.
        let base = if script.mode == Mode::Rules && carried.is_none() {
            script.options
        } else {
            ir.default_options()
        };
        let options = if is_builtin(&hash) {
            match guest::retail_options(m, &ir) {
                Ok(Some(options)) => options,
                Ok(None) => base,
                Err(why) => {
                    return self.refuse(full, format!("the record's settings do not fit the built-in: {why}"));
                }
            }
        } else {
            base
        };
        let name = ir.header.name.clone();
        let damage = wants_damage(&ir);
        let mut vm = match Vm::new(ir, options, SEED) {
            Ok(vm) => vm,
            Err(e) => return self.refuse(full, format!("the VM refused the blob: {e}")),
        };
        // The first round's scores for a sim report (docs/runtime/sim.md,
        // "Rounds"); each later round's is [`Runtime::round_start`]'s.
        crate::sim_rounds::clear();
        crate::sim_rounds::push(crate::sim_rounds::read(m, 0));
        let mut pending = Vec::new();
        let (error, start_line) = {
            let mut g = GuestGame::new(m, &mut pending)
                .with_table(&table)
                .with_zones(&zones)
                .with_tags(&tags)
                .with_film(self.film)
                .with_bodies(&self.bodies);
            // The first round began inside `GAME_ACTIVATE` (its round
            // reset, `ENGINE_ROUND_RESET`, ran before this observer), so
            // its `on round_start` follows `on init` here; each later
            // round's comes from [`Runtime::round_start`].
            vm.fire(&mut g, Input::Init);
            if g.error.is_none() {
                vm.round_start_as(&mut g, game_type);
            }
            (g.error, g.start_line)
        };
        let mut running = Running {
            record_engine: engine,
            record_prefix: if script.mode == Mode::Rules {
                script.prefix
            } else {
                hash.0[..8].try_into().unwrap()
            },
            vm,
            game_type,
            start_line,
            hash,
            source,
            options,
            written: Vec::new(),
            pending,
            authority: None,
            pre: None,
            post: None,
            table,
            zones,
            seen: Seen::default(),
            census,
            teams,
            tags,
            carried,
            wants_damage: damage,
        };
        if let Err(why) = save(&mut running, &mut *self.store) {
            return self.refuse(full, why);
        }
        let line = format!(
            "rules: {name} ({}) from {} on engine {engine}, options {options:#010x}",
            hash,
            source.name()
        );
        let line = match &running.carried {
            Some(c) => format!(
                "{line}, for {} ({}) from {}, which uses it",
                c.name,
                c.hash,
                c.source.name()
            ),
            None => line,
        };
        self.status = Status::Running(Box::new(running));
        if let Some(e) = error {
            self.error(0, "init", describe(&e));
        }
        line
    }

    /// The `uses` script whose game runs the built-in it uses here, if
    /// any.
    pub fn carried(&self) -> Option<&Carried> {
        self.running().and_then(|r| r.carried.as_ref())
    }

    /// Stops the running script with `why`, removing its record.
    fn stop(&mut self, why: String) {
        let hash = self.hash();
        eprintln!("rules: stopped: {why}");
        self.store.remove();
        ::bots::objective::script::clear(None);
        self.status = Status::Refused { hash, why };
        self.show_cue.forget();
    }

    /// Before the game tick: the VM's tick begins and its queued calls join
    /// the pending ones. The work is for the async body; none means the
    /// tick runs as it is. A blocked game's first boundary ends it, on the
    /// authority only: one `end_round`, through the call `end_round` makes.
    pub fn pre_tick(&mut self, m: &dyn GuestMemory) -> Option<Work> {
        let tick = bots::guest::game_tick(m)?.wrapping_add(1);
        if let Status::Blocked { end, .. } = &mut self.status {
            if !std::mem::take(end) {
                return None;
            }
            return Some(Work {
                tick,
                ask_authority: true,
                calls: vec![Call::End],
                sight: Vec::new(),
            });
        }
        let Status::Running(r) = &mut self.status else {
            return None;
        };
        if r.pre == Some(tick) {
            return None;
        }
        r.pre = Some(tick);
        if let Err(why) = sync(r, &*self.store) {
            self.stop(why);
            return None;
        }
        r.vm.begin_tick(tick);
        // The picks the local players made since the last boundary
        // (`super::world::pick`); one for a picker the script no longer
        // offers is refused by the VM and dropped.
        for pick in super::world::pick::take() {
            r.vm.choose(pick.player, pick.slot, pick.row);
        }
        // The loadout picks, the same way: the VM refuses a row the
        // player's window does not open.
        for pick in super::world::choice::take() {
            r.vm.choose_loadout(pick.player, pick.row);
        }
        {
            let Running {
                vm,
                pending,
                table,
                zones,
                tags,
                ..
            } = &mut **r;
            let mut g = GuestGame::new(m, pending)
                .with_table(table)
                .with_zones(zones)
                .with_tags(tags)
                .with_film(self.film)
                .with_bodies(&self.bodies);
            vm.flush_queue(&mut g);
        }
        if let Err(why) = save(r, &mut *self.store) {
            self.stop(why);
            return None;
        }
        // The boundary's messages: shown to this machine's local users and
        // queued for the show channel here, on the authority only; they
        // make no engine call, so they are never in the work. Until the
        // authority answer is in they wait, as the engine calls do; a
        // machine that is not the authority shows none.
        let (messages, rest): (Vec<Call>, Vec<Call>) = std::mem::take(&mut r.pending)
            .into_iter()
            .partition(|c| matches!(c, Call::Message(_) | Call::SoundTo { .. }));
        let mut plays = Vec::new();
        let held = match r.authority {
            Some(true) => {
                let mut outgoing = Vec::new();
                let mut sounds = Vec::new();
                for c in messages {
                    match c {
                        Call::Message(o) => outgoing.push(o),
                        Call::SoundTo { to, path } => sounds.push((to, path)),
                        _ => {}
                    }
                }
                super::world::show::publish(m, r.vm.ir(), tick, outgoing);
                plays = super::world::audio::publish(m, r.vm.ir(), tick, sounds);
                Vec::new()
            }
            None => messages,
            Some(false) => Vec::new(),
        };
        let ask_authority = r.authority.is_none();
        let mut calls = rest;
        calls.extend(plays);
        // The next local player to ask for a pick, on the authority and
        // never in a film, which replays the picks it recorded.
        if r.authority == Some(true)
            && !self.film
            && let Some(ask) = super::world::pick::plan(&r.vm, &super::world::pick::seats(m), tick)
        {
            calls.push(Call::OfferPick(ask));
        }
        if r.authority == Some(true)
            && !self.film
            && let Some(ask) = super::world::choice::plan(&r.vm, &super::world::pick::seats(m), tick)
        {
            calls.push(Call::OfferChoice(ask));
        }
        if let Some(ty) = r.start_line
            && super::start_line::players_ready(m)
        {
            calls.push(Call::StartLine(ty));
            r.start_line = None;
        }
        r.pending = held;
        let sight: Vec<_> = if r.vm.ext().stopped.is_none() {
            r.vm.sight()
                .iter()
                .map(|p| super::world::physics::SightQuery::plan(m, &r.table, p.from, p.to))
                .collect()
        } else {
            Vec::new()
        };
        (ask_authority || !calls.is_empty() || !sight.is_empty()).then_some(Work {
            tick,
            ask_authority,
            calls,
            sight,
        })
    }

    /// Runs one handler group over the guest, then writes the record.
    fn run(&mut self, m: &dyn GuestMemory, f: impl FnOnce(&mut Vm, &mut GuestGame<'_>)) {
        let Status::Running(r) = &mut self.status else {
            return;
        };
        if r.authority == Some(false) {
            return;
        }
        if let Err(why) = sync(r, &*self.store) {
            self.stop(why);
            return;
        }
        let tick = r.vm.ext().tick;
        let error = {
            let Running {
                vm,
                pending,
                table,
                zones,
                tags,
                ..
            } = &mut **r;
            let mut g = GuestGame::new(m, pending)
                .with_table(table)
                .with_zones(zones)
                .with_tags(tags)
                .with_film(self.film)
                .with_bodies(&self.bodies);
            f(vm, &mut g);
            (g.start_line, g.error)
        };
        let (start_line, error) = error;
        if let Some(ty) = start_line {
            r.start_line = Some(ty);
        }
        let saved = save(r, &mut *self.store);
        if let Some(e) = error {
            self.error(tick, "script", describe(&e));
        }
        if let Err(why) = saved {
            self.stop(why);
        }
    }

    /// A player was killed: `on kill`, with its calls kept for the next
    /// tick boundary.
    pub fn kill(&mut self, m: &dyn GuestMemory, victim: u8, killer: Option<u8>) {
        self.run(m, |vm, g| {
            vm.fire(g, Input::Kill { victim, killer });
        });
    }

    /// A later round began (the engine's round reset,
    /// `ENGINE_ROUND_RESET`): the last round's end in the VM, which takes
    /// its round state (the `ticks_held` counts, spawn rules, bodies, shown
    /// areas, HUD, offers; `on round_end` is not delivered, so no handler
    /// runs), then `on round_start`, with its calls kept for the next tick
    /// boundary. The first round's is [`Runtime::start`]'s.
    pub fn round_start(&mut self, m: &dyn GuestMemory) {
        let Some(r) = self.running() else { return };
        let ty = r.game_type;
        let projects_role = r.record_engine == super::roles::JUGGERNAUT;
        // The scores as the new round opens, before the handler's own.
        crate::sim_rounds::push(crate::sim_rounds::read(m, r.vm.ext().tick));
        self.run(m, |vm, g| {
            let before = vm.ext().role_mask;
            vm.fire(g, Input::RoundEnd);
            vm.clear_roles();
            let first_call = g.calls.len();
            vm.round_start_as(g, ty);
            // Retail Juggernaut keeps its native mask across round resets.
            // Release any old holder the new round's handler did not clear
            // explicitly or crown again.
            if projects_role {
                let expired = before & !vm.ext().role_mask;
                for player in 0u8..16 {
                    let clear = Call::RoleTraits { player, on: false };
                    if expired & (1 << player) != 0 && !g.calls[first_call..].contains(&clear) {
                        g.calls.push(clear);
                    }
                }
            }
        });
    }

    /// Player `player` left the game (`ENGINE_PLAYER_LEFT`, after the
    /// engine took it off the roster): `on quit`, with its calls kept for
    /// the next tick boundary.
    pub fn quit(&mut self, m: &dyn GuestMemory, player: u8) {
        self.run(m, |vm, g| {
            vm.fire(g, Input::Quit(player));
        });
    }

    /// A superbounce by player `player` off the strip `spot`
    /// (`superbounce_chain::spot_id`), the `chain`th of its chain, a
    /// Monster with `monster`: `on bounce`, with its calls kept for the
    /// next tick boundary.
    pub fn bounce(&mut self, m: &dyn GuestMemory, player: u8, spot: u32, chain: u32, monster: bool) {
        self.run(m, |vm, g| {
            vm.fire(
                g,
                Input::Bounce {
                    player,
                    spot,
                    chain,
                    monster,
                },
            );
        });
    }

    /// Player `player` fell out of the map during a bounce: `on bounced`,
    /// with its calls kept for the next tick boundary.
    pub fn bounced(&mut self, m: &dyn GuestMemory, player: u8) {
        self.run(m, |vm, g| {
            vm.fire(g, Input::Bounced(player));
        });
    }

    /// The players' bodies at the medal chain's step, replacing the last
    /// set for a script's biped reads.
    pub fn bodies(&mut self, bodies: &[crate::superbounce_chain::Body]) {
        self.bodies.clear();
        self.bodies.extend_from_slice(bodies);
    }

    /// What one tick's medal chain saw of the players' physics
    /// (`superbounce_chain::Medals::physics`), in order: `on touchdown`,
    /// `on launch` and `on grounded` (`super::world::physics::inputs`),
    /// with their calls kept for the next tick boundary.
    pub fn physics(&mut self, m: &dyn GuestMemory, physics: &[(u8, crate::superbounce_chain::Physics)]) {
        let wants = |r: &Running| {
            r.vm.ir()
                .handlers
                .iter()
                .any(|h| matches!(h.event, Event::Touchdown | Event::Launch | Event::Grounded))
        };
        if physics.is_empty() || !self.running().is_some_and(wants) {
            return;
        }
        let inputs = super::world::physics::inputs(physics);
        self.run(m, |vm, g| {
            for input in inputs {
                vm.fire(g, input);
            }
        });
    }

    /// One tick's medal awards (`superbounce_chain::Medals::step`), in
    /// order: each bounce as [`Runtime::bounce`] with its chain's count
    /// and whether it was a Monster, each Bounced as [`Runtime::bounced`].
    /// A medal test's awards never come here (`superbounce_mode` steps
    /// only the chain's own).
    pub fn bounces(
        &mut self,
        m: &dyn GuestMemory,
        awards: &[crate::superbounce_chain::Award],
    ) {
        use hud_kit_core::views::superbounce::Medal;
        for a in awards {
            if let Some(spot) = a.spot {
                self.bounce(m, a.slot, spot, a.chain, a.monster());
            } else if a.medals.contains(&Medal::Bounced) {
                self.bounced(m, a.slot);
            }
        }
    }

    /// After the game tick: what changed on the roster and in the zones
    /// since the VM was last told (`on death`, `on spawn`, `on leave`, `on
    /// enter`; [`zones::events`]), then timers, then `on tick`. A script
    /// with no handler for an event has none of it worked out. `events` is
    /// the tick's sim events: its damage (`on damage`, the first
    /// [`data::MAX_DAMAGE`], in the order the game applied it) is told
    /// before the roster's changes, so before `on tick`.
    pub fn post_tick(&mut self, m: &dyn GuestMemory, events: &[SimEvent]) {
        if (self.hud_on || self.objective_on) && !matches!(self.status, Status::Running(_)) {
            self.publish_hud(None);
        }
        let Some(tick) = bots::guest::game_tick(m) else {
            return;
        };
        let mut seen = match &mut self.status {
            Status::Running(r) if r.post != Some(tick) => {
                r.post = Some(tick);
                std::mem::take(&mut r.seen)
            }
            _ => return,
        };
        let wants_damage = matches!(&self.status, Status::Running(r) if r.wants_damage);
        let damage = if wants_damage { data::damage_inputs(events) } else { Vec::new() };
        self.run(m, |vm, g| {
            for input in damage {
                vm.fire(g, input);
            }
            tell(vm, g, &mut seen);
            vm.update(g);
        });
        if let Status::Running(r) = &mut self.status {
            r.seen = seen;
        }
        self.publish_walls();
        self.publish_hud(Some(m));
        self.send_show();
        self.send_walls(tick);
        self.send_picks(tick);
    }

    /// The async body's outcome: what the authority check answered, and
    /// the calls the engine refused (entry name, error).
    pub fn finish(&mut self, tick: u32, authority: Option<bool>, refused: Vec<Refused>) {
        for r in refused {
            self.error(tick, &r.call, &r.error);
            // A put the world refused did not move the barrier the VM
            // thinks it opened: the VM closes it in its state again
            // (`Vm::put_refused`), saved with the next tick's state.
            if let (Some(place), Status::Running(run)) = (r.put, &mut self.status) {
                run.vm.put_refused(place.object, place.at);
            }
        }
        // A machine that is not the authority ends no blocked game: its host
        // decides.
        if matches!(self.status, Status::Blocked { .. }) && authority == Some(false) {
            self.status = Status::Idle(
                "this machine is not the game engine's authority; the host runs the script",
            );
            return;
        }
        let Status::Running(r) = &mut self.status else {
            return;
        };
        match authority {
            Some(true) => r.authority = Some(true),
            Some(false) => {
                self.store.remove();
                self.status = Status::Idle(
                    "this machine is not the game engine's authority; the host runs the script",
                );
                self.show_cue.forget();
                eprintln!("rules: idle: this machine is not the game engine's authority");
            }
            None => {}
        }
    }

    /// Store the boundary's sight answers before any of the tick's handlers.
    fn finish_sight(&mut self, tick: u32, sight: Vec<(u32, u32, bool)>) {
        let Status::Running(r) = &mut self.status else {
            return;
        };
        if r.pre != Some(tick) || sight.is_empty() {
            return;
        }
        if let Err(why) = sync(r, &*self.store) {
            self.stop(why);
            return;
        }
        for (from, to, clear) in sight {
            r.vm.record_sight(from, to, clear);
        }
        if let Err(why) = save(r, &mut *self.store) {
            self.stop(why);
        }
    }

    /// The game ended: the record is removed and the script stops.
    pub fn ended(&mut self) {
        let name = match &self.status {
            Status::Running(r) => r.vm.ir().header.name.clone(),
            _ => return,
        };
        eprintln!("rules: {name} ended with the game");
        self.store.remove();
        // The script's bot goals end with it (`bots::objective::script`).
        ::bots::objective::script::clear(None);
        self.status = Status::Idle("the game ended");
        self.wall_cue = None;
        self.pick_cue = None;
        self.show_cue.forget();
        self.publish_walls();
        self.publish_hud(None);
    }

    /// Swaps the running script's program between ticks, keeping every
    /// slot value (`super::reload`, `Vm::swap_program`): the record is
    /// synced first and written after. An error keeps the old program.
    pub fn swap(&mut self, ir: Ir) -> Result<Sha256, String> {
        let film = self.film;
        let r = match &mut self.status {
            Status::Running(r) => r,
            _ => return Err("no script runs in this game".into()),
        };
        // A program with no handlers would score nowhere: the engine's
        // routines stay answered in place. A `uses` script's game runs the
        // built-in it uses, which a restart picks.
        if ir.header.uses.is_some() {
            return Err(
                "the new script has a uses line and no handlers to swap in; restart the game with it".into(),
            );
        }
        if let Some(why) = record_refusal(&ir, r.record_engine) {
            return Err(why);
        }
        if let Err(why) = sync(r, &*self.store) {
            self.stop(why.clone());
            return Err(why);
        }
        // The new program's labels must hold on the map the game started
        // with, as at the start.
        if !film {
            let problems = ::rules::labels::check(&ir, &r.census, &r.teams);
            if !problems.is_empty() {
                let texts: Vec<String> = problems.into_iter().map(|p| p.text).collect();
                return Err(format!(
                    "the map does not have what the script needs: {}",
                    texts.join("; ")
                ));
            }
        }
        if !film {
            let problems = tag_problems(&ir, &r.tags);
            if !problems.is_empty() {
                return Err(format!(
                    "the map does not have what the script needs: {}",
                    problems.join("; ")
                ));
            }
        }
        let hash = ir.id();
        let mut table = r.table.clone();
        table.relabel(&ir);
        r.vm.swap_program(ir).map_err(|e| e.to_string())?;
        if r.carried.is_none() {
            r.game_type = r.vm.ir().header.game_type;
            if r.start_line.is_some() {
                r.start_line = Some(r.game_type);
            }
        }
        r.table = table;
        r.hash = hash;
        r.wants_damage = wants_damage(r.vm.ir());
        self.show_cue.forget();
        if let Err(why) = save(r, &mut *self.store) {
            self.stop(why.clone());
            return Err(why);
        }
        Ok(hash)
    }

    /// Why a snapshot can't be taken now: calls wait for the tick boundary
    /// outside guest memory.
    pub fn snapshot_blocker(&self) -> Option<String> {
        let r = self.running()?;
        (!r.pending.is_empty()).then(|| {
            format!(
                "{} rules calls wait for the next tick boundary outside guest memory",
                r.pending.len()
            )
        })
    }

    /// `rules.zones`: the running script's zones; none when no script
    /// runs.
    pub fn zones(&self) -> wire::Zones {
        let Some(r) = self.running() else {
            return wire::Zones::default();
        };
        let mut out = Vec::new();
        for (handle, z) in r.zones.iter() {
            let Some(t) = r.table.get(handle) else { continue };
            let zones::Shape::Area { at, yaw, area } = &z.shape;
            let mut plain = serde_json::Map::new();
            match &area.shape {
                game_markers::Shape::Radius(radius) => plain.insert("radius".into(), (*radius).into()),
                game_markers::Shape::Box { half } => plain.insert("box".into(), serde_json::json!(half)),
                game_markers::Shape::Sphere(radius) => plain.insert("sphere".into(), (*radius).into()),
                game_markers::Shape::Path(points) => plain.insert("path".into(), serde_json::json!(points)),
            };
            plain.insert("below".into(), area.below.into());
            plain.insert("above".into(), area.above.into());
            // A box turns with its flag; no other shape has a facing.
            if matches!(area.shape, game_markers::Shape::Box { .. }) {
                plain.insert("yaw".into(), (*yaw).into());
            }
            out.push(wire::Zone {
                id: t.id(),
                label: t.text.clone(),
                number: t.number,
                shape: z.source.name().into(),
                area: plain.into(),
                at: *at,
                reports: z.reporter == handle,
            });
        }
        out.sort_by_key(|z| z.id);
        wire::Zones { zones: out }
    }

    /// `rules.state`.
    pub fn state(&self) -> wire::State {
        let errors = self.errors.iter().cloned().collect();
        match &self.status {
            Status::Idle(why) => wire::State {
                refused: Some((*why).into()),
                errors,
                ..Default::default()
            },
            Status::Refused { hash, why } => wire::State {
                refused: Some(why.clone()),
                hash: hash.clone(),
                errors,
                ..Default::default()
            },
            Status::Blocked {
                hash,
                problems,
                forge,
                table,
                ..
            } => wire::State {
                refused: Some(if *forge {
                    "the map does not have what the script needs; in a Forge session the script does not run until it does".into()
                } else {
                    "the map does not have what the script needs; the game was ended".into()
                }),
                hash: Some(hash.clone()),
                errors,
                blocked: problems.clone(),
                labelled: table.view(),
                ..Default::default()
            },
            Status::Running(r) => {
                let ir = r.vm.ir();
                let ext = r.vm.ext();
                wire::State {
                    running: true,
                    refused: None,
                    script: Some(ir.header.name.clone()),
                    version: Some(ir.header.version),
                    hash: Some(r.hash.to_string()),
                    source: Some(r.source.name().into()),
                    carrier: u8::try_from(r.record_engine).ok(),
                    scoring: Some(wire::Scoring::Script),
                    options: Some(r.options),
                    authority: r.authority,
                    stopped: ext.stopped.as_ref().map(describe),
                    budget: Some(wire::Budget {
                        tick: ext.tick,
                        ops: ext.ops,
                        limit: TICK_BUDGET,
                        worst_tick: r.vm.report().worst_tick,
                    }),
                    pending: r.pending.len(),
                    errors,
                    last_reload: None,
                    blocked: Vec::new(),
                    // The labelled things, for a script that reads labels.
                    labelled: if ir.labels.is_empty() {
                        Vec::new()
                    } else {
                        r.table.view()
                    },
                }
            }
        }
    }
}

/// Tells the VM what changed since `seen`, in [`zones::events`]' order,
/// marking each change it heard. A zone event the VM carries to the next
/// tick stays unmarked, so it is told again.
fn tell(vm: &mut Vm, g: &mut GuestGame<'_>, seen: &mut Seen) {
    let wants = |e: Event| vm.ir().handlers.iter().any(|h| h.event == e);
    let roster = wants(Event::Spawn) || wants(Event::Death);
    let zoned = wants(Event::Enter) || wants(Event::Leave);
    if !roster && !zoned {
        return;
    }
    let (alive, listed) = zones::alive_mask(g.m);
    seen.keep_listed(listed);
    let now = match (zoned, g.zones, g.table) {
        (true, Some(z), Some(t)) => {
            let bodies = zones::bodies(g.m);
            z.occupancy(&bodies, |h| t.get(h).is_some_and(|thing| thing.label.is_some()))
        }
        _ => Default::default(),
    };
    for change in zones::events(seen, alive, listed, &now) {
        let input = match change {
            Change::Death(p) => Input::Death(p),
            Change::Spawn(p) => {
                super::world::choice::spawned(p);
                Input::Spawn(p)
            }
            Change::Leave { zone, unit } => Input::Leave { zone, object: unit },
            Change::Enter { zone, unit } => Input::Enter { zone, object: unit },
        };
        if vm.fire(g, input) != Fired::Carried {
            seen.told(change);
        }
    }
}

#[cfg(test)]
mod tests;
