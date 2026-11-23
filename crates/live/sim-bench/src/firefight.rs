//! A Firefight match file (docs/runtime/sim.md, "Firefight"):
//! `game_type = "firefight"` and its `[firefight]` table, the seat rule
//! for the bots, the choices the game reads, and the report's section.
//!
//! A Firefight match is a Slayer game with team play on, on a map that
//! carries the Firefight kit (docs/game/firefight.md). The bots are the
//! players: they sit on the player teams `0..teams` only, never on the
//! AI's team, which is not a seat.

use serde::{Deserialize, Serialize};

/// The match file's `game_type` for Firefight.
pub const GAME_TYPE: &str = "firefight";

/// Whether a match file's `game_type` names Firefight.
pub fn is_game_type(s: &str) -> bool {
    s.trim().eq_ignore_ascii_case(GAME_TYPE)
}

/// A built-in wave set, in the game's menu order
/// (`firefight::builtin::definitions`): the position is the
/// `firefight.waveset` choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Set {
    #[default]
    Holdout,
    Attrition,
    LastStand,
    Crossfire,
}

impl Set {
    pub const ALL: [Set; 4] = [Set::Holdout, Set::Attrition, Set::LastStand, Set::Crossfire];

    pub fn parse(s: &str) -> Result<Set, String> {
        let k: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        Set::ALL
            .into_iter()
            .find(|set| set.key().replace('_', "") == k)
            .ok_or_else(|| {
                format!("firefight.set = {s:?}: holdout, attrition, last_stand or crossfire")
            })
    }

    /// The match file's name for it.
    pub fn key(self) -> &'static str {
        match self {
            Set::Holdout => "holdout",
            Set::Attrition => "attrition",
            Set::LastStand => "last_stand",
            Set::Crossfire => "crossfire",
        }
    }

    /// The definition's name.
    pub fn name(self) -> &'static str {
        match self {
            Set::Holdout => "Holdout",
            Set::Attrition => "Attrition",
            Set::LastStand => "Last Stand",
            Set::Crossfire => "Crossfire",
        }
    }

    /// The `firefight.waveset` choice: the position in the menu.
    pub fn index(self) -> i64 {
        Set::ALL.iter().position(|s| *s == self).unwrap_or(0) as i64
    }

    /// The player teams the set has of its own: Crossfire two, the rest one.
    pub fn own_teams(self) -> u8 {
        match self {
            Set::Crossfire => 2,
            _ => 1,
        }
    }
}

/// How hard: the `firefight.difficulty` choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    #[default]
    Normal,
    Heroic,
    Legendary,
}

impl Difficulty {
    pub fn parse(s: &str) -> Result<Difficulty, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "easy" => Difficulty::Easy,
            "normal" => Difficulty::Normal,
            "heroic" => Difficulty::Heroic,
            "legendary" => Difficulty::Legendary,
            _ => {
                return Err(format!(
                    "firefight.difficulty = {s:?}: easy, normal, heroic or legendary"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Normal => "normal",
            Difficulty::Heroic => "heroic",
            Difficulty::Legendary => "legendary",
        }
    }

    /// The `firefight.difficulty` choice.
    pub fn index(self) -> i64 {
        match self {
            Difficulty::Easy => 0,
            Difficulty::Normal => 1,
            Difficulty::Heroic => 2,
            Difficulty::Legendary => 3,
        }
    }
}

/// The starting lives the game offers (`firefight.lives`'s choices other
/// than the set's own).
pub const LIVES: [u16; 7] = [1, 3, 5, 8, 12, 20, 30];

/// A match file's `[firefight]` table. Every key is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirefightConfig {
    /// `holdout` (the default), `attrition`, `last_stand` or `crossfire`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    /// `easy`, `normal` (the default), `heroic` or `legendary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<String>,
    /// Player teams, 1 or 2; unset: the set's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams: Option<u8>,
    /// Starting lives, one of [`LIVES`]; unset: the set's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lives: Option<u16>,
    /// Whether each round switches on its skull; unset: on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skulls: Option<bool>,
}

/// A Firefight match, checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Firefight {
    pub set: Set,
    pub difficulty: Difficulty,
    /// The player teams the match plays with: the table's, else the set's.
    pub teams: u8,
    /// The table named the teams (else the game uses the set's own).
    pub teams_chosen: bool,
    pub lives: Option<u16>,
    pub skulls: bool,
}

/// A value of one of the game's settings (`firefight.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Bool(bool),
    Int(i64),
}

impl FirefightConfig {
    /// The match it describes, or every problem with it.
    pub fn resolve(&self) -> Result<Firefight, Vec<String>> {
        let mut problems = Vec::new();
        let set = match &self.set {
            Some(s) => Set::parse(s).unwrap_or_else(|e| {
                problems.push(e);
                Set::default()
            }),
            None => Set::default(),
        };
        let difficulty = match &self.difficulty {
            Some(s) => Difficulty::parse(s).unwrap_or_else(|e| {
                problems.push(e);
                Difficulty::default()
            }),
            None => Difficulty::default(),
        };
        if let Some(t) = self.teams
            && !(1..=2).contains(&t)
        {
            problems.push(format!("firefight.teams = {t}: 1 or 2"));
        }
        if let Some(n) = self.lives
            && !LIVES.contains(&n)
        {
            let list: Vec<String> = LIVES.iter().map(u16::to_string).collect();
            problems.push(format!("firefight.lives = {n}: one of {}", list.join(", ")));
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Firefight {
            set,
            difficulty,
            teams: self.teams.unwrap_or(set.own_teams()),
            teams_chosen: self.teams.is_some(),
            lives: self.lives,
            skulls: self.skulls.unwrap_or(true),
        })
    }
}

impl Firefight {
    /// The problem with a bot on `team`, if it is not a player team.
    pub fn seat_problem(&self, team: u8) -> Option<String> {
        if team < self.teams {
            return None;
        }
        Some(match self.teams {
            1 => format!("team {team}: Firefight seats are on the player team 0"),
            n => format!("team {team}: Firefight seats are on the player teams 0..{}", n - 1),
        })
    }

    /// Seats the bots whose team is `None` on the player team with the
    /// fewest bots so far (the lower team on a tie), in seat order, after
    /// the named ones. A team outside the player teams is left as it is
    /// ([`Firefight::seat_problem`] refuses it).
    pub fn balance(&self, teams: &mut [Option<u8>]) {
        let n = usize::from(self.teams.max(1));
        let mut count = vec![0usize; n];
        for t in teams.iter().flatten() {
            if let Some(c) = count.get_mut(usize::from(*t)) {
                *c += 1;
            }
        }
        for t in teams.iter_mut().filter(|t| t.is_none()) {
            let (best, _) = count
                .iter()
                .enumerate()
                .min_by_key(|&(i, c)| (*c, i))
                .unwrap_or((0, &0));
            count[best] += 1;
            *t = Some(best as u8);
        }
    }

    /// The game's settings for the match (`firefight.*`, read by the
    /// runtime's `Options::read` when the game starts). `firefight.lives`
    /// 0 and `firefight.teams` 0 are the set's own.
    pub fn settings(&self) -> Vec<(&'static str, Choice)> {
        vec![
            ("firefight.enabled", Choice::Bool(true)),
            ("firefight.waveset", Choice::Int(self.set.index())),
            ("firefight.difficulty", Choice::Int(self.difficulty.index())),
            ("firefight.lives", Choice::Int(self.lives.map_or(0, i64::from))),
            ("firefight.skulls", Choice::Bool(self.skulls)),
            (
                "firefight.teams",
                Choice::Int(if self.teams_chosen { i64::from(self.teams) } else { 0 }),
            ),
        ]
    }
}

/// The report's `firefight` section: how far the match got and the score
/// (docs/runtime/sim.md, "Firefight").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirefightReport {
    /// The wave set's name.
    pub set: String,
    pub teams: u8,
    /// The match ended with the director's result (else a limit stopped it).
    pub ended: bool,
    /// `won`, `lost`, `team_won` or `draw` (`live_proto::firefight`), once
    /// ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// The winning team, on `team_won`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
    pub rounds_cleared: u32,
    /// Each player team's points, by team.
    pub scores: Vec<u32>,
    pub lives_used: u32,
}

impl FirefightReport {
    /// The section for a match the director ended with `ended`.
    pub fn ended(ended: &live_proto::firefight::FirefightEnded) -> Self {
        FirefightReport {
            set: ended.set.clone(),
            teams: ended.teams,
            ended: true,
            outcome: Some(ended.outcome.clone()),
            team: ended.team,
            rounds_cleared: ended.rounds_cleared,
            scores: ended.scores.clone(),
            lives_used: ended.lives_used,
        }
    }

    /// The section for a match a limit stopped first: the player teams'
    /// scores as the game last showed them (the report's
    /// `result.teams`), no result.
    pub fn unfinished(m: &Firefight, teams: &[crate::metrics::TeamResult]) -> Self {
        let scores = (0..m.teams)
            .map(|t| {
                teams
                    .iter()
                    .find(|r| r.team == i32::from(t))
                    .map_or(0, |r| r.score.clamp(0, i64::from(u32::MAX)) as u32)
            })
            .collect();
        FirefightReport {
            set: m.set.name().into(),
            teams: m.teams,
            scores,
            ..Default::default()
        }
    }
}
