//! Quick Options on the pause menu's Game Setup: a few of the variant's
//! settings, changed for the next game (docs/game/settings-menu.md "Game
//! Setup from the pause menu").
//!
//! **Why at the restart, never live.** A game plays the copy of the
//! variant made at its start (`G+0x13c`, `G = [0x4e6948]`;
//! docs/game/game-options.md "The record and its limits"). The host sends
//! the record to the other consoles only with the game's parameters
//! (messages `0x21` and `0x22`, "How a variant reaches other machines"),
//! and which code reads each setting from the copy during a game (the
//! score and time limits, the respawn time) is not traced. A change
//! written there would reach this console only, or none, so no setting
//! applies live ([`applies`]): every change waits in the visit's draft and
//! the next game starts with it, through the lobby's own variant setter.
//!
//! The values are the ones the variant editor offers for the setting on
//! this disc (`community_variants::last_offered`, read from the `sily`
//! tags at the last main menu load), else [`Setting::fallback`]'s, each
//! inside the clean-up's limits; a value the clean-up refuses is never
//! stored ([`apply`]).

use std::collections::HashMap;

use variants::record::{self, GameType, GameVariant, SettingId};

/// When a change on the page takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applies {
    /// At once, in the game being played.
    Live,
    /// When the game restarts with the visit's settings.
    AtRestart,
}

/// One of the page's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    ScoreToWin,
    TimeLimit,
    Lives,
    RespawnTime,
}

impl Setting {
    pub const ALL: [Setting; 4] = [
        Setting::ScoreToWin,
        Setting::TimeLimit,
        Setting::Lives,
        Setting::RespawnTime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Setting::ScoreToWin => "Score to Win",
            Setting::TimeLimit => "Time Limit",
            Setting::Lives => "Lives per Round",
            Setting::RespawnTime => "Respawn Time",
        }
    }

    /// What the value picker says about it.
    pub fn help(self) -> &'static str {
        match self {
            Setting::ScoreToWin => "The score that wins a round of the next game.",
            Setting::TimeLimit => "How long a round of the next game lasts.",
            Setting::Lives => "How many lives each player has in a round of the next game.",
            Setting::RespawnTime => "How long a player waits to respawn in the next game.",
        }
    }

    /// The setting number for a variant of game type `ty`: the score is
    /// each type's own (all stored at `+0x50`).
    pub fn id(self, ty: GameType) -> Option<SettingId> {
        match self {
            Setting::ScoreToWin => ty.score_setting(),
            Setting::TimeLimit => Some(SettingId(10)),
            Setting::Lives => Some(SettingId(16)),
            Setting::RespawnTime => Some(SettingId(17)),
        }
    }

    /// The values offered when the disc's were not read: the steps the
    /// retail editor offers as far as docs/game/game-options.md lists them,
    /// each inside the clean-up's limits. Time in seconds.
    pub fn fallback(self) -> &'static [i32] {
        match self {
            Setting::ScoreToWin => &[0, 5, 10, 15, 20, 25, 50, 100, 150, 200, 250],
            Setting::TimeLimit => &[0, 60, 120, 180, 300, 600, 900, 1200, 1800, 2700, 3600],
            Setting::Lives => &[0, 1, 2, 3, 4, 5, 10, 25, 50, 99],
            Setting::RespawnTime => &[3, 5, 10, 15, 20, 30],
        }
    }

    /// A value as the row and the picker show it.
    pub fn text(self, v: i32) -> String {
        let plural = |n: i32, one: &str| {
            if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {one}s")
            }
        };
        match self {
            Setting::ScoreToWin | Setting::Lives if v == 0 => "Unlimited".into(),
            Setting::ScoreToWin | Setting::Lives => v.to_string(),
            Setting::TimeLimit if v == 0 => "None".into(),
            Setting::TimeLimit if v % 60 == 0 => plural(v / 60, "Minute"),
            Setting::TimeLimit | Setting::RespawnTime => plural(v, "Second"),
        }
    }
}

/// Every setting on the page waits for the restart (module docs).
pub fn applies(_: Setting) -> Applies {
    Applies::AtRestart
}

/// One row: a setting, its values and the one the variant holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickRow {
    pub setting: Setting,
    pub id: SettingId,
    pub values: Vec<i32>,
    pub current: usize,
}

impl QuickRow {
    /// The row as one line: the label, two spaces, the value.
    pub fn line(&self) -> String {
        let v = self.values.get(self.current).copied().unwrap_or_default();
        format!("{}  {}", self.setting.label(), self.setting.text(v))
    }

    /// The values as the picker lists them.
    pub fn texts(&self) -> Vec<String> {
        self.values.iter().map(|v| self.setting.text(*v)).collect()
    }
}

/// The page's rows for variant `v`: each setting its game type has, with
/// the offered values (`offered`, else the fallback), the held one among
/// them.
pub fn rows(v: &GameVariant, offered: Option<&HashMap<u32, Vec<i32>>>) -> Vec<QuickRow> {
    let ty = v.game_type();
    Setting::ALL
        .into_iter()
        .filter_map(|setting| {
            let id = setting.id(ty)?;
            let held = v.get(id)?;
            let mut values = offered
                .and_then(|o| o.get(&id.0))
                .filter(|vs| !vs.is_empty())
                .cloned()
                .unwrap_or_else(|| setting.fallback().to_vec());
            if !values.contains(&held) {
                values.push(held);
            }
            values.sort_unstable();
            values.dedup();
            let current = values.iter().position(|x| *x == held).unwrap_or(0);
            Some(QuickRow {
                setting,
                id,
                values,
                current,
            })
        })
        .collect()
}

/// `v` with setting `id` at `value`; refused by name when the record would
/// not be stored (the clean-up's limits, `record::refused`).
pub fn apply(v: &GameVariant, id: SettingId, value: i32) -> Result<GameVariant, String> {
    let mut out = v.clone();
    out.set(id, value).map_err(|e| e.to_string())?;
    match record::refused(&out).first() {
        Some(p) => Err(p.what.clone()),
        None => Ok(out),
    }
}
