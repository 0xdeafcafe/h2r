use live_proto::game::{Frame, PlayerScore, TeamScore, Winner};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One game event before server-specific signing or file serialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameEvent {
    pub game_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
}

impl GameEvent {
    pub fn new(game_id: impl Into<String>, kind: impl Into<String>, data: impl Serialize) -> Self {
        Self {
            game_id: game_id.into(),
            kind: kind.into(),
            data: serde_json::to_value(data).expect("telemetry event data serializes"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerStats {
    pub score: i64,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub suicides: u32,
}

impl PlayerStats {
    pub const ZERO: Self = Self {
        score: 0,
        kills: 0,
        deaths: 0,
        assists: 0,
        suicides: 0,
    };

    pub fn wire(&self, index: u32) -> PlayerScore {
        PlayerScore {
            index,
            score: self.score,
            kills: self.kills,
            deaths: self.deaths,
            assists: self.assists,
        }
    }
}

/// A player's appearance read from the signed-in profile of their local
/// player slot (docs/live-telemetry.md, "Player appearance"). The game's
/// rank is an Xbox Live attribute, not guest memory, and is not read.
pub use live_proto::game::Appearance;

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub datum: u32,
    pub player: live_proto::game::Player,
    pub appearance: Appearance,
    pub unit: u32,
    pub stats: PlayerStats,
}

impl PlayerState {
    /// The player as roster events send it, with the appearance when any of
    /// it is known.
    pub fn wire_player(&self) -> live_proto::game::Player {
        live_proto::game::Player {
            appearance: self.appearance.known(),
            ..self.player.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectState {
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub vehicle: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameInfo {
    pub map: String,
    pub mode: String,
    pub variant: Option<String>,
    pub team_play: bool,
    pub score_limit: Option<i64>,
    pub time_limit_s: Option<i64>,
    pub tick_rate: u16,
    pub tick: u64,
}

impl GameInfo {
    pub fn is_match(&self) -> bool {
        matches!(self.mode.as_str(), "campaign" | "multiplayer")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub tick: u64,
    pub has_stats: bool,
    pub players: Vec<PlayerState>,
    pub teams: Vec<TeamScore>,
    pub objects: Vec<Option<ObjectState>>,
}

impl Snapshot {
    pub fn scores(&self) -> Vec<PlayerScore> {
        self.players
            .iter()
            .map(|p| p.stats.wire(p.player.index))
            .collect()
    }

    pub fn winner(&self, team_play: bool) -> Option<Winner> {
        if team_play {
            let mut rows = self.teams.clone();
            rows.sort_by_key(|t| t.team);
            let max = rows.iter().map(|t| t.score).max()?;
            let mut best = rows.into_iter().filter(|t| t.score == max);
            let first = best.next()?;
            if best.next().is_some() {
                None
            } else {
                Some(Winner::Team { team: first.team })
            }
        } else {
            let max = self.players.iter().map(|p| p.stats.score).max()?;
            let mut best = self.players.iter().filter(|p| p.stats.score == max);
            let first = best.next()?;
            if best.next().is_some() {
                None
            } else {
                Some(Winner::Player {
                    player: first.player.index,
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Event(GameEvent),
    Frame(Frame),
}
