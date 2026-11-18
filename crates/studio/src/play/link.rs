//! The connection to a running game's agent socket, on its own thread so
//! the UI never waits on the game.
//!
//! The thread connects (retrying while a launched game starts), then runs
//! the calls it is sent in order and, when idle, polls `session.status`
//! and `world.query` a few times a second. Everything comes back as
//! [`Event`]s that the UI drains once a frame ([`Link::drain`]).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use agent_proto::{CallError, Client, Endpoint, method};
use serde_json::{Value, json};

/// How often the idle link polls the game.
pub const POLL: Duration = Duration::from_millis(250);
/// How long one call may take before the link is taken as lost.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// The most world objects one poll lists.
pub const OBJECTS: usize = 300;

/// What a call was for, so its answer finds its way back.
#[derive(Clone, Debug, PartialEq)]
pub enum Purpose {
    /// `edit.tags {group: scnr}`: the loaded scenario's datum.
    Scenario,
    /// `sim.session`: whether host writes are allowed.
    SessionKind,
    /// An `edit.set` of a placement field (the diff's index).
    Edit(u64),
    Teleport,
    Control(&'static str),
    /// A call the user asked for by hand.
    Other,
}

/// A call for the link's thread.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: String,
    pub params: Value,
    pub purpose: Purpose,
}

/// What the link reports.
#[derive(Clone, Debug)]
pub enum Event {
    Connected,
    /// Connecting failed for good, or the connection broke.
    Lost(String),
    /// One poll: `session.status` and `world.query`'s answers.
    Poll {
        status: Value,
        world: Value,
    },
    Answer {
        purpose: Purpose,
        method: String,
        result: Result<Value, String>,
    },
}

/// How to connect.
#[derive(Clone, Debug)]
pub struct Options {
    /// Keep trying to connect for this long (a game being launched opens
    /// its socket once it has booted); zero tries once.
    pub wait: Duration,
    pub poll: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            wait: Duration::ZERO,
            poll: POLL,
        }
    }
}

/// The UI's end of a connection. Dropping it ends the thread.
pub struct Link {
    pub endpoint: Endpoint,
    calls: Sender<Call>,
    events: Receiver<Event>,
    stop: Sender<()>,
}

impl Link {
    pub fn open(endpoint: Endpoint, options: Options) -> Link {
        let (calls, call_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let (stop, stop_rx) = mpsc::channel();
        let ep = endpoint.clone();
        let _ = thread::Builder::new()
            .name("h2-editor-agent".into())
            .spawn(move || run(ep, options, call_rx, event_tx, stop_rx));
        Link {
            endpoint,
            calls,
            events,
            stop,
        }
    }

    pub fn call(&self, method: &str, params: Value, purpose: Purpose) {
        let _ = self.calls.send(Call {
            method: method.to_owned(),
            params,
            purpose,
        });
    }

    /// Everything reported since the last drain.
    pub fn drain(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Waits up to `t` for the next event (tests).
    pub fn next(&self, t: Duration) -> Option<Event> {
        self.events.recv_timeout(t).ok()
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn connect(ep: &Endpoint, options: &Options, stop: &Receiver<()>) -> Result<Client, String> {
    let until = Instant::now() + options.wait;
    loop {
        match Client::connect_to(ep) {
            Ok(c) => return Ok(c),
            Err(e) if Instant::now() >= until => {
                return Err(format!(
                    "connecting to {ep}: {e} (is a game running with its agent socket there?)"
                ));
            }
            Err(_) => {}
        }
        match stop.recv_timeout(Duration::from_millis(300)) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return Err("stopped".into()),
        }
    }
}

fn run(
    ep: Endpoint,
    options: Options,
    calls: Receiver<Call>,
    events: Sender<Event>,
    stop: Receiver<()>,
) {
    let mut c = match connect(&ep, &options, &stop) {
        Ok(c) => c,
        Err(e) => {
            let _ = events.send(Event::Lost(e));
            return;
        }
    };
    let _ = c.set_timeout(Some(CALL_TIMEOUT));
    let _ = events.send(Event::Connected);
    let mut next_poll = Instant::now();
    loop {
        if stop.try_recv().is_ok() {
            return;
        }
        let wait = next_poll.saturating_duration_since(Instant::now());
        match calls.recv_timeout(wait) {
            Ok(call) => {
                let result = c.call(&call.method, call.params);
                let broke = matches!(&result, Err(e) if lost(e));
                let _ = events.send(Event::Answer {
                    purpose: call.purpose,
                    method: call.method,
                    result: result.map_err(|e| e.to_string()),
                });
                if broke {
                    let _ = events.send(Event::Lost("the game closed the connection".into()));
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                next_poll = Instant::now() + options.poll;
                let status = c.call(method::STATUS, Value::Null);
                let world = c.call(
                    method::WORLD_QUERY,
                    json!({
                        "fields": ["pos", "forward", "team", "name", "mounted_on"],
                        "limit": OBJECTS,
                    }),
                );
                match (status, world) {
                    (Ok(status), Ok(world)) => {
                        let _ = events.send(Event::Poll { status, world });
                    }
                    // World reads may be refused for a moment (a load): the
                    // status still says where the game is.
                    (Ok(status), Err(e)) if !lost(&e) => {
                        let _ = events.send(Event::Poll {
                            status,
                            world: Value::Null,
                        });
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        let _ = events.send(Event::Lost(e.to_string()));
                        return;
                    }
                }
            }
        }
    }
}

/// An error that means the connection is gone (not a refused request).
fn lost(e: &CallError) -> bool {
    !matches!(e, CallError::Rpc(_))
}
