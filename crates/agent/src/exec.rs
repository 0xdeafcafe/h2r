//! The executor: requests queue here from connection threads and are
//! answered on the game's thread, at a tick boundary when the game is
//! ticking, between slices when it isn't.
//!
//! Nothing here touches guest memory except through a [`Reader`] the game
//! thread passes in, so every answer is read on the game's own thread.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use agent_proto::{
    At, Boundary, CountParams, Counters, Counts, Event, EventKind, Features, GroupBy, Kind,
    Notification, OBJECT_FIELDS, Object, ObjectId, ObjectParams, ObjectResult, PROTOCOL,
    PauseParams, Phase, QueryParams, QueryResult, Request, Response, RpcError, Status, StepParams,
    SubscribeParams, Until, WaitParams, Waited, code, method,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::reader::{GameStatus, Reader};

/// One connection.
pub type ConnId = u64;

/// With no tick for this long the game counts as loading, and world reads
/// are answered between slices instead of waiting for a tick.
pub const TICK_STALE: Duration = Duration::from_millis(500);
/// How long a paused game sleeps between looking at its queue.
const PAUSE_POLL: Duration = Duration::from_millis(20);
const PAUSE_DEFAULT: Duration = Duration::from_secs(60);
const PAUSE_MAX: Duration = Duration::from_secs(600);
const WAIT_DEFAULT: Duration = Duration::from_secs(30);
const WAIT_MAX: Duration = Duration::from_secs(600);
const STEP_MAX: u64 = 1800;
const SLICE_REFRESH: Duration = Duration::from_millis(20);

/// The executor's handle. Cheap to clone; the game thread and every
/// connection share one.
#[derive(Clone, Default)]
pub struct Agent {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    pending: VecDeque<(ConnId, Request)>,
    waits: Vec<Waiting>,
    conns: BTreeMap<ConnId, Conn>,
    next_conn: ConnId,
    controller: Option<ConnId>,
    /// Paused until this instant (then it resumes by itself).
    paused: Option<Instant>,
    /// How long the current pause may last, re-armed after each step.
    pause_len: Duration,
    /// Ticks left to run before pausing again (`session.step`).
    stepping: Option<u64>,
    /// When the status was last read between slices.
    slice_refresh: Option<Instant>,
    ticks: u64,
    presents: u64,
    game: GameStatus,
    phase: Option<Phase>,
    cinematic: bool,
    last_tick: Option<Instant>,
    /// The boundary the current answers are read at.
    boundary: Option<Boundary>,
    plan: Vec<String>,
    shutdown: bool,
}

struct Conn {
    tx: Sender<String>,
    events: BTreeSet<EventKind>,
}

struct Waiting {
    conn: ConnId,
    id: Value,
    until: Until,
    from_ticks: u64,
    tick_limit: Option<u64>,
    deadline: Instant,
}

/// Where the game thread calls from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Where {
    Tick,
    Slice,
}

impl Agent {
    pub fn new() -> Agent {
        Agent::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new connection; lines for it (responses and events) go to `tx`.
    pub fn connect(&self, tx: Sender<String>) -> ConnId {
        let mut s = self.lock();
        s.next_conn += 1;
        let id = s.next_conn;
        s.conns.insert(
            id,
            Conn {
                tx,
                events: BTreeSet::new(),
            },
        );
        id
    }

    /// The connection is gone: drop its waits, and if it held control,
    /// release it (a pause it left resumes).
    pub fn disconnect(&self, conn: ConnId) {
        let mut s = self.lock();
        s.conns.remove(&conn);
        s.waits.retain(|w| w.conn != conn);
        s.pending.retain(|(c, _)| *c != conn);
        if s.controller == Some(conn) {
            s.controller = None;
            s.resume("released");
        }
        drop(s);
        self.shared.wake.notify_all();
    }

    /// A line from a connection. Parse errors are answered at once;
    /// requests queue for the game thread.
    pub fn submit(&self, conn: ConnId, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let mut s = self.lock();
        match serde_json::from_str::<Request>(line) {
            Ok(r) if r.jsonrpc == "2.0" => s.pending.push_back((conn, r)),
            Ok(r) => {
                let id = r.id.unwrap_or(Value::Null);
                s.send(
                    conn,
                    Response::err(
                        id,
                        RpcError::new(code::INVALID_REQUEST, "jsonrpc must be \"2.0\""),
                    ),
                );
            }
            Err(e) => s.send(
                conn,
                Response::err(Value::Null, RpcError::new(code::PARSE, e.to_string())),
            ),
        }
        drop(s);
        self.shared.wake.notify_all();
    }

    /// The feature plan's text, for `features.list`.
    pub fn set_plan(&self, lines: Vec<String>) {
        self.lock().plan = lines;
    }

    /// HaloScript `cinematic_start` / `cinematic_stop` ran.
    pub fn set_cinematic(&self, on: bool) {
        self.lock().cinematic = on;
    }

    /// The game is exiting: fail what's queued and waiting, and let a
    /// paused game go.
    pub fn shutdown(&self) {
        let mut s = self.lock();
        s.shutdown = true;
        s.paused = None;
        s.stepping = None;
        for (conn, r) in std::mem::take(&mut s.pending) {
            if let Some(id) = r.id {
                s.send(
                    conn,
                    Response::err(id, RpcError::new(code::SHUTDOWN, "the game is exiting")),
                );
            }
        }
        for w in std::mem::take(&mut s.waits) {
            s.send(
                w.conn,
                Response::err(w.id, RpcError::new(code::SHUTDOWN, "the game is exiting")),
            );
        }
        drop(s);
        self.shared.wake.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.lock().paused.is_some()
    }

    /// After each game tick, on the game's thread. Answers everything
    /// queued, settles waits, and blocks here while paused, answering
    /// requests as they arrive.
    pub fn on_tick(&self, reader: &mut dyn Reader) {
        let mut s = self.lock();
        if s.shutdown {
            return;
        }
        s.ticks += 1;
        s.last_tick = Some(Instant::now());
        s.refresh(reader, Where::Tick);
        match s.stepping {
            Some(left) if left <= 1 => {
                s.stepping = None;
                s.paused = Some(Instant::now() + s.pause_len);
                let at = s.at();
                s.emit(Event::Paused { at });
            }
            Some(left) => s.stepping = Some(left - 1),
            None => {}
        }
        s.serve(reader, Where::Tick);
        s.settle_waits();
        while let Some(until) = s.paused {
            if s.shutdown {
                break;
            }
            if Instant::now() >= until {
                s.resume("timeout");
                break;
            }
            s = self
                .shared
                .wake
                .wait_timeout(s, PAUSE_POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            s.serve(reader, Where::Tick);
            s.settle_waits();
        }
    }

    /// Between scheduler slices, on the game's thread. Answers what doesn't
    /// need a tick; world reads too once the game has stopped ticking.
    pub fn between_slices(&self, reader: &mut dyn Reader, presents: u64) {
        let mut s = self.lock();
        if s.shutdown {
            return;
        }
        s.presents = presents;
        // While ticking, the tick boundary does the reading; between
        // slices only queued requests that don't need a tick, and wall
        // clock timeouts. Once ticks stop (loading), read here, at most
        // every SLICE_REFRESH, for waits and events.
        let due = s.slice_refresh.is_none_or(|t| t.elapsed() >= SLICE_REFRESH);
        let idle_work = !s.ticking() && due && (!s.waits.is_empty() || s.wants_events());
        if s.pending.is_empty() && !idle_work {
            if !s.waits.is_empty() {
                s.settle_waits();
            }
            return;
        }
        s.slice_refresh = Some(Instant::now());
        s.refresh(reader, Where::Slice);
        s.serve(reader, Where::Slice);
        s.settle_waits();
    }
}

impl State {
    fn send(&self, conn: ConnId, r: Response) {
        if let Some(c) = self.conns.get(&conn) {
            let _ =
                c.tx.send(serde_json::to_string(&r).expect("responses serialise"));
        }
    }

    fn wants_events(&self) -> bool {
        self.conns.values().any(|c| !c.events.is_empty())
    }

    fn emit(&self, e: Event) {
        let kind = e.kind();
        let line = serde_json::to_string(&Notification::event(e)).expect("events serialise");
        for c in self.conns.values().filter(|c| c.events.contains(&kind)) {
            let _ = c.tx.send(line.clone());
        }
    }

    fn ticking(&self) -> bool {
        self.last_tick.is_some_and(|t| t.elapsed() < TICK_STALE)
    }

    fn phase_now(&self) -> Phase {
        if !self.ticking() && self.paused.is_none() {
            return Phase::Loading;
        }
        if self.cinematic {
            return Phase::Cinematic;
        }
        match self.game.mode.as_deref() {
            Some("main_menu") => Phase::Menu,
            Some("campaign" | "multiplayer") => Phase::Gameplay,
            _ => Phase::Loading,
        }
    }

    fn at(&self) -> At {
        At {
            tick: self.game.tick,
            ticks: self.ticks,
            present: self.presents,
            map: self.game.map.clone(),
            mode: self.game.mode.clone(),
            bsp: self.game.bsp,
            phase: self.phase.unwrap_or(Phase::Loading),
            paused: self.paused.is_some(),
            boundary: self.boundary.unwrap_or(Boundary::Slice),
        }
    }

    /// Reads the game's status and emits what changed.
    fn refresh(&mut self, reader: &mut dyn Reader, where_: Where) -> At {
        self.boundary = Some(match where_ {
            Where::Tick => Boundary::Tick,
            Where::Slice => Boundary::Slice,
        });
        let old = std::mem::replace(&mut self.game, reader.status());
        if old.map.is_some() && old.map != self.game.map {
            // A cutscene doesn't outlive its map.
            self.cinematic = false;
        }
        let old_phase = self.phase;
        self.phase = Some(self.phase_now());
        let at = self.at();
        if old.map != self.game.map {
            self.emit(Event::Map {
                from: old.map,
                to: self.game.map.clone(),
                at: at.clone(),
            });
        }
        if old.bsp != self.game.bsp {
            self.emit(Event::Bsp {
                from: old.bsp,
                to: self.game.bsp,
                at: at.clone(),
            });
        }
        if let Some(from) = old_phase
            && Some(from) != self.phase
        {
            self.emit(Event::Phase {
                from,
                to: at.phase,
                at: at.clone(),
            });
        }
        at
    }

    fn resume(&mut self, why: &str) {
        if self.paused.take().is_some() || self.stepping.take().is_some() {
            let at = self.at();
            self.emit(Event::Resumed {
                at,
                why: why.into(),
            });
        }
    }

    /// Answers queued requests. Between slices, world reads stay queued
    /// while the game is still ticking, so they're read at a tick.
    fn serve(&mut self, reader: &mut dyn Reader, where_: Where) {
        let defer_world = where_ == Where::Slice && self.ticking();
        let mut keep = VecDeque::new();
        while let Some((conn, req)) = self.pending.pop_front() {
            if defer_world && req.method.starts_with("world.") {
                keep.push_back((conn, req));
                continue;
            }
            let id = req.id.clone();
            let out = self.handle(conn, req, reader);
            if let (Some(id), Some(out)) = (id, out) {
                let r = match out {
                    Ok(v) => Response::ok(id, v),
                    Err(e) => Response::err(id, e),
                };
                self.send(conn, r);
            }
        }
        self.pending = keep;
    }

    fn settle_waits(&mut self) {
        let now = Instant::now();
        let waits = std::mem::take(&mut self.waits);
        for w in waits {
            let passed = self.ticks - w.from_ticks;
            if self.holds(&w.until, passed) {
                let r = Waited {
                    at: self.at(),
                    ticks: passed,
                };
                self.send(w.conn, Response::ok(w.id, to_value(r)));
            } else if w.tick_limit.is_some_and(|n| passed >= n) || now >= w.deadline {
                let mut e = RpcError::new(code::TIMEOUT, "timed out before the condition held");
                e.data = Some(to_value(self.at()));
                self.send(w.conn, Response::err(w.id, e));
            } else {
                self.waits.push(w);
            }
        }
    }

    fn holds(&self, u: &Until, passed: u64) -> bool {
        u.tick
            .is_none_or(|t| self.game.tick.is_some_and(|now| now >= t))
            && u.ticks.is_none_or(|n| passed >= n)
            && u.present.is_none_or(|p| self.presents >= p)
            && u.phase.is_none_or(|p| self.phase == Some(p))
            && u.map
                .as_ref()
                .is_none_or(|m| self.game.map.as_ref() == Some(m))
            && u.paused.is_none_or(|p| self.paused.is_some() == p)
    }

    /// `None`: answered later (a wait).
    fn handle(
        &mut self,
        conn: ConnId,
        req: Request,
        reader: &mut dyn Reader,
    ) -> Option<Result<Value, RpcError>> {
        let m = req.method.as_str();
        if method::CONTROL.contains(&m) && self.controller != Some(conn) {
            return Some(Err(RpcError::new(
                code::NOT_CONTROLLER,
                "session.claim first: only the controlling connection may pause or step",
            )));
        }
        let p = req.params;
        Some(match m {
            method::STATUS => Ok(to_value(Status {
                at: self.at(),
                protocol: PROTOCOL,
                fps: timeline::phase::window_summary().map(|s| s.fps),
                controller: self.controller == Some(conn),
                claimed: self.controller.is_some(),
            })),
            method::METHODS => Ok(Value::from(method::ALL.to_vec())),
            method::CLAIM => match self.controller {
                Some(c) if c != conn => Err(RpcError::new(
                    code::NOT_CONTROLLER,
                    "another connection holds control",
                )),
                _ => {
                    self.controller = Some(conn);
                    Ok(serde_json::json!({ "at": self.at(), "controller": true }))
                }
            },
            method::RELEASE => {
                if self.controller == Some(conn) {
                    self.controller = None;
                    self.resume("released");
                }
                Ok(serde_json::json!({ "at": self.at(), "controller": false }))
            }
            method::PAUSE => params::<PauseParams>(p).map(|pp| {
                let max = pp
                    .max_ms
                    .map_or(PAUSE_DEFAULT, Duration::from_millis)
                    .min(PAUSE_MAX);
                if self.paused.is_none() {
                    self.pause_len = max;
                    self.paused = Some(Instant::now() + max);
                    self.stepping = None;
                    let at = self.at();
                    self.emit(Event::Paused { at });
                }
                serde_json::json!({ "at": self.at() })
            }),
            method::RESUME => {
                self.resume("request");
                Ok(serde_json::json!({ "at": self.at() }))
            }
            method::STEP => {
                let sp = match params::<StepParams>(p) {
                    Ok(sp) => sp,
                    Err(e) => return Some(Err(e)),
                };
                if !(1..=STEP_MAX).contains(&sp.ticks) {
                    return Some(Err(RpcError::new(
                        code::INVALID_PARAMS,
                        format!("ticks must be 1 to {STEP_MAX}"),
                    )));
                }
                if self.paused.take().is_none() {
                    return Some(Err(RpcError::new(
                        code::INVALID_REQUEST,
                        "session.pause first: step runs a paused game for some ticks",
                    )));
                }
                self.stepping = Some(sp.ticks);
                self.add_wait(
                    conn,
                    req.id?,
                    Until {
                        ticks: Some(sp.ticks),
                        paused: Some(true),
                        ..Default::default()
                    },
                    None,
                    WAIT_MAX,
                );
                return None;
            }
            method::WAIT => {
                let wp = match params::<WaitParams>(p) {
                    Ok(wp) => wp,
                    Err(e) => return Some(Err(e)),
                };
                if wp.until.is_empty() {
                    return Some(Err(RpcError::new(
                        code::INVALID_PARAMS,
                        "until needs at least one condition",
                    )));
                }
                let timeout = wp
                    .timeout_ms
                    .map_or(WAIT_DEFAULT, Duration::from_millis)
                    .min(WAIT_MAX);
                self.add_wait(conn, req.id?, wp.until, wp.timeout_ticks, timeout);
                // Already true: settled before the next tick.
                self.settle_waits();
                return None;
            }
            method::SUBSCRIBE => params::<SubscribeParams>(p).map(|sp| {
                if let Some(c) = self.conns.get_mut(&conn) {
                    c.events = sp.events.into_iter().collect();
                }
                serde_json::json!({ "at": self.at() })
            }),
            method::WORLD_COUNT => {
                params::<CountParams>(p).and_then(|cp| self.count(reader, cp).map(to_value))
            }
            method::WORLD_QUERY => {
                params::<QueryParams>(p).and_then(|qp| self.query(reader, qp).map(to_value))
            }
            method::WORLD_OBJECT => {
                params::<ObjectParams>(p).and_then(|op| self.object(reader, op).map(to_value))
            }
            method::PROFILE_COUNTERS => self.counters().map(to_value),
            method::FEATURES => Ok(to_value(Features {
                at: self.at(),
                plan: self.plan.clone(),
            })),
            other => Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("no method {other}; session.methods lists them"),
            )),
        })
    }

    fn add_wait(
        &mut self,
        conn: ConnId,
        id: Value,
        until: Until,
        tick_limit: Option<u64>,
        timeout: Duration,
    ) {
        self.waits.push(Waiting {
            conn,
            id,
            until,
            from_ticks: self.ticks,
            tick_limit,
            deadline: Instant::now() + timeout,
        });
    }

    fn objects(&self, reader: &mut dyn Reader) -> Result<Vec<Object>, RpcError> {
        reader.objects().map_err(|e| {
            let mut err = RpcError::new(code::UNAVAILABLE, format!("reading the world: {e}"));
            err.data = Some(to_value(self.at()));
            err
        })
    }

    fn count(&self, reader: &mut dyn Reader, p: CountParams) -> Result<Counts, RpcError> {
        let objects = self.objects(reader)?;
        let mut groups = BTreeMap::new();
        for o in &objects {
            let key = match p.group_by {
                GroupBy::Kind => o.kind.as_str().to_owned(),
                GroupBy::Team => o.team.map_or("none".into(), |t| t.to_string()),
            };
            *groups.entry(key).or_insert(0) += 1;
        }
        Ok(Counts {
            at: self.at(),
            total: objects.len(),
            groups,
        })
    }

    fn query(&self, reader: &mut dyn Reader, p: QueryParams) -> Result<QueryResult, RpcError> {
        if let Some(f) = &p.fields
            && let Some(bad) = f.iter().find(|f| !OBJECT_FIELDS.contains(&f.as_str()))
        {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("no field {bad}; fields are {}", OBJECT_FIELDS.join(", ")),
            ));
        }
        let objects = self.objects(reader)?;
        let page = query(objects, &p)?;
        Ok(QueryResult {
            at: self.at(),
            total: page.total,
            offset: page.offset,
            objects: page.objects,
            more: page.more,
        })
    }

    fn object(&self, reader: &mut dyn Reader, p: ObjectParams) -> Result<ObjectResult, RpcError> {
        if ObjectId::parse(&p.id).is_none() {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("{} is not an id (player.N or kind.xxxxxxxx)", p.id),
            ));
        }
        match self.objects(reader)?.into_iter().find(|o| o.id == p.id) {
            Some(object) => Ok(ObjectResult {
                at: self.at(),
                object,
            }),
            None => {
                let mut e = RpcError::new(code::GONE, format!("{} is not in the world", p.id));
                e.data = Some(to_value(self.at()));
                Err(e)
            }
        }
    }

    fn counters(&self) -> Result<Counters, RpcError> {
        let s = timeline::phase::window_summary().ok_or_else(|| {
            RpcError::new(code::UNAVAILABLE, "no frames in the frame-stats window yet")
        })?;
        let phase_ms = timeline::phase::Phase::ALL
            .iter()
            .map(|p| (p.label().to_owned(), round(s.phase_ms[*p as usize])))
            .collect();
        let counts = timeline::phase::Counter::ALL
            .iter()
            .map(|c| (c.label().to_owned(), round(s.count_mean[*c as usize])))
            .collect();
        Ok(Counters {
            at: self.at(),
            frames: s.frames,
            fps: round(s.fps),
            p50_ms: round(s.p50_ms),
            p99_ms: round(s.p99_ms),
            max_ms: round(s.max_ms),
            phase_ms,
            counts,
        })
    }
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn to_value<T: Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("results serialise")
}

/// Missing params read as `{}`, so methods whose fields are all optional
/// take none.
fn params<T: DeserializeOwned>(p: Value) -> Result<T, RpcError> {
    let p = if p.is_null() {
        serde_json::json!({})
    } else {
        p
    };
    serde_json::from_value(p).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f64>()
        .sqrt()
}

/// A page of a query, before the stamp.
#[derive(Debug, PartialEq)]
pub struct Page {
    pub total: usize,
    pub offset: usize,
    pub objects: Vec<Value>,
    pub more: bool,
}

/// Filters, sorts and pages `objects`.
pub fn query(objects: Vec<Object>, p: &QueryParams) -> Result<Page, RpcError> {
    let limit = p
        .limit
        .unwrap_or(QueryParams::DEFAULT_LIMIT)
        .clamp(1, QueryParams::MAX_LIMIT);
    let offset = p.offset.unwrap_or(0);
    let center = match &p.near {
        None => None,
        Some(n) => Some(match (n.point, n.player) {
            (Some(pt), None) => pt,
            (None, Some(i)) => {
                let id = ObjectId {
                    kind: Kind::Player,
                    key: i,
                }
                .to_string();
                objects
                    .iter()
                    .find(|o| o.id == id)
                    .and_then(|o| o.pos)
                    .ok_or_else(|| {
                        RpcError::new(code::GONE, format!("{id} has no unit in the world"))
                    })?
            }
            _ => {
                return Err(RpcError::new(
                    code::INVALID_PARAMS,
                    "near needs exactly one of point or player",
                ));
            }
        }),
    };
    let mut hits: Vec<Object> = objects
        .into_iter()
        .filter(|o| p.kind.as_ref().is_none_or(|k| k.contains(&o.kind)))
        .collect();
    if let (Some(c), Some(n)) = (center, &p.near) {
        hits = hits
            .into_iter()
            .filter_map(|mut o| {
                let d = dist(o.pos?, c);
                (d <= n.radius).then(|| {
                    o.distance = Some(round(d));
                    o
                })
            })
            .collect();
        hits.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
    }
    let total = hits.len();
    let page: Vec<Value> = hits
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|o| trim(to_value(o), p.fields.as_deref()))
        .collect();
    Ok(Page {
        total,
        offset,
        more: offset + page.len() < total,
        objects: page,
    })
}

fn trim(v: Value, fields: Option<&[String]>) -> Value {
    let (Value::Object(mut m), Some(fields)) = (v.clone(), fields) else {
        return v;
    };
    m.retain(|k, _| k == "id" || k == "kind" || fields.iter().any(|f| f == k));
    Value::Object(m)
}

#[cfg(test)]
mod tests;
