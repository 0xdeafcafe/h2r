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

#[tokio::test]
async fn dry_run_apply_verify_and_rollback_keep_legacy_sources_exact() {
    use live_proto::{game, pack, sign::EventBody};
    use live_server::store::NewEvent;
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    let boss = h.create("Boss").await;
    let id = h.start_game(&boss, two_player_game(&boss)).await;
    let admin = format!("/v1/admin/replay-archives/{id}");
    let step = |request: serde_json::Value| {
        let uri = format!("{admin}/step");
        let token = h.bearer(&boss, "admin.write");
        let h = &h;
        async move {
            h.call(
                Method::POST,
                &uri,
                &[token, ("content-type", "application/json".into())],
                request.to_string(),
            )
            .await
        }
    };
    assert_eq!(
        step(json!({"mode":"apply"})).await.status,
        StatusCode::CONFLICT
    );
    let frames = [0, 2, 901].map(|tick| game::Frame {
        tick,
        game_id: id.clone(),
        players: vec![game::FramePlayer {
            alive: true,
            pos: [1.0 / 65536.0, -0.0, 1.0 / 37.0],
            ..Default::default()
        }],
        ..Default::default()
    });
    let packed = pack::encode_keyframe(&frames[2]).unwrap();
    let data = [
        (game::GAME_FRAME, serde_json::to_value(&frames[0]).unwrap()),
        (game::GAME_FRAME, serde_json::to_value(&frames[1]).unwrap()),
        (
            game::GAME_FRAME,
            json!({"tick":901,"packed":pack::to_base64(&packed)}),
        ),
        (game::GAME_ENDED, json!({"tick":901,"reason":"finished"})),
    ];
    let events = data
        .into_iter()
        .map(|(kind, d)| {
            let body = EventBody::new(
                &h.operator,
                &format!("server:{}", h.id),
                &game::stream(&id),
                kind,
                d,
                time::OffsetDateTime::now_utc(),
            );
            NewEvent {
                jws: body.sign(&h.operator).into(),
                jws_index: None,
                body,
            }
        })
        .collect();
    let written = h.live.store().append(events, &[]).await.unwrap();
    let seal = written.last().unwrap().position;
    // Wait until the signed end is visible in this already-running server.
    let mut applied = h.live.applied();
    applied.wait_for(|n| *n >= seal).await.unwrap();
    let public = format!("/v1/games/{id}/chunks?format=archive");
    let mut dry = json!({"mode":"dry_run","source_position":seal});
    let first = step(dry.clone()).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json());
    assert_eq!(first.json()["frames"], 2);
    dry["cursor"] = first.json()["next"].clone();
    let dry_end = step(dry).await.json();
    assert_eq!(dry_end["done"], true);
    assert_eq!(
        h.live.store().head().await.unwrap(),
        seal,
        "dry run writes nothing"
    );
    assert!(h.live.store().blob_names().await.unwrap().is_empty());
    assert_eq!(
        step(json!({"mode":"apply","source_position":seal-1}))
            .await
            .status,
        StatusCode::CONFLICT
    );
    let first = step(json!({"mode":"apply","source_position":seal}))
        .await
        .json();
    assert_eq!(first["state"], "staging");
    assert!(h.get(&public).await.json().get("archive_sha256").is_none());
    let next = json!({"mode":"apply","cursor":first["next"],"source_position":seal});
    let end = step(next.clone()).await.json();
    assert_eq!(end["done"], true);
    assert_eq!(end["state"], "published");
    assert_eq!(end["totals"]["frames"], 3);
    assert_eq!(end["totals"], dry_end["totals"]);
    assert_eq!(step(next).await.json()["totals"], end["totals"]);
    let index = h.get(&public).await.json();
    assert_eq!(index["items"].as_array().unwrap().len(), 2);
    let digest = end["archive_sha256"].as_str().unwrap();
    let url = format!("/v1/games/{id}/archive/{digest}/0");
    let first = h.get(&url).await;
    assert_eq!(first.headers["content-encoding"], "br");
    assert_eq!(
        first.headers["cache-control"],
        "public, max-age=31536000, immutable"
    );
    live_proto::archive::verify(&frames[..2], &first.body).unwrap();
    let second = h.get(&format!("/v1/games/{id}/archive/{digest}/1")).await;
    live_proto::archive::verify(&[pack::decode_keyframe(&packed).unwrap()], &second.body).unwrap();
    let v = step(json!({"mode":"verify"})).await.json();
    assert_eq!(
        step(json!({"mode":"verify","cursor":v["next"]}))
            .await
            .json()["backup_sha256"],
        end["backup_sha256"]
    );
    let rb = format!("{admin}/rollback");
    let rollback = |backup: serde_json::Value| {
        let token = h.bearer(&boss, "admin.write");
        let h = &h;
        let rb = &rb;
        async move {
            h.call(
                Method::POST,
                rb,
                &[token, ("content-type", "application/json".into())],
                json!({"archive_sha256":digest,"backup_sha256":backup}).to_string(),
            )
            .await
        }
    };
    assert_eq!(rollback(json!("stale")).await.status, StatusCode::CONFLICT);
    assert_eq!(
        rollback(end["backup_sha256"].clone()).await.json()["state"],
        "rolled_back"
    );
    assert_eq!(h.get(&url).await.status, StatusCode::NOT_FOUND);
    assert!(h.get(&public).await.json().get("archive_sha256").is_none());
    assert_eq!(
        h.get(&format!("/v1/games/{id}/frames?format=json"))
            .await
            .json()["items"][0]["players"][0]["pos"][0],
        1.0 / 65536.0
    );
}
