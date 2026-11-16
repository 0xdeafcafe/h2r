//! `POST /v1/local/signin` through the router: the one-use, this-machine-only
//! sign-in link `h2 play --live` prints (docs/live/live-admin.md §4).

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use live_server::local_signin::OFFER_TTL;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Out {
    status: StatusCode,
    cache: Option<String>,
    body: Value,
}

async fn post(
    h: &common::Harness,
    peer: Option<&str>,
    headers: &[(&str, &str)],
    body: Value,
) -> Out {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/v1/local/signin")
        .header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut req = req.body(Body::from(body.to_string())).unwrap();
    if let Some(p) = peer {
        req.extensions_mut()
            .insert(ConnectInfo(p.parse::<SocketAddr>().unwrap()));
    }
    let resp = h.app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let cache = resp
        .headers()
        .get("cache-control")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    Out {
        status,
        cache,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

const LO: Option<&str> = Some("127.0.0.1:50000");

fn entry() -> Value {
    json!({"xuid": "0009000000000042", "gamertag": "afr", "key": "secret"})
}

#[tokio::test]
async fn absent_until_an_offer_is_made() {
    let h = common::harness().await;
    let r = post(&h, LO, &[], json!({"once": "x"})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.body["message"], "no such endpoint");
}

#[tokio::test]
async fn a_loopback_peer_redeems_once() {
    let h = common::harness().await;
    let token = h.live.offer_local_signin(entry(), OFFER_TTL);
    let r = post(&h, LO, &[], json!({ "once": token })).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, entry());
    assert_eq!(r.cache.as_deref(), Some("no-store"));

    // Used: gone, with the same answer as a wrong token.
    let again = post(&h, LO, &[], json!({ "once": token })).await;
    let wrong = post(&h, LO, &[], json!({"once": "A".repeat(44)})).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    assert_eq!(again.body, wrong.body);
    assert_eq!(wrong.status, StatusCode::NOT_FOUND);
    assert_eq!(again.cache.as_deref(), Some("no-store"));
}

#[tokio::test]
async fn an_expired_link_answers_like_a_wrong_one() {
    let h = common::harness().await;
    let token = h
        .live
        .offer_local_signin(entry(), Duration::from_millis(20));
    tokio::time::sleep(Duration::from_millis(40)).await;
    let r = post(&h, LO, &[], json!({ "once": token })).await;
    let wrong = post(&h, LO, &[], json!({"once": "nope"})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.body, wrong.body);
    assert_eq!(
        r.body["message"],
        "this sign-in link is unknown, used or expired"
    );
}

#[tokio::test]
async fn other_machines_and_proxies_never_see_it() {
    let h = common::harness().await;
    let token = h.live.offer_local_signin(entry(), OFFER_TTL);
    for (peer, headers) in [
        (Some("192.168.1.20:50000"), vec![]),
        (None, vec![]),
        (LO, vec![("x-forwarded-for", "203.0.113.9")]),
        (LO, vec![("origin", "https://evil.example")]),
        (LO, vec![("host", "rebound.example:8074")]),
    ] {
        let r = post(&h, peer, &headers, json!({ "once": token })).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{peer:?} {headers:?}");
        assert_eq!(r.body["message"], "no such endpoint");
    }
    // None of those consumed it; this server's own page still can.
    let r = post(
        &h,
        LO,
        &[
            ("host", "127.0.0.1:8074"),
            ("origin", "http://127.0.0.1:8074"),
        ],
        json!({ "once": token }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn json_only_and_rate_limited() {
    let h = common::harness().await;
    let token = h.live.offer_local_signin(entry(), OFFER_TTL);
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/v1/local/signin")
        .header("content-type", "text/plain")
        .body(Body::from(json!({ "once": token }).to_string()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:1".parse::<SocketAddr>().unwrap()));
    let resp = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let mut limited = false;
    for _ in 0..20 {
        if post(&h, LO, &[], json!({"once": "guess"})).await.status == StatusCode::TOO_MANY_REQUESTS
        {
            limited = true;
            break;
        }
    }
    assert!(limited, "guessing is slowed down");
}
