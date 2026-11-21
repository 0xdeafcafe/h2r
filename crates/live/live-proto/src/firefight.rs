//! Firefight on Live (docs/game/firefight.md, "Scoring and stats"): the two
//! game events the host reports, and the summary the stats projection
//! reads out of them.
//!
//! `FirefightScore` is one of the director's `Score` commands, as it was
//! credited. `FirefightEnded` closes a match. Both come only from the
//! host and are public; neither feeds ranks (`firefight_scores_do_not_touch_ranks`).
//! The stats projection (`live-server`'s `stats`, which the Worker composes
//! as well) is the one place that folds them into a [`Summary`], so both
//! homes give the same one.

use serde::{Deserialize, Serialize};

/// The `game_type` of a Firefight score in a film's `ObjectiveScore`.
pub const GAME_TYPE: &str = "firefight";

/// Why points were credited, as the director names it.
pub const REASONS: [&str; 2] = ["kill", "assist"];
/// How a match ended.
pub const OUTCOMES: [&str; 2] = ["won", "lost"];

/// Most points one credit carries. A kill is worth hundreds at most.
pub const MAX_POINTS: u32 = 1_000_000;
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
    /// The kit class of the actor killed. A kill carries it; an assist does
    /// not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

/// A match's end: how far it got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirefightEnded {
    pub tick: u64,
    /// The wave set's name.
    pub set: String,
    /// One of [`OUTCOMES`].
    pub outcome: String,
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
            return Err("reason must be kill or assist".into());
        }
        if self.points > MAX_POINTS {
            return Err(format!("points is at most {MAX_POINTS}"));
        }
        match (&self.class, self.reason.as_str()) {
            (Some(c), "kill") if !plain(c) => {
                Err("class is 1 to 32 characters, no control characters".into())
            }
            (Some(_), "assist") => Err("an assist names no class".into()),
            _ => Ok(()),
        }
    }
}

impl FirefightEnded {
    pub fn validate(&self) -> Result<(), String> {
        if !plain(&self.set) {
            return Err("set is 1 to 32 characters, no control characters".into());
        }
        if !OUTCOMES.contains(&self.outcome.as_str()) {
            return Err("outcome must be won or lost".into());
        }
        Ok(())
    }
}

/// Firefight's part of a stats summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Finished matches.
    pub games: u64,
    pub won: u64,
    pub rounds_cleared: u64,
    pub lives_used: u64,
    /// Points credited, over every player.
    pub score: u64,
    /// By matches, then name.
    pub sets: Vec<SetRow>,
    /// By kills, then name.
    pub kills_by_class: Vec<ClassRow>,
    /// Accounts only (a player with an `xuid`), by score, then name.
    pub players: Vec<PlayerRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetRow {
    pub set: String,
    pub games: u64,
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
        assert!(score("bonus", None).validate().is_err());
        assert!(score("assist", Some("grunt")).validate().is_err());
        assert!(score("kill", Some("")).validate().is_err());
        assert!(score("kill", Some("gr\nunt")).validate().is_err());
        let over = FirefightScore {
            points: MAX_POINTS + 1,
            ..score("kill", None)
        };
        assert!(over.validate().is_err());
    }

    #[test]
    fn an_end_names_its_set_and_outcome() {
        let end = |set: &str, outcome: &str| FirefightEnded {
            tick: 9,
            set: set.into(),
            outcome: outcome.into(),
            rounds_cleared: 2,
            lives_used: 3,
        };
        assert_eq!(end("Last Stand", "won").validate(), Ok(()));
        assert!(end("", "won").validate().is_err());
        assert!(end("Holdout", "draw").validate().is_err());
        assert!(end(&"x".repeat(33), "lost").validate().is_err());
    }

    #[test]
    fn unknown_fields_are_refused() {
        let bad =
            serde_json::json!({"tick": 1, "player": 0, "points": 1, "reason": "kill", "extra": 1});
        assert!(serde_json::from_value::<FirefightScore>(bad).is_err());
    }
}
