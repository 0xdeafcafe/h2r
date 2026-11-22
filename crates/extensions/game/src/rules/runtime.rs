//! The rules VM in the running game: the [`Game`] over guest memory, one
//! game's script and state, and what each moment of the game does with it
//! (docs/game/rules-engine.md, "The host runtime").
//!
//! The feature (`super::feature`) and the superbounce mode call in at six
//! moments:
//!
//! - [`Runtime::start`] after a successful `GAME_ACTIVATE`: read the
//!   record's script reference, find the blob, verify it, refuse what this
//!   host cannot run by name, start the VM, run `on init` and set the
//!   `RULES` record. A refusal is kept with its reason and logged; the game
//!   then plays as the carrier engine's lifted rules.
//! - [`Runtime::pre_tick`] before each game tick: the VM's tick begins and
//!   its queued calls join the calls the handlers made since the last
//!   boundary. The feature makes them, in order, in an async body
//!   ([`run_work`]) and then runs the tick. At the first boundary the body
//!   also asks `GAME_ENGINE_AUTHORITATIVE`; a machine that is not the
//!   authority drops the script and its record.
//! - [`Runtime::kill`] at `PLAYER_KILLED`'s entry: `on kill`.
//! - [`Runtime::bounces`] after each game tick, from the superbounce
//!   mode's medal chain: `on bounce`, each bounce on the tick its chain
//!   files it.
//! - [`Runtime::post_tick`] after each game tick: timers, then `on tick`.
//! - [`Runtime::ended`] after `GAME_END`: the record is removed.
//!
//! **Calls wait for the tick boundary.** `PLAYER_KILLED` is opaque, so a
//! hook there can only observe; the carrier's kill callback (Slayer's
//! vtable slot `+0x78`) has no named point; and the per-tick group runs
//! after `GAME_TICK`, not in place of the engine update `0x15c150`. So
//! every call a handler makes is kept and made by the `before` on the next
//! `GAME_TICK`, through the mechanism Firefight's tick hook uses, ahead of
//! the VM's queued calls. A score therefore lands at the next tick
//! boundary rather than inside the kill, and a read in the same handler
//! does not see it.
//!
//! **State.** The VM's state is the `RULES` record of the `SimExt` blob
//! ([`Store`]), written after every run that changed it. Before each run
//! the record is compared with what was last written; a checkpoint revert
//! or a snapshot load that changed it resumes the VM from it, and a record
//! that is gone (a full restore) stops the script with a refusal.

use std::collections::VecDeque;
use std::sync::OnceLock;

use ::rules::caps::{self, Kind, Status as EntryStatus, id};
use ::rules::ir::{Event, Ir, Op};
use ::rules::vm::{Arg, Call as VmCall, Game, Input, Placed, RulesExt, ScriptError, TICK_BUDGET, Vm};
use ::rules::policy::{self, Context};
use ::rules::{Sha256, verify};
use agent_proto::rules as wire;
use engine::GuestMemory;
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::points;
use variants::ext::{Ext, Mode};

use super::guest;
use crate::firefight::runtime::read_players;

/// The match seed until the session data carries one (as Firefight's).
pub const SEED: u64 = 0;
/// Engine indices whose built-in script has passed parity and so runs for
/// a record with no script mode (rules-engine.md, "A record with no
/// mode"). Empty: Slayer stays lifted until its port passes.
pub const PORTED: &[u8] = &[];
/// The engine refusals `rules.state` keeps.
pub const ERRORS_KEPT: usize = 16;
/// The options mode of a multiplayer game (`[GAME_GLOBALS] + 8`).
pub const MODE_MULTIPLAYER: u32 = 2;

/// The events this runtime delivers. A script answering any other is
/// refused at load by the event's name.
pub const DELIVERED: &[Event] = &[
    Event::Init,
    Event::Kill,
    Event::Timer,
    Event::Tick,
    Event::Bounce,
];
/// The read entries this runtime answers.
pub const READS: &[u16] = &[
    id::PLAYER_SCORE,
    id::TEAM_SCORE,
    id::KILLS,
    id::DEATHS,
    id::ALIVE,
    id::TEAM_OF,
    id::LIVES,
    id::ROUND_TIME_LEFT,
    id::IS_OBJECTIVE,
];
/// The action entries this runtime makes.
pub const ACTIONS: &[u16] = &[id::SCORE, id::END_ROUND, id::SET_LIVES, id::RESPAWN];

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

/// One built-in script, compiled once: its engine index, blob and hash.
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Score { player: u8, delta: i32 },
    SetLives { player: u8, lives: i32 },
    Respawn(u8),
    /// `end_round`, or the round end of a script error.
    End,
}

impl Call {
    /// The API entry's name.
    pub fn name(&self) -> &'static str {
        match self {
            Call::Score { .. } => "score",
            Call::SetLives { .. } => "set_lives",
            Call::Respawn(_) => "respawn",
            Call::End => "end_round",
        }
    }
}

/// What the async body before a tick does.
#[derive(Clone, Debug, PartialEq, Eq)]
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
/// named first (`verify::unhostable`), then what this runtime does not do
/// yet: an event it does not deliver, an entry it does not answer or make,
/// a variant field whose offset is not traced, a grant, a Forge label.
pub fn host_refusal(ir: &Ir) -> Option<String> {
    if let Some(e) = verify::unhostable(ir) {
        let status = match e.status {
            EntryStatus::Traced => "traced but not a named point",
            EntryStatus::Untraced => "untraced",
            EntryStatus::NotBuilt => "not built",
            EntryStatus::Named | EntryStatus::VmOnly => "not hostable",
        };
        return Some(format!(
            "the script calls {}, whose engine entry is {status}",
            caps::spelled(e)
        ));
    }
    if !ir.labels.is_empty() {
        return Some("the script names Forge labels, which are not built".into());
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
    /// The script error the VM stopped on, if it did.
    pub error: Option<ScriptError>,
}

impl<'a> GuestGame<'a> {
    pub fn new(m: &'a dyn GuestMemory, calls: &'a mut Vec<Call>) -> Self {
        Self {
            m,
            calls,
            error: None,
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
    fn players(&self) -> Vec<u8> {
        self.roster().into_iter().map(|(slot, _, _)| slot).collect()
    }

    fn teams(&self) -> Vec<u8> {
        let mut teams: Vec<u8> = self.roster().into_iter().map(|(_, _, t)| t).collect();
        teams.sort_unstable();
        teams.dedup();
        teams
    }

    /// Forge labels are not built; a script naming one is refused at load.
    fn labelled(&self, _label: &str) -> Vec<Placed> {
        Vec::new()
    }

    fn variant(&self, field: &str) -> i32 {
        guest::variant(self.m, field).unwrap_or(0)
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
            id::ROUND_TIME_LEFT => guest::round_time_left(m).unwrap_or(guest::NO_TIME_LIMIT),
            id::IS_OBJECTIVE => match call.args.first() {
                Some(Arg::Object(Some(o))) => guest::is_objective(m, *o).unwrap_or(false) as i32,
                _ => 0,
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
            // The winner is not passed: naming one is not traced.
            id::END_ROUND => Some(Call::End),
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
    pub vm: Vm,
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
}

impl Running {
    /// Calls waiting for the next tick boundary.
    pub fn pending(&self) -> &[Call] {
        &self.pending
    }
}

/// Whether a script runs in this game, and why not.
pub enum Status {
    Idle(&'static str),
    /// `hash`: the full hash, or the record's prefix when the blob was not
    /// found.
    Refused { hash: Option<String>, why: String },
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
        }
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

    /// The running blob's full hash, for the Live session's `rules` field.
    pub fn hash(&self) -> Option<String> {
        self.running().map(|r| r.hash.to_string())
    }

    fn refuse(&mut self, hash: Option<String>, why: String) -> String {
        let line = format!("rules: refused, the game plays the carrier engine's own rules: {why}");
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
                let builtin = builtins().iter().find(|b| b.id.0[..8] == script.prefix);
                match builtin {
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
        if i32::from(ir.header.carrier) != engine {
            return self.refuse(
                full,
                format!(
                    "the script rides engine {}, and the record is engine {engine}",
                    ir.header.carrier
                ),
            );
        }
        // The policy first: a refusal by kind of game names the capability.
        if let Some(d) = policy::check(self.context, &ir.header.requires, ir.header.api_version)
            .into_iter()
            .next()
        {
            return self.refuse(full, d.to_string());
        }
        if let Some(why) = host_refusal(&ir) {
            return self.refuse(full, why);
        }
        let options = if script.mode == Mode::Rules {
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
        let error = {
            let mut g = GuestGame::new(m, &mut pending);
            vm.fire(&mut g, Input::Init);
            g.error
        };
        let mut running = Running {
            vm,
            hash,
            source,
            options,
            written: Vec::new(),
            pending,
            authority: None,
            pre: None,
            post: None,
        };
        if let Err(why) = save(&mut running, &mut *self.store) {
            return self.refuse(full, why);
        }
        self.status = Status::Running(Box::new(running));
        if let Some(e) = error {
            self.error(0, "init", describe(&e));
        }
        format!(
            "rules: {name} ({}) from {} on engine {engine}, options {options:#010x}",
            hash,
            source.name()
        )
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
    /// tick runs as it is.
    pub fn pre_tick(&mut self, m: &dyn GuestMemory) -> Option<Work> {
        let tick = bots::guest::game_tick(m)?.wrapping_add(1);
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
            let Running { vm, pending, .. } = &mut **r;
            let mut g = GuestGame::new(m, pending);
            vm.flush_queue(&mut g);
        }
        if let Err(why) = save(r, &mut *self.store) {
            self.stop(why);
            return None;
        }
        let ask_authority = r.authority.is_none();
        let calls = std::mem::take(&mut r.pending);
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
            let Running { vm, pending, .. } = &mut **r;
            let mut g = GuestGame::new(m, pending);
            f(vm, &mut g);
            g.error
        };
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

    /// A superbounce by player `player` off the strip `spot`
    /// (`superbounce_chain::spot_id`): `on bounce`, with its calls kept
    /// for the next tick boundary.
    pub fn bounce(&mut self, m: &dyn GuestMemory, player: u8, spot: u32) {
        self.run(m, |vm, g| {
            vm.fire(g, Input::Bounce(player, spot));
        });
    }

    /// One tick's medal awards (`superbounce_chain::Medals::step`): each
    /// bounce as [`Runtime::bounce`], in order. A Bounced and a medal test
    /// name no strip and are not bounces.
    pub fn bounces(
        &mut self,
        m: &dyn GuestMemory,
        awards: &[crate::superbounce_chain::Award],
    ) {
        for a in awards {
            if let Some(spot) = a.spot {
                self.bounce(m, a.slot, spot);
            }
        }
    }

    /// After the game tick: timers, then `on tick`.
    pub fn post_tick(&mut self, m: &dyn GuestMemory) {
        let Some(tick) = bots::guest::game_tick(m) else {
            return;
        };
        match &mut self.status {
            Status::Running(r) if r.post != Some(tick) => r.post = Some(tick),
            _ => return,
        }
        self.run(m, |vm, g| {
            vm.update(g);
        });
    }

    /// The async body's outcome: what the authority check answered, and
    /// the calls the engine refused (entry name, error).
    pub fn finish(&mut self, tick: u32, authority: Option<bool>, refused: Vec<(String, String)>) {
        for (call, error) in refused {
            self.error(tick, &call, error);
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
        if let Status::Running(r) = &self.status {
            eprintln!("rules: {} ended with the game", r.vm.ir().header.name);
            self.store.remove();
            self.status = Status::Idle("the game ended");
        }
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
                    carrier: Some(ir.header.carrier),
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
                }
            }
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
            Call::Score { player, delta } => guest::score(cx, player, delta).await,
            Call::SetLives { player, lives } => guest::set_lives(cx, player, lives).await,
            Call::Respawn(player) => crate::firefight::guest::respawn(cx, player).await,
            Call::End => crate::firefight::guest::end_game(cx).await,
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
