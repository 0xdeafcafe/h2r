//! Co-op sessions in lists, presence activity, and the finished co-op games
//! derived from session events.
mod common;
use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use common::*;
use live_proto::{
    lxm,
    session::{self, Campaign, NewSession, Slots, Update},
    sign::{EventBody, sign_batch},
    streams, types,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn coop(visibility: session::Visibility) -> NewSession {
    NewSession {
        addr: Some("192.168.2.4".parse().unwrap()),
        port: 3074,
        host_address: vec![0; 36],
        slots: Slots {
            public_filled: 1,
            public_open: 1,
            ..Default::default()
        },
        attributes: BTreeMap::new(),
        flags: session::COOP.into(),
        hopper_id: None,
        map: None,
        name: None,
        language: None,
        visibility,
        map_id: None,
        skulls: None,
        campaign: Some(Campaign {
            level: "03a_oldmombasa".into(),
            name: Some("Outskirts".into()),
            difficulty: 2,
        }),
        ping_ms: None,
        variant_ext: None,
    }
}

async fn create(h: &Harness, u: &User, data: &NewSession) -> Resp {
    h.call(
        Method::POST,
        "/v1/sessions",
        &[
            (
                "authorization",
                format!("Bearer {}", h.token(&u.key, &u.author, lxm::SESSIONS_WRITE)),
            ),
            ("content-type", "application/json".into()),
        ],
        Body::from(serde_json::to_vec(data).unwrap()),
    )
    .await
}

async fn send(h: &Harness, u: &User, id: &str, events: &[(&str, Value)]) -> Resp {
    let bodies: Vec<_> = events
        .iter()
        .map(|(k, d)| {
            EventBody::at_unix(&u.key, &u.author, &session::stream(id), k, d.clone(), now())
        })
        .collect();
    h.call(
        Method::POST,
        &format!("/v1/sessions/{id}/events"),
        &[
            (
                "authorization",
                format!("Bearer {}", h.token(&u.key, &u.author, lxm::SESSIONS_WRITE)),
            ),
            ("content-type", "application/jose".into()),
        ],
        Body::from(sign_batch(&u.key, &bodies)),
    )
    .await
}

async fn presence_event(h: &Harness, u: &User, kind: &str, data: Value) -> Resp {
    let jws = h.event(u, &streams::presence(u.xuid), kind, data);
    h.post_event(u, jws).await
}

#[tokio::test]
async fn coop_sessions_round_trip_and_filter() {
    let h = harness().await;
    let a = h.create("Host").await;
    let r = create(&h, &a, &coop(Default::default())).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let id = r.json()["session_id"].as_str().unwrap().to_string();
    // A plain custom game next to it.
    let mut custom = coop(Default::default());
    custom.flags = "custom".into();
    custom.campaign = None;
    assert_eq!(create(&h, &a, &custom).await.status, StatusCode::CREATED);
    // Only a co-op session can carry a campaign.
    let mut bad = coop(Default::default());
    bad.flags = "custom".into();
    assert_eq!(create(&h, &a, &bad).await.status, StatusCode::BAD_REQUEST);

    let all = h.get("/v1/sessions").await.json();
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
    let only = h.get("/v1/sessions?flags=coop").await.json();
    let items = only["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["session_id"], id);
    assert_eq!(items[0]["advertisement"]["flags"], "coop");
    assert_eq!(items[0]["advertisement"]["campaign"]["name"], "Outskirts");
    assert_eq!(items[0]["advertisement"]["campaign"]["difficulty"], 2);
    let one = h.get(&format!("/v1/sessions/{id}")).await.json();
    assert_eq!(
        one["session"]["advertisement"]["campaign"]["level"],
        "03a_oldmombasa"
    );
    assert_eq!(
        h.get("/v1/sessions?flags=ranked").await.json()["items"],
        json!([])
    );

    // The host changes the difficulty with a signed update.
    let up = serde_json::to_value(Update {
        campaign: Some(Campaign {
            level: "03a_oldmombasa".into(),
            name: Some("Outskirts".into()),
            difficulty: 3,
        }),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        send(&h, &a, &id, &[(session::UPDATED, up)]).await.status,
        StatusCode::CREATED
    );
    let one = h.get(&format!("/v1/sessions/{id}")).await.json();
    assert_eq!(one["session"]["advertisement"]["campaign"]["difficulty"], 3);
    assert_eq!(one["session"]["advertisement"]["flags"], "coop");
}

#[tokio::test]
async fn friends_only_coop_sessions_stay_hidden_from_strangers() {
    let h = harness().await;
    let a = h.create("Host").await;
    let b = h.create("Friend").await;
    let c = h.create("Stranger").await;
    let ask = h.event(
        &a,
        &streams::account(a.xuid),
        types::FRIEND_REQUESTED,
        json!({"from": a.xuid, "to": b.xuid}),
    );
    h.post_event(&a, ask).await;
    let yes = h.event(
        &b,
        &streams::account(b.xuid),
        types::FRIEND_ACCEPTED,
        json!({"from": a.xuid, "to": b.xuid}),
    );
    h.post_event(&b, yes).await;
    let r = create(&h, &a, &coop(session::Visibility::Friends)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let count = |u: Option<&User>| {
        let headers: Vec<_> = u.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
        let h = &h;
        async move {
            h.call(
                Method::GET,
                "/v1/sessions?flags=coop",
                &headers,
                Body::empty(),
            )
            .await
            .json()["items"]
                .as_array()
                .unwrap()
                .len()
        }
    };
    assert_eq!(count(None).await, 0);
    assert_eq!(count(Some(&c)).await, 0);
    assert_eq!(count(Some(&b)).await, 1);
    assert_eq!(count(Some(&a)).await, 1);
}

#[tokio::test]
async fn presence_activity_is_validated_served_and_cleared() {
    let h = harness().await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;
    let ask = h.event(
        &b,
        &streams::account(b.xuid),
        types::FRIEND_REQUESTED,
        json!({"from": b.xuid, "to": a.xuid}),
    );
    h.post_event(&b, ask).await;
    let yes = h.event(
        &a,
        &streams::account(a.xuid),
        types::FRIEND_ACCEPTED,
        json!({"from": b.xuid, "to": a.xuid}),
    );
    h.post_event(&a, yes).await;

    let bad = |activity: &str| json!({"xuid": a.xuid, "state": "online", "activity": activity});
    for activity in ["", "   ", "tab\there", &"x".repeat(65)] {
        let r = presence_event(&h, &a, types::SIGNED_IN, bad(activity)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{activity:?}");
    }
    let r = presence_event(&h, &a, types::SIGNED_IN, bad("Co-op: Outskirts")).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let list = h.get("/v1/presence").await.json();
    assert_eq!(list["items"][0]["activity"], "Co-op: Outskirts");
    let friends = h.get("/v1/accounts/Bravo/friends").await.json();
    assert_eq!(
        friends["friends"][0]["presence"]["activity"],
        "Co-op: Outskirts"
    );

    // A change without activity clears it.
    let r = presence_event(
        &h,
        &a,
        types::PRESENCE_CHANGED,
        json!({"xuid": a.xuid, "state": "online"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let list = h.get("/v1/presence").await.json();
    assert!(list["items"][0].get("activity").is_none());
    let friends = h.get("/v1/accounts/Bravo/friends").await.json();
    assert!(friends["friends"][0]["presence"].get("activity").is_none());
}

#[tokio::test]
async fn finished_coop_games_are_derived_from_session_events() {
    let h = harness().await;
    let a = h.create("Host").await;
    let b = h.create("Guest").await;
    let c = h.create("Third").await;
    assert_eq!(h.get("/v1/coop/games").await.json()["items"], json!([]));

    // Game one: the guest joins, leaves, and the host closes. The guest
    // still counts.
    let id1 = create(&h, &a, &coop(Default::default())).await.json()["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    send(&h, &a, &id1, &[(session::JOINED, json!({"xuid": b.xuid}))]).await;
    send(&h, &a, &id1, &[(session::LEFT, json!({"xuid": b.xuid}))]).await;
    // Still open: not finished.
    assert_eq!(h.get("/v1/coop/games").await.json()["items"], json!([]));
    send(
        &h,
        &a,
        &id1,
        &[(session::DELETED, json!({"reason": "closed"}))],
    )
    .await;

    // A co-op session nobody joined is not a game; neither is a custom one
    // with a guest.
    let empty = create(&h, &a, &coop(Default::default())).await.json()["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    send(
        &h,
        &a,
        &empty,
        &[(session::DELETED, json!({"reason": "closed"}))],
    )
    .await;
    let mut custom = coop(Default::default());
    custom.flags = "custom".into();
    custom.campaign = None;
    let cu = create(&h, &a, &custom).await.json()["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    send(&h, &a, &cu, &[(session::JOINED, json!({"xuid": b.xuid}))]).await;
    send(
        &h,
        &a,
        &cu,
        &[(session::DELETED, json!({"reason": "closed"}))],
    )
    .await;

    // Game two: two guests, host_left.
    let id2 = create(&h, &b, &coop(Default::default())).await.json()["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    send(&h, &b, &id2, &[(session::JOINED, json!({"xuid": c.xuid}))]).await;
    send(&h, &b, &id2, &[(session::JOINED, json!({"xuid": a.xuid}))]).await;
    send(
        &h,
        &b,
        &id2,
        &[(session::DELETED, json!({"reason": "host_left"}))],
    )
    .await;

    let page = h.get("/v1/coop/games").await.json();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{page}");
    assert_eq!(items[0]["session_id"], id2, "newest first");
    assert_eq!(items[0]["end_reason"], "host_left");
    let tags: Vec<_> = items[0]["players"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["gamertag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["Guest", "Third", "Host"]);
    assert_eq!(items[1]["session_id"], id1);
    assert_eq!(items[1]["host"]["gamertag"], "Host");
    assert_eq!(items[1]["players"][1]["xuid"], b.xuid.to_string());
    assert_eq!(items[1]["campaign"]["level"], "03a_oldmombasa");
    assert_eq!(items[1]["campaign"]["difficulty"], 2);
    assert_eq!(items[1]["end_reason"], "closed");
    assert!(items[1]["started"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(items[1]["ended"].as_str().is_some_and(|s| !s.is_empty()));

    // Paging by the cursor.
    let p1 = h.get("/v1/coop/games?limit=1").await.json();
    assert_eq!(p1["items"][0]["session_id"], id2);
    let cursor = p1["next"].as_str().unwrap();
    let p2 = h
        .get(&format!("/v1/coop/games?limit=1&cursor={cursor}"))
        .await
        .json();
    assert_eq!(p2["items"][0]["session_id"], id1);

    // Player filter by gamertag and by XUID.
    let only_c = h.get("/v1/coop/games?player=third").await.json();
    assert_eq!(only_c["items"].as_array().unwrap().len(), 1);
    let only_c = h
        .get(&format!("/v1/coop/games?player={}", c.xuid))
        .await
        .json();
    assert_eq!(only_c["items"][0]["session_id"], id2);
    assert_eq!(
        h.get("/v1/coop/games?player=nobody").await.json()["items"],
        json!([])
    );
    assert_eq!(
        h.get("/v1/coop/games?cursor=x").await.status,
        StatusCode::BAD_REQUEST
    );
}

/// What each viewer sees of one co-op game, endpoint by endpoint.
#[derive(Debug, PartialEq)]
struct Seen {
    /// `GET /v1/coop/games`: how many finished co-op games.
    coop: usize,
    /// `GET /v1/coop/games?player=Guest`.
    coop_player: usize,
    /// `GET /v1/games`: how many games.
    games: usize,
    /// `GET /v1/games/{id}` answers.
    detail: bool,
    /// `GET /v1/games/{id}/frames` answers.
    frames: bool,
    /// `GET /v1/accounts/Host`: its recent games.
    recent: usize,
    /// `GET /v1/events` (read a few at a time) carries the game's events.
    log_game: bool,
    /// ... and the session's.
    log_session: bool,
    /// `/v1/stream` from the start carries the game's events.
    stream_game: bool,
    stream_session: bool,
    /// `GET /v1/stats/summary?window=all`: games counted.
    stats: u64,
}

const ALL: Seen = Seen {
    coop: 1,
    coop_player: 1,
    games: 1,
    detail: true,
    frames: true,
    recent: 1,
    log_game: true,
    log_session: true,
    stream_game: true,
    stream_session: true,
    stats: 1,
};
const NONE: Seen = Seen {
    coop: 0,
    coop_player: 0,
    games: 0,
    detail: false,
    frames: false,
    recent: 0,
    log_game: false,
    log_session: false,
    stream_game: false,
    stream_session: false,
    stats: 0,
};

/// Every stream named in `GET /v1/events`, read three at a time: hidden
/// events leave pages short (or empty), but the cursor runs on to the head.
async fn log_streams(h: &Harness, u: Option<&User>) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0u64;
    let head = h.live.position();
    for _ in 0..1000 {
        let headers: Vec<_> = u.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
        let r = h
            .call(
                Method::GET,
                &format!("/v1/events?limit=3&cursor={cursor}"),
                &headers,
                Body::empty(),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        let page = r.json();
        for e in page["items"].as_array().unwrap() {
            assert!(e["position"].as_u64().unwrap() > cursor);
            out.push(e["stream"].as_str().unwrap().to_string());
        }
        match page["next"].as_str() {
            Some(n) => cursor = n.parse().unwrap(),
            None => return out,
        }
    }
    panic!("the cursor never reached the head {head}");
}

/// Every stream named in `/v1/stream` from the start up to `caught_up`,
/// signed in with `?auth=1` when `u` is set; and the caught-up position.
async fn stream_streams(
    addr: std::net::SocketAddr,
    h: &Harness,
    u: Option<&User>,
) -> (Vec<String>, u64) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let q = if u.is_some() {
        "?from=0&auth=1"
    } else {
        "?from=0"
    };
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/stream{q}"))
        .await
        .unwrap();
    if let Some(u) = u {
        let token = h.token(&u.key, &u.author, lxm::READ);
        ws.send(Message::Text(
            json!({"kind": "auth", "token": token}).to_string().into(),
        ))
        .await
        .unwrap();
    }
    let mut out = Vec::new();
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("a message in time")
            .unwrap()
            .unwrap();
        let Message::Text(t) = msg else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["kind"].as_str() {
            Some("event") => out.push(v["event"]["stream"].as_str().unwrap().to_string()),
            Some("caught_up") => return (out, v["position"].as_u64().unwrap()),
            _ => {}
        }
    }
}

/// A host and a guest play one Live co-op campaign game in a session of
/// `visibility`: the session closes after the guest joined (a finished co-op
/// game) and the host's `online` campaign game links to it. Returns what the
/// anonymous reader, a stranger, the host's friend, the guest and an admin
/// each see.
async fn coop_seen_by(visibility: session::Visibility) -> [(&'static str, Seen); 5] {
    let h = start(
        std::sync::Arc::new(live_server::store::MemoryStore::new()),
        live_server::Policy {
            admins: vec![live_proto::Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    let a = h.create("Host").await;
    let b = h.create("Guest").await;
    let f = h.create("Friend").await;
    let c = h.create("Stranger").await;
    let boss = h.create("Boss").await;
    let ask = h.event(
        &a,
        &streams::account(a.xuid),
        types::FRIEND_REQUESTED,
        json!({"from": a.xuid, "to": f.xuid}),
    );
    h.post_event(&a, ask).await;
    let yes = h.event(
        &f,
        &streams::account(f.xuid),
        types::FRIEND_ACCEPTED,
        json!({"from": a.xuid, "to": f.xuid}),
    );
    h.post_event(&f, yes).await;

    let r = create(&h, &a, &coop(visibility)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let sid = r.json()["session_id"].as_str().unwrap().to_string();
    send(&h, &a, &sid, &[(session::JOINED, json!({"xuid": b.xuid}))]).await;
    let game = h
        .start_game(
            &a,
            json!({
                "map": "03a_oldmombasa", "mode": "campaign", "kind": "campaign",
                "difficulty": "heroic", "online": true, "team_play": false,
                "players": [
                    {"index": 0, "name": "Host", "xuid": a.xuid, "team": 0, "local": true},
                    {"index": 1, "name": "Guest", "xuid": b.xuid, "team": 0, "local": false}
                ]
            }),
        )
        .await;
    h.live.ingest_frame(&live_proto::Author::parse(&a.author).unwrap(), live_proto::game::Frame {
        game_id: game.clone(), tick: 2, ..Default::default()
    }).await.unwrap();
    let end = h.batch(
        &a,
        &game,
        &[("GameEnded", json!({"tick": 2, "reason": "finished"}))],
    );
    assert_eq!(
        h.post_batch(&a, &game, end).await.status,
        StatusCode::CREATED
    );
    send(
        &h,
        &a,
        &sid,
        &[(session::DELETED, json!({"reason": "closed"}))],
    )
    .await;

    let read = |u: Option<&User>, uri: String| {
        let headers: Vec<_> = u.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
        let h = &h;
        async move { h.call(Method::GET, &uri, &headers, Body::empty()).await }
    };
    let operator = |h: &Harness| {
        (
            "authorization",
            format!(
                "Bearer {}",
                h.token(&h.operator, "server:live.test", lxm::READ)
            ),
        )
    };
    let len = |r: Resp, key: &str| {
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        r.json()[key].as_array().unwrap().len()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    h.live.sweep().await;
    let archive_index = format!("/v1/games/{game}/chunks?format=archive");
    let root = read(Some(&a), archive_index.clone()).await.json();
    let archive_segment = format!("/v1/games/{game}/archive/{}/0", root["archive_sha256"].as_str().unwrap());
    let game_stream = format!("game-{game}");
    let session_stream = format!("session-{sid}");
    let mut out = Vec::new();
    for (name, u) in [
        ("anonymous", None),
        ("stranger", Some(&c)),
        ("friend", Some(&f)),
        ("guest", Some(&b)),
        ("admin", Some(&boss)),
    ] {
        let detail = read(u, format!("/v1/games/{game}")).await.status;
        assert!(matches!(detail, StatusCode::OK | StatusCode::NOT_FOUND));
        let frames = read(u, format!("/v1/games/{game}/frames")).await.status;
        // Chunks follow the game's visibility as its frames do.
        let chunks = read(u, format!("/v1/games/{game}/chunks")).await.status;
        assert_eq!(chunks, frames, "{name}");
        assert_eq!(read(u, archive_index.clone()).await.status, detail, "{name} archive index");
        let segment = read(u, archive_segment.clone()).await;
        assert_eq!(segment.status, detail, "{name} immutable segment");
        if u.is_some() || segment.status == StatusCode::NOT_FOUND {
            assert_eq!(segment.headers["cache-control"], "private, no-store", "{name}");
        } else {
            assert!(segment.headers["cache-control"].to_str().unwrap().contains("immutable"));
        }
        let log = log_streams(&h, u).await;
        let (stream, caught_up) = stream_streams(addr, &h, u).await;
        // Hidden events or not, the reader catches up to the head.
        assert_eq!(caught_up, h.live.position(), "{name}");
        let stats = read(u, "/v1/stats/summary?window=all".into()).await.json()["totals"]["games"]
            .as_u64()
            .unwrap();
        out.push((
            name,
            Seen {
                coop: len(read(u, "/v1/coop/games".into()).await, "items"),
                coop_player: len(read(u, "/v1/coop/games?player=Guest".into()).await, "items"),
                games: len(read(u, "/v1/games".into()).await, "items"),
                detail: detail == StatusCode::OK,
                frames: frames == StatusCode::OK,
                recent: len(read(u, "/v1/accounts/Host".into()).await, "recent_games"),
                log_game: log.contains(&game_stream),
                log_session: log.contains(&session_stream),
                stream_game: stream.contains(&game_stream),
                stream_session: stream.contains(&session_stream),
                stats,
            },
        ));
    }
    // The operator's own token is an admin's too.
    let op = h
        .call(
            Method::GET,
            "/v1/coop/games",
            &[operator(&h)],
            Body::empty(),
        )
        .await;
    assert_eq!(len(op, "items"), 1);
    // The host always sees its own game.
    assert_eq!(len(read(Some(&a), "/v1/games".into()).await, "items"), 1);
    // The finished game says how it was listed.
    let all = h
        .call(
            Method::GET,
            "/v1/coop/games",
            &[operator(&h)],
            Body::empty(),
        )
        .await
        .json();
    assert_eq!(all["items"][0]["visibility"], visibility.as_str());
    out.try_into().unwrap()
}

#[tokio::test]
async fn public_coop_games_show_to_everyone() {
    for (who, seen) in coop_seen_by(session::Visibility::Public).await {
        assert_eq!(seen, ALL, "{who}");
    }
}

#[tokio::test]
async fn friends_only_coop_games_show_to_players_friends_and_admins() {
    let seen = coop_seen_by(session::Visibility::Friends).await;
    assert_eq!(
        seen,
        [
            ("anonymous", NONE),
            ("stranger", NONE),
            ("friend", Seen { stats: 0, ..ALL }),
            ("guest", Seen { stats: 0, ..ALL }),
            ("admin", ALL),
        ]
    );
}

#[tokio::test]
async fn invite_only_coop_games_show_to_players_and_admins() {
    let seen = coop_seen_by(session::Visibility::Invite).await;
    assert_eq!(
        seen,
        [
            ("anonymous", NONE),
            ("stranger", NONE),
            ("friend", NONE),
            ("guest", Seen { stats: 0, ..ALL }),
            ("admin", ALL),
        ]
    );
}

/// A campaign game played outside any co-op session (split-screen, or Live
/// co-op from a client that has no session) stays public.
#[tokio::test]
async fn campaign_games_without_a_session_stay_public() {
    let h = harness().await;
    let a = h.create("Solo").await;
    h.start_game(
        &a,
        json!({
            "map": "01a_tutorial", "mode": "campaign", "kind": "campaign",
            "online": true, "team_play": false,
            "players": [{"index": 0, "name": "Solo", "xuid": a.xuid, "team": 0, "local": true}]
        }),
    )
    .await;
    assert_eq!(
        h.get("/v1/games").await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// A running co-op game follows its session's visibility; once ended it
/// keeps the last one.
#[tokio::test]
async fn a_running_coop_game_follows_its_session_visibility() {
    let h = harness().await;
    let a = h.create("Host").await;
    let b = h.create("Guest").await;
    let c = h.create("Stranger").await;
    let sid = create(&h, &a, &coop(session::Visibility::Public))
        .await
        .json()["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    send(&h, &a, &sid, &[(session::JOINED, json!({"xuid": b.xuid}))]).await;
    let game = h
        .start_game(
            &a,
            json!({
                "map": "03a_oldmombasa", "mode": "campaign", "kind": "campaign",
                "online": true, "team_play": false,
                "players": [
                    {"index": 0, "name": "Host", "xuid": a.xuid, "team": 0, "local": true},
                    {"index": 1, "name": "Guest", "xuid": b.xuid, "team": 0, "local": false}
                ]
            }),
        )
        .await;
    let seen = |u: &User| {
        let headers = vec![h.bearer(u, lxm::READ)];
        let h = &h;
        let uri = format!("/v1/games/{game}");
        async move {
            h.call(Method::GET, &uri, &headers, Body::empty())
                .await
                .status
                == StatusCode::OK
        }
    };
    let set = |v: session::Visibility| {
        let up = serde_json::to_value(Update {
            visibility: Some(v),
            ..Default::default()
        })
        .unwrap();
        let (h, a, sid) = (&h, &a, sid.clone());
        async move {
            let r = send(h, a, &sid, &[(session::UPDATED, up)]).await;
            assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
        }
    };
    assert!(seen(&c).await);
    set(session::Visibility::Invite).await;
    assert!(!seen(&c).await, "hidden once the host made it invite-only");
    assert!(seen(&b).await);
    set(session::Visibility::Friends).await;
    // The game ends while friends-only; a later change does not move it.
    let end = h.batch(
        &a,
        &game,
        &[("GameEnded", json!({"tick": 2, "reason": "quit"}))],
    );
    assert_eq!(
        h.post_batch(&a, &game, end).await.status,
        StatusCode::CREATED
    );
    set(session::Visibility::Public).await;
    assert!(!seen(&c).await, "the ended game keeps friends-only");
    assert!(seen(&b).await);
    // Anonymous stats leave it out; the log hides its events from the stranger.
    assert_eq!(
        h.get("/v1/stats/summary?window=all").await.json()["totals"]["games"],
        0
    );
    let log = log_streams(&h, Some(&c)).await;
    assert!(!log.contains(&format!("game-{game}")));
}
