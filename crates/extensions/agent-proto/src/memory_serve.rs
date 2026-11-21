//! The `host.memory` reads shared by the game and editor socket servers
//! (docs/runtime/agent-api.md, "Finding a leak after the fact"). Reports,
//! archive I/O and symbolization run on the requesting connection thread.
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::host_memory::{MemoryParams, MemoryResult};

use crate::memory_forensics as m;
use crate::{RpcError, code, method};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn unavailable(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(code::UNAVAILABLE, e.to_string())
}

fn params<T: DeserializeOwned>(value: Value) -> Result<T, RpcError> {
    let object = match value {
        Value::Null => Value::Object(Default::default()),
        obj @ Value::Object(_) => obj,
        _ => {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "expected a params object",
            ));
        }
    };
    serde_json::from_value(object).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

pub fn supports(name: &str) -> bool {
    name == method::HOST_MEMORY || m::METHODS.contains(&name)
}

pub fn call(name: &str, value: Value) -> Result<Value, RpcError> {
    if name == method::HOST_MEMORY {
        return snapshot_reply(value);
    }

    // A fan-out of clients should not multiply the workspace for parsing
    // recorder pages and symbolizing stacks while memory is scarce.
    static READING: Mutex<()> = Mutex::new(());
    let _guard = READING
        .try_lock()
        .map_err(|_| unavailable("another memory read is in progress"))?;
    match name {
        m::HISTORY => {
            let req = params::<m::HistoryParams>(value)?;
            let records =
                mem_watch::history_page(req.since_ms, 24, 512 * 1024).map_err(unavailable)?;
            let next_since_ms = records.last().and_then(|r| r.t_ms.checked_add(1));
            Ok(json!({ "records": records, "next_since_ms": next_since_ms }))
        }
        m::HEAP => {
            let req = params::<m::HeapParams>(value)?;
            if !(1..=128).contains(&req.top) {
                return Err(RpcError::new(code::INVALID_PARAMS, "top must be 1 to 128"));
            }
            let heap = mem_watch::heap::snapshot(req.top).map_err(unavailable)?;
            serde_json::to_value(heap).map_err(unavailable)
        }
        m::GPU => {
            params::<m::GpuParams>(value)?;
            serde_json::to_value(mem_watch::gpu::census()).map_err(unavailable)
        }
        m::DIFF => {
            let req = params::<m::DiffParams>(value)?;
            let diff = mem_watch::diff_since(req.from_ms).map_err(unavailable)?;
            serde_json::to_value(diff).map_err(unavailable)
        }
        m::POSTMORTEM => {
            let req = params::<m::PostmortemParams>(value)?;
            let dir = mem_watch::active_report_dir().map_err(unavailable)?;
            let report = mem_watch::recorder::postmortem(&dir, req.pid.map(|pid| pid.get()))
                .map_err(unavailable)?;
            serde_json::to_value(report).map_err(unavailable)
        }
        _ => Err(RpcError::new(code::METHOD_NOT_FOUND, name)),
    }
}

pub fn snapshot_reply(params: Value) -> Result<Value, RpcError> {
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

    #[test]
    fn memory_names_are_exact_and_requests_cannot_select_arbitrary_files() {
        for name in m::METHODS {
            assert!(supports(name));
        }
        assert!(!supports("host.memory.unregistered"));
        assert!(params::<m::PostmortemParams>(json!({"pid":54})).is_ok());
        for value in [
            json!(["file"]),
            json!({"path":"elsewhere"}),
            json!({"pid":0}),
        ] {
            assert_eq!(
                params::<m::PostmortemParams>(value).unwrap_err().code,
                code::INVALID_PARAMS
            );
        }
        assert!(params::<m::DiffParams>(Value::Null).is_err());
    }
}

#[cfg(test)]
mod snapshot_tests {
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
