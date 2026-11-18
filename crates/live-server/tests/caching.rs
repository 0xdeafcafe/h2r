//! What the CDN may keep (`crates/live-server/src/caching.rs`): built
//! assets for a year, the app's HTML revalidated, anonymous public reads a
//! few seconds with an ETag, everything with a token never; and the
//! page-load snapshot (`GET /v1/snapshot`).
mod common;
use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use live_server::caching::{IMMUTABLE, PRIVATE, PUBLIC, REVALIDATE};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn built_assets_are_immutable_and_the_app_revalidates() {
    let h = harness().await;
    let dir = temp_dir().join("web");
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html><title>x</title>").unwrap();
    std::fs::write(dir.join("assets/index-Bl7_M9u5.js"), "1").unwrap();
    let app = live_server::http::router_with_web(h.live.clone(), Some(&dir));
    let get = |uri: &str| {
        let req = axum::http::Request::get(uri).body(Body::empty()).unwrap();
        let app = app.clone();
        async move { app.oneshot(req).await.unwrap() }
    };
    let r = get("/assets/index-Bl7_M9u5.js").await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()["cache-control"], IMMUTABLE);
    for page in ["/", "/games/halo2/games", "/dash/players"] {
        let r = get(page).await;
        assert_eq!(r.status(), StatusCode::OK, "{page}");
        assert_eq!(r.headers()["cache-control"], REVALIDATE, "{page}");
    }
    // A missing asset (answered with the app's HTML) is not kept for a
    // year.
    let r = get("/assets/gone-0000.js").await;
    assert_eq!(r.headers()["cache-control"], REVALIDATE);
    // API paths stay JSON, and errors are never kept.
    let r = get("/v1/games/0123456789abcdef0123456789abcdef").await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_eq!(r.headers()["cache-control"], PRIVATE);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn public_reads_carry_an_etag_and_answer_304() {
    let h = harness().await;
    let host = h.create("Host").await;
    h.start_game(&host, two_player_game(&host)).await;
    let r = h.get("/v1/games?state=live").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["cache-control"], PUBLIC);
    let tag = r.headers["etag"].to_str().unwrap().to_string();
    assert!(tag.starts_with("W/\""), "{tag}");
    let again = h
        .call(
            Method::GET,
            "/v1/games?state=live",
            &[("if-none-match", tag.clone())],
            Body::empty(),
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    assert!(again.body.is_empty());
    assert_eq!(again.headers["etag"].to_str().unwrap(), tag);
    // A change makes a new tag.
    h.start_game(&host, two_player_game(&host)).await;
    let changed = h
        .call(
            Method::GET,
            "/v1/games?state=live",
            &[("if-none-match", tag.clone())],
            Body::empty(),
        )
        .await;
    assert_eq!(changed.status, StatusCode::OK);
    assert_ne!(changed.headers["etag"].to_str().unwrap(), tag);
    // With a token, or marked as sent with one: private, no tag.
    for (uri, headers) in [
        (
            "/v1/games?state=live",
            vec![h.bearer(&host, live_proto::lxm::READ)],
        ),
        ("/v1/games?state=live&auth=1", vec![]),
    ] {
        let r = h.call(Method::GET, uri, &headers, Body::empty()).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}");
        assert_eq!(r.headers["cache-control"], PRIVATE, "{uri}");
        assert!(!r.headers.contains_key("etag"), "{uri}");
    }
    // Writes are not touched.
    let r = h
        .call(
            Method::POST,
            "/v1/games",
            &[h.bearer(&host, live_proto::lxm::GAMES_WRITE)],
            two_player_game(&host).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(!r.headers.contains_key("cache-control"));
}

/// The snapshot is the lists, the details and the log's tail, at one
/// position, without the 1 Hz GameFrames the stream leaves out too.
#[tokio::test]
async fn the_snapshot_is_one_read_of_what_the_page_needs() {
    let h = harness().await;
    let host = h.create("Host").await;
    let live_game = h.start_game(&host, two_player_game(&host)).await;
    let ended = h.start_game(&host, two_player_game(&host)).await;
    let r = h
        .post_batch(
            &host,
            &ended,
            h.batch(
                &host,
                &ended,
                &[(
                    "GameEnded",
                    json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
                )],
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    // A recorded frame in the log (as ingest writes one a second).
    let body = live_proto::sign::EventBody::new(
        &h.operator,
        &format!("server:{}", h.id),
        &live_proto::game::stream(&live_game),
        live_proto::game::GAME_FRAME,
        json!({"tick": 30}),
        time::OffsetDateTime::now_utc(),
    );
    let frame = live_server::store::NewEvent {
        jws: body.sign(&h.operator).into(),
        jws_index: None,
        body,
    };
    h.live.store().append(vec![frame], &[]).await.unwrap();

    let snap = h.get("/v1/snapshot").await;
    assert_eq!(snap.status, StatusCode::OK, "{}", snap.error());
    let s = snap.json();
    let ids = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|g| g["game_id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(&s["live"]["items"]), std::slice::from_ref(&live_game));
    assert_eq!(ids(&s["ended"]["items"]), std::slice::from_ref(&ended));
    assert_eq!(ids(&s["details"]), [live_game.clone(), ended.clone()]);
    assert_eq!(
        s["details"][1],
        h.get(&format!("/v1/games/{ended}")).await.json()
    );
    assert_eq!(
        s["accounts"]["items"],
        h.get("/v1/accounts?limit=1000").await.json()["items"]
    );
    let position = s["position"].as_u64().unwrap();
    let tail = s["tail"].as_array().unwrap();
    assert!(!tail.is_empty());
    assert!(
        tail.iter().all(|e| e["type"] != "GameFrame"),
        "no GameFrames"
    );
    assert!(
        tail.iter()
            .all(|e| e["position"].as_u64().unwrap() <= position)
    );
    assert!(tail.iter().any(|e| e["type"] == "GameEnded"));
}
