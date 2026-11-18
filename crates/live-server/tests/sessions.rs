//! Session router, signed batches, projection/replay and trusted tunnel source IP.
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
    session::{self, AttributeValue, NewSession, Slots, Update},
    sign::{EventBody, sign_batch},
};
use live_server::{Live, Policy, ServerIdentity, store::MemoryStore};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;
fn new() -> NewSession {
    NewSession {
        // A local-network address: a declared internet one would not be
        // listed (session_visibility.rs).
        addr: Some("192.168.2.4".parse().unwrap()),
        port: 3074,
        host_address: vec![0; 36],
        slots: Slots {
            public_filled: 1,
            public_open: 7,
            ..Default::default()
        },
        attributes: BTreeMap::from([(3, AttributeValue::Integer(42))]),
        flags: "custom".into(),
        hopper_id: Some(7),
        map: Some("synthetic".into()),
        name: Some("Test lobby".into()),
        language: Some("en".into()),
        visibility: Default::default(),
        map_id: None,
        skulls: None,
        campaign: None,
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
fn body(u: &User, id: &str, kind: &str, data: Value) -> EventBody {
    EventBody::at_unix(&u.key, &u.author, &session::stream(id), kind, data, now())
}
async fn batch(h: &Harness, u: &User, id: &str, bodies: &[EventBody]) -> Resp {
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
        Body::from(sign_batch(&u.key, bodies)),
    )
    .await
}
#[tokio::test]
async fn host_lifecycle_filters_noops_authority_and_atomic_batches() {
    let h = harness().await;
    let a = h.create("Host").await;
    let b = h.create("Joiner").await;
    let r = create(&h, &a, &new()).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let doc = r.json();
    let id = doc["session_id"].as_str().unwrap();
    assert_eq!(doc["session"]["host"], a.author);
    assert_eq!(doc["session"]["host_xuid"], a.xuid.to_string());
    assert_eq!(doc["session"]["xnkid"], id);
    assert!(!doc.to_string().contains("exchange"));
    assert_eq!(
        h.get("/v1/sessions?hopper=7&map=synthetic").await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        h.get("/v1/sessions?hopper=8").await.json()["items"],
        json!([])
    );
    let update = serde_json::to_value(Update {
        slots: Some(Slots {
            public_filled: 2,
            public_open: 6,
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        batch(
            &h,
            &b,
            id,
            &[body(&b, id, session::UPDATED, update.clone())]
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        batch(
            &h,
            &a,
            id,
            &[body(&a, id, session::UPDATED, update.clone())]
        )
        .await
        .status,
        StatusCode::CREATED
    );
    let pos = h.live.position();
    let no_op = body(&a, id, session::UPDATED, update);
    assert_eq!(
        batch(&h, &a, id, std::slice::from_ref(&no_op)).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        h.live.position(),
        pos,
        "no heartbeat events for unchanged updates"
    );
    assert_eq!(
        batch(&h, &a, id, &[no_op]).await.status,
        StatusCode::CONFLICT
    );
    let joined = body(
        &a,
        id,
        session::JOINED,
        json!({"xuid":b.xuid,"slot":"public"}),
    );
    let bad = body(&a, id, session::LEFT, json!({"xuid":a.xuid}));
    assert_eq!(
        batch(&h, &a, id, &[joined.clone(), bad]).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(h.live.position(), pos, "refused batch writes nothing");
    assert_eq!(
        batch(&h, &a, id, &[joined]).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        batch(
            &h,
            &a,
            id,
            &[body(&a, id, session::LEFT, json!({"xuid":b.xuid}))]
        )
        .await
        .status,
        StatusCode::CREATED
    );
    assert_eq!(
        batch(
            &h,
            &a,
            id,
            &[body(&a, id, session::DELETED, json!({"reason":"closed"}))]
        )
        .await
        .status,
        StatusCode::CREATED
    );
    assert_eq!(h.get("/v1/sessions").await.json()["items"], json!([]));
    assert_eq!(
        h.get(&format!("/v1/sessions/{id}")).await.json()["session"]["state"],
        "deleted"
    );
    assert_eq!(
        batch(&h, &a, id, &[body(&a, id, session::UPDATED, json!({}))])
            .await
            .status,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn signed_stream_time_key_and_private_read_rules_are_enforced() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            private: true,
            ..Default::default()
        },
    )
    .await;
    let a = h.create("Host").await;
    let b = h.create("Other").await;
    let r = create(&h, &a, &new()).await;
    let id = r.json()["session_id"].as_str().unwrap().to_string();
    let mut event = body(&a, &id, session::UPDATED, json!({}));
    event.stream = "session-0000000000000000".into();
    assert_eq!(
        batch(&h, &a, &id, &[event]).await.status,
        StatusCode::FORBIDDEN
    );
    // Another player, signing their own event, still may not update it.
    let event = body(&b, &id, session::UPDATED, json!({}));
    assert_eq!(
        batch(&h, &b, &id, &[event]).await.status,
        StatusCode::FORBIDDEN
    );
    let old = EventBody::at_unix(
        &a.key,
        &a.author,
        &session::stream(&id),
        session::UPDATED,
        json!({}),
        now() - 1000,
    );
    assert_eq!(
        batch(&h, &a, &id, &[old]).await.status,
        StatusCode::BAD_REQUEST
    );
    for path in ["/v1/sessions".into(), format!("/v1/sessions/{id}")] {
        assert_eq!(h.get(&path).await.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            h.call(
                Method::GET,
                &path,
                &[(
                    "authorization",
                    format!("Bearer {}", h.token(&a.key, &a.author, lxm::READ))
                )],
                Body::empty()
            )
            .await
            .status,
            StatusCode::OK
        );
    }
    // The generic append endpoint cannot forge the reserved projection.
    assert_eq!(
        h.post_event(
            &a,
            h.event(&a, &session::stream(&id), session::EXPIRED, json!({}))
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn expired_sessions_rebuild_from_the_log_and_limits_reject_bad_data() {
    let store = Arc::new(MemoryStore::new());
    let h = start(
        store.clone(),
        Policy {
            session_idle_s: -1,
            ..Default::default()
        },
    )
    .await;
    let a = h.create("Host").await;
    let mut invalid = new();
    invalid.slots.public_open = u32::MAX;
    assert_eq!(
        create(&h, &a, &invalid).await.status,
        StatusCode::BAD_REQUEST
    );
    invalid = new();
    invalid
        .attributes
        .insert(0x100002, AttributeValue::String("x".repeat(401)));
    assert_eq!(
        create(&h, &a, &invalid).await.status,
        StatusCode::BAD_REQUEST
    );
    let r = create(&h, &a, &new()).await;
    let id = r.json()["session_id"].as_str().unwrap().to_string();
    h.live.sweep().await;
    assert_eq!(
        h.get(&format!("/v1/sessions/{id}")).await.json()["session"]["state"],
        "expired"
    );
    let position = h.live.position();
    h.live.sweep().await;
    assert_eq!(h.live.position(), position);
    let replay = Live::start(
        store,
        ServerIdentity::load(&h.dir).unwrap(),
        Policy::default(),
    )
    .await
    .unwrap();
    let response = live_server::http::router(replay)
        .oneshot(
            Request::builder()
                .uri(format!("/v1/sessions/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let value: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(value["session"]["state"], "expired");
}
#[tokio::test]
async fn observed_endpoint_uses_the_trusted_public_url_client_ip_policy() {
    for (tunnel, peer, expected) in [
        (true, "127.0.0.1:1234", "203.0.113.7"),
        (false, "127.0.0.1:1234", "127.0.0.1"),
        (true, "192.0.2.6:1234", "192.0.2.6"),
    ] {
        let policy = Policy::default();
        policy.public.set("https://tunnel.example", tunnel);
        let h = start(Arc::new(MemoryStore::new()), policy).await;
        let a = h.create("Host").await;
        let mut value = new();
        value.addr = None;
        let req = Request::builder()
            .method(Method::POST)
            .uri("/v1/sessions")
            .header(
                "authorization",
                format!("Bearer {}", h.token(&a.key, &a.author, lxm::SESSIONS_WRITE)),
            )
            .header("content-type", "application/json")
            .header("cf-connecting-ip", "203.0.113.7")
            .extension(ConnectInfo(peer.parse::<std::net::SocketAddr>().unwrap()))
            .body(Body::from(serde_json::to_vec(&value).unwrap()))
            .unwrap();
        let response = h.app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["session"]["endpoint"]["addr"], expected);
        assert_eq!(body["session"]["endpoint"]["source"], "observed");
    }
}

#[tokio::test]
async fn pagination_and_presence_ping_refresh_without_events() {
    let h = harness().await;
    let a = h.create("Host").await;
    assert_eq!(h.sign_in(&a).await.status, StatusCode::CREATED);
    let mut ids = Vec::new();
    for _ in 0..3 {
        let r = create(&h, &a, &new()).await;
        assert_eq!(r.status, StatusCode::CREATED);
        ids.push(r.json()["session_id"].as_str().unwrap().to_string());
    }
    ids.sort();
    let first = h.get("/v1/sessions?limit=1").await.json();
    assert_eq!(first["items"][0]["session_id"], ids[0]);
    let second = h
        .get(&format!(
            "/v1/sessions?limit=1&cursor={}",
            first["next"].as_str().unwrap()
        ))
        .await
        .json();
    assert_eq!(second["items"][0]["session_id"], ids[1]);
    let third = h
        .get(&format!(
            "/v1/sessions?limit=1&cursor={}",
            second["next"].as_str().unwrap()
        ))
        .await
        .json();
    assert_eq!(third["items"][0]["session_id"], ids[2]);
    assert!(third["next"].is_null());
    for path in [
        "/v1/sessions?limit=0",
        "/v1/sessions?limit=1001",
        "/v1/sessions?hopper=not-a-number",
        "/v1/sessions?attributes=not-json",
    ] {
        assert_eq!(h.get(path).await.status, StatusCode::BAD_REQUEST);
    }
    let before =
        h.get(&format!("/v1/sessions/{}", ids[0])).await.json()["session"]["last_activity"]
            .as_i64()
            .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let position = h.live.position();
    let token = h.token(&a.key, &a.author, lxm::EVENTS_WRITE);
    assert_eq!(h.live.ping(Some(&token)).unwrap(), 90);
    assert_eq!(h.live.position(), position);
    let after = h.get(&format!("/v1/sessions/{}", ids[0])).await.json()["session"]["last_activity"]
        .as_i64()
        .unwrap();
    assert!(
        after > before,
        "host ping refreshes all open ads without appending events"
    );
}
fn optimatch_path(q: &live_proto::optimatch::Query, limit: u32) -> String {
    let json = serde_json::to_string(q).unwrap();
    let encoded: String = json
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("/v1/sessions?limit={limit}&optimatch={encoded}")
}
fn optimatch_session(
    hopper: i64,
    average: i64,
    lowest: i64,
    highest: i64,
    room: u64,
) -> NewSession {
    let i = |v: i64| AttributeValue::Integer(v as u64);
    NewSession {
        flags: "hopper".into(),
        hopper_id: Some(hopper as u32),
        attributes: BTreeMap::from([
            (1, i(hopper)),
            (2, i(average)),
            (3, i(room.count_ones() as i64)),
            (5, i(lowest)),
            (6, i(highest)),
            (7, i(0)),
            (8, AttributeValue::Integer(room)),
        ]),
        ..new()
    }
}
/// The game's OptiMatch search (procedure 1, decoded in the HLE): the server
/// keeps the playlist's sessions whose levels sit inside the searching
/// party's window and that have room for the party, closest mean first.
#[tokio::test]
async fn optimatch_searches_filter_by_playlist_window_and_room() {
    use live_proto::optimatch::Query;
    let h = harness().await;
    let a = h.create("Host").await;
    assert_eq!(h.sign_in(&a).await.status, StatusCode::CREATED);
    let mut ids = BTreeMap::new();
    for (name, s) in [
        ("near", optimatch_session(3, 11, 9, 13, 0b111)),
        ("nearest", optimatch_session(3, 10, 9, 12, 0b11)),
        ("one seat", optimatch_session(3, 10, 9, 12, 0b1)),
        ("too good", optimatch_session(3, 20, 18, 22, 0b111)),
        ("other playlist", optimatch_session(4, 10, 9, 12, 0b111)),
        ("custom", new()),
    ] {
        let r = create(&h, &a, &s).await;
        assert_eq!(r.status, StatusCode::CREATED, "{name}");
        ids.insert(r.json()["session_id"].as_str().unwrap().to_string(), name);
    }
    let names = |page: Value| -> Vec<&str> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| ids[s["session_id"].as_str().unwrap()])
            .collect()
    };
    let duo = Query {
        hopper: 3,
        average: 10,
        lowest: Some(8),
        highest: Some(14),
        party: Some(2),
    };
    let page = h.get(&optimatch_path(&duo, 50)).await.json();
    assert_eq!(names(page.clone()), ["nearest", "near"]);
    assert!(page["next"].is_null());
    assert_eq!(
        names(h.get(&optimatch_path(&duo, 1)).await.json()),
        ["nearest"]
    );
    // Widened all the way (an unranked search drops the window): the strong
    // lobby comes back, last.
    let open = Query {
        lowest: None,
        highest: None,
        ..duo
    };
    assert_eq!(
        names(h.get(&optimatch_path(&open, 50)).await.json()),
        ["nearest", "near", "too good"]
    );
    // The custom games browser's form: hopper -1 finds no playlist session.
    let browser = Query {
        hopper: -1,
        average: 0,
        lowest: None,
        highest: None,
        party: None,
    };
    assert_eq!(
        names(h.get(&optimatch_path(&browser, 50)).await.json()),
        ["custom"]
    );
    assert_eq!(
        h.get("/v1/sessions?optimatch=not-json").await.status,
        StatusCode::BAD_REQUEST
    );
}
