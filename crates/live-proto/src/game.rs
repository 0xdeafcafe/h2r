//! Game streams (`game-<id>`): event data, frames and the read shapes of
//! `/v1/games` (docs/live-api.md, Events and Clarifications 6-10). The
//! server validates every game event against these types; clients build
//! them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Xuid;

pub const GAME_STARTED: &str = "GameStarted";
pub const PLAYER_JOINED: &str = "PlayerJoined";
pub const PLAYER_LEFT: &str = "PlayerLeft";
pub const PLAYER_KILLED: &str = "PlayerKilled";
pub const SCORE_CHANGED: &str = "ScoreChanged";
pub const GAME_FRAME: &str = "GameFrame";
pub const GAME_ENDED: &str = "GameEnded";
/// A map outline was stored (stream `map-<map>`).
pub const MAP_OUTLINE_STORED: &str = "MapOutlineStored";

/// The game event types a player's console may append in a batch; the
/// server writes `GameStarted` (from `POST /v1/games`) and `GameFrame`.
pub const CLIENT_GAME_EVENTS: [&str; 5] = [
    PLAYER_JOINED,
    PLAYER_LEFT,
    PLAYER_KILLED,
    SCORE_CHANGED,
    GAME_ENDED,
];

/// Most players a game holds (Halo 2 allows 16).
pub const MAX_PLAYERS: usize = 16;

pub fn stream(game_id: &str) -> String {
    format!("game-{game_id}")
}

pub fn map_stream(map: &str) -> String {
    format!("map-{map}")
}

/// Game ids and map names: `[A-Za-z0-9_-]{1,64}` (they are stream names).
pub fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A player of a game. `index` is the game's slot, stable for the game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub index: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xuid: Option<Xuid>,
    pub team: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<bool>,
    /// What the player looks like, as far as the game's memory says
    /// (docs/live-telemetry.md, Player appearance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<Appearance>,
}

/// A player's colours and emblem: `primary`..`quaternary` index the art
/// manifest's 18-colour palette, `emblem_foreground` 0..63 and
/// `emblem_background` 0..31. Every field is independently optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quaternary: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emblem_foreground: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emblem_background: Option<u8>,
}

impl Appearance {
    /// True when no field is known.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `None` when nothing is known, so the key is left out.
    pub fn known(self) -> Option<Self> {
        (!self.is_empty()).then_some(self)
    }
}

/// `POST /v1/games`: `GameStarted` data without `game_id` (or `host`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewGame {
    #[serde(default)]
    pub console: Option<String>,
    pub map: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub team_play: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit_s: Option<i64>,
    #[serde(default)]
    pub tick: Option<u64>,
    pub players: Vec<Player>,
}

/// `GameStarted` data, written by the server: the request plus the id it
/// chose and `host`, the author who started the game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameStarted {
    pub game_id: String,
    pub host: String,
    #[serde(flatten)]
    pub game: NewGame,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerJoined {
    pub player: Player,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerLeft {
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerKilled {
    pub tick: u64,
    pub victim: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killer: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    pub kind: String,
    /// The victim's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f64; 3]>,
}

pub const KILL_KINDS: [&str; 4] = ["kill", "suicide", "betrayal", "world"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerScore {
    pub index: u32,
    pub score: i64,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamScore {
    pub team: i32,
    pub score: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreChanged {
    pub tick: u64,
    pub players: Vec<PlayerScore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams: Option<Vec<TeamScore>>,
}

/// `{team}` in team games, `{player}` otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Winner {
    Team { team: i32 },
    Player { player: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameEnded {
    pub tick: u64,
    pub reason: String,
    #[serde(default)]
    pub winner: Option<Winner>,
    #[serde(default)]
    pub players: Vec<PlayerScore>,
}

pub const END_REASONS: [&str; 3] = ["finished", "quit", "lost"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FramePlayer {
    pub index: u32,
    pub alive: bool,
    pub pos: [f64; 3],
    pub yaw: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<String>,
}

/// One telemetry frame (`tick` is the game tick, 30 per second).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub game_id: String,
    pub tick: u64,
    pub players: Vec<FramePlayer>,
}

/// A map outline (`PUT /v1/maps/{map}/outline`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outline {
    pub map: String,
    pub bounds: Bounds,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub z: [f64; 2],
    pub polygons: Vec<Vec<[f64; 2]>>,
}

/// An entry of `GET /v1/games`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSummary {
    pub game_id: String,
    pub map: String,
    pub mode: String,
    #[serde(default)]
    pub variant: Option<String>,
    pub team_play: bool,
    /// `live` or `ended`.
    pub state: String,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    pub players: Vec<Player>,
    #[serde(default)]
    pub teams: Option<Vec<TeamScore>>,
}

/// A kill as `GET /v1/games/{id}` lists it: the event data plus where it
/// is in the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillRecord {
    #[serde(flatten)]
    pub kill: PlayerKilled,
    pub position: u64,
    pub at: String,
}

/// `GET /v1/games/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameDetail {
    #[serde(flatten)]
    pub summary: GameSummary,
    pub console: Option<String>,
    pub host: String,
    pub score_limit: Option<i64>,
    pub time_limit_s: Option<i64>,
    pub tick: Option<u64>,
    pub last_tick: Option<u64>,
    pub scoreboard: Vec<PlayerScore>,
    /// The last 50, oldest first.
    pub kills: Vec<KillRecord>,
    pub winner: Option<Winner>,
    pub end_reason: Option<String>,
    pub position: u64,
}

/// What `POST /v1/games` returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameCreated {
    pub game_id: String,
    pub position: u64,
}

/// Checks game event data by type; `Err` is a message for a 400.
pub fn validate(kind: &str, data: &Value, players: &[Player]) -> Result<(), String> {
    fn parse<T: serde::de::DeserializeOwned>(data: &Value) -> Result<T, String> {
        T::deserialize(data).map_err(|e| format!("data: {e}"))
    }
    let known = |i: u32| players.iter().any(|p| p.index == i);
    match kind {
        PLAYER_JOINED => {
            let d: PlayerJoined = parse(data)?;
            if known(d.player.index) {
                return Err(format!("player {} is already in the game", d.player.index));
            }
            if players.len() >= MAX_PLAYERS {
                return Err("the game is full".into());
            }
        }
        PLAYER_LEFT => {
            let d: PlayerLeft = parse(data)?;
            if !known(d.index) {
                return Err(format!("no player {}", d.index));
            }
        }
        PLAYER_KILLED => {
            let d: PlayerKilled = parse(data)?;
            if !KILL_KINDS.contains(&d.kind.as_str()) {
                return Err("kind must be kill, suicide, betrayal or world".into());
            }
            if !known(d.victim) || d.killer.is_some_and(|k| !known(k)) {
                return Err("victim and killer must be players of the game".into());
            }
            if d.pos.is_some_and(|p| p.iter().any(|c| !c.is_finite())) {
                return Err("pos must be finite".into());
            }
        }
        SCORE_CHANGED => {
            let d: ScoreChanged = parse(data)?;
            if d.players.iter().any(|s| !known(s.index)) {
                return Err("scores must be for players of the game".into());
            }
        }
        GAME_ENDED => {
            let d: GameEnded = parse(data)?;
            if !END_REASONS.contains(&d.reason.as_str()) {
                return Err("reason must be finished, quit or lost".into());
            }
            if let Some(Winner::Player { player }) = d.winner
                && !known(player)
            {
                return Err("winner must be a player of the game".into());
            }
        }
        _ => return Err(format!("{kind} is not a game event a console writes")),
    }
    Ok(())
}

/// Checks a new game's data.
pub fn validate_new(game: &NewGame) -> Result<(), String> {
    if !valid_id(&game.map) {
        return Err("map must be [A-Za-z0-9_-]{1,64}".into());
    }
    if !matches!(game.mode.as_str(), "campaign" | "multiplayer") {
        return Err("mode must be campaign or multiplayer".into());
    }
    if game.players.len() > MAX_PLAYERS {
        return Err("at most 16 players".into());
    }
    let mut seen = std::collections::HashSet::new();
    if !game.players.iter().all(|p| seen.insert(p.index)) {
        return Err("player indexes must be unique".into());
    }
    if game.players.iter().any(|p| p.name.chars().count() > 32) {
        return Err("player names are at most 32 characters".into());
    }
    Ok(())
}

/// Checks a telemetry frame.
pub fn validate_frame(frame: &Frame) -> Result<(), String> {
    if frame.players.len() > MAX_PLAYERS {
        return Err("at most 16 players".into());
    }
    let finite = |p: &FramePlayer| p.pos.iter().all(|c| c.is_finite()) && p.yaw.is_finite();
    if !frame.players.iter().all(finite) {
        return Err("positions must be finite".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn players() -> Vec<Player> {
        vec![
            Player {
                index: 0,
                name: "A".into(),
                xuid: None,
                team: 0,
                local: Some(true),
                appearance: None,
            },
            Player {
                index: 1,
                name: "B".into(),
                xuid: None,
                team: 1,
                local: None,
                appearance: None,
            },
        ]
    }

    #[test]
    fn game_events_validate() {
        let p = players();
        for (kind, data) in [
            (
                PLAYER_JOINED,
                json!({"player": {"index": 2, "name": "C", "team": 0}}),
            ),
            (PLAYER_LEFT, json!({"index": 1})),
            (
                PLAYER_KILLED,
                json!({"tick": 9, "victim": 1, "killer": 0, "weapon": "br", "kind": "kill", "pos": [1.0, 2.0, 3.0]}),
            ),
            (
                PLAYER_KILLED,
                json!({"tick": 9, "victim": 1, "kind": "world"}),
            ),
            (
                SCORE_CHANGED,
                json!({"tick": 9, "players": [{"index": 0, "score": 1, "kills": 1, "deaths": 0, "assists": 0}], "teams": [{"team": 0, "score": 1}]}),
            ),
            (
                GAME_ENDED,
                json!({"tick": 99, "reason": "finished", "winner": {"team": 0}, "players": []}),
            ),
            (
                GAME_ENDED,
                json!({"tick": 99, "reason": "quit", "winner": null}),
            ),
        ] {
            assert_eq!(validate(kind, &data, &p), Ok(()), "{kind} {data}");
        }
        for (kind, data) in [
            (
                PLAYER_JOINED,
                json!({"player": {"index": 1, "name": "dup", "team": 0}}),
            ),
            (PLAYER_LEFT, json!({"index": 7})),
            (
                PLAYER_KILLED,
                json!({"tick": 9, "victim": 1, "kind": "murder"}),
            ),
            (
                PLAYER_KILLED,
                json!({"tick": 9, "victim": 5, "kind": "kill"}),
            ),
            (PLAYER_KILLED, json!({"victim": 1, "kind": "kill"})),
            (
                SCORE_CHANGED,
                json!({"tick": 9, "players": [{"index": 4, "score": 1, "kills": 1, "deaths": 0, "assists": 0}]}),
            ),
            (GAME_ENDED, json!({"tick": 9, "reason": "bored"})),
            (
                GAME_ENDED,
                json!({"tick": 9, "reason": "finished", "winner": {"player": 9}}),
            ),
            (GAME_STARTED, json!({})),
            (GAME_FRAME, json!({})),
        ] {
            assert!(validate(kind, &data, &p).is_err(), "{kind} {data}");
        }
    }

    #[test]
    fn new_games_and_frames() {
        let g: NewGame = serde_json::from_value(json!({
            "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Slayer",
            "team_play": true, "players": [{"index": 0, "name": "A", "team": 0}]
        }))
        .unwrap();
        assert_eq!(validate_new(&g), Ok(()));
        assert!(
            validate_new(&NewGame {
                map: "a/b".into(),
                ..g.clone()
            })
            .is_err()
        );
        assert!(
            validate_new(&NewGame {
                mode: "firefight".into(),
                ..g.clone()
            })
            .is_err()
        );
        assert!(
            validate_new(&NewGame {
                players: vec![g.players[0].clone(), g.players[0].clone()],
                ..g.clone()
            })
            .is_err()
        );

        let started = GameStarted {
            game_id: "g1".into(),
            host: "acct:x@h".into(),
            game: g,
        };
        let v = serde_json::to_value(&started).unwrap();
        assert_eq!(v["game_id"], "g1");
        assert_eq!(v["map"], "lockout");
        assert_eq!(serde_json::from_value::<GameStarted>(v).unwrap(), started);

        let f: Frame = serde_json::from_value(json!({"game_id": "g1", "tick": 30, "players": [{"index": 0, "alive": true, "pos": [1, 2, 3], "yaw": 1.5}]})).unwrap();
        assert_eq!(validate_frame(&f), Ok(()));
        assert!(valid_id("7f3c-a_B"));
        assert!(!valid_id("") && !valid_id("a b") && !valid_id(&"x".repeat(65)));
    }

    #[test]
    fn winners_are_untagged() {
        assert_eq!(
            serde_json::to_value(Winner::Team { team: 1 }).unwrap(),
            json!({"team": 1})
        );
        assert_eq!(
            serde_json::from_value::<Winner>(json!({"player": 3})).unwrap(),
            Winner::Player { player: 3 }
        );
    }
}
