//! `GET /v1/stats/summary` through the router.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

#[tokio::test]
async fn the_summary_follows_games_as_they_are_played() {
    let h = harness().await;
    let empty = h.get("/v1/stats/summary").await;
    assert_eq!(empty.status, StatusCode::OK);
    let v = empty.json();
    assert_eq!(v["window"], "7d");
    assert_eq!(v["totals"]["games"], 0);
    assert_eq!(v["players"], json!([]));

    let host = h.create("Host").await;
    let id = h.start_game(&host, two_player_game(&host)).await;
    let jws = h.batch(
        &host,
        &id,
        &[
            (
                "PlayerKilled",
                json!({"tick": 30, "victim": 1, "killer": 0, "weapon": "battle_rifle", "kind": "kill"}),
            ),
            (
                "GameEnded",
                json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
            ),
        ],
    );
    assert_eq!(
        h.post_batch(&host, &id, jws).await.status,
        StatusCode::CREATED
    );

    for window in ["today", "7d", "30d", "all"] {
        let v = h
            .get(&format!("/v1/stats/summary?window={window}"))
            .await
            .json();
        assert_eq!(v["window"], window);
        assert_eq!(v["totals"]["games"], 1, "{window}");
        assert_eq!(v["totals"]["kills"], 1);
        assert_eq!(v["all_time"]["kills"], 1);
        assert_eq!(v["last_24h"]["games"], 1);
        assert_eq!(v["players"][0]["xuid"], host.xuid.to_string());
        assert_eq!(v["players"][0]["kills"], 1);
        assert_eq!(v["players"][0]["wins"], 1);
        assert_eq!(
            v["weapons"][0],
            json!({"weapon": "battle_rifle", "kills": 1})
        );
        assert_eq!(v["maps"][0]["map"], "lockout");
        assert_eq!(v["activity"][0]["games"], 1);
        assert!(v["position"].as_u64().unwrap() > 0);
    }
    assert!(h.get("/v1/stats/summary?window=all").await.json()["from"].is_null());

    let bad = h.get("/v1/stats/summary?window=1y").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

const FIREFIGHT_GOLDEN: &str = include_str!("fixtures/firefight-summary.json");

#[tokio::test]
async fn the_firefight_summary_is_the_golden_one_and_players_cannot_write_it() {
    let h = harness().await;
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    let id = h.start_game(&host, firefight_game(&host)).await;
    // An account that is not the game's host writes no score.
    let forged = h.batch(
        &guest,
        &id,
        &[(
            "FirefightScore",
            json!({"tick": 5, "player": 0, "points": 1000, "reason": "kill", "class": "grunt"}),
        )],
    );
    assert_eq!(h.post_batch(&guest, &id, forged).await.status, StatusCode::FORBIDDEN);

    // A bad score is refused whole.
    let bad = h.batch(
        &host,
        &id,
        &[("FirefightScore", json!({"tick": 5, "player": 7, "points": 1, "reason": "kill"}))],
    );
    assert_eq!(h.post_batch(&host, &id, bad).await.status, StatusCode::BAD_REQUEST);

    // A two-team result that names no winner is refused whole too.
    let crossfire = h.start_game(&host, firefight_crossfire_game(&host)).await;
    let bad = h.batch(
        &host,
        &crossfire,
        &[(
            "FirefightEnded",
            json!({"tick": 5, "set": "Crossfire", "teams": 2, "outcome": "team_won", "scores": [1, 2], "rounds_cleared": 0, "lives_used": 0}),
        )],
    );
    assert_eq!(h.post_batch(&host, &crossfire, bad).await.status, StatusCode::BAD_REQUEST);

    // The one-team match, then the two-team one.
    for (game, events) in [(&id, firefight_batch()), (&crossfire, firefight_crossfire_batch())] {
        let pairs: Vec<(&str, serde_json::Value)> =
            events.iter().map(|(k, d)| (k.as_str(), d.clone())).collect();
        let jws = h.batch(&host, game, &pairs);
        assert_eq!(h.post_batch(&host, game, jws).await.status, StatusCode::CREATED);
    }

    let golden: serde_json::Value =
        serde_json::from_str(&FIREFIGHT_GOLDEN.replace("$HOST", &host.xuid.to_string())).unwrap();
    let v = h.get("/v1/stats/summary?window=all").await.json();
    assert_eq!(v["firefight"], golden);
    assert_eq!(v["totals"]["kills"], 0);
}
