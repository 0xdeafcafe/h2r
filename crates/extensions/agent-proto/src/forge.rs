//! Wire types for Forge's runtime measurements and local session methods.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::sim::{SessionView, hashes::HashesReply};

pub mod method {
    pub const CAPACITY: &str = "forge.capacity";
    pub const STATUS: &str = "forge.status";
    pub const DOC: &str = "forge.doc";
    pub const LOG: &str = "forge.log";
    pub const INPUT: &str = "forge.input";
    pub const PROPOSE: &str = "forge.propose";
    pub const PROBE: &str = "forge.probe";
    pub const LOOK: &str = "forge.look";
    pub const LOOK_SET: &str = "forge.look.set";
    pub const MONITORS: &str = "forge.status.monitors";
    pub const THINGS: &str = "forge.things";
}

/// The kinds of thing `forge.things` names, as its `kind` filter and each
/// thing's `kind` spell them (docs/game/forge-map-variants.md, "Things").
pub const THING_KINDS: [&str; 5] = [
    "object",
    "item_spawner",
    "starting_point",
    "goal",
    "teleporter",
];

/// The names a refused Forge edit carries in its error's `data.code`
/// (docs/runtime/agent-api.md, "Local Forge session").
pub mod refusal {
    /// The live object the edit names no longer resolves by kind and salt.
    pub const OBJECT_GONE: &str = "forge_object_gone";
    /// The edit names a block whose live objects `world.object.*` cannot
    /// resolve.
    pub const OBJECT_KIND: &str = "forge_object_kind";
    /// The own entry's live object is held or attached.
    pub const OBJECT_HELD: &str = "forge_object_held";
    /// The map's entry is not the one the edit was made on: its print
    /// differs.
    pub const PRINT: &str = "forge_print";
    /// The edit names an entry of a block the session could not read.
    pub const LEFT_OUT: &str = "forge_left_out";
    /// The target's value is not the one `was` names.
    pub const WAS: &str = "forge_was";
    /// Any other refusal: the message says why.
    pub const REFUSED: &str = "forge_refused";
}

/// `forge.things`: every thing of the session, or those of one kind, or
/// only those the document changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThingsParams {
    /// One of [`THING_KINDS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `true` lists only the changed things (own entries an entry op
    /// changes, and every added one); `false` only the unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<bool>,
}

/// One monitor input. Directions are normalized axes in camera space.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum InputParams {
    Button {
        action: String,
    },
    Analog {
        movement: Option<[f32; 3]>,
        look: Option<[f32; 2]>,
        roll: Option<f32>,
        #[serde(default)]
        rotating: bool,
    },
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
    fn things_params_take_a_kind_and_changed_and_nothing_else() {
        for value in [
            json!({}),
            json!({"kind": "goal"}),
            json!({"changed": true}),
            json!({"kind": "object", "changed": false}),
        ] {
            assert_eq!(
                to_value(from_value::<ThingsParams>(value.clone()).unwrap()).unwrap(),
                value
            );
        }
        assert!(from_value::<ThingsParams>(json!({"block": "weapons"})).is_err());
        assert!(from_value::<ThingsParams>(json!({"changed": "yes"})).is_err());
    }

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
