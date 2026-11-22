//! Physical memory use of the process serving the game agent.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryParams {
    pub report: bool,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct MemoryResult {
    pub footprint_bytes: u64,
    pub peak_bytes: u64,
    pub soft: u64,
    pub hard: u64,
    pub growth_bytes_per_min: f64,
    /// Process uptime in milliseconds and footprint in bytes, oldest first.
    pub samples: Vec<(u64, u64)>,
    pub probes: BTreeMap<String, u64>,
    pub reports: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn host_memory_round_trips_the_wire_shape() {
        let wire = json!({
            "footprint_bytes": 8192, "peak_bytes": 16384, "soft": 0, "hard": 12288,
            "growth_bytes_per_min": -245760.0, "samples": [[1000,16384],[3000,8192]],
            "probes": {"agent.pending_requests":1}, "reports":["logs/mem/report.json"]
        });
        let result: MemoryResult = from_value(wire.clone()).unwrap();
        assert_eq!(to_value(result).unwrap(), wire);
        assert!(!from_value::<MemoryParams>(json!({})).unwrap().report);
        assert_eq!(
            to_value(from_value::<MemoryParams>(json!({"report": true})).unwrap()).unwrap(),
            json!({"report": true})
        );
    }
}
