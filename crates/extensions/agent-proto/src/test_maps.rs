//! `maps.test.*`: test maps built from recipes on the user's own disc and
//! loaded by the running game (docs/game/test-maps.md). The recipe itself is
//! `scenario_edit::recipe::Recipe`; here it travels as JSON.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `maps.test.list`: one map, or every recipe and test map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `maps.test.create`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateParams {
    /// A built-in recipe's name, or a whole recipe as an object.
    pub recipe: Value,
    /// Build it on this disc map in place of the recipe's base; the map is
    /// then called `<recipe>-<base>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Replace a test map of the same name.
    #[serde(default)]
    pub replace: bool,
}

/// `maps.test.load` and `maps.test.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameParams {
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Building,
    Ready,
    Failed,
}

/// A built-in recipe.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeInfo {
    pub name: String,
    pub title: String,
    pub base: String,
    /// `full` or `approximate`.
    pub fidelity: String,
    pub purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Where the game's multiplayer list has a test map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// The list's map id.
    pub id: u32,
    /// The path the game loads it from (the list's slot `+0xb4c`).
    pub path: String,
}

/// One test map: building, built or failed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TestMap {
    pub name: String,
    pub base: String,
    pub state: State,
    /// The package directory under `<data>/maps/`.
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The package's `MapId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_id: Option<String>,
    /// `None` until the map has a slot in the game's multiplayer list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed: Option<Listed>,
    /// What the build did (`scenario_edit::recipe::Built`): the anchor,
    /// what was placed where, notes and the checks for `sim.expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListResult {
    pub recipes: Vec<RecipeInfo>,
    pub maps: Vec<TestMap>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateResult {
    pub name: String,
    pub state: State,
    pub dir: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadResult {
    pub name: String,
    pub map: Listed,
    /// The game's options check (`0x138180`) on what was queued: reported,
    /// not enforced, as the level boot does.
    pub check: bool,
    pub queued: bool,
    /// The build's checks, for `sim.expect` once the map has loaded.
    pub expect: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveResult {
    pub name: String,
    pub removed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_read_the_wire_forms() {
        let p: CreateParams = serde_json::from_value(json!({"recipe": "spawn-grid"})).unwrap();
        assert_eq!(p.recipe, json!("spawn-grid"));
        assert!(!p.replace && p.base.is_none());
        let p: CreateParams = serde_json::from_value(
            json!({"recipe": {"name": "x"}, "base": "lockout", "replace": true}),
        )
        .unwrap();
        assert_eq!(p.base.as_deref(), Some("lockout"));
        assert!(p.replace);
        assert!(serde_json::from_value::<CreateParams>(json!({"recipe": "x", "colour": 1})).is_err());
        assert!(serde_json::from_value::<CreateParams>(json!({})).is_err());
        let n: NameParams = serde_json::from_value(json!({"name": "spawn-grid"})).unwrap();
        assert_eq!(n.name, "spawn-grid");
        let l: ListParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!(l, ListParams::default());
    }

    #[test]
    fn results_say_their_state_in_snake_case() {
        let m = TestMap {
            name: "spawn-grid".into(),
            base: "coagulation".into(),
            state: State::Ready,
            dir: "test-spawn-grid".into(),
            listed: Some(Listed { id: 0x1_0000, path: "t:\\$C\\x\\m".into() }),
            ..TestMap::default()
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["state"], "ready");
        assert_eq!(v["listed"]["id"], 0x1_0000);
        assert!(v.get("error").is_none());
        let back: TestMap = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }
}
