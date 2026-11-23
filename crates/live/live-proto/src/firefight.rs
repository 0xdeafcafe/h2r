//! Firefight on Live (docs/game/firefight.md, "Scoring and stats"): the two
//! game events the host reports, and the summary the stats projection
//! reads out of them.
//!
//! `FirefightScore` is one of the director's `Score` commands, as it was
//! credited. `FirefightEnded` closes a match: one player team against the
//! AI, or two that also fight each other. Both come only from the host and
//! are public; neither feeds ranks (`firefight_scores_do_not_touch_ranks`).
//! The stats projection (`live-server`'s `stats`, which the Worker composes
//! as well) is the one place that folds them into a [`Summary`], so both
//! homes give the same one.

use serde::{Deserialize, Serialize};

/// The `game_type` of a Firefight score in a film's `ObjectiveScore`.
pub const GAME_TYPE: &str = "firefight";

/// Why points were credited, as the director names it: an actor killed,
/// an assist on one, or (two teams) a player of the other team killed.
pub const REASONS: [&str; 3] = ["kill", "assist", "player_kill"];
/// How a match ended. One team: `won` or `lost`. Two teams: `team_won`
/// (with the team), `draw`, or `lost` when every team present ran out of
/// lives.
pub const OUTCOMES: [&str; 4] = ["won", "lost", "team_won", "draw"];
/// The most player teams a match has.
pub const MAX_TEAMS: u8 = 2;

/// Most points one credit carries. A kill is worth hundreds at most.
pub const MAX_POINTS: u32 = 1_000_000;
/// Most points a team's total in a result carries.
pub const MAX_SCORE: u32 = 100_000_000;
/// Most rows a summary lists in each of its tables.
pub const MAX_ROWS: usize = 64;

/// One `Score` command, as credited to a player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirefightScore {
    pub tick: u64,
    /// The credited player's roster slot.
    pub player: u32,
    pub points: u32,
    /// One of [`REASONS`].
    pub reason: String,
    /// The kit class of the actor killed. A kill carries it; an assist and
    /// a player kill do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

/// A match's end: how far it got, and who won.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirefightEnded {
    pub tick: u64,
    /// The wave set's name.
    pub set: String,
    /// Player teams: 1 or 2 ([`MAX_TEAMS`]).
    pub teams: u8,
    /// One of [`OUTCOMES`].
    pub outcome: String,
    /// The winning team (0 or 1), on `team_won` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
    /// Each team's points, by team: one entry per team.
    pub scores: Vec<u32>,
    /// Rounds whose last wave was cleared.
    pub rounds_cleared: u32,
    /// Lives the players spent, summed.
    pub lives_used: u32,
}

/// A name that is one to 32 printable characters.
fn plain(s: &str) -> bool {
    (1..=32).contains(&s.chars().count()) && !s.chars().any(char::is_control)
}

impl FirefightScore {
    pub fn validate(&self) -> Result<(), String> {
        if !REASONS.contains(&self.reason.as_str()) {
            return Err("reason must be kill, assist or player_kill".into());
        }
        if self.points > MAX_POINTS {
            return Err(format!("points is at most {MAX_POINTS}"));
        }
        match (&self.class, self.reason.as_str()) {
            (Some(c), "kill") if !plain(c) => {
                Err("class is 1 to 32 characters, no control characters".into())
            }
            (Some(_), "assist") => Err("an assist names no class".into()),
            (Some(_), "player_kill") => Err("a player kill names no class".into()),
            _ => Ok(()),
        }
    }
}

impl FirefightEnded {
    pub fn validate(&self) -> Result<(), String> {
        if !plain(&self.set) {
            return Err("set is 1 to 32 characters, no control characters".into());
        }
        if !(1..=MAX_TEAMS).contains(&self.teams) {
            return Err(format!("teams is 1 to {MAX_TEAMS}"));
        }
        if self.scores.len() != usize::from(self.teams) {
            return Err("scores has one entry per team".into());
        }
        if self.scores.iter().any(|&s| s > MAX_SCORE) {
            return Err(format!("a team's score is at most {MAX_SCORE}"));
        }
        let two = self.teams == 2;
        match (self.outcome.as_str(), self.team) {
            ("won", None) if !two => Ok(()),
            ("lost", None) => Ok(()),
            ("team_won", Some(t)) if two && t < self.teams => Ok(()),
            ("draw", None) if two => Ok(()),
            ("won", _) => Err("won is one team's outcome and names no team".into()),
            ("team_won", _) => Err("team_won is two teams' outcome, with the team 0 or 1".into()),
            ("draw", _) => Err("draw is two teams' outcome and names no team".into()),
            ("lost", Some(_)) => Err("only team_won names a team".into()),
            _ => Err("outcome must be won, lost, team_won or draw".into()),
        }
    }

    /// The winning team of a two-team match: `team_won`'s team.
    pub fn winner(&self) -> Option<u8> {
        (self.outcome == "team_won").then_some(self.team).flatten()
    }
}

/// Firefight's part of a stats summary.
///
/// `games` counts every finished match, of one team or two; `won` counts
/// one team's wins against the AI. Two-team matches are counted again in
/// [`Summary::versus`], by which team won.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Finished matches.
    pub games: u64,
    /// One-team matches won.
    pub won: u64,
    pub rounds_cleared: u64,
    pub lives_used: u64,
    /// Points credited, over every player.
    pub score: u64,
    /// Players of the other team killed, over every player (two teams).
    pub player_kills: u64,
    /// The finished two-team matches.
    pub versus: VersusRow,
    /// By matches, then name.
    pub sets: Vec<SetRow>,
    /// By kills, then name.
    pub kills_by_class: Vec<ClassRow>,
    /// Accounts only (a player with an `xuid`), by score, then name.
    pub players: Vec<PlayerRow>,
}

/// Finished two-team matches: how many, each team's wins and the draws.
/// A match both teams lost counts in `games` alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersusRow {
    pub games: u64,
    /// By team: team 0's wins, then team 1's.
    pub wins: [u64; 2],
    pub draws: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetRow {
    pub set: String,
    pub games: u64,
    /// One-team wins ([`Summary::won`]).
    pub won: u64,
    pub rounds_cleared: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassRow {
    pub class: String,
    pub kills: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerRow {
    pub xuid: String,
    pub name: String,
    pub score: u64,
    pub kills: u64,
    pub assists: u64,
    /// Players of the other team killed.
    pub player_kills: u64,
    pub games: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(reason: &str, class: Option<&str>) -> FirefightScore {
        FirefightScore {
            tick: 1,
            player: 0,
            points: 100,
            reason: reason.into(),
            class: class.map(Into::into),
        }
    }

    #[test]
    fn a_score_names_a_known_reason_and_a_class_only_on_a_kill() {
        assert_eq!(score("kill", Some("grunt")).validate(), Ok(()));
        assert_eq!(score("assist", None).validate(), Ok(()));
        assert_eq!(score("player_kill", None).validate(), Ok(()));
        assert!(score("bonus", None).validate().is_err());
        assert!(score("assist", Some("grunt")).validate().is_err());
        assert!(score("player_kill", Some("grunt")).validate().is_err());
        assert!(score("kill", Some("")).validate().is_err());
        assert!(score("kill", Some("gr\nunt")).validate().is_err());
        let over = FirefightScore {
            points: MAX_POINTS + 1,
            ..score("kill", None)
        };
        assert!(over.validate().is_err());
    }

    fn end(set: &str, teams: u8, outcome: &str, team: Option<u8>) -> FirefightEnded {
        FirefightEnded {
            tick: 9,
            set: set.into(),
            teams,
            outcome: outcome.into(),
            team,
            scores: vec![100; usize::from(teams)],
            rounds_cleared: 2,
            lives_used: 3,
        }
    }

    #[test]
    fn an_end_names_its_set_and_outcome() {
        assert_eq!(end("Last Stand", 1, "won", None).validate(), Ok(()));
        assert_eq!(end("Holdout", 1, "lost", None).validate(), Ok(()));
        assert!(end("", 1, "won", None).validate().is_err());
        assert!(end("Holdout", 1, "drawn", None).validate().is_err());
        assert!(end(&"x".repeat(33), 1, "lost", None).validate().is_err());
    }

    #[test]
    fn a_two_team_end_names_the_winner_or_a_draw() {
        let won = end("Crossfire", 2, "team_won", Some(1));
        assert_eq!(won.validate(), Ok(()));
        assert_eq!(won.winner(), Some(1));
        assert_eq!(end("Crossfire", 2, "draw", None).validate(), Ok(()));
        assert_eq!(end("Crossfire", 2, "lost", None).validate(), Ok(()));
        assert_eq!(end("Crossfire", 2, "draw", None).winner(), None);
        // A team out of range, a winner on one team, a team on a draw.
        assert!(end("Crossfire", 2, "team_won", Some(2)).validate().is_err());
        assert!(end("Crossfire", 2, "team_won", None).validate().is_err());
        assert!(end("Holdout", 1, "team_won", Some(0)).validate().is_err());
        assert!(end("Holdout", 1, "draw", None).validate().is_err());
        assert!(end("Crossfire", 2, "won", None).validate().is_err());
        assert!(end("Crossfire", 2, "draw", Some(0)).validate().is_err());
        assert!(end("Crossfire", 2, "lost", Some(0)).validate().is_err());
        // One score per team, teams 1 or 2.
        assert!(end("Crossfire", 3, "draw", None).validate().is_err());
        assert!(end("Holdout", 0, "lost", None).validate().is_err());
        let short = FirefightEnded {
            scores: vec![5],
            ..end("Crossfire", 2, "draw", None)
        };
        assert!(short.validate().is_err());
        let over = FirefightEnded {
            scores: vec![MAX_SCORE + 1],
            ..end("Holdout", 1, "won", None)
        };
        assert!(over.validate().is_err());
    }

    #[test]
    fn an_end_reads_the_wire_names() {
        let v = serde_json::json!({
            "tick": 800, "set": "Crossfire", "teams": 2, "outcome": "team_won",
            "team": 0, "scores": [450, 300], "rounds_cleared": 3, "lives_used": 4
        });
        let e: FirefightEnded = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(e.winner(), Some(0));
        assert_eq!(serde_json::to_value(&e).unwrap(), v);
    }

    #[test]
    fn unknown_fields_are_refused() {
        let bad =
            serde_json::json!({"tick": 1, "player": 0, "points": 1, "reason": "kill", "extra": 1});
        assert!(serde_json::from_value::<FirefightScore>(bad).is_err());
    }
}
