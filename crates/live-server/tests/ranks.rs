//! Per-playlist ranks through the router: a finished ranked matchmaking
//! game writes one `RankChanged` per player; unranked, custom and lost
//! games write none; the standings, history and leaderboard read them back.

mod common;

use axum::http::StatusCode;
use common::*;
use live_proto::rank::{self, Standing};
use serde_json::{Value, json};

/// A two-player matchmaking game on `playlist`, both players with accounts.
fn duel(host: &User, guest: &User, playlist: &str, kind: &str) -> Value {
    json!({
        "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Team Slayer",
        "team_play": true, "tick": 0, "kind": kind,
        "playlist": "Synthetic", "playlist_id": playlist,
        "players": [
            {"index": 0, "name": "Host", "xuid": host.xuid, "team": 0, "local": true},
            {"index": 1, "name": "Guest", "xuid": guest.xuid, "team": 1}
        ]
    })
}

async fn play(h: &Harness, host: &User, game: Value, reason: &str) -> String {
    let id = h.start_game(host, game).await;
    let end = h.batch(
        host,
        &id,
        &[
            (
                "ScoreChanged",
                json!({"tick": 800, "players": [
                    {"index": 0, "score": 5, "kills": 5, "deaths": 1, "assists": 0},
                    {"index": 1, "score": 1, "kills": 1, "deaths": 5, "assists": 0}],
                    "teams": [{"team": 0, "score": 5}, {"team": 1, "score": 1}]}),
            ),
            (
                "GameEnded",
                json!({"tick": 900, "reason": reason, "winner": {"team": 0}}),
            ),
        ],
    );
    let r = h.post_batch(host, &id, end).await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    id
}

fn changes(h: &Value) -> Vec<Value> {
    h["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == rank::CHANGED)
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_finished_ranked_game_ranks_both_players_once() {
    let h = harness().await;
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    // The default playlists: 1 Team Slayer is ranked teams, 7 Unranked Teams.
    let id = play(
        &h,
        &host,
        duel(&host, &guest, "1", "matchmaking"),
        "finished",
    )
    .await;

    let host_rank = h
        .get(&format!("/v1/events?stream=rank-{}", host.xuid))
        .await
        .json();
    let c = changes(&host_rank);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["author"], "server:live.test");
    assert_eq!(c[0]["data"]["game_id"], id.as_str());
    assert_eq!(c[0]["data"]["playlist"], "1");
    let after: Standing = serde_json::from_value(c[0]["data"]["after"].clone()).unwrap();
    assert_eq!((after.level, after.experience, after.wins), (2, 100, 1));
    // A level 1 loser loses nothing, but the game counts.
    let guest_rank = h
        .get(&format!("/v1/events?stream=rank-{}", guest.xuid))
        .await
        .json();
    let after: Standing =
        serde_json::from_value(changes(&guest_rank)[0]["data"]["after"].clone()).unwrap();
    assert_eq!((after.level, after.experience, after.games), (1, 0, 1));

    let ranks = h.get("/v1/accounts/Host/ranks").await.json();
    assert_eq!(ranks["ranks"]["1"]["level"], 2);
    let history = h.get("/v1/accounts/Host/ranks/1").await.json();
    assert_eq!(history["standing"]["experience"], 100);
    assert_eq!(history["history"][0]["game_id"], id.as_str());
    assert_eq!(history["history"][0]["before"]["level"], 1);
    let board = h.get("/v1/ranks/1").await.json();
    assert_eq!(board["items"][0]["gamertag"], "Host");
    assert_eq!(board["items"][1]["gamertag"], "Guest");

    // A second win climbs again.
    play(
        &h,
        &host,
        duel(&host, &guest, "1", "matchmaking"),
        "finished",
    )
    .await;
    let history = h.get("/v1/accounts/Host/ranks/1").await.json();
    // Beating a lower level is worth less: 89 points from level 2.
    assert_eq!(history["standing"]["experience"], 189);
    assert_eq!(history["standing"]["games"], 2);
    assert_eq!(history["history"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unranked_custom_and_lost_games_rank_nobody() {
    let h = harness().await;
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    play(
        &h,
        &host,
        duel(&host, &guest, "7", "matchmaking"),
        "finished",
    )
    .await;
    play(&h, &host, duel(&host, &guest, "1", "matchmaking"), "quit").await;
    let mut custom = duel(&host, &guest, "1", "custom");
    custom.as_object_mut().unwrap().remove("playlist");
    custom.as_object_mut().unwrap().remove("playlist_id");
    play(&h, &host, custom, "finished").await;
    let ranks = h.get("/v1/accounts/Host/ranks").await.json();
    assert_eq!(ranks["ranks"], json!({}));
    assert_eq!(h.get("/v1/ranks/1").await.json()["items"], json!([]));
    assert_eq!(
        h.get("/v1/accounts/Nobody/ranks").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.get("/v1/ranks/1?limit=0").await.status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn players_cannot_write_their_own_rank() {
    let h = harness().await;
    let host = h.create("Host").await;
    let forged = json!({"xuid": host.xuid, "playlist": "1", "game_id": "g",
        "before": Standing::default(), "after": Standing { level: 50, ..Standing::default() }});
    for stream in [
        format!("rank-{}", host.xuid),
        format!("account-{}", host.xuid),
    ] {
        let jws = h.event(&host, &stream, rank::CHANGED, forged.clone());
        assert_eq!(h.post_event(&host, jws).await.status, StatusCode::FORBIDDEN);
    }
}
