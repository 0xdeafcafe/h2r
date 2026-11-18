//! Games, outlines, invites, presence pings, the managers and the web app
//! fallback, through the router.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use live_proto::sign::{AccountRequest, KeyPair};
use live_proto::{Author, lxm, streams, types};
use live_server::store::MemoryStore;
use live_server::{Policy, http};
use serde_json::{Value, json};

fn kill(tick: u64, victim: u32, killer: u32) -> (&'static str, Value) {
    (
        "PlayerKilled",
        json!({"tick": tick, "victim": victim, "killer": killer, "weapon": "battle_rifle", "kind": "kill", "pos": [1.5, -2.0, 3.25]}),
    )
}

fn score(tick: u64, a: i64, b: i64) -> (&'static str, Value) {
    (
        "ScoreChanged",
        json!({"tick": tick,
        "players": [{"index": 0, "score": a, "kills": a, "deaths": b, "assists": 0}, {"index": 1, "score": b, "kills": b, "deaths": a, "assists": 0}],
        "teams": [{"team": 0, "score": a}, {"team": 1, "score": b}]}),
    )
}

#[tokio::test]
async fn a_game_from_start_to_end() {
    let h = harness().await;
    let host = h.create("Host").await;
    let id = h.start_game(&host, two_player_game(&host)).await;
    assert_eq!(id.len(), 32);

    let list = h.get("/v1/games?state=live").await.json();
    assert_eq!(list["items"][0]["game_id"], id.as_str());
    assert_eq!(list["items"][0]["team_play"], true);
    assert_eq!(list["items"][0]["state"], "live");
    assert_eq!(
        h.get("/v1/games?state=ended").await.json()["items"],
        json!([])
    );

    // GameStarted is the server's, naming the host.
    let started = &h.get(&format!("/v1/events?stream=game-{id}")).await.json()["items"][0];
    assert_eq!(
        (started["type"].as_str(), started["author"].as_str()),
        (Some("GameStarted"), Some("server:live.test"))
    );
    assert_eq!(started["data"]["host"], host.author.as_str());

    let jws = h.batch(
        &host,
        &id,
        &[
            (
                "PlayerJoined",
                json!({"player": {"index": 2, "name": "Late", "team": 0}}),
            ),
            kill(300, 1, 0),
            kill(330, 2, 1),
            score(330, 1, 1),
        ],
    );
    let r = h.post_batch(&host, &id, jws.clone()).await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let last = r.json()["position"].as_u64().unwrap();

    // Each stored event keeps the batch JWS and its index; the chain runs on.
    let events = h.get(&format!("/v1/events?stream=game-{id}")).await.json();
    let items = events["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(items[4]["position"], last);
    assert_eq!(items[2]["jws_index"], 1);
    assert_eq!(items[2]["jws"].as_str(), Some(jws.as_str()));
    assert!(items[0].get("jws_index").is_none());
    for w in items.windows(2) {
        let want =
            live_proto::sign::chain_hash(w[0]["hash"].as_str(), w[1]["jws"].as_str().unwrap());
        assert_eq!(w[1]["hash"].as_str(), Some(want.as_str()));
    }

    // The same batch again: its nonces are spent.
    assert_eq!(
        h.post_batch(&host, &id, jws).await.status,
        StatusCode::CONFLICT
    );

    let end = h.batch(
        &host,
        &id,
        &[
            score(900, 2, 1),
            (
                "GameEnded",
                json!({"tick": 900, "reason": "finished", "winner": {"team": 0},
        "players": [{"index": 0, "score": 2, "kills": 2, "deaths": 1, "assists": 0}]}),
            ),
        ],
    );
    assert_eq!(
        h.post_batch(&host, &id, end).await.status,
        StatusCode::CREATED
    );

    let g = h.get(&format!("/v1/games/{id}")).await.json();
    assert_eq!(g["state"], "ended");
    assert_eq!(g["end_reason"], "finished");
    assert_eq!(g["winner"], json!({"team": 0}));
    assert_eq!(g["last_tick"], 900);
    assert_eq!(g["host"], host.author.as_str());
    assert_eq!(g["console"], "Den");
    assert_eq!(g["scoreboard"][0]["score"], 2);
    assert_eq!(
        g["teams"],
        json!([{"team": 0, "score": 2}, {"team": 1, "score": 1}])
    );
    assert_eq!(g["players"].as_array().unwrap().len(), 3);
    let kills = g["kills"].as_array().unwrap();
    assert_eq!(kills.len(), 2);
    assert_eq!(kills[0]["pos"], json!([1.5, -2.0, 3.25]));
    assert!(kills[0]["position"].as_u64().is_some() && kills[0]["at"].is_string());
    assert!(g["ended_at"].is_string());

    // An ended game takes no more events.
    let late = h.batch(&host, &id, &[kill(1000, 1, 0)]);
    assert_eq!(
        h.post_batch(&host, &id, late).await.status,
        StatusCode::CONFLICT
    );

    let account = h.get("/v1/accounts/Host").await.json();
    assert_eq!(account["recent_games"][0]["game_id"], id.as_str());
    assert_eq!(
        h.get("/v1/games?state=ended").await.json()["items"][0]["game_id"],
        id.as_str()
    );
}

/// A modded Lockout is `map: "lockout"` with a `map_id`: the game keeps it,
/// and the list can keep it apart from Lockout's other games.
#[tokio::test]
async fn games_keep_their_map_id_and_filter_by_it() {
    let h = harness().await;
    let host = h.create("Host").await;
    let retail = h.start_game(&host, two_player_game(&host)).await;
    let modded_id = "ab".repeat(32);
    let mut game = two_player_game(&host);
    game["map_id"] = json!(modded_id);
    let modded = h.start_game(&host, game).await;
    let mut other = two_player_game(&host);
    other["map"] = json!("midship");
    let midship = h.start_game(&host, other).await;

    let ids = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["game_id"].as_str().unwrap().to_string())
            .collect()
    };
    let all = h.get("/v1/games").await.json();
    assert_eq!(ids(&all), [midship.clone(), modded.clone(), retail.clone()]);
    assert_eq!(all["items"][1]["map_id"], modded_id.as_str());
    assert_eq!(all["items"][1]["map"], "lockout");
    assert!(all["items"][2].get("map_id").is_none(), "none is left out");
    assert_eq!(
        h.get(&format!("/v1/games/{modded}")).await.json()["map_id"],
        modded_id.as_str()
    );
    assert_eq!(
        ids(&h.get("/v1/games?map=lockout").await.json()),
        [modded.clone(), retail.clone()]
    );
    assert_eq!(
        ids(&h.get(&format!("/v1/games?map_id={modded_id}")).await.json()),
        std::slice::from_ref(&modded)
    );
    assert_eq!(
        ids(&h.get("/v1/games?map=lockout&map_id=none").await.json()),
        std::slice::from_ref(&retail)
    );
    assert_eq!(
        h.get("/v1/games?map_id=nope").await.status,
        StatusCode::BAD_REQUEST
    );
    // A malformed map_id on the way in is refused, not dropped.
    let mut bad = two_player_game(&host);
    bad["map_id"] = json!("xyz");
    let r = h
        .call(
            Method::POST,
            "/v1/games",
            &[h.bearer(&host, lxm::GAMES_WRITE)],
            bad.to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

/// A game's skulls are kept as sent, shown in the list, the detail and the
/// account's recent games, and filter the list; old payloads have none.
#[tokio::test]
async fn games_keep_their_skulls_and_filter_by_them() {
    let h = harness().await;
    let host = h.create("Host").await;
    let plain = h.start_game(&host, two_player_game(&host)).await;
    let mut game = two_player_game(&host);
    game["skulls"] = json!([4, 6]);
    let both = h.start_game(&host, game).await;
    let mut game = two_player_game(&host);
    game["skulls"] = json!([6]);
    let blind = h.start_game(&host, game).await;
    let mut campaign = two_player_game(&host);
    for (k, v) in [
        ("map", json!("03a_oldmombasa")),
        ("mode", json!("campaign")),
        ("kind", json!("campaign")),
        ("team_play", json!(false)),
        ("skulls", json!([0, 6, 12])),
    ] {
        campaign[k] = v;
    }
    campaign.as_object_mut().unwrap().remove("variant");
    campaign.as_object_mut().unwrap().remove("score_limit");
    let mythic = h.start_game(&host, campaign).await;

    let ids = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["game_id"].as_str().unwrap().to_string())
            .collect()
    };
    let all = h.get("/v1/games").await.json();
    assert_eq!(
        ids(&all),
        [mythic.clone(), blind.clone(), both.clone(), plain.clone()]
    );
    assert_eq!(all["items"][0]["skulls"], json!([0, 6, 12]));
    assert_eq!(all["items"][2]["skulls"], json!([4, 6]));
    assert!(all["items"][3].get("skulls").is_none(), "none is left out");
    assert_eq!(
        h.get(&format!("/v1/games/{both}")).await.json()["skulls"],
        json!([4, 6])
    );
    let tag = "Host";
    assert_eq!(
        h.get(&format!("/v1/accounts/{tag}")).await.json()["recent_games"][0]["skulls"],
        json!([0, 6, 12])
    );
    assert_eq!(
        ids(&h.get("/v1/games?skulls=6").await.json()),
        [mythic.clone(), blind.clone(), both.clone()]
    );
    assert_eq!(
        ids(&h.get("/v1/games?skulls=6,4").await.json()),
        std::slice::from_ref(&both)
    );
    assert_eq!(
        ids(&h.get("/v1/games?skulls=12&map=03a_oldmombasa").await.json()),
        std::slice::from_ref(&mythic)
    );
    assert!(ids(&h.get("/v1/games?skulls=14").await.json()).is_empty());
    for bad in ["64", "x", "-1"] {
        assert_eq!(
            h.get(&format!("/v1/games?skulls={bad}")).await.status,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    // A multiplayer game naming a campaign-only skull is refused.
    let mut bad = two_player_game(&host);
    bad["skulls"] = json!([12]);
    let r = h
        .call(
            Method::POST,
            "/v1/games",
            &[h.bearer(&host, lxm::GAMES_WRITE)],
            bad.to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn game_writes_are_checked() {
    let h = harness().await;
    let host = h.create("Host").await;
    let stranger = h.create("Stranger").await;
    let player = h.create("Player").await;
    let mut game = two_player_game(&host);
    game["players"][1]["xuid"] = json!(player.xuid);
    let id = h.start_game(&host, game).await;

    // Not in the game.
    let r = h
        .post_batch(&stranger, &id, h.batch(&stranger, &id, &[kill(1, 1, 0)]))
        .await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    // A player of the game may write.
    assert_eq!(
        h.post_batch(&player, &id, h.batch(&player, &id, &[kill(1, 0, 1)]))
            .await
            .status,
        StatusCode::CREATED
    );

    // Bad data, server-only types, other streams, GameEnded not last.
    for events in [
        vec![(
            "PlayerKilled",
            json!({"tick": 2, "victim": 9, "kind": "kill"}),
        )],
        vec![(
            "PlayerKilled",
            json!({"tick": 2, "victim": 1, "kind": "murder"}),
        )],
        vec![("GameEnded", json!({"tick": 2, "reason": "bored"}))],
        vec![
            ("GameEnded", json!({"tick": 2, "reason": "quit"})),
            kill(3, 1, 0),
        ],
        vec![
            kill(2, 1, 0),
            ("PlayerLeft", json!({"index": 1})),
            kill(3, 1, 0),
        ],
    ] {
        let r = h.post_batch(&host, &id, h.batch(&host, &id, &events)).await;
        assert_eq!(
            r.status,
            StatusCode::BAD_REQUEST,
            "{events:?}: {}",
            String::from_utf8_lossy(&r.body)
        );
    }
    for events in [
        vec![("GameFrame", json!({}))],
        vec![("GameStarted", json!({}))],
    ] {
        let r = h.post_batch(&host, &id, h.batch(&host, &id, &events)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    let elsewhere = live_proto::sign::sign_batch(
        &host.key,
        &[live_proto::sign::EventBody::new(
            &host.key,
            &host.author,
            "game-other",
            "PlayerKilled",
            json!({"tick": 1, "victim": 1, "kind": "world"}),
            time::OffsetDateTime::now_utc(),
        )],
    );
    assert_eq!(
        h.post_batch(&host, &id, elsewhere).await.status,
        StatusCode::FORBIDDEN
    );
    // Nothing of the refused batches landed.
    assert_eq!(
        h.get(&format!("/v1/events?stream=game-{id}")).await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Tokens: none, the wrong method.
    let jws = h.batch(&host, &id, &[kill(4, 1, 0)]);
    let uri = format!("/v1/games/{id}/events");
    assert_eq!(
        h.call(Method::POST, &uri, &[], jws.clone()).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.call(
            Method::POST,
            &uri,
            &[h.bearer(&host, lxm::EVENTS_WRITE)],
            jws
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    // Bad games.
    let r = h
        .call(
            Method::POST,
            "/v1/games",
            &[h.bearer(&host, lxm::GAMES_WRITE)],
            json!({"map": "a b", "mode": "multiplayer", "team_play": false, "players": []})
                .to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        h.post_batch(&host, "nope", h.batch(&host, "nope", &[kill(1, 1, 0)]))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn map_outlines() {
    let h = harness().await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;
    let outline = json!({"map": "lockout", "bounds": {"min": [0, 0, 0], "max": [10, 10, 5]},
        "layers": [{"z": [0, 2], "polygons": [[[0, 0], [10, 0], [10, 10]]]}]});
    assert_eq!(
        h.get("/v1/maps/lockout/outline").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.put_outline(&a, "lockout", &outline.clone()).await.status,
        StatusCode::OK
    );
    assert_eq!(h.get("/v1/maps/lockout/outline").await.json(), outline);
    // The first upload wins.
    assert_eq!(
        h.put_outline(&b, "lockout", &outline.clone()).await.status,
        StatusCode::CONFLICT
    );
    // The path and body must agree, and it must be an outline.
    let mut other = outline.clone();
    other["map"] = json!("midship");
    assert_eq!(
        h.put_outline(&a, "lockout", &other).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.put_outline(&a, "lockout", &json!({"map": "lockout"}))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    // The operator may replace it.
    let operator = Author::Server(h.id.clone()).to_string();
    let auth = format!(
        "Bearer {}",
        h.token(&h.operator, &operator, lxm::GAMES_WRITE)
    );
    let mut fixed = outline.clone();
    fixed["bounds"]["max"] = json!([20, 20, 5]);
    let r = h
        .call(
            Method::PUT,
            "/v1/maps/lockout/outline",
            &[("authorization", auth)],
            fixed.to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(h.get("/v1/maps/lockout/outline").await.json(), fixed);
}

#[tokio::test]
async fn invites_and_pings() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            invite_only: true,
            ..Policy::default()
        },
    )
    .await;
    let operator = Author::Server(h.id.clone()).to_string();
    let auth = format!(
        "Bearer {}",
        h.token(&h.operator, &operator, lxm::INVITES_WRITE)
    );
    let r = h
        .call(
            Method::POST,
            "/v1/invites",
            &[("authorization", auth)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let invite = r.json()["invite"].as_str().unwrap().to_string();
    let signup = AccountRequest::sign(
        &KeyPair::generate(),
        "Pete",
        Some(invite),
        "live.test",
        now(),
    );
    let r = h.call(Method::POST, "/v1/accounts", &[], signup).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let pete = User {
        key: KeyPair::generate(),
        xuid: live_proto::Xuid::parse(r.json()["xuid"].as_str().unwrap()).unwrap(),
        author: r.json()["id"].as_str().unwrap().into(),
    };
    // (A fresh key is not Pete's: the token fails.)
    assert_eq!(h.ping(&pete).await.status, StatusCode::UNAUTHORIZED);

    let h = harness().await;
    let pete = h.create("Pete").await;
    let auth = h.bearer(&pete, lxm::INVITES_WRITE);
    assert_eq!(
        h.call(Method::POST, "/v1/invites", &[auth], Body::empty())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(h.ping(&pete).await.status, StatusCode::NOT_FOUND);
    assert_eq!(h.sign_in(&pete).await.status, StatusCode::CREATED);
    let r = h.ping(&pete).await;
    assert_eq!(r.json(), json!({"ok": true, "expires_in": 90}));
}

/// A Durable Object's server runs nothing on a timer: it says when the
/// sweep is due, and sweeps when called.
#[tokio::test]
async fn a_driven_server_sweeps_when_called_at_the_time_it_gives() {
    let policy = Policy {
        presence_ttl_s: 1,
        game_idle_s: 1,
        sweep_every: Duration::from_millis(50),
        ..Policy::default()
    };
    let h = start_driven(Arc::new(MemoryStore::new()), policy).await;
    assert_eq!(h.live.sweep_due(), None, "nothing to watch");
    assert!(!h.live.packed_active());
    let pete = h.create("Pete").await;
    assert_eq!(h.sign_in(&pete).await.status, StatusCode::CREATED);
    let due = h.live.sweep_due().expect("presence to expire");
    assert!(due > now() && due <= now() + 3, "{due} vs {}", now());
    let id = h.start_game(&pete, two_player_game(&pete)).await;
    let r = h
        .post_batch(&pete, &id, h.batch(&pete, &id, &[score(60, 1, 0)]))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);

    // Past the deadlines, nothing happens on its own.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(h.live.sweep_due().unwrap() <= now());
    assert_eq!(
        h.get("/v1/presence").await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // The host's alarm calls the sweep.
    h.live.sweep().await;
    assert_eq!(h.get("/v1/presence").await.json()["items"], json!([]));
    let g = h.get(&format!("/v1/games/{id}")).await.json();
    assert_eq!(
        (g["state"].as_str(), g["end_reason"].as_str()),
        (Some("ended"), Some("lost"))
    );
    assert_eq!(h.live.sweep_due(), None, "nothing left to watch");
    assert!(!h.live.packed_active());
}

#[tokio::test]
async fn quiet_presence_expires_and_idle_games_are_lost() {
    let policy = Policy {
        presence_ttl_s: 1,
        game_idle_s: 1,
        sweep_every: Duration::from_millis(100),
        ..Policy::default()
    };
    let h = start(Arc::new(MemoryStore::new()), policy).await;
    let pete = h.create("Pete").await;
    assert_eq!(h.sign_in(&pete).await.status, StatusCode::CREATED);
    let id = h.start_game(&pete, two_player_game(&pete)).await;
    let r = h
        .post_batch(&pete, &id, h.batch(&pete, &id, &[score(60, 1, 0)]))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);

    // Pings keep presence alive past the TTL.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(h.ping(&pete).await.status, StatusCode::OK);
    }
    assert_eq!(
        h.get("/v1/presence").await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(h.get("/v1/presence").await.json()["items"], json!([]));
    let expired = h
        .get(&format!(
            "/v1/events?stream={}",
            streams::presence(pete.xuid)
        ))
        .await
        .json();
    let last = expired["items"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (last["type"].as_str(), last["author"].as_str()),
        (Some(types::PRESENCE_EXPIRED), Some("server:live.test"))
    );
    // Exactly one, not one per sweep.
    assert_eq!(expired["items"].as_array().unwrap().len(), 2);

    let g = h.get(&format!("/v1/games/{id}")).await.json();
    assert_eq!(
        (g["state"].as_str(), g["end_reason"].as_str()),
        (Some("ended"), Some("lost"))
    );
    assert_eq!(g["last_tick"], 60);
    assert_eq!(g["scoreboard"][0]["score"], 1);
}

#[tokio::test]
async fn the_web_app_is_served_with_a_spa_fallback() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>h2 live</title>",
    )
    .unwrap();
    std::fs::write(dir.join("assets/app.js"), "console.log(1)").unwrap();
    let h = harness().await;
    let app = http::router_with_web(h.live.clone(), Some(&dir));
    let get = |uri: &str| {
        let req = axum::http::Request::get(uri).body(Body::empty()).unwrap();
        let app = app.clone();
        async move {
            use http_body_util::BodyExt;
            use tower::ServiceExt;
            let r = app.oneshot(req).await.unwrap();
            let status = r.status();
            (
                status,
                String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec())
                    .unwrap(),
            )
        }
    };
    assert_eq!(
        get("/").await,
        (
            StatusCode::OK,
            "<!doctype html><title>h2 live</title>".into()
        )
    );
    assert_eq!(
        get("/games/abc").await.1,
        "<!doctype html><title>h2 live</title>"
    );
    assert_eq!(get("/assets/app.js").await.1, "console.log(1)");
    let (status, body) = get("/v1/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("not_found"));
    assert_eq!(get("/healthz").await.0, StatusCode::OK);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_embedded_server_falls_back_to_its_file_store_then_memory() {
    use std::io::{Read, Write};
    let dir = temp_dir();
    let start = || {
        live_server::start_embedded(
            "postgres://127.0.0.1:1/h2live",
            "127.0.0.1:0".parse().unwrap(),
            &dir,
            Default::default(),
        )
        .unwrap()
    };
    // No Postgres: the log goes to the data directory, starting with the
    // default message of the day.
    let file = start();
    assert_eq!(file.store, "file");
    assert!(
        live_server::file_store_dir(&dir)
            .join("events.jsonl")
            .is_file()
    );
    assert_eq!(file.live.position(), 1);
    // A second server cannot share that directory: it runs in memory.
    let server = start();
    assert_eq!(server.store, "memory");
    drop(file);
    let addr = server.url.strip_prefix("http://").unwrap().to_string();
    let mut s = std::net::TcpStream::connect(&addr).unwrap();
    s.write_all(b"GET /healthz HTTP/1.0\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).unwrap();
    assert!(
        reply.starts_with("HTTP/1.0 200") || reply.starts_with("HTTP/1.1 200"),
        "{reply}"
    );
    assert!(reply.ends_with(r#"{"ok":true,"position":0}"#), "{reply}");
    drop(server);
    // Stopped: the port is closed (or refuses soon after).
    std::thread::sleep(Duration::from_millis(200));
    assert!(std::net::TcpStream::connect(&addr).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Deadlines and games come back from the log: a restart over the same
/// store expires presence nobody pings and loses games nobody plays.
#[tokio::test]
async fn managers_rebuild_from_the_log() {
    let store: Arc<MemoryStore> = Arc::new(MemoryStore::new());
    let id = {
        let h = start(store.clone(), Policy::default()).await;
        let pete = h.create("Pete").await;
        assert_eq!(h.sign_in(&pete).await.status, StatusCode::CREATED);
        h.start_game(&pete, two_player_game(&pete)).await
    };
    let policy = Policy {
        presence_ttl_s: 1,
        game_idle_s: 1,
        sweep_every: Duration::from_millis(100),
        ..Policy::default()
    };
    let h = start(store, policy).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(h.get("/v1/presence").await.json()["items"], json!([]));
    assert_eq!(
        h.get(&format!("/v1/games/{id}")).await.json()["end_reason"],
        "lost"
    );
    assert_eq!(h.get("/v1/accounts/Pete").await.json()["online"], false);
}

/// Games, outlines and a restart against Postgres (`H2_LIVE_TEST_DB`).
#[tokio::test]
async fn postgres_games() {
    let Some(url) = std::env::var("H2_LIVE_TEST_DB")
        .ok()
        .filter(|s| !s.is_empty())
    else {
        eprintln!("skipping: H2_LIVE_TEST_DB is not set");
        return;
    };
    let store = Arc::new(live_server::pg::PostgresStore::connect(&url).await.unwrap());
    let tag = format!(
        "G{}",
        live_proto::sign::hex(&live_proto::sign::random_id().as_bytes()[..6])
    );
    let map = tag.to_lowercase();
    let outline = json!({"map": map, "bounds": {"min": [0, 0, 0], "max": [1, 1, 1]}, "layers": []});
    let id = {
        let h = start(store.clone(), Policy::default()).await;
        let host = h.create(&tag).await;
        let id = h.start_game(&host, two_player_game(&host)).await;
        let r = h
            .post_batch(
                &host,
                &id,
                h.batch(&host, &id, &[kill(10, 1, 0), score(10, 1, 0)]),
            )
            .await;
        assert_eq!(
            r.status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&r.body)
        );
        assert_eq!(
            h.put_outline(&host, &map, &outline).await.status,
            StatusCode::OK
        );
        assert_eq!(
            h.put_outline(&host, &map, &outline).await.status,
            StatusCode::CONFLICT
        );
        id
    };
    let h = start(store, Policy::default()).await;
    let g = h.get(&format!("/v1/games/{id}")).await.json();
    assert_eq!(
        (g["state"].as_str(), g["kills"][0]["pos"].clone()),
        (Some("live"), json!([1.5, -2.0, 3.25]))
    );
    assert_eq!(g["scoreboard"][0]["score"], 1);
    assert_eq!(
        h.get(&format!("/v1/maps/{map}/outline")).await.json(),
        outline
    );
    assert_eq!(
        h.get(&format!("/v1/accounts/{tag}")).await.json()["recent_games"][0]["game_id"],
        id.as_str()
    );
}

fn http_get(addr: &str, path: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.0\r\nHost: x\r\n\r\n").unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).unwrap();
    reply
}

/// Game art: served from its directory to this machine, refused (as a 404,
/// so the web app falls back) to anyone else unless shared, and a 404 when
/// there is none.
#[tokio::test]
async fn game_art_is_loopback_only_unless_shared() {
    use axum::extract::ConnectInfo;
    use live_server::art::GameArt;
    use live_server::http::Site;
    use tower::ServiceExt;

    let art = temp_dir();
    std::fs::create_dir_all(art.join("rank")).unwrap();
    std::fs::write(art.join("manifest.json"), r#"{"version":1}"#).unwrap();
    std::fs::write(art.join("rank/1.png"), b"png").unwrap();
    let h = harness().await;
    let get = |share: bool, peer: Option<&str>, path: &str| {
        let site = Site {
            web: None,
            game_art: GameArt::found(&art, share),
        };
        let app = http::router_with_site(h.live.clone(), &site);
        let mut req = axum::http::Request::get(path).body(Body::empty()).unwrap();
        if let Some(p) = peer {
            req.extensions_mut()
                .insert(ConnectInfo(p.parse::<std::net::SocketAddr>().unwrap()));
        }
        async move { app.oneshot(req).await.unwrap().status() }
    };
    assert_eq!(
        get(false, Some("127.0.0.1:9"), "/art/game/manifest.json").await,
        StatusCode::OK
    );
    assert_eq!(
        get(false, Some("127.0.0.1:9"), "/art/game/rank/1.png").await,
        StatusCode::OK
    );
    assert_eq!(
        get(false, Some("127.0.0.1:9"), "/art/game/rank/2.png").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(false, Some("10.0.0.7:9"), "/art/game/manifest.json").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(false, None, "/art/game/manifest.json").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(true, Some("10.0.0.7:9"), "/art/game/rank/1.png").await,
        StatusCode::OK
    );
    // No art at all.
    let r = h.get("/art/game/manifest.json").await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::NOT_FOUND, "not_found")
    );

    // A real socket carries the peer address through.
    let data = temp_dir();
    let site = Site {
        web: None,
        game_art: GameArt::found(&art, false),
    };
    let server = tokio::task::spawn_blocking(move || {
        live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &data,
            site,
        )
        .unwrap()
    })
    .await
    .unwrap();
    let addr = server.url.strip_prefix("http://").unwrap().to_string();
    let reply = tokio::task::spawn_blocking(move || http_get(&addr, "/art/game/manifest.json"))
        .await
        .unwrap();
    assert!(reply.ends_with(r#"{"version":1}"#), "{reply}");
    tokio::task::spawn_blocking(move || drop(server))
        .await
        .unwrap();
    std::fs::remove_dir_all(&art).unwrap();
}

#[tokio::test]
async fn players_join_themselves_and_only_the_host_ends_the_game() {
    let h = harness().await;
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    let friend = h.create("Friend").await;
    let game = json!({
        "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Slayer",
        "team_play": true, "tick": 0,
        "players": [
            {"index": 0, "name": "Host", "xuid": host.xuid, "team": 0, "local": true},
            {"index": 1, "name": "Guest", "xuid": guest.xuid, "team": 1}
        ]
    });
    // A record naming one account twice is refused.
    let mut twice = game.clone();
    twice["players"][1]["xuid"] = json!(host.xuid);
    let r = h
        .call(
            Method::POST,
            "/v1/games",
            &[h.bearer(&host, lxm::GAMES_WRITE)],
            twice.to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let id = h.start_game(&host, game).await;
    let join = |by: &User, index: u32, who: &User| {
        h.batch(
            by,
            &id,
            &[(
                "PlayerJoined",
                json!({"tick": 10, "player":
                    {"index": index, "name": "P", "xuid": who.xuid, "team": 0}}),
            )],
        )
    };
    // A guest cannot add anyone else.
    assert_eq!(
        h.post_batch(&guest, &id, join(&guest, 2, &friend))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // An account already in the game cannot join it again.
    assert_eq!(
        h.post_batch(&host, &id, join(&host, 2, &guest))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    // The host adds a player; one who left may join again as themselves.
    assert_eq!(
        h.post_batch(&host, &id, join(&host, 2, &friend))
            .await
            .status,
        StatusCode::CREATED
    );
    let left = h.batch(
        &host,
        &id,
        &[("PlayerLeft", json!({"tick": 20, "index": 2}))],
    );
    assert_eq!(
        h.post_batch(&host, &id, left).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        h.post_batch(&friend, &id, join(&friend, 3, &friend))
            .await
            .status,
        StatusCode::CREATED
    );

    // A guest cannot end the game (it leaves instead); the host does.
    let end = |by: &User| {
        h.batch(
            by,
            &id,
            &[(
                "GameEnded",
                json!({"tick": 900, "reason": "quit", "winner": {"team": 0}}),
            )],
        )
    };
    assert_eq!(
        h.post_batch(&guest, &id, end(&guest)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.post_batch(&host, &id, end(&host)).await.status,
        StatusCode::CREATED
    );
}
