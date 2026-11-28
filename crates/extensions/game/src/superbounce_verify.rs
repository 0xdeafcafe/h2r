//! `superbounce.verify`: trials of the loaded map's spots in the running
//! game (docs/game/superbounce.md, "Verifying spots"; the methods in
//! docs/runtime/agent-api.md).
//!
//! A batch is planned from the scan ([`plan`]), then run one trial at a
//! time on one seat by an agent ticker (`Agent::add_ticker`), a tick at a
//! time, because a batch runs far longer than a deferred answer may wait.
//! The first local player is driven through its pad
//! ([`crate::superbounce_assist::set_verify_pad`]); a brain-driven bot by
//! holding its pad (`bots::phantom::Phantoms::hold_pad`), or, for a route,
//! by ordering it (`bots.bounce_to`'s order). Trials reset by teleport (the
//! game's own, `agent::sim::calls`) and by writing the level pair word, not
//! by snapshots: `state.save` refuses a game that is not quiescent.
//!
//! What a trial does is [`trial`]; the verdicts, lines and the verified
//! file's entries are [`summary`]. A finished batch of the scan's spots
//! writes the map's verified file (`bots::spots::store_verified`), which
//! the mode's scan, the HUD and the bots read back: proven spots first,
//! failed ones hidden.

pub mod plan;
pub mod summary;
pub mod trial;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use agent::sim::calls::{self, Call, PendingCall, Shared};
use agent::{Access, GameCtx};
use agent_proto::superbounce::{
    self as wire, VerifyParams, VerifyReport, VerifyStarted, VerifyStatusParams, VerifyTick,
    VerifyTrial,
};
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use serde_json::Value;

use plan::{Plan, Trial, TrialKind};
use trial::{Pad, Read, Run};

/// The ticker's name.
pub const TICKER: &str = "superbounce.verify";
/// Where an object's origin is in its body: the point a teleport places
/// and the frame of every spot's heights
/// ([`crate::superbounce_mode::OBJECT_POSITION`]).
const OBJECT_POSITION: u32 = crate::superbounce_mode::OBJECT_POSITION;
/// The most trace rows kept.
pub const MAX_TRACE: usize = 2_000;

static CALLS: OnceLock<Shared> = OnceLock::new();
static JOB: Mutex<Option<Job>> = Mutex::new(None);
static NEXT_JOB: AtomicU32 = AtomicU32::new(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The game's own calls queue, for the trials' teleports (agent_glue).
pub fn set_calls(q: Shared) {
    let _ = CALLS.set(q);
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

/// The game's calls queue, once agent_glue has set it (`player.goto`'s
/// turn uses it too).
pub(crate) fn calls() -> Option<&'static Shared> {
    CALLS.get()
}

// ---- the seat ----

/// Who the trials drive.
#[derive(Clone, Debug, PartialEq)]
pub enum Seat {
    /// `player.<index>`, through the first controller's pad.
    Pad { index: u32 },
    /// A brain-driven bot, by its phantom handle.
    Bot { handle: u32, name: String },
}

/// A bot as [`parse_seat`] matches it: handle, gamertag, player index.
pub type BotSeat = (u32, String, Option<u32>);

/// The seat `s` names: a bot by gamertag (any case), handle or its
/// `player.<n>`, else `player.<n>` through the pad.
pub fn parse_seat(s: &str, bots: &[BotSeat]) -> Result<Seat, String> {
    let s = s.trim();
    let player = s
        .strip_prefix("player.")
        .and_then(|n| n.parse::<u32>().ok());
    let handle = s.parse::<u32>().ok();
    if let Some((h, name, _)) = bots.iter().find(|(h, name, p)| {
        name.eq_ignore_ascii_case(s) || Some(*h) == handle || (player.is_some() && *p == player)
    }) {
        return Ok(Seat::Bot {
            handle: *h,
            name: name.clone(),
        });
    }
    match player {
        Some(index) => Ok(Seat::Pad { index }),
        None => Err(format!(
            "seat {s:?}: player.<n> or a brain-driven bot's name or handle (bots.stats)"
        )),
    }
}

fn bot_seats() -> Vec<BotSeat> {
    bots::phantom::global()
        .list()
        .into_iter()
        .map(|b| (b.handle.0, b.identity.gamertag, b.player))
        .collect()
}

impl Seat {
    pub fn name(&self) -> String {
        match self {
            Seat::Pad { index } => format!("player.{index}"),
            Seat::Bot { name, .. } => name.clone(),
        }
    }

    /// Its player's index now; `None` for a bot that has not joined or has
    /// gone.
    fn index(&self) -> Option<u32> {
        match self {
            Seat::Pad { index } => Some(*index),
            Seat::Bot { handle, .. } => bots::phantom::global()
                .list()
                .into_iter()
                .find(|b| b.handle.0 == *handle)
                .and_then(|b| b.player),
        }
    }

    /// Holds `pad` (`None` lets go), the walk turned against `yaw`.
    fn hold(&self, pad: Option<Pad>, yaw: Option<f32>) -> bool {
        let stick = pad.map(|p| stick_for(p.walk, yaw.unwrap_or(0.0)));
        match self {
            Seat::Pad { .. } => {
                crate::superbounce_assist::set_verify_pad(pad.zip(stick).map(|(p, stick)| {
                    crate::superbounce_assist::VerifyPad {
                        stick,
                        crouch: p.crouch,
                        jump: p.jump,
                    }
                }))
            }
            Seat::Bot { handle, .. } => {
                let held = pad.zip(stick).map(|(p, stick)| {
                    let mut h = input::host::HostPadState::default();
                    h.left_stick = (stick[0], stick[1]);
                    h.buttons.set(bots::mind::CROUCH, p.crouch);
                    h.buttons.set(bots::mind::JUMP, p.jump);
                    h
                });
                bots::phantom::global().hold_pad(bots::phantom::PhantomHandle(*handle), held)
            }
        }
    }
}

/// A world walk (X and Y, length 0..1) as the left stick `[x (right),
/// y (forward)]` for a body facing `yaw`: the bots' `movement_stick`.
pub fn stick_for(walk: [f32; 2], yaw: f32) -> [f32; 2] {
    if !walk.iter().chain(&[yaw]).all(|v| v.is_finite()) {
        return [0.0; 2];
    }
    let (s, c) = yaw.sin_cos();
    let x = walk[0] * s - walk[1] * c;
    let y = walk[0] * c + walk[1] * s;
    let n = x.hypot(y);
    if n > 1.0 { [x / n, y / n] } else { [x, y] }
}

// ---- reading and writing the game ----

/// What seat `index` reads now, and its unit's handle (`player.goto`
/// reads `player.0` with it too).
pub(crate) fn read_seat(m: &dyn GuestMemory, index: u32) -> Option<(Read, u32)> {
    use live_telemetry::layout::{
        BIPED_FLAGS, BODY_VELOCITY, UNIT_AIM, UNIT_CROUCH, UNIT_FLAG_CROUCH, UNIT_FLAGS,
    };
    let unit = agent::sim::state::unit_handle(m, index).ok()?;
    let p = agent::sim::state::player(m, index).ok()?;
    let base = agent::sim::state::unit(m, p).ok()??.addr;
    let f = |at: u32| {
        m.u32(base.wrapping_add(at))
            .ok()
            .map(f32::from_bits)
            .filter(|v| v.is_finite())
    };
    let v3 = |at: u32| Some([f(at)?, f(at + 4)?, f(at + 8)?]);
    let pos = v3(OBJECT_POSITION);
    let yaw = match (f(UNIT_AIM), f(UNIT_AIM + 4)) {
        (Some(x), Some(y)) if x != 0.0 || y != 0.0 => Some(y.atan2(x)),
        _ => None,
    };
    let read = Read {
        pos,
        vel: v3(BODY_VELOCITY),
        yaw,
        airborne: m
            .u16(base.wrapping_add(BIPED_FLAGS))
            .is_ok_and(|flags| flags & 1 != 0),
        request: m
            .u32(base.wrapping_add(UNIT_FLAGS))
            .is_ok_and(|flags| flags & UNIT_FLAG_CROUCH != 0),
        crouch: f(UNIT_CROUCH),
        standing_fits: pos.and_then(crate::superbounce_mode::standing_fits_at),
        word: live_telemetry::havok_agents::fixed_pair_word(m, unit)
            .ok()
            .flatten(),
    };
    Some((read, unit))
}

/// Writes the unit's level pair word; `false` with no single pair.
fn write_word(m: &mut dyn GuestMemory, unit: u32, word: u32) -> bool {
    use live_telemetry::havok_agents as hk;
    let Ok(Some(entry)) = hk::fixed_pair(&*m, unit) else {
        return false;
    };
    matches!(hk::set_pair_word(m, unit, entry, word), Ok(Some(_)))
}

fn facing(yaw: f32) -> [f32; 3] {
    let (s, c) = yaw.sin_cos();
    [c, s, 0.0]
}

// ---- the job ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    fn name(self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }
}

/// A route trial: the bot ordered to the trial's `start`, its order watched.
struct RouteRun {
    index: usize,
    trial: Trial,
    asked: bool,
    ticks: u32,
    view: Option<bots::mind::OrderView>,
    note: Option<String>,
}

impl RouteRun {
    fn outcome(&self) -> VerifyTrial {
        let mut v = VerifyTrial {
            index: self.index,
            spot: self.trial.spot,
            start: self.trial.start,
            note: self.note.clone(),
            ..VerifyTrial::default()
        };
        if let Some(o) = &self.view {
            let bounced = o.landing.is_some_and(|t| t.bounced());
            v.ok = o.state == "done" && bounced;
            let step = o.steps.get(o.step).copied().unwrap_or("-");
            v.route = Some(match (o.state, o.error) {
                ("done", _) if bounced => "done".to_string(),
                ("done", _) => "done_no_bounce".to_string(),
                (s, Some(e)) => format!("{s} at {step}: {e}"),
                (s, None) => format!("{s} at {step}/{}", o.phase),
            });
            if let (Some(t), Some(site)) = (o.landing, o.site) {
                v.speed_in = Some(t.speed_in);
                v.vz_out = Some(t.vz_out);
                v.landed_at = Some(t.at);
                v.landing = Some(t.result(site.need).into());
            }
            v.steps = o
                .log
                .iter()
                .map(|l| format!("{} {} {}", l.tick, l.step, l.what))
                .collect();
        } else if v.note.is_none() {
            v.note = Some("the bot never took the order".into());
        }
        v
    }
}

enum Running {
    Body {
        run: Run,
        ticket: Option<PendingCall>,
    },
    Route(RouteRun),
}

struct Job {
    id: u32,
    plan: Plan,
    seat: Seat,
    map: String,
    state: JobState,
    next: usize,
    running: Option<Running>,
    outcomes: Vec<VerifyTrial>,
    trace: Vec<VerifyTick>,
    saved: Option<String>,
    error: Option<String>,
    notes: Vec<String>,
}

impl Job {
    fn planned(&self) -> Vec<usize> {
        let mut n = vec![0; self.plan.targets.len()];
        for t in &self.plan.trials {
            n[t.spot] += 1;
        }
        n
    }

    fn report(&self, brief: bool) -> VerifyReport {
        let kind = self.plan.config.kind;
        let results = summary::results(kind, &self.plan.targets, &self.planned(), &self.outcomes);
        let mut lines = summary::lines(kind, &self.plan.targets, &results);
        lines.extend(self.notes.iter().cloned());
        let current = match &self.running {
            Some(Running::Body { run, .. }) => Some(format!("trial {} {}", run.index, run.phase())),
            Some(Running::Route(r)) => Some(format!(
                "trial {} {}",
                r.index,
                r.view.as_ref().map_or("asked", |v| v.state)
            )),
            None => None,
        };
        VerifyReport {
            state: self.state.name().into(),
            job: Some(self.id),
            kind: kind.name().into(),
            seat: self.seat.name(),
            map: self.map.clone(),
            done: self.outcomes.len(),
            total: self.plan.trials.len(),
            current,
            spots: self.plan.targets.iter().map(|t| t.view()).collect(),
            results,
            trials: if brief {
                Vec::new()
            } else {
                self.outcomes.clone()
            },
            trace: self.trace.clone(),
            lines,
            saved: self.saved.clone(),
            error: self.error.clone(),
        }
    }

    /// Lets go of the seat.
    fn release(&self) {
        self.seat.hold(None, None);
    }

    fn end(&mut self, state: JobState) {
        if let Some(r) = self.running.take() {
            let out = match r {
                Running::Body { mut run, .. } => {
                    run.abort("the batch ended");
                    run.outcome()
                }
                Running::Route(r) => r.outcome(),
            };
            // A cut-off trial says nothing about its spot.
            if state == JobState::Done {
                self.outcomes.push(out);
            }
        }
        self.release();
        self.state = state;
        if state == JobState::Done && self.plan.config.save {
            self.save();
        }
    }

    fn save(&mut self) {
        let kind = self.plan.config.kind;
        let results = summary::results(kind, &self.plan.targets, &self.planned(), &self.outcomes);
        let entries = summary::entries(kind, &self.plan.targets, &results, &self.outcomes);
        if entries.is_empty() {
            return;
        }
        match bots::spots::store_verified(&self.map, &entries) {
            Ok(Some(path)) => self.saved = Some(path.display().to_string()),
            Ok(None) => self.notes.push(
                "not saved: no bots data directory (the verdicts hold until the game ends)".into(),
            ),
            Err(e) => self.notes.push(format!("not saved: {e}")),
        }
    }

    /// One tick of the batch.
    fn tick(&mut self, ctx: &mut GameCtx<'_>) {
        if self.state != JobState::Running {
            return;
        }
        // A batch is about one map: a new one (or none) ends it.
        let map = ctx
            .read_guest(|m| crate::health::loaded_map_name(m).ok())
            .ok()
            .flatten();
        if map.as_deref() != Some(self.map.as_str()) {
            self.error = Some("the map changed or the game ended".into());
            self.end(JobState::Failed);
            return;
        }
        if self.running.is_none() {
            let Some(&trial) = self.plan.trials.get(self.next) else {
                self.end(JobState::Done);
                return;
            };
            let index = self.next;
            self.next += 1;
            let spot = self.plan.targets[trial.spot].spot;
            self.running = Some(match self.plan.config.kind {
                TrialKind::Route => Running::Route(RouteRun {
                    index,
                    trial,
                    asked: false,
                    ticks: 0,
                    view: None,
                    note: None,
                }),
                _ => Running::Body {
                    run: Run::new(index, trial, spot, &self.plan.config),
                    ticket: None,
                },
            });
        }
        let Some(mut running) = self.running.take() else {
            return;
        };
        let finished = match &mut running {
            Running::Body { run, ticket } => self.body_tick(ctx, run, ticket),
            Running::Route(r) => self.route_tick(r),
        };
        if finished {
            let out = match &running {
                Running::Body { run, .. } => run.outcome(),
                Running::Route(r) => r.outcome(),
            };
            self.outcomes.push(out);
            // Let go between trials: the next one starts from a clean pad.
            self.release();
        } else {
            self.running = Some(running);
        }
    }

    fn body_tick(
        &mut self,
        ctx: &mut GameCtx<'_>,
        run: &mut Run,
        ticket: &mut Option<PendingCall>,
    ) -> bool {
        let Some(index) = self.seat.index() else {
            run.abort("the seat has no player now");
            return true;
        };
        if let Some(t) = ticket.as_ref()
            && let Some(out) = t.outcome()
        {
            *ticket = None;
            run.teleported(out.map(|_| ()).map_err(|e| e.message));
        }
        let mut seen = None;
        ctx.reader.with_guest(&mut |m| seen = read_seat(&*m, index));
        let (read, unit) = seen.unwrap_or((Read::default(), 0));
        let act = run.step(&read);
        if self.plan.config.trace == Some(run.index) && self.trace.len() < MAX_TRACE {
            self.trace.push(run.trace(&read, &act));
        }
        if act.word.is_some() || act.teleport.is_some() {
            let mut handle = None;
            ctx.reader.with_guest(&mut |m| {
                if let Some(w) = act.word
                    && unit != 0
                {
                    write_word(m, unit, w);
                }
                if act.teleport.is_some() {
                    handle = agent::sim::state::player_handle(&*m, index).ok();
                }
            });
            if let Some((to, yaw)) = act.teleport {
                let submitted = match (CALLS.get(), handle) {
                    (Some(q), Some(player)) => calls::submit(
                        q,
                        Call::Teleport {
                            player,
                            pos: Some(to),
                            facing: Some(facing(yaw)),
                        },
                    ),
                    (None, _) => Err("the agent-calls feature is off".into()),
                    (_, None) => Err(format!("no player.{index}")),
                };
                match submitted {
                    Ok(t) => *ticket = Some(t),
                    Err(e) => run.teleported(Err(e)),
                }
            }
        }
        if run.done() {
            return true;
        }
        // While a teleport is waited for, the pad rests.
        let pad = if run.waiting() {
            Pad::default()
        } else {
            act.pad
        };
        self.seat.hold(Some(pad), read.yaw);
        false
    }

    fn route_tick(&mut self, r: &mut RouteRun) -> bool {
        let Seat::Bot { handle, .. } = &self.seat else {
            r.note = Some("a route needs a bot seat".into());
            return true;
        };
        let phantoms = bots::phantom::global();
        let h = bots::phantom::PhantomHandle(*handle);
        if !r.asked {
            r.asked = true;
            phantoms.hold_pad(h, None);
            if !phantoms.order_bounce(h, r.trial.start, self.plan.config.mode) {
                r.note = Some("the bot is not brain-driven".into());
                return true;
            }
            return false;
        }
        r.ticks += 1;
        let Some(view) = phantoms.order_view(h) else {
            r.note = Some("the bot has gone".into());
            return true;
        };
        // The first ticks can still show the last order's end.
        let settled = r.ticks > 2 && matches!(view.state, "done" | "failed" | "none");
        r.view = Some(view);
        if settled {
            return true;
        }
        if r.ticks >= self.plan.config.route {
            r.note = Some(format!("no end in {} ticks", self.plan.config.route));
            return true;
        }
        false
    }
}

// ---- the methods ----

fn params<T: serde::de::DeserializeOwned + Default>(v: Value, method: &str) -> Result<T, RpcError> {
    if v.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(v).map_err(|e| invalid(format!("{method}: {e}")))
}

/// `superbounce.verify`.
fn verify(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    if v.is_null() {
        return Err(invalid(
            "superbounce.verify: kind is required (crouch, drop or route)",
        ));
    }
    let p: VerifyParams = params(v, wire::VERIFY)?;
    if let Some(j) = lock(&JOB).as_ref()
        && j.state == JobState::Running
        && !p.dry_run
    {
        return Err(invalid(format!(
            "batch {} is running; superbounce.verify_cancel stops it",
            j.id
        )));
    }
    let loaded = ctx
        .read_guest(|m| crate::health::loaded_map_name(m).ok())?
        .filter(|n| !n.is_empty())
        .ok_or_else(|| unavailable("no map is loaded"))?;
    let scanned = if p.points.is_some() {
        Vec::new()
    } else {
        let (map, spots) = crate::superbounce_mode::scanned_spots();
        if map.as_deref() != Some(loaded.as_str()) || spots.is_empty() {
            return Err(unavailable(
                "no spot scan of this map: turn the mode on (play a game whose script requires hud.superbounce) and \
                 wait for superbounce.state's scan.status to read ready",
            ));
        }
        spots.to_vec()
    };
    let plan = plan::plan(&p, &scanned).map_err(invalid)?;
    let seat =
        parse_seat(p.seat.as_deref().unwrap_or("player.0"), &bot_seats()).map_err(invalid)?;
    match (&seat, plan.config.kind) {
        (Seat::Pad { .. }, TrialKind::Route) => {
            return Err(invalid("a route trial needs a brain-driven bot seat"));
        }
        (Seat::Pad { .. }, _) if crate::player_goto::running() => {
            return Err(invalid(
                "player.goto is driving player.0; player.goto_cancel stops it",
            ));
        }
        (Seat::Pad { .. }, _) if !crate::superbounce_assist::tapped() => {
            return Err(unavailable(
                "the pad tap is not in place (the superbounce-assist feature is off), so \
                 player.0 can't be driven; use a bot seat",
            ));
        }
        (Seat::Bot { name, .. }, _) if !seat.hold(None, None) => {
            return Err(invalid(format!("{name} is not brain-driven")));
        }
        _ => {}
    }
    if plan.config.kind != TrialKind::Route && CALLS.get().is_none_or(|q| !calls::lock(q).armed()) {
        return Err(unavailable(
            "the game's teleport isn't reachable (the agent-calls feature is off)",
        ));
    }
    if let Some(index) = match &seat {
        Seat::Pad { index } => Some(*index),
        Seat::Bot { .. } => seat.index(),
    } {
        ctx.read_guest(|m| agent::sim::state::unit_handle(m, index))??;
    } else {
        return Err(unavailable(format!(
            "{} has not joined the game",
            seat.name()
        )));
    }
    let id = if p.dry_run {
        0
    } else {
        NEXT_JOB.fetch_add(1, Ordering::Relaxed)
    };
    let started = VerifyStarted {
        job: id,
        kind: plan.config.kind.name().into(),
        seat: seat.name(),
        map: loaded.clone(),
        spots: plan.targets.iter().map(|t| t.view()).collect(),
        trials: plan.trials.iter().map(|t| t.view()).collect(),
        estimated_ticks: plan::estimate(&plan),
        dry_run: p.dry_run,
    };
    if !p.dry_run {
        *lock(&JOB) = Some(Job {
            id,
            plan,
            seat,
            map: loaded,
            state: JobState::Running,
            next: 0,
            running: None,
            outcomes: Vec::new(),
            trace: Vec::new(),
            saved: None,
            error: None,
            notes: Vec::new(),
        });
    }
    Ok(serde_json::to_value(started).expect("verify serialises"))
}

fn report_now(brief: bool) -> VerifyReport {
    match lock(&JOB).as_ref() {
        Some(j) => j.report(brief),
        None => VerifyReport {
            state: "none".into(),
            ..VerifyReport::default()
        },
    }
}

/// Whether a batch is running (its trials' bounces are recorded as ghost
/// runs, `superbounce_ghost`).
pub(crate) fn running() -> bool {
    lock(&JOB)
        .as_ref()
        .is_some_and(|j| j.state == JobState::Running)
}

/// Whether a running batch drives `player.0`'s pad (`player.goto` waits
/// for it).
pub(crate) fn drives_pad() -> bool {
    lock(&JOB)
        .as_ref()
        .is_some_and(|j| j.state == JobState::Running && matches!(j.seat, Seat::Pad { .. }))
}

/// `superbounce.verify_status`.
fn verify_status(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let p: VerifyStatusParams = params(v, wire::VERIFY_STATUS)?;
    let brief = p.brief;
    let reply = move || serde_json::to_value(report_now(brief)).expect("report serialises");
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

/// `superbounce.verify_cancel`.
fn verify_cancel(v: Value) -> Result<Value, RpcError> {
    if !(v.is_null() || v.as_object().is_some_and(|o| o.is_empty())) {
        return Err(invalid("superbounce.verify_cancel takes no parameters"));
    }
    let mut j = lock(&JOB);
    let report = match j.as_mut() {
        Some(job) => {
            if job.state == JobState::Running {
                job.end(JobState::Cancelled);
            }
            job.report(false)
        }
        None => VerifyReport {
            state: "none".into(),
            ..VerifyReport::default()
        },
    };
    Ok(serde_json::to_value(report).expect("report serialises"))
}

/// The ticker: one tick of the batch, if one is running.
fn on_tick(ctx: &mut GameCtx<'_>) {
    let mut j = lock(&JOB);
    if let Some(job) = j.as_mut() {
        job.tick(ctx);
    }
}

/// Adds the methods and the ticker to `agent`.
pub fn install(agent: &agent::Agent) {
    agent.add_game_method(wire::VERIFY, Access::Act, verify);
    agent.add_game_method(wire::VERIFY_STATUS, Access::Read, verify_status);
    agent.add_method(wire::VERIFY_CANCEL, Access::Act, verify_cancel);
    agent.add_ticker(TICKER, on_tick);
}
