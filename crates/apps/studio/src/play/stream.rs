//! The editor's single binary subscription. Keep capture headers beside the
//! interpolation window so the aim cursor sees cuts even between UI frames.
use agent_proto::live::{self, Frame, Header, Kind};
use agent_proto::{Client, ConnectionShutdown, Endpoint};
use live_mirror::{Sample, Store};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// Only header history lives here. Store applies the frames, rejects broken
/// deltas, and supplies both exact and interpolated world records.
#[derive(Clone)]
pub(super) struct Captures {
    store: Store,
    recent: VecDeque<(Header, Instant)>,
    last_seq: Option<u64>,
    segment: u64,
    tick_rate: f64,
    object_count: OnceLock<usize>,
}

impl Captures {
    pub(super) fn new(tick_rate: f64) -> io::Result<Self> {
        Ok(Self {
            store: Store::new(tick_rate, 1)?,
            recent: VecDeque::with_capacity(3),
            last_seq: None,
            segment: 0,
            tick_rate,
            object_count: OnceLock::new(),
        })
    }

    pub(super) fn apply(&mut self, frame: &Frame, at: Instant) -> io::Result<()> {
        self.store.apply(frame, at)?;
        let header = frame.header;
        let prev_seq = self.last_seq.replace(header.seq);
        if header.kind == Kind::Notice {
            return Ok(());
        }
        if header.cut
            || prev_seq.map(|s| s.wrapping_add(1)) != Some(header.seq)
            || self
                .recent
                .back()
                .is_none_or(|(p, _)| p.epoch != header.epoch || p.tick >= header.tick)
        {
            self.segment = self.segment.wrapping_add(1);
            self.recent.clear();
        }
        if self.recent.len() == 3 {
            self.recent.pop_front();
        }
        self.recent.push_back((header, at));
        self.object_count = OnceLock::new();
        Ok(())
    }

    pub(super) fn latest_header(&self) -> Option<(u64, Header)> {
        Some((self.segment, self.recent.back()?.0))
    }

    pub(super) fn sample(&self, now: Instant, latest: bool) -> Option<Sample> {
        let sample = self.store.sample(now, latest)?;
        if self
            .recent
            .back()
            .is_some_and(|(h, _)| h.tick == sample.tick)
        {
            self.object_count.get_or_init(|| sample.objects.len());
        }
        Some(sample)
    }

    // Store displays two intervals behind the clock and extrapolates at most
    // one more. This subscription uses the default interval of one tick.
    pub(super) fn age_ticks(&self, now: Instant) -> f64 {
        self.recent.back().map_or(0.0, |(_, arrived)| {
            now.saturating_duration_since(*arrived).as_secs_f64() * self.tick_rate
        })
    }

    fn fps(&self) -> Option<f64> {
        let span = self
            .recent
            .back()?
            .1
            .saturating_duration_since(self.recent.front()?.1);
        (!span.is_zero()).then(|| (self.recent.len() - 1) as f64 / span.as_secs_f64())
    }

    fn object_count(&self) -> usize {
        *self.object_count.get_or_init(|| {
            self.latest_header()
                .and_then(|(_, h)| self.store.at_tick(h.tick))
                .map_or(0, |s| s.objects.len())
        })
    }
}

/// The binary connection opened beside the editor's JSON control link.
/// The worker applies each delta even if the window has stopped painting.
pub struct Stream {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("status", &self.status(false))
            .finish()
    }
}

#[derive(Default)]
struct State {
    stopped: bool,
    shutdown: Option<ConnectionShutdown>,
    hello: Value,
    store: Option<Arc<Captures>>,
    bytes: u64,
    error: Option<String>,
}

impl Stream {
    pub fn open(endpoint: Endpoint) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let worker = Arc::clone(&state);
        let started = thread::Builder::new()
            .name("h2-editor-mirror".into())
            .spawn(move || {
                let result = receive(&endpoint, &worker);
                let mut state = worker.lock().unwrap();
                state.shutdown = None;
                if !state.stopped
                    && let Err(e) = result
                {
                    state.error = Some(e);
                }
            });
        if let Err(e) = started {
            state.lock().unwrap().error = Some(e.to_string());
        }
        Self { state }
    }

    pub fn error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }

    /// A coherent interpolation window. Its header and samples belong to the
    /// same stream state even if the receiver accepts another frame.
    pub(super) fn snapshot(&self) -> Option<Arc<Captures>> {
        let state = self.state.lock().unwrap();
        if state.error.is_some() {
            return None;
        }
        state.store.clone()
    }

    pub fn sample(&self, latest: bool) -> Option<Sample> {
        self.snapshot()?.sample(Instant::now(), latest)
    }

    pub fn at_tick(&self, tick: u64) -> Option<Sample> {
        self.snapshot()?.store.at_tick(tick)
    }

    pub fn status(&self, latest: bool) -> Value {
        let state = self.state.lock().unwrap();
        let snapshot = state.store.clone();
        let store = snapshot.as_ref();
        let first = store.and_then(|s| s.recent.front().map(|v| v.0));
        let last = store.and_then(|s| s.latest_header().map(|v| v.1));
        let fps = store.and_then(|s| s.fps());
        let delay_ms = store.map(|s| {
            if latest || last.is_some_and(|h| h.paused) {
                0.0
            } else {
                2000.0 / s.tick_rate
            }
        });
        let phase = if state.error.is_some() {
            "lost"
        } else if last.is_some() {
            "streaming"
        } else if store.is_some() {
            "subscribed"
        } else {
            "connecting"
        };
        let mut status = json!({
            "state": phase, "stream": state.hello["stream"], "protocol": state.hello["protocol"],
            "tick_rate": state.hello["tick_rate"],
            "layers": state.hello["layers"], "map": state.hello["map"], "refused": state.hello["refused"],
            "fps": fps, "delay_ms": delay_ms, "notice": state.error,
            "first_tick": first.map(|h| h.tick), "tick": last.map(|h| h.tick),
            "bytes": state.bytes,
        });
        drop(state);
        status["objects"] = json!(store.map_or(0, |s| s.object_count()));
        status
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.stopped = true;
        if let Some(handle) = state.shutdown.take() {
            let _ = handle.shutdown();
        }
    }
}

fn receive(endpoint: &Endpoint, state: &Mutex<State>) -> Result<(), String> {
    let mut client = Client::connect_with_timeout(endpoint, Some(Duration::from_secs(10)))
        .map_err(|e| format!("connecting to {endpoint}: {e}"))?;
    client
        .set_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    {
        let mut state = state.lock().unwrap();
        if state.stopped {
            return Ok(());
        }
        state.shutdown = Some(client.shutdown_handle().map_err(|e| e.to_string())?);
    }
    let hello = client
        .call(live::SUBSCRIBE, json!({"layers": ["objects", "players"]}))
        .map_err(|e| format!("{}: {e}", live::SUBSCRIBE))?;
    if hello["protocol"].as_str() != Some(live::PROTOCOL) {
        return Err(format!(
            "unsupported live stream protocol {}",
            hello["protocol"]
        ));
    }
    let rate = hello["tick_rate"]
        .as_f64()
        .ok_or("missing live stream tick_rate")?;
    let mut store = Captures::new(rate).map_err(|e| e.to_string())?;
    {
        let mut state = state.lock().unwrap();
        state.hello = hello;
        state.store = Some(Arc::new(store.clone()));
    }
    // A paused game can be quiet indefinitely. Drop interrupts read_exact,
    // including when the game has written only the prefix of a frame.
    client.set_timeout(None).map_err(|e| e.to_string())?;
    let mut reader = CountingReader {
        inner: client.into_live_reader(),
        bytes: 0,
    };
    loop {
        let frame = Frame::read(&mut reader).map_err(|e| format!("reading live frame: {e}"))?;
        store
            .apply(&frame, Instant::now())
            .map_err(|e| e.to_string())?;
        let mut state = state.lock().unwrap();
        if state.stopped {
            return Ok(());
        }
        // The UI holds a coherent snapshot while the worker accepts the next frame.
        state.store = Some(Arc::new(store.clone()));
        state.bytes = reader.bytes;
    }
}

struct CountingReader<R> {
    inner: R,
    bytes: u64,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::live::{Layers, World};

    #[test]
    fn latest_header_remembers_a_cut_after_its_key_leaves_the_window() {
        let mut captures = Captures::new(30.0).unwrap();
        let now = Instant::now();
        let world = World::default();
        let frame = |seq, tick, cut, delta| {
            Frame::between(
                Header {
                    seq,
                    tick,
                    epoch: 1,
                    kind: if delta { Kind::Delta } else { Kind::Key },
                    paused: false,
                    cut,
                },
                Layers {
                    objects: true,
                    players: true,
                },
                if delta { Some(&world) } else { None },
                &world,
            )
        };
        captures.apply(&frame(1, 7, false, false), now).unwrap();
        let before = captures.latest_header().unwrap();
        captures.apply(&frame(2, 7, true, false), now).unwrap();
        for seq in 3..=5 {
            captures
                .apply(&frame(seq, seq + 5, false, true), now)
                .unwrap();
        }
        assert!(captures.store.at_tick(7).is_none());
        let after = captures.latest_header().unwrap();
        assert_ne!(before.0, after.0);
        assert!(!after.1.cut);
        assert_eq!(after.1.seq, 5);
    }
}
