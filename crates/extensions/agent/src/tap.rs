//! Input as a player gives it, at `xapi`'s input tap: the layer
//! `H2_INPUT_RECORD` and `H2_INPUT_REPLAY` use, where each `XInputGetState`
//! result passes just before the guest gets it. One mechanism wherever the
//! game runs with XAPI input: `h2 serve`, `h2 play`, the Xbox app.
//!
//! * `input.pad` queues steps per controller port on an
//!   `input::backend::remote::Remote`. A step holds for wall-clock `ms`,
//!   game `ticks`, or both, and until the guest has read that pad `reads`
//!   times; it merges over what the read returns
//!   (`input::backend::fallback::merge`), so a real pad and the keyboard
//!   still work, and the game can't tell an agent from a person.
//! * `input.record` / `input.replay` log every read to a named input log
//!   (`xapi::tap`'s format) or answer reads from one, read for read.
//!
//! [`AgentTap`] is the tap; it shares an [`Input`] with the executor, which
//! moves the clock (the game's own tick) at each tick. The input methods
//! are host methods ([`add_input_methods`]), registered when the tap is
//! made, so a game without XAPI input doesn't list them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use agent_proto::{
    AtTick, InputMode, PORTS, PadParams, PadStep, RecordParams, ReplayCounts, ReplayParams,
    RpcError, code, method,
};
use input::backend::fallback::merge;
use input::backend::remote::{Remote, Step};
use input::gamepad::{Buttons, Gamepad};
use serde_json::{Value, json};
use xapi::tap::{InputLog, InputTap, PACKET, Recorder, ReplayStats, Replayer};

use crate::{Access, Agent};

/// What one handle was last given, to keep its packet numbers moving.
#[derive(Debug, Clone, Copy, Default)]
struct Delivered {
    /// Added to the live packet number from the first agent step on.
    offset: u32,
    number: u32,
    pad: [u8; Gamepad::SIZE],
    seen: bool,
}

/// Recording or replaying through the tap.
#[derive(Default)]
enum Mode {
    #[default]
    Live,
    Recording {
        name: String,
        path: PathBuf,
        recorder: Recorder,
        log: Arc<Mutex<InputLog>>,
        from_tick: Option<u64>,
    },
    Replaying {
        name: String,
        path: PathBuf,
        reads: u64,
        replayer: Replayer,
        stats: Arc<Mutex<ReplayStats>>,
    },
}

/// The pads' queues and what the tap knows of the game's controllers.
#[derive(Default)]
pub struct Input {
    /// The game's tick as last stamped.
    clock: Option<u64>,
    /// The same, for the recorder's tick stamps.
    ticks: Arc<AtomicU64>,
    mode: Mode,
    /// Each controller port's steps.
    pads: [Remote; PORTS as usize],
    /// The first read of a guarded menu press can cancel the steps if
    /// its row no longer owns input. Ordinary pad steps have no guard.
    menu_press: Option<MenuPress>,
    /// Open handles and the port each was opened on.
    ports: BTreeMap<u32, u32>,
    delivered: BTreeMap<u32, Delivered>,
    /// An [`AgentTap`] was made (the game has XAPI input).
    pub(crate) tap: bool,
    /// Where input logs (and captures) go; the cache directory unless set.
    pub(crate) dir: Option<PathBuf>,
    /// The flight recorder's reads since the last tick (`crate::trace`);
    /// `None` while it isn't running.
    pub(crate) flight: Option<FlightReads>,
}

/// Controller reads as the game got them, kept for the flight recorder
/// until the next tick takes them.
#[derive(Default)]
pub(crate) struct FlightReads {
    /// Port (when known), handle, status, packet.
    pub reads: Vec<(Option<u32>, u32, u32, [u8; PACKET])>,
    /// Reads past `InputRead::MAX_PER_TICK`, not kept.
    pub dropped: u32,
}

type CheckMenu = Box<dyn FnOnce(&dyn xapi::GuestMemory) -> bool + Send>;

struct MenuPress {
    conn: crate::ConnId,
    deadline: Instant,
    check: Option<CheckMenu>,
    result: Arc<OnceLock<Result<(), RpcError>>>,
}

/// One outstanding RPC. Giving up its deferred answer releases its
/// gesture, even if the input device has stopped polling.
pub struct MenuReceipt {
    input: Arc<Mutex<Input>>,
    result: Arc<OnceLock<Result<(), RpcError>>>,
}

impl MenuReceipt {
    pub fn result(&self) -> Option<Result<(), RpcError>> {
        self.result.get().cloned()
    }
}

impl Drop for MenuReceipt {
    fn drop(&mut self) {
        let mut input = lock(&self.input);
        if input
            .menu_press
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(&p.result, &self.result))
        {
            input.cancel_menu("menu request ended");
        }
    }
}

/// Queue one controller-zero menu gesture through the installed agent tap.
pub struct MenuPad(Arc<Mutex<Input>>);

impl MenuPad {
    pub const TIMEOUT: Duration = Duration::from_secs(2);

    pub fn queue(
        &self,
        conn: crate::ConnId,
        steps: &[PadStep],
        check: impl FnOnce(&dyn xapi::GuestMemory) -> bool + Send + 'static,
    ) -> Result<MenuReceipt, RpcError> {
        let steps = pad_steps(steps).map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
        let mut input = lock(&self.0);
        input.expire_menu_press();
        if !input.ports.values().any(|&port| port == 0) {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "controller zero is not open",
            ));
        }
        if input.pads[0].busy() {
            return Err(RpcError::new(
                code::BUSY,
                "controller zero has pending input",
            ));
        }
        let result = Arc::new(OnceLock::new());
        input.pads[0].replace(steps);
        input.menu_press = Some(MenuPress {
            conn,
            deadline: Instant::now() + Self::TIMEOUT,
            check: Some(Box::new(check)),
            result: Arc::clone(&result),
        });
        Ok(MenuReceipt {
            input: Arc::clone(&self.0),
            result,
        })
    }
}

impl Input {
    /// Where input logs (and captures) go.
    pub fn dir(&self) -> PathBuf {
        self.dir.clone().unwrap_or_else(agent_proto::cache_dir)
    }

    /// The game's tick moved (or a map loaded and it started again).
    pub fn set_clock(&mut self, tick: u64) {
        self.expire_menu_press();
        self.clock = Some(tick);
        self.ticks.store(tick, Ordering::Relaxed);
    }

    /// Drops every port's steps (a map changed: tick timing starts again).
    pub fn clear_pads(&mut self) {
        self.cancel_menu("map changed");
        self.pads.iter().for_each(Remote::clear);
    }

    fn expire_menu_press(&mut self) {
        if self.menu_press.as_ref().is_some_and(|p| p.deadline <= Instant::now()) {
            self.finish_menu(Err(RpcError::new(code::TIMEOUT, "menu input read timed out")));
        }
    }

    fn finish_menu(&mut self, result: Result<(), RpcError>) {
        if let Some(p) = self.menu_press.take() {
            if result.is_err() {
                self.pads[0].clear();
            }
            let _ = p.result.set(result);
        }
    }

    fn cancel_menu(&mut self, why: &str) {
        if self.menu_press.is_some() {
            self.finish_menu(Err(RpcError::new(code::INTERRUPTED, why)));
        }
    }

    pub(crate) fn menu_owner(&self) -> Option<crate::ConnId> {
        self.menu_press.as_ref().map(|p| p.conn)
    }

    pub(crate) fn release_menu(&mut self, conn: crate::ConnId) {
        if self.menu_owner() == Some(conn) {
            self.cancel_menu("menu controller released");
        }
    }

    /// The queue for `port` (0 to 3).
    pub fn pad(&self, port: u32) -> Option<&Remote> {
        self.pads.get(port as usize)
    }

    pub fn open_ports(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.ports.values().copied().collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Starts logging every read to `path` (written at [`stop`](Self::stop)).
    pub fn record(&mut self, name: &str, path: PathBuf) -> Result<(), String> {
        self.must_be_live()?;
        let (recorder, log) = Recorder::new();
        self.mode = Mode::Recording {
            name: name.to_owned(),
            path,
            recorder: recorder.with_ticks(Arc::clone(&self.ticks)),
            log,
            from_tick: self.clock,
        };
        Ok(())
    }

    /// Answers reads from `log`, read for read, from the next read on.
    pub fn replay(&mut self, name: &str, path: PathBuf, log: InputLog) -> Result<(), String> {
        self.must_be_live()?;
        let reads = log.reads.len() as u64;
        let (replayer, stats) = Replayer::new(log);
        // The log's packet numbers are what the game got: the agent's own
        // numbering starts again on top of them.
        self.delivered.clear();
        self.mode = Mode::Replaying {
            name: name.to_owned(),
            path,
            reads,
            replayer,
            stats,
        };
        Ok(())
    }

    fn must_be_live(&self) -> Result<(), String> {
        match &self.mode {
            Mode::Live => Ok(()),
            Mode::Recording { name, .. } => {
                Err(format!("already recording {name}; input.stop first"))
            }
            Mode::Replaying { name, .. } => {
                Err(format!("already replaying {name}; input.stop first"))
            }
        }
    }

    /// What the tap is doing, as the wire says it.
    pub fn mode(&self) -> InputMode {
        match &self.mode {
            Mode::Live => InputMode::Live,
            Mode::Recording {
                name,
                path,
                log,
                from_tick,
                ..
            } => InputMode::Recording {
                name: name.clone(),
                path: path.display().to_string(),
                reads: log.lock().unwrap_or_else(|e| e.into_inner()).reads.len() as u64,
                from_tick: *from_tick,
            },
            Mode::Replaying {
                name,
                path,
                reads,
                stats,
                ..
            } => {
                let s = *stats.lock().unwrap_or_else(|e| e.into_inner());
                InputMode::Replaying {
                    name: name.clone(),
                    path: path.display().to_string(),
                    counts: ReplayCounts {
                        reads: *reads,
                        replayed: s.replayed,
                        matched: s.replayed - s.handle_mismatches,
                        handle_mismatches: s.handle_mismatches,
                        past_end: s.past_end,
                    },
                }
            }
        }
    }

    /// Ends a recording or replay: what it was, and for a recording the
    /// log to write. `None` when live.
    pub fn stop(&mut self) -> Option<(InputMode, Option<(PathBuf, InputLog)>)> {
        let ended = self.mode();
        match std::mem::take(&mut self.mode) {
            Mode::Live => None,
            Mode::Recording { path, log, .. } => {
                let log = std::mem::take(&mut *log.lock().unwrap_or_else(|e| e.into_inner()));
                Some((ended, Some((path, log))))
            }
            Mode::Replaying { .. } => Some((ended, None)),
        }
    }

    /// One guest read of `handle`: a replay answers it, then the port's
    /// pad step merges over it, then a recording logs what the game gets.
    fn read(
        &mut self,
        handle: u32,
        status: &mut u32,
        packet: &mut [u8; PACKET],
        memory: Option<&dyn xapi::GuestMemory>,
    ) {
        if let Mode::Replaying { replayer, .. } = &mut self.mode {
            replayer.read(handle, status, packet);
        }
        let menu_port = self.menu_press.is_some() && self.ports.get(&handle) == Some(&0);
        if menu_port {
            self.expire_menu_press();
            if *status != 0 {
                self.cancel_menu("controller zero disconnected");
            } else if let Some(check) = self.menu_press.as_mut().and_then(|p| p.check.take())
                && !memory.is_some_and(check)
            {
                self.finish_menu(Err(RpcError::new(
                    code::GONE,
                    "menu focus changed before pad read",
                )));
            }
        }
        self.step(handle, packet);
        if menu_port && !self.pads[0].busy() {
            self.finish_menu(Ok(()));
        }
        if let Mode::Recording { recorder, .. } = &mut self.mode {
            recorder.read(handle, status, packet);
        }
        if let Some(f) = &mut self.flight {
            if f.reads.len() < agent_proto::trace::InputRead::MAX_PER_TICK {
                let port = self.ports.get(&handle).copied();
                f.reads.push((port, handle, *status, *packet));
            } else {
                f.dropped += 1;
            }
        }
    }

    /// The handle's port's current step merges over the read, which counts
    /// as one of the step's reads. The packet number moves whenever what
    /// the game gets changes, as a real pad's does; reads pass untouched
    /// until the first step.
    fn step(&mut self, handle: u32, packet: &mut [u8; PACKET]) {
        let remote = self
            .ports
            .get(&handle)
            .and_then(|&p| self.pads.get(p as usize))
            .filter(|r| r.busy());
        let stepped = remote.is_some();
        if let Some(r) = remote {
            let step = r.state_at_tick(Instant::now(), self.clock);
            r.state_read();
            let live = Gamepad::from_bytes(packet[4..].try_into().unwrap());
            packet[4..].copy_from_slice(&merge(live, step).to_bytes());
        }
        let d = self.delivered.entry(handle).or_default();
        let live = u32::from_le_bytes(packet[..4].try_into().unwrap());
        let mut number = live.wrapping_add(d.offset);
        let now: [u8; Gamepad::SIZE] = packet[4..].try_into().unwrap();
        let touched = stepped || d.offset != 0;
        if touched && d.seen && now != d.pad && number == d.number {
            d.offset = d.offset.wrapping_add(1);
            number = number.wrapping_add(1);
        }
        if touched {
            packet[..4].copy_from_slice(&number.to_le_bytes());
        }
        d.number = number;
        d.pad = now;
        d.seen = true;
    }
}

/// Where another tap (`H2_INPUT_RECORD` or `H2_INPUT_REPLAY`'s) goes
/// relative to the agent's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chain {
    /// Before: it stands in for the live pad (a replay), and the agent's
    /// steps apply on top.
    Before,
    /// After: it sees what the game gets (a recording), agent steps
    /// included.
    After,
}

/// The input tap: agent steps over the live (or replayed) pad.
pub struct AgentTap {
    input: Arc<Mutex<Input>>,
    inner: Option<(Box<dyn InputTap>, Chain)>,
}

impl AgentTap {
    pub fn menu_pad(&self) -> MenuPad {
        MenuPad(Arc::clone(&self.input))
    }

    pub(crate) fn new(input: Arc<Mutex<Input>>, inner: Option<(Box<dyn InputTap>, Chain)>) -> Self {
        lock(&input).tap = true;
        AgentTap { input, inner }
    }
}

pub(crate) fn lock(input: &Mutex<Input>) -> MutexGuard<'_, Input> {
    input.lock().unwrap_or_else(|e| e.into_inner())
}

impl InputTap for AgentTap {
    fn sync(&mut self, input: &mut input::InputManager) {
        match &mut self.inner {
            Some((tap, _)) => tap.sync(input),
            None => input.sync(),
        }
    }

    fn restore_input(&mut self, cursor: u64, input: &mut input::InputManager) -> Result<(), String> {
        match &mut self.inner {
            Some((tap, _)) => tap.restore_input(cursor, input),
            None => Ok(()),
        }
    }

    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        self.read_with_memory(handle, status, packet, None);
    }

    fn read_with_memory(
        &mut self,
        handle: u32,
        status: &mut u32,
        packet: &mut [u8; PACKET],
        memory: Option<&dyn xapi::GuestMemory>,
    ) {
        if let Some((t, Chain::Before)) = &mut self.inner {
            t.read_with_memory(handle, status, packet, memory);
        }
        lock(&self.input).read(handle, status, packet, memory);
        if let Some((t, Chain::After)) = &mut self.inner {
            t.read_with_memory(handle, status, packet, memory);
        }
    }

    fn opened(&mut self, handle: u32, port: u32) {
        {
            let mut i = lock(&self.input);
            i.ports.insert(handle, port);
            i.delivered.remove(&handle);
        }
        if let Some((t, _)) = &mut self.inner {
            t.opened(handle, port);
        }
    }

    fn closed(&mut self, handle: u32) {
        {
            let mut i = lock(&self.input);
            if i.ports.remove(&handle) == Some(0) {
                i.cancel_menu("controller zero closed");
            }
            i.delivered.remove(&handle);
        }
        if let Some((t, _)) = &mut self.inner {
            t.closed(handle);
        }
    }

    /// The chained recording's or replay's: the agent's own steps keep no
    /// log.
    fn cursor(&self) -> Option<u64> {
        self.inner.as_ref().and_then(|(t, _)| t.cursor())
    }

    fn seek(&mut self, cursor: u64) -> Result<(), String> {
        match &mut self.inner {
            Some((t, _)) => t.seek(cursor),
            None => Ok(()),
        }
    }
}

// ---- the methods ----

pub(crate) const DIGITAL: &[(&str, Buttons)] = &[
    ("up", Buttons::DPAD_UP),
    ("down", Buttons::DPAD_DOWN),
    ("left", Buttons::DPAD_LEFT),
    ("right", Buttons::DPAD_RIGHT),
    ("start", Buttons::START),
    ("back", Buttons::BACK),
    ("ls", Buttons::LEFT_THUMB),
    ("rs", Buttons::RIGHT_THUMB),
];

/// Pressure buttons and their `bAnalogButtons` index.
pub(crate) const ANALOG: &[(&str, usize)] = &[
    ("a", 0),
    ("b", 1),
    ("x", 2),
    ("y", 3),
    ("black", 4),
    ("white", 5),
    ("lt", 6),
    ("rt", 7),
];

/// `input.pad` steps as [`Remote`] steps.
pub fn pad_steps(steps: &[PadStep]) -> Result<Vec<Step>, String> {
    steps
        .iter()
        .map(|s| {
            s.check()?;
            let mut g = Gamepad::default();
            for b in &s.buttons {
                let b = b.to_ascii_lowercase();
                if let Some((_, bit)) = DIGITAL.iter().find(|(n, _)| *n == b) {
                    g.buttons.set(*bit, true);
                } else if let Some((_, i)) = ANALOG.iter().find(|(n, _)| *n == b) {
                    g.analog[*i] = 255;
                }
            }
            let pull = |v: f32| (v * 255.0).round() as u8;
            if let Some(v) = s.lt {
                g.analog[6] = pull(v);
            }
            if let Some(v) = s.rt {
                g.analog[7] = pull(v);
            }
            let axis = |v: f32| (v * 32767.0).round() as i16;
            (g.thumb_lx, g.thumb_ly) = (axis(s.left[0]), axis(s.left[1]));
            (g.thumb_rx, g.thumb_ry) = (axis(s.right[0]), axis(s.right[1]));
            Ok(Step {
                pad: g,
                hold: s.ms.map(Duration::from_millis),
                ticks: s.ticks,
                at_tick: None,
                reads: s.reads.unwrap_or(1),
            })
        })
        .collect()
}

/// A port's pad: what is held, and what is queued.
fn pad_state(r: &Remote) -> Value {
    let s = r.status();
    let g = s.pad;
    let mut buttons: Vec<&str> = Vec::new();
    for (name, bit) in DIGITAL {
        if g.buttons.contains(*bit) {
            buttons.push(name);
        }
    }
    for (name, i) in ANALOG {
        if g.analog[*i] > 0 {
            buttons.push(name);
        }
    }
    let axis = |v: i16| (f64::from(v) / 32767.0 * 1000.0).round() / 1000.0;
    json!({
        "active": s.active,
        "queued": s.queued,
        "done": s.done,
        "buttons": buttons,
        "left": [axis(g.thumb_lx), axis(g.thumb_ly)],
        "right": [axis(g.thumb_rx), axis(g.thumb_ry)],
    })
}

/// `input.state`: player one's pad as `pad` (as before ports), every
/// port's in `pads`, the open ports, and recording or replay.
fn state(i: &Input) -> Value {
    let mut v = json!({
        "pad": pad_state(&i.pads[0]),
        "pads": i.pads.iter().map(pad_state).collect::<Vec<_>>(),
        "open": i.open_ports(),
        "tick": i.clock,
    });
    if let (Value::Object(o), Value::Object(m)) = (&mut v, serde_json::to_value(i.mode()).unwrap())
    {
        o.extend(m);
    }
    v
}

fn params<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn player(p: u32) -> Result<usize, RpcError> {
    if p < PORTS {
        Ok(p as usize)
    } else {
        Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("player must be 0 to {}", PORTS - 1),
        ))
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseParams {
    #[serde(default)]
    player: Option<u32>,
}

/// Where `name`'s input log lives under `dir`.
pub fn log_path(dir: &Path, name: &str) -> Result<PathBuf, RpcError> {
    if !agent_proto::valid_log_name(name) {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("{name:?}: a log name is letters, digits, '.', '_' and '-'"),
        ));
    }
    Ok(dir.join("inputs").join(format!("{name}.input")))
}

/// Ends a recording (writing its file) or a replay.
pub(crate) fn stop_and_write(input: &Mutex<Input>) -> Result<InputMode, RpcError> {
    let stopped = lock(input).stop();
    let Some((ended, file)) = stopped else {
        return Err(RpcError::new(
            code::INVALID_REQUEST,
            "nothing is recording or replaying",
        ));
    };
    if let Some((path, log)) = file {
        let write = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, log.to_text()));
        if let Err(e) = write {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                format!("writing {}: {e}", path.display()),
            ));
        }
    }
    Ok(ended)
}

/// Registers `input.pad`, `input.release`, `input.state`, `input.record`,
/// `input.replay` and `input.stop` on `agent`, over `input`. Acting needs
/// no claim unless another connection holds control ([`Access::Act`]).
pub(crate) fn add_input_methods(agent: &Agent, input: &Arc<Mutex<Input>>) {
    let i = Arc::clone(input);
    agent.add_method(method::INPUT_PAD, Access::Act, move |v| {
        let p: PadParams = params(v)?;
        let port = player(p.player)?;
        let mut steps = pad_steps(&p.steps).map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
        if let (Some(AtTick::Tick(t)), Some(first)) = (p.at_tick, steps.first_mut()) {
            first.at_tick = Some(t);
        }
        let n = steps.len();
        let mut i = lock(&i);
        if port == 0 && i.menu_press.is_some() {
            if p.append {
                return Err(RpcError::new(code::BUSY, "menu press in progress"));
            }
            i.cancel_menu("pad queue replaced");
        }
        let r = &i.pads[port];
        if p.append {
            r.append(steps);
        } else {
            r.replace(steps);
        }
        Ok(json!({
            "queued": n,
            "player": p.player,
            "open": i.open_ports().contains(&p.player),
            "pad": pad_state(r),
        }))
    });
    let i = Arc::clone(input);
    agent.add_method(method::INPUT_RELEASE, Access::Act, move |v| {
        let p: ReleaseParams = params(v)?;
        let mut i = lock(&i);
        if p.player.unwrap_or(0) == 0 {
            i.cancel_menu("pad queue released");
        }
        match p.player {
            Some(port) => i.pads[player(port)?].clear(),
            None => i.pads.iter().for_each(Remote::clear),
        }
        Ok(json!({ "pad": pad_state(&i.pads[0]) }))
    });
    let i = Arc::clone(input);
    agent.add_method(method::INPUT_STATE, Access::Read, move |_| {
        Ok(state(&lock(&i)))
    });
    let i = Arc::clone(input);
    agent.add_method(method::INPUT_RECORD, Access::Act, move |v| {
        let p: RecordParams = params(v)?;
        let path = log_path(&lock(&i).dir(), &p.name)?;
        if path.exists() && !p.replace {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("{} exists; \"replace\": true overwrites it", path.display()),
            ));
        }
        let mut i = lock(&i);
        i.record(&p.name, path)
            .map_err(|e| RpcError::new(code::INVALID_REQUEST, e))?;
        Ok(state(&i))
    });
    let i = Arc::clone(input);
    agent.add_method(method::INPUT_REPLAY, Access::Act, move |v| {
        let p: ReplayParams = params(v)?;
        let path = log_path(&lock(&i).dir(), &p.name)?;
        let bad =
            |e: String| RpcError::new(code::INVALID_PARAMS, format!("{}: {e}", path.display()));
        let text = std::fs::read_to_string(&path).map_err(|e| bad(e.to_string()))?;
        let log = InputLog::parse(&text).map_err(bad)?;
        let mut i = lock(&i);
        i.replay(&p.name, path.clone(), log)
            .map_err(|e| RpcError::new(code::INVALID_REQUEST, e))?;
        Ok(state(&i))
    });
    let i = Arc::clone(input);
    agent.add_method(method::INPUT_STOP, Access::Act, move |_| {
        stop_and_write(&i).map(|m| serde_json::to_value(m).unwrap())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use input::gamepad::State;

    const H0: u32 = 0x5800_0000;
    const H1: u32 = 0x5800_0004;

    fn live(number: u32, lx: i16) -> [u8; PACKET] {
        State {
            packet_number: number,
            gamepad: Gamepad {
                thumb_lx: lx,
                ..Default::default()
            },
        }
        .to_bytes()
    }

    fn read(tap: &mut AgentTap, handle: u32, from: [u8; PACKET]) -> (u32, Gamepad) {
        let (mut status, mut p) = (0, from);
        tap.read(handle, &mut status, &mut p);
        let number = u32::from_le_bytes(p[..4].try_into().unwrap());
        (number, Gamepad::from_bytes(p[4..].try_into().unwrap()))
    }

    fn tap() -> (Arc<Mutex<Input>>, AgentTap) {
        let input = Arc::new(Mutex::new(Input::default()));
        let mut t = AgentTap::new(Arc::clone(&input), None);
        t.opened(H0, 0);
        t.opened(H1, 1);
        (input, t)
    }

    fn steps(v: Value) -> Vec<Step> {
        pad_steps(&serde_json::from_value::<Vec<PadStep>>(v).unwrap()).unwrap()
    }

    #[test]
    fn menu_ack_waits_for_release_and_checks_focus_at_the_actual_read() {
        let (_input, mut tap) = tap();
        let menu = tap.menu_pad();
        let mut guest = runtime::Guest::new();
        let mem = xapi::register::RuntimeMemory::new(&mut guest);
        let steps: Vec<PadStep> = serde_json::from_value(json!([
            {"buttons": ["a"], "ms": 0, "reads": 1}, {"ms": 0, "reads": 1}
        ]))
        .unwrap();
        let poll = |tap: &mut AgentTap| {
            let mut packet = live(0, 0);
            let mut status = 0;
            tap.read_with_memory(H0, &mut status, &mut packet, Some(&mem));
            Gamepad::from_bytes(packet[4..].try_into().unwrap()).analog[0]
        };
        let stale = menu.queue(1, &steps, |_| false).unwrap();
        assert_eq!(poll(&mut tap), 0);
        assert_eq!(stale.result().unwrap().unwrap_err().code, code::GONE);

        let press = menu.queue(1, &steps, |_| true).unwrap();
        assert_eq!(poll(&mut tap), 255);
        assert!(press.result().is_none());
        assert_eq!(poll(&mut tap), 0);
        assert_eq!(poll(&mut tap), 0);
        assert_eq!(press.result(), Some(Ok(())));
    }

    #[test]
    fn expired_cancelled_and_abandoned_gestures_leave_no_button() {
        let (input, mut tap) = tap();
        let menu = tap.menu_pad();
        let steps = vec![PadStep {
            buttons: vec!["a".into()],
            ..Default::default()
        }];
        let expired = menu.queue(1, &steps, |_| true).unwrap();
        lock(&input).menu_press.as_mut().unwrap().deadline = Instant::now();
        assert_eq!(read(&mut tap, H0, live(0, 0)).1.analog[0], 0);
        assert_eq!(expired.result().unwrap().unwrap_err().code, code::TIMEOUT);

        let cancelled = menu.queue(1, &steps, |_| true).unwrap();
        assert_eq!(
            menu.queue(2, &steps, |_| true).err().unwrap().code,
            code::BUSY
        );
        lock(&input).release_menu(1);
        assert_eq!(
            cancelled.result().unwrap().unwrap_err().code,
            code::INTERRUPTED
        );
        drop(menu.queue(2, &steps, |_| true).unwrap());
        assert!(!lock(&input).pads[0].busy());
        tap.closed(H0);
        assert_eq!(
            menu.queue(1, &steps, |_| true).err().unwrap().code,
            code::UNAVAILABLE
        );
    }

    #[test]
    fn pad_steps_become_reports() {
        let steps: Vec<PadStep> = serde_json::from_value(json!([
            {"buttons": ["A", "start", "rt"], "left": [0.5, -1], "ms": 120},
            {"right": [1, 0], "lt": 0.5, "reads": 0, "ticks": 3}
        ]))
        .unwrap();
        let s = pad_steps(&steps).unwrap();
        assert_eq!(s[0].pad.analog[0], 255);
        assert_eq!(s[0].pad.analog[7], 255);
        assert!(s[0].pad.buttons.contains(Buttons::START));
        assert_eq!((s[0].pad.thumb_lx, s[0].pad.thumb_ly), (16384, -32767));
        assert_eq!(s[0].hold, Some(Duration::from_millis(120)));
        assert_eq!(s[0].reads, 1);
        assert_eq!(s[1].pad.thumb_rx, 32767);
        assert_eq!(s[1].pad.analog[6], 128);
        assert_eq!((s[1].hold, s[1].reads, s[1].ticks), (None, 0, Some(3)));
        let bad: Vec<PadStep> = serde_json::from_value(json!([{"buttons": ["jump"]}])).unwrap();
        assert!(pad_steps(&bad).is_err());
        // Every name the protocol offers maps to something.
        for b in agent_proto::PAD_BUTTONS {
            assert!(
                DIGITAL.iter().any(|(n, _)| n == b) || ANALOG.iter().any(|(n, _)| n == b),
                "{b}"
            );
        }
    }

    #[test]
    fn a_step_holds_for_its_ticks_on_its_player_only_merged_over_the_live_pad() {
        let (input, mut t) = tap();
        lock(&input).set_clock(10);
        lock(&input).pads[1].replace(steps(json!([{"buttons": ["a"], "ticks": 2}])));
        // Player 0 is untouched, byte for byte.
        let (mut s, mut p) = (0, live(7, 300));
        t.read(H0, &mut s, &mut p);
        assert_eq!(p, live(7, 300));
        // Player 1 gets the step for ticks 10 and 11, over its live stick.
        for tick in [10, 11] {
            lock(&input).set_clock(tick);
            let (_, g) = read(&mut t, H1, live(7, 300));
            assert_eq!((g.analog[0], g.thumb_lx), (255, 300), "tick {tick}");
        }
        lock(&input).set_clock(12);
        let (_, g) = read(&mut t, H1, live(7, 300));
        assert_eq!((g.analog[0], g.thumb_lx), (0, 300));
        assert!(!lock(&input).pads[1].busy(), "a finished step is gone");
    }

    #[test]
    fn a_step_waits_for_its_tick_and_needs_its_reads() {
        let (input, mut t) = tap();
        lock(&input).set_clock(5);
        let mut s = steps(json!([{"buttons": ["x"], "ticks": 1, "reads": 2}]));
        s[0].at_tick = Some(8);
        lock(&input).pads[0].replace(s);
        let pressed = |t: &mut AgentTap, tick| {
            lock(&input).set_clock(tick);
            read(t, H0, live(1, 0)).1.analog[2]
        };
        assert_eq!(pressed(&mut t, 7), 0);
        assert_eq!(pressed(&mut t, 8), 255);
        // Its tick is over, but the game read it only once.
        assert_eq!(pressed(&mut t, 9), 255);
        assert_eq!(pressed(&mut t, 9), 0);
    }

    #[test]
    fn the_packet_number_moves_when_the_agent_changes_the_pad() {
        let (input, mut t) = tap();
        // The live pad's number doesn't move (nobody touches it), but the
        // pad the game gets does: so must the number.
        assert_eq!(read(&mut t, H0, live(40, 0)).0, 40);
        lock(&input).set_clock(0);
        lock(&input).pads[0].replace(steps(json!([{"buttons": ["b"], "ticks": 1}])));
        let (n1, _) = read(&mut t, H0, live(40, 0));
        assert_eq!(n1, 41);
        let (n2, _) = read(&mut t, H0, live(40, 0));
        assert_eq!(n2, 41, "same pad, same number");
        lock(&input).set_clock(1);
        let (n3, g) = read(&mut t, H0, live(40, 0));
        assert_eq!((n3, g.analog[1]), (42, 0), "released");
        assert_eq!(read(&mut t, H0, live(43, 5)).0, 45, "live moves keep it");
    }

    #[test]
    fn a_recording_replays_read_for_read_with_the_agent_steps_in_it() {
        let (input, mut t) = tap();
        lock(&input).set_clock(20);
        lock(&input)
            .record("fight", "/c/fight.input".into())
            .unwrap();
        assert!(lock(&input).record("again", "/c/x".into()).is_err());
        lock(&input).pads[0].replace(steps(json!([{"buttons": ["a"], "ticks": 2}])));
        let mut seen = Vec::new();
        for tick in 20..24 {
            lock(&input).set_clock(tick);
            seen.push(read(&mut t, H0, live(3, tick as i16)));
            seen.push(read(&mut t, H1, live(3, -1)));
        }
        let InputMode::Recording {
            reads, from_tick, ..
        } = lock(&input).mode()
        else {
            panic!("recording")
        };
        assert_eq!((reads, from_tick), (8, Some(20)));
        let (ended, file) = lock(&input).stop().unwrap();
        assert!(matches!(ended, InputMode::Recording { reads: 8, .. }));
        let (path, log) = file.unwrap();
        assert_eq!(path, PathBuf::from("/c/fight.input"));
        assert_eq!(log.reads[0].tick, Some(20), "stamped with the game tick");
        assert!(lock(&input).stop().is_none(), "live again");

        // Replayed over a live pad that does something else entirely.
        let log = InputLog::parse(&log.to_text()).unwrap();
        lock(&input)
            .replay("fight", "/c/fight.input".into(), log)
            .unwrap();
        let mut got = Vec::new();
        for _ in 20..24 {
            got.push(read(&mut t, H0, live(77, 999)));
            got.push(read(&mut t, H1, live(77, 999)));
        }
        assert_eq!(got, seen);
        read(&mut t, H0, live(77, 999));
        let InputMode::Replaying { counts, .. } = lock(&input).mode() else {
            panic!("replaying")
        };
        assert_eq!(
            counts,
            ReplayCounts {
                reads: 8,
                replayed: 8,
                matched: 8,
                handle_mismatches: 0,
                past_end: 1
            }
        );
    }

    #[test]
    fn a_chained_replay_comes_first_and_a_chained_recording_sees_the_agent() {
        use xapi::tap::Read;
        let input = Arc::new(Mutex::new(Input::default()));
        let (rec, log) = Recorder::new();
        let mut t = AgentTap::new(Arc::clone(&input), Some((Box::new(rec), Chain::After)));
        t.opened(H0, 0);
        lock(&input).pads[0].replace(steps(json!([{"buttons": ["a"]}])));
        read(&mut t, H0, live(1, 0));
        let got = log.lock().unwrap().reads[0].packet;
        assert_eq!(
            Gamepad::from_bytes(got[4..].try_into().unwrap()).analog[0],
            255
        );

        let replay = InputLog {
            reads: vec![Read {
                tick: None,
                handle: H0,
                status: 0,
                packet: live(9, 1234),
            }],
        };
        let (rp, _) = Replayer::new(replay);
        let input = Arc::new(Mutex::new(Input::default()));
        let mut t = AgentTap::new(Arc::clone(&input), Some((Box::new(rp), Chain::Before)));
        t.opened(H0, 0);
        let (n, g) = read(&mut t, H0, live(1, 0));
        assert_eq!((n, g.thumb_lx), (9, 1234), "the replay stands in for live");
    }
}
