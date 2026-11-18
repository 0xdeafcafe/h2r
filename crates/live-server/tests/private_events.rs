//! Private event kinds in the log: game invitations show only to their two
//! players, friend events only to the two accounts and their friends,
//! content reports only to admins. The operator and admins see all of them;
//! anonymous readers none, on `/v1/events`, the `/v1/snapshot` tail and
//! `/v1/stream` alike.
mod common;
use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use common::*;
use live_proto::{
    lxm, mailbox,
    maps::CONTENT_REPORTED,
    session::{NewSession, Slots},
    sign::KeyPair,
    streams, types,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const FRIEND: [&str; 2] = [types::FRIEND_REQUESTED, types::FRIEND_ACCEPTED];
const INVITE: [&str; 2] = [mailbox::SENT, mailbox::ANSWERED];
const PRIVATE: [&str; 5] = [
    types::FRIEND_REQUESTED,
    types::FRIEND_ACCEPTED,
    mailbox::SENT,
    mailbox::ANSWERED,
    CONTENT_REPORTED,
];

fn created(r: Resp) {
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
}

async fn friend(h: &Harness, by: &User, kind: &str, from: &User, to: &User) {
    let jws = h.event(
        by,
        &streams::account(by.xuid),
        kind,
        json!({"from": from.xuid, "to": to.xuid}),
    );
    created(h.post_event(by, jws).await);
}

async fn open_session(h: &Harness, host: &User) -> String {
    created(h.sign_in(host).await);
    let n = NewSession {
        addr: Some("127.0.0.1".parse().unwrap()),
        port: 31000,
        host_address: vec![0; 36],
        slots: Slots {
            public_filled: 1,
            public_open: 7,
            ..Default::default()
        },
        attributes: BTreeMap::new(),
        flags: "custom".into(),
        hopper_id: None,
        map: None,
        name: None,
        language: None,
        visibility: Default::default(),
        map_id: None,
        skulls: None,
        campaign: None,
        ping_ms: None,
    };
    let r = h
        .call(
            Method::POST,
            "/v1/sessions",
            &[
                h.bearer(host, lxm::SESSIONS_WRITE),
                ("content-type", "application/json".into()),
            ],
            serde_json::to_vec(&n).unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    r.json()["session_id"].as_str().unwrap().into()
}

/// The private event types a reader sees on `/v1/events` and in the
/// snapshot's tail (asserted equal), as a set. `reader` is a key and its
/// author id; tokens are single use, so each read gets a fresh one.
async fn seen(h: &Harness, reader: Option<(&KeyPair, &str)>) -> BTreeSet<String> {
    let mut out = Vec::new();
    for uri in ["/v1/events", "/v1/snapshot"] {
        let headers: Vec<_> = reader
            .map(|(k, iss)| {
                (
                    "authorization",
                    format!("Bearer {}", h.token(k, iss, lxm::READ)),
                )
            })
            .into_iter()
            .collect();
        let r = h.call(Method::GET, uri, &headers, Body::empty()).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}: {}", r.error());
        let v = r.json();
        let items = if uri == "/v1/events" {
            &v["items"]
        } else {
            &v["tail"]
        };
        out.push(private_types(items.as_array().unwrap()));
    }
    assert_eq!(out[0], out[1], "/v1/events and the snapshot tail agree");
    out.remove(0)
}

fn private_types(events: &[Value]) -> BTreeSet<String> {
    events
        .iter()
        .filter_map(|e| e["type"].as_str())
        .filter(|t| PRIVATE.contains(t))
        .map(str::to_string)
        .collect()
}

fn set(kinds: &[&str]) -> BTreeSet<String> {
    kinds.iter().map(|k| k.to_string()).collect()
}

#[tokio::test]
async fn invites_friends_and_reports_show_only_to_who_they_concern() {
    let h = start(
        std::sync::Arc::new(live_server::store::MemoryStore::new()),
        live_server::Policy {
            admins: vec![live_proto::Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    h.live.set_data_dir(&h.dir);
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    let pal = h.create("Pal").await;
    let stranger = h.create("Stranger").await;
    let boss = h.create("Boss").await;

    // Pal and Host are friends; Host has asked Guest (pending).
    friend(&h, &pal, types::FRIEND_REQUESTED, &pal, &host).await;
    friend(&h, &host, types::FRIEND_ACCEPTED, &pal, &host).await;
    friend(&h, &host, types::FRIEND_REQUESTED, &host, &guest).await;

    // Host invites Guest to a game; Guest declines.
    let sid = open_session(&h, &host).await;
    let sent = h
        .post_event(
            &host,
            h.event(
                &host,
                &mailbox::stream(guest.xuid),
                mailbox::SENT,
                json!({"from": host.xuid, "session_id": sid}),
            ),
        )
        .await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.error());
    let invite = sent.json()["position"].as_u64().unwrap();
    let answered = h
        .post_event(
            &guest,
            h.event(
                &guest,
                &mailbox::stream(guest.xuid),
                mailbox::ANSWERED,
                json!({"by": guest.xuid, "session_id": sid, "invite_id": invite, "answer": "decline"}),
            ),
        )
        .await;
    assert_eq!(answered.status, StatusCode::CREATED, "{}", answered.error());

    // Stranger reports some content Guest uploaded.
    let bytes = b"PATCH invented bytes";
    let sha = content::Sha256::of(bytes);
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/content/{sha}"),
            &[h.bearer(&guest, lxm::CONTENT_WRITE)],
            Body::from(bytes.to_vec()),
        )
        .await;
    assert!(r.status.is_success(), "{}", r.error());
    let r = h
        .call(
            Method::POST,
            &format!("/v1/content/{sha}/report"),
            &[h.bearer(&stranger, lxm::CONTENT_WRITE)],
            json!({"reason": "not mine"}).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());

    let everything = set(&PRIVATE);
    fn me(u: &User) -> Option<(&KeyPair, &str)> {
        Some((&u.key, u.author.as_str()))
    }
    let cases = [
        ("anonymous", None, set(&[])),
        // The reporter itself: reports are for admins only.
        ("stranger", me(&stranger), set(&[])),
        // A party to every friend event (one as a friend of Host) and
        // the sender of the invitation.
        ("host", me(&host), set(&[FRIEND, INVITE].concat())),
        // The invitation's recipient, and a party to the pending request.
        ("guest", me(&guest), set(&[FRIEND[0], INVITE[0], INVITE[1]])),
        // Host's friend: every friend event of Host's, no invitation.
        ("pal", me(&pal), set(&FRIEND)),
        ("admin", me(&boss), everything.clone()),
        (
            "operator",
            Some((&h.operator, "server:live.test")),
            everything.clone(),
        ),
    ];
    for (who, reader, want) in cases {
        assert_eq!(seen(&h, reader).await, want, "{who}");
    }

    // The friend events a party sees are exactly the ones concerning it:
    // Guest gets the request to it, not Pal's friendship with Host.
    let r = h
        .call(
            Method::GET,
            "/v1/events",
            &[h.bearer(&guest, lxm::READ)],
            Body::empty(),
        )
        .await;
    let friends: Vec<Value> = r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| FRIEND.contains(&e["type"].as_str().unwrap()))
        .cloned()
        .collect();
    assert_eq!(friends.len(), 1, "{friends:?}");
    assert_eq!(friends[0]["data"]["from"], host.xuid.to_string());

    // The stream filters the same way, replay included.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    let token = |u: &User| h.token(&u.key, &u.author, lxm::READ);
    for (who, t, want) in [
        ("anonymous", None, set(&[])),
        ("stranger", Some(token(&stranger)), set(&[])),
        ("pal", Some(token(&pal)), set(&FRIEND)),
        (
            "guest",
            Some(token(&guest)),
            set(&[FRIEND[0], INVITE[0], INVITE[1]]),
        ),
        ("admin", Some(token(&boss)), everything.clone()),
    ] {
        assert_eq!(private_types(&streamed(addr, t).await), want, "{who}");
    }
}
