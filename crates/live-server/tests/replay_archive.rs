//! Inventory used by the finished-game archiver and its migration CLI.
mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::{start, two_player_game};
use live_proto::Gamertag;
use live_server::{Policy, store::MemoryStore};
use serde_json::json;

#[tokio::test]
async fn archive_inventory_is_admin_only_paged_and_tracks_sealed_games() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    let admin = h.create("Boss").await;
    let host = h.create("Host").await;
    let older = h.start_game(&host, two_player_game(&host)).await;
    let newer = h.start_game(&host, two_player_game(&host)).await;
    let path = "/v1/admin/replay-archives";
    assert_eq!(h.get(path).await.status, StatusCode::UNAUTHORIZED);
    let denied = h
        .call(
            Method::GET,
            path,
            &[h.bearer(&host, "admin.read")],
            Body::empty(),
        )
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let get = |uri: String| {
        let token = h.bearer(&admin, "admin.read");
        let h = &h;
        async move { h.call(Method::GET, &uri, &[token], Body::empty()).await }
    };
    let page = get(format!("{path}?limit=1")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.headers["cache-control"], "private, no-store");
    let page = page.json();
    assert_eq!(page["items"][0]["game_id"], newer);
    assert_eq!(page["items"][0]["state"], "live");
    let next = page["next"].as_str().unwrap();
    let second = get(format!("{path}?limit=1&cursor={next}")).await.json();
    assert_eq!(second["items"][0]["game_id"], older);
    assert!(second["next"].is_null());
    let end = h.batch(
        &host,
        &newer,
        &[("GameEnded", json!({"tick": 12, "reason": "finished"}))],
    );
    assert_eq!(
        h.post_batch(&host, &newer, end).await.status,
        StatusCode::CREATED
    );
    let info = get(format!("{path}/{newer}")).await.json();
    assert_eq!(info["ended"], true);
    assert_eq!(info["state"], "empty");
    assert!(
        info["source_position"].as_u64().unwrap()
            > page["items"][0]["source_position"].as_u64().unwrap()
    );
}
