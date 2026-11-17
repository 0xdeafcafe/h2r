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
}

const ALL: Seen = Seen {
    coop: 1,
    coop_player: 1,
    games: 1,
    detail: true,
    frames: true,
    recent: 1,
};
const NONE: Seen = Seen {
    coop: 0,
    coop_player: 0,
    games: 0,
    detail: false,
    frames: false,
    recent: 0,
};

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
        out.push((
            name,
            Seen {
                coop: len(read(u, "/v1/coop/games".into()).await, "items"),
                coop_player: len(read(u, "/v1/coop/games?player=Guest".into()).await, "items"),
                games: len(read(u, "/v1/games".into()).await, "items"),
                detail: detail == StatusCode::OK,
                frames: frames == StatusCode::OK,
                recent: len(read(u, "/v1/accounts/Host".into()).await, "recent_games"),
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
            ("friend", ALL),
            ("guest", ALL),
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
            ("guest", ALL),
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
