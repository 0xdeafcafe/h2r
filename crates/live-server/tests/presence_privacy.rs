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
