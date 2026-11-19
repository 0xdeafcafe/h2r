//! Dedicated host HTTP contract through the in-memory router harness.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode, header};
use common::{Resp, harness, start};
use live_proto::{Gamertag, lxm};
use live_server::{Policy, store::MemoryStore};

const DETAIL: &str = "/v1/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA";
const REGISTER: &str = "/v1/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA/register";
const REPORT: &str = "/v1/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA/report";
const TUNNEL: &str = "/v1/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA/tunnel";
const INVENTORY: &str = "/v1/admin/hosts";
const STOP: &str = "/v1/admin/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA/stop";

const ENDPOINT: &str = r#"{
    "transport": "udp",
    "udp": "192.0.2.12:31000",
    "xnaddr": "c000020cc000020c18790000000000000000000000000000000000000000000000000000"
}"#;
const PLAYING: &str = r#"{"state":"playing","players":2}"#;

fn assert_unauthorized(response: &Resp) {
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.error(), "unauthorized");
    assert_eq!(response.headers[header::WWW_AUTHENTICATE], "Bearer");
}

#[tokio::test]
async fn host_routes_require_bearers_on_a_public_server() {
    let h = harness().await;
    for (method, uri, body) in [
        (Method::POST, REGISTER, ENDPOINT),
        (Method::POST, REPORT, PLAYING),
        (Method::GET, DETAIL, ""),
        (Method::GET, INVENTORY, ""),
        (Method::POST, STOP, ""),
    ] {
        assert_unauthorized(&h.call(method, uri, &[], body).await);
    }
}

#[tokio::test]
async fn host_writes_reject_account_tokens_with_host_scopes() {
    let h = harness().await;
    let player = h.create("Player").await;
    for (uri, body, scope) in [
        (REGISTER, ENDPOINT, "hosts.register"),
        (REPORT, PLAYING, "hosts.report"),
    ] {
        let response = h
            .call(Method::POST, uri, &[h.bearer(&player, scope)], body)
            .await;
        match response.status {
            StatusCode::UNAUTHORIZED => assert_unauthorized(&response),
            StatusCode::FORBIDDEN => assert_eq!(response.error(), "forbidden"),
            status => panic!("{uri} returned {status} for an account token"),
        }
    }
}

#[tokio::test]
async fn host_admin_routes_require_the_matching_scope_and_an_admin() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Policy::default()
        },
    )
    .await;
    let admin = h.create("Boss").await;
    let player = h.create("Player").await;

    for (method, uri, scope, wrong_scope) in [
        (Method::GET, INVENTORY, "admin.read", lxm::READ),
        (Method::POST, STOP, "admin.write", "admin.read"),
    ] {
        assert_unauthorized(
            &h.call(
                method.clone(),
                uri,
                &[h.bearer(&admin, wrong_scope)],
                Body::empty(),
            )
            .await,
        );
        let denied = h
            .call(method, uri, &[h.bearer(&player, scope)], Body::empty())
            .await;
        assert_eq!(denied.status, StatusCode::FORBIDDEN);
        assert_eq!(denied.error(), "forbidden");
    }

    let inventory = h
        .call(
            Method::GET,
            INVENTORY,
            &[h.bearer(&admin, "admin.read")],
            Body::empty(),
        )
        .await;
    assert_eq!(inventory.status, StatusCode::OK);
    assert_eq!(inventory.headers[header::CACHE_CONTROL], "private, no-store");
}

#[tokio::test]
async fn host_request_bodies_reject_invalid_shapes() {
    let h = harness().await;
    for (uri, body) in [
        (REGISTER, "{"),
        (REGISTER, r#"{"transport":"udp"}"#),
        (REPORT, r#"{"state":"joinable","players":2}"#),
        (REPORT, r#"{"state":"playing","players":-1}"#),
        (REPORT, r#"{"state":"playing"}"#),
        (REPORT, r#"{"state":"ended","players":0,"reason":"unknown"}"#),
    ] {
        let response = h.call(Method::POST, uri, &[], body).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{uri} {body}");
        assert_eq!(response.error(), "invalid");
    }
}

#[tokio::test]
async fn host_request_body_caps_are_route_specific() {
    let h = harness().await;
    for (uri, body, max) in [(REGISTER, ENDPOINT, 8 * 1024), (REPORT, PLAYING, 1024)] {
        let mut bytes = body.as_bytes().to_vec();
        bytes.resize(max, b' ');
        assert_unauthorized(&h.call(Method::POST, uri, &[], bytes.clone()).await);

        bytes.push(b' ');
        let response = h.call(Method::POST, uri, &[], bytes).await;
        assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE, "{uri}");
        assert_eq!(response.error(), "too_large");
    }
}

#[tokio::test]
async fn host_reads_share_the_api_read_limit() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            read_rate: (0.0, 1),
            ..Policy::default()
        },
    )
    .await;
    assert_unauthorized(&h.get(DETAIL).await);
    let response = h.get(INVENTORY).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error(), "rate_limited");
}

#[tokio::test]
async fn host_tunnel_route_returns_json_501_until_implemented() {
    let h = harness().await;
    let response = h.get(TUNNEL).await;
    assert_eq!(response.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(response.error(), "unimplemented");
}
