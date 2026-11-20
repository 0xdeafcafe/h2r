//! Theater through the Live router and the signed-log projection.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::*;
use http_body_util::BodyExt;
use live_proto::{Gamertag, Xuid, lxm};
use live_server::app::Timers;
use live_server::store::MemoryStore;
use live_server::{Live, Policy, ServerIdentity, http, page_meta};
use serde_json::{Value, json};
use tower::ServiceExt;

fn player_films(player: Xuid, kind: &str) -> String {
    format!("/v1/films?player={player}&kind={kind}")
}

fn clips(game: &str) -> String {
    format!("/v1/films/{game}/clips")
}

async fn write(h: &Harness, user: &User, method: Method, path: &str, body: Body) -> Resp {
    h.call(method, path, &[h.bearer(user, lxm::FILMS_WRITE)], body)
        .await
}

async fn pin(h: &Harness, user: &User, game: &str, save: bool) -> Resp {
    let method = if save { Method::POST } else { Method::DELETE };
    write(
        h,
        user,
        method,
        &format!("/v1/films/{game}/save"),
        Body::empty(),
    )
    .await
}

async fn clip(h: &Harness, user: &User, game: &str, body: Value) -> Resp {
    write(
        h,
        user,
        Method::POST,
        &clips(game),
        Body::from(body.to_string()),
    )
    .await
}

async fn create_clip(h: &Harness, user: &User, game: &str, title: &str) -> Value {
    let r = clip(
        h,
        user,
        game,
        json!({"from_tick":0,"to_tick":900,"title":title}),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
    r.json()
}

async fn read_as(h: &Harness, user: &User, path: &str) -> Resp {
    h.call(
        Method::GET,
        path,
        &[h.bearer(user, lxm::READ)],
        Body::empty(),
    )
    .await
}

async fn assert_static_preview(h: &Harness, path: &str) {
    let meta = h.get(&format!("/v1/page-meta?path={path}")).await;
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.json(), json!(page_meta::describe_static(path)));
}

async fn restart(h: &mut Harness, store: Arc<MemoryStore>, policy: Policy) {
    let identity = ServerIdentity::load(&h.dir).unwrap();
    let live = Live::start_with(store, identity, policy, Timers::Driven)
        .await
        .unwrap();
    h.app = http::router(live.clone());
    h.live = live;
}

#[tokio::test]
async fn recent_films_are_derived_and_pins_protect_the_archive_until_the_last_unpin() {
    let store = Arc::new(MemoryStore::new());
    let policy = Policy {
        admins: vec![Gamertag::new("Boss").unwrap()],
        ..Default::default()
    };
    let mut h = start_driven(store.clone(), policy.clone()).await;
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    let boss = h.create("Boss").await;
    let mut start = two_player_game(&host);
    start["tick"] = json!(30);
    let oldest = h.start_game(&host, start).await;
    let end = h.batch(
        &host,
        &oldest,
        &[
            (
                "PlayerJoined",
                json!({"player":{"index":2,"name":"Guest","team":1,"xuid":guest.xuid}}),
            ),
            ("PlayerLeft", json!({"index":2})),
            (
                "GameEnded",
                json!({"tick":933,"reason":"finished","winner":{"team":0}}),
            ),
        ],
    );
    assert_eq!(
        h.post_batch(&host, &oldest, end).await.status,
        StatusCode::CREATED
    );

    let recent = player_films(host.xuid, "recent");
    let saved = player_films(host.xuid, "saved");
    let guest_recent = h.get(&player_films(guest.xuid, "recent")).await.json();
    assert_eq!(guest_recent["items"][0]["result"], "loss");
    assert_eq!(guest_recent["items"][0]["duration_s"], 30);
    assert_eq!(h.get(&recent).await.json()["items"][0]["result"], "win");

    let mut ids = Vec::new();
    for _ in 0..25 {
        ids.push(h.start_game(&host, two_player_game(&host)).await);
    }
    let r = h.get(&recent).await.json();
    let ids: Vec<_> = ids.iter().rev().map(String::as_str).collect();
    assert_eq!(
        r["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["game_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ids
    );
    assert!(r["next"].is_null());
    assert_eq!(h.get(&saved).await.json()["items"], json!([]));

    let path = format!("/v1/films/{oldest}/save");
    assert_eq!(
        h.call(Method::POST, &path, &[], Body::empty()).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        pin(&h, &boss, &oldest, true).await.status,
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        pin(&h, &host, &oldest, true).await.status,
        StatusCode::NO_CONTENT
    );
    let position = h.live.position();
    assert_eq!(
        pin(&h, &host, &oldest, true).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(h.live.position(), position);
    assert_eq!(
        pin(&h, &guest, &oldest, true).await.status,
        StatusCode::NO_CONTENT
    );
    let authored = create_clip(&h, &guest, &oldest, "An older film").await;

    restart(&mut h, store, policy).await;
    let saved_item = &h.get(&saved).await.json()["items"][0];
    assert_eq!(saved_item["game_id"], oldest);
    assert_eq!(saved_item["map"], "lockout");
    assert_eq!(saved_item["mode"], "multiplayer");
    assert_eq!(saved_item["variant"], "Team Slayer");
    assert!(saved_item["started_at"].is_string());
    assert_eq!(saved_item["duration_s"], 30);
    assert_eq!(saved_item["result"], "win");
    let player_clips = h.get(&player_films(guest.xuid, "clips")).await.json();
    assert_eq!(player_clips["items"][0]["clip"], authored);
    assert_eq!(player_clips["items"][0]["film"], guest_recent["items"][0]);

    let inventory = |h: &Harness| h.bearer(&boss, "admin.read");
    let archive = format!("/v1/admin/replay-archives/{oldest}");
    assert_eq!(
        h.call(Method::GET, &archive, &[inventory(&h)], Body::empty())
            .await
            .json()["pinned"],
        true
    );
    assert_eq!(
        pin(&h, &host, &oldest, false).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(h.get(&saved).await.json()["items"], json!([]));
    assert_eq!(
        h.call(Method::GET, &archive, &[inventory(&h)], Body::empty())
            .await
            .json()["pinned"],
        true
    );
    assert_eq!(
        pin(&h, &guest, &oldest, false).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        pin(&h, &guest, &oldest, false).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        h.call(Method::GET, &archive, &[inventory(&h)], Body::empty())
            .await
            .json()["pinned"],
        false
    );
}

#[tokio::test]
async fn clip_validation_authorization_and_link_previews() {
    let h = start_driven(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    let host = h.create("Host").await;
    let author = h.create("Author").await;
    let admin = h.create("Boss").await;
    let game = h.start_game(&host, two_player_game(&host)).await;
    let other_game = h.start_game(&host, two_player_game(&host)).await;
    let body = json!({"from_tick":10,"to_tick":910,"title":"Clip"});
    for headers in [vec![], vec![h.bearer(&author, lxm::READ)]] {
        assert_eq!(
            h.call(Method::POST, &clips(&game), &headers, body.to_string())
                .await
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
    for (from, to, title) in [
        (0, 899, "Short".into()),
        (0, 9001, "Long".into()),
        (1000, 100, "Reverse".into()),
        (0, 900, "é".repeat(61)),
        (0, 900, " \t\u{0000}".into()),
    ] {
        let r = clip(
            &h,
            &author,
            &game,
            json!({"from_tick":from,"to_tick":to,"title":title}),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.json());
        assert_eq!(r.error(), "invalid");
    }
    let made = clip(
        &h,
        &author,
        &game,
        json!({
            "from_tick": 10, "to_tick": 910, "title": " \u{0000} > \n Final \u{202e} & \t ",
            "pov": 1, "camera": "follow", "by": host.xuid, "created_ms": 0
        }),
    )
    .await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.json());
    let made = made.json();
    let id = made["id"].as_str().unwrap();
    assert_eq!(
        made,
        json!({
            "id": id, "game": game, "from_tick": 10, "to_tick": 910, "title": "> Final &",
            "by": author.xuid, "pov": 1, "camera": "follow", "created_ms": made["created_ms"]
        })
    );
    assert!(made["created_ms"].as_i64().unwrap() > 0);

    let other = clip(
        &h,
        &author,
        &other_game,
        json!({"from_tick":0,"to_tick":9000,"title":"é".repeat(60)}),
    )
    .await;
    assert_eq!(other.status, StatusCode::CREATED, "{:?}", other.json());
    let other = other.json();
    assert!(other.get("pov").is_none());
    assert!(other.get("camera").is_none());
    let authored = h.get(&player_films(author.xuid, "clips")).await.json();
    assert_eq!(authored["items"][0]["clip"], other);
    assert_eq!(authored["items"][1]["clip"], made);
    assert_eq!(authored["items"][1]["film"]["game_id"], game);
    assert!(authored["items"][1]["film"]["result"].is_null());
    let authored_path = player_films(author.xuid, "clips");
    let first_page = h.get(&format!("{authored_path}&limit=1")).await.json();
    assert_eq!(first_page["items"], json!([authored["items"][0]]));
    let cursor = first_page["next"].as_str().unwrap();
    let last_page = h
        .get(&format!("{authored_path}&limit=1&cursor={cursor}"))
        .await
        .json();
    assert_eq!(last_page["items"], json!([authored["items"][1]]));
    assert!(last_page["next"].is_null());
    assert_eq!(h.get(&clips(&game)).await.json()["items"], json!([made]));

    let item = format!("/v1/clips/{id}");
    assert_eq!(h.get(&item).await.json(), made);
    assert_eq!(
        write(&h, &host, Method::DELETE, &item, Body::empty())
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    let path = format!("/theater/{game}/clip/{id}");
    let meta = h.get(&format!("/v1/page-meta?path={path}")).await.json();
    assert_eq!(meta["page"], "> Final &");
    assert_eq!(
        meta["description"],
        "A 30-second Theater clip from Team Slayer on Lockout."
    );
    let web = h.dir.join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(
        web.join("index.html"),
        "<html><head></head><body></body></html>",
    )
    .unwrap();
    let html = http::router_with_web(h.live.clone(), Some(&web))
        .oneshot(
            Request::get(path.as_str())
                .header("host", "live.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(html.status(), StatusCode::OK);
    let body = html.into_body().collect().await.unwrap().to_bytes();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains(r#"property="og:title" content="&gt; Final &amp;""#));
    assert!(body.contains(&format!("https://live.example{path}")));
    assert_static_preview(&h, &format!("/theater/{other_game}/clip/{id}")).await;

    assert_eq!(
        write(&h, &admin, Method::DELETE, &item, Body::empty())
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(h.get(&item).await.status, StatusCode::NOT_FOUND);
    assert_eq!(h.get(&clips(&game)).await.json()["items"], json!([]));
    let authored = h.get(&player_films(author.xuid, "clips")).await.json();
    assert_eq!(authored["items"].as_array().unwrap().len(), 1);
    assert_eq!(authored["items"][0]["clip"], other);
    assert_static_preview(&h, &path).await;
}

#[tokio::test]
async fn clip_quota_spans_games_and_survives_deletion_and_restart() {
    let store = Arc::new(MemoryStore::new());
    let policy = Policy::default();
    let mut h = start_driven(store.clone(), policy.clone()).await;
    let author = h.create("Author").await;
    let another = h.create("Another").await;
    let game = h.start_game(&author, two_player_game(&author)).await;
    let other_game = h.start_game(&author, two_player_game(&author)).await;
    let first = create_clip(&h, &author, &game, "First").await;
    for _ in 1..19 {
        create_clip(&h, &author, &game, "Play").await;
    }

    let body = json!({"from_tick":0,"to_tick":900,"title":"Quota boundary"});
    let (a, b) = tokio::join!(
        clip(&h, &author, &game, body.clone()),
        clip(&h, &author, &other_game, body.clone()),
    );
    let limited = if a.status == StatusCode::CREATED {
        b
    } else {
        assert_eq!(b.status, StatusCode::CREATED, "{:?}", b.json());
        a
    };
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.error(), "rate_limited");
    assert!(
        limited.json()["message"]
            .as_str()
            .unwrap()
            .contains("20 clips")
    );

    let first = format!("/v1/clips/{}", first["id"].as_str().unwrap());
    assert_eq!(
        write(&h, &author, Method::DELETE, &first, Body::empty())
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    restart(&mut h, store, policy).await;
    assert_eq!(h.get(&first).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        h.get(&player_films(author.xuid, "clips")).await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        19
    );
    assert_eq!(
        clip(&h, &author, &game, body).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    create_clip(&h, &another, &other_game, "Another player's clip").await;
}

#[tokio::test]
async fn hidden_games_hide_theater_lists_clips_log_entries_and_previews() {
    let h = harness().await;
    let host = h.create("Host").await;
    let stranger = h.create("Stranger").await;
    let session = h
        .call(
            Method::POST,
            "/v1/sessions",
            &[h.bearer(&host, lxm::SESSIONS_WRITE)],
            json!({
                "addr": "192.168.2.4", "port": 3074, "host_address": vec![0; 36],
                "slots": {"public_filled":1,"public_open":1,"private_filled":0,"private_open":0},
                "attributes": {}, "flags": "coop", "visibility": "invite",
                "campaign": {"level":"03a_oldmombasa","name":"Outskirts","difficulty":2}
            })
            .to_string(),
        )
        .await;
    assert_eq!(session.status, StatusCode::CREATED, "{:?}", session.json());
    let game = h
        .start_game(
            &host,
            json!({
                "map":"03a_oldmombasa","mode":"campaign","online":true,"team_play":false,
                "players":[{"index":0,"name":"Host","xuid":host.xuid,"team":0}]
            }),
        )
        .await;
    assert_eq!(
        h.get(&format!("/v1/games/{game}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pin(&h, &host, &game, true).await.status,
        StatusCode::NO_CONTENT
    );
    let made = create_clip(&h, &host, &game, "Hidden clip").await;
    let id = made["id"].as_str().unwrap();
    let item = format!("/v1/clips/{id}");
    for path in [clips(&game), item.clone()] {
        assert_eq!(h.get(&path).await.status, StatusCode::NOT_FOUND);
        assert_eq!(
            read_as(&h, &stranger, &path).await.status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(read_as(&h, &host, &path).await.status, StatusCode::OK);
    }
    for kind in ["recent", "saved", "clips"] {
        let path = player_films(host.xuid, kind);
        assert_eq!(h.get(&path).await.json()["items"], json!([]));
        assert_eq!(
            read_as(&h, &stranger, &path).await.json()["items"],
            json!([])
        );
        assert_eq!(
            read_as(&h, &host, &path).await.json()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        clip(
            &h,
            &stranger,
            &game,
            json!({"from_tick":0,"to_tick":900,"title":"Denied"})
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        write(&h, &stranger, Method::DELETE, &item, Body::empty())
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.get("/v1/events?stream=theater").await.json()["items"],
        json!([])
    );
    assert_static_preview(&h, &format!("/theater/{game}/clip/{id}")).await;
}

#[tokio::test]
async fn private_server_requires_read_tokens_and_uses_generic_previews() {
    let h = start_driven(
        Arc::new(MemoryStore::new()),
        Policy {
            private: true,
            ..Default::default()
        },
    )
    .await;
    let host = h.create("Host").await;
    let game = h.start_game(&host, two_player_game(&host)).await;
    let made = create_clip(&h, &host, &game, "Private clip").await;
    let id = made["id"].as_str().unwrap();
    for path in [
        player_films(host.xuid, "recent"),
        clips(&game),
        format!("/v1/clips/{id}"),
    ] {
        assert_eq!(h.get(&path).await.status, StatusCode::UNAUTHORIZED);
        assert_eq!(read_as(&h, &host, &path).await.status, StatusCode::OK);
    }
    assert_static_preview(&h, &format!("/theater/{game}/clip/{id}")).await;
}
