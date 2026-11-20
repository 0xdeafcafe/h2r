//! Usage accounting through the router (docs/live/live-api.md "Usage"):
//! `GET /v1/admin/usage` and `/v1/admin/usage/accounts` are an admin's, and
//! count what the log says and what the ingest socket metered.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::{Harness, start};
use live_proto::{ClientMessage, Gamertag, lxm, streams, types};
use live_server::Policy;
use live_server::ingest::IngestSession;
use live_server::store::MemoryStore;
use serde_json::{Value, json};

async fn server() -> Harness {
    start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Policy::default()
        },
    )
    .await
}

async fn get(h: &Harness, uri: &str, auth: Option<(&'static str, String)>) -> common::Resp {
    h.call(
        Method::GET,
        uri,
        &auth.into_iter().collect::<Vec<_>>(),
        Body::empty(),
    )
    .await
}

#[tokio::test]
async fn only_admins_read_usage() {
    let h = server().await;
    let boss = h.create("Boss").await;
    let pleb = h.create("Pleb").await;
    for uri in ["/v1/admin/usage", "/v1/admin/usage/accounts"] {
        assert_eq!(
            get(&h, uri, None).await.status,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
        let r = get(&h, uri, Some(h.bearer(&pleb, "admin.read"))).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{uri}");
        // The right account with the wrong method is refused too.
        let r = get(&h, uri, Some(h.bearer(&boss, lxm::READ))).await;
        assert_ne!(r.status, StatusCode::OK, "{uri}");
        let r = get(&h, uri, Some(h.bearer(&boss, "admin.read"))).await;
        assert_eq!(
            r.status,
            StatusCode::OK,
            "{uri}: {}",
            String::from_utf8_lossy(&r.body)
        );
    }
    let r = get(
        &h,
        "/v1/admin/usage?days=0",
        Some(h.bearer(&boss, "admin.read")),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = get(
        &h,
        "/v1/admin/usage/accounts?sort=nope",
        Some(h.bearer(&boss, "admin.read")),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_log_and_the_ingest_socket_both_count() {
    let h = server().await;
    let boss = h.create("Boss").await;
    let alpha = h.create("Alpha").await;
    let bravo = h.create("Bravo").await;
    assert_eq!(h.sign_in(&alpha).await.status, StatusCode::CREATED);
    let ask = h.event(
        &alpha,
        &streams::account(alpha.xuid),
        types::FRIEND_REQUESTED,
        json!({"from": alpha.xuid, "to": bravo.xuid}),
    );
    let ask_len = ask.len() as i64;
    assert_eq!(h.post_event(&alpha, ask).await.status, StatusCode::CREATED);

    // Frames over the ingest socket: metered by the bytes received.
    let game_id = h.start_game(&alpha, common::two_player_game(&alpha)).await;
    let mut ingest = IngestSession::new(h.live.clone());
    let auth = serde_json::to_string(&ClientMessage::Auth {
        token: h.token(&alpha.key, &alpha.author, lxm::GAMES_WRITE),
    })
    .unwrap();
    assert!(ingest.on_text(&auth).await.is_empty());
    let frame = live_proto::game::Frame {
        game_id: game_id.clone(),
        tick: 1,
        ..Default::default()
    };
    let text = serde_json::to_string(&ClientMessage::Frame {
        frame: Box::new(frame),
    })
    .unwrap();
    ingest.on_text(&text).await;
    ingest.on_binary(&[0u8; 40]).await;

    let admin = || Some(h.bearer(&boss, "admin.read"));
    let o = get(&h, "/v1/admin/usage?days=7", admin()).await.json();
    let today = &o["days"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(today["sign_ins"], 1);
    assert_eq!(today["messages"]["friend_requests"], 1);
    assert_eq!(today["messages"]["invites"], 0);
    assert_eq!(today["up"]["frames"], text.len() as i64 + 40);
    assert!(today["up"]["events"].as_i64().unwrap() >= ask_len);
    assert!(today["events"].as_i64().unwrap() >= 5, "{today}");
    assert!(
        o["event_types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["type"] == "FriendRequested" && t["count"] == 1)
    );
    assert!(o["stored"]["events"].as_i64().unwrap() > 0);
    assert_eq!(o["keep_days"], 400);

    let page = get(
        &h,
        "/v1/admin/usage/accounts?days=7&sort=uploaded&limit=1",
        admin(),
    )
    .await
    .json();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    // Signing up alone is the server's event, not the account's doing.
    assert_eq!(page["total"], 1, "{page}");
    let top: &Value = &page["items"][0];
    assert_eq!(top["gamertag"], "Alpha");
    assert_eq!(top["xuid"], alpha.xuid.to_string());
    assert_eq!(top["up"]["frames"], text.len() as i64 + 40);
    assert_eq!(top["messages_total"], 1);
    assert_eq!(top["sign_ins"], 1);
    assert_eq!(top["games"], 1);
    assert_eq!(top["active_days"], 1);
}
