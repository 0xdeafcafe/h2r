//! Per-playlist rank (docs/live/live-matchmaking.md §8): after a ranked
//! matchmaking game finishes, the server applies [`live_proto::rank`] to its
//! players and writes one server-signed `RankChanged` per player on
//! `rank-<xuid>`. The read model keeps every player's standing per playlist,
//! a short history, and which games were ranked, so a replay of the log
//! rebuilds the same ranks without re-running the rules.
//!
//! No background work: ranking runs in the request that ended the game.
use crate::{
    app::{ApiError, Live, invalid, not_found},
    model::{Game, State},
    store::RecordedEvent,
};
use axum::{
    extract::{Path, Query, State as HttpState},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use live_proto::{
    Author, Xuid,
    game::Winner,
    hopper::{HALO_2_HOPPER_FILE, HOPPERS, HopperFile},
    rank::{self, Changed, Entrant, LeaderboardRow, Standing},
};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

/// History kept per player and playlist.
pub const HISTORY: usize = 100;

/// One history row: a game's change to a standing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryEntry {
    pub game_id: String,
    pub at: String,
    pub position: u64,
    pub before: Standing,
    pub after: Standing,
}

/// The projection of the `rank-*` streams.
#[derive(Debug, Default)]
pub struct Ranks {
    /// Playlist -> player -> standing.
    pub standings: BTreeMap<String, HashMap<Xuid, Standing>>,
    pub history: HashMap<(Xuid, String), VecDeque<HistoryEntry>>,
    /// Games already ranked (a second end never ranks twice).
    pub games: BTreeSet<String>,
    /// Arbitration rounds that have ranked a game (`{session}-{round}`):
    /// a round ranks one game.
    pub rounds: BTreeSet<String>,
}

impl Ranks {
    pub fn standing(&self, xuid: Xuid, playlist: &str) -> Standing {
        self.standings
            .get(playlist)
            .and_then(|p| p.get(&xuid))
            .copied()
            .unwrap_or_default()
    }

    /// Up to `n` of `bots` (persistent bot accounts) for a match at
    /// `level` on `playlist` (docs/live/live-matchmaking.md §8.4, "Matched
    /// to yours"): closest level first, then fewest games, then xuid. A bot
    /// with no games there stands at level 1. The roster endpoint fills any
    /// gap with made-up bots.
    pub fn nearest(
        &self,
        playlist: &str,
        level: u8,
        n: usize,
        bots: impl IntoIterator<Item = Xuid>,
    ) -> Vec<Xuid> {
        let mut scored: Vec<(u8, u32, Xuid)> = bots
            .into_iter()
            .map(|x| {
                let s = self.standing(x, playlist);
                (s.level.abs_diff(level), s.games, x)
            })
            .collect();
        scored.sort_unstable();
        scored.dedup_by_key(|(_, _, x)| *x);
        scored.into_iter().take(n).map(|(_, _, x)| x).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_bots_are_the_closest_level_then_the_least_played() {
        let mut r = Ranks::default();
        let at = |level, games| Standing {
            level,
            games,
            ..Standing::default()
        };
        let p = r.standings.entry("1".into()).or_default();
        p.insert(Xuid(1), at(10, 5));
        p.insert(Xuid(2), at(12, 1));
        p.insert(Xuid(3), at(12, 9));
        p.insert(Xuid(4), at(30, 2));
        // Xuid(5) has never played this playlist: level 1.
        let bots = [Xuid(1), Xuid(2), Xuid(3), Xuid(4), Xuid(5)];
        assert_eq!(r.nearest("1", 12, 3, bots), [Xuid(2), Xuid(3), Xuid(1)]);
        assert_eq!(r.nearest("1", 1, 1, bots), [Xuid(5)]);
        assert_eq!(r.nearest("1", 50, 9, bots).len(), 5);
        assert_eq!(r.nearest("2", 1, 2, bots), [Xuid(1), Xuid(2)]);
    }
}

pub(crate) fn project(s: &mut State, e: &RecordedEvent) {
    let b = &e.body;
    if b.kind != rank::CHANGED {
        return;
    }
    let Ok(c) = serde_json::from_value::<Changed>(b.data.clone()) else {
        eprintln!("live: skipping {} at {}", b.kind, e.position);
        return;
    };
    let r = &mut s.ranks;
    r.games.insert(c.game_id.clone());
    if let Some(round) = &c.round {
        r.rounds.insert(round.clone());
    }
    r.standings
        .entry(c.playlist.clone())
        .or_default()
        .insert(c.xuid, c.after);
    let h = r.history.entry((c.xuid, c.playlist)).or_default();
    if h.len() == HISTORY {
        h.pop_front();
    }
    h.push_back(HistoryEntry {
        game_id: c.game_id,
        at: b.at.clone(),
        position: e.position,
        before: c.before,
        after: c.after,
    });
}

/// Who played a finished game and where they placed, for the rank rules:
/// players with an account only. Team games place teams by the winner,
/// then by team score; free-for-all places players by the winner, then by
/// score. Players who left before the end are quitters.
pub fn entrants(g: &Game) -> Vec<Entrant> {
    let Some((ended, _)) = &g.ended else {
        return Vec::new();
    };
    let team_play = g.started.game.team_play;
    let score_of = |index: u32| {
        g.scoreboard
            .iter()
            .find(|p| p.index == index)
            .map_or(0, |p| p.score)
    };
    let team_score = |team: i32| {
        g.teams
            .as_ref()
            .and_then(|t| t.iter().find(|s| s.team == team))
            .map_or(0, |s| s.score)
    };
    // Higher key places better.
    let key = |team: i32, index: u32| -> (bool, i64) {
        let won = match &ended.winner {
            Some(Winner::Team { team: t }) => team_play && *t == team,
            Some(Winner::Player { player }) => !team_play && *player == index,
            None => false,
        };
        (
            won,
            if team_play {
                team_score(team)
            } else {
                score_of(index)
            },
        )
    };
    let mut keys: Vec<(bool, i64)> = g.players.iter().map(|p| key(p.team, p.index)).collect();
    keys.sort();
    keys.dedup();
    keys.reverse();
    let mut out: Vec<Entrant> = g
        .players
        .iter()
        .filter_map(|p| {
            let xuid = p.xuid?;
            let k = key(p.team, p.index);
            Some(Entrant {
                xuid,
                team: team_play.then_some(p.team),
                place: keys.iter().position(|v| *v == k).unwrap_or(0) as u32,
                quit: false,
            })
        })
        .collect();
    for x in &g.xuids {
        if !out.iter().any(|e| e.xuid == *x) {
            out.push(Entrant {
                xuid: *x,
                team: None,
                place: 0,
                quit: true,
            });
        }
    }
    out
}

impl Live {
    /// Whether `playlist` (a hopper descriptor index) is ranked in the
    /// hopper file the server serves.
    pub async fn ranked_playlist(&self, playlist: &str) -> bool {
        let Ok(index) = playlist.parse::<usize>() else {
            return false;
        };
        self.hoppers()
            .await
            .and_then(|f| f.descriptor(index).ok())
            .and_then(|d| d.kind())
            .is_some_and(|k| k.ranked())
    }

    /// The served hopper's title for a game record's `playlist_id` (a
    /// descriptor index), as `NewGame.playlist` takes it: at most 32
    /// characters, none of them control characters. `None` when the id
    /// names no hopper or the hopper has no title.
    pub(crate) async fn playlist_name(&self, playlist_id: Option<&str>) -> Option<String> {
        let index = playlist_id?
            .parse::<usize>()
            .ok()
            .filter(|i| *i < HOPPERS)?;
        let title = self.hoppers().await?.text(index).ok()?.title;
        let name: String = title.chars().filter(|c| !c.is_control()).take(32).collect();
        let name = name.trim().to_owned();
        (!name.is_empty()).then_some(name)
    }

    async fn hoppers(&self) -> Option<HopperFile> {
        let (_, bytes) = self
            .file(live_proto::HALO_2_TITLE_ID, HALO_2_HOPPER_FILE)
            .await
            .ok()?;
        HopperFile::parse_legacy(&bytes).ok()
    }

    /// Ranked playlists by leaderboard: each ranked descriptor's playlist
    /// id (what the game's stats read names) to its index.
    async fn ranked_leaderboards(&self) -> BTreeMap<u32, String> {
        let Some(f) = self.hoppers().await else {
            return BTreeMap::new();
        };
        (0..HOPPERS)
            .filter_map(|i| {
                let d = f.descriptor(i).ok()?;
                d.kind()
                    .is_some_and(|k| k.ranked())
                    .then(|| (d.playlist_id, i.to_string()))
            })
            .collect()
    }

    /// Ranks a finished ranked matchmaking game once: one `RankChanged` per
    /// player with an account who registered the game's arbitration round
    /// (its session's latest round the host registered that has not ranked
    /// a game). Anything else (custom, campaign, unranked, lost contact, no
    /// session or round, already ranked) writes nothing.
    pub(crate) async fn rank_game(&self, game_id: &str) -> Result<(), ApiError> {
        let _guard = self.rank_writes.lock().await;
        let (playlist, entrants, round) = {
            let s = self.model.read();
            if s.ranks.games.contains(game_id) {
                return Ok(());
            }
            let Some(g) = s.games.get(game_id) else {
                return Ok(());
            };
            let started = &g.started.game;
            let finished = g
                .ended
                .as_ref()
                .is_some_and(|(e, _)| e.reason == "finished");
            let Some(playlist) = started.playlist_id.clone() else {
                return Ok(());
            };
            if !finished || started.kind() != "matchmaking" {
                return Ok(());
            }
            // Arbitration (docs/live/live-matchmaking.md §8.4): the host's
            // latest unused round in the game's session; only the humans
            // whose own account registered it are ranked.
            let Some(session) = started.session_id.as_deref() else {
                return Ok(());
            };
            let Some(Author::Account { xuid: host, .. }) = Author::parse(&g.started.host) else {
                return Ok(());
            };
            let Some((round, r)) = s.arbitration.for_game(session, host, &s.ranks.rounds) else {
                return Ok(());
            };
            let entrants: Vec<Entrant> = entrants(g)
                .into_iter()
                .filter(|e| r.registered.contains_key(&e.xuid))
                .collect();
            (playlist, entrants, format!("{session}-{round}"))
        };
        if entrants.len() < 2 || !self.ranked_playlist(&playlist).await {
            return Ok(());
        }
        let game: Vec<(Entrant, Standing)> = {
            let s = self.model.read();
            entrants
                .iter()
                .map(|e| (*e, s.ranks.standing(e.xuid, &playlist)))
                .collect()
        };
        let after = rank::apply(&game);
        let events = game
            .iter()
            .zip(after)
            .filter(|((_, before), after)| before != after)
            .map(|((e, before), after)| {
                let c = Changed {
                    xuid: e.xuid,
                    playlist: playlist.clone(),
                    game_id: game_id.to_string(),
                    before: *before,
                    after,
                    bot: false,
                    round: Some(round.clone()),
                };
                self.server_event(
                    rank::stream(e.xuid),
                    rank::CHANGED,
                    serde_json::to_value(c).expect("serializes"),
                )
            })
            .collect::<Vec<_>>();
        if events.is_empty() {
            return Ok(());
        }
        let recorded = self.store.append(events, &[]).await?;
        let last = recorded.last().map_or(0, |e| e.position);
        self.wait_applied(last).await?;
        Ok(())
    }
}

type Shared = HttpState<Arc<Live>>;

pub(crate) fn xuid_of(s: &State, who: &str) -> Option<Xuid> {
    Xuid::parse(who)
        .ok()
        .filter(|x| s.accounts.contains_key(x))
        .or_else(|| {
            live_proto::Gamertag::new(who)
                .ok()
                .and_then(|t| s.account_by_tag(&t).map(|a| a.xuid))
        })
}

/// `GET /v1/accounts/{who}/ranks`: every playlist the player has a rank in.
pub(crate) async fn player_ranks(
    HttpState(live): Shared,
    headers: HeaderMap,
    Path(who): Path<String>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let s = live.model.read();
    let xuid = xuid_of(&s, &who).ok_or_else(|| not_found("no such account"))?;
    let ranks: BTreeMap<&String, Standing> = s
        .ranks
        .standings
        .iter()
        .filter_map(|(p, m)| m.get(&xuid).map(|st| (p, *st)))
        .collect();
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"xuid": xuid, "ranks": ranks, "position": live.position()}),
    ))
}

/// `GET /v1/accounts/{who}/ranks/{playlist}`: the standing and its history,
/// newest first.
pub(crate) async fn player_history(
    HttpState(live): Shared,
    headers: HeaderMap,
    Path((who, playlist)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let s = live.model.read();
    let xuid = xuid_of(&s, &who).ok_or_else(|| not_found("no such account"))?;
    let history: Vec<&HistoryEntry> = s
        .ranks
        .history
        .get(&(xuid, playlist.clone()))
        .map(|h| h.iter().rev().collect())
        .unwrap_or_default();
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({
            "xuid": xuid,
            "playlist": playlist,
            "standing": s.ranks.standing(xuid, &playlist),
            "history": history,
            "position": live.position(),
        }),
    ))
}

#[derive(Serialize)]
struct Row {
    xuid: Xuid,
    gamertag: String,
    #[serde(flatten)]
    standing: Standing,
}

/// `GET /v1/ranks/{playlist}?limit=`: the playlist's leaderboard: level,
/// then experience, best first.
pub(crate) async fn leaderboard(
    HttpState(live): Shared,
    headers: HeaderMap,
    Path(playlist): Path<String>,
    Query(q): Query<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let limit = q
        .get("limit")
        .map(|v| v.parse::<usize>())
        .transpose()
        .map_err(|_| invalid("invalid limit"))?
        .unwrap_or(100);
    if !(1..=1000).contains(&limit) {
        return Err(invalid("leaderboard limit is 1..1000"));
    }
    let s = live.model.read();
    let mut rows: Vec<Row> = s
        .ranks
        .standings
        .get(&playlist)
        .into_iter()
        .flatten()
        .map(|(x, st)| Row {
            xuid: *x,
            gamertag: s
                .accounts
                .get(x)
                .map(|a| a.gamertag.to_string())
                .unwrap_or_default(),
            standing: *st,
        })
        .collect();
    rows.sort_by(|a, b| {
        (b.standing.level, b.standing.experience)
            .cmp(&(a.standing.level, a.standing.experience))
            .then(a.xuid.cmp(&b.xuid))
    });
    rows.truncate(limit);
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"playlist": playlist, "items": rows, "position": live.position()}),
    ))
}

/// Most players and leaderboards in one stats read (the game reads at most
/// four players on sixteen hoppers, plus its recent-players board).
const MAX_STATS_XUIDS: usize = 64;
const MAX_STATS_BOARDS: usize = 32;

fn list<T: std::str::FromStr>(q: &BTreeMap<String, String>, key: &str) -> Result<Vec<T>, ApiError> {
    q.get(key)
        .map(String::as_str)
        .unwrap_or("")
        .split(',')
        .filter(|v| !v.is_empty())
        .map(|v| v.parse().map_err(|_| invalid(format!("invalid {key}"))))
        .collect()
}

/// `GET /v1/rank-stats?leaderboards=&xuids=`: the game's stats read
/// (docs/live/live-matchmaking.md §8.1). Each leaderboard is a playlist id;
/// one row per player for every leaderboard that is a ranked playlist in
/// the served hopper file, a player with no games at level 1. Other
/// leaderboards (the recent-players board, unranked playlists) have no
/// rows.
pub(crate) async fn stats(
    HttpState(live): Shared,
    headers: HeaderMap,
    Query(q): Query<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let mut boards: Vec<u32> = list(&q, "leaderboards")?;
    let mut xuids: Vec<Xuid> = list::<String>(&q, "xuids")?
        .iter()
        .map(|x| Xuid::parse(x).map_err(|_| invalid("invalid xuids")))
        .collect::<Result<_, _>>()?;
    boards.sort_unstable();
    boards.dedup();
    xuids.sort_unstable();
    xuids.dedup();
    if boards.len() > MAX_STATS_BOARDS || xuids.len() > MAX_STATS_XUIDS {
        return Err(invalid("too many leaderboards or xuids"));
    }
    let ranked = live.ranked_leaderboards().await;
    let s = live.model.read();
    let r = &s.ranks;
    let items: Vec<LeaderboardRow> = boards
        .iter()
        .filter_map(|b| Some((*b, ranked.get(b)?)))
        .flat_map(|(b, playlist)| {
            xuids.iter().map(move |x| LeaderboardRow {
                xuid: *x,
                leaderboard: b,
                playlist: playlist.clone(),
                standing: r.standing(*x, playlist),
            })
        })
        .collect();
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"items": items, "position": live.position()}),
    ))
}
