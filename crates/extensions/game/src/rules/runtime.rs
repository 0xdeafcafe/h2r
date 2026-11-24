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
//! - [`Runtime::bounces`] after each game tick, from the superbounce
//!   mode's medal chain: `on bounce` and `on bounced`, each bounce and
//!   Bounced on the tick its chain files it.
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
//! does not see it. The carrier's own scoring does not run beside it
//! (`super::scoring`).
//!
//! **State.** The VM's state is the `RULES` record of the `SimExt` blob
//! ([`Store`]), written after every run that changed it. Before each run
//! the record is compared with what was last written; a checkpoint revert
//! or a snapshot load that changed it resumes the VM from it, and a record
//! that is gone (a full restore) stops the script with a refusal.

use std::collections::VecDeque;
use std::sync::OnceLock;

use ::rules::caps::{self, Kind, Status as EntryStatus, id};
use ::rules::ir::{Event, Ir, Op, TagGroup};
use ::rules::vm::{Arg, Call as VmCall, Fired, Game, Input, Placed, RulesExt, ScriptError, TICK_BUDGET, Vm};
use ::rules::policy::{self, Context};
use ::rules::{Sha256, verify};
use ::rules::types::Type;
use agent_proto::rules as wire;
use engine::GuestMemory;
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::points;
use variants::ext::{Ext, Mode};

use super::census::{self, Table};
use super::guest;
use super::world::zones::{self, Change, Seen, Zones};
use crate::firefight::runtime::read_players;
use forge_doc::LabelledThing;

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
    // From the roster and the zones at each tick's end
    // (`super::world::zones`).
    Event::Spawn,
    Event::Death,
    Event::Enter,
    Event::Leave,
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
];
/// The action entries this runtime makes.
pub const ACTIONS: &[u16] = &[
    id::SCORE,
    id::END_ROUND,
    id::SET_LIVES,
    id::RESPAWN,
    id::ROLE_TRAITS,
    id::RESET_OBJECTIVE,
    id::BOT_GO,
    id::BOT_HOLD,
    id::BOT_FOLLOW,
    id::BOT_ATTACK,
    id::BOT_FREE,
    // Shown to this machine's local users and sent as show cues
    // (`super::world::show`).
    id::MESSAGE,
    id::MESSAGE_TEAM,
    id::MESSAGE_ALL,
    // The multiplayer list's weapons (`super::world::loadout`).
    id::SET_LOADOUT,
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
    id::UNMARK,
];

/// Where the `RULES` record lives: the process's `SimExt` blob in the
/// game ([`SimExtStore`]), a local container in tests.
pub trait Store: Send {
    fn get(&self) -> Option<Vec<u8>>;
    fn set(&mut self, bytes: Vec<u8>) -> Result<(), String>;
    fn remove(&mut self);
}

/// The process's `SimExt` blob (`sim_ext::global`), which checkpoints,
/// snapshots, film keyframes and the sim hash carry.
pub struct SimExtStore;

impl Store for SimExtStore {
    fn get(&self) -> Option<Vec<u8>> {
        sim_ext::global().ext.get(&sim_ext::RULES).map(<[u8]>::to_vec)
    }
    fn set(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        sim_ext::global()
            .ext
            .set(&sim_ext::RULES, bytes)
            .map_err(|e| e.to_string())
    }
    fn remove(&mut self) {
        sim_ext::global().ext.remove(&sim_ext::RULES);
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
/// script's label reads. In the game it is [`census::things`]: the loaded
/// scenario's netgame flags and starting locations in guest memory.
pub type CensusSource = dyn Fn(&dyn GuestMemory) -> Result<Vec<LabelledThing>, String> + Send + Sync;

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

/// One built-in script, compiled once: its engine index (an example's
/// carrier), blob and hash.
pub struct Builtin {
    pub engine: u8,
    pub blob: Vec<u8>,
    pub id: Sha256,
}

/// Every built-in script (`rules::builtin`), compiled once. A built-in
/// that does not compile is left out with a line.
pub fn builtins() -> &'static [Builtin] {
    static BUILT: OnceLock<Vec<Builtin>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut out = Vec::new();
        for &(name, engine) in caps::CARRIERS {
            let Some(src) = ::rules::builtin::builtin(engine) else {
                continue;
            };
            match ::rules::compile(src) {
                Ok(ir) => out.push(Builtin {
                    engine,
                    blob: ir.encode(),
                    id: ir.id(),
                }),
                Err(e) => eprintln!("rules: the built-in {name} script does not compile: {e}"),
            }
        }
        out
    })
}

/// Every example script (`rules::builtin::examples`), compiled once, each
/// with its carrier as `engine`. A record names one by its prefix, as any
/// script; they are not ports, so a record with no mode never runs one.
pub fn examples() -> &'static [Builtin] {
    static BUILT: OnceLock<Vec<Builtin>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut out = Vec::new();
        for &(name, src) in ::rules::builtin::examples() {
            match ::rules::compile(src) {
                Ok(ir) => out.push(Builtin {
                    engine: ir.header.engine(),
                    blob: ir.encode(),
                    id: ir.id(),
                }),
                Err(e) => eprintln!("rules: the example {name} script does not compile: {e}"),
            }
        }
        out
    })
}

/// Every listed script (`rules::builtin::LISTED`, the built-in list's
/// community variants), compiled once, each with its carrier as
/// `engine`. A built-in entry's record names one by its prefix
/// (`crate::community_variants::write_blocks`), so a game on it, or on a
/// copy of it, runs it (docs/game/rules-variants.md, "Built-in list
/// entries"). Superbounce and Siege are examples too; [`compiled_in`]
/// finds them there first.
pub fn listed() -> &'static [Builtin] {
    static BUILT: OnceLock<Vec<Builtin>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut out = Vec::new();
        for b in ::rules::builtin::LISTED {
            match ::rules::compile(b.source) {
                Ok(ir) => out.push(Builtin {
                    engine: ir.header.engine(),
                    blob: ir.encode(),
                    id: ir.id(),
                }),
                Err(e) => eprintln!("rules: the listed {} script does not compile: {e}", b.name),
            }
        }
        out
    })
}

/// Every script of the default Live lineup
/// (`rules::builtin::MATCHMAKING`), compiled once, each with its carrier as
/// `engine`. A default hopper entry's record names one by its prefix
/// (`live_proto::hopper::defaults`), so a match on it runs it, as the
/// built-in game each `uses`.
pub fn matchmaking() -> &'static [Builtin] {
    static BUILT: OnceLock<Vec<Builtin>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut out = Vec::new();
        for m in ::rules::builtin::MATCHMAKING {
            match ::rules::compile(m.source) {
                Ok(ir) => out.push(Builtin {
                    engine: ir.header.engine(),
                    blob: ir.encode(),
                    id: ir.id(),
                }),
                Err(e) => eprintln!("rules: the matchmaking {} script does not compile: {e}", m.key),
            }
        }
        out
    })
}

/// The compiled-in script whose hash starts with `prefix`: a port
/// ([`builtins`]), an example ([`examples`]), a listed script
/// ([`listed`]) or a default-lineup script ([`matchmaking`]).
pub fn compiled_in(prefix: [u8; 8]) -> Option<&'static Builtin> {
    builtins()
        .iter()
        .chain(examples())
        .chain(listed())
        .chain(matchmaking())
        .find(|b| b.id.0[..8] == prefix)
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
    /// `bot_go`, `bot_hold`, `bot_follow`, `bot_attack` and `bot_free`: an
    /// outside order to the phantom in `player`'s seat, or `None` to free
    /// it (`super::world::bots`).
    BotOrder {
        player: u8,
        order: Option<bots::brain::Order>,
    },
    /// `end_round`, or the round end of a script error.
    End,
    /// `message`, `message_team` or `message_all`: not an engine call but
    /// a show cue, made at the boundary by [`Runtime::pre_tick`]
    /// (`super::world::show`).
    Message(super::world::show::Outgoing),
    /// `set_loadout`: the player's living unit gets the weapons, by tag
    /// path (`super::world::loadout`).
    SetLoadout {
        player: u8,
        primary: Option<String>,
        secondary: Option<String>,
    },
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
            Call::ResetObjective(_) => "reset_objective",
            Call::BotOrder { order, .. } => match order {
                Some(bots::brain::Order::Go(_)) => "bot_go",
                Some(bots::brain::Order::Hold(..)) => "bot_hold",
                Some(bots::brain::Order::Follow(_)) => "bot_follow",
                Some(bots::brain::Order::Attack(_)) => "bot_attack",
                None => "bot_free",
            },
            Call::End => "end_round",
            Call::Message(_) => "message",
            Call::SetLoadout { .. } => "set_loadout",
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

/// The game, as the VM reads and changes it at one moment. Reads are made
/// at once; actions are kept in `calls` for the next tick boundary.
pub struct GuestGame<'a> {
    pub m: &'a dyn GuestMemory,
    pub calls: &'a mut Vec<Call>,
    start_line: Option<Type>,
    /// The script error the VM stopped on, if it did.
    pub error: Option<ScriptError>,
    /// The map's labelled things; none reads as a map with none.
    pub table: Option<&'a Table>,
    /// This run plays a film back: a bot order is not replayed (the seat's
    /// recorded pad plays), so none is made (docs/game/rules-world-api.md,
    /// "Films and Theater").
    film: bool,
    /// The map's zones, for `inside` and `ticks_held`; none reads as a map
    /// with none (`super::world::zones`).
    pub zones: Option<&'a Zones>,
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
        }
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

    fn roster(&self) -> Vec<(u8, bool, u8)> {
        read_players(self.m)
            .unwrap_or_default()
            .into_iter()
            .map(|(p, _)| (p.slot, p.alive, p.team))
            .collect()
    }
}

fn player(call: &VmCall, n: usize) -> Option<u8> {
    match call.args.get(n) {
        Some(Arg::Player(Some(p))) => Some(*p),
        _ => None,
    }
}

impl Game for GuestGame<'_> {
    fn start_line(&mut self, ty: Type) {
        self.start_line = Some(ty);
    }

    fn players(&self) -> Vec<u8> {
        self.roster().into_iter().map(|(slot, _, _)| slot).collect()
    }

    fn teams(&self) -> Vec<u8> {
        let mut teams: Vec<u8> = self.roster().into_iter().map(|(_, _, t)| t).collect();
        teams.sort_unstable();
        teams.dedup();
        teams
    }

    /// The map's things with this label ([`Table::labelled`]).
    fn labelled(&self, label: &str) -> Vec<Placed> {
        self.table.map_or_else(Vec::new, |t| t.labelled(label))
    }

    fn objectives(&self) -> Vec<u32> {
        super::objectives::objects(self.m).unwrap_or_default()
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
        let on_player = |f: fn(&dyn GuestMemory, u8) -> Result<i32, engine::MemFault>| {
            p.map_or(0, |p| f(m, p).unwrap_or(0))
        };
        match call.entry.id {
            id::PLAYER_SCORE => on_player(guest::player_score),
            id::KILLS => on_player(guest::kills),
            id::DEATHS => on_player(guest::deaths),
            id::LIVES => on_player(guest::lives),
            id::TEAM_SCORE => match call.args.first() {
                Some(Arg::Team(Some(t))) => guest::team_score(m, *t).unwrap_or(0),
                _ => 0,
            },
            id::ALIVE => p.is_some_and(|p| self.roster().iter().any(|&(s, alive, _)| s == p && alive))
                as i32,
            id::TEAM_OF => p
                .and_then(|p| self.roster().into_iter().find(|&(s, _, _)| s == p))
                .map_or(::rules::ty::NONE, |(_, _, t)| i32::from(t)),
            id::SLOT_OF => p.map_or(::rules::ty::NONE, i32::from),
            id::SIDE_OF => match p {
                None => ::rules::ty::NONE,
                Some(p) if self.variant("team_play") == 0 => 16 + i32::from(p),
                Some(p) => self
                    .roster()
                    .into_iter()
                    .find(|&(s, _, _)| s == p)
                    .map_or(::rules::ty::NONE, |(_, _, t)| i32::from(t)),
            },
            id::ROUND_TIME_LEFT => guest::round_time_left(m).unwrap_or(guest::NO_TIME_LIMIT),
            id::IS_OBJECTIVE => match call.args.first() {
                Some(Arg::Object(Some(o))) => guest::is_objective(m, *o).unwrap_or(false) as i32,
                _ => 0,
            },
            id::CARRIER_OF => match call.args.first() {
                Some(Arg::Object(Some(o))) => super::objectives::carrier(m, *o)
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
            id::PLAYER_OF => match call.args.first() {
                Some(Arg::Object(Some(o))) => zones::player_of(m, *o).map_or(::rules::ty::NONE, i32::from),
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
            id::SCORE => p.zip(int(1)).map(|(player, delta)| Call::Score { player, delta }),
            id::SET_LIVES => p.zip(int(1)).map(|(player, lives)| Call::SetLives { player, lives }),
            id::RESPAWN => p.map(Call::Respawn),
            // The VM holds the role on every record. The retail Juggernaut
            // setter also posts the announcement and updates its team mask.
            id::ROLE_TRAITS => match call.args.get(1) {
                Some(Arg::Bool(on)) if guest::engine_index(self.m).ok().flatten() == Some(super::roles::JUGGERNAUT) => {
                    p.map(|player| Call::RoleTraits { player, on: *on })
                }
                _ => None,
            },
            id::RESET_OBJECTIVE => match call.args.first() {
                Some(Arg::Object(Some(o))) => Some(Call::ResetObjective(*o)),
                _ => None,
            },
            // Shown and sent as a show cue at the boundary
            // (`super::world::show::publish`), not made as an engine call.
            id::MESSAGE | id::MESSAGE_TEAM | id::MESSAGE_ALL => {
                super::world::show::outgoing(call).map(Call::Message)
            }
            // The winner is not passed: naming one is not traced.
            id::END_ROUND => Some(Call::End),
            // The VM keeps the rule in its state, which the spawn chooser's
            // hooks read at each spawn (`super::world::spawns`).
            id::SPAWN_AREAS | id::SPAWN_NEAR => None,
            // The VM keeps the body in its state, which the spawn's hooks
            // read (`super::world::body`).
            id::SET_BODY => None,
            // The VM keeps the shown areas in its state; the walls are
            // drawn from it (`super::world::walls`).
            id::SHOW_AREA | id::HIDE_AREA => None,
            // The VM keeps the show state; the HUDs are drawn from it
            // (`super::world::hud`).
            id::HUD_VALUE | id::HUD_FILL | id::HUD_STYLE | id::HUD_TEXT | id::HUD_SHOW | id::MARK | id::UNMARK => None,
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
                    super::world::bots::plan(self.m, self.table, call)
                        .map(|(player, order)| Call::BotOrder { player, order })
                }
            }
            _ => None,
        };
        if let Some(c) = made {
            self.calls.push(c);
        }
    }

    fn script_error(&mut self, error: &ScriptError) {
        self.error = Some(*error);
        self.calls.push(Call::End);
    }
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
    /// The `uses` script the record names, when `vm` runs the built-in it
    /// uses.
    pub carried: Option<Carried>,
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
    /// The walls cue last sent on the actor channel, and the tick it was
    /// sent: the list is sent when it changes and again every
    /// `::rules::show::WALLS_REPEAT` ticks while it is not empty, so a
    /// machine that joins late has it (`super::world::show::walls`).
    wall_cue: Option<(Vec<(u32, Option<u8>)>, u32)>,
}

/// Whether the authority sends the walls cue now: the list changed since
/// the last one (an emptied list included), or it is not empty and the
/// last one is `WALLS_REPEAT` ticks old. Nothing shown and nothing sent is
/// nothing to say.
fn walls_due(last: Option<&(Vec<(u32, Option<u8>)>, u32)>, shown: &[(u32, Option<u8>)], tick: u32) -> bool {
    match last {
        None => !shown.is_empty(),
        Some((sent, at)) => {
            sent != shown || (!shown.is_empty() && tick.wrapping_sub(*at) >= ::rules::show::WALLS_REPEAT)
        }
    }
}

fn describe(e: &ScriptError) -> String {
    format!(
        "script error {:?} at tick {}, handler {}, op {}",
        e.kind, e.tick, e.handler, e.pc
    )
}

/// The record's bytes changed since `r` wrote them (a revert or a load):
/// resume from them. A record that is gone stops the script.
fn sync(r: &mut Running, store: &dyn Store) -> Result<(), String> {
    let Some(now) = store.get() else {
        return Err("the RULES record was cleared (a full restore); the script stops".into());
    };
    if now == r.written {
        return Ok(());
    }
    let ext = RulesExt::decode(&now).map_err(|e| format!("the RULES record: {e}"))?;
    r.vm = Vm::resume(r.vm.ir().clone(), r.options, ext)
        .map_err(|e| format!("resuming from the RULES record: {e}"))?;
    r.written = now;
    Ok(())
}

/// Writes the VM's state when it changed.
fn save(r: &mut Running, store: &mut dyn Store) -> Result<(), String> {
    let bytes = r.vm.ext().encode();
    if bytes != r.written {
        store.set(bytes.clone())?;
        r.written = bytes;
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
            census: Box::new(census::things),
            film: false,
            forge: false,
            walled: false,
            hud_on: false,
            wall_cue: None,
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
        match (self.census)(m) {
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
        let (things, problems) = self.gate(m, &ir, &teams);
        let table = Table::new(&things, &ir);
        if !problems.is_empty() {
            return self.block(hash, problems, table);
        }
        if let Some(why) = runtime_refusal(&ir) {
            return self.refuse(full, for_carried(carried.as_ref(), &ir, why));
        }
        let census = census::census(&things);
        let zones = Zones::read(m, &table);
        // The bots read a hill from the same areas (`bots::objective`).
        ::bots::objective::set_flag_areas(table.flag_areas());
        // The record's option bits are its script's; a carried script has
        // no options, so the built-in plays its own defaults.
        let options = if script.mode == Mode::Rules && carried.is_none() {
            script.options
        } else {
            ir.default_options()
        };
        let name = ir.header.name.clone();
        let mut vm = match Vm::new(ir, options, SEED) {
            Ok(vm) => vm,
            Err(e) => return self.refuse(full, format!("the VM refused the blob: {e}")),
        };
        let mut pending = Vec::new();
        let (error, start_line) = {
            let mut g = GuestGame::new(m, &mut pending)
                .with_table(&table)
                .with_zones(&zones)
                .with_film(self.film);
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
            carried,
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
        self.status = Status::Refused { hash, why };
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
        {
            let Running {
                vm,
                pending,
                table,
                zones,
                ..
            } = &mut **r;
            let mut g = GuestGame::new(m, pending)
                .with_table(table)
                .with_zones(zones)
                .with_film(self.film);
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
            .partition(|c| matches!(c, Call::Message(_)));
        let held = match r.authority {
            Some(true) => {
                let messages = messages
                    .into_iter()
                    .filter_map(|c| match c {
                        Call::Message(outgoing) => Some(outgoing),
                        _ => None,
                    })
                    .collect();
                super::world::show::publish(m, r.vm.ir(), tick, messages);
                Vec::new()
            }
            None => messages,
            Some(false) => Vec::new(),
        };
        let ask_authority = r.authority.is_none();
        let mut calls = rest;
        if let Some(ty) = r.start_line
            && super::start_line::players_ready(m)
        {
            calls.push(Call::StartLine(ty));
            r.start_line = None;
        }
        r.pending = held;
        (ask_authority || !calls.is_empty()).then_some(Work {
            tick,
            ask_authority,
            calls,
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
                ..
            } = &mut **r;
            let mut g = GuestGame::new(m, pending)
                .with_table(table)
                .with_zones(zones)
                .with_film(self.film);
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
    /// `ENGINE_ROUND_RESET`): `on round_start`, with its calls kept for
    /// the next tick boundary. The first round's is [`Runtime::start`]'s.
    pub fn round_start(&mut self, m: &dyn GuestMemory) {
        let Some(r) = self.running() else { return };
        let ty = r.game_type;
        let projects_role = r.record_engine == super::roles::JUGGERNAUT;
        self.run(m, |vm, g| {
            let before = vm.ext().role_mask;
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
    /// with no handler for an event has none of it worked out.
    pub fn post_tick(&mut self, m: &dyn GuestMemory) {
        if self.hud_on && !matches!(self.status, Status::Running(_)) {
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
        self.run(m, |vm, g| {
            tell(vm, g, &mut seen);
            vm.update(g);
        });
        if let Status::Running(r) = &mut self.status {
            r.seen = seen;
        }
        self.publish_walls();
        self.publish_hud(Some(m));
        self.send_walls(tick);
    }

    /// Feeds the objective HUDs from the running script's show state, for
    /// each local user (`super::world::hud`), and hides them once no
    /// script feeds them. `None` for a game with no guest to read.
    fn publish_hud(&mut self, m: Option<&dyn GuestMemory>) {
        let on = match (&self.status, m) {
            (Status::Running(r), Some(m)) => super::world::hud::publish(m, r.vm.ir(), r.vm.show(), &r.zones),
            _ => false,
        };
        if self.hud_on && !on {
            super::world::hud::clear();
        }
        self.hud_on = on;
    }

    /// Sends the areas the script shows to the clients as a walls cue
    /// (`::rules::show::Cue::Walls`), from the authority that runs the
    /// script. A film's run sends none: it has no channel.
    fn send_walls(&mut self, tick: u32) {
        let shown = match &self.status {
            Status::Running(r) if r.authority == Some(true) && !self.film => r.vm.shown_areas().to_vec(),
            _ => return,
        };
        if walls_due(self.wall_cue.as_ref(), &shown, tick) {
            super::world::show::walls(tick, shown.clone());
            self.wall_cue = Some((shown, tick));
        }
    }

    /// The walls the running script shows in play (`show_area`), in the
    /// order it showed them; none when no script runs
    /// (`super::world::walls`).
    pub fn walls(&self) -> Vec<crate::forge_glue::markers_world::Wall> {
        match self.running() {
            Some(r) => super::world::walls::of(r.vm.shown_areas(), &r.zones),
            None => Vec::new(),
        }
    }

    /// Hands the walls to the window's world-line source: after each
    /// tick, and once more with none when the script's end took them off.
    fn publish_walls(&mut self) {
        let walls = self.walls();
        if !walls.is_empty() || self.walled {
            self.walled = !walls.is_empty();
            crate::forge_glue::markers_world::publish_walls(walls);
        }
    }

    /// The async body's outcome: what the authority check answered, and
    /// the calls the engine refused (entry name, error).
    pub fn finish(&mut self, tick: u32, authority: Option<bool>, refused: Vec<(String, String)>) {
        for (call, error) in refused {
            self.error(tick, &call, error);
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
                eprintln!("rules: idle: this machine is not the game engine's authority");
            }
            None => {}
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
        self.status = Status::Idle("the game ended");
        self.wall_cue = None;
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
            let zones::Shape::Area { at, area } = &z.shape;
            let mut plain = serde_json::Map::new();
            match &area.shape {
                game_markers::Shape::Radius(radius) => plain.insert("radius".into(), (*radius).into()),
                game_markers::Shape::Path(points) => plain.insert("path".into(), serde_json::json!(points)),
            };
            plain.insert("below".into(), area.below.into());
            plain.insert("above".into(), area.above.into());
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
            Change::Spawn(p) => Input::Spawn(p),
            Change::Leave { zone, unit } => Input::Leave { zone, object: unit },
            Change::Enter { zone, unit } => Input::Enter { zone, object: unit },
        };
        if vm.fire(g, input) != Fired::Carried {
            seen.told(change);
        }
    }
}

/// Before the tick: the authority check when asked, then each call in
/// order. Returns the answer and the calls the engine refused.
pub async fn make_calls(cx: &Cx, work: &Work) -> (Option<bool>, Vec<(String, String)>) {
    let mut refused = Vec::new();
    let mut authority = None;
    if work.ask_authority {
        match guest::authoritative(cx).await {
            Ok(a) => authority = Some(a),
            Err(e) => {
                refused.push(("authority".to_string(), e.to_string()));
                authority = Some(false);
            }
        }
        if authority == Some(false) {
            return (authority, refused);
        }
    }
    for call in &work.calls {
        let result = match *call {
            Call::StartLine(ty) => super::start_line::post(cx, work.tick, ty).await,
            Call::Score { player, delta } => guest::score(cx, player, delta).await,
            Call::SetLives { player, lives } => guest::set_lives(cx, player, lives).await,
            Call::Respawn(player) => crate::firefight::guest::respawn(cx, player).await,
            Call::RoleTraits { player, on } => super::roles::set(cx, player, on).await,
            Call::ResetObjective(object) => super::objectives::reset(cx, object).await,
            Call::BotOrder { player, order } => {
                if super::world::bots::deliver(player, order) {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("player {player} is no bot"))
                }
            }
            Call::End => crate::firefight::guest::end_game(cx).await,
            // Messages are made at the boundary by `Runtime::pre_tick`
            // (`super::world::show::publish`); one reaching this far makes
            // no engine call.
            Call::Message(_) => Ok(()),
            Call::SetLoadout {
                player,
                ref primary,
                ref secondary,
            } => super::world::loadout::give(cx, player, primary.as_deref(), secondary.as_deref()).await,
        };
        if let Err(e) = result {
            refused.push((call.name().to_string(), e.to_string()));
        }
    }
    (authority, refused)
}

/// The async body before a tick with work: the calls, then the tick itself
/// (its chain runs again; `reentry` lets the hook pass).
pub async fn run_work(
    cx: Cx,
    shared: std::sync::Arc<super::feature::Shared>,
    work: Work,
) -> Result<Ret, EntryError> {
    use std::sync::atomic::Ordering;
    let (authority, refused) = make_calls(&cx, &work).await;
    shared.lock().finish(work.tick, authority, refused);
    shared.reentry.store(true, Ordering::SeqCst);
    let r = cx.call(GuestCall::new(points::GAME_TICK.base)).await;
    shared.reentry.store(false, Ordering::SeqCst);
    Ok(Ret::new(r?.eax, 0))
}

#[cfg(test)]
mod tests;
