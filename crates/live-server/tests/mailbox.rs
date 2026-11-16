//! Signed game invites, authority, races and replay through the memory router.
mod common;
use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use common::*;
use live_proto::{
    lxm, mailbox,
    session::{NewSession, Slots},
    sign::SignedEvent,
};
use live_server::{Live, Policy, ServerIdentity, store::MemoryStore};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
async fn session(h: &Harness, u: &User) -> String {
    assert_eq!(h.sign_in(u).await.status, StatusCode::CREATED);
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
    };
    let r = h
        .call(
            Method::POST,
            "/v1/sessions",
            &[
                h.bearer(u, lxm::SESSIONS_WRITE),
                ("content-type", "application/json".into()),
            ],
            serde_json::to_vec(&n).unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    r.json()["session_id"].as_str().unwrap().into()
}
async fn read(h: &Harness, u: &User) -> Resp {
    h.call(
        Method::GET,
        &format!("/v1/accounts/by-xuid/{}/mailbox", u.xuid),
        &[h.bearer(u, lxm::READ)],
        Body::empty(),
    )
    .await
}
async fn send(h: &Harness, a: &User, b: &User, id: &str) -> Resp {
    h.post_event(
        a,
        h.event(
            a,
            &mailbox::stream(b.xuid),
            mailbox::SENT,
            json!({"from":a.xuid,"session_id":id}),
        ),
    )
    .await
}
fn answer(h: &Harness, u: &User, id: &str, invite: u64, answer: &str) -> String {
    h.event(
        u,
        &mailbox::stream(u.xuid),
        mailbox::ANSWERED,
        json!({"by":u.xuid,"session_id":id,"invite_id":invite,"answer":answer}),
    )
}
#[tokio::test]
async fn invite_send_accept_decline_remove_and_signed_provenance() {
    let h = harness().await;
    let a = h.create("Host").await;
    let b = h.create("Joiner").await;
    let id = session(&h, &a).await;
    for response in ["accept", "decline", "remove"] {
        let r = send(&h, &a, &b, &id).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
        let event = r.json();
        let stored = SignedEvent::open(event["jws"].as_str().unwrap()).unwrap();
        stored.verify(a.key.public()).unwrap();
        let list = read(&h, &b).await;
        assert_eq!(list.status, StatusCode::OK);
        let v = list.json();
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["items"][0]["from"], a.xuid.to_string());
        assert_eq!(v["items"][0]["gamertag"], "Host");
        let invite = v["items"][0]["id"].as_u64().unwrap();
        assert_eq!(invite, event["position"].as_u64().unwrap());
        assert_eq!(send(&h, &a, &b, &id).await.status, StatusCode::CONFLICT);
        let r = h
            .post_event(&b, answer(&h, &b, &id, invite, response))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
        let stored = SignedEvent::open(r.json()["jws"].as_str().unwrap()).unwrap();
        stored.verify(b.key.public()).unwrap();
        assert_eq!(read(&h, &b).await.json()["items"], json!([]));
        assert_eq!(
            h.post_event(&b, answer(&h, &b, &id, invite, response))
                .await
                .status,
            StatusCode::CONFLICT
        );
    }
}
#[tokio::test]
async fn mailbox_requires_owner_read_and_actual_signed_in_host_send() {
    let h = harness().await;
    let a = h.create("Host").await;
    let b = h.create("Joiner").await;
    let other = h.create("Other").await;
    let id = session(&h, &a).await;
    assert_eq!(send(&h, &b, &a, &id).await.status, StatusCode::FORBIDDEN);
    assert_eq!(send(&h, &a, &a, &id).await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        send(&h, &a, &b, "8000000000000000").await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.post_event(
            &b,
            h.event(
                &b,
                &mailbox::stream(a.xuid),
                mailbox::SENT,
                json!({"from":a.xuid,"session_id":id})
            )
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.post_event(
            &a,
            h.event(
                &a,
                &mailbox::stream(b.xuid),
                mailbox::SENT,
                json!({"from":a.xuid,"session_id":id,"exchange_key":"forbidden extra data"})
            )
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    let sent = send(&h, &a, &b, &id).await;
    assert_eq!(sent.status, StatusCode::CREATED);
    let invite = sent.json()["position"].as_u64().unwrap();
    assert_eq!(
        h.get(&format!("/v1/accounts/by-xuid/{}/mailbox", b.xuid))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.call(
            Method::GET,
            &format!("/v1/accounts/by-xuid/{}/mailbox", b.xuid),
            &[h.bearer(&other, lxm::READ)],
            Body::empty()
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.post_event(
            &other,
            h.event(
                &other,
                &mailbox::stream(b.xuid),
                mailbox::ANSWERED,
                json!({"by":b.xuid,"session_id":id,"invite_id":invite,"answer":"accept"})
            )
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.post_event(&b, answer(&h, &b, &id, invite, "not-an-answer"))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        read(&h, &b).await.json()["items"].as_array().unwrap().len(),
        1
    );
}
#[tokio::test]
async fn concurrent_answers_commit_once_and_closed_sessions_leave_no_inbox_item() {
    let h = harness().await;
    let a = h.create("Host").await;
    let b = h.create("Joiner").await;
    let id = session(&h, &a).await;
    let sent = send(&h, &a, &b, &id).await;
    let invite = sent.json()["position"].as_u64().unwrap();
    let (x, y) = tokio::join!(
        h.post_event(&b, answer(&h, &b, &id, invite, "accept")),
        h.post_event(&b, answer(&h, &b, &id, invite, "decline"))
    );
    assert!(
        (x.status == StatusCode::CREATED && y.status == StatusCode::CONFLICT)
            || (y.status == StatusCode::CREATED && x.status == StatusCode::CONFLICT)
    );
    assert_eq!(send(&h, &a, &b, &id).await.status, StatusCode::CREATED);
    let p = read(&h, &b).await.json()["items"][0]["id"]
        .as_u64()
        .unwrap();
    let e = live_proto::sign::EventBody::at_unix(
        &a.key,
        &a.author,
        &live_proto::session::stream(&id),
        live_proto::session::DELETED,
        json!({"reason":"closed"}),
        now(),
    );
    let r = h
        .call(
            Method::POST,
            &format!("/v1/sessions/{id}/events"),
            &[h.bearer(&a, lxm::SESSIONS_WRITE)],
            live_proto::sign::sign_batch(&a.key, &[e]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(read(&h, &b).await.json()["items"], json!([]));
    assert_eq!(
        h.post_event(&b, answer(&h, &b, &id, p, "accept"))
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(send(&h, &a, &b, &id).await.status, StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn pending_mailbox_replays_from_the_event_log() {
    let store = Arc::new(MemoryStore::new());
    let h = start(store.clone(), Policy::default()).await;
    let a = h.create("Host").await;
    let b = h.create("Joiner").await;
    let id = session(&h, &a).await;
    assert_eq!(send(&h, &a, &b, &id).await.status, StatusCode::CREATED);
    let before = read(&h, &b).await.json();
    let replay = Live::start(
        store,
        ServerIdentity::load(&h.dir).unwrap(),
        Policy::default(),
    )
    .await
    .unwrap();
    let h2 = Harness {
        app: live_server::http::router(replay.clone()),
        live: replay,
        operator: ServerIdentity::load(&h.dir).unwrap().key,
        id: h.id.clone(),
        dir: temp_dir(),
    };
    let after: Value = read(&h2, &b).await.json();
    assert_eq!(after, before);
}
