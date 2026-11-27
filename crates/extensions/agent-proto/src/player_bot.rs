//! `player.bot`: hand `player.0`, the local human seat, to a Clanker brain
//! and back (docs/runtime/agent-api-sim.md, "`player.bot`: a Clanker plays
//! the player's seat"; the glue is `game::player_bot`). Wire types only.

use serde::{Deserialize, Serialize};

/// `player.bot`: turn the brain on or off for `player.0`. An act.
pub const BOT: &str = "player.bot";

/// The methods above.
pub const ALL: &[&str] = &[BOT];

/// `player.bot`'s params. With no params it only reads the state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// `easy`, `normal` (default), `heroic` or `legendary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<String>,
    /// The Clankers page's Aggression: `calm`, `normal` (default) or
    /// `wild`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

/// Where the seat stands, the reply to every call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BotStatus {
    /// The brain has the seat (it may still wait for a game).
    pub on: bool,
    /// The brain's pad went to the game at the last pad read: a
    /// multiplayer game with `player.0` in it.
    #[serde(default)]
    pub driving: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub difficulty: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub style: String,
    /// The name the personality is hashed from: the player's own, once
    /// the brain has seen the seat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Ticks the brain has played since it was turned on.
    #[serde(default)]
    pub ticks: u64,
    /// Why it is off, once it was on: `agent` (`on: false`), `pad` (a
    /// real press), `setting` (`player.bot` turned off) or the reason
    /// the seat could not be played.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_are_optional_and_strict() {
        let p: BotParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!(p, BotParams::default());
        let p: BotParams =
            serde_json::from_value(json!({"on": true, "difficulty": "heroic", "style": "wild"}))
                .unwrap();
        assert_eq!(p.on, Some(true));
        assert_eq!(p.difficulty.as_deref(), Some("heroic"));
        assert_eq!(p.style.as_deref(), Some("wild"));
        assert!(serde_json::from_value::<BotParams>(json!({"on": 1})).is_err());
        assert!(serde_json::from_value::<BotParams>(json!({"seat": 1})).is_err());
        assert_eq!(ALL, [BOT]);
    }

    #[test]
    fn an_off_status_is_small() {
        let v = serde_json::to_value(BotStatus::default()).unwrap();
        assert_eq!(v, json!({"on": false, "driving": false, "ticks": 0}));
        let s = BotStatus {
            on: true,
            difficulty: "normal".into(),
            name: Some("Chief".into()),
            ..BotStatus::default()
        };
        let back: BotStatus = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
