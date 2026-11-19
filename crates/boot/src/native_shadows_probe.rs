//! Bounded results from the shadow-map readback. The agent reads completed GPU work.
use std::{collections::VecDeque, sync::Mutex};

use agent_proto::{RpcError, code};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct MapComparison {
    pub frame: u64,
    pub generation: u64,
    pub view: u32,
    pub identity: [u32; 2],
    pub site: &'static str,
    pub kind: &'static str,
    pub copy: &'static str,
    pub size: [u32; 2],
    pub candidates: usize,
    pub projection: [[u32; 4]; 9],
    pub packets: usize,
    pub texels_differ: usize,
    pub first: Option<[usize; 2]>,
}

#[derive(Serialize, Default)]
struct Results {
    completed: u64,
    nonempty: u64,
    mismatched: u64,
    recent: VecDeque<MapComparison>,
}

impl Results {
    fn record(&mut self, map: MapComparison) {
        self.completed += 1;
        self.nonempty += u64::from(map.packets > 0);
        self.mismatched += u64::from(map.texels_differ > 0);
        if self.recent.len() == 32 {
            self.recent.pop_front();
        }
        self.recent.push_back(map);
    }
}
static RESULTS: Mutex<Results> = Mutex::new(Results {
    completed: 0,
    nonempty: 0,
    mismatched: 0,
    recent: VecDeque::new(),
});

pub(crate) fn record(map: MapComparison) {
    RESULTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .record(map);
}

pub(crate) fn read(params: Value) -> Result<Value, RpcError> {
    if !params.is_null() && !params.as_object().is_some_and(|m| m.is_empty()) {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            "render.shadows takes no params",
        ));
    }
    let results = RESULTS.lock().unwrap_or_else(|e| e.into_inner());
    Ok(
        serde_json::json!({ "comparing": crate::native_shadows_glue::comparison_enabled(), "maps": &*results }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_history_is_bounded_and_keeps_empty_and_mismatched_counts() {
        let mut results = Results::default();
        for generation in 0..34 {
            results.record(MapComparison {
                frame: 100,
                generation,
                view: 1,
                identity: [2, 3],
                site: "light",
                kind: "depth",
                copy: "reference",
                size: [4, 4],
                candidates: 1,
                projection: [[0; 4]; 9],
                packets: usize::from(generation != 1),
                texels_differ: usize::from(generation == 3),
                first: (generation == 3).then_some([0, 1]),
            });
        }
        assert_eq!(
            (results.completed, results.nonempty, results.mismatched),
            (34, 33, 1)
        );
        assert_eq!(results.recent.len(), 32);
        assert_eq!(results.recent[0].generation, 2);
        assert_eq!(results.recent[1].first, Some([0, 1]));
        assert!(read(serde_json::json!({"frame": 1})).is_err());
        assert!(read(serde_json::json!({})).unwrap()["maps"]["recent"].is_array());
    }
}
