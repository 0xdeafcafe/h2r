//! `sim.*` (docs/runtime/agent-api.md, "sim"): simulated players and
//! direct game state, so an agent can test co-op, multiplayer and Live
//! flows from one running game.
//!
//! - `sim.player.add` / `remove` / `list` / `pad`: players made by the bots
//!   ([`bots::Bots`], implemented over `bots::phantom` in boot), driven by
//!   a [`bots::PadSource`].
//! - `sim.state.set`: host writes to a player ([`state`]), at a tick
//!   boundary, refused where they would desync a co-op session. A position
//!   or facing goes through the game's own teleport ([`calls`]), answered
//!   after the next tick.
//! - `sim.give`, `sim.spawn`: a weapon for a player, an object at a point,
//!   made by the game's own functions ([`calls`]) at the next tick.
//! - `sim.view`, `sim.expect`: the game as one JSON document, and
//!   `h2 agent run`'s expectations judged against it at one tick.
//! - `sim.session`: the session kind, from the host's [`SessionProbe`].
//! - `sim.objects`: every live object through `sim-state`'s typed views
//!   ([`objects`]).
//! - `sim.events`: the last tick's simulation events from `sim-state`
//!   ([`events`]).
//! - `sim.ledger`: the guest phases of the game tick the Rust driver still
//!   calls ([`ledger`]); no guest reads.
//! - `sim.render_writes`: the render-writes census ([`render_writes`]); no
//!   guest reads.
//! - `player.mark` / `player.recall`: a player's position and facing saved
//!   under a slot, and put back through `sim.state.set`'s teleport.
//!
//! They are game methods ([`crate::Agent::add_game_method`]): answered on
//! the game's thread at a tick boundary while the game ticks, with guest
//! memory at hand. `host.pause` and `host.step` ([`pause`]) are installed
//! with them but answered on the connection's thread, as is `host.quit`
//! ([`quit`]).

pub mod bots;
pub mod calls;
mod edits;
pub use edits::{MapRebind, MapReload, MapReloader, PreparedMapReload, ReservedMapReload};
/// The active game's options block, as `map.reload {}` replays it
/// (`rules.reload`'s restart patches its variant's script reference).
pub use edits::active_options;
pub mod events;
pub mod hashes;
pub mod ledger;
pub mod objects;
pub mod pause;
pub mod recording;
pub mod quit;
pub mod render_writes;
pub mod shot;
pub mod state;
pub mod window;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::expect;
use agent_proto::sim::{
    self as wire, AddParams, DriveParams, ExpectParams, GiveParams, Mark, MarkParams, PlayerKind,
    RemoveParams, SessionKind, SessionView, SetParams, SimPlayer, SourceKind, SpawnParams,
    ViewParams, method,
};
use agent_proto::{Boundary, Kind, ObjectId, RpcError, code};
use guest_layout::GuestMemory;
use input::backend::remote::Remote;
use serde_json::{Map, Value, json};

use crate::exec::GameCtx;
use crate::{Access, Agent};
use bots::{Bot, Bots, PadSource};
use calls::{Call, Tags};

/// Reads the session kind from guest memory: the host's (boot reads the
/// simulation mode and the squad session as `coop_probe` does).
pub type SessionProbe = Box<dyn FnMut(&dyn GuestMemory) -> SessionView + Send>;
type SharedProbe = Arc<Mutex<Option<SessionProbe>>>;

fn read_session(probe: &SharedProbe, guest: &dyn GuestMemory) -> SessionView {
    probe
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .map(|probe| probe(guest))
        .unwrap_or_default()
        .settle()
}

/// One simulated player.
struct Entry {
    bot: Bot,
    source: SourceKind,
    team: Option<i32>,
    identity: wire::Identity,
    /// The script queue, for a `script` source.
    script: Option<Remote>,
}

/// The simulated players and the backends.
pub struct Sim {
    bots: Box<dyn Bots>,
    probe: SharedProbe,
    players: BTreeMap<u32, Entry>,
    next: u32,
    /// The game's own functions, once the host installs them.
    calls: Option<calls::Shared>,
    map_rebind: Option<MapRebind>,
    adopt_editor: Option<Box<dyn Fn() -> Result<(), String> + Send>>,
    /// Tag names for `sim.give` and `sim.spawn`.
    tags: Option<Box<dyn Tags>>,
    /// `player.mark`'s saved places, by slot.
    marks: BTreeMap<String, Mark>,
}

/// A handle on the installed `sim.*` state.
#[derive(Clone)]
pub struct SimHandle(Arc<Mutex<Sim>>);

impl SimHandle {
    fn lock(&self) -> MutexGuard<'_, Sim> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Swaps the bots backend (in tests).
    pub fn set_bots(&self, bots: Box<dyn Bots>) {
        self.lock().bots = bots;
    }

    pub(crate) fn writable_session(&self, ctx: &mut GameCtx<'_>) -> Result<SessionView, RpcError> {
        self.lock().host_write(ctx)
    }

    pub(crate) fn session_view(&self, ctx: &mut GameCtx<'_>) -> SessionView {
        self.lock().session(ctx)
    }

    pub(crate) fn replication_session(
        &self,
        ctx: &mut GameCtx<'_>,
    ) -> Result<SessionView, RpcError> {
        let session = self.writable_session(ctx)?;
        if session.kind != SessionKind::MpHost {
            let mut error = RpcError::new(
                code::INVALID_REQUEST,
                "replication requires an mp_host session",
            );
            error.data = Some(json!({ "session": session }));
            return Err(error);
        }
        Ok(session)
    }

    pub(crate) fn replication_spawn(
        &self,
        params: SpawnParams,
        session: SessionView,
        group: String,
        ctx: &mut GameCtx<'_>,
    ) -> R {
        self.lock().spawn_object(params, Some(session), Some(&group), ctx)
    }

    pub(crate) fn probe_queue(&self) -> Result<calls::Shared, RpcError> {
        self.lock().calls()
    }

    pub(crate) fn string_id(&self, name: &str) -> Result<u32, RpcError> {
        self.lock()
            .tags
            .as_mut()
            .and_then(|tags| tags.string_id(name))
            .ok_or_else(|| invalid(format!("loaded map has no unique string ID for {name:?}")))
    }
    pub(crate) fn string(&self, id: u32) -> Result<String, RpcError> {
        self.lock()
            .tags
            .as_mut()
            .and_then(|tags| tags.string(id))
            .ok_or_else(|| invalid(format!("loaded map has no name for string ID {id:#x}")))
    }

    pub(crate) fn probe_tag(&self, name: &str, group: &str) -> Result<calls::TagRef, RpcError> {
        self.lock().find_tag(name, &[group])
    }

    /// Sets the session probe.
    pub fn set_probe(&self, probe: SessionProbe) {
        *self.lock().probe.lock().unwrap_or_else(|e| e.into_inner()) = Some(probe);
    }

    /// The queue [`calls::CallsFeature`] empties at each tick: teleports,
    /// `sim.give` and `sim.spawn` go through it.
    pub fn set_calls(&self, q: calls::Shared) {
        self.lock().calls = Some(q);
    }

    /// That queue, once set: other host methods that call the game at the
    /// next tick use it too (`maps.test.load`).
    pub fn calls(&self) -> Option<calls::Shared> {
        self.lock().calls.clone()
    }

    /// Where `sim.give` and `sim.spawn` find tags by name or datum.
    pub fn set_tags(&self, tags: Box<dyn Tags>) {
        self.lock().tags = Some(tags);
    }
}

/// Registers the `sim.*` methods on `agent`.
pub fn install(agent: &Agent, bots: Box<dyn Bots>, probe: Option<SessionProbe>) -> SimHandle {
    let h = SimHandle(Arc::new(Mutex::new(Sim {
        bots,
        probe: Arc::new(Mutex::new(probe)),
        players: BTreeMap::new(),
        next: 1,
        calls: None,
        map_rebind: None,
        adopt_editor: None,
        tags: None,
        marks: BTreeMap::new(),
    })));
    let add = |name: &str, access: Access, f: fn(&mut Sim, Value, &mut GameCtx<'_>) -> R| {
        let h = h.clone();
        agent.add_game_method(name, access, move |v, ctx| f(&mut h.lock(), v, ctx));
    };
    add(method::PLAYER_ADD, Access::Act, Sim::player_add);
    add(method::PLAYER_REMOVE, Access::Act, Sim::player_remove);
    add(method::PLAYER_LIST, Access::Read, Sim::player_list);
    add(method::PLAYER_PAD, Access::Act, Sim::player_pad);
    add(method::STATE_SET, Access::Act, Sim::state_set);
    add(method::VIEW, Access::Read, Sim::view_method);
    add(method::EXPECT, Access::Read, Sim::expect);
    add(method::SESSION, Access::Read, Sim::session_method);
    add(method::GIVE, Access::Act, Sim::give);
    add(method::SPAWN, Access::Act, Sim::spawn);
    add(method::AUDIO_PLAY, Access::Act, Sim::audio_play);
    add(method::OBJECT_ANIMATE, Access::Act, Sim::object_animate);
    add(method::RECORDING_PLAY, Access::Act, Sim::recording_play);
    add(method::OBJECTS, Access::Read, Sim::objects_method);
    add(method::EVENTS, Access::Read, Sim::events_method);
    add(method::PLAYER_MARK, Access::Act, Sim::player_mark);
    add(method::PLAYER_RECALL, Access::Act, Sim::player_recall);
    ledger::install(agent);
    render_writes::install(agent);
    pause::install(agent, agent_proto::host_pause::hub());
    quit::install(agent, agent_proto::host_quit::hub());
    shot::install(agent, agent_proto::host_shot::hub());
    window::install(agent, agent_proto::host_window::hub());
    h
}

type R = Result<Value, RpcError>;

fn params<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn unavailable(msg: String) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

/// Ticks a teleport's answer may wait for its tick (the game makes queued
/// calls at the start of the next one).
const TELEPORT_TICKS: u64 = 30;
/// The same for `sim.give` and `sim.spawn`.
const CALL_TICKS: u64 = 30;

/// The `<n>` of `sim.<n>`.
fn sim_number(id: &str) -> Option<u32> {
    id.strip_prefix("sim.")?.parse().ok()
}

impl Sim {
    fn session(&mut self, ctx: &mut GameCtx<'_>) -> SessionView {
        let mut v = None;
        ctx.reader.with_guest(&mut |m| v = Some(read_session(&self.probe, m)));
        v.unwrap_or_else(|| SessionView::default().settle())
    }

    fn entry(&self, id: &str) -> Result<(u32, &Entry), RpcError> {
        sim_number(id)
            .and_then(|n| self.players.get(&n).map(|e| (n, e)))
            .ok_or_else(|| RpcError::new(code::GONE, format!("no simulated player {id}")))
    }

    fn describe(&self, n: u32, e: &Entry) -> SimPlayer {
        SimPlayer {
            id: format!("sim.{n}"),
            kind: PlayerKind::Phantom,
            source: e.source,
            team: e.team,
            identity: Some(e.identity.clone()),
            player: self
                .bots
                .phantom_player(e.bot.handle)
                .map(|i| format!("player.{i}")),
            brain: self.bots.brain_view(e.bot),
        }
    }

    fn player_add(&mut self, v: Value, _: &mut GameCtx<'_>) -> R {
        if v.get("kind").and_then(Value::as_str) == Some("local") {
            return Err(invalid(
                "controller seats are for people. Use kind = \"phantom\" and provide an identity",
            ));
        }
        let p: AddParams = params(v)?;
        let identity = p.identity.ok_or_else(|| {
            invalid(
                "a phantom needs \"identity\": {xuid, gamertag}, issued by the local \
                 embedded Live server (sim never makes accounts)",
            )
        })?;
        if identity.xuid.trim().is_empty() || identity.gamertag.trim().is_empty() {
            return Err(invalid("identity: xuid and gamertag"));
        }
        let (source, script) = match p.source {
            SourceKind::Script => {
                let steps = crate::tap::pad_steps(&p.steps).map_err(invalid)?;
                let r = Remote::new();
                r.replace(steps);
                (PadSource::Script(r.clone()), Some(r))
            }
            SourceKind::Recording => {
                let name = p
                    .recording
                    .clone()
                    .ok_or_else(|| invalid("a recording source needs \"recording\": <log name>"))?;
                let path = crate::tap::log_path(&agent_proto::output_dir(), &name)?;
                (PadSource::Recording { name, path }, None)
            }
            SourceKind::Brain => (PadSource::Brain, None),
        };
        if p.source != SourceKind::Script && !p.steps.is_empty() {
            return Err(invalid("steps go with a script source"));
        }
        if p.source != SourceKind::Recording && p.recording.is_some() {
            return Err(invalid("recording goes with a recording source"));
        }
        let handle = self
            .bots
            .add_phantom(&identity, p.team, source)
            .map_err(|e| RpcError::new(code::UNAVAILABLE, e))?;
        let n = self.next;
        self.next += 1;
        self.players.insert(
            n,
            Entry {
                bot: Bot { handle },
                source: p.source,
                team: p.team,
                identity,
                script,
            },
        );
        Ok(serde_json::to_value(self.describe(n, &self.players[&n])).expect("serialises"))
    }

    fn player_remove(&mut self, v: Value, _: &mut GameCtx<'_>) -> R {
        let p: RemoveParams = params(v)?;
        let ids: Vec<u32> = match (&p.id, p.all) {
            (Some(_), true) | (None, false) => {
                return Err(invalid("give id, or all: true"));
            }
            (None, true) => self.players.keys().copied().collect(),
            (Some(id), false) => vec![self.entry(id)?.0],
        };
        let mut removed = Vec::new();
        let mut failed = Vec::new();
        for n in ids {
            let bot = self.players[&n].bot;
            match self.bots.remove(bot) {
                Ok(()) => {
                    self.players.remove(&n);
                    removed.push(format!("sim.{n}"));
                }
                Err(e) => failed.push(json!({ "id": format!("sim.{n}"), "error": e })),
            }
        }
        if !failed.is_empty() && removed.is_empty() {
            let mut e = RpcError::new(code::UNAVAILABLE, "the bots backend refused");
            e.data = Some(json!(failed));
            return Err(e);
        }
        Ok(json!({ "removed": removed, "failed": failed }))
    }

    fn player_list(&mut self, _: Value, ctx: &mut GameCtx<'_>) -> R {
        let clankers = ctx
            .with_guest(|m| Ok(self.bots.clankers(Some(m))))
            .unwrap_or_else(|_| self.bots.clankers(None));
        let players: Vec<_> = self.players.iter().map(|(n, e)| self.describe(*n, e)).collect();
        Ok(json!({ "players": players, "clankers": clankers }))
    }

    fn player_pad(&mut self, v: Value, _: &mut GameCtx<'_>) -> R {
        let p: DriveParams = params(v)?;
        let (_, e) = self.entry(&p.id)?;
        let Some(remote) = &e.script else {
            return Err(RpcError::new(
                code::INVALID_REQUEST,
                "this phantom's pad comes from its brain or recording. Add it with a script source to drive it",
            ));
        };
        let steps = crate::tap::pad_steps(&p.steps).map_err(invalid)?;
        let queued = steps.len();
        if p.append {
            remote.append(steps);
        } else {
            remote.replace(steps);
        }
        Ok(json!({ "id": p.id, "queued": queued, "via": "script" }))
    }

    /// `player.<n>` or a simulated player's `sim.<n>` as a players-array
    /// index.
    fn target(&self, who: &str) -> Result<u32, RpcError> {
        if let Some(id) = ObjectId::parse(who) {
            if id.kind == Kind::Player {
                return Ok(id.key);
            }
            return Err(invalid(format!("{who}: a player.<n> or sim.<n>")));
        }
        let (_, e) = self.entry(who)?;
        self.bots.phantom_player(e.bot.handle).ok_or_else(|| {
            RpcError::new(code::GONE, format!("{who} isn't a player in the game yet"))
        })
    }

    fn state_set(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: SetParams = params(v)?;
        self.set_state(p, ctx, Map::new())
    }

    /// `sim.state.set` with `p`; `extra`'s fields join the answer.
    fn set_state(&mut self, p: SetParams, ctx: &mut GameCtx<'_>, extra: Map<String, Value>) -> R {
        state::validate(&p)?;
        let session = self.host_write(ctx)?;
        let index = self.target(&p.player)?;
        let facing = p.facing_vector().map_err(invalid)?;
        let with_extra = move |mut v: Value| {
            if let Some(o) = v.as_object_mut() {
                o.extend(extra.clone());
            }
            v
        };
        if p.pos.is_none() && facing.is_none() {
            let wrote = ctx.with_guest(|m| state::apply(m, index, &p))?;
            return Ok(with_extra(
                json!({ "player": format!("player.{index}"), "wrote": wrote, "session": session }),
            ));
        }
        // A teleport: the game's own, at the next tick (calls).
        let q = self.calls()?;
        let handle = ctx.with_guest(|m| {
            state::unit_handle(m, index)?;
            state::player_handle(m, index)
        })?;
        let mut wrote = ctx.with_guest(|m| state::apply(m, index, &p))?;
        let ticket = calls::submit(
            &q,
            Call::Teleport {
                player: handle,
                pos: p.pos.map(|v| v.map(|c| c as f32)),
                facing: facing.map(|v| v.map(|c| c as f32)),
            },
        )
        .map_err(unavailable)?;
        if p.pos.is_some() {
            wrote.push("pos");
        }
        if facing.is_some() {
            wrote.push("facing");
        }
        let session = serde_json::to_value(session).expect("serialises");
        ctx.defer(
            TELEPORT_TICKS,
            Box::new(move |_: &mut GameCtx<'_>| {
                let out = ticket.outcome()?;
                Some(out.map(|t| {
                    with_extra(json!({
                        "player": format!("player.{index}"),
                        "wrote": wrote,
                        "teleport": t,
                        "session": session,
                    }))
                }))
            }),
        );
        Ok(Value::Null)
    }

    /// The player a mark method means: `who`, else the player on local
    /// user 0 (`world.query`'s `local_user`).
    fn mark_player(who: Option<&str>, objects: &[agent_proto::Object]) -> Result<String, RpcError> {
        if let Some(w) = who {
            return Ok(w.to_owned());
        }
        objects
            .iter()
            .find(|o| o.kind == Kind::Player && o.local_user == Some(0))
            .map(|o| o.id.clone())
            .ok_or_else(|| RpcError::new(code::GONE, "no local player (give player)"))
    }

    /// `player.mark {slot?, player?}`: the player's position and aim now,
    /// kept under the slot (replacing what it held).
    fn player_mark(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: MarkParams = params(v)?;
        let slot = p.slot_name().map_err(invalid)?;
        if !self.marks.contains_key(&slot) && self.marks.len() >= wire::MAX_MARKS {
            return Err(invalid(format!(
                "{} marks already: reuse a slot ({})",
                wire::MAX_MARKS,
                self.marks.keys().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        let objects = ctx
            .reader
            .objects()
            .map_err(|e| RpcError::new(code::UNAVAILABLE, e))?;
        let who = Self::mark_player(p.player.as_deref(), &objects)?;
        // A simulated player's sim.<n> is the game's player.<i>.
        let id = format!("player.{}", self.target(&who)?);
        let o = objects
            .iter()
            .find(|o| o.kind == Kind::Player && o.id == id)
            .ok_or_else(|| RpcError::new(code::GONE, format!("no {who} in the game")))?;
        let pos = o.pos.ok_or_else(|| {
            RpcError::new(code::GONE, format!("{who} has no position (between lives?)"))
        })?;
        let mark = Mark {
            slot: slot.clone(),
            player: id,
            pos,
            forward: o.forward,
            yaw: o.yaw,
            pitch: o.pitch,
            map: ctx.at.map.clone(),
        };
        self.marks.insert(slot, mark.clone());
        let mut v = serde_json::to_value(&mark).expect("serialises");
        v["slots"] = json!(self.marks.keys().collect::<Vec<_>>());
        Ok(v)
    }

    /// `player.recall {slot?, player?}`: the slot's mark put back through
    /// `sim.state.set`'s teleport, answered after the next tick.
    fn player_recall(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: MarkParams = params(v)?;
        let slot = p.slot_name().map_err(invalid)?;
        let mark = self.marks.get(&slot).cloned().ok_or_else(|| {
            RpcError::new(
                code::GONE,
                format!("no mark {slot:?} (player.mark makes one)"),
            )
        })?;
        if let (Some(at), Some(here)) = (&mark.map, &ctx.at.map)
            && !at.eq_ignore_ascii_case(here)
        {
            return Err(RpcError::new(
                code::INVALID_REQUEST,
                format!("mark {slot:?} is on {at}; this is {here}"),
            ));
        }
        let who = match p.player.as_deref() {
            Some(w) => w.to_owned(),
            None => {
                let objects = ctx
                    .reader
                    .objects()
                    .map_err(|e| RpcError::new(code::UNAVAILABLE, e))?;
                Self::mark_player(None, &objects).unwrap_or_else(|_| mark.player.clone())
            }
        };
        let mut extra = Map::new();
        extra.insert("slot".into(), json!(slot));
        extra.insert("mark".into(), serde_json::to_value(&mark).expect("serialises"));
        self.set_state(mark.recall(who), ctx, extra)
    }

    /// The calls queue, or why there is none.
    fn calls(&self) -> Result<calls::Shared, RpcError> {
        match &self.calls {
            Some(q) if calls::lock(q).armed() => Ok(Arc::clone(q)),
            _ => Err(unavailable(
                "the game's own functions (teleport, give, spawn) aren't reachable here: the \
                 agent-calls feature isn't installed"
                    .to_string(),
            )),
        }
    }

    /// A host write may happen now: the session takes writes and the game
    /// ticks.
    fn host_write(&mut self, ctx: &mut GameCtx<'_>) -> Result<SessionView, RpcError> {
        let session = self.session(ctx);
        if let Err(why) = session.kind.writes() {
            let mut e = RpcError::new(code::INVALID_REQUEST, why);
            e.data = Some(json!({ "session": session }));
            return Err(e);
        }
        if ctx.at.boundary != Boundary::Tick {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "host writes happen at a tick boundary, and the game isn't ticking",
            ));
        }
        Ok(session)
    }

    /// `tag` resolved in the loaded map, a bare name among the tags of
    /// `among` (a weapon's name is also its model's, collision's and
    /// animations').
    fn find_tag(&mut self, tag: &str, among: &[&str]) -> Result<calls::TagRef, RpcError> {
        let tags = self.tags.as_mut().ok_or_else(|| {
            unavailable("no tag lookup here (the game has no edit.* tags)".to_string())
        })?;
        tags.find(tag, among).map_err(invalid)
    }

    fn give(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: GiveParams = params(v)?;
        if let Some(a) = &p.ammo {
            state::validate_ammo(a)?;
        }
        let session = self.host_write(ctx)?;
        let index = self.target(&p.player)?;
        let q = self.calls()?;
        let tag = self.find_tag(&p.weapon, &["weap"])?;
        if tag.group != "weap" {
            return Err(invalid(format!(
                "{}: a {} tag, not a weapon (weap)",
                p.weapon, tag.group
            )));
        }
        let unit = ctx.with_guest(|m| state::unit_handle(m, index))?;
        let mode = if p.keep {
            calls::addr::MODE_ADD
        } else {
            calls::addr::MODE_REPLACE
        };
        let ticket = calls::submit(
            &q,
            Call::Give {
                unit,
                weapon: tag.datum,
                mode,
            },
        )
        .map_err(unavailable)?;
        let (ammo, session) = (p.ammo, serde_json::to_value(session).expect("serialises"));
        ctx.defer(
            CALL_TICKS,
            Box::new(move |ctx: &mut GameCtx<'_>| {
                let out = ticket.outcome()?;
                Some(out.and_then(|made| {
                    let object = made["object"].as_str().unwrap_or_default().to_owned();
                    let datum = u32::from_str_radix(&object, 16).unwrap_or(u32::MAX);
                    if let Some(a) = ammo {
                        ctx.with_guest(|m| state::set_magazine(m, datum, a))?;
                    }
                    let held = ctx.with_guest(|m| Ok(state::held_object(m, index)))?;
                    Ok(json!({
                        "player": format!("player.{index}"),
                        "weapon": tag.json(),
                        "object": object,
                        "mode": made["mode"],
                        "held": held.as_deref() == Some(object.as_str()),
                        "session": session,
                    }))
                }))
            }),
        );
        Ok(Value::Null)
    }

    /// `audio.play`: queue the game's own sound start for a sound tag at
    /// a player's unit. Presentation only, so a co-op session may ask.
    fn audio_play(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: wire::PlayParams = params(v)?;
        let index = self.target(p.player.as_deref().unwrap_or("player.0"))?;
        let q = self.calls()?;
        let tag = self.find_tag(&p.sound, &["snd!"])?;
        if tag.group != "snd!" {
            return Err(invalid(format!(
                "{}: a {} tag, not a sound (snd!)",
                p.sound, tag.group
            )));
        }
        let unit = ctx.with_guest(|m| state::unit_handle(m, index))?;
        let ticket = calls::submit(
            &q,
            Call::Sound {
                unit,
                sound: tag.datum,
            },
        )
        .map_err(unavailable)?;
        ctx.defer(
            CALL_TICKS,
            Box::new(move |_ctx: &mut GameCtx<'_>| {
                let out = ticket.outcome()?;
                Some(out.map(|made| {
                    json!({
                        "player": format!("player.{index}"),
                        "sound": tag.json(),
                        "instance": made["instance"],
                    })
                }))
            }),
        );
        Ok(Value::Null)
    }

    /// `world.object.animate`: queue the start of a named animation of an
    /// object's own graph. Host only: the object's pose is simulation
    /// state.
    fn object_animate(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: wire::AnimateParams = params(v)?;
        let id = edits::object_id(&p.id)?;
        let session = self.host_write(ctx)?;
        let q = self.calls()?;
        let name = self
            .tags
            .as_mut()
            .and_then(|tags| tags.string_id(&p.animation))
            .ok_or_else(|| {
                invalid(format!("loaded map has no unique string ID for {:?}", p.animation))
            })?;
        let ticket = calls::submit(&q, Call::Animate { id, name }).map_err(unavailable)?;
        let session = serde_json::to_value(session).expect("serialises");
        ctx.defer(
            CALL_TICKS,
            Box::new(move |_: &mut GameCtx<'_>| {
                let out = ticket.outcome()?;
                Some(out.map(|mut made| {
                    made["object"] = json!(id.to_string());
                    made["session"] = session.clone();
                    made
                }))
            }),
        );
        Ok(Value::Null)
    }

    /// `recording.play`: queue the engine's recording player for a named
    /// recording on a player's unit. The unit moves as the recording's
    /// control stream drives it, so a co-op session may not ask.
    fn recording_play(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: wire::RecordingPlayParams = params(v)?;
        self.host_write(ctx)?;
        let flags = recording::flags(p.mode.as_deref())?;
        let index = self.target(p.player.as_deref().unwrap_or("player.0"))?;
        let q = self.calls()?;
        let (slot, name) = ctx.with_guest(|m| recording::find(m, &p.recording))?;
        let unit = ctx.with_guest(|m| state::unit_handle(m, index))?;
        let ticket = calls::submit(
            &q,
            Call::Recording {
                unit,
                index: slot,
                flags,
            },
        )
        .map_err(unavailable)?;
        ctx.defer(
            CALL_TICKS,
            Box::new(move |_ctx: &mut GameCtx<'_>| {
                let out = ticket.outcome()?;
                Some(out.map(|started| {
                    json!({
                        "player": format!("player.{index}"),
                        "recording": name,
                        "index": slot,
                        "unit": started["unit"],
                    })
                }))
            }),
        );
        Ok(Value::Null)
    }

    fn spawn(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: SpawnParams = params(v)?;
        self.spawn_object(p, None, None, ctx)
    }

    fn spawn_object(
        &mut self,
        p: SpawnParams,
        session: Option<SessionView>,
        group: Option<&str>,
        ctx: &mut GameCtx<'_>,
    ) -> R {
        if !p.pos.iter().all(|v| v.is_finite() && v.abs() < 1e6) {
            return Err(invalid("pos: three finite world coordinates"));
        }
        let facing = wire::facing_vector(p.yaw, None, None).map_err(invalid)?;
        let session = match session {
            Some(session) => session,
            None => self.host_write(ctx)?,
        };
        let q = self.calls()?;
        let groups;
        let among = match group {
            Some(group) => {
                groups = [group];
                groups.as_slice()
            }
            None => wire::OBJECT_GROUPS,
        };
        let tag = self.find_tag(&p.tag, among)?;
        if !among.contains(&tag.group.as_str()) {
            return Err(invalid(format!(
                "{}: a {} tag; objects are {}",
                p.tag,
                tag.group,
                among.join(" ")
            )));
        }
        let mp_scenario = if group.is_some() {
            Some(ctx.with_guest(|m| {
                guest_layout::global::SCENARIO
                    .pointer(m)
                    .map_err(|e| unavailable(format!("reading the scenario at {:#x}", e.addr)))?
                    .ok_or_else(|| unavailable("no loaded scenario".into()))
            })?)
        } else {
            None
        };
        let ticket = calls::submit(
            &q,
            Call::Spawn {
                tag: tag.datum,
                pos: p.pos.map(|c| c as f32),
                facing: facing.map(|v| v.map(|c| c as f32)),
                mp_scenario,
            },
        )
        .map_err(unavailable)?;
        ctx.defer(
            CALL_TICKS,
            Box::new(move |_: &mut GameCtx<'_>| {
                let out = ticket.outcome()?;
                Some(out.and_then(|made| {
                    if mp_scenario.is_none() {
                        return Ok(json!({
                            "tag": tag.json(),
                            "object": made["object"],
                            "pos": made["pos"],
                            "session": session,
                        }));
                    }
                    #[derive(serde::Deserialize)]
                    struct Made {
                        object: String,
                        pos: Option<[f32; 3]>,
                    }
                    let made: Made = serde_json::from_value(made)
                        .map_err(|e| unavailable(format!("invalid spawn outcome: {e}")))?;
                    let reply = agent_proto::forge::ProbeReply::Replication {
                        session: session.clone(),
                        tag: agent_proto::forge::ProbeTag {
                            tag: format!("{:#010x}", tag.datum),
                            group: tag.group.clone(),
                            name: tag.name.clone(),
                        },
                        object: made.object,
                        pos: made.pos,
                    };
                    Ok(serde_json::to_value(reply).expect("serialises"))
                }))
            }),
        );
        Ok(Value::Null)
    }

    fn objects_method(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: wire::objects::ObjectsParams = params(v)?;
        let reply = ctx.with_guest(|m| objects::list(&*m, &p))?;
        Ok(serde_json::to_value(reply).expect("serialises"))
    }

    /// Answered at the tick boundary on the game thread, whose event
    /// buffer holds the tick that just ran.
    fn events_method(&mut self, v: Value, _: &mut GameCtx<'_>) -> R {
        let p: wire::events::EventsParams = params(v)?;
        let reply = sim_state::events(|e| events::reply(e, &p))?;
        Ok(serde_json::to_value(reply).expect("serialises"))
    }

    fn session_method(&mut self, _: Value, ctx: &mut GameCtx<'_>) -> R {
        Ok(serde_json::to_value(self.session(ctx)).expect("serialises"))
    }

    fn view_method(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: ViewParams = params(v)?;
        let roots = p
            .roots
            .unwrap_or_else(|| wire::DEFAULT_ROOTS.iter().map(|r| (*r).into()).collect());
        wire::check_roots(&roots)?;
        self.view(&roots, ctx)
    }

    fn expect(&mut self, v: Value, ctx: &mut GameCtx<'_>) -> R {
        let p: ExpectParams = params(v)?;
        if p.checks.is_empty() {
            return Err(invalid("checks: at least one"));
        }
        let roots = wire::roots(&p.checks);
        wire::check_roots(&roots)?;
        let view = self.view(&roots, ctx)?;
        let mut v = expect::verdict(&view, &p.checks);
        v["at"] = view["at"].clone();
        Ok(v)
    }

    /// The document `sim.view` answers, with only `roots` read.
    fn view(&mut self, roots: &[String], ctx: &mut GameCtx<'_>) -> R {
        let has = |r: &str| roots.iter().any(|x| x == r);
        let mut out = Map::new();
        out.insert(
            "at".into(),
            serde_json::to_value(&ctx.at).expect("serialises"),
        );
        if has("session") {
            let s = self.session(ctx);
            out.insert(
                "session".into(),
                serde_json::to_value(s).expect("serialises"),
            );
        }
        for (root, m) in [
            ("ui", agent_proto::method::UI_SCREEN),
            ("input", agent_proto::method::INPUT_STATE),
        ] {
            if has(root) {
                let v = match ctx.host(m, Value::Null) {
                    Some(Ok(mut v)) => {
                        if let Some(o) = v.as_object_mut() {
                            o.remove("at");
                        }
                        v
                    }
                    _ => Value::Null,
                };
                out.insert(root.into(), v);
            }
        }
        let world = ["player", "object", "count", "sim"].iter().any(|r| has(r));
        let objects = if world {
            ctx.reader
                .objects()
                .map_err(|e| RpcError::new(code::UNAVAILABLE, e))?
        } else {
            Vec::new()
        };
        if has("player") {
            let mut players = Map::new();
            for o in objects.iter().filter(|o| o.kind == Kind::Player) {
                let Some(id) = ObjectId::parse(&o.id) else {
                    continue;
                };
                let mut v = serde_json::to_value(o).expect("serialises");
                let vit = ctx
                    .with_guest(|m| state::vitals(m, id.key))
                    .unwrap_or_default();
                if let Some(obj) = v.as_object_mut() {
                    obj.extend(vit);
                }
                players.insert(id.key.to_string(), v);
            }
            out.insert("player".into(), Value::Object(players));
        }
        if has("object") {
            let all: Map<String, Value> = objects
                .iter()
                .map(|o| (o.id.clone(), serde_json::to_value(o).expect("serialises")))
                .collect();
            out.insert("object".into(), Value::Object(all));
        }
        if has("count") {
            let mut c = Map::new();
            for k in Kind::ALL {
                let n = objects.iter().filter(|o| o.kind == k).count();
                c.insert(k.as_str().into(), json!(n));
            }
            let mut teams: BTreeMap<String, usize> = BTreeMap::new();
            for o in objects.iter().filter(|o| o.kind == Kind::Player) {
                if let Some(t) = o.team {
                    *teams.entry(t.to_string()).or_default() += 1;
                }
            }
            c.insert("team".into(), json!(teams));
            c.insert("total".into(), json!(objects.len()));
            out.insert("count".into(), Value::Object(c));
        }
        if has("sim") {
            let mut s = Map::new();
            s.insert("count".into(), json!(self.players.len()));
            for (n, e) in &self.players {
                let d = self.describe(*n, e);
                s.insert(n.to_string(), serde_json::to_value(d).expect("serialises"));
            }
            out.insert("sim".into(), Value::Object(s));
        }
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests;
