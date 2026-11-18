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
    Xuid,
    game::Winner,
    hopper::{HALO_2_HOPPER_FILE, HopperFile},
    rank::{self, Changed, Entrant, Standing},
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
}

impl Ranks {
    pub fn standing(&self, xuid: Xuid, playlist: &str) -> Standing {
        self.standings
            .get(playlist)
            .and_then(|p| p.get(&xuid))
            .copied()
            .unwrap_or_default()
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
        let Ok((_, bytes)) = self
            .file(live_proto::HALO_2_TITLE_ID, HALO_2_HOPPER_FILE)
            .await
        else {
            return false;
        };
        HopperFile::parse_legacy(&bytes)
            .ok()
            .and_then(|f| f.descriptor(index).ok())
            .and_then(|d| d.kind())
            .is_some_and(|k| k.ranked())
    }

    /// Ranks a finished ranked matchmaking game once: one `RankChanged` per
    /// player with an account. Anything else (custom, campaign, unranked,
    /// lost contact, already ranked) writes nothing.
    pub(crate) async fn rank_game(&self, game_id: &str) -> Result<(), ApiError> {
        let _guard = self.rank_writes.lock().await;
        let (playlist, entrants) = {
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
            (playlist, entrants(g))
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
