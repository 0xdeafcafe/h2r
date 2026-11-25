//! The paired game's picture: `render.picture` read for the Game panel
//! (docs/editor/game-view.md, "Frames" and "Attached mode").
//!
//! A [`Picture`] opens the game's shared-memory ring on a connection of its
//! own. A thread keeps that connection: it makes the `render.picture.open`
//! call and then reads the `picture` events, which only keep the socket
//! drained. The frames themselves are read on the caller's thread, by
//! [`Picture::poll`], straight from the mapped ring: the panel polls once a
//! frame while it is drawn and `game.status` and `game.snapshot` poll when
//! they are asked. A ring nobody polls costs the game nothing (it stops
//! reading frames back a second after the newest one went unread).
//!
//! Only a local game has a ring. A game over `tcp://` sends JPEG frames on
//! its socket instead; Studio does not decode them yet, so such a picture
//! is [`State::Failed`] with the reason.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use agent_proto::picture::{self as wire, OpenParams, Opened, Ring, Transport};
use agent_proto::{Client, ConnectionShutdown, Endpoint};

/// No new frame for this long and the picture is stalled.
pub const STALL: Duration = Duration::from_secs(2);
/// The frame rate is counted over this window.
const RATE_WINDOW: Duration = Duration::from_secs(1);
/// How long the game may take to answer `render.picture.open`.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// One frame taken from the ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, top row first.
    pub rgba: Vec<u8>,
    /// The ring's frame number.
    pub number: u64,
    /// The game's tick when the frame was taken.
    pub tick: Option<u64>,
    /// Frames the game had presented.
    pub present: u64,
    /// Counts the frames this picture has taken, from 1: a texture
    /// uploaded at an older `rev` is out of date.
    pub rev: u64,
}

/// Where a picture is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The game has not answered `render.picture.open` yet.
    Connecting,
    /// Frames are coming.
    Open,
    /// The ring is open, but no new frame has come for [`STALL`].
    Stalled,
    /// The game closed the picture.
    Closed,
    /// The picture could not be opened or its connection broke.
    Failed(String),
}

impl State {
    /// The spelling `game.status` uses.
    pub fn name(&self) -> &'static str {
        match self {
            State::Connecting => "connecting",
            State::Open => "open",
            State::Stalled => "stalled",
            State::Closed => "closed",
            State::Failed(_) => "failed",
        }
    }
}

/// What the panel and `game.status` say about a picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub state: State,
    /// The size of the newest frame taken.
    pub size: Option<[u32; 2]>,
    /// The rate the game was asked for.
    pub asked_fps: Option<u32>,
    /// Frames taken since the picture opened.
    pub frames: u64,
    /// Frames the game published that were never taken (the panel polled
    /// less often than the game published).
    pub dropped: u64,
    /// Frames taken a second, over the last second.
    pub fps: f64,
    /// Time since the newest frame was taken (since the ring opened,
    /// before the first).
    pub age: Option<Duration>,
    /// The game's tick in the newest frame.
    pub tick: Option<u64>,
    /// `picture` events the game sent.
    pub events: u64,
}

#[derive(Default)]
struct Shared {
    opened: Option<Opened>,
    failed: Option<String>,
    events: u64,
    stopped: bool,
    shutdown: Option<ConnectionShutdown>,
}

/// A game's picture, held by the pairing.
pub struct Picture {
    shared: Arc<Mutex<Shared>>,
    ring: Option<Ring>,
    ring_failed: Option<String>,
    scratch: Vec<u8>,
    frame: Option<Frame>,
    taken: u64,
    dropped: u64,
    last_number: u64,
    arrivals: VecDeque<Instant>,
    /// When the ring opened, then when the newest frame was taken.
    since: Option<Instant>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picture")
            .field("taken", &self.taken)
            .field("dropped", &self.dropped)
            .finish_non_exhaustive()
    }
}

impl Picture {
    fn empty(shared: Shared) -> Picture {
        Picture {
            shared: Arc::new(Mutex::new(shared)),
            ring: None,
            ring_failed: None,
            scratch: Vec::new(),
            frame: None,
            taken: 0,
            dropped: 0,
            last_number: 0,
            arrivals: VecDeque::new(),
            since: None,
        }
    }

    /// Opens the picture of the game at `endpoint`, on a connection and a
    /// thread of its own. The answer is read by [`Picture::poll`].
    pub fn open(endpoint: Endpoint) -> Picture {
        if let Endpoint::Tcp(_) = endpoint {
            return Picture::failed(
                "a game over tcp:// sends its picture as JPEG frames on the socket, which Studio does not decode yet",
            );
        }
        let picture = Picture::empty(Shared::default());
        let shared = Arc::clone(&picture.shared);
        let started = std::thread::Builder::new()
            .name("h2-editor-picture".into())
            .spawn({
                let shared = Arc::clone(&shared);
                move || {
                    let result = run(&endpoint, &shared);
                    let mut s = lock(&shared);
                    s.shutdown = None;
                    if !s.stopped
                        && let Err(why) = result
                    {
                        s.failed = Some(why);
                    }
                }
            });
        if let Err(e) = started {
            lock(&shared).failed = Some(e.to_string());
        }
        picture
    }

    /// A picture that failed before it began.
    pub fn failed(why: impl Into<String>) -> Picture {
        Picture::empty(Shared {
            failed: Some(why.into()),
            ..Shared::default()
        })
    }

    /// A picture over a ring already mapped, with no connection: the
    /// tests' fake game writes the ring itself.
    pub fn with_ring(ring: Ring, opened: Opened, now: Instant) -> Picture {
        let mut picture = Picture::empty(Shared {
            opened: Some(opened),
            ..Shared::default()
        });
        picture.ring = Some(ring);
        picture.since = Some(now);
        picture
    }

    /// Whether this picture is over and a new one should be opened (the
    /// game closed it, or its connection broke).
    pub fn dead(&self) -> bool {
        matches!(
            self.state(Instant::now()),
            State::Closed | State::Failed(_)
        )
    }

    /// Takes the newest frame the game has published since the last one, if
    /// any. Returns whether a new frame was taken.
    pub fn poll(&mut self, now: Instant) -> bool {
        self.map_ring(now);
        let Some(ring) = &self.ring else {
            return false;
        };
        if ring.newest() <= self.last_number {
            return false;
        }
        let Some(meta) = ring.read_newest(&mut self.scratch) else {
            return false;
        };
        if meta.frame <= self.last_number {
            return false;
        }
        if self.last_number > 0 {
            self.dropped += meta.frame - self.last_number - 1;
        }
        self.last_number = meta.frame;
        self.taken += 1;
        let mut rgba = std::mem::take(&mut self.scratch);
        if let Some(old) = self.frame.take() {
            // Keep the old frame's allocation for the next read.
            self.scratch = old.rgba;
        }
        rgba.truncate(meta.width as usize * meta.height as usize * 4);
        self.frame = Some(Frame {
            width: meta.width,
            height: meta.height,
            rgba,
            number: meta.frame,
            tick: meta.tick,
            present: meta.present,
            rev: self.taken,
        });
        self.arrivals.push_back(now);
        while self
            .arrivals
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) > RATE_WINDOW)
        {
            self.arrivals.pop_front();
        }
        self.since = Some(now);
        true
    }

    /// Maps the ring once the game has answered.
    fn map_ring(&mut self, now: Instant) {
        if self.ring.is_some() || self.ring_failed.is_some() {
            return;
        }
        let opened = lock(&self.shared).opened.clone();
        let Some(opened) = opened else { return };
        let mapped = match (opened.transport, &opened.path) {
            (Transport::Ring, Some(path)) => Ring::open(path)
                .map_err(|e| format!("mapping the picture ring {}: {e}", path.display())),
            (Transport::Ring, None) => Err("the game opened a ring with no path".to_owned()),
            (Transport::Stream, _) => Err("the game opened a stream, not a ring".to_owned()),
        };
        match mapped {
            Ok(ring) => {
                self.ring = Some(ring);
                self.since = Some(now);
            }
            Err(why) => self.ring_failed = Some(why),
        }
    }

    /// The newest frame taken.
    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    pub fn state(&self, now: Instant) -> State {
        if let Some(why) = &self.ring_failed {
            return State::Failed(why.clone());
        }
        // A game that closed the picture then dropped the connection is
        // closed, not broken.
        if self.ring.as_ref().is_some_and(Ring::closed) {
            return State::Closed;
        }
        let (failed, opened) = {
            let s = lock(&self.shared);
            (s.failed.clone(), s.opened.is_some())
        };
        if let Some(why) = failed {
            return State::Failed(why);
        }
        if !opened || self.ring.is_none() {
            return State::Connecting;
        }
        match self.since {
            Some(t) if now.saturating_duration_since(t) > STALL => State::Stalled,
            _ => State::Open,
        }
    }

    pub fn stats(&self, now: Instant) -> Stats {
        let s = lock(&self.shared);
        let asked_fps = s.opened.as_ref().map(|o| o.fps);
        let events = s.events;
        drop(s);
        let recent = self
            .arrivals
            .iter()
            .filter(|t| now.saturating_duration_since(**t) <= RATE_WINDOW)
            .count();
        Stats {
            state: self.state(now),
            size: self.frame.as_ref().map(|f| [f.width, f.height]),
            asked_fps,
            frames: self.taken,
            dropped: self.dropped,
            fps: recent as f64 / RATE_WINDOW.as_secs_f64(),
            age: self.since.map(|t| now.saturating_duration_since(t)),
            tick: self.frame.as_ref().and_then(|f| f.tick),
            events,
        }
    }

    /// The newest frame as a PNG's pixels, taking a newer one first.
    pub fn snapshot(&mut self, now: Instant) -> Option<&Frame> {
        self.poll(now);
        self.frame.as_ref()
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        // The game removes the ring's file when this connection drops.
        let mut s = lock(&self.shared);
        s.stopped = true;
        if let Some(handle) = s.shutdown.take() {
            let _ = handle.shutdown();
        }
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// The connection's thread: open the picture, then drain its events.
fn run(endpoint: &Endpoint, shared: &Mutex<Shared>) -> Result<(), String> {
    let mut client = Client::connect_with_timeout(endpoint, Some(OPEN_TIMEOUT))
        .map_err(|e| format!("connecting to {endpoint}: {e}"))?;
    client
        .set_timeout(Some(OPEN_TIMEOUT))
        .map_err(|e| e.to_string())?;
    {
        let mut s = lock(shared);
        if s.stopped {
            return Ok(());
        }
        s.shutdown = Some(client.shutdown_handle().map_err(|e| e.to_string())?);
    }
    let params = OpenParams {
        transport: Some(Transport::Ring),
        ..OpenParams::default()
    };
    let reply = client
        .call(
            wire::OPEN,
            serde_json::to_value(&params).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("{}: {e}", wire::OPEN))?;
    let opened: Opened = serde_json::from_value(reply)
        .map_err(|e| format!("{}'s reply: {e}", wire::OPEN))?;
    lock(shared).opened = Some(opened);
    // A game is quiet when nobody reads, and a paused one may be quiet for
    // good: the drain waits as long as it takes. Dropping the picture shuts
    // the connection down.
    client.set_timeout(None).map_err(|e| e.to_string())?;
    loop {
        match client.next_event() {
            Ok(agent_proto::Event::Picture(_)) => lock(shared).events += 1,
            Ok(_) => {}
            Err(e) => return Err(format!("the picture's connection: {e}")),
        }
    }
}

#[cfg(test)]
mod tests;
