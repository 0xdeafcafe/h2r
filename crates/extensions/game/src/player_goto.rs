//! `player.goto`: drive the local player to a point or a spot with the
//! bots' route brain (docs/runtime/agent-api.md, "`player.goto`: drive the
//! player somewhere"; docs/game/bots.md, "Driving a player"), so an agent
//! gets somewhere in one call instead of stepping the pad bit by bit.
//!
//! The autopilot is [`bots::goto::Goto`]: the bots' planner and route
//! runner over the map's nav mesh, with replanning when the player strays
//! or stops getting nearer. This module is its glue: an agent ticker that
//! reads `player.0` each tick, steps the autopilot and holds the first
//! controller's pad with its motion (the pad tap's
//! [`crate::superbounce_assist::set_verify_pad`], the same one
//! `superbounce.verify` drives `player.0` with, so the two never run at
//! once); and the nav mesh brought up for the loaded map when no bot has
//! ([`nav_for`]). A turn to `face` at the end is the game's own teleport,
//! with no move.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use agent::sim::calls::{self, Call, PendingCall};
use agent::{Access, GameCtx};
use agent_proto::goto::{
    self as wire, GotoParams, GotoStarted, GotoStatus, GotoStatusParams, GotoTo,
};
use agent_proto::{RpcError, code};
use bots::cache::MapData;
use bots::goto::{Ask, Goto, Target, Then};
use bots::spots::{Crouch, Known, Site};
use serde_json::Value;
use superbounce::{Kind, Spot};

/// The ticker's name.
pub const TICKER: &str = "player.goto";
/// The seat driven: the first local player, whose pad the tap replaces.
pub const PLAYER: u32 = 0;
/// Ticks a drive waits for the nav mesh before it gives up: 2 minutes (a
/// mesh built from nothing takes a while).
pub const NAV_WAIT: u32 = 3_600;
/// Ticks the turn at the end may take.
pub const FACE_TICKS: u32 = 60;

static JOB: Mutex<Option<Job>> = Mutex::new(None);
static NEXT_JOB: AtomicU32 = AtomicU32::new(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

// ---- the nav mesh, with or without bots ----

/// How the loaded map's nav mesh stands.
#[derive(Clone, Debug)]
pub enum Nav {
    Ready(Arc<MapData>),
    /// Being read or built (by the bots' glue, or asked for here).
    Loading,
    Failed(String),
}

/// The map asked for here and its load generation, and why its read
/// failed, if it did.
static ASKED: Mutex<Option<(String, u64, Option<String>)>> = Mutex::new(None);

/// The nav mesh of `map` (the loaded one), asking for it when nothing has:
/// with bots in the game their glue loads it (`bot_glue`, only while a
/// phantom plays); with none it is asked for here the same way, from the
/// nav worker when it runs (`file`: the map's mounted source, which the
/// worker builds from when the cache is stale), else read from the cache
/// directory (`h2 bots nav <map>` builds it by hand).
pub fn nav_for(map: &str, file: Option<String>) -> Nav {
    if bots::autonav::loaded().eq_ignore_ascii_case(map) {
        if let Some(d) = bots::cache::current() {
            return Nav::Ready(d);
        }
        let asked = lock(&ASKED).clone();
        match asked {
            Some((m, _, Some(why))) if m.eq_ignore_ascii_case(map) => return Nav::Failed(why),
            Some((m, _, None)) if m.eq_ignore_ascii_case(map) => return Nav::Loading,
            _ if !bots::phantom::global().is_empty() => return Nav::Loading,
            _ => {}
        }
    }
    request(map, file)
}

fn request(map: &str, file: Option<String>) -> Nav {
    let (generation, retired) = bots::autonav::map_loaded(map);
    *lock(&ASKED) = Some((map.to_owned(), generation, None));
    let name = map.to_owned();
    if let Some(h) = bots::autonav::handle() {
        if let Some(r) = retired {
            // A mesh's many allocations are freed off the game's thread.
            let _ = background::spawn("goto-nav-retire", move || drop(r));
        }
        h.want_active(&name, file, generation);
        return Nav::Loading;
    }
    let Some(dir) = bots::spots::verified_dir() else {
        return Nav::Failed("no bots data directory to read the nav mesh from".into());
    };
    let spawned = background::spawn("goto-nav", move || {
        drop(retired);
        if let Err(why) = bots::autonav::load_cached(&dir, &name, generation, None) {
            let why = format!(
                "nav mesh {} (build it with `h2 bots nav {name}`)",
                why.why()
            );
            let mut a = lock(&ASKED);
            if a.as_ref()
                .is_some_and(|(m, g, _)| *m == name && *g == generation)
            {
                *a = Some((name, generation, Some(why)));
            }
        }
    });
    match spawned {
        Ok(_) => Nav::Loading,
        Err(e) => Nav::Failed(format!("no thread to read the nav mesh: {e}")),
    }
}

// ---- the target ----

/// What `to` names on `map`: a point, or a spot of the mode's scan
/// (`scanned`, verified statuses applied) by its index among its kind, best
/// first (`superbounce_verify::plan::candidates`), with what the map's
/// verified file says of it.
pub fn target(
    to: &GotoTo,
    scanned: &[Spot],
    verified: &[superbounce::verified::Entry],
) -> Result<Target, String> {
    match to {
        GotoTo::Point(p) => {
            if p.iter().all(|c| c.is_finite()) {
                Ok(Target::Point(*p))
            } else {
                Err("to: the point must be finite".into())
            }
        }
        GotoTo::Spot(r) => {
            let kind = match r.kind.as_deref().unwrap_or("bounce") {
                "bounce" => Kind::Bounce,
                "crouch" => Kind::Crouch,
                k => return Err(format!("to.kind {k:?}: bounce or crouch")),
            };
            if scanned.is_empty() {
                return Err(
                    "no spot scan of this map: turn the mode on (superbounce.set {\"on\":true}) \
                     and wait for superbounce.state's scan.status to read ready"
                        .into(),
                );
            }
            let list = crate::superbounce_verify::plan::candidates(scanned, kind);
            let s = list.get(r.spot).ok_or_else(|| {
                format!(
                    "to.spot {}: the scan has {} of that kind",
                    r.spot,
                    list.len()
                )
            })?;
            let e = superbounce::verified::entry_for(verified, s);
            Ok(match kind {
                Kind::Crouch => Target::Crouch(Crouch::from_spot(s, e)),
                Kind::Bounce => Target::Site(
                    Site::from_spot(s)
                        .expect("a bounce spot is a site")
                        .verified(e),
                ),
            })
        }
    }
}

/// Where a target ends before the map's mesh is known: the point, a
/// crouch spot's start, a standing bounce's approach or a drop's strip.
fn rough_goal(t: &Target) -> [f32; 3] {
    match t {
        Target::Point(p) => *p,
        Target::Crouch(c) => bots::bounce_route::prime_start(c),
        Target::Site(s) => bots::bounce_route::approach(s, &|_| None).unwrap_or(s.target()),
    }
}

// ---- the job ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Loading,
    Running,
    Facing,
    Done,
    Failed,
    Cancelled,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Loading => "loading",
            State::Running => "running",
            State::Facing => "facing",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }

    fn live(self) -> bool {
        matches!(self, State::Loading | State::Running | State::Facing)
    }
}

struct Job {
    id: u32,
    ask: Ask,
    goto: Option<Goto>,
    map: String,
    known: Known,
    face: Option<f32>,
    state: State,
    nav: Option<Arc<MapData>>,
    /// The turn at the end, and when it was asked for.
    ticket: Option<PendingCall>,
    faced_at: u32,
    ticks: u32,
    position: Option<[f32; 3]>,
    error: Option<String>,
}

impl Job {
    fn report(&self) -> GotoStatus {
        let s = self.goto.as_ref().map(|g| g.status(self.position));
        let distance = s.as_ref().and_then(|s| s.distance).or_else(|| {
            let at = self.position?;
            let nav = self.nav.as_deref()?;
            bots::goto::straight(&self.ask.target, at, nav)
        });
        GotoStatus {
            state: self.state.name().into(),
            job: Some(self.id),
            kind: self.ask.target.kind().into(),
            goal: Some(self.goal()),
            then: self.ask.then.name().into(),
            steps: s
                .as_ref()
                .map(|s| s.steps.iter().map(|n| n.to_string()).collect())
                .unwrap_or_default(),
            step: s.as_ref().map_or(0, |s| s.step),
            phase: s.as_ref().map_or("", |s| s.phase).into(),
            primed: s.as_ref().is_some_and(|s| s.primed),
            position: self.position,
            distance,
            path: s.as_ref().map(|s| s.path.clone()).unwrap_or_default(),
            replans: s.as_ref().map_or(0, |s| s.replans),
            replanned: s.as_ref().and_then(|s| s.replanned).map(Into::into),
            ticks: self.ticks,
            error: self.error.clone(),
        }
    }

    fn goal(&self) -> [f32; 3] {
        match (&self.ask.target, self.nav.as_deref()) {
            (Target::Site(s), Some(nav)) => {
                bots::bounce_route::approach(s, &|s| bots::bounce_route::launch_floor(nav, s))
                    .unwrap_or(s.target())
            }
            (t, _) => rough_goal(t),
        }
    }

    fn end(&mut self, state: State, error: Option<String>) {
        crate::superbounce_assist::set_verify_pad(None);
        self.state = state;
        self.error = error;
        self.ticket = None;
    }

    /// One tick of the drive.
    fn tick(&mut self, ctx: &mut GameCtx<'_>) {
        if !self.state.live() {
            return;
        }
        self.ticks += 1;
        let read = ctx
            .read_guest(|m| {
                let map = crate::health::loaded_map_name(m).ok();
                let file = crate::map_slots::active_file(&crate::layout_glue::ReadGm(m));
                (map, file, crate::superbounce_verify::read_seat(m, PLAYER))
            })
            .ok();
        let Some((map, file, seat)) = read else {
            return;
        };
        if map.as_deref() != Some(self.map.as_str()) {
            return self.end(
                State::Failed,
                Some("the map changed or the game ended".into()),
            );
        }
        if self.state == State::Facing {
            if let Some(out) = self.ticket.as_ref().and_then(|t| t.outcome()) {
                return match out {
                    Ok(_) => self.end(State::Done, None),
                    Err(e) => self.end(State::Failed, Some(format!("the turn: {}", e.message))),
                };
            }
            if self.ticks.wrapping_sub(self.faced_at) > FACE_TICKS {
                self.end(State::Failed, Some("the turn was never made".into()));
            }
            return;
        }
        let nav = match self.nav.clone() {
            Some(n) => n,
            None => match nav_for(&self.map, file) {
                Nav::Ready(d) => {
                    self.nav = Some(Arc::clone(&d));
                    self.state = State::Running;
                    d
                }
                Nav::Loading if self.ticks > NAV_WAIT => {
                    let why = format!("no nav mesh for {} in {NAV_WAIT} ticks", self.map);
                    return self.end(State::Failed, Some(why));
                }
                Nav::Loading => return,
                Nav::Failed(why) => return self.end(State::Failed, Some(why)),
            },
        };
        let Some((read, _)) = seat else {
            return self.end(State::Failed, Some(format!("player.{PLAYER} has no unit")));
        };
        let Some(at) = read.pos else {
            return self.end(
                State::Failed,
                Some(format!("player.{PLAYER} has no position")),
            );
        };
        self.position = Some(at);
        let tick = self.ticks;
        let goto = self
            .goto
            .get_or_insert_with(|| Goto::new(self.ask, tick, u64::from(tick)));
        let out = goto.step(&nav, &self.known, at, tick, read.word);
        match out.done {
            None => {
                let m = out.motion;
                let walk = [m.dir[0] * m.speed, m.dir[1] * m.speed];
                crate::superbounce_assist::set_verify_pad(Some(
                    crate::superbounce_assist::VerifyPad {
                        stick: crate::superbounce_verify::stick_for(walk, read.yaw.unwrap_or(0.0)),
                        crouch: m.crouch,
                        jump: m.jump,
                    },
                ));
            }
            Some(Ok(())) => {
                crate::superbounce_assist::set_verify_pad(None);
                match self.face {
                    Some(yaw) => self.turn(ctx, yaw),
                    None => self.end(State::Done, None),
                }
            }
            Some(Err(why)) => self.end(State::Failed, Some(why.into())),
        }
    }

    /// Turns the player to `yaw` with the game's teleport, not moving it.
    fn turn(&mut self, ctx: &mut GameCtx<'_>, yaw: f32) {
        let handle = ctx
            .read_guest(|m| agent::sim::state::player_handle(m, PLAYER).ok())
            .ok()
            .flatten();
        let (s, c) = yaw.sin_cos();
        let submitted = match (crate::superbounce_verify::calls(), handle) {
            (Some(q), Some(player)) => calls::submit(
                q,
                Call::Teleport {
                    player,
                    pos: None,
                    facing: Some([c, s, 0.0]),
                },
            ),
            (None, _) => Err("the agent-calls feature is off".into()),
            (_, None) => Err(format!("no player.{PLAYER}")),
        };
        match submitted {
            Ok(t) => {
                self.ticket = Some(t);
                self.faced_at = self.ticks;
                self.state = State::Facing;
            }
            Err(e) => self.end(State::Failed, Some(format!("the turn: {e}"))),
        }
    }
}

/// Whether a drive holds `player.0`'s pad now (`superbounce.verify` waits
/// for it).
pub fn running() -> bool {
    lock(&JOB).as_ref().is_some_and(|j| j.state.live())
}

// ---- the methods ----

fn params<T: serde::de::DeserializeOwned + Default>(v: Value, method: &str) -> Result<T, RpcError> {
    if v.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(v).map_err(|e| invalid(format!("{method}: {e}")))
}

/// The ask `p` makes, against the mode's scan.
pub fn ask_for(
    p: &GotoParams,
    scanned: &[Spot],
    verified: &[superbounce::verified::Entry],
) -> Result<Ask, String> {
    let target = target(&p.to, scanned, verified)?;
    let then = match p.then.as_deref() {
        None => Then::Stop,
        Some(t) => {
            Then::parse(t).ok_or_else(|| format!("then {t:?}: stop, crouch_recipe or bounce"))?
        }
    };
    let mut ask = Ask::new(target, then);
    if let Some(w) = p.within {
        if !(bots::goto::WITHIN_MIN..=bots::goto::WITHIN_MAX).contains(&w) {
            return Err(format!(
                "within {w}: {} to {} world units",
                bots::goto::WITHIN_MIN,
                bots::goto::WITHIN_MAX
            ));
        }
        ask.within = w;
    }
    if let Some(t) = p.timeout_ticks {
        if t == 0 {
            return Err("timeout_ticks: at least 1".into());
        }
        ask.timeout = t;
    }
    if p.face.is_some_and(|f| !f.is_finite()) {
        return Err("face: a finite yaw in radians".into());
    }
    ask.check().map_err(String::from)?;
    Ok(ask)
}

/// `player.goto`.
fn goto(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    if v.is_null() {
        return Err(invalid(
            "player.goto: to is required (a point [x, y, z] or {\"spot\": n})",
        ));
    }
    let p: GotoParams =
        serde_json::from_value(v).map_err(|e| invalid(format!("{}: {e}", wire::GOTO)))?;
    if crate::superbounce_verify::drives_pad() {
        return Err(invalid(
            "a superbounce.verify batch is driving player.0; superbounce.verify_cancel stops it",
        ));
    }
    if !crate::superbounce_assist::tapped() {
        return Err(unavailable(
            "the pad tap is not in place (the superbounce-assist feature is off), so player.0 \
             can't be driven",
        ));
    }
    if p.face.is_some()
        && crate::superbounce_verify::calls().is_none_or(|q| !calls::lock(q).armed())
    {
        return Err(unavailable(
            "face needs the game's teleport, which isn't reachable (the agent-calls feature is off)",
        ));
    }
    let map = ctx
        .read_guest(|m| crate::health::loaded_map_name(m).ok())?
        .filter(|n| !n.is_empty())
        .ok_or_else(|| unavailable("no map is loaded"))?;
    ctx.read_guest(|m| agent::sim::state::unit_handle(m, PLAYER))??;
    let (scan_map, scanned) = crate::superbounce_mode::scanned_spots();
    let scanned: &[Spot] = if scan_map.as_deref() == Some(map.as_str()) {
        &scanned
    } else {
        &[]
    };
    let verified = bots::spots::verified_for(&map);
    let ask = ask_for(&p, scanned, &verified).map_err(invalid)?;
    let known = Known::merge_verified(bots::glitches::bounces(&map), scanned, &verified);
    let file = ctx
        .read_guest(|m| crate::map_slots::active_file(&crate::layout_glue::ReadGm(m)))
        .ok()
        .flatten();
    let nav = match nav_for(&map, file) {
        Nav::Ready(d) => Some(d),
        Nav::Loading => None,
        Nav::Failed(why) => return Err(unavailable(why)),
    };
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    let mut j = lock(&JOB);
    // A new drive replaces the last.
    if let Some(old) = j.as_mut().filter(|o| o.state.live()) {
        old.end(
            State::Cancelled,
            Some("replaced by a new player.goto".into()),
        );
    }
    let job = Job {
        id,
        ask,
        goto: None,
        map: map.clone(),
        known,
        face: p.face,
        state: if nav.is_some() {
            State::Running
        } else {
            State::Loading
        },
        nav,
        ticket: None,
        faced_at: 0,
        ticks: 0,
        position: None,
        error: None,
    };
    let started = GotoStarted {
        job: id,
        kind: ask.target.kind().into(),
        goal: job.goal(),
        then: ask.then.name().into(),
        map,
        nav: if job.nav.is_some() {
            "ready"
        } else {
            "loading"
        }
        .into(),
    };
    *j = Some(job);
    Ok(serde_json::to_value(started).expect("player.goto serialises"))
}

fn report_now() -> GotoStatus {
    match lock(&JOB).as_ref() {
        Some(j) => j.report(),
        None => GotoStatus {
            state: "none".into(),
            ..GotoStatus::default()
        },
    }
}

/// `player.goto_status`.
fn goto_status(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let p: GotoStatusParams = params(v, wire::GOTO_STATUS)?;
    let reply = || serde_json::to_value(report_now()).expect("player.goto_status serialises");
    match p.wait_ticks {
        Some(w) if w > 0 && running() => {
            // Answered by the poll itself before the defer's own limit.
            let wait = w.min(agent::DEFER_TICKS_MAX - 10);
            let mut waited = 0u64;
            ctx.defer(
                wait + 5,
                Box::new(move |_: &mut GameCtx<'_>| {
                    waited += 1;
                    (waited >= wait || !running()).then(|| Ok::<Value, RpcError>(reply()))
                }),
            );
            Ok(Value::Null)
        }
        _ => Ok(reply()),
    }
}

/// `player.goto_cancel`.
fn goto_cancel(v: Value) -> Result<Value, RpcError> {
    if !(v.is_null() || v.as_object().is_some_and(|o| o.is_empty())) {
        return Err(invalid("player.goto_cancel takes no parameters"));
    }
    let mut j = lock(&JOB);
    let report = match j.as_mut() {
        Some(job) => {
            if job.state.live() {
                job.end(State::Cancelled, None);
            }
            job.report()
        }
        None => GotoStatus {
            state: "none".into(),
            ..GotoStatus::default()
        },
    };
    Ok(serde_json::to_value(report).expect("player.goto_cancel serialises"))
}

/// The ticker: one tick of the drive, if one is under way.
fn on_tick(ctx: &mut GameCtx<'_>) {
    let mut j = lock(&JOB);
    if let Some(job) = j.as_mut() {
        job.tick(ctx);
    }
}

/// Adds the methods and the ticker to `agent`.
pub fn install(agent: &agent::Agent) {
    agent.add_game_method(wire::GOTO, Access::Act, goto);
    agent.add_game_method(wire::GOTO_STATUS, Access::Read, goto_status);
    agent.add_method(wire::GOTO_CANCEL, Access::Act, goto_cancel);
    agent.add_ticker(TICKER, on_tick);
}

#[cfg(test)]
mod tests;
