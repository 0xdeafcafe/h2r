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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FramePlayer {
    pub index: u32,
    pub alive: bool,
    pub pos: [f64; 3],
    pub yaw: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<String>,
    /// The player's biped, from its definition tag: `"spartan"` or
    /// `"elite"`; other bipeds pass through as a short id (readers fall back
    /// to spartan).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub species: Option<String>,
    /// Where the unit aims, in game coordinates (z up), unit length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<[f64; 3]>,
    /// The held weapon's tag file stem, lower case (`"battle_rifle"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    /// Shots the held weapon fired since this player's previous frame; zero
    /// is sent as absent.
    #[serde(default, skip_serializing_if = "no_shots")]
    pub shots: Option<u32>,
}

fn no_shots(shots: &Option<u32>) -> bool {
    shots.is_none_or(|n| n == 0)
}

/// Most shots one frame may report (frames are 15 Hz).
pub const MAX_FRAME_SHOTS: u32 = 64;

/// A frame's `species` or `weapon` id: `[a-z0-9_]{1,32}`.
pub fn valid_model_id(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// One telemetry frame (`tick` is the game tick, 30 per second).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub game_id: String,
    pub tick: u64,
    pub players: Vec<FramePlayer>,
    /// Vehicles with someone in them (players or AI). `FramePlayer.vehicle`
    /// and `FrameActor.vehicle` hold the matching `id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vehicles: Vec<FrameVehicle>,
    /// AI-controlled bipeds, alive ones only, at most [`MAX_FRAME_ACTORS`]
    /// (nearest to any player first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<FrameActor>,
}

/// An occupied vehicle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameVehicle {
    /// The vehicle's object datum in hex, as `FramePlayer.vehicle`.
    pub id: String,
    /// The `vehi` tag file stem (`warthog`, `ghost`, `banshee`, ...), or
    /// `unknown` when the name is not known.
    pub kind: String,
    pub pos: [f64; 3],
    /// Unit length, game coordinates (z up).
    pub forward: [f64; 3],
    /// Unit length; banshees roll and pitch.
    pub up: [f64; 3],
}

/// An AI-controlled biped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameActor {
    /// The biped's object datum in hex.
    pub id: String,
    /// The directory of its biped tag (`grunt`, `jackal`, `elite`, `marine`,
    /// ...), or `unknown` when the name is not known.
    pub species: String,
    pub pos: [f64; 3],
    pub yaw: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    #[serde(default, skip_serializing_if = "no_shots")]
    pub shots: Option<u32>,
    /// `covenant`, `human`, `flood`, `sentinel`, `prophet`, `player`, ... or
    /// the raw team index as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<String>,
}

/// Most actors one frame may carry.
pub const MAX_FRAME_ACTORS: usize = 128;
/// Most vehicles one frame may carry.
pub const MAX_FRAME_VEHICLES: usize = 64;

/// Who fired, for carrying shots between frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shooter {
    Player(u32),
    Actor(String),
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
    for p in &frame.players {
        if let Some(aim) = p.aim
            && (!aim.iter().all(|c| c.is_finite()) || length(aim) < 1e-6)
        {
            return Err("aim must be a finite, non-zero vector".into());
        }
        let ids = [&p.species, &p.weapon];
        if ids.into_iter().flatten().any(|id| !valid_model_id(id)) {
            return Err("species and weapon must be [a-z0-9_]{1,32}".into());
        }
        if p.shots.is_some_and(|n| n > MAX_FRAME_SHOTS) {
            return Err(format!("at most {MAX_FRAME_SHOTS} shots a frame"));
        }
        if p.vehicle.as_deref().is_some_and(|id| !valid_model_id(id)) {
            return Err("vehicle ids must be [a-z0-9_]{1,32}".into());
        }
    }
    if frame.vehicles.len() > MAX_FRAME_VEHICLES {
        return Err(format!("at most {MAX_FRAME_VEHICLES} vehicles"));
    }
    if frame.actors.len() > MAX_FRAME_ACTORS {
        return Err(format!("at most {MAX_FRAME_ACTORS} actors"));
    }
    let direction = |v: [f64; 3]| v.iter().all(|c| c.is_finite()) && length(v) >= 1e-6;
    for v in &frame.vehicles {
        if !valid_model_id(&v.id) || !valid_model_id(&v.kind) {
            return Err("vehicle id and kind must be [a-z0-9_]{1,32}".into());
        }
        if !v.pos.iter().all(|c| c.is_finite()) || !direction(v.forward) || !direction(v.up) {
            return Err("vehicle pos must be finite, forward and up finite and non-zero".into());
        }
    }
    for a in &frame.actors {
        let ids = [
            Some(&a.id),
            Some(&a.species),
            a.weapon.as_ref(),
            a.team.as_ref(),
            a.vehicle.as_ref(),
        ];
        if ids.into_iter().flatten().any(|id| !valid_model_id(id)) {
            return Err("actor ids must be [a-z0-9_]{1,32}".into());
        }
        if !a.pos.iter().all(|c| c.is_finite()) || !a.yaw.is_finite() {
            return Err("actor positions must be finite".into());
        }
        if a.aim.is_some_and(|aim| !direction(aim)) {
            return Err("aim must be a finite, non-zero vector".into());
        }
        if a.shots.is_some_and(|n| n > MAX_FRAME_SHOTS) {
            return Err(format!("at most {MAX_FRAME_SHOTS} shots a frame"));
        }
    }
    Ok(())
}

fn length(v: [f64; 3]) -> f64 {
    v.iter().map(|c| c * c).sum::<f64>().sqrt()
}

impl Frame {
    /// Each player's and actor's shot count, for carrying into a later
    /// frame when this one is not delivered.
    pub fn shots(&self) -> impl Iterator<Item = (Shooter, u32)> + '_ {
        let players = self
            .players
            .iter()
            .filter_map(|p| Some((Shooter::Player(p.index), p.shots.filter(|n| *n > 0)?)));
        let actors = self
            .actors
            .iter()
            .filter_map(|a| Some((Shooter::Actor(a.id.clone()), a.shots.filter(|n| *n > 0)?)));
        players.chain(actors)
    }

    /// Adds shots from frames that were dropped (coalesced or throttled) to
    /// this one, so `shots` stays "since the previous delivered frame".
    /// Shooters not in this frame lose theirs; counts cap at
    /// [`MAX_FRAME_SHOTS`].
    pub fn add_shots(&mut self, shots: impl IntoIterator<Item = (Shooter, u32)>) {
        fn add(slot: &mut Option<u32>, n: u32) {
            let total = slot.unwrap_or(0).saturating_add(n).min(MAX_FRAME_SHOTS);
            *slot = (total > 0).then_some(total);
        }
        for (shooter, n) in shots {
            match shooter {
                Shooter::Player(index) => {
                    if let Some(p) = self.players.iter_mut().find(|p| p.index == index) {
                        add(&mut p.shots, n);
                    }
                }
                Shooter::Actor(id) => {
                    if let Some(a) = self.actors.iter_mut().find(|a| a.id == id) {
                        add(&mut a.shots, n);
                    }
                }
            }
        }
    }
}

/// Puts a validated frame in its stored form: every `aim` unit length and a
/// zero `shots` absent.
pub fn normalize_frame(frame: &mut Frame) {
    fn unit(v: &mut [f64; 3]) {
        let len = length(*v);
        if len > 0.0 {
            v.iter_mut().for_each(|c| *c /= len);
        }
    }
    for v in &mut frame.vehicles {
        unit(&mut v.forward);
        unit(&mut v.up);
    }
    for a in &mut frame.actors {
        if let Some(aim) = &mut a.aim {
            unit(aim);
        }
        if a.shots == Some(0) {
            a.shots = None;
        }
    }
    for p in &mut frame.players {
        if let Some(aim) = &mut p.aim {
            let len = length(*aim);
            if len > 0.0 {
                aim.iter_mut().for_each(|c| *c /= len);
            }
        }
        if p.shots == Some(0) {
            p.shots = None;
        }
    }
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
    fn frame_players_carry_species_aim_weapon_and_shots() {
        let wire = json!({"game_id": "g1", "tick": 30, "players": [
            {"index": 0, "alive": true, "pos": [1.0, 2.0, 3.0], "yaw": 1.5,
             "species": "elite", "aim": [0.6, 0.8, 0.0], "weapon": "battle_rifle", "shots": 3},
            {"index": 1, "alive": false, "pos": [0.0, 0.0, 0.0], "yaw": 0.0}
        ]});
        let frame: Frame = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(frame.players[0].species.as_deref(), Some("elite"));
        assert_eq!(frame.players[0].aim, Some([0.6, 0.8, 0.0]));
        assert_eq!(frame.players[0].weapon.as_deref(), Some("battle_rifle"));
        assert_eq!(frame.players[0].shots, Some(3));
        assert_eq!(
            frame.players[1],
            FramePlayer {
                index: 1,
                ..FramePlayer::default()
            }
        );
        assert_eq!(validate_frame(&frame), Ok(()));
        // Round trip: absent fields stay absent, present ones keep their shape.
        assert_eq!(serde_json::to_value(&frame).unwrap(), wire);

        // Zero shots are sent as absent.
        let quiet = FramePlayer {
            shots: Some(0),
            ..FramePlayer::default()
        };
        assert!(serde_json::to_value(&quiet).unwrap().get("shots").is_none());
    }

    fn campaign_frame() -> Value {
        json!({"game_id": "g1", "tick": 30,
        "players": [{"index": 0, "alive": true, "pos": [1.0, 2.0, 3.0], "yaw": 0.5,
                     "vehicle": "90040004"}],
        "vehicles": [{"id": "90040004", "kind": "warthog", "pos": [1.0, 2.0, 3.0],
                      "forward": [1.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0]}],
        "actors": [
            {"id": "90070007", "species": "grunt", "pos": [5.0, 6.0, 7.0], "yaw": 1.0,
             "aim": [0.0, 1.0, 0.0], "weapon": "plasma_pistol", "shots": 2,
             "team": "covenant"},
            {"id": "90080008", "species": "marine", "pos": [1.0, 2.0, 3.0], "yaw": 0.0,
             "team": "human", "vehicle": "90040004"}
        ]})
    }

    #[test]
    fn frames_carry_vehicles_and_actors() {
        let wire = campaign_frame();
        let frame: Frame = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(frame.vehicles[0].kind, "warthog");
        assert_eq!(frame.actors[0].team.as_deref(), Some("covenant"));
        assert_eq!(frame.actors[1].vehicle.as_deref(), Some("90040004"));
        assert_eq!(validate_frame(&frame), Ok(()));
        assert_eq!(serde_json::to_value(&frame).unwrap(), wire);
        // Old frames have neither, and they are not sent empty.
        let old: Frame =
            serde_json::from_value(json!({"game_id": "g1", "tick": 2, "players": []})).unwrap();
        assert!(old.vehicles.is_empty() && old.actors.is_empty());
        let sent = serde_json::to_value(&old).unwrap();
        assert!(sent.get("vehicles").is_none() && sent.get("actors").is_none());
    }

    #[test]
    fn vehicle_and_actor_validation() {
        let good: Frame = serde_json::from_value(campaign_frame()).unwrap();
        let mut bad = Vec::new();
        let mut f = good.clone();
        f.vehicles[0].up = [0.0; 3];
        bad.push(f);
        let mut f = good.clone();
        f.vehicles[0].forward[0] = f64::NAN;
        bad.push(f);
        let mut f = good.clone();
        f.vehicles[0].kind = "Warthog".into();
        bad.push(f);
        let mut f = good.clone();
        f.vehicles[0].pos[2] = f64::INFINITY;
        bad.push(f);
        let mut f = good.clone();
        f.vehicles = vec![good.vehicles[0].clone(); MAX_FRAME_VEHICLES + 1];
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].species = String::new();
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].team = Some("-1".into());
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].aim = Some([f64::NAN, 0.0, 0.0]);
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].yaw = f64::NAN;
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].shots = Some(MAX_FRAME_SHOTS + 1);
        bad.push(f);
        let mut f = good.clone();
        f.actors = vec![good.actors[1].clone(); MAX_FRAME_ACTORS + 1];
        bad.push(f);
        let mut f = good.clone();
        f.players[0].vehicle = Some("not hex!".into());
        bad.push(f);
        for f in bad {
            assert!(validate_frame(&f).is_err(), "{f:?}");
        }
        let mut f = good;
        f.vehicles[0].up = [0.0, 0.0, 2.0];
        f.actors[0].aim = Some([0.0, 3.0, 4.0]);
        f.actors[0].shots = Some(0);
        normalize_frame(&mut f);
        assert_eq!(f.vehicles[0].up, [0.0, 0.0, 1.0]);
        assert_eq!(f.actors[0].aim, Some([0.0, 0.6, 0.8]));
        assert_eq!(f.actors[0].shots, None);
    }

    #[test]
    fn dropped_frames_carry_their_shots_forward() {
        let player = |index, shots| FramePlayer {
            index,
            shots,
            ..FramePlayer::default()
        };
        let actor = |id: &str, shots| FrameActor {
            id: id.into(),
            species: "grunt".into(),
            shots,
            ..FrameActor::default()
        };
        let dropped = Frame {
            game_id: "g1".into(),
            tick: 2,
            players: vec![player(0, Some(2)), player(1, None), player(3, Some(5))],
            actors: vec![actor("9001000a", Some(4)), actor("9002000b", Some(1))],
            ..Frame::default()
        };
        let mut next = Frame {
            game_id: "g1".into(),
            tick: 4,
            players: vec![
                player(0, Some(1)),
                player(1, None),
                player(2, Some(MAX_FRAME_SHOTS)),
            ],
            actors: vec![actor("9001000a", None)],
            ..Frame::default()
        };
        assert_eq!(
            dropped.shots().collect::<Vec<_>>(),
            [
                (Shooter::Player(0), 2),
                (Shooter::Player(3), 5),
                (Shooter::Actor("9001000a".into()), 4),
                (Shooter::Actor("9002000b".into()), 1),
            ]
        );
        next.add_shots(dropped.shots().collect::<Vec<_>>());
        next.add_shots([(Shooter::Player(2), 9)]);
        assert_eq!(
            next.players.iter().map(|p| p.shots).collect::<Vec<_>>(),
            [Some(3), None, Some(MAX_FRAME_SHOTS)],
            "player 3 left; counts cap"
        );
        assert_eq!(next.actors[0].shots, Some(4), "actor 9002000b is gone");
    }

    #[test]
    fn frame_validation_rejects_bad_aim_ids_and_shot_counts() {
        let with = |p: FramePlayer| Frame {
            game_id: "g1".into(),
            tick: 2,
            players: vec![p],
            ..Frame::default()
        };
        let ok = FramePlayer {
            species: Some("spartan".into()),
            aim: Some([0.0, 0.0, 2.0]),
            weapon: Some("smg".into()),
            shots: Some(MAX_FRAME_SHOTS),
            ..FramePlayer::default()
        };
        assert_eq!(validate_frame(&with(ok.clone())), Ok(()));
        for bad in [
            FramePlayer {
                aim: Some([f64::NAN, 0.0, 1.0]),
                ..ok.clone()
            },
            FramePlayer {
                aim: Some([f64::INFINITY, 0.0, 0.0]),
                ..ok.clone()
            },
            FramePlayer {
                aim: Some([0.0; 3]),
                ..ok.clone()
            },
            FramePlayer {
                species: Some("Elite".into()),
                ..ok.clone()
            },
            FramePlayer {
                weapon: Some("objects\\weapons\\smg".into()),
                ..ok.clone()
            },
            FramePlayer {
                weapon: Some(String::new()),
                ..ok.clone()
            },
            FramePlayer {
                weapon: Some("x".repeat(33)),
                ..ok.clone()
            },
            FramePlayer {
                shots: Some(MAX_FRAME_SHOTS + 1),
                ..ok.clone()
            },
        ] {
            assert!(validate_frame(&with(bad.clone())).is_err(), "{bad:?}");
        }

        // Stored frames have unit aim and no zero shot count.
        let mut frame = with(FramePlayer {
            shots: Some(0),
            ..ok
        });
        normalize_frame(&mut frame);
        assert_eq!(frame.players[0].aim, Some([0.0, 0.0, 1.0]));
        assert_eq!(frame.players[0].shots, None);
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
