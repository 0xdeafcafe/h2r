//! Tick-boundary edits to the running map.

use serde::{Deserialize, Serialize};

use crate::At;

/// The requested edit cannot be made in this session or from this placement/file.
/// Same wire value as the editor protocol's REFUSED.
pub const REFUSED: i64 = 11;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectSetParams {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub up: Option<[f32; 3]>,
    /// The map epoch the caller saw (`edit.map`). The host refuses the
    /// edit when the map changed before it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_peers: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectDeleteParams {
    pub id: String,
    /// The map epoch the caller saw (`edit.map`). The host refuses the
    /// edit when the map changed before it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_peers: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub block: String,
    pub index: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectRespawnParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// A tag datum handle in hex: every placed object of that definition is
    /// remade, linked to its placement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// The map epoch the caller saw (`edit.map`). The host refuses the
    /// edit when the map changed before it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_peers: Option<u32>,
}

/// The bytes a record held before the edit: the bytes in hex, or their
/// FNV-1a 64 hash as 16 hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Was {
    Bytes(String),
    Hash { hash: String },
}

/// One record of a placement block to write: `bytes` is exactly one stride, in
/// hex. `was` is checked against the slot's current bytes (a slot past the
/// loaded count holds what the last removal left there, and may omit it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementRecord {
    pub index: u16,
    pub bytes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<Was>,
}

/// Point scenario object name `name` at placement `placement` of the block's
/// type (-1: nothing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementName {
    pub name: u16,
    pub placement: i16,
}

/// Give the live objects of old placement `from` the new placement index `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementRelink {
    pub from: u16,
    pub to: u16,
}

/// One transaction on one scenario placement block (docs/editor/realtime-edits.md,
/// "Placements"). Indices in `delete` and `relink.from` are the old numbering;
/// `records`, `names`, `relink.to` and `create` are the new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementEditParams {
    /// A `placement::BLOCKS` name, or `starting_locations`, `netgame_equipment`,
    /// `netgame_flags`, `starting_equipment` (any `map::scenario_layout` name).
    pub block: String,
    /// The root count the editor's edit started from.
    pub was_count: u32,
    /// The root count after the edit.
    pub count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<PlacementRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<PlacementName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delete: Vec<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relink: Vec<PlacementRelink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub create: Vec<u16>,
    /// The map epoch the caller saw (`edit.map`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_peers: Option<u32>,
}

/// Where a block grew to, when it had to move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementMoved {
    pub address: u32,
    pub capacity: u32,
}

/// What became of one index of `create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementCreated {
    pub index: u16,
    /// The new object's id, when one was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// The object carries its placement index, as a loaded one does.
    pub linked: bool,
    pub created: bool,
    /// Why no object was made: the record is written and shows after a reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementEditResult {
    pub at: At,
    /// The root count now.
    pub count: u32,
    /// How many records the block holds room for.
    pub capacity: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<PlacementMoved>,
    pub deleted: Vec<String>,
    pub relinked: Vec<String>,
    pub created: Vec<PlacementCreated>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapReloadParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_peers: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectSetResult {
    pub at: At,
    pub object: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectDeleteResult {
    pub at: At,
    pub deleted: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectRespawnResult {
    pub at: At,
    pub old: Option<String>,
    pub object: String,
    pub linked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapReloadResult {
    pub queued: bool,
    pub map: String,
    /// The activation to wait for in the public tick stamp.
    #[serde(default)]
    pub activation: u64,
    /// The play copy bound by a file reload, and its v8 header checksum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edit_wire_shapes() {
        let p: ObjectSetParams = serde_json::from_value(json!({
            "id":"crate.abcd0001", "up":[0,1,0]
        })).unwrap();
        assert_eq!(p.up, Some([0.0, 1.0, 0.0]));
        assert!(p.pos.is_none() && p.forward.is_none());
        assert!(serde_json::from_value::<ObjectSetParams>(json!({
            "id":"crate.abcd0001", "velocity":[0,0,0]
        })).is_err());
        let p: ObjectRespawnParams = serde_json::from_value(json!({
            "placement":{"block":"crates","index":2}
        })).unwrap();
        assert_eq!(p.placement.unwrap().index, 2);
        assert!(p.id.is_none() && p.definition.is_none());
        assert!(p.epoch.is_none());
        let p: ObjectRespawnParams =
            serde_json::from_value(json!({"definition":"e1230004"})).unwrap();
        assert_eq!(p.definition.as_deref(), Some("e1230004"));
        let p: ObjectDeleteParams =
            serde_json::from_value(json!({"id":"crate.abcd0001", "epoch": 7})).unwrap();
        assert_eq!(p.epoch, Some(7));
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            json!({"id":"crate.abcd0001", "epoch": 7})
        );
        assert_eq!(serde_json::to_value(MapReloadParams::default()).unwrap(), json!({}));
    }

    #[test]
    fn placement_edit_wire_shapes() {
        let params = json!({
            "block": "scenery", "was_count": 5, "count": 4,
            "records": [{"index": 1, "bytes": "00ff", "was": {"hash": "cbf29ce484222325"}},
                        {"index": 2, "bytes": "00", "was": "ab"}],
            "names": [{"name": 0, "placement": -1}],
            "delete": [1], "relink": [{"from": 4, "to": 1}],
            "confirm_peers": 2
        });
        let parsed: PlacementEditParams = serde_json::from_value(params.clone()).unwrap();
        assert_eq!(parsed.records[0].was, Some(Was::Hash { hash: "cbf29ce484222325".into() }));
        assert_eq!(parsed.records[1].was, Some(Was::Bytes("ab".into())));
        assert_eq!(serde_json::to_value(parsed).unwrap(), params);
        let minimal: PlacementEditParams =
            serde_json::from_value(json!({"block": "crates", "was_count": 0, "count": 0})).unwrap();
        assert_eq!(
            serde_json::to_value(minimal).unwrap(),
            json!({"block": "crates", "was_count": 0, "count": 0})
        );
        assert!(serde_json::from_value::<PlacementEditParams>(
            json!({"block": "crates", "was_count": 0, "count": 0, "capacity": 3})
        ).is_err());
        let created = PlacementCreated {
            index: 3, object: None, linked: false, created: false, why: Some("named".into()),
        };
        assert_eq!(
            serde_json::to_value(created).unwrap(),
            json!({"index": 3, "linked": false, "created": false, "why": "named"})
        );
    }

    #[test]
    fn map_reload_wire_shapes() {
        let file = "/scratch/reload/fixture.map";
        let params = json!({ "file": file, "confirm_peers": 2 });
        let parsed: MapReloadParams = serde_json::from_value(params.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), params);
        for result in [
            json!({ "queued": true, "map": "fixture", "activation": 3 }),
            json!({ "queued": true, "map": "fixture", "activation": 3, "file": file, "checksum": u32::MAX }),
        ] {
            let parsed: MapReloadResult = serde_json::from_value(result.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), result);
        }
        let older: MapReloadResult =
            serde_json::from_value(json!({ "queued": true, "map": "fixture" })).unwrap();
        assert_eq!(older.activation, 0);
    }
}
