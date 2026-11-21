//! Request types for the process memory flight recorder. All time filters
//! use the recorder's monotonic process uptime in milliseconds.
use serde::{Deserialize, Serialize};

pub const HISTORY: &str = "host.memory.history";
pub const HEAP: &str = "host.memory.heap";
pub const GPU: &str = "host.memory.gpu";
pub const DIFF: &str = "host.memory.diff";
pub const POSTMORTEM: &str = "host.memory.postmortem";
pub const METHODS: &[&str] = &[HISTORY, HEAP, GPU, DIFF, POSTMORTEM];

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryParams {
    pub since_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HeapParams {
    #[serde(default = "default_top")]
    pub top: usize,
}

fn default_top() -> usize {
    20
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GpuParams {}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiffParams {
    pub from_ms: u64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PostmortemParams {
    pub pid: Option<std::num::NonZeroU32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn recorder_filters_and_heap_defaults_match_the_wire() {
        let wire = json!({"since_ms":12345});
        assert_eq!(
            to_value(from_value::<HistoryParams>(wire.clone()).unwrap()).unwrap(),
            wire
        );
        assert_eq!(from_value::<HeapParams>(json!({})).unwrap().top, 20);
        assert!(from_value::<DiffParams>(json!({})).is_err());
        assert!(from_value::<DiffParams>(json!({"from_ms":-1})).is_err());
        assert!(from_value::<GpuParams>(json!({"device":1})).is_err());
        assert!(from_value::<PostmortemParams>(json!({"pid":0})).is_err());
        assert!(from_value::<PostmortemParams>(json!({"file":"/elsewhere"})).is_err());
    }
}
