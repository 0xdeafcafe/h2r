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
//!
//! They are game methods ([`crate::Agent::add_game_method`]): answered on
//! the game's thread at a tick boundary while the game ticks, with guest
//! memory at hand.

pub mod bots;
pub mod calls;
pub mod events;
pub mod hashes;
pub mod objects;
pub mod state;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::expect;
use agent_proto::sim::{
    self as wire, AddParams, DriveParams, ExpectParams, GiveParams, PlayerKind, RemoveParams,
    SessionKind, SessionView, SetParams, SimPlayer, SourceKind, SpawnParams, ViewParams, method,
};
use agent_proto::{Boundary, Kind, ObjectId, RpcError, code};
use engine::GuestMemory;
use input::backend::remote::Remote;
use serde_json::{Map, Value, json};

use crate::exec::GameCtx;
use crate::{Access, Agent};
use bots::{Bot, Bots, PadSource};
use calls::{Call, Tags};

/// Reads the session kind from guest memory: the host's (boot reads the
/// simulation mode and the squad session as `coop_probe` does).
pub type SessionProbe = Box<dyn FnMut(&dyn GuestMemory) -> SessionView + Send>;

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
    probe: Option<SessionProbe>,
    players: BTreeMap<u32, Entry>,
    next: u32,
    /// The game's own functions, once the host installs them.
    calls: Option<calls::Shared>,
    /// Tag names for `sim.give` and `sim.spawn`.
    tags: Option<Box<dyn Tags>>,
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

    /// Sets the session probe.
    pub fn set_probe(&self, probe: SessionProbe) {
        self.lock().probe = Some(probe);
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
        probe,
        players: BTreeMap::new(),
        next: 1,
        calls: None,
        tags: None,
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
    add(method::OBJECTS, Access::Read, Sim::objects_method);
    add(method::EVENTS, Access::Read, Sim::events_method);
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
        let Some(probe) = self.probe.as_mut() else {
            return SessionView {
                kind: SessionKind::Unknown,
                ..Default::default()
            }
            .settle();
        };
        let mut v = None;
        ctx.reader.with_guest(&mut |m| v = Some(probe(m)));
        v.unwrap_or_default().settle()
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
                if !agent_proto::valid_log_name(&name) {
                    return Err(invalid(format!("{name}: not a log name")));
                }
                let path = agent_proto::cache_dir()
                    .join("inputs")
                    .join(format!("{name}.input"));
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
        state::validate(&p)?;
        let session = self.host_write(ctx)?;
        let index = self.target(&p.player)?;
        let facing = p.facing_vector().map_err(invalid)?;
        if p.pos.is_none() && facing.is_none() {
            let wrote = ctx.with_guest(|m| state::apply(m, index, &p))?;
            return Ok(
                json!({ "player": format!("player.{index}"), "wrote": wrote, "session": session }),
            );
        }
        // A teleport: the game's own, at the next tick (calls).
        let q = self.calls()?;
        let handle = ctx.with_guest(|m| {
            state::unit_handle(m, index)?;
            state::player_handle(m, index)
        })?;
        let mut wrote = ctx.with_guest(|m| state::apply(m, index, &p))?;
        let ticket = calls::lock(&q)
            .submit(Call::Teleport {
                player: handle,
                pos: p.pos.map(|v| v.map(|c| c as f32)),
                facing: facing.map(|v| v.map(|c| c as f32)),
            })
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
                let out = calls::lock(&q).take(ticket)?;
                Some(out.map_err(unavailable).map(|t| {
                    json!({
                        "player": format!("player.{index}"),
                        "wrote": wrote,
                        "teleport": t,
                        "session": session,
                    })
                }))
            }),
        );
        Ok(Value::Null)
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
        let ticket = calls::lock(&q)
            .submit(Call::Give {
                unit,
                weapon: tag.datum,
                mode,
            })
            .map_err(unavailable)?;
        let (ammo, session) = (p.ammo, serde_json::to_value(session).expect("serialises"));
        ctx.defer(
            CALL_TICKS,
            Box::new(move |ctx: &mut GameCtx<'_>| {
                let out = calls::lock(&q).take(ticket)?;
                Some(out.map_err(unavailable).and_then(|made| {
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
        let ticket = calls::lock(&q)
            .submit(Call::Spawn {
                tag: tag.datum,
                pos: p.pos.map(|c| c as f32),
                facing: facing.map(|v| v.map(|c| c as f32)),
                mp_scenario,
            })
            .map_err(unavailable)?;
        ctx.defer(
            CALL_TICKS,
            Box::new(move |_: &mut GameCtx<'_>| {
                let out = calls::lock(&q).take(ticket)?;
                Some(out.map_err(unavailable).and_then(|made| {
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
