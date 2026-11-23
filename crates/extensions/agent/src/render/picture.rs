//! `render.picture`: the game's presented frames for a client to show
//! (docs/runtime/agent-api.md, "Picture feed"; docs/editor/game-view.md).
//!
//! [`Pictures`] holds the open pictures. A worker thread, started with the
//! first and gone with the last, reads the newest presented frame back
//! through a [`FrameFeed`] (the window shell's readback, attached at
//! [`agent_proto::picture::hub`]) when some picture is due, scales it to
//! each picture's width and hands it on: into a ring's next slot, with a
//! `picture` event to the connection that opened it, or as a JPEG onto a
//! stream's binary queue. Nothing here runs on the game thread except
//! opening, closing and the stamp.
//!
//! The idle skip: a ring whose newest frame stays unread for
//! [`IDLE_AFTER`] gets no frames, and a stream whose queue is full gets
//! none, so a client that stops reading costs the game no readbacks.
//! When nothing is due the worker reads nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use agent_proto::picture::{
    self as wire, Grabbed, Opened, PictureEvent, Ring, SlotMeta, Transport,
};
use agent_proto::{Notification, RpcError, code};

use crate::ConnId;
use crate::live::{FramesTx, Sent};

/// A ring whose newest frame nobody took for this long gets no more
/// until it is read.
pub const IDLE_AFTER: Duration = Duration::from_secs(1);
/// Stream frames in flight to the socket writer at most.
pub(crate) const STREAM_QUEUE: usize = 3;
const JPEG_QUALITY: u8 = 75;
/// How long the worker waits when the source has no new frame yet.
const NO_FRAME_POLL: Duration = Duration::from_millis(2);
/// How long the worker waits after the source failed.
const FAILED_POLL: Duration = Duration::from_millis(100);
/// How often an idle picture is looked at again.
const IDLE_POLL: Duration = Duration::from_millis(50);

/// Where frames come from.
pub trait FrameFeed: Send + Sync {
    /// `Err` when there is no source to open a picture on.
    fn ready(&self) -> Result<(), String>;
    /// The newest presented frame unless it is `after` ([`Grabbed::seq`]).
    fn grab(&self, after: Option<u64>) -> Result<Option<Grabbed>, String>;
}

/// The process's frame source: the hub the window shell attaches to.
struct HubFeed;

impl FrameFeed for HubFeed {
    fn ready(&self) -> Result<(), String> {
        if wire::hub().attached() {
            Ok(())
        } else {
            Err(wire::NO_SOURCE.into())
        }
    }

    fn grab(&self, after: Option<u64>) -> Result<Option<Grabbed>, String> {
        wire::hub().grab(after)
    }
}

/// Queues one JSON line for the opening connection; `false` when its
/// queue is full (the event is dropped; the frame is still in the ring).
pub(crate) type EventSink = Box<dyn Fn(String) -> bool + Send + Sync>;

enum Out {
    Ring {
        ring: Ring,
        path: PathBuf,
        events: Option<EventSink>,
    },
    Stream(FramesTx),
}

struct Entry {
    conn: ConnId,
    width: u32,
    interval: Duration,
    out: Arc<Out>,
    /// Frames go out once the reply has (ordering on the connection).
    started: bool,
    opened: Instant,
    next_due: Instant,
    frame: u64,
    /// The ring's read acknowledgement as last seen, and when it moved.
    read_seen: u64,
    read_moved: Instant,
    events_dropped: u64,
}

impl Entry {
    /// Skipped: a ring unread for [`IDLE_AFTER`], a full stream queue.
    fn idle(&mut self, now: Instant) -> bool {
        match &*self.out {
            Out::Ring { ring, .. } => {
                let read = ring.read_ack();
                if read != self.read_seen {
                    self.read_seen = read;
                    self.read_moved = now;
                }
                self.frame > read && now.saturating_duration_since(self.read_moved) >= IDLE_AFTER
            }
            Out::Stream(tx) => tx.queued() >= STREAM_QUEUE as u64,
        }
    }
}

impl Drop for Out {
    fn drop(&mut self) {
        match self {
            Out::Ring { ring, path, .. } => {
                ring.mark_closed();
                let _ = std::fs::remove_file(&*path);
            }
            Out::Stream(tx) => tx.close(),
        }
    }
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    open: BTreeMap<u64, Entry>,
    /// The source's number of the newest frame read back.
    last_seq: Option<u64>,
    worker: bool,
    error: Option<String>,
}

/// What one pass of the worker did ([`Pictures::pump`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pump {
    /// No picture is open.
    Closed,
    /// Nothing due before this instant.
    Wait(Instant),
    /// Pictures were due but the source has no frame newer than the last.
    NoFrame,
    /// The source failed.
    Failed(String),
    /// A frame went to this many pictures.
    Published(usize),
}

/// The open pictures. One per [`crate::Agent`]; tests make their own.
pub struct Pictures {
    feed: Arc<dyn FrameFeed>,
    dir: PathBuf,
    /// Run a worker thread (tests drive [`Pictures::pump`] themselves).
    worker: bool,
    inner: Mutex<Inner>,
    wake: Condvar,
    /// The game's tick (`u64::MAX`: none) and presents, for the stamps.
    tick: AtomicU64,
    present: AtomicU64,
    /// Frames read back from the source.
    readbacks: AtomicU64,
}

impl Default for Pictures {
    fn default() -> Self {
        Pictures::new(
            Arc::new(HubFeed),
            agent_proto::cache_dir().join("pictures"),
            true,
        )
    }
}

impl Pictures {
    /// Pictures reading `feed`, keeping ring files in `dir`.
    pub fn new(feed: Arc<dyn FrameFeed>, dir: PathBuf, worker: bool) -> Pictures {
        Pictures {
            feed,
            dir,
            worker,
            inner: Mutex::new(Inner::default()),
            wake: Condvar::new(),
            tick: AtomicU64::new(u64::MAX),
            present: AtomicU64::new(0),
            readbacks: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The game's stamp for the next frames (the game thread, each tick).
    pub fn stamp(&self, tick: Option<u64>, present: u64) {
        self.tick.store(tick.unwrap_or(u64::MAX), Ordering::Relaxed);
        self.present.store(present, Ordering::Relaxed);
    }

    /// Frames read back from the source so far.
    pub fn readbacks(&self) -> u64 {
        self.readbacks.load(Ordering::Relaxed)
    }

    /// Pictures open.
    pub fn count(&self) -> usize {
        self.lock().open.len()
    }

    /// Opens a ring for `conn`. Its frames start at [`Pictures::start`].
    pub(crate) fn open_ring(
        self: &Arc<Self>,
        conn: ConnId,
        params: &wire::OpenParams,
        events: Option<EventSink>,
    ) -> Result<Opened, RpcError> {
        self.open(conn, params, Transport::Ring, |path, width| {
            let ring = Ring::create(path, (width, width)).map_err(|e| {
                RpcError::new(
                    code::UNAVAILABLE,
                    format!("creating {}: {e}", path.display()),
                )
            })?;
            Ok(Out::Ring {
                ring,
                path: path.to_owned(),
                events,
            })
        })
    }

    /// Opens a stream for `conn` onto `tx`. Its frames start at
    /// [`Pictures::start`].
    pub(crate) fn open_stream(
        self: &Arc<Self>,
        conn: ConnId,
        params: &wire::OpenParams,
        tx: FramesTx,
    ) -> Result<Opened, RpcError> {
        self.open(conn, params, Transport::Stream, |_, _| Ok(Out::Stream(tx)))
    }

    fn open(
        self: &Arc<Self>,
        conn: ConnId,
        params: &wire::OpenParams,
        transport: Transport,
        make: impl FnOnce(&Path, u32) -> Result<Out, RpcError>,
    ) -> Result<Opened, RpcError> {
        let (width, fps, asked) = params
            .resolve()
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
        debug_assert_eq!(asked, transport);
        self.feed
            .ready()
            .map_err(|e| RpcError::new(code::UNAVAILABLE, e))?;
        let mut inner = self.lock();
        inner.next_id += 1;
        let id = inner.next_id;
        let path = match transport {
            Transport::Ring => {
                std::fs::create_dir_all(&self.dir).map_err(|e| {
                    RpcError::new(
                        code::UNAVAILABLE,
                        format!("creating {}: {e}", self.dir.display()),
                    )
                })?;
                Some(
                    self.dir
                        .join(format!("picture-{}-{id}.rgba", std::process::id())),
                )
            }
            Transport::Stream => None,
        };
        let out = make(path.as_deref().unwrap_or(Path::new("")), width)?;
        let now = Instant::now();
        inner.open.insert(
            id,
            Entry {
                conn,
                width,
                interval: Duration::from_secs(1) / fps,
                out: Arc::new(out),
                started: false,
                opened: now,
                next_due: now,
                frame: 0,
                read_seen: 0,
                read_moved: now,
                events_dropped: 0,
            },
        );
        Ok(Opened {
            id,
            transport,
            path,
            size: [width, width],
            format: match transport {
                Transport::Ring => "rgba8",
                Transport::Stream => "jpeg",
            }
            .into(),
            fps,
        })
    }

    /// The reply went out: frames may follow. Starts the worker.
    pub(crate) fn start(self: &Arc<Self>, id: u64) {
        let mut inner = self.lock();
        let now = Instant::now();
        if let Some(e) = inner.open.get_mut(&id) {
            e.started = true;
            e.opened = now;
            e.next_due = now;
            e.read_moved = now;
        }
        if self.worker && !inner.worker {
            inner.worker = true;
            let me = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name("agent-picture".into())
                .spawn(move || me.run());
            if spawned.is_err() {
                inner.worker = false;
                inner.error = Some("could not start the picture worker".into());
            }
        }
        drop(inner);
        self.wake.notify_all();
    }

    /// Closes picture `id`: its ring file is removed, its stream ends.
    pub fn close(&self, id: u64) -> bool {
        let entry = self.lock().open.remove(&id);
        self.wake.notify_all();
        entry.is_some()
    }

    /// Closes every picture `conn` opened (it disconnected).
    pub fn close_conn(&self, conn: ConnId) {
        self.retain_conns(|c| c != conn);
    }

    /// Keeps only the pictures whose connection `alive` says is still up.
    pub fn retain_conns(&self, alive: impl Fn(ConnId) -> bool) {
        let mut inner = self.lock();
        if inner.open.values().all(|e| alive(e.conn)) {
            return;
        }
        // Dropped outside the lock: removing a file is not instant.
        let mut gone = Vec::new();
        inner.open.retain(|_, e| {
            let keep = alive(e.conn);
            if !keep {
                gone.push(e.out.clone());
            }
            keep
        });
        drop(inner);
        drop(gone);
        self.wake.notify_all();
    }

    /// `render.picture`'s part of a status: each picture's frames.
    pub fn status(&self) -> serde_json::Value {
        let inner = self.lock();
        let pictures: Vec<_> = inner
            .open
            .iter()
            .map(|(id, e)| {
                let (written, dropped) = match &*e.out {
                    Out::Stream(tx) => tx.written(),
                    Out::Ring { .. } => (e.frame, e.events_dropped),
                };
                serde_json::json!({"id": id, "frames": e.frame, "written": written, "dropped": dropped})
            })
            .collect();
        serde_json::json!({"pictures": pictures, "readbacks": self.readbacks(), "error": inner.error})
    }

    fn run(self: Arc<Self>) {
        loop {
            match self.pump_at(Instant::now(), true) {
                Pump::Closed => return,
                Pump::Published(_) => {}
                Pump::NoFrame => std::thread::sleep(NO_FRAME_POLL),
                Pump::Failed(_) => std::thread::sleep(FAILED_POLL),
                Pump::Wait(until) => {
                    let inner = self.lock();
                    let wait = until.saturating_duration_since(Instant::now());
                    if !wait.is_zero() {
                        let _ = self.wake.wait_timeout(inner, wait);
                    }
                }
            }
        }
    }

    /// One pass: if a picture is due at `now`, read the newest frame back
    /// and give it to every due picture.
    pub fn pump(&self, now: Instant) -> Pump {
        self.pump_at(now, false)
    }

    fn pump_at(&self, now: Instant, worker: bool) -> Pump {
        // Which pictures are due.
        let (due, after) = {
            let mut inner = self.lock();
            if inner.open.is_empty() {
                if worker {
                    inner.worker = false;
                }
                return Pump::Closed;
            }
            let mut due = Vec::new();
            let mut next = now + IDLE_POLL;
            for (id, e) in inner.open.iter_mut() {
                if !e.started || e.idle(now) {
                    continue;
                }
                if e.next_due <= now {
                    due.push(*id);
                } else {
                    next = next.min(e.next_due);
                }
            }
            if due.is_empty() {
                return Pump::Wait(next);
            }
            (due, inner.last_seq)
        };
        let grabbed = match self.feed.grab(after) {
            Ok(Some(g)) => g,
            Ok(None) => return Pump::NoFrame,
            Err(e) => {
                self.lock().error = Some(e.clone());
                return Pump::Failed(e);
            }
        };
        self.readbacks.fetch_add(1, Ordering::Relaxed);
        let tick = self.tick.load(Ordering::Relaxed);
        let tick = (tick != u64::MAX).then_some(tick);
        let present = self.present.load(Ordering::Relaxed);

        // Rings are written under the lock (a copy); stream frames are
        // encoded after it.
        let mut scaled: BTreeMap<(u32, u32), Vec<u8>> = BTreeMap::new();
        let mut lines = Vec::new();
        let mut streams = Vec::new();
        let mut published = 0;
        {
            let mut inner = self.lock();
            inner.last_seq = Some(grabbed.seq);
            inner.error = None;
            for id in due {
                let Some(e) = inner.open.get_mut(&id) else {
                    continue;
                };
                let (w, h) = fit(grabbed.size, e.width);
                let pixels = scaled
                    .entry((w, h))
                    .or_insert_with(|| scale_rgba(&grabbed.rgba, grabbed.size, (w, h)));
                e.frame += 1;
                e.next_due = if now.saturating_duration_since(e.next_due) >= e.interval {
                    now + e.interval
                } else {
                    e.next_due + e.interval
                };
                let event = PictureEvent {
                    id,
                    frame: e.frame,
                    tick,
                    present,
                    time_ms: now.saturating_duration_since(e.opened).as_secs_f64() * 1000.0,
                    width: w,
                    height: h,
                };
                match &*e.out {
                    Out::Ring { ring, events, .. } => {
                        ring.publish(
                            SlotMeta {
                                frame: e.frame,
                                tick,
                                present,
                                time_us: now.saturating_duration_since(e.opened).as_micros() as u64,
                                width: w,
                                height: h,
                            },
                            pixels,
                        );
                        if events.is_some() {
                            let line = serde_json::to_string(&Notification::event(
                                agent_proto::Event::Picture(event),
                            ))
                            .expect("events serialise");
                            lines.push((id, e.out.clone(), line));
                        }
                    }
                    Out::Stream(_) => streams.push((id, e.out.clone(), event, pixels.clone())),
                }
                published += 1;
            }
        }
        let mut dropped_events = Vec::new();
        for (id, out, line) in lines {
            if let Out::Ring {
                events: Some(sink), ..
            } = &*out
                && !sink(line)
            {
                dropped_events.push(id);
            }
        }
        let mut gone = Vec::new();
        for (id, out, event, pixels) in streams {
            let Out::Stream(tx) = &*out else { continue };
            let jpeg = match encode_jpeg(&pixels, (event.width, event.height)) {
                Ok(j) => j,
                Err(e) => {
                    self.lock().error = Some(e);
                    continue;
                }
            };
            if tx.send(wire::encode_stream_frame(&event, &jpeg)) == Sent::Gone {
                gone.push(id);
            }
        }
        if !dropped_events.is_empty() || !gone.is_empty() {
            let mut inner = self.lock();
            for id in dropped_events {
                if let Some(e) = inner.open.get_mut(&id) {
                    e.events_dropped += 1;
                }
            }
            let closed: Vec<_> = gone.iter().filter_map(|id| inner.open.remove(id)).collect();
            drop(inner);
            drop(closed);
        }
        Pump::Published(published)
    }
}

/// The size a `src` frame shows at in a `width` x `width` box: its aspect
/// kept, never scaled up, at least one pixel each way.
pub fn fit(src: (u32, u32), width: u32) -> (u32, u32) {
    let (w, h) = (src.0.max(1), src.1.max(1));
    if w <= width && h <= width {
        return (w, h);
    }
    let scale = width as f64 / w.max(h) as f64;
    let dim = |v: u32| ((v as f64 * scale).round() as u32).clamp(1, width);
    (dim(w), dim(h))
}

/// `src` (RGBA8, `from` pixels) scaled to `to` by averaging the source
/// pixels under each target pixel.
pub fn scale_rgba(src: &[u8], from: (u32, u32), to: (u32, u32)) -> Vec<u8> {
    if from == to {
        return src.to_vec();
    }
    let (sw, sh) = (from.0 as usize, from.1 as usize);
    let (dw, dh) = (to.0 as usize, to.1 as usize);
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let y0 = y * sh / dh;
        let y1 = ((y + 1) * sh / dh).max(y0 + 1).min(sh);
        for x in 0..dw {
            let x0 = x * sw / dw;
            let x1 = ((x + 1) * sw / dw).max(x0 + 1).min(sw);
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                let row = &src[(sy * sw + x0) * 4..(sy * sw + x1) * 4];
                for px in row.chunks_exact(4) {
                    for c in 0..4 {
                        sum[c] += u32::from(px[c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let at = (y * dw + x) * 4;
            for c in 0..4 {
                out[at + c] = ((sum[c] + n / 2) / n) as u8;
            }
        }
    }
    out
}

/// RGBA8 as a JPEG (alpha dropped).
pub fn encode_jpeg(rgba: &[u8], size: (u32, u32)) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode(&rgb, size.0, size.1, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("encoding a picture frame: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests;
