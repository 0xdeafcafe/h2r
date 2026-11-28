//! The executor: requests queue here from connection threads and are
//! answered on the game's thread, at a tick boundary when the game is
//! ticking, between slices when it isn't.
//!
//! Nothing here touches guest memory except through a [`Reader`] the game
//! thread passes in, so every answer is read on the game's own thread.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

use crate::capture::{self, FrameSource, Sampler, TickMark, TimelineFrames};
use crate::loads::{LoadTracker, TimelineTotals, TotalsSource};
use crate::tap::{self as input, AgentTap, Chain, Input};
use crate::trace::{self, Recorders};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_proto::{
    At, Boundary, Capture, CaptureParams, CountParams, Counters, Counts, Event, EventKind,
    Features, GroupBy, Kind, LoadParams, Loads, Notification, OBJECT_FIELDS, Object, ObjectId,
    ObjectParams, ObjectResult, PROTOCOL, PauseParams, Phase, QueryParams, QueryResult, Request,
    Response, RpcError, Status, StepParams, SubscribeParams, Until, WaitParams, Waited, code,
    method,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::reader::{GameStatus, Reader, TagNames};

mod mirror;
mod picture;
mod poll_theater;
mod snap;
pub use snap::{SnapshotMachine, Snapshots};

/// One connection.
pub type ConnId = u64;

/// Who may call a registered method ([`Agent::add_method`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Any connection: it only reads.
    Read,
    /// Any connection while none holds control, else only the controller:
    /// it acts on the game (pad input, the profiler), and a one-shot
    /// `h2 agent` call should not need `session.claim` first.
    Act,
    /// Only the connection that holds control through `session.claim`.
    Claimed,
}

impl Access {
    fn check(self, controller: Option<ConnId>, conn: ConnId) -> Result<(), RpcError> {
        let message = match self {
            Self::Act | Self::Claimed if controller.is_some_and(|c| c != conn) => {
                "another connection holds control"
            }
            Self::Claimed if controller.is_none() => "session.claim first on this connection",
            _ => return Ok(()),
        };
        Err(RpcError::new(code::NOT_CONTROLLER, message))
    }
}

/// A method the game's host answers ([`Agent::add_method`]): params in,
/// result or error out. It runs on the game's thread with the agent's
/// state locked, so it must be quick and must not call the [`Agent`].
pub type HostMethod = Box<dyn FnMut(Value) -> Result<Value, RpcError> + Send>;

/// A long-running host read with no guest access, answered on its connection thread.
type ConnectionRead = Arc<dyn Fn(Value) -> Result<Value, RpcError> + Send + Sync>;

/// A method that reads (or writes) the game at the boundary it is answered
/// at ([`Agent::add_game_method`]): the `sim.*` methods. Like `world.*`,
/// it waits for a tick while the game ticks.
pub type GameMethod = Box<dyn FnMut(Value, &mut GameCtx<'_>) -> Result<Value, RpcError> + Send>;

/// Tells which external Live sessions refuse edits: `Some(why)` to refuse.
/// A Live custom game this console hosts admits them, as a System Link
/// host does; matchmaking, a client and an unreadable session do not.
pub type LiveEditProbe = fn(&dyn guest_layout::GuestMemory) -> Option<&'static str>;

/// Why edits are refused on an external Live session, if they are. With a
/// [`LiveEditProbe`] it decides for a Live game; without one, any session
/// the `live_session` probe calls external Live is refused.
pub(crate) fn live_edit_refusal(
    live_session: Option<fn(&dyn guest_layout::GuestMemory) -> bool>,
    live_edit: Option<LiveEditProbe>,
    m: &dyn guest_layout::GuestMemory,
) -> Option<&'static str> {
    match live_edit {
        Some(probe) => probe(m),
        None => live_session
            .is_some_and(|probe| probe(m))
            .then_some("external Live session"),
    }
}

/// What a [`GameMethod`] gets besides its params.
pub struct GameCtx<'a> {
    /// The connection owning this request and any input it queues.
    pub conn: ConnId,
    pub(crate) epoch: u64,
    pub(crate) map_epoch: u64,
    /// A queued edit reads the current epoch without taking the executor lock.
    map_lifetime: &'a mut Option<Arc<AtomicU64>>,
    pub(crate) live_session: Option<fn(&dyn guest_layout::GuestMemory) -> bool>,
    /// Which Live sessions still refuse edits ([`live_edit_refusal`]).
    pub(crate) live_edit: Option<LiveEditProbe>,
    /// The game at this boundary ([`Reader::with_guest`] for guest memory).
    pub reader: &'a mut dyn Reader,
    /// The stamp of this boundary.
    pub at: At,
    host: &'a mut BTreeMap<String, (Access, HostMethod)>,
    /// Set by [`GameCtx::defer`]: the answer comes at a later boundary.
    later: Option<DeferredAnswer>,
}

/// Work run at every game tick ([`Agent::add_ticker`]), with the game at
/// the tick boundary, for an act that spans more ticks than a deferred
/// answer may wait ([`DEFER_TICKS_MAX`]): a method starts it, the ticker
/// drives it, a read reports on it.
pub type Ticker = Box<dyn FnMut(&mut GameCtx<'_>) + Send>;

/// The connection id a [`Ticker`]'s [`GameCtx`] carries: no connection's.
pub const TICKER_CONN: ConnId = ConnId::MAX;

/// A deferred answer ([`GameCtx::defer`]): polled at each boundary after
/// the one that asked, `None` until it is ready.
pub type Later = Box<dyn FnMut(&mut GameCtx<'_>) -> Option<Result<Value, RpcError>> + Send>;

struct DeferredAnswer {
    poll: Later,
    ticks: u64,
    wall: Duration,
    // A decision made by guest code waits for the next tick. A render
    // thread can finish an already sealed scene, and a Theater transport
    // action can complete, while the guest tick is held.
    paused_safe: bool,
}

/// The longest a deferred answer may wait, in game ticks, and in wall time
/// whatever the ticks (a game that stopped ticking).
pub const DEFER_TICKS_MAX: u64 = 300;
const DEFER_WALL: Duration = Duration::from_secs(60);

impl GameCtx<'_> {
    pub(crate) fn edit_lifetime(&mut self) -> Arc<AtomicU64> {
        let epoch = self.epoch;
        self.map_lifetime.get_or_insert_with(|| Arc::new(AtomicU64::new(epoch))).clone()
    }

    /// Runs an operation once with guest memory at this boundary.
    pub(crate) fn with_guest<T>(
        &mut self,
        f: impl FnOnce(&mut dyn guest_layout::GuestMemory) -> Result<T, RpcError>,
    ) -> Result<T, RpcError> {
        let mut f = Some(f);
        let mut out = None;
        let ran = self.reader.with_guest(&mut |m| {
            if let Some(f) = f.take() {
                out = Some(f(m));
            }
        });
        match out {
            Some(r) if ran => r,
            _ => Err(RpcError::new(
                code::UNAVAILABLE,
                "no guest memory here (the game isn't running)",
            )),
        }
    }

    /// Inspect guest state at this game boundary.
    pub fn read_guest<T>(&mut self, f: impl FnOnce(&dyn guest_layout::GuestMemory) -> T) -> Result<T, RpcError> {
        self.with_guest(|m| Ok(f(m)))
    }

    /// Answers a host method (`ui.screen`, `input.state`) as a client
    /// would get it, when the host registered one of that name.
    pub fn host(&mut self, name: &str, params: Value) -> Option<Result<Value, RpcError>> {
        self.host.get_mut(name).map(|(_, f)| f(params))
    }

    /// Answers later: `poll` runs at each boundary from the next one until
    /// it gives the answer, or `ticks` game ticks pass (at most
    /// [`DEFER_TICKS_MAX`]; then `TIMEOUT`). For work the game does at its
    /// next tick (guest calls a method queued). What the method returns is
    /// dropped. A paused game holds the answer until it steps.
    pub fn defer(&mut self, ticks: u64, poll: Later) {
        self.defer_for(ticks, DEFER_WALL, poll);
    }

    /// File preparation runs in wall time. A fast host may advance hundreds of
    /// ticks while copying one map, so only the normal wall deadline applies.
    pub(crate) fn defer_file(&mut self, poll: Later) {
        self.later = Some(DeferredAnswer { poll, ticks: u64::MAX, wall: DEFER_WALL, paused_safe: false });
    }

    /// A deferred operation with its own, shorter wall-clock deadline.
    pub fn defer_for(&mut self, ticks: u64, wall: Duration, poll: Later) {
        self.later = Some(DeferredAnswer {
            poll, ticks: ticks.clamp(1, DEFER_TICKS_MAX), wall: wall.min(DEFER_WALL), paused_safe: false,
        });
    }

    /// Poll work on another thread at each boundary, including a held
    /// tick. The poll cannot read or write guest state.
    pub(crate) fn defer_host(
        &mut self,
        mut poll: impl FnMut() -> Option<Result<Value, RpcError>> + Send + 'static,
    ) {
        self.defer_at_boundary(Box::new(move |_| poll()));
    }

    /// A host-side action can complete on a Theater transport poll while the
    /// guest tick is still held. Its poll must not depend on guest execution.
    pub fn defer_at_boundary(&mut self, poll: Later) {
        self.later = Some(DeferredAnswer { poll, ticks: DEFER_TICKS_MAX, wall: DEFER_WALL, paused_safe: true });
    }
}

/// A game method's answer still coming ([`GameCtx::defer`]).
struct Deferred {
    conn: ConnId,
    id: Option<Value>,
    poll: Later,
    from_ticks: u64,
    tick_limit: u64,
    paused_safe: bool,
    deadline: Instant,
}

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
    collision_traces: Arc<crate::CollisionTraces>,
    /// Level collision live commits replaced, for the debug views.
    collision_edits: Arc<crate::debug_views::CollisionEdits>,
    /// The BSP activation `edit.collision.commit` runs, when the game has
    /// one ([`Agent::set_collision_activation`]).
    collision_activation: crate::hot::collision::Activator,
    /// Told of every structure collision commit
    /// ([`Agent::on_structure_collision`]).
    structure_listeners: crate::hot::collision::StructureListeners,
    editor_calls: OnceLock<crate::sim::calls::Shared>,
    state: Mutex<State>,
    wake: Condvar,
    connection_reads_running: AtomicUsize,
}

#[derive(Default)]
struct State {
    live: crate::live::Mirror,
    /// `render.picture`'s open pictures (`exec/picture.rs`).
    pictures: Arc<crate::render::picture::Pictures>,
    /// The `camera` layer's source ([`Agent::set_camera_source`]), and why
    /// this run has none ([`Agent::set_camera_unavailable`]).
    cameras: Option<crate::live::CameraSource>,
    camera_off: Option<String>,
    editor_marks: Option<crate::editor::MarkObserver>,
    debug_views: Option<crate::debug_views::Observer>,
    live_epoch: u64,
    // The last epoch that replaced the map or restored its state.
    map_epoch: u64,
    activation: u64,
    map_lifetime: Option<Arc<AtomicU64>>,
    live_session: Option<fn(&dyn guest_layout::GuestMemory) -> bool>,
    live_edit: Option<LiveEditProbe>,
    pending: VecDeque<Queued>,
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
    /// Ticks since the current map loaded.
    map_ticks: u64,
    presents: u64,
    game: GameStatus,
    phase: Option<Phase>,
    cinematic: bool,
    last_tick: Option<Instant>,
    /// The boundary the current answers are read at.
    boundary: Option<Boundary>,
    plan: Vec<String>,
    shutdown: bool,
    /// The game thread is inside the pause loop, holding the tick.
    holding: bool,
    /// Methods the host answers, by name.
    host: BTreeMap<String, (Access, HostMethod)>,
    connection_reads: BTreeMap<String, ConnectionRead>,
    /// Methods answered with the game at hand, by name.
    game_methods: BTreeMap<String, (Access, GameMethod)>,
    /// The tick observer isn't installed (switched off, or the plan left
    /// it out): nothing can pause, and world reads come between slices.
    no_tick_hook: bool,
    /// Pad queues, recording or replay, the controllers the game has open
    /// and where logs go: shared with the input tap and its methods.
    input: Arc<Mutex<Input>>,
    /// The `profile.capture` running.
    capture: Option<Capturing>,
    /// Frame stats (`timeline::phase` unless a test sets one).
    frames: Option<Box<dyn FrameSource>>,
    /// A sampling profiler, when the game was built with one.
    sampler: Option<Box<dyn Sampler>>,
    /// Level loads and stalls, from the ticks (`load.timeline`).
    loads: LoadTracker,
    /// Frame-stats running totals (`timeline::phase` unless a test sets
    /// one).
    totals: Option<Box<dyn TotalsSource>>,
    /// `trace.*` and `flight.*`: rows taken at every tick.
    recorders: Recorders,
    /// `state.*`'s store, once the host installs snapshots.
    snapshots: Option<Box<dyn Snapshots>>,
    /// Saves and loads waiting for a quiescent slice boundary.
    snap_jobs: VecDeque<snap::Job>,
    /// Game methods' answers still coming ([`GameCtx::defer`]).
    deferred: Vec<Deferred>,
    /// Work run at every tick ([`Agent::add_ticker`]), by name.
    tickers: BTreeMap<String, Ticker>,
    /// Names `world.*` results' tags ([`Agent::set_tag_names`]).
    tag_names: Option<Mutex<Box<dyn TagNames>>>,
    /// Fills `world.*` results' `physics.standing_fits`
    /// ([`Agent::set_standing_fits`]).
    standing_fits: Option<Mutex<Box<dyn crate::reader::StandingFits>>>,
}

struct Queued {
    conn: ConnId,
    request: Request,
    _count: crate::memory::PendingRequest,
}

/// A `profile.capture` waiting for its window to pass.
struct Capturing {
    conn: ConnId,
    id: Value,
    from_ticks: u64,
    tick_limit: Option<u64>,
    deadline: Instant,
    budget_ms: f64,
    spikes: usize,
    marks: Vec<TickMark>,
    /// Files are named after this.
    stem: String,
    /// `None`: not asked for; `Err`: why not.
    sampling: Option<Result<(), String>>,
}

enum Output {
    Local(SyncSender<String>),
    Socket(SyncSender<crate::server::Output>),
}

struct Conn {
    _connection: mem_watch::Gauge,
    tx: Output,
    /// Once upgraded, both events and further requests are disabled.
    streaming: bool,
    events: BTreeSet<EventKind>,
    /// Its outgoing queue filled up: the client isn't reading. Dropped at
    /// the next prune, whatever it asked.
    dead: bool,
    /// Its client stopped sending (a half-close or a disconnect): answer
    /// what it already asked, then drop it.
    closing: bool,
}

struct Waiting {
    conn: ConnId,
    id: Value,
    until: Until,
    from_ticks: u64,
    tick_limit: Option<u64>,
    deadline: Instant,
    /// A `session.step`'s answer: interrupted by a resume, pause or release.
    step: bool,
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

    /// A new connection; lines for it (responses and events) go to `tx`,
    /// a bounded queue: a client that lets it fill is dropped.
    pub fn connect(&self, tx: SyncSender<String>) -> ConnId {
        self.connect_to(Output::Local(tx))
    }

    pub(crate) fn connect_socket(&self, tx: SyncSender<crate::server::Output>) -> ConnId {
        self.connect_to(Output::Socket(tx))
    }

    fn connect_to(&self, tx: Output) -> ConnId {
        let mut s = self.lock();
        s.next_conn += 1;
        let id = s.next_conn;
        s.conns.insert(
            id,
            Conn {
                _connection: mem_watch::agent_activity::connection(),
                tx,
                streaming: false,
                events: BTreeSet::new(),
                closing: false,
                dead: false,
            },
        );
        id
    }

    /// The connection sent its last line (it closed, or half-closed): if
    /// it held control, release it (a pause it left resumes). What it
    /// already asked is still answered; then it is dropped.
    pub fn disconnect(&self, conn: ConnId) {
        let mut s = self.lock();
        if let Some(c) = s.conns.get_mut(&conn) {
            c.closing = true;
        }
        s.live.close(conn);
        s.pictures.close_conn(conn);
        input::lock(&s.input).release_menu(conn);
        if s.controller == Some(conn) {
            s.controller = None;
            s.resume("released");
        }
        s.prune();
        drop(s);
        self.shared.wake.notify_all();
    }

    /// A line from a connection. Parse errors are answered at once;
    /// requests queue for the game thread.
    pub fn submit(&self, conn: ConnId, line: &str) {
        let _scope = mem_watch::scope("agent.request");
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        // Parsed before taking the lock, so a long line never holds up the
        // game thread.
        let parsed = match serde_json::from_str::<Value>(line) {
            Err(e) => Err(Response::err(
                Value::Null,
                RpcError::new(code::PARSE, e.to_string()),
            )),
            Ok(v) => {
                let id = v.get("id").cloned().unwrap_or(Value::Null);
                match serde_json::from_value::<Request>(v) {
                    Ok(r) if r.jsonrpc == "2.0" => Ok(r),
                    Ok(_) => Err(Response::err(
                        id,
                        RpcError::new(code::INVALID_REQUEST, "jsonrpc must be \"2.0\""),
                    )),
                    Err(e) => Err(Response::err(
                        id,
                        RpcError::new(
                            code::INVALID_REQUEST,
                            format!("not a request (batches aren't supported): {e}"),
                        ),
                    )),
                }
            }
        };
        let mut s = self.lock();
        if s.conns.get(&conn).is_none_or(|c| c.streaming) {
            return;
        }
        match parsed {
            Ok(r) if s.shutdown => {
                if let Some(id) = r.id {
                    s.send(
                        conn,
                        Response::err(id, RpcError::new(code::SHUTDOWN, "the game is exiting")),
                    );
                }
            }
            Ok(r) if s.connection_reads.contains_key(&r.method) => {
                let read = s.connection_reads[&r.method].clone();
                let running = &self.shared.connection_reads_running;
                running.fetch_add(1, Ordering::SeqCst);
                struct InFlight<'a>(&'a AtomicUsize);
                impl Drop for InFlight<'_> {
                    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); }
                }
                let _job = InFlight(running);
                drop(s);
                mem_watch::agent_activity::served();
                let result = read(r.params);
                if let Some(id) = r.id {
                    let mut s = self.lock();
                    let reply = match result {
                        Ok(mut value) => {
                            if let Value::Object(obj) = &mut value {
                                obj.entry("at").or_insert_with(|| to_value(s.at()));
                            }
                            Response::ok(id, value)
                        }
                        Err(e) => Response::err(id, e),
                    };
                    s.send(conn, reply);
                }
                return;
            }
            Ok(r) => s.pending.push_back(Queued {
                conn,
                request: r,
                _count: crate::memory::PendingRequest::new(),
            }),
            Err(resp) => s.send(conn, resp),
        }
        drop(s);
        self.shared.wake.notify_all();
    }

    /// The tick observer isn't installed: control methods are refused and
    /// world reads are answered between slices.
    pub fn set_no_tick_hook(&self) {
        let mut state = self.lock();
        state.no_tick_hook = true;
        if let Some(marks) = &state.editor_marks {
            marks.set_no_tick_hook();
        }
    }

    /// Answers `name` with `f` (replacing an earlier one of that name).
    /// A result that is a JSON object gets the `at` stamp unless it has
    /// one. Core methods can't be replaced.
    pub fn add_method(
        &self,
        name: &str,
        access: Access,
        f: impl FnMut(Value) -> Result<Value, RpcError> + Send + 'static,
    ) {
        assert!(
            !method::ALL.contains(&name),
            "{name} is a core method of the agent API"
        );
        self.lock()
            .host
            .insert(name.to_owned(), (access, Box::new(f)));
    }

    /// Independent host reads still waiting to return a response.
    pub fn connection_read_pending(&self) -> bool {
        self.shared.connection_reads_running.load(Ordering::SeqCst) > 0
    }

    /// Register an independent host read, such as a second player's replay
    /// verification. Its connection waits for the reply while the game runs.
    pub fn add_connection_read(
        &self,
        name: &str,
        f: impl Fn(Value) -> Result<Value, RpcError> + Send + Sync + 'static,
    ) {
        assert!(!method::ALL.contains(&name));
        self.lock().connection_reads.insert(name.to_owned(), Arc::new(f));
    }

    /// Answers `name` with `f`, given the game at the boundary
    /// ([`GameCtx`]), as [`add_method`](Self::add_method) otherwise.
    pub fn add_game_method(
        &self,
        name: &str,
        access: Access,
        f: impl FnMut(Value, &mut GameCtx<'_>) -> Result<Value, RpcError> + Send + 'static,
    ) {
        assert!(
            !method::ALL.contains(&name),
            "{name} is a core method of the agent API"
        );
        self.lock()
            .game_methods
            .insert(name.to_owned(), (access, Box::new(f)));
    }

    /// Runs `f` at every game tick, after the tick's requests are served
    /// and before deferred answers are polled, once a tick (not again
    /// while a paused game holds the tick), with the game at the boundary
    /// ([`GameCtx`]; its `conn` is [`TICKER_CONN`]). Replaces an earlier
    /// ticker of that name. Like a host method it runs with the agent's
    /// state locked: it must be quick and must not call the [`Agent`].
    pub fn add_ticker(
        &self,
        name: &str,
        f: impl FnMut(&mut GameCtx<'_>) + Send + 'static,
    ) {
        self.lock().tickers.insert(name.to_owned(), Box::new(f));
    }

    pub fn collision_traces(&self) -> Arc<crate::CollisionTraces> {
        self.shared.collision_traces.clone()
    }

    pub(crate) fn collision_edits(&self) -> Arc<crate::debug_views::CollisionEdits> {
        self.shared.collision_edits.clone()
    }

    pub(crate) fn collision_activator(&self) -> crate::hot::collision::Activator {
        self.shared.collision_activation.clone()
    }

    /// The BSP activation a collision commit with `activate` or `mode:
    /// rebuild` runs (docs/editor/realtime-edits.md, "Collision"). Without
    /// one those commits are refused.
    pub fn set_collision_activation(&self, activation: Box<dyn crate::hot::collision::Activation>) {
        *self
            .shared
            .collision_activation
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(activation);
    }

    /// Calls `f` after every `edit.collision.commit` that replaces a
    /// structure's level collision (after its activation, when it asks for
    /// one), from the commit's tick, with the guest held: `f` must not
    /// wait on the game.
    pub fn on_structure_collision(&self, f: impl Fn(&crate::hot::collision::StructureCommit) + Send + Sync + 'static) {
        self.shared
            .structure_listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::new(f));
    }

    pub(crate) fn structure_listeners(&self) -> crate::hot::collision::StructureListeners {
        self.shared.structure_listeners.clone()
    }

    pub(crate) fn set_debug_views(&self, views: crate::debug_views::Observer) {
        self.lock().debug_views = Some(views);
    }

    pub(crate) fn set_editor_marks(&self, marks: crate::editor::MarkObserver) {
        let mut state = self.lock();
        if state.no_tick_hook {
            marks.set_no_tick_hook();
        }
        state.editor_marks = Some(marks);
    }

    /// The feature plan's text, for `features.list`.
    pub fn set_plan(&self, lines: Vec<String>) {
        self.lock().plan = lines;
    }

    /// HaloScript `cinematic_start` / `cinematic_stop` ran.
    pub fn set_cinematic(&self, on: bool) {
        self.lock().cinematic = on;
    }

    /// A successful full game start. Checkpoint reactivation takes a different
    /// path and never publishes an editor reload's reserved identity.
    pub(crate) fn game_started(&self, activation: u64) {
        let mut s = self.lock();
        s.activation = activation;
        s.map_ticks = 0;
    }

    /// Checkpoint restore invalidates the old instance even at a later clock.
    pub(crate) fn live_cut(&self) {
        self.shared.collision_traces.clear();
        let mut s = self.lock();
        s.advance_live_epoch();
        s.map_epoch = s.live_epoch;
        input::lock(&s.input).clear_routes("checkpoint_restored");
    }

    /// What a machine snapshot keeps of the game as the agent saw it
    /// (docs/runtime/snapshots.md): whether a cutscene is running, the map
    /// and its tick count. Without them a loaded cutscene reads as gameplay,
    /// and the map the agent last saw makes the first tick after a load
    /// look like a map change. Connections, pauses and recordings are the
    /// session's, not the game's, and stay.
    pub fn save_state(&self) -> Vec<u8> {
        let s = self.lock();
        let mut w = snapshot::codec::Writer::new();
        w.bool(s.cinematic);
        match &s.game.map {
            Some(map) => w.bool(true).str(map),
            None => w.bool(false),
        };
        w.u64(s.map_ticks);
        w.into_bytes()
    }

    /// Puts [`Self::save_state`]'s bytes back; nothing changes unless all
    /// of them decode.
    pub fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        let r = &mut snapshot::codec::Reader::new(bytes, "agent");
        let e = |e: snapshot::Error| e.to_string();
        let cinematic = r.bool().map_err(e)?;
        let map = if r.bool().map_err(e)? {
            Some(r.str().map_err(e)?.to_owned())
        } else {
            None
        };
        let map_ticks = r.u64().map_err(e)?;
        r.finish().map_err(e)?;
        self.shared.collision_traces.clear();
        let mut s = self.lock();
        s.cinematic = cinematic;
        s.game.map = map;
        s.map_ticks = map_ticks;
        s.activation = 0;
        s.advance_live_epoch();
        s.map_epoch = s.live_epoch;
        input::lock(&s.input).clear_routes("snapshot_loaded");
        Ok(())
    }

    /// The game is exiting: fail what's queued and waiting, and let a
    /// paused game go.
    pub fn shutdown(&self) {
        let mut s = self.lock();
        // A recording still running is kept.
        let recording = matches!(
            input::lock(&s.input).mode(),
            agent_proto::InputMode::Recording { .. }
        );
        if recording && let Err(e) = input::stop_and_write(&s.input) {
            eprintln!("agent: writing the input recording at exit: {}", e.message);
        }
        s.recorders.shutdown();
        s.live.close_all();
        // Ring files go with the game.
        s.pictures.retain_conns(|_| false);
        s.fail_snapshots();
        s.shutdown = true;
        s.paused = None;
        s.stepping = None;
        for Queued { conn, request: r, .. } in std::mem::take(&mut s.pending) {
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
        for d in std::mem::take(&mut s.deferred) {
            if let Some(id) = d.id {
                s.send(
                    d.conn,
                    Response::err(id, RpcError::new(code::SHUTDOWN, "the game is exiting")),
                );
            }
        }
        if let Some(c) = s.cancel_capture() {
            s.send(
                c.conn,
                Response::err(c.id, RpcError::new(code::SHUTDOWN, "the game is exiting")),
            );
        }
        drop(s);
        self.shared.wake.notify_all();
    }

    pub(crate) fn edit_input(&self) -> Arc<Mutex<Input>> {
        Arc::clone(&self.lock().input)
    }

    pub(crate) fn editor_adopted(&self, queue: crate::sim::calls::Shared) {
        let _ = self.shared.editor_calls.set(queue);
    }

    /// Machine snapshots cannot reproduce an unlinked placement preview.
    /// An unadopted game can answer this without locking the call queue.
    pub fn editor_snapshot_refusal(&self) -> Option<String> {
        crate::sim::calls::lock(self.shared.editor_calls.get()?).preview_refusal()
    }

    /// A callback installed on the agent can observe its lifetime without
    /// keeping the executor alive through its own method table.
    pub(crate) fn weak_handle(&self) -> impl Fn() -> Option<Agent> + Send + use<> {
        let weak = Arc::downgrade(&self.shared);
        move || weak.upgrade().map(|shared| Agent { shared })
    }

    /// The input tap for `xapi`, wrapping the one already there (`inner`,
    /// from `H2_INPUT_RECORD` or `H2_INPUT_REPLAY`) where `Chain` says, and
    /// the `input.*` host methods that drive it.
    pub fn input_tap(&self, inner: Option<(Box<dyn xapi::tap::InputTap>, Chain)>) -> AgentTap {
        let input = Arc::clone(&self.lock().input);
        input::add_input_methods(self, &input);
        AgentTap::new(input, inner)
    }

    /// Override the root for input logs, traces, flights and profile captures.
    pub fn set_dir(&self, dir: PathBuf) {
        input::lock(&self.lock().input).dir = Some(dir);
    }

    /// Where frames for `profile.capture` come from (tests).
    pub fn set_frame_source(&self, frames: Box<dyn FrameSource>) {
        self.lock().frames = Some(frames);
    }

    /// Where running frame totals for `load.timeline` come from (tests).
    pub fn set_totals_source(&self, totals: Box<dyn TotalsSource>) {
        self.lock().totals = Some(totals);
    }

    /// The sampling profiler `profile.capture {"sample": true}` runs.
    pub fn set_sampler(&self, sampler: Box<dyn Sampler>) {
        self.lock().sampler = Some(sampler);
    }

    /// Where `world.*` results' `tag_name`s come from (the loaded map's
    /// tag names); without it objects carry only the `tag` datum.
    pub fn set_tag_names(&self, names: Box<dyn TagNames>) {
        self.lock().tag_names = Some(Mutex::new(names));
    }

    /// Where `world.*` results' `physics.standing_fits` comes from (the
    /// superbounce mode's capsule test); without it the field is absent.
    pub fn set_standing_fits(&self, fits: Box<dyn crate::reader::StandingFits>) {
        self.lock().standing_fits = Some(Mutex::new(fits));
    }

    #[cfg(test)]
    pub(crate) fn input_for_tests(&self) -> Arc<Mutex<Input>> {
        Arc::clone(&self.lock().input)
    }

    pub fn is_paused(&self) -> bool {
        self.lock().paused.is_some()
    }

    /// After each game tick, on the game's thread. Answers everything
    /// queued, settles waits, and blocks here while paused, answering
    /// requests as they arrive. True when a `state.*` save or load is due
    /// at this slice's end: the caller ends the slice early
    /// (`Runtime::request_preempt`). A paused game lets such a job go to
    /// the slice end and holds again at the next tick.
    pub fn on_tick(&self, reader: &mut dyn Reader) -> bool {
        let mut s = self.lock();
        if s.shutdown {
            return false;
        }
        s.ticks += 1;
        s.last_tick = Some(Instant::now());
        s.refresh(reader, Where::Tick);
        s.pictures.stamp(s.game.tick, s.presents);
        s.track_load();
        let stepping = s.stepping.is_some();
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
        s.mark_tick();
        s.record_tick(reader);
        if let Some(marks) = &s.editor_marks {
            marks.on_tick(reader);
        }
        if let Some(views) = &s.debug_views
            && views.active()
        {
            views.refresh(reader, &s.game);
        }
        s.serve(reader, Where::Tick);
        s.run_tickers(reader);
        s.capture_live(reader, None, stepping);
        s.settle_deferred(reader);
        s.settle_waits();
        s.settle_capture();
        let mut snap = s.arm_snapshots();
        while let Some(until) = s.paused {
            if s.shutdown || snap {
                break;
            }
            if Instant::now() >= until {
                s.resume("timeout");
                break;
            }
            s.holding = true;
            s = self
                .shared
                .wake
                .wait_timeout(s, PAUSE_POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            s.serve(reader, Where::Tick);
            s.settle_deferred(reader);
            s.settle_waits();
            s.settle_capture();
            snap = s.arm_snapshots();
        }
        if s.holding {
            s.holding = false;
            // The tick was held, not missing: the game is still ticking.
            s.last_tick = Some(Instant::now());
        }
        s.prune();
        snap
    }

    /// Between scheduler slices, on the game's thread. Answers what doesn't
    /// need a tick; world reads too once the game has stopped ticking.
    pub fn between_slices(&self, reader: &mut dyn Reader, presents: u64) {
        let mut s = self.lock();
        if s.shutdown {
            return;
        }
        s.presents = presents;
        s.pictures.stamp(s.game.tick, presents);
        // A pause asked for while the game isn't ticking (loading) takes
        // hold at the next tick; its limit still runs out here.
        if s.paused.is_some_and(|until| Instant::now() >= until) {
            s.resume("timeout");
        }
        // A capture by time ends here too (the game may not be ticking).
        s.settle_capture();
        // While ticking, the tick boundary does the reading; between
        // slices only queued requests that don't need a tick, and wall
        // clock timeouts. Once ticks stop (loading), read here, at most
        // every SLICE_REFRESH, for waits and events.
        let due = s.slice_refresh.is_none_or(|t| t.elapsed() >= SLICE_REFRESH);
        // A socketless menu has no tick hook. Maintain its map cache on
        // the existing slice clock while a debug view needs geometry.
        let local_views = s.no_tick_hook && s.debug_views.as_ref().is_some_and(|v| v.active());
        let idle_work =
            !s.ticking() && due && (local_views || !s.waits.is_empty() || s.wants_events());
        if !s.deferred.is_empty() && !s.ticking() {
            s.settle_deferred(reader);
        }
        if s.pending.is_empty() && !idle_work {
            if !s.waits.is_empty() {
                s.settle_waits();
                s.prune();
            }
            return;
        }
        s.slice_refresh = Some(Instant::now());
        // While the game ticks, a read here could catch a tick half done
        // (a BSP switch, a map change): answers keep the last tick's state.
        if !s.ticking() {
            s.refresh(reader, Where::Slice);
            if local_views && let Some(views) = &s.debug_views {
                views.refresh(reader, &s.game);
            }
        }
        s.serve(reader, Where::Slice);
        s.settle_waits();
        s.prune();
    }
}

impl State {
    fn send(&mut self, conn: ConnId, r: Response) {
        let _scope = mem_watch::scope("agent.reply");
        let line = serde_json::to_string(&r).expect("responses serialise");
        if let Some(c) = self.conns.get_mut(&conn) {
            Self::push(c, line);
        }
    }

    /// Queues a line; a full queue marks the connection dead (the game
    /// thread never blocks on a client).
    fn push(c: &mut Conn, line: String) {
        if c.streaming { return; }
        c.dead |= match &c.tx {
            Output::Local(tx) => tx.try_send(line).is_err(),
            Output::Socket(tx) => tx.try_send(crate::server::Output::Line(line.into())).is_err(),
        };
    }

    fn wants_events(&self) -> bool {
        self.conns.values().any(|c| !c.events.is_empty())
    }

    fn emit(&mut self, e: Event) {
        let _scope = mem_watch::scope("agent.reply");
        let kind = e.kind();
        let line = serde_json::to_string(&Notification::event(e)).expect("events serialise");
        for c in self.conns.values_mut().filter(|c| c.events.contains(&kind)) {
            Self::push(c, line.clone());
        }
    }

    fn ticking(&self) -> bool {
        self.last_tick.is_some_and(|t| t.elapsed() < TICK_STALE)
    }

    fn phase_now(&self) -> Phase {
        // Without the tick observer there are no ticks to go by.
        if !self.no_tick_hook && !self.ticking() && !self.holding {
            return Phase::Loading;
        }
        // The main menu's background runs as a cutscene of its own
        // (`cinematic_start` fires there), so the menu wins.
        match self.game.mode.as_deref() {
            Some("main_menu") => Phase::Menu,
            Some("campaign" | "multiplayer") if self.cinematic => Phase::Cinematic,
            Some("campaign" | "multiplayer") => Phase::Gameplay,
            _ => Phase::Loading,
        }
    }

    fn at(&self) -> At {
        At {
            tick: self.game.tick,
            ticks: self.ticks,
            activation: self.activation,
            map_ticks: self.map_ticks,
            present: self.presents,
            map: self.game.map.clone(),
            mode: self.game.mode.clone(),
            bsp: self.game.bsp,
            phase: self.phase.unwrap_or(Phase::Loading),
            paused: self.paused.is_some(),
            boundary: self.boundary.unwrap_or(Boundary::Slice),
        }
    }

    fn read_totals(&mut self) -> Option<timeline::phase::Totals> {
        self.totals
            .get_or_insert_with(|| Box::new(TimelineTotals))
            .totals()
    }

    /// After a tick's refresh: feeds the load tracker.
    fn track_load(&mut self) {
        let totals = self.read_totals();
        let (map, bsp, present) = (self.game.map.clone(), self.game.bsp, self.presents);
        self.loads
            .on_tick(Instant::now(), map, bsp, present, totals);
    }

    fn loads(&mut self, p: LoadParams) -> Result<Loads, RpcError> {
        let n = p.last.unwrap_or(LoadParams::DEFAULT);
        if n == 0 || n > LoadParams::KEEP {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("last: 1 to {}", LoadParams::KEEP),
            ));
        }
        let totals = self.read_totals();
        let (loads, current) = self.loads.report(n, Instant::now(), totals);
        Ok(Loads {
            at: self.at(),
            loads,
            current,
            no_frame_stats: self.loads.no_frame_stats || totals.is_none(),
        })
    }

    fn advance_live_epoch(&mut self) {
        self.live_epoch += 1;
        if let Some(lifetime) = &self.map_lifetime {
            lifetime.store(self.live_epoch, Ordering::Release);
        }
        if let Some(marks) = &self.editor_marks {
            marks.invalidate();
        }
    }

    /// Reads the game's status and emits what changed.
    fn refresh(&mut self, reader: &mut dyn Reader, where_: Where) -> At {
        self.boundary = Some(match where_ {
            Where::Tick => Boundary::Tick,
            Where::Slice => Boundary::Slice,
        });
        let old = std::mem::replace(&mut self.game, reader.status());
        let map_changed = old.map != self.game.map;
        let bsp_changed = old.bsp != self.game.bsp;
        let rewound = old
            .tick
            .zip(self.game.tick)
            .is_some_and(|(prev, tick)| tick < prev);
        if map_changed || rewound || bsp_changed {
            self.advance_live_epoch();
            if map_changed || rewound {
                self.map_epoch = self.live_epoch;
            }
        }
        if map_changed {
            self.map_ticks = 0;
        }
        // Counting this tick, so a map's first tick says 1 in its events too.
        if where_ == Where::Tick {
            self.map_ticks += 1;
        }
        if old.map.is_some() && map_changed {
            // A cutscene doesn't outlive its map, nor do pad steps (the
            // game's tick starts again).
            self.cinematic = false;
            input::lock(&self.input).clear_pads();
        }
        if let Some(t) = self.game.tick {
            input::lock(&self.input).set_clock(t);
        }
        let old_phase = self.phase;
        self.phase = Some(self.phase_now());
        let at = self.at();
        if map_changed {
            self.emit(Event::Map {
                from: old.map,
                to: self.game.map.clone(),
                at: at.clone(),
            });
        }
        if bsp_changed {
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
        self.interrupt_steps(why);
    }

    /// A step that won't finish as asked (the game resumed, or paused
    /// again before its ticks ran) is answered now, not at its timeout.
    fn interrupt_steps(&mut self, why: &str) {
        let (steps, rest): (Vec<Waiting>, Vec<Waiting>) = std::mem::take(&mut self.waits)
            .into_iter()
            .partition(|w| w.step);
        self.waits = rest;
        for w in steps {
            let mut e = RpcError::new(
                code::INTERRUPTED,
                format!(
                    "the step was interrupted ({why}) after {} ticks",
                    self.ticks - w.from_ticks
                ),
            );
            e.data = Some(to_value(self.at()));
            self.send(w.conn, Response::err(w.id, e));
        }
    }

    /// Drops closed connections with nothing left to answer.
    fn prune(&mut self) {
        let busy: BTreeSet<ConnId> = self
            .pending
            .iter()
            .map(|q| q.conn)
            .chain(self.waits.iter().map(|w| w.conn))
            .chain(self.capture.as_ref().map(|c| c.conn))
            .chain(self.snap_jobs.iter().map(|j| j.conn()))
            .chain(self.deferred.iter().map(|d| d.conn))
            .collect();
        let dead: BTreeSet<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| c.dead)
            .map(|(id, _)| *id)
            .collect();
        if !dead.is_empty() {
            self.pending.retain(|q| !dead.contains(&q.conn));
            self.waits.retain(|w| !dead.contains(&w.conn));
            self.snap_jobs.retain(|j| !dead.contains(&j.conn()));
            self.deferred.retain(|d| !dead.contains(&d.conn));
        }
        self.conns
            .retain(|id, c| !c.dead && (!c.closing || busy.contains(id)));
        // A picture never outlives its connection.
        let conns = &self.conns;
        self.pictures.retain_conns(|c| conns.contains_key(&c));
        // A capture nobody will read is stopped.
        if self
            .capture
            .as_ref()
            .is_some_and(|c| !self.conns.contains_key(&c.conn))
        {
            self.cancel_capture();
        }
        // Control never outlives its connection.
        if self
            .controller
            .is_some_and(|c| !self.conns.contains_key(&c))
        {
            self.controller = None;
            self.resume("released");
        }
    }

    /// Answers queued requests. Between slices, world reads stay queued
    /// while the game is still ticking, so they're read at a tick.
    fn serve(&mut self, reader: &mut dyn Reader, where_: Where) {
        let defer_world = where_ == Where::Slice && self.ticking();
        let mut keep = VecDeque::new();
        while let Some(queued) = self.pending.pop_front() {
            let conn = queued.conn;
            let req = &queued.request;
            if self.conns.get(&conn).is_none_or(|c| c.streaming) { continue; }
            if defer_world
                && (req.method.starts_with("world.") || matches!(req.method.as_str(), agent_proto::live::SUBSCRIBE | agent_proto::live::STATUS)
                    || self.game_methods.contains_key(&req.method))
            {
                keep.push_back(queued);
                continue;
            }
            let id = req.id.clone();
            let out = self.handle(conn, queued.request, reader);
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

    /// Runs every [`Ticker`] once, at this tick.
    fn run_tickers(&mut self, reader: &mut dyn Reader) {
        if self.tickers.is_empty() {
            return;
        }
        let at = self.at();
        let mut tickers = std::mem::take(&mut self.tickers);
        for f in tickers.values_mut() {
            let mut ctx = GameCtx {
                conn: TICKER_CONN,
                epoch: self.live_epoch,
                map_epoch: self.map_epoch,
                map_lifetime: &mut self.map_lifetime,
                live_session: self.live_session,
                live_edit: self.live_edit,
                reader: &mut *reader,
                at: at.clone(),
                host: &mut self.host,
                later: None,
            };
            f(&mut ctx);
        }
        // A ticker added meanwhile can't be (the state is locked), so the
        // old set goes back whole.
        self.tickers = tickers;
    }

    /// Polls the deferred answers: answers what is ready, times out what
    /// waited too long.
    fn settle_deferred(&mut self, reader: &mut dyn Reader) {
        self.settle_deferred_at(reader, false);
    }

    fn settle_deferred_at(&mut self, reader: &mut dyn Reader, theater_paused: bool) {
        if self.deferred.is_empty() {
            return;
        }
        let now = Instant::now();
        let at = self.at();
        for mut d in std::mem::take(&mut self.deferred) {
            if theater_paused && !d.paused_safe {
                self.deferred.push(d);
                continue;
            }
            let fresh = self.ticks == d.from_ticks && !d.paused_safe;
            let out = if fresh {
                None
            } else {
                let mut ctx = GameCtx {
                    conn: d.conn,
                    epoch: self.live_epoch,
                    map_epoch: self.map_epoch,
                    map_lifetime: &mut self.map_lifetime,
                    live_session: self.live_session,
                    live_edit: self.live_edit,
                    reader: &mut *reader,
                    at: at.clone(),
                    host: &mut self.host,
                    later: None,
                };
                (d.poll)(&mut ctx)
            };
            let out = match out {
                Some(r) => Some(r),
                None if self.ticks - d.from_ticks >= d.tick_limit || now >= d.deadline => {
                    let mut e = RpcError::new(
                        code::TIMEOUT,
                        format!(
                            "the game didn't answer within {} ticks (it runs the work at its next tick)",
                            self.ticks - d.from_ticks
                        ),
                    );
                    e.data = Some(to_value(&at));
                    Some(Err(e))
                }
                None => None,
            };
            match (out, d.id.take()) {
                (None, id) => {
                    d.id = id;
                    self.deferred.push(d);
                }
                (Some(_), None) => {}
                (Some(r), Some(id)) => {
                    let r = match r {
                        Ok(mut v) => {
                            if let Value::Object(o) = &mut v {
                                o.entry("at").or_insert_with(|| to_value(&at));
                            }
                            Response::ok(id, v)
                        }
                        Err(e) => Response::err(id, e),
                    };
                    self.send(d.conn, r);
                }
            }
        }
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
            && u.activation.is_none_or(|n| self.activation == n)
            && u.ticks.is_none_or(|n| passed >= n)
            && u.map_ticks.is_none_or(|n| self.map_ticks >= n)
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
        mem_watch::agent_activity::served();
        let _scope = mem_watch::scope("agent.request");
        let m = req.method.as_str();
        if agent_proto::live::METHODS.contains(&m) {
            return self.live_method(conn, req, reader);
        }
        if agent_proto::picture::METHODS.contains(&m) {
            return self.picture_method(conn, req);
        }
        if method::CONTROL.contains(&m) && self.controller != Some(conn) {
            return Some(Err(RpcError::new(
                code::NOT_CONTROLLER,
                "session.claim first: only the controlling connection may pause, step or send input",
            )));
        }
        if self.no_tick_hook && (m == method::PAUSE || m == method::STEP) {
            return Some(Err(RpcError::new(
                code::UNAVAILABLE,
                "the agent's tick observer isn't installed (H2_FEATURES=-agent?): nothing can pause",
            )));
        }
        let p = req.params;
        if let Some((access, _)) = self.host.get(m) {
            if let Err(e) = access.check(self.controller, conn) {
                return Some(Err(e));
            }
            let at = to_value(self.at());
            let (_, f) = self.host.get_mut(m)?;
            return Some(f(p).map(|mut v| {
                if let Value::Object(o) = &mut v {
                    o.entry("at").or_insert(at);
                }
                v
            }));
        }
        if snap::is_method(m) {
            let at = to_value(self.at());
            return self.state_method(conn, req.id, m, p).map(|r| {
                r.map(|mut v| {
                    if let Value::Object(o) = &mut v {
                        o.entry("at").or_insert(at);
                    }
                    v
                })
            });
        }
        if trace::is_method(m) {
            let dir = input::lock(&self.input).output_dir();
            let at = self.at();
            return Some(self.recorders.handle(m, p, at, &dir, &self.input));
        }
        if self.game_methods.contains_key(m) {
            if m == method::UI_PRESS && self.conns.get(&conn).is_none_or(|c| c.closing) {
                return Some(Err(RpcError::new(code::INTERRUPTED, "menu connection closed")));
            }
            let at = self.at();
            let stamp = to_value(&at);
            let (access, _) = self.game_methods.get(m)?;
            if let Err(e) = access.check(self.controller, conn) {
                return Some(Err(e));
            }
            let (_, f) = self.game_methods.get_mut(m)?;
            let mut ctx = GameCtx {
                conn,
                epoch: self.live_epoch,
                map_epoch: self.map_epoch,
                map_lifetime: &mut self.map_lifetime,
                live_session: self.live_session,
                live_edit: self.live_edit,
                reader,
                at,
                host: &mut self.host,
                later: None,
            };
            let out = f(p, &mut ctx);
            if let (Ok(_), Some(later)) = (&out, ctx.later.take()) {
                self.deferred.push(Deferred {
                    conn,
                    id: req.id,
                    poll: later.poll,
                    from_ticks: self.ticks,
                    tick_limit: later.ticks,
                    paused_safe: later.paused_safe,
                    deadline: Instant::now() + later.wall,
                });
                return None;
            }
            return Some(out.map(|mut v| {
                if let Value::Object(o) = &mut v {
                    o.entry("at").or_insert(stamp);
                }
                v
            }));
        }
        Some(match m {
            method::INPUT_LIST => {
                let dir = input::lock(&self.input).output_dir();
                crate::outputs::input_list(&dir)
            }
            method::INPUT_LOG_READ => {
                let dir = input::lock(&self.input).output_dir();
                params(p).and_then(|p| crate::outputs::input_read(&dir, p))
            }
            method::STATUS => Ok(to_value(Status {
                at: self.at(),
                protocol: PROTOCOL,
                fps: timeline::phase::window_summary().map(|s| s.fps),
                controller: self.controller == Some(conn),
                claimed: self.controller.is_some(),
            })),
            method::METHODS => Ok(Value::from(
                method::ALL
                    .iter()
                    .map(|m| (*m).to_owned())
                    .chain(
                        agent_proto::trace::method::ALL
                            .iter()
                            .map(|m| (*m).to_owned()),
                    )
                    .chain(
                        agent_proto::state::method::ALL
                            .iter()
                            .map(|m| (*m).to_owned()),
                    )
                    .chain(agent_proto::live::METHODS.iter().map(|m| (*m).to_owned()))
                    .chain(agent_proto::picture::METHODS.iter().map(|m| (*m).to_owned()))
                    .chain(self.host.keys().cloned())
                    .chain(self.connection_reads.keys().cloned())
                    .chain(self.game_methods.keys().cloned())
                    .collect::<Vec<String>>(),
            )),
            method::CLAIM if self.conns.get(&conn).is_none_or(|c| c.closing) => Err(RpcError::new(
                code::NOT_CONTROLLER,
                "a closed connection can't take control",
            )),
            method::CLAIM => match self.controller {
                Some(c) if c != conn => Err(RpcError::new(
                    code::NOT_CONTROLLER,
                    "another connection holds control",
                )),
                _ => {
                    let mut input = input::lock(&self.input);
                    if let Some(owner) = input.menu_owner().filter(|owner| *owner != conn) {
                        input.release_menu(owner);
                    }
                    drop(input);
                    self.controller = Some(conn);
                    Ok(serde_json::json!({ "at": self.at(), "controller": true }))
                }
            },
            method::RELEASE => {
                input::lock(&self.input).release_menu(conn);
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
                // A second pause re-arms the limit.
                let was = self.paused.replace(Instant::now() + max);
                self.pause_len = max;
                if self.stepping.take().is_some() {
                    self.interrupt_steps("paused again");
                }
                if was.is_none() {
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
                if let Some(w) = self.waits.last_mut() {
                    w.step = true;
                }
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
            method::LOAD_TIMELINE => params::<LoadParams>(p)
                .and_then(|lp| self.loads(lp))
                .map(to_value),
            method::FEATURES => Ok(to_value(Features::new(self.at(), self.plan.clone()))),
            method::PROFILE_CAPTURE => {
                let cp = match params::<CaptureParams>(p) {
                    Ok(cp) => cp,
                    Err(e) => return Some(Err(e)),
                };
                return match self.start_capture(conn, req.id?, cp) {
                    Ok(()) => None,
                    Err(e) => Some(Err(e)),
                };
            }
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
            step: false,
        });
    }

    fn objects(&self, reader: &mut dyn Reader) -> Result<Vec<Object>, RpcError> {
        let mut objects = reader.objects().map_err(|e| {
            let mut err = RpcError::new(code::UNAVAILABLE, format!("reading the world: {e}"));
            err.data = Some(to_value(self.at()));
            err
        })?;
        if let Some(names) = &self.tag_names {
            let mut names = names.lock().unwrap_or_else(|e| e.into_inner());
            crate::reader::name_tags(&mut **names, &mut objects);
        }
        if let Some(fits) = &self.standing_fits {
            let mut fits = fits.lock().unwrap_or_else(|e| e.into_inner());
            crate::reader::fill_standing_fits(&mut **fits, &mut objects);
        }
        Ok(objects)
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

    fn frames(&mut self) -> &mut dyn FrameSource {
        self.frames
            .get_or_insert_with(|| Box::new(TimelineFrames))
            .as_mut()
    }

    fn start_capture(&mut self, conn: ConnId, id: Value, p: CaptureParams) -> Result<(), RpcError> {
        let bad = |m: String| Err(RpcError::new(code::INVALID_PARAMS, m));
        let (tick_limit, window) = match (p.ticks, p.ms) {
            (Some(t), None) if (1..=CaptureParams::MAX_TICKS).contains(&t) => (Some(t), WAIT_MAX),
            (None, Some(ms)) if (1..=CaptureParams::MAX_MS).contains(&ms) => {
                (None, Duration::from_millis(ms))
            }
            (Some(_), None) => {
                return bad(format!("ticks must be 1 to {}", CaptureParams::MAX_TICKS));
            }
            (None, Some(_)) => return bad(format!("ms must be 1 to {}", CaptureParams::MAX_MS)),
            _ => return bad("give ticks or ms (one of them)".into()),
        };
        let budget_ms = p.budget_ms.unwrap_or(CaptureParams::DEFAULT_BUDGET_MS);
        if !(budget_ms.is_finite() && budget_ms > 0.0) {
            return bad("budget_ms must be more than 0".into());
        }
        let spikes = p.spikes.unwrap_or(CaptureParams::DEFAULT_SPIKES);
        if spikes > CaptureParams::MAX_SPIKES {
            return bad(format!(
                "spikes must be at most {}",
                CaptureParams::MAX_SPIKES
            ));
        }
        if self.capture.is_some() {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "a capture is already running; one at a time",
            ));
        }
        if !self.frames().begin() {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "frame stats are off, so there is nothing to capture",
            ));
        }
        let stem = format!(
            "capture-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        );
        let sampling = p.sample.then(|| match self.sampler.as_mut() {
            None => {
                Err("sampling needs an h2 built with --features perf; this one wasn't".to_owned())
            }
            Some(s) => s.start(),
        });
        self.capture = Some(Capturing {
            conn,
            id,
            from_ticks: self.ticks,
            tick_limit,
            deadline: Instant::now() + window,
            budget_ms,
            spikes,
            marks: vec![self.tick_mark()],
            stem,
            sampling,
        });
        Ok(())
    }

    /// This tick's rows for a trace or the flight recorder.
    fn record_tick(&mut self, reader: &mut dyn Reader) {
        if !self.recorders.active() {
            return;
        }
        let tick = trace::Tick {
            ticks: self.ticks,
            tick: self.game.tick,
            map: self.game.map.clone(),
        };
        self.recorders.on_tick(&tick, reader, &self.input);
    }

    fn tick_mark(&self) -> TickMark {
        TickMark {
            ns: timeline::now_ns(),
            tick: self.game.tick,
            map: self.game.map.clone(),
        }
    }

    fn mark_tick(&mut self) {
        if self.capture.is_some() {
            let m = self.tick_mark();
            if let Some(c) = self.capture.as_mut() {
                c.marks.push(m);
            }
        }
    }

    /// Stops a capture without answering it; returns it so the caller can.
    fn cancel_capture(&mut self) -> Option<Capturing> {
        let c = self.capture.take()?;
        self.frames().end();
        if let (Some(Ok(())), Some(s)) = (&c.sampling, self.sampler.as_mut()) {
            let _ = s.finish();
        }
        Some(c)
    }

    /// Answers the capture once its window has passed.
    fn settle_capture(&mut self) {
        let Some(c) = &self.capture else { return };
        let by_ticks = c.tick_limit.is_some_and(|n| self.ticks - c.from_ticks >= n);
        if !by_ticks && Instant::now() < c.deadline {
            return;
        }
        let c = self.capture.take().expect("checked above");
        let frames = self.frames().end().unwrap_or_default();
        let unavailable = |e: String| serde_json::json!({ "unavailable": e });
        let sampling = c.sampling.map(|s| match s {
            Err(e) => unavailable(e),
            Ok(()) => match self.sampler.as_mut().map(|s| s.finish()) {
                Some(Ok(v)) => v,
                Some(Err(e)) => unavailable(e),
                None => unavailable("the sampler went away".into()),
            },
        });
        let stats = capture::summarise(&frames, &c.marks, c.budget_ms, c.spikes);
        let csv = self.dir().join("captures").join(format!("{}.csv", c.stem));
        let reply = match capture::write_csv(&csv, &frames, &c.marks) {
            Ok(()) => Ok(to_value(Capture {
                at: self.at(),
                ticks: self.ticks - c.from_ticks,
                complete: c.tick_limit.is_none() || by_ticks,
                stats,
                csv: csv.display().to_string(),
                sampling,
            })),
            Err(e) => Err(RpcError::new(
                code::UNAVAILABLE,
                format!("writing {}: {e}", csv.display()),
            )),
        };
        let r = match reply {
            Ok(v) => Response::ok(c.id, v),
            Err(e) => Response::err(c.id, e),
        };
        self.send(c.conn, r);
    }

    fn dir(&self) -> PathBuf {
        input::lock(&self.input).dir()
    }

    fn counters(&self) -> Result<Counters, RpcError> {
        let s = timeline::phase::window_summary().ok_or_else(|| {
            RpcError::new(code::UNAVAILABLE, "no frames in the frame-stats window yet")
        })?;
        let phase_ms = timeline::phase::Phase::ALL
            .iter()
            .map(|p| (p.label().to_owned(), round(s.phase_ms[*p as usize])))
            .collect();
        let mut counts: BTreeMap<String, f64> = timeline::phase::counter_columns()
            .filter(|&(i, _)| i < timeline::phase::Counter::ALL.len() || s.count_mean[i] > 0.0)
            .map(|(i, label)| (label.to_owned(), round(s.count_mean[i])))
            .collect();
        counts.extend(tick_counts(&sim_bridge::tick::ledger()));
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

/// The tick driver's mean time per tick since the game started
/// (docs/runtime/agent-api.md, `profile.counters`): `sim_tick_phase_<n>_ns`
/// for phase `n` (1 to 29, retail order), `sim_tick_ns` for the tick body
/// and `sim_loop_ms` for the mean entry-to-entry time, which includes the
/// sound that runs outside the phases. Nothing before the first tick.
fn tick_counts(l: &engine::sim_tick::Ledger) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    if l.ticks() == 0 {
        return out;
    }
    let per = |ns: u64| round(ns as f64 / l.ticks() as f64);
    for i in 0..engine::sim_tick::PHASES.len() {
        out.insert(format!("sim_tick_phase_{}_ns", i + 1), per(l.phase_ns(i)));
    }
    out.insert("sim_tick_ns".to_owned(), per(l.tick_ns()));
    if l.loops() > 0 {
        out.insert("sim_loop_ms".to_owned(), round(l.loop_ns() as f64 / 1e6 / l.loops() as f64));
    }
    out
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
    let tag = p.tag.as_deref().map(TagMatch::new);
    let mut hits: Vec<Object> = objects
        .into_iter()
        .filter(|o| p.kind.as_ref().is_none_or(|k| k.contains(&o.kind)))
        .filter(|o| tag.as_ref().is_none_or(|t| t.matches(o)))
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

/// A query's `tag`: a datum, or text the tag's name contains.
enum TagMatch {
    Datum(u32),
    Name(String),
}

impl TagMatch {
    fn new(t: &str) -> TagMatch {
        let t = t.trim();
        let hex = t.strip_prefix("0x").unwrap_or(t);
        if hex.len() == 8
            && let Ok(d) = u32::from_str_radix(hex, 16)
        {
            return TagMatch::Datum(d);
        }
        TagMatch::Name(fold_tag_name(t))
    }

    fn matches(&self, o: &Object) -> bool {
        match self {
            TagMatch::Datum(d) => {
                o.tag
                    .as_deref()
                    .and_then(|t| u32::from_str_radix(t, 16).ok())
                    == Some(*d)
            }
            TagMatch::Name(n) => o
                .tag_name
                .as_deref()
                .is_some_and(|name| fold_tag_name(name).contains(n.as_str())),
        }
    }
}

/// A tag name for matching: lower case, `/` as `\`.
fn fold_tag_name(name: &str) -> String {
    name.to_ascii_lowercase().replace('/', "\\")
}

pub(crate) fn trim(v: Value, fields: Option<&[String]>) -> Value {
    let (Value::Object(mut m), Some(fields)) = (v.clone(), fields) else {
        return v;
    };
    m.retain(|k, _| k == "id" || k == "kind" || fields.iter().any(|f| f == k));
    Value::Object(m)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tick_count_tests {
    use super::tick_counts;
    use engine::sim_tick::{Ledger, PHASES};

    #[test]
    fn counters_carry_the_ticks_phase_means() {
        assert!(tick_counts(&Ledger::new()).is_empty());
        let mut l = Ledger::new();
        let mut ns = [0u64; PHASES.len()];
        ns[0] = 3_000;
        l.finish_tick(1, &ns, 5_000);
        l.finish_tick(2, &ns, 5_000);
        let c = tick_counts(&l);
        assert_eq!(c["sim_tick_phase_1_ns"], 3_000.0);
        assert_eq!(c["sim_tick_ns"], 5_000.0);
        assert!(c.contains_key("sim_tick_phase_29_ns") && !c.contains_key("sim_loop_ms"));
    }
}
