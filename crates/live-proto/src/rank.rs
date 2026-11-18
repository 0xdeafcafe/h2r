//! Per-playlist rank: the level 1..50 and experience a player has in a
//! ranked playlist, and the rules that move them after a game.
//!
//! The mechanism follows Halo 2's, as traced in the retail game
//! (docs/live/live-matchmaking.md §8); the numbers are our own, chosen for
//! this server, not the game's tuning tables:
//!
//! - Experience is a points total per playlist. After a ranked game each
//!   player earns or loses points against every opponent (players on other
//!   teams; everyone else in free-for-all): a beaten opponent is worth
//!   more the higher their level is above yours, a loss costs more the
//!   lower theirs is. Ties are worth nothing. The change is the mean over
//!   opponents, clamped to ±[`MAX_DELTA`].
//! - New players lose little: what a loss costs scales up with level, and
//!   what a win earns scales down near the top.
//! - The level is where the experience sits in a table of thresholds, with
//!   hysteresis: a player only drops a level once their experience falls
//!   below the midpoint of the level below.
//!
//! Every player starts each ranked playlist at level 1 with no experience.
//! Bots rank through exactly the same rules (docs/game/bots.md §9). The
//! server applies them once per finished ranked game and records each
//! change as a `RankChanged` event on the player's `rank-<xuid>` stream.
use crate::Xuid;
use serde::{Deserialize, Serialize};

pub const CHANGED: &str = "RankChanged";
pub const MIN_LEVEL: u8 = 1;
pub const MAX_LEVEL: u8 = 50;
/// The most one game can move experience, either way.
pub const MAX_DELTA: i64 = 200;
/// Experience never goes above this (the game keeps it in 30 bits).
pub const MAX_EXPERIENCE: u32 = 0x3fff_ffff;

pub fn stream(xuid: Xuid) -> String {
    format!("rank-{xuid}")
}

/// One player's standing in one playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Standing {
    /// The level shown: 1..50.
    pub level: u8,
    /// The points the level is read from.
    pub experience: u32,
    /// The highest level ever reached in the playlist.
    pub highest: u8,
    pub games: u32,
    pub wins: u32,
}

impl Default for Standing {
    fn default() -> Self {
        Standing {
            level: MIN_LEVEL,
            experience: 0,
            highest: MIN_LEVEL,
            games: 0,
            wins: 0,
        }
    }
}

/// A player's part in a finished ranked game.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entrant {
    pub xuid: Xuid,
    /// The team in a team game; players on one team are not opponents.
    pub team: Option<i32>,
    /// 0 for the winning team or player, then down the standings; equal
    /// places tie.
    pub place: u32,
    /// Left before the end: loses to everyone, and never wins.
    pub quit: bool,
}

/// `RankChanged` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changed {
    pub xuid: Xuid,
    /// The playlist (hopper descriptor index, as games and sessions name it).
    pub playlist: String,
    pub game_id: String,
    pub before: Standing,
    pub after: Standing,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bot: bool,
    /// The arbitration round that ranked the game: `{session_id}-{round_id}`
    /// (docs/live/live-matchmaking.md §8.4). A round ranks one game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<String>,
}

/// Experience needed for `level` (1..=50): level 1 needs none.
pub fn threshold(level: u8) -> u32 {
    let k = u32::from(level.clamp(MIN_LEVEL, MAX_LEVEL) - 1);
    25 * k * k + 75 * k
}

/// A bot's skill (0 to 1) for a level (docs/game/bots.md §8.2 and §9.4):
/// `(level - 1) / 49`, continuous, 0 at level 1 and 1 at level 50. The
/// roster's `skill` when bots are matched to the humans' rank.
pub fn bot_skill(level: u8) -> f32 {
    f32::from(level.clamp(MIN_LEVEL, MAX_LEVEL) - MIN_LEVEL) / f32::from(MAX_LEVEL - MIN_LEVEL)
}

/// The level `experience` reaches, before hysteresis.
pub fn level_for(experience: u32) -> u8 {
    (MIN_LEVEL..=MAX_LEVEL)
        .rev()
        .find(|l| threshold(*l) <= experience)
        .unwrap_or(MIN_LEVEL)
}

/// The level after a change, from the level before: up as far as the
/// experience reaches, but down only once it is below the midpoint of the
/// level below.
pub fn next_level(before: u8, experience: u32) -> u8 {
    let reached = level_for(experience);
    if reached >= before {
        return reached;
    }
    let mut level = before;
    while level > MIN_LEVEL {
        let mid = (threshold(level - 1) + threshold(level)) / 2;
        if experience < mid {
            level -= 1;
        } else {
            break;
        }
    }
    level
}

/// Points for beating an opponent `diff` levels above you (negative:
/// below), before scaling by your level.
fn win_points(diff: i32) -> f64 {
    (100.0 * 1.12f64.powi(diff)).clamp(20.0, 180.0)
}

/// What a win is worth at `level`, as a fraction: all of it until 40, then
/// down to half at 50.
fn win_scale(level: u8) -> f64 {
    if level <= 40 {
        1.0
    } else {
        1.0 - 0.05 * f64::from(level - 40)
    }
}

/// What a loss costs at `level`, as a fraction: nothing at level 1, all of
/// it from level 11.
fn loss_scale(level: u8) -> f64 {
    (0.1 * f64::from(level.max(MIN_LEVEL) - 1)).min(1.0)
}

/// The new standings of every entrant, in the same order. A game with
/// fewer than two sides changes nothing.
pub fn apply(game: &[(Entrant, Standing)]) -> Vec<Standing> {
    let worst = game.iter().map(|(e, _)| e.place).max().unwrap_or(0) + 1;
    let place = |e: &Entrant| if e.quit { worst } else { e.place };
    let mut sides: Vec<_> = game.iter().map(|(e, _)| e.team.ok_or(e.xuid.0)).collect();
    sides.sort();
    sides.dedup();
    game.iter()
        .map(|(me, s)| {
            if sides.len() < 2 {
                return *s;
            }
            let mut sum = 0.0;
            let mut n = 0u32;
            for (them, t) in game {
                if them.xuid == me.xuid || (me.team.is_some() && me.team == them.team) {
                    continue;
                }
                n += 1;
                let diff = i32::from(t.level) - i32::from(s.level);
                sum += match place(me).cmp(&place(them)) {
                    std::cmp::Ordering::Less => win_points(diff) * win_scale(s.level),
                    std::cmp::Ordering::Equal => 0.0,
                    std::cmp::Ordering::Greater => -win_points(-diff) * loss_scale(s.level),
                };
            }
            if n == 0 {
                return *s;
            }
            let delta = ((sum / f64::from(n)).round() as i64).clamp(-MAX_DELTA, MAX_DELTA);
            let experience =
                (i64::from(s.experience) + delta).clamp(0, i64::from(MAX_EXPERIENCE)) as u32;
            let level = next_level(s.level, experience);
            let won = !me.quit && me.place == 0;
            Standing {
                level,
                experience,
                highest: s.highest.max(level),
                games: s.games + 1,
                wins: s.wins + u32::from(won),
            }
        })
        .collect()
}

/// The stats the game reads per playlist leaderboard (ids 1..=6,
/// docs/live/live-matchmaking.md §8.1).
pub const LIVE_STATS: std::ops::RangeInclusive<u16> = 1..=6;
/// Stat 1: experience.
pub const STAT_EXPERIENCE: u16 = 1;
/// Stat 2: the level, stored from 0.
pub const STAT_LEVEL: u16 = 2;
/// Stat 3: the highest level reached, stored from 0.
pub const STAT_HIGHEST: u16 = 3;

/// The value of stat `id` on a playlist's leaderboard for a standing, as
/// the game reads it: levels from 0, the counters it keeps in 4..=6 (not
/// decoded) as 0. `None` for ids the game never reads.
pub fn live_stat(s: &Standing, id: u16) -> Option<u32> {
    match id {
        STAT_EXPERIENCE => Some(s.experience.min(MAX_EXPERIENCE)),
        STAT_LEVEL => Some(u32::from(s.level.clamp(MIN_LEVEL, MAX_LEVEL) - 1)),
        STAT_HIGHEST => Some(u32::from(s.highest.clamp(MIN_LEVEL, MAX_LEVEL) - 1)),
        4..=6 => Some(0),
        _ => None,
    }
}

/// One row of `GET /v1/rank-stats`: a player's standing on the playlist
/// whose leaderboard (the descriptor's playlist id) the game asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderboardRow {
    pub xuid: Xuid,
    pub leaderboard: u32,
    /// The playlist (hopper descriptor index).
    pub playlist: String,
    pub standing: Standing,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bot_skill_runs_from_level_1_to_50() {
        assert_eq!(bot_skill(1), 0.0);
        assert_eq!(bot_skill(50), 1.0);
        assert_eq!(bot_skill(0), 0.0);
        assert_eq!(bot_skill(99), 1.0);
        assert!((bot_skill(25) - 24.0 / 49.0).abs() < 1e-6);
    }
    #[test]
    fn live_stats_store_levels_from_zero() {
        let s = Standing {
            level: 12,
            experience: 3_450,
            highest: 14,
            games: 9,
            wins: 5,
        };
        assert_eq!(live_stat(&s, STAT_EXPERIENCE), Some(3_450));
        assert_eq!(live_stat(&s, STAT_LEVEL), Some(11));
        assert_eq!(live_stat(&s, STAT_HIGHEST), Some(13));
        assert_eq!(live_stat(&s, 5), Some(0));
        assert_eq!(live_stat(&s, 7), None);
        assert_eq!(live_stat(&Standing::default(), STAT_LEVEL), Some(0));
    }
    fn e(x: u64, team: Option<i32>, place: u32) -> Entrant {
        Entrant {
            xuid: Xuid(x),
            team,
            place,
            quit: false,
        }
    }
    fn at(level: u8) -> Standing {
        Standing {
            level,
            highest: level,
            experience: threshold(level),
            ..Standing::default()
        }
    }

    #[test]
    fn thresholds_rise_and_cover_every_level() {
        assert_eq!(threshold(1), 0);
        assert_eq!(threshold(2), 100);
        for l in 2..=MAX_LEVEL {
            assert!(threshold(l) > threshold(l - 1));
            assert_eq!(level_for(threshold(l)), l);
            assert_eq!(level_for(threshold(l) - 1), l - 1);
        }
        assert_eq!(level_for(MAX_EXPERIENCE), MAX_LEVEL);
    }

    #[test]
    fn levels_drop_only_below_the_midpoint() {
        let mid = (threshold(9) + threshold(10)) / 2;
        assert_eq!(next_level(10, threshold(10) - 1), 10);
        assert_eq!(next_level(10, mid), 10);
        assert_eq!(next_level(10, mid - 1), 9);
        assert_eq!(next_level(10, 0), 1);
        assert_eq!(next_level(3, threshold(6)), 6);
    }

    #[test]
    fn a_win_earns_and_an_upset_earns_more() {
        let even = apply(&[(e(1, None, 0), at(10)), (e(2, None, 1), at(10))]);
        assert_eq!(even[0].experience, threshold(10) + 100);
        assert_eq!(
            even[1].experience,
            threshold(10) - 90,
            "a loss at level 10 costs 90%"
        );
        assert_eq!((even[0].games, even[0].wins), (1, 1));
        assert_eq!((even[1].games, even[1].wins), (1, 0));
        let upset = apply(&[(e(1, None, 0), at(5)), (e(2, None, 1), at(15))]);
        assert!(upset[0].experience - threshold(5) > 125);
        let expected = apply(&[(e(1, None, 0), at(15)), (e(2, None, 1), at(5))]);
        assert!(expected[0].experience - threshold(15) < 50);
    }

    #[test]
    fn new_players_lose_nothing_and_climb() {
        let fresh = Standing::default();
        let out = apply(&[(e(1, None, 1), fresh), (e(2, None, 0), fresh)]);
        assert_eq!(out[0].experience, 0);
        assert_eq!(out[0].level, 1);
        assert_eq!(
            (out[1].experience, out[1].level, out[1].highest),
            (100, 2, 2)
        );
    }

    #[test]
    fn teams_ties_quitters_and_one_sided_games() {
        let s = at(20);
        let teams = apply(&[
            (e(1, Some(0), 0), s),
            (e(2, Some(0), 0), s),
            (e(3, Some(1), 1), s),
        ]);
        assert_eq!(teams[0].experience, threshold(20) + 100);
        assert_eq!(teams[2].experience, threshold(20) - 100);
        let tie = apply(&[(e(1, None, 0), s), (e(2, None, 0), s)]);
        assert_eq!(tie[0].experience, s.experience);
        assert_eq!(tie[0].wins, 1);
        let mut quit = e(1, None, 0);
        quit.quit = true;
        let out = apply(&[(quit, s), (e(2, None, 1), s)]);
        assert!(out[0].experience < s.experience && out[0].wins == 0);
        let out = apply(&[(e(1, Some(0), 0), s), (e(2, Some(0), 0), s)]);
        assert_eq!(out, vec![s, s]);
    }

    #[test]
    fn one_game_moves_at_most_the_cap() {
        let out = apply(&[(e(1, None, 0), at(1)), (e(2, None, 1), at(50))]);
        assert_eq!(out[0].experience, 180);
        let mut top = at(50);
        top.experience = threshold(50) + 10;
        let out = apply(&[(e(1, None, 1), top), (e(2, None, 0), at(1))]);
        assert_eq!(out[0].experience, threshold(50) + 10 - 180);
        assert_eq!(out[0].level, 50);
    }

    #[test]
    fn changes_serialize_with_the_bot_flag_only_when_set() {
        let c = Changed {
            xuid: Xuid(9),
            playlist: "1".into(),
            game_id: "g".into(),
            before: Standing::default(),
            after: Standing::default(),
            bot: false,
            round: None,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("bot").is_none());
        assert!(v.get("round").is_none());
        assert_eq!(serde_json::from_value::<Changed>(v).unwrap(), c);
    }
}
