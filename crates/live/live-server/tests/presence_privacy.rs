//! Presence for readers who may not see a player's game or session: a
//! friends-only or invite-only co-op game's id and activity show only to
//! those allowed to see it; everyone else sees the player in "In a game".
mod common;
use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use common::*;
use live_proto::{
    lxm,
    session::{self, Campaign, NewSession, Slots},
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
        browser: None,
    }
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
            h.bearer(u, lxm::SESSIONS_WRITE),
            ("content-type", "application/jose".into()),
        ],
        Body::from(sign_batch(&u.key, &bodies)),
    )
    .await
}

/// What a reader sees of the host's presence: (game id, activity).
type Seen = (Option<String>, Option<String>);

fn seen(v: &Value) -> Seen {
    (
        v["game_id"].as_str().map(str::to_string),
        v["activity"].as_str().map(str::to_string),
    )
}

/// The host plays a co-op game in a session of `visibility` and says so in
/// its presence. For each reader: its presence in `/v1/presence`, in its
/// friend's friends list, in the log's `PresenceChanged`, and on the stream.
async fn presence_seen_by(visibility: session::Visibility) -> Vec<(&'static str, [Seen; 4])> {
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
    let r = h
        .call(
            Method::POST,
            "/v1/sessions",
            &[
                h.bearer(&a, lxm::SESSIONS_WRITE),
                ("content-type", "application/json".into()),
            ],
            Body::from(serde_json::to_vec(&coop(visibility)).unwrap()),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let sid = r.json()["session_id"].as_str().unwrap().to_string();
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
    let jws = h.event(
        &a,
        &streams::presence(a.xuid),
        types::SIGNED_IN,
        json!({"xuid": a.xuid, "state": "playing", "game_id": game, "activity": "Co-op: Outskirts"}),
    );
    let r = h.post_event(&a, jws).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());

    let mut out = Vec::new();
    for (name, u) in [
        ("anonymous", None),
        ("stranger", Some(&c)),
        ("friend", Some(&f)),
        ("guest", Some(&b)),
        ("self", Some(&a)),
        ("admin", Some(&boss)),
    ] {
        let get = |uri: String| {
            let headers: Vec<_> = u.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
            let h = &h;
            async move {
                let r = h.call(Method::GET, &uri, &headers, Body::empty()).await;
                assert_eq!(r.status, StatusCode::OK, "{uri}: {}", r.error());
                r.json()
            }
        };
        let list = get("/v1/presence".into()).await;
        let mine = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["gamertag"] == "Host")
            .unwrap()
            .clone();
        // The page-load snapshot shows this reader what the lists do, and
        // only an anonymous one may be kept by a shared cache.
        let snap = get("/v1/snapshot".into()).await;
        assert_eq!(snap["presence"]["items"], list["items"], "{name}");
        for state in ["live", "ended"] {
            let games = get(format!("/v1/games?state={state}&limit=100")).await;
            assert_eq!(snap[state]["items"], games["items"], "{name} {state}");
        }
        let shown: Vec<_> = snap["details"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["game_id"].clone())
            .collect();
        let listed: Vec<_> = snap["live"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["game_id"].clone())
            .collect();
        assert_eq!(
            shown, listed,
            "{name}: a detail for each live game it may see"
        );
        let headers: Vec<_> = u.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
        for uri in ["/v1/snapshot", "/v1/presence", "/v1/games"] {
            let r = h.call(Method::GET, uri, &headers, Body::empty()).await;
            let cache = r.headers["cache-control"].to_str().unwrap();
            if u.is_some() {
                assert_eq!(cache, live_server::caching::PRIVATE, "{name} {uri}");
                assert!(!r.headers.contains_key("etag"), "{name} {uri}");
            } else {
                assert_eq!(cache, live_server::caching::PUBLIC, "{name} {uri}");
            }
        }
        let friends = get("/v1/accounts/Friend/friends".into()).await;
        let log = get(format!("/v1/events?stream=presence-{}", a.xuid)).await;
        let ev = log["items"].as_array().unwrap().last().unwrap().clone();
        let redacted = ev["data"]["redacted"] == true;
        // A redacted event withholds the signature over the real data.
        assert_eq!(ev["jws"] == "", redacted, "{name}");
        let streamed = stream_presence(addr, &h, u, a.xuid).await;
        assert_eq!(streamed["data"]["redacted"] == true, redacted, "{name}");
        out.push((
            name,
            [
                seen(&mine),
                seen(&friends["friends"][0]["presence"]),
                seen(&ev["data"]),
                seen(&streamed["data"]),
            ],
        ));
    }
    out
}

/// The last presence event of `xuid` on `/v1/stream` from the start.
async fn stream_presence(
    addr: std::net::SocketAddr,
    h: &Harness,
    u: Option<&User>,
    xuid: live_proto::Xuid,
) -> Value {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let auth = if u.is_some() { "&auth=1" } else { "" };
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/v1/stream?from=0&stream=presence-{xuid}{auth}"
    ))
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
    let mut last = Value::Null;
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("a message in time")
            .unwrap()
            .unwrap();
        let Message::Text(t) = msg else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["kind"].as_str() {
            Some("event") => last = v["event"].clone(),
            Some("caught_up") => return last,
            _ => {}
        }
    }
}

fn full() -> [Seen; 4] {
    let s: Seen = (None, Some("Co-op: Outskirts".into()));
    [s.clone(), s.clone(), s.clone(), s]
}

fn generic() -> [Seen; 4] {
    let s: Seen = (None, Some("In a game".into()));
    [s.clone(), s.clone(), s.clone(), s]
}

/// `full` with the game id filled in: the real one, as the admin sees it.
fn with_game(seen: &[(&'static str, [Seen; 4])]) -> impl Fn([Seen; 4]) -> [Seen; 4] + use<> {
    let game = seen.iter().find(|(n, _)| *n == "admin").unwrap().1[0]
        .0
        .clone();
    move |mut x| {
        for s in &mut x {
            if s.1.as_deref() != Some("In a game") {
                s.0 = game.clone();
            }
        }
        x
    }
}

#[tokio::test]
async fn public_coop_presence_shows_to_everyone() {
    let seen = presence_seen_by(session::Visibility::Public).await;
    let g = with_game(&seen);
    assert!(seen[0].1[0].0.is_some(), "the game id is there");
    for (who, s) in &seen {
        assert_eq!(*s, g(full()), "{who}");
    }
}

#[tokio::test]
async fn friends_only_coop_presence_shows_to_friends_players_and_admins() {
    let seen = presence_seen_by(session::Visibility::Friends).await;
    let g = with_game(&seen);
    let want = [
        ("anonymous", generic()),
        ("stranger", generic()),
        ("friend", g(full())),
        ("guest", g(full())),
        ("self", g(full())),
        ("admin", g(full())),
    ];
    assert_eq!(seen, want);
}

#[tokio::test]
async fn invite_only_coop_presence_shows_to_players_and_admins() {
    let seen = presence_seen_by(session::Visibility::Invite).await;
    let g = with_game(&seen);
    let want = [
        ("anonymous", generic()),
        ("stranger", generic()),
        ("friend", generic()),
        ("guest", g(full())),
        ("self", g(full())),
        ("admin", g(full())),
    ];
    assert_eq!(seen, want);
}

/// A console's build and capabilities ride on its presence and show to
/// every reader, friend or not, so a host can read its guests' before a
/// game; the server keeps their shape and refuses anything else.
#[tokio::test]
async fn build_and_capabilities_show_to_everyone_and_are_checked() {
    let h = start(
        std::sync::Arc::new(live_server::store::MemoryStore::new()),
        live_server::Policy::default(),
    )
    .await;
    let a = h.create("Host").await;
    let old = h.create("Oldie").await;
    let c = h.create("Stranger").await;
    let presence = |u: &User, extra: Value| {
        let mut d = json!({"xuid": u.xuid, "state": "online", "game_id": null});
        for (k, v) in extra.as_object().unwrap() {
            d[k] = v.clone();
        }
        h.event(u, &streams::presence(u.xuid), types::SIGNED_IN, d)
    };
    let r = h
        .post_event(
            &a,
            presence(
                &a,
                json!({"build": "a65fa01", "capabilities": [live_proto::capability::VARIANT_EXT]}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    // An older build sends neither.
    let r = h.post_event(&old, presence(&old, json!({}))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());

    for bad in [
        json!({"build": "A65FA01"}),
        json!({"build": "a65f"}),
        json!({"build": "unknown"}),
        json!({"capabilities": ["Variant-Ext/1"]}),
        json!({"capabilities": ["variant-ext"]}),
        json!({"capabilities": vec!["variant-ext/1"; 17]}),
        json!({"capabilities": [format!("{}/1", "a".repeat(40))]}),
    ] {
        let r = h.post_event(&c, presence(&c, bad.clone())).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }

    for u in [None, Some(&c), Some(&old)] {
        let headers: Vec<_> = u.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
        let r = h
            .call(Method::GET, "/v1/presence", &headers, Body::empty())
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        let items: Vec<live_proto::Presence> =
            serde_json::from_value(r.json()["items"].clone()).unwrap();
        let host = items.iter().find(|p| p.xuid == a.xuid).unwrap();
        assert_eq!(host.build.as_deref(), Some("a65fa01"));
        assert!(host.has(live_proto::capability::VARIANT_EXT));
        let lacking: Vec<_> = live_proto::lacking(
            live_proto::capability::VARIANT_EXT,
            &[a.xuid, old.xuid],
            &items,
        )
        .iter()
        .map(|p| p.gamertag.as_str().to_string())
        .collect();
        assert_eq!(lacking, ["Oldie"]);
    }
}
