//! `trace.*` and `flight.*` (docs/runtime/agent-api.md, "Tick traces and
//! the flight recorder"): rows of chosen objects taken inside the game at
//! every tick boundary, so none is lost to a slow client.
//!
//! * A trace streams each row ([`agent_proto::trace::TickRow`]) to a JSONL
//!   file as the tick ends: memory stays one buffered writer however long
//!   it runs. The file is flushed every second of ticks, at `trace.stop`
//!   and when the game exits.
//! * The flight recorder keeps the last N seconds of rows in a ring, each
//!   with the controller reads of its tick (taken at the input tap,
//!   [`crate::tap::FlightReads`]), bounded by rows and by bytes;
//!   `flight.dump` writes the ring out.
//!
//! [`Recorders`] lives in the executor's state; the executor calls
//! [`Recorders::on_tick`] at each tick and routes the methods here.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent_proto::trace::{
    DumpParams, FlightParams, FlightStatus, InputRead, MAX_OBJECTS, TICKS_PER_SECOND, TickRow,
    TraceParams, TraceStatus, method,
};
use agent_proto::{At, OBJECT_FIELDS, Object, ObjectId, RpcError, code};
use input::gamepad::Gamepad;
use serde_json::Value;

use crate::reader::Reader;
use crate::tap::{self, ANALOG, DIGITAL, FlightReads, Input};

/// Rows written between flushes of a trace file (one second of ticks).
const FLUSH_ROWS: u64 = TICKS_PER_SECOND as u64;
/// The flight ring's bytes at most: past this the oldest rows go even
/// before `seconds` is reached.
pub(crate) const FLIGHT_MAX_BYTES: usize = 64 << 20;

/// What a trace or flight recorder keeps of each tick.
#[derive(Debug, Clone)]
struct Selection {
    objects: Vec<String>,
    fields: Option<Vec<String>>,
}

impl Selection {
    fn new(objects: Vec<String>, fields: Option<Vec<String>>) -> Result<Selection, RpcError> {
        let bad = |m: String| Err(RpcError::new(code::INVALID_PARAMS, m));
        if objects.is_empty() || objects.len() > MAX_OBJECTS {
            return bad(format!("objects must name 1 to {MAX_OBJECTS} ids"));
        }
        if let Some(id) = objects.iter().find(|id| ObjectId::parse(id).is_none()) {
            return bad(format!("{id} is not an id (player.N or kind.xxxxxxxx)"));
        }
        if let Some(f) = &fields
            && let Some(x) = f.iter().find(|f| !OBJECT_FIELDS.contains(&f.as_str()))
        {
            return bad(format!(
                "no field {x}; fields are {}",
                OBJECT_FIELDS.join(", ")
            ));
        }
        let mut seen = Vec::with_capacity(objects.len());
        for id in objects {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        Ok(Selection {
            objects: seen,
            fields,
        })
    }

    /// This tick's row from the world (or the read's failure).
    fn row(&self, tick: &Tick, world: &Result<Vec<Object>, String>) -> TickRow {
        let mut row = TickRow {
            tick: tick.tick,
            ticks: tick.ticks,
            map: tick.map.clone(),
            ..Default::default()
        };
        match world {
            Err(e) => row.error = Some(e.clone()),
            Ok(objects) => {
                for id in &self.objects {
                    match objects.iter().find(|o| &o.id == id) {
                        Some(o) => {
                            let v = serde_json::to_value(o).expect("objects serialise");
                            row.objects
                                .insert(id.clone(), crate::exec::trim(v, self.fields.as_deref()));
                        }
                        None => row.gone.push(id.clone()),
                    }
                }
            }
        }
        row
    }
}

/// The tick a row is taken at.
#[derive(Debug, Clone, Default)]
pub(crate) struct Tick {
    /// The agent's count of ticks.
    pub ticks: u64,
    /// The game's own tick.
    pub tick: Option<u64>,
    pub map: Option<String>,
}

struct Trace {
    sel: Selection,
    path: PathBuf,
    /// `None` once stopped (or failed).
    out: Option<BufWriter<File>>,
    rows: u64,
    bytes: u64,
    first_tick: Option<u64>,
    last_tick: Option<u64>,
    error: Option<String>,
}

impl Trace {
    fn write(&mut self, row: &TickRow) {
        let Some(out) = self.out.as_mut() else { return };
        let mut line = serde_json::to_string(row).expect("rows serialise");
        line.push('\n');
        let wrote = out.write_all(line.as_bytes()).and_then(|()| {
            if (self.rows + 1).is_multiple_of(FLUSH_ROWS) {
                out.flush()
            } else {
                Ok(())
            }
        });
        match wrote {
            Ok(()) => {
                self.rows += 1;
                self.bytes += line.len() as u64;
                self.first_tick = self.first_tick.or(row.tick);
                self.last_tick = row.tick.or(self.last_tick);
            }
            Err(e) => self.fail(e),
        }
    }

    fn fail(&mut self, e: std::io::Error) {
        self.error = Some(format!("writing {}: {e}", self.path.display()));
        self.out = None;
    }

    /// Flushes and closes the file.
    fn stop(&mut self) {
        if let Some(mut out) = self.out.take()
            && let Err(e) = out.flush()
        {
            self.fail(e);
        }
    }

    fn status(&self, at: At) -> TraceStatus {
        TraceStatus {
            at,
            active: self.out.is_some(),
            path: Some(self.path.display().to_string()),
            objects: self.sel.objects.clone(),
            fields: self.sel.fields.clone(),
            rows: self.rows,
            bytes: self.bytes,
            first_tick: self.first_tick,
            last_tick: self.last_tick,
            error: self.error.clone(),
        }
    }
}

struct Flight {
    sel: Selection,
    seconds: u32,
    capacity: usize,
    max_bytes: usize,
    /// Each row's game tick and its JSON line.
    ring: VecDeque<(Option<u64>, String)>,
    bytes: usize,
    ticks: u64,
}

impl Flight {
    fn push(&mut self, row: &TickRow) {
        let line = serde_json::to_string(row).expect("rows serialise");
        self.bytes += line.len();
        self.ring.push_back((row.tick, line));
        self.ticks += 1;
        while self.ring.len() > self.capacity
            || (self.bytes > self.max_bytes && self.ring.len() > 1)
        {
            if let Some((_, old)) = self.ring.pop_front() {
                self.bytes -= old.len();
            }
        }
    }

    fn status(&self, at: At, active: bool) -> FlightStatus {
        FlightStatus {
            at,
            active,
            objects: self.sel.objects.clone(),
            fields: self.sel.fields.clone(),
            seconds: self.seconds,
            capacity: self.capacity,
            rows: self.ring.len(),
            bytes: self.bytes,
            ticks: self.ticks,
            first_tick: self.ring.front().and_then(|r| r.0),
            last_tick: self.ring.back().and_then(|r| r.0),
            path: None,
        }
    }

    fn dump(&self, path: &Path) -> std::io::Result<()> {
        let mut out = BufWriter::new(create(path)?);
        for (_, line) in &self.ring {
            out.write_all(line.as_bytes())?;
            out.write_all(b"\n")?;
        }
        out.flush()
    }
}

/// The trace and the flight recorder, as the executor holds them.
#[derive(Default)]
pub(crate) struct Recorders {
    /// The running trace, or the last one (stopped) for `trace.status`.
    trace: Option<Trace>,
    flight: Option<Flight>,
}

/// Whether `m` is answered here.
pub(crate) fn is_method(m: &str) -> bool {
    method::ALL.contains(&m)
}

impl Recorders {
    /// Something wants this tick's row.
    pub fn active(&self) -> bool {
        self.trace.as_ref().is_some_and(|t| t.out.is_some()) || self.flight.is_some()
    }

    /// At each tick boundary, before requests are served: the trace's row
    /// and the flight recorder's, with the controller reads since the last
    /// tick. Reads the world once.
    pub fn on_tick(&mut self, tick: &Tick, reader: &mut dyn Reader, input: &Mutex<Input>) {
        if !self.active() {
            return;
        }
        let world = reader.objects();
        if let Some(t) = self.trace.as_mut().filter(|t| t.out.is_some()) {
            t.write(&t.sel.row(tick, &world));
        }
        if let Some(f) = self.flight.as_mut() {
            let mut row = f.sel.row(tick, &world);
            let mut i = tap::lock(input);
            if i.tap {
                let reads = i.flight.replace(FlightReads::default()).unwrap_or_default();
                row.input = Some(reads.reads.iter().map(input_read).collect());
                row.input_dropped = reads.dropped;
            }
            drop(i);
            f.push(&row);
        }
    }

    /// The game is exiting: a trace's file is flushed.
    pub fn shutdown(&mut self) {
        if let Some(t) = self.trace.as_mut() {
            t.stop();
        }
    }

    /// Answers a `trace.*` or `flight.*` method. `dir` is the cache
    /// directory (files go under its `traces/` and `flights/`).
    pub fn handle(
        &mut self,
        m: &str,
        p: Value,
        at: At,
        dir: &Path,
        input: &Mutex<Input>,
    ) -> Result<Value, RpcError> {
        match m {
            method::TRACE_START => {
                let p: TraceParams = params(p)?;
                if self.trace.as_ref().is_some_and(|t| t.out.is_some()) {
                    return Err(RpcError::new(
                        code::UNAVAILABLE,
                        "a trace is already running; trace.stop first",
                    ));
                }
                let sel = Selection::new(p.objects, p.fields)?;
                let path = place(dir, "traces", "trace", p.path.as_deref());
                let file = create(&path).map_err(|e| {
                    RpcError::new(
                        code::UNAVAILABLE,
                        format!("creating {}: {e}", path.display()),
                    )
                })?;
                let t = Trace {
                    sel,
                    path,
                    out: Some(BufWriter::with_capacity(64 << 10, file)),
                    rows: 0,
                    bytes: 0,
                    first_tick: None,
                    last_tick: None,
                    error: None,
                };
                let v = to_value(t.status(at));
                self.trace = Some(t);
                Ok(v)
            }
            method::TRACE_STOP => {
                let t = self.trace.as_mut().filter(|t| t.out.is_some()).ok_or_else(|| {
                    RpcError::new(code::INVALID_REQUEST, "no trace is running")
                })?;
                t.stop();
                Ok(to_value(t.status(at)))
            }
            method::TRACE_STATUS => Ok(match &self.trace {
                Some(t) => to_value(t.status(at)),
                None => serde_json::json!({ "at": at, "active": false }),
            }),
            method::FLIGHT_START => {
                let p: FlightParams = params(p)?;
                if self.flight.is_some() {
                    return Err(RpcError::new(
                        code::UNAVAILABLE,
                        "the flight recorder is already running; flight.stop first",
                    ));
                }
                let seconds = p.seconds.unwrap_or(FlightParams::DEFAULT_SECONDS);
                if !(1..=FlightParams::MAX_SECONDS).contains(&seconds) {
                    return Err(RpcError::new(
                        code::INVALID_PARAMS,
                        format!("seconds must be 1 to {}", FlightParams::MAX_SECONDS),
                    ));
                }
                let sel = Selection::new(p.objects, p.fields)?;
                let capacity = (seconds * TICKS_PER_SECOND) as usize;
                let f = Flight {
                    sel,
                    seconds,
                    capacity,
                    max_bytes: FLIGHT_MAX_BYTES,
                    ring: VecDeque::with_capacity(capacity.min(4096)),
                    bytes: 0,
                    ticks: 0,
                };
                // Reads from now on; none from before the start.
                tap::lock(input).flight = Some(FlightReads::default());
                let v = to_value(f.status(at, true));
                self.flight = Some(f);
                Ok(v)
            }
            method::FLIGHT_DUMP => {
                let p: DumpParams = params(p)?;
                let f = self.flight.as_ref().ok_or_else(not_flying)?;
                let path = place(dir, "flights", "flight", p.path.as_deref());
                f.dump(&path).map_err(|e| {
                    RpcError::new(
                        code::UNAVAILABLE,
                        format!("writing {}: {e}", path.display()),
                    )
                })?;
                let mut s = f.status(at, true);
                s.path = Some(path.display().to_string());
                Ok(to_value(s))
            }
            method::FLIGHT_STOP => {
                let f = self.flight.take().ok_or_else(not_flying)?;
                tap::lock(input).flight = None;
                Ok(to_value(f.status(at, false)))
            }
            method::FLIGHT_STATUS => Ok(match &self.flight {
                Some(f) => to_value(f.status(at, true)),
                None => serde_json::json!({ "at": at, "active": false }),
            }),
            other => Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("no method {other}"),
            )),
        }
    }
}

fn not_flying() -> RpcError {
    RpcError::new(
        code::INVALID_REQUEST,
        "the flight recorder isn't running; flight.start first",
    )
}

/// `path` as asked (relative ones under `dir/sub`), or a fresh name there.
fn place(dir: &Path, sub: &str, stem: &str, path: Option<&str>) -> PathBuf {
    match path {
        Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
        Some(p) => dir.join(sub).join(p),
        None => {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            dir.join(sub).join(format!("{stem}-{ms}.jsonl"))
        }
    }
}

fn create(path: &Path) -> std::io::Result<File> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    File::create(path)
}

/// One kept read, named as `input.state` names a pad.
fn input_read(r: &(Option<u32>, u32, u32, [u8; xapi::tap::PACKET])) -> InputRead {
    let (port, handle, status, packet) = *r;
    let pad: [u8; Gamepad::SIZE] = packet[4..].try_into().expect("a packet holds a pad");
    let g = Gamepad::from_bytes(&pad);
    let mut buttons: Vec<String> = DIGITAL
        .iter()
        .filter(|(_, bit)| g.buttons.contains(*bit))
        .map(|(n, _)| (*n).to_owned())
        .collect();
    buttons.extend(
        ANALOG
            .iter()
            .filter(|(_, i)| g.analog[*i] > 0)
            .map(|(n, _)| (*n).to_owned()),
    );
    let axis = |v: i16| (f64::from(v) / 32767.0 * 1000.0).round() / 1000.0;
    InputRead {
        port,
        handle,
        status,
        packet: u32::from_le_bytes(packet[..4].try_into().expect("4 bytes")),
        buttons,
        left: [axis(g.thumb_lx), axis(g.thumb_ly)],
        right: [axis(g.thumb_rx), axis(g.thumb_ry)],
        raw: pad.iter().map(|b| format!("{b:02x}")).collect(),
    }
}

fn params<T: serde::de::DeserializeOwned>(p: Value) -> Result<T, RpcError> {
    let p = if p.is_null() {
        serde_json::json!({})
    } else {
        p
    };
    serde_json::from_value(p).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("results serialise")
}

#[cfg(test)]
mod tests;
