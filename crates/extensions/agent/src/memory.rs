//! Host reads and counters for the process memory watchdog.
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use agent_proto::method;

use crate::Agent;

static REQUESTS: AtomicUsize = AtomicUsize::new(0);
static REPLIES: AtomicUsize = AtomicUsize::new(0);
static REPLY_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Count a pending request until it is dispatched or discarded.
pub(crate) struct PendingRequest;

impl PendingRequest {
    pub(crate) fn new() -> Self {
        REQUESTS.fetch_add(1, Relaxed);
        Self
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        REQUESTS.fetch_sub(1, Relaxed);
    }
}

/// Count the buffer through socket write completion, including queued lines
/// discarded on disconnect or when a slow reader fills its channel.
pub(crate) struct ReplyLine(String);

impl From<String> for ReplyLine {
    fn from(line: String) -> Self {
        REPLY_BYTES.fetch_add(line.capacity(), Relaxed);
        REPLIES.fetch_add(1, Relaxed);
        Self(line)
    }
}

impl Deref for ReplyLine {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl Drop for ReplyLine {
    fn drop(&mut self) {
        REPLY_BYTES.fetch_sub(self.0.capacity(), Relaxed);
        REPLIES.fetch_sub(1, Relaxed);
    }
}

impl Agent {
    /// Install the watchdog read on the connection thread so collecting
    /// a requested diagnostic report does not hold a game tick.
    pub fn install_host_memory(&self) {
        mem_watch::register_probe("agent.pending_requests", || REQUESTS.load(Relaxed) as u64);
        mem_watch::register_probe("agent.socket_reply_lines", || REPLIES.load(Relaxed) as u64);
        mem_watch::register_probe("agent.socket_reply_bytes", || {
            REPLY_BYTES.load(Relaxed) as u64
        });
        self.add_connection_read(method::HOST_MEMORY, crate::forensics::snapshot_reply);
        for &name in agent_proto::memory_forensics::METHODS {
            self.add_connection_read(name, move |p| crate::forensics::call(name, p));
        }
    }
}
