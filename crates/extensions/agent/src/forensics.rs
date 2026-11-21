//! Memory reads shared by the game and studio socket servers. Reports,
//! archive I/O and symbolization run on the requesting connection thread.
use std::sync::Mutex;

use agent_proto::memory_forensics as m;
use agent_proto::{RpcError, code, method};
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
        return crate::memory::snapshot_reply(value);
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
