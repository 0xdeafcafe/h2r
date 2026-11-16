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
