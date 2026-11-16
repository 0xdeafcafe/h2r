//! Session visibility (docs/live-custom-games.md): who finds a session in
//! `GET /v1/sessions`, friends-only through the friends graph, and listed
//! sessions held to the address the server saw the host connect from.
mod common;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode},
};
use common::*;
use http_body_util::BodyExt;
use live_proto::{
    lxm,
    session::{self, NewSession, Slots, Visibility},
    sign::{EventBody, sign_batch},
    streams, types,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tower::ServiceExt;

fn ad(addr: Option<&str>, visibility: Visibility) -> NewSession {
    NewSession {
        addr: addr.map(|a| a.parse().unwrap()),
        port: 3074,
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
        visibility,
    }
}

/// `POST /v1/sessions` as `u`, from TCP peer `peer` when given.
async fn create_from(h: &Harness, u: &User, data: &NewSession, peer: Option<&str>) -> Value {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/v1/sessions")
        .header(
            "authorization",
            format!("Bearer {}", h.token(&u.key, &u.author, lxm::SESSIONS_WRITE)),
        )
        .header("content-type", "application/json");
    if let Some(p) = peer {
        req = req.extension(ConnectInfo(p.parse::<std::net::SocketAddr>().unwrap()));
    }
    let response = h
        .app
        .clone()
        .oneshot(
            req.body(Body::from(serde_json::to_vec(data).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["session"].clone()
}

/// The ids `GET /v1/sessions` lists, read anonymously or as `viewer`.
async fn listed(h: &Harness, viewer: Option<&User>) -> Vec<String> {
    let headers: Vec<_> = viewer.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
    let r = h
        .call(Method::GET, "/v1/sessions", &headers, Body::empty())
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let mut ids: Vec<String> = r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

async fn befriend(h: &Harness, a: &User, b: &User) {
    let ask = h.event(
        a,
        &streams::account(a.xuid),
        types::FRIEND_REQUESTED,
        json!({"from": a.xuid, "to": b.xuid}),
    );
    assert_eq!(h.post_event(a, ask).await.status, StatusCode::CREATED);
    let yes = h.event(
        b,
        &streams::account(b.xuid),
        types::FRIEND_ACCEPTED,
        json!({"from": a.xuid, "to": b.xuid}),
    );
    assert_eq!(h.post_event(b, yes).await.status, StatusCode::CREATED);
}

fn id(s: &Value) -> String {
    s["session_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn lists_follow_visibility_and_the_friends_graph() {
    let h = harness().await;
    let host = h.create("Host").await;
    let friend = h.create("Friend").await;
    let stranger = h.create("Stranger").await;
    befriend(&h, &host, &friend).await;
    let lan = Some("192.168.1.50");
    let public = id(&create_from(&h, &host, &ad(lan, Visibility::Public), None).await);
    let friends = id(&create_from(&h, &host, &ad(lan, Visibility::Friends), None).await);
    let invite = id(&create_from(&h, &host, &ad(lan, Visibility::Invite), None).await);
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };

    assert_eq!(
        listed(&h, None).await,
        std::slice::from_ref(&public),
        "anonymous"
    );
    assert_eq!(
        listed(&h, Some(&stranger)).await,
        std::slice::from_ref(&public)
    );
    assert_eq!(
        listed(&h, Some(&friend)).await,
        sorted(vec![public.clone(), friends.clone()])
    );
    assert_eq!(
        listed(&h, Some(&host)).await,
        sorted(vec![public.clone(), friends.clone(), invite.clone()]),
        "hosts see their own"
    );
    // The id stays the capability: invitations read any of them.
    for s in [&public, &friends, &invite] {
        assert_eq!(
            h.get(&format!("/v1/sessions/{s}")).await.status,
            StatusCode::OK
        );
    }
    // A filter by visibility, and a bad bearer is refused, not anonymous.
    let r = h.get("/v1/sessions?visibility=friends").await;
    assert_eq!(r.json()["items"], json!([]));
    let r = h
        .call(
            Method::GET,
            "/v1/sessions",
            &[("authorization", "Bearer nonsense".into())],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // The create record carries the field; public is the omitted default.
    let doc = h.get(&format!("/v1/sessions/{friends}")).await.json();
    assert_eq!(doc["session"]["advertisement"]["visibility"], "friends");
    let doc = h.get(&format!("/v1/sessions/{public}")).await.json();
    assert!(doc["session"]["advertisement"].get("visibility").is_none());
}

#[tokio::test]
async fn the_host_changes_visibility_with_a_signed_update() {
    let h = harness().await;
    let host = h.create("Host").await;
    let s = id(&create_from(&h, &host, &ad(Some("10.0.0.5"), Visibility::Invite), None).await);
    assert!(listed(&h, None).await.is_empty());
    let body = EventBody::at_unix(
        &host.key,
        &host.author,
        &session::stream(&s),
        session::UPDATED,
        json!({"visibility": "public"}),
        now(),
    );
    let r = h
        .call(
            Method::POST,
            &format!("/v1/sessions/{s}/events"),
            &[
                h.bearer(&host, lxm::SESSIONS_WRITE),
                ("content-type", "application/jose".into()),
            ],
            Body::from(sign_batch(&host.key, &[body])),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(listed(&h, None).await, [s]);
}

#[tokio::test]
async fn listed_sessions_use_the_observed_address_not_a_declared_one() {
    let h = harness().await;
    let host = h.create("Host").await;
    // Listed and seen from the internet: the declared address is ignored.
    let s = create_from(
        &h,
        &host,
        &ad(Some("198.51.100.99"), Visibility::Public),
        Some("203.0.113.8:5000"),
    )
    .await;
    assert_eq!(s["endpoint"]["addr"], "203.0.113.8");
    assert_eq!(s["endpoint"]["source"], "observed");
    assert_eq!(listed(&h, None).await, [id(&s)]);
    // A LAN-only address behind NAT is replaced the same way.
    let s = create_from(
        &h,
        &host,
        &ad(Some("192.168.1.50"), Visibility::Friends),
        Some("203.0.113.8:5001"),
    )
    .await;
    assert_eq!(s["endpoint"]["addr"], "203.0.113.8");
    // Invite-only keeps its declared address, as before this field.
    let s = create_from(
        &h,
        &host,
        &ad(Some("198.51.100.99"), Visibility::Invite),
        Some("203.0.113.8:5002"),
    )
    .await;
    assert_eq!(s["endpoint"]["addr"], "198.51.100.99");
    assert_eq!(s["endpoint"]["source"], "declared");
}

#[tokio::test]
async fn a_declared_internet_address_is_created_but_never_listed() {
    let h = harness().await;
    let host = h.create("Host").await;
    // From loopback (the server's own machine) or with no observation, the
    // declared address stands; an internet one cannot be listed, so nobody's
    // browser is pointed at a third party.
    for peer in [Some("127.0.0.1:4000"), None] {
        let s = create_from(
            &h,
            &host,
            &ad(Some("198.51.100.99"), Visibility::Public),
            peer,
        )
        .await;
        assert_eq!(s["endpoint"]["source"], "declared");
        assert!(!listed(&h, None).await.contains(&id(&s)));
        assert!(!listed(&h, Some(&host)).await.contains(&id(&s)));
    }
    // From loopback a local-network declaration is listed: two instances
    // and a server on one machine.
    let s = create_from(
        &h,
        &host,
        &ad(Some("192.168.1.50"), Visibility::Public),
        Some("127.0.0.1:4001"),
    )
    .await;
    assert_eq!(s["endpoint"]["addr"], "192.168.1.50");
    assert!(listed(&h, None).await.contains(&id(&s)));
}
