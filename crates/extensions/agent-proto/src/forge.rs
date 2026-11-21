//! Wire types for Forge's runtime measurements.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::sim::{SessionView, hashes::HashesReply};

pub mod method {
    pub const CAPACITY: &str = "forge.capacity";
    pub const PROBE: &str = "forge.probe";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProbeParams {
    Replication {
        group: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pos: Option<[f64; 3]>,
    },
    Sync {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_tick: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    Ai {},
    Kit {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityReply {
    pub tag_arena: TagArena,
    pub object_pool: ObjectPool,
    pub blocks: BTreeMap<String, BlockCapacity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagArena {
    pub low: u32,
    pub high: u32,
    pub headroom_bytes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectPool {
    pub used: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockCapacity {
    pub count: u32,
    /// Known engine limit. A null limit means it could not be established.
    pub limit: Option<u32>,
    /// Contiguous tail bearing the probe's candidate spare marker. Null
    /// means this block has no identified marker.
    pub kit_left: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeTag {
    pub tag: String,
    pub group: String,
    pub name: Option<String>,
}

/// A probe returns observations. Comparing them across machines or runs
/// is the caller's job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "snake_case")]
pub enum ProbeReply {
    Replication {
        session: SessionView,
        tag: ProbeTag,
        object: String,
        /// Position at creation, in world units, if the engine reports one.
        pos: Option<[f32; 3]>,
    },
    Sync {
        session: SessionView,
        /// Scenario kind from the loaded tag (0 campaign, 1 MP, 2 menu).
        scenario_type: Option<u16>,
        variant_engine: Option<i32>,
        /// The simulation's out-of-sync latch, if a world exists.
        desync: Option<bool>,
        hashes: HashesReply,
    },
    Ai {
        enabled: Option<bool>,
        active: Option<bool>,
        actors: u32,
    },
    Kit {
        blocks: BTreeMap<String, BlockCapacity>,
        objects_by_group: BTreeMap<String, u32>,
        actors: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn probe_params_allow_only_fields_of_the_selected_probe() {
        for value in [
            json!({"what": "replication", "group": "bloc"}),
            json!({"what": "replication", "group": "weap", "tag": "example", "pos": [1.0, 2.0, 3.0]}),
            json!({"what": "sync", "from_tick": 90, "max": 30}),
            json!({"what": "ai"}),
            json!({"what": "kit"}),
        ] {
            assert_eq!(
                to_value(from_value::<ProbeParams>(value.clone()).unwrap()).unwrap(),
                value
            );
        }
        for value in [
            json!({"what": "replication"}),
            json!({"what": "ai", "group": "bloc"}),
            json!({"what": "kit", "max": 10}),
            json!({"what": "other"}),
        ] {
            assert!(from_value::<ProbeParams>(value).is_err());
        }
    }
}
