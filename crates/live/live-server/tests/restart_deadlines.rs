//! A restart (a Worker deploy restarts the hub) is not an absence: pings
//! and game frames are not in the log, so presence, open sessions and live
//! games get a fresh deadline when the log is replayed, not the one their
//! last event left them.
mod common;

use axum::http::StatusCode;
use common::*;
use live_server::Policy;
use live_server::store::MemoryStore;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn a_restart_does_not_expire_players_who_are_still_pinging() {
    let store = Arc::new(MemoryStore::new());
    let (pete, id) = {
        let h = start(store.clone(), Policy::default()).await;
        let pete = h.create("Pete").await;
        assert_eq!(h.sign_in(&pete).await.status, StatusCode::CREATED);
        let id = h.start_game(&pete, two_player_game(&pete)).await;
        (pete, id)
    };
    // Pete sits in the game, pinging (not in the log), for longer than
    // the TTL; then the server restarts.
    tokio::time::sleep(Duration::from_millis(3100)).await;
    let policy = Policy {
        presence_ttl_s: 2,
        game_idle_s: 2,
        ..Policy::default()
    };
    let h = start_driven(store, policy).await;
    h.live.sweep().await;
    assert_eq!(
        h.get("/v1/presence").await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "a restart expired a player who never left"
    );
    assert_eq!(h.ping(&pete).await.status, StatusCode::OK);
    assert_ne!(
        h.get(&format!("/v1/games/{id}")).await.json()["state"],
        "ended",
        "a restart lost a game in play"
    );

    // Whoever really is gone still expires a TTL after the restart.
    tokio::time::sleep(Duration::from_millis(3100)).await;
    h.live.sweep().await;
    assert_eq!(h.get("/v1/presence").await.json()["items"], json!([]));
    assert_eq!(
        h.get(&format!("/v1/games/{id}")).await.json()["end_reason"],
        "lost"
    );
}
