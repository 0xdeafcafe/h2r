//! Host reads and counters for the process memory watchdog.
use std::io;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use agent_proto::host_memory::{MemoryParams, MemoryResult};
use agent_proto::{RpcError, code, method};
use serde_json::Value;

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
        self.add_connection_read(method::HOST_MEMORY, snapshot_reply);
        for &name in agent_proto::memory_forensics::METHODS {
            self.add_connection_read(name, move |p| crate::forensics::call(name, p));
        }
    }
}

pub(crate) fn snapshot_reply(params: Value) -> Result<Value, RpcError> {
    memory(
        params,
        || {
            let s = mem_watch::snapshot();
            MemoryResult {
                footprint_bytes: s.footprint_bytes,
                peak_bytes: s.peak_bytes,
                soft: s.soft,
                hard: s.hard,
                growth_bytes_per_min: s.growth_bytes_per_min,
                samples: s.samples,
                probes: s.probes,
                reports: s.reports,
            }
        },
        mem_watch::report,
    )
}

fn memory(
    params: Value,
    snapshot: impl FnOnce() -> MemoryResult,
    report: impl FnOnce() -> io::Result<PathBuf>,
) -> Result<Value, RpcError> {
    let params = if params.is_null() {
        MemoryParams::default()
    } else {
        serde_json::from_value(params)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
    };
    let path =
        if params.report {
            Some(report().map_err(|e| {
                RpcError::new(code::UNAVAILABLE, format!("writing memory report: {e}"))
            })?)
        } else {
            None
        };
    let mut result = snapshot();
    if let Some(path) = path {
        // Put this request's report last even if another completed since.
        result.reports.retain(|old| *old != path);
        result.reports.push(path);
    }
    Ok(serde_json::to_value(result).expect("host memory snapshot serialises"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sampled() -> MemoryResult {
        MemoryResult {
            footprint_bytes: 20,
            peak_bytes: 33,
            soft: 40,
            hard: 80,
            growth_bytes_per_min: -390.0,
            samples: vec![(1000, 33), (3000, 20)],
            probes: [("input.pad_queued_steps".into(), 4)].into(),
            reports: vec!["first.json".into(), "second.json".into()],
        }
    }

    #[test]
    fn reading_a_snapshot_is_passive_and_report_names_its_own_file() {
        let read = memory(Value::Null, sampled, || panic!("passive read wrote a file")).unwrap();
        assert_eq!(
            serde_json::from_value::<MemoryResult>(read).unwrap(),
            sampled()
        );
        let reply = memory(json!({"report":true}), sampled, || Ok("first.json".into())).unwrap();
        assert_eq!(reply["reports"], json!(["second.json", "first.json"]));
    }

    #[test]
    fn invalid_params_and_failed_reports_return_rpc_errors() {
        for bad in [json!({"reports":true}), json!({"report":"yes"}), json!(12)] {
            assert_eq!(
                memory(bad, sampled, || unreachable!()).unwrap_err().code,
                code::INVALID_PARAMS
            );
        }
        let err = memory(
            json!({"report":true}),
            || unreachable!(),
            || Err(io::Error::other("full")),
        )
        .unwrap_err();
        assert_eq!(err.code, code::UNAVAILABLE);
        assert!(err.message.contains("full"));
    }
}
