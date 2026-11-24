//! The connection to a running game's agent socket, on its own thread so
//! the UI never waits on the game.
//!
//! The thread connects (retrying while a launched game starts), then runs
//! the calls it is sent in order and, when idle, polls `session.status`.
//! The binary mirror has its own connection. Control replies come back as
//! [`Event`]s that the UI drains once a frame ([`Link::drain`]).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};

use agent_proto::{CallError, Client, Endpoint, method};
use serde_json::Value;

use super::{presence::PlayerMove, stream::Stream};

mod object;
pub(super) use object::ObjectDispatch;

/// How often the idle link polls the game.
pub const POLL: Duration = Duration::from_millis(250);
/// How long one call may take before the link is taken as lost.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// What a call was for, so its answer finds its way back.
#[derive(Clone, Debug, PartialEq)]
pub enum Purpose {
    /// The loaded file, map epoch and session write permissions.
    Map,
    Capabilities,
    Reload(u64),
    Focus,
    Indicator,
    Marks,
    Journal(u64),
    Revert {
        leave: bool,
        epoch: u64,
    },

    /// Stream counters on the JSON control connection.
    LiveStatus,
    /// One in-flight conditional document batch.
    Edit(u64),
    Object(u64),
    Take { id: u64, at: u32 },
    Teleport(u64),
    Spawn(Instant),
    /// One call of the subject's object in the own game (`play::subject`).
    Subject(u64),
    Control(&'static str),
    /// The script editor's `rules.reload`.
    Rules,
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

#[derive(Debug)]
enum QueuedCall {
    Control {
        call: Call,
        /// A queued move cannot keep its stream alive after the editor unpairs.
        player: Option<(Weak<Stream>, PlayerMove)>,
    },
    /// Play owns the payload until the worker enters the socket.
    Object { ticket: u64, edit: Weak<ObjectDispatch> },
}

impl QueuedCall {
    fn check_player(&self) -> Result<(), CallError> {
        let Self::Control { player: Some((stream, request)), .. } = self else {
            return Ok(());
        };
        stream
            .upgrade()
            .ok_or("the game stream closed before the move")
            .and_then(|s| request.check_latest(&s))
            .map_err(|why| {
                CallError::Rpc(agent_proto::RpcError::new(agent_proto::sim::REFUSED, why))
            })
    }
}

/// What the link reports.
#[derive(Clone, Debug)]
pub enum Event {
    Connected,
    /// Connecting failed for good, or the connection broke.
    Lost(String),
    /// The control connection's latest session status.
    Poll(Value),
    /// The edit was withdrawn or its stream changed before it reached the socket.
    ObjectSkipped(u64),
    Answer {
        purpose: Purpose,
        method: String,
        result: Result<Value, agent_proto::RpcError>,
    },
}

/// How to connect.
#[derive(Clone, Debug)]
pub struct Options {
    /// Keep trying to connect for this long (a game being launched opens
    /// its socket once it has booted); zero tries once.
    pub wait: Duration,
    pub poll: Duration,
    /// How long an unanswered request keeps this connection open.
    pub call_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            wait: Duration::ZERO,
            poll: POLL,
            call_timeout: CALL_TIMEOUT,
        }
    }
}

/// The UI's end of a connection. Dropping it ends the thread.
pub struct Link {
    pub endpoint: Endpoint,
    calls: Sender<QueuedCall>,
    events: Receiver<Event>,
    stop: Sender<Close>,
}

#[derive(Debug, Default)]
struct Close {
    clears: Vec<Call>,
    done: Option<Sender<()>>,
}

/// A retiring worker can still hold or clear either lease on this game.
pub(super) struct ClosingLink {
    pub(super) endpoint: Endpoint,
    done: Receiver<()>,
}

impl ClosingLink {
    pub(super) fn pending(&self) -> bool {
        // A dropped sender means the worker already lost the connection.
        matches!(self.done.try_recv(), Err(mpsc::TryRecvError::Empty))
    }
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
        let call = Call {
            method: method.to_owned(),
            params,
            purpose,
        };
        let _ = self.calls.send(QueuedCall::Control { call, player: None });
    }

    pub(super) fn offer_object(&self, ticket: u64, edit: &Arc<ObjectDispatch>) -> bool {
        self.calls.send(QueuedCall::Object { ticket, edit: Arc::downgrade(edit) }).is_ok()
    }

    pub(super) fn move_player(
        &self,
        request: PlayerMove,
        stream: &Arc<Stream>,
        ticket: u64,
    ) -> Result<(), String> {
        let call = Call {
            method: agent_proto::sim::method::STATE_SET.into(),
            params: request.state_set_params(),
            purpose: Purpose::Teleport(ticket),
        };
        self.calls
            .send(QueuedCall::Control {
                call,
                player: Some((Arc::downgrade(stream), request)),
            })
            .map_err(|_| "the game link closed before the move".to_owned())
    }

    /// Discard queued work and release presentation leases on this connection.
    /// The worker finishes an in-flight call before sending the clears.
    pub(super) fn close_with(self, clears: Vec<Call>) -> ClosingLink {
        let (ack, done) = mpsc::channel();
        let _ = self.stop.send(Close {
            clears,
            done: Some(ack),
        });
        ClosingLink {
            endpoint: self.endpoint.clone(),
            done,
        }
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
        let _ = self.stop.send(Close::default());
    }
}

fn connect(ep: &Endpoint, options: &Options, stop: &Receiver<Close>) -> Result<Client, String> {
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
    calls: Receiver<QueuedCall>,
    events: Sender<Event>,
    stop: Receiver<Close>,
) {
    let mut c = match connect(&ep, &options, &stop) {
        Ok(c) => c,
        Err(e) => {
            let _ = events.send(Event::Lost(e));
            return;
        }
    };
    let _ = c.set_timeout(Some(options.call_timeout));
    let _ = events.send(Event::Connected);
    let mut next_poll = Instant::now();
    loop {
        let wait = next_poll.saturating_duration_since(Instant::now());
        match next_work(&stop, || calls.recv_timeout(wait)) {
            Work::Call(queued) => {
                let checked = queued.check_player();
                // Reading the exact sample can overlap an unpair. Give its
                // close request priority again before entering the socket.
                if let Ok(close) = stop.try_recv() {
                    clear_leases(&mut c, close);
                    return;
                }
                let call = match queued {
                    QueuedCall::Control { call, .. } => call,
                    QueuedCall::Object { ticket, edit } => {
                        let edit = edit.upgrade();
                        let current = edit.as_ref().is_some_and(|edit| edit.current_stream());
                        if let Ok(close) = stop.try_recv() {
                            clear_leases(&mut c, close);
                            return;
                        }
                        let Some(call) = edit.and_then(|edit| edit.dispatch(current)) else {
                            let _ = events.send(Event::ObjectSkipped(ticket));
                            continue;
                        };
                        call
                    }
                };
                let result = checked.and_then(|()| c.call(&call.method, call.params));
                let broke = matches!(&result, Err(e) if lost(e));
                if broke {
                    let _ = events.send(Event::Lost(result.unwrap_err().to_string()));
                    return;
                }
                let _ = events.send(Event::Answer {
                    purpose: call.purpose,
                    method: call.method,
                    result: result.map_err(|e| match e {
                        CallError::Rpc(e) => e,
                        e => agent_proto::RpcError::new(
                            agent_proto::code::UNAVAILABLE,
                            e.to_string(),
                        ),
                    }),
                });
            }
            Work::Close(close) => {
                clear_leases(&mut c, close);
                return;
            }
            Work::Poll => {
                next_poll = Instant::now() + options.poll;
                match c.call(method::STATUS, Value::Null) {
                    Ok(status) => {
                        let _ = events.send(Event::Poll(status));
                    }
                    Err(e) => {
                        let _ = events.send(Event::Lost(e.to_string()));
                        return;
                    }
                }
            }
        }
    }
}

enum Work {
    Call(QueuedCall),
    Poll,
    Close(Close),
}

fn next_work(
    stop: &Receiver<Close>,
    receive: impl FnOnce() -> Result<QueuedCall, RecvTimeoutError>,
) -> Work {
    if let Ok(close) = stop.try_recv() {
        return Work::Close(close);
    }
    let received = receive();
    // Unpair can arrive while recv_timeout waits, including when it wakes for a move.
    if let Ok(close) = stop.try_recv() {
        return Work::Close(close);
    }
    match received {
        Ok(call) => Work::Call(call),
        Err(RecvTimeoutError::Timeout) => Work::Poll,
        Err(RecvTimeoutError::Disconnected) => Work::Close(Close::default()),
    }
}

fn clear_leases(client: &mut Client, close: Close) {
    for clear in close.clears {
        let _ = client.call(&clear.method, clear.params);
    }
    if let Some(done) = close.done {
        let _ = done.send(());
    }
}

/// An error that means the connection is gone (not a refused request).
fn lost(e: &CallError) -> bool {
    !matches!(e, CallError::Rpc(_))
}

#[cfg(all(test, unix))]
mod tests;
