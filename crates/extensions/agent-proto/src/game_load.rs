//! `game.load` and `game.levels` (docs/runtime/agent-api.md, "Loading a
//! level"): queue any campaign mission or multiplayer map on the disc as
//! the next game, as the level boot starts one (`game_load.rs` in `game`).

use serde::{Deserialize, Serialize};

pub mod method {
    /// Queue a campaign mission or multiplayer map ([`super::LoadParams`]).
    pub const LOAD: &str = "game.load";
    /// The levels `game.load` takes ([`super::Levels`]).
    pub const LEVELS: &str = "game.levels";
    pub const ALL: &[&str] = &[LOAD, LEVELS];
}

/// Campaign difficulties, as `game.load` names them.
pub const DIFFICULTIES: &[&str] = &["easy", "normal", "heroic", "legendary"];

/// Multiplayer game types, as `game.load`'s `variant` names them: index =
/// the `goof` game type (docs/game/community-gametypes.md).
pub const GAME_TYPES: &[&str] = &[
    "slayer",
    "oddball",
    "juggernaut",
    "king",
    "ctf",
    "assault",
    "territories",
];

/// The `goof` game type `name` names (`"koth"` and `"king_of_the_hill"`
/// are King of the Hill), any case.
pub fn game_type(name: &str) -> Option<u32> {
    let n = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    let n = match n.as_str() {
        "koth" | "king_of_the_hill" => "king",
        "capture_the_flag" => "ctf",
        other => other,
    };
    GAME_TYPES.iter().position(|t| *t == n).map(|i| i as u32)
}

/// `game.load`'s params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadParams {
    /// A map file's stem (`03a_oldmombasa`, `lockout`) or a scenario path,
    /// any case; an installed campaign's map by its name (`a30`).
    pub scenario: String,
    /// Campaign only: one of [`DIFFICULTIES`] (default normal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<String>,
    /// Multiplayer only: one of [`GAME_TYPES`], the game type's default
    /// variant (default slayer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// One level `game.load` takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Level {
    /// The map file's stem: what `scenario` takes.
    pub name: String,
    pub scenario: String,
}

/// `game.levels`'s result: the disc's levels, by kind, sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Levels {
    pub campaign: Vec<Level>,
    pub multiplayer: Vec<Level>,
    pub difficulties: Vec<String>,
    pub variants: Vec<String>,
}

/// `game.load`'s result, after the next tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loaded {
    pub name: String,
    pub scenario: String,
    /// `campaign` or `multiplayer`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// The game's options check passed.
    pub check: bool,
    /// The options were queued for the next transition.
    pub queued: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn game_types_by_name() {
        assert_eq!(game_type("Slayer"), Some(0));
        assert_eq!(game_type("ctf"), Some(4));
        assert_eq!(game_type("Capture the Flag"), Some(4));
        assert_eq!(game_type("koth"), Some(3));
        assert_eq!(game_type("king-of-the-hill"), Some(3));
        assert_eq!(game_type("territories"), Some(6));
        assert_eq!(game_type("race"), None);
    }

    #[test]
    fn load_params_need_a_scenario_and_refuse_strays() {
        let p: LoadParams = serde_json::from_value(json!({"scenario": "lockout"})).unwrap();
        assert_eq!((p.difficulty, p.variant), (None, None));
        assert!(serde_json::from_value::<LoadParams>(json!({})).is_err());
        assert!(
            serde_json::from_value::<LoadParams>(json!({"scenario": "x", "map": "y"})).is_err()
        );
    }
}
