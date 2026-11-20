use super::{Sample, Store};
use agent_proto::live::{self, Frame};
use agent_proto::{Client, ConnectionShutdown, Endpoint};
use serde_json::{Value, json};
use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
    store: Option<Store>,
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

    fn store(&self) -> Option<Store> {
        let state = self.state.lock().unwrap();
        if state.error.is_some() {
            return None;
        }
        state.store.clone()
    }

    pub fn sample(&self, latest: bool) -> Option<Sample> {
        self.store()?.sample(Instant::now(), latest)
    }

    pub fn at_tick(&self, tick: u64) -> Option<Sample> {
        self.store()?.at_tick(tick)
    }

    pub fn status(&self, latest: bool) -> Value {
        let state = self.state.lock().unwrap();
        let store = state.store.as_ref();
        let first = store.and_then(|s| s.samples.front());
        let last = store.and_then(|s| s.samples.back());
        let elapsed = first
            .zip(last)
            .map(|(a, b)| b.arrived.saturating_duration_since(a.arrived));
        let fps = elapsed
            .filter(|d| !d.is_zero())
            .zip(store)
            .map(|(d, s)| (s.samples.len() - 1) as f64 / d.as_secs_f64());
        let delay_ms = store.map(|s| {
            if latest || last.is_some_and(|s| s.header.paused) {
                0.0
            } else {
                2000.0 * f64::from(s.every) / s.tick_rate
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
        json!({
            "state": phase, "stream": state.hello["stream"], "protocol": state.hello["protocol"],
            "tick_rate": state.hello["tick_rate"],
            "layers": state.hello["layers"], "map": state.hello["map"], "refused": state.hello["refused"],
            "fps": fps, "delay_ms": delay_ms, "notice": state.error,
            "first_tick": first.map(|s| s.header.tick), "tick": last.map(|s| s.header.tick),
            "objects": last.map_or(0, |s| s.world.objects.len()), "bytes": state.bytes,
        })
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
    let mut store = Store::new(rate, 1).map_err(|e| e.to_string())?;
    {
        let mut state = state.lock().unwrap();
        state.hello = hello;
        state.store = Some(store.clone());
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
        // Just the three Arc references cross the lock. Diff application and
        // viewport sampling each happen on their own thread outside the lock.
        state.store = Some(store.clone());
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
