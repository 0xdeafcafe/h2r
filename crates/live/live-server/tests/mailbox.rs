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
        ping_ms: None,
        variant_ext: None,
        rules: None,
        firefight: None,
        browser: None,
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
async fn befriend(h: &Harness, a: &User, b: &User) {
    for (by, kind) in [
        (a, live_proto::types::FRIEND_REQUESTED),
        (b, live_proto::types::FRIEND_ACCEPTED),
    ] {
        let r = h
            .post_event(
                by,
                h.event(
                    by,
                    &live_proto::streams::account(by.xuid),
                    kind,
                    json!({"from":a.xuid,"to":b.xuid}),
                ),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    }
}
fn text_event(h: &Harness, from: &User, to: &User, text: &str) -> String {
    h.event(
        from,
        &mailbox::stream(to.xuid),
        mailbox::TEXT_SENT,
        json!({"from":from.xuid,"text":text}),
    )
}
fn delete_event(h: &Harness, u: &User, id: u64) -> String {
    h.event(
        u,
        &mailbox::stream(u.xuid),
        mailbox::TEXT_DELETED,
        json!({"by":u.xuid,"message_id":id}),
    )
}
#[tokio::test]
async fn friends_send_read_and_delete_text_messages() {
    let h = harness().await;
    let a = h.create("Sender").await;
    let b = h.create("Reader").await;
    let other = h.create("Other").await;
    befriend(&h, &a, &b).await;
    let r = h.post_event(&a, text_event(&h, &a, &b, "hello \u{96ea}\nthere")).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let event = r.json();
    let stored = SignedEvent::open(event["jws"].as_str().unwrap()).unwrap();
    stored.verify(a.key.public()).unwrap();
    let id = event["position"].as_u64().unwrap();
    let v = read(&h, &b).await.json();
    assert_eq!(v["items"], json!([]));
    let m = &v["messages"];
    assert_eq!(m.as_array().unwrap().len(), 1);
    assert_eq!(m[0]["id"].as_u64().unwrap(), id);
    assert_eq!(m[0]["from"], a.xuid.to_string());
    assert_eq!(m[0]["to"], b.xuid.to_string());
    assert_eq!(m[0]["gamertag"], "Sender");
    assert_eq!(m[0]["text"], "hello \u{96ea}\nthere");
    assert!(m[0]["sent_at"].as_i64().unwrap() > 0);
    // Only the owner reads the mailbox; the sender's own has nothing.
    assert_eq!(read(&h, &a).await.json()["messages"], json!([]));
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
    // Only the recipient deletes, and only a message that is there.
    assert_eq!(
        h.post_event(&a, delete_event(&h, &a, id)).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.post_event(
            &a,
            h.event(
                &a,
                &mailbox::stream(b.xuid),
                mailbox::TEXT_DELETED,
                json!({"by":b.xuid,"message_id":id})
            )
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.post_event(&b, delete_event(&h, &b, id + 1000)).await.status,
        StatusCode::BAD_REQUEST
    );
    let r = h.post_event(&b, delete_event(&h, &b, id)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(read(&h, &b).await.json()["messages"], json!([]));
    assert_eq!(
        h.post_event(&b, delete_event(&h, &b, id)).await.status,
        StatusCode::BAD_REQUEST,
        "a deleted message is gone"
    );
}
#[tokio::test]
async fn text_messages_need_friends_and_sound_text() {
    let h = harness().await;
    let a = h.create("Sender").await;
    let b = h.create("Reader").await;
    let c = h.create("Stranger").await;
    befriend(&h, &a, &b).await;
    // Not friends, either way, or to oneself.
    assert_eq!(
        h.post_event(&c, text_event(&h, &c, &b, "hi")).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.post_event(&b, text_event(&h, &b, &c, "hi")).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.post_event(&a, text_event(&h, &a, &a, "hi")).await.status,
        StatusCode::FORBIDDEN
    );
    // The sender is the signing account.
    assert_eq!(
        h.post_event(
            &a,
            h.event(
                &a,
                &mailbox::stream(b.xuid),
                mailbox::TEXT_SENT,
                json!({"from":b.xuid,"text":"as someone else"})
            )
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    // Text: not empty, blank, too long or with control characters; no
    // unknown fields.
    for bad in [
        String::new(),
        "  \n".into(),
        "x".repeat(mailbox::TEXT_MAX + 1),
        "bell\u{7}".into(),
        "nul\u{0}".into(),
    ] {
        assert_eq!(
            h.post_event(&a, text_event(&h, &a, &b, &bad)).await.status,
            StatusCode::BAD_REQUEST,
            "{bad:?}"
        );
    }
    assert_eq!(
        h.post_event(
            &a,
            h.event(
                &a,
                &mailbox::stream(b.xuid),
                mailbox::TEXT_SENT,
                json!({"from":a.xuid,"text":"hi","session_id":"8000000000000001"})
            )
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    let longest = "x".repeat(mailbox::TEXT_MAX);
    assert_eq!(
        h.post_event(&a, text_event(&h, &a, &b, &longest)).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        read(&h, &b).await.json()["messages"].as_array().unwrap().len(),
        1
    );
    // Unfriending ends messaging.
    let r = h
        .post_event(
            &a,
            h.event(
                &a,
                &live_proto::streams::account(a.xuid),
                live_proto::types::FRIEND_REMOVED,
                json!({"by":a.xuid,"friend":b.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(
        h.post_event(&a, text_event(&h, &a, &b, "hi")).await.status,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn text_messages_show_in_the_log_only_to_their_two_accounts() {
    let h = harness().await;
    let a = h.create("Sender").await;
    let b = h.create("Reader").await;
    let c = h.create("Friend").await;
    befriend(&h, &a, &b).await;
    befriend(&h, &a, &c).await;
    let r = h.post_event(&a, text_event(&h, &a, &b, "secret")).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let id = r.json()["position"].as_u64().unwrap();
    let r = h.post_event(&b, delete_event(&h, &b, id)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let kinds = |u: Option<&User>| {
        let h = &h;
        let headers: Vec<_> = u.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
        async move {
            let r = h
                .call(Method::GET, "/v1/events", &headers, Body::empty())
                .await;
            assert_eq!(r.status, StatusCode::OK, "{}", r.error());
            r.json()["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e["type"].as_str())
                .filter(|t| mailbox::is_kind(t))
                .map(String::from)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(kinds(None).await, Vec::<String>::new());
    assert_eq!(kinds(Some(&c)).await, Vec::<String>::new(), "a friend of the sender");
    assert_eq!(kinds(Some(&a)).await, [mailbox::TEXT_SENT]);
    assert_eq!(
        kinds(Some(&b)).await,
        [mailbox::TEXT_SENT, mailbox::TEXT_DELETED]
    );
}
#[tokio::test]
async fn text_messages_replay_from_the_event_log() {
    let store = Arc::new(MemoryStore::new());
    let h = start(store.clone(), Policy::default()).await;
    let a = h.create("Sender").await;
    let b = h.create("Reader").await;
    befriend(&h, &a, &b).await;
    let r = h.post_event(&a, text_event(&h, &a, &b, "one")).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = h.post_event(&a, text_event(&h, &a, &b, "two")).await;
    let two = r.json()["position"].as_u64().unwrap();
    let r = h.post_event(&b, delete_event(&h, &b, two)).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let before = read(&h, &b).await.json();
    assert_eq!(before["messages"].as_array().unwrap().len(), 1);
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
    assert_eq!(read(&h2, &b).await.json(), before);
}
