//! Network addresses (a session's declared IPv4, its observed endpoint, the
//! host's XNADDR) reach only the operator through the log, and only signed-in
//! games through the session endpoints they join by. Everyone else reads a
//! redacted view.
mod common;
use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use common::*;
use live_proto::{
    lxm,
    session::{self, NewSession, Slots},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The host's LAN address, as it declares it.
const LAN: &str = "10.0.3.59";
/// A distinctive XNADDR: these bytes must never reach a public reader.
const XNADDR: [u8; 36] = [0xab; 36];

fn custom() -> NewSession {
    NewSession {
        addr: Some(LAN.parse().unwrap()),
        port: 3074,
        host_address: XNADDR.to_vec(),
        slots: Slots {
            public_filled: 1,
            public_open: 7,
            ..Default::default()
        },
        attributes: BTreeMap::new(),
        flags: "custom".into(),
        hopper_id: None,
        map: Some("lockout".into()),
        name: Some("Slayer".into()),
        language: None,
        visibility: session::Visibility::Public,
        map_id: None,
        skulls: None,
        campaign: None,
        ping_ms: None,
        variant_ext: None,
    }
}

/// Whether `v` holds a network address anywhere: the LAN address as text,
/// an address-holding key, or the XNADDR's bytes.
fn has_address(v: &Value) -> bool {
    let text = v.to_string();
    let xna = serde_json::to_string(&XNADDR.to_vec()).unwrap();
    text.contains(LAN)
        || text.contains(&xna)
        || text.contains("\"endpoint\"")
        || text.contains("\"host_address\"")
        || text.contains("\"addr\"")
}

fn operator(h: &Harness) -> (&'static str, String) {
    (
        "authorization",
        format!(
            "Bearer {}",
            h.token(&h.operator, "server:live.test", lxm::READ)
        ),
    )
}

async fn get(h: &Harness, uri: &str, auth: Option<(&'static str, String)>) -> Value {
    let headers: Vec<_> = auth.into_iter().collect();
    let r = h.call(Method::GET, uri, &headers, Body::empty()).await;
    assert_eq!(r.status, StatusCode::OK, "{uri}: {}", r.error());
    r.json()
}

/// The `SessionCreated` event of the log as a reader gets it.
fn created(page: &Value) -> Value {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == session::CREATED)
        .expect("the SessionCreated event")
        .clone()
}

#[tokio::test]
async fn only_the_operator_reads_addresses_in_the_log() {
    let h = start(
        std::sync::Arc::new(live_server::store::MemoryStore::new()),
        live_server::Policy {
            admins: vec![live_proto::Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    let host = h.create("Host").await;
    let stranger = h.create("Stranger").await;
    let boss = h.create("Boss").await;
    let r = h
        .call(
            Method::POST,
            "/v1/sessions",
            &[
                h.bearer(&host, lxm::SESSIONS_WRITE),
                ("content-type", "application/json".into()),
            ],
            Body::from(serde_json::to_vec(&custom()).unwrap()),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let sid = r.json()["session_id"].as_str().unwrap().to_string();

    // The operator reads the log as stored: the addresses, and a signature
    // over them.
    let op = created(&get(&h, "/v1/events", Some(operator(&h))).await);
    assert!(has_address(&op), "{op}");
    assert_eq!(op["data"]["advertisement"]["addr"], LAN);
    assert!(!op["jws"].as_str().unwrap().is_empty());
    assert!(op["data"].get("redacted").is_none());

    // Anyone else, anonymous or signed in (a stranger, the host itself, an
    // admin account), gets the event without them, marked redacted, with
    // no signature (it covered the addresses).
    for (who, auth) in [
        ("anonymous", None),
        ("stranger", Some(h.bearer(&stranger, lxm::READ))),
        ("host", Some(h.bearer(&host, lxm::READ))),
        ("admin", Some(h.bearer(&boss, lxm::READ))),
    ] {
        let page = get(&h, "/v1/events", auth).await;
        assert!(!has_address(&page), "{who}: {page}");
        let e = created(&page);
        assert_eq!(e["data"]["redacted"], true, "{who}");
        assert_eq!(e["jws"], "", "{who}");
        // What a list needs is still there.
        assert_eq!(e["data"]["advertisement"]["name"], "Slayer", "{who}");
        assert_eq!(e["data"]["session_id"], sid, "{who}");
    }

    // The snapshot's event tail (what the edge caches for anonymous pages)
    // is redacted the same way; the operator's is whole.
    for (who, auth) in [
        ("anonymous", None),
        ("stranger", Some(h.bearer(&stranger, lxm::READ))),
    ] {
        let snap = get(&h, "/v1/snapshot", auth).await;
        let tail = snap["tail"].as_array().unwrap();
        assert!(
            tail.iter().any(|e| e["type"] == session::CREATED),
            "{who}: {snap}"
        );
        assert!(!has_address(&snap), "{who}: {snap}");
    }
    let snap = get(&h, "/v1/snapshot", Some(operator(&h))).await;
    assert!(has_address(&snap["tail"]));

    // The same on the stream.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    for (who, token) in [
        ("anonymous", None),
        (
            "stranger",
            Some(h.token(&stranger.key, &stranger.author, lxm::READ)),
        ),
    ] {
        let events = streamed(addr, token).await;
        assert!(events.iter().any(|e| e["type"] == session::CREATED));
        for e in &events {
            assert!(!has_address(e), "{who}: {e}");
        }
    }
    let events = streamed(
        addr,
        Some(h.token(&h.operator, "server:live.test", lxm::READ)),
    )
    .await;
    assert!(
        events.iter().any(has_address),
        "the operator's stream is whole"
    );
}

#[tokio::test]
async fn session_endpoints_give_addresses_only_to_signed_in_games() {
    let h = harness().await;
    let host = h.create("Host").await;
    let joiner = h.create("Joiner").await;
    let r = h
        .call(
            Method::POST,
            "/v1/sessions",
            &[
                h.bearer(&host, lxm::SESSIONS_WRITE),
                ("content-type", "application/json".into()),
            ],
            Body::from(serde_json::to_vec(&custom()).unwrap()),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let sid = r.json()["session_id"].as_str().unwrap().to_string();

    // Anonymous: the list and the detail, without addresses.
    let list = get(&h, "/v1/sessions", None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(!has_address(&list), "{list}");
    let one = get(&h, &format!("/v1/sessions/{sid}"), None).await;
    assert!(!has_address(&one), "{one}");
    assert_eq!(one["session"]["advertisement"]["name"], "Slayer");

    // A signed-in game finding a host to join gets what it joins by.
    let list = get(&h, "/v1/sessions", Some(h.bearer(&joiner, lxm::READ))).await;
    assert_eq!(list["items"][0]["endpoint"]["addr"], LAN);
    assert_eq!(
        list["items"][0]["advertisement"]["host_address"],
        json!(XNADDR.to_vec())
    );
    let one = get(
        &h,
        &format!("/v1/sessions/{sid}"),
        Some(h.bearer(&joiner, lxm::READ)),
    )
    .await;
    assert_eq!(one["session"]["endpoint"]["addr"], LAN);
}

#[tokio::test]
async fn an_events_page_holds_at_most_200() {
    let h = start(
        std::sync::Arc::new(live_server::store::MemoryStore::new()),
        live_server::Policy {
            create_rate: (1000.0, 1000),
            ..Default::default()
        },
    )
    .await;
    for i in 0..70 {
        h.create(&format!("Player {i}")).await;
    }
    // 70 accounts write 210 events.
    let page = get(&h, "/v1/events?limit=1000", None).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 200);
    assert_eq!(page["next"], "200");
}

#[test]
fn addresses_are_stripped_at_any_depth() {
    let mut v = json!({
        "advertisement": {"addr": "10.0.0.1", "port": 3074, "host_address": [1, 2], "name": "x"},
        "endpoint": {"addr": "203.0.113.9", "port": 3074, "source": "observed"},
        "nested": [{"ip": "1.2.3.4", "xnaddr": "ab", "keep": 1}],
        "port": 9,
    });
    assert!(live_server::strip_addresses(&mut v));
    assert_eq!(
        v,
        json!({"advertisement": {"name": "x"}, "nested": [{"keep": 1}], "port": 9})
    );
    let mut plain = json!({"xuid": "0009000000000001", "state": "online"});
    assert!(!live_server::strip_addresses(&mut plain));
}
