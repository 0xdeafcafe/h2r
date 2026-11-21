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
    /// The map epoch the caller saw (`edit.map`). The host refuses the
    /// edit when the map changed before it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_peers: Option<u32>,
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
        assert!(p.id.is_none());
        assert!(p.epoch.is_none());
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
