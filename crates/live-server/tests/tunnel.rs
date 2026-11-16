//! Serving through cloudflared (docs/live/live-hosting.md): the tunnel connects
//! from loopback and adds its headers, so this-machine-only routes must
//! refuse what it delivers; the public URL shows in discovery.

mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use live_server::art::GameArt;
use live_server::http::{self, Site};
use live_server::local_signin::OFFER_TTL;
use serde_json::json;
use tower::ServiceExt;

/// What cloudflared adds to every request it forwards.
const CLOUDFLARED: &[(&str, &str)] = &[
    ("cf-connecting-ip", "203.0.113.9"),
    ("cf-ray", "8f1e2d3c4b5a6978-LHR"),
    ("cf-visitor", r#"{"scheme":"https"}"#),
    ("cf-ipcountry", "GB"),
    ("x-forwarded-for", "203.0.113.9"),
    ("x-forwarded-proto", "https"),
];

fn from_loopback(mut req: Request<Body>) -> Request<Body> {
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50123".parse::<SocketAddr>().unwrap(),
    ));
    req
}

#[tokio::test]
async fn game_art_is_refused_through_the_tunnel() {
    let art = common::temp_dir();
    std::fs::create_dir_all(&art).unwrap();
    std::fs::write(art.join("manifest.json"), r#"{"version":1}"#).unwrap();
    let h = common::harness().await;
    let site = Site {
        web: None,
        game_art: GameArt::found(&art, false),
    };
    let app = http::router_with_site(h.live.clone(), &site);
    let get = |headers: &[(&str, &str)]| {
        let mut req = Request::get("/art/game/manifest.json");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = from_loopback(req.body(Body::empty()).unwrap());
        let app = app.clone();
        async move { app.oneshot(req).await.unwrap().status() }
    };
    assert_eq!(get(&[]).await, StatusCode::OK, "this machine");
    assert_eq!(get(CLOUDFLARED).await, StatusCode::NOT_FOUND);
    for one in CLOUDFLARED {
        assert_eq!(
            get(std::slice::from_ref(one)).await,
            StatusCode::NOT_FOUND,
            "{one:?}"
        );
    }
}

#[tokio::test]
async fn local_signin_is_refused_through_the_tunnel() {
    let h = common::harness().await;
    let token = h
        .live
        .offer_local_signin(json!({"gamertag": "afr"}), OFFER_TTL);
    let post = |headers: &[(&str, &str)]| {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/v1/local/signin")
            .header("content-type", "application/json")
            .header("host", "127.0.0.1:8074");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = from_loopback(
            req.body(Body::from(json!({ "once": token }).to_string()))
                .unwrap(),
        );
        let app = h.app.clone();
        async move { app.oneshot(req).await.unwrap().status() }
    };
    assert_eq!(post(CLOUDFLARED).await, StatusCode::NOT_FOUND);
    for one in CLOUDFLARED {
        assert_eq!(
            post(std::slice::from_ref(one)).await,
            StatusCode::NOT_FOUND,
            "{one:?}"
        );
    }
    // The tunnel's own host name, with no proxy headers at all.
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/v1/local/signin")
        .header("content-type", "application/json")
        .header("host", "friendly-test-host.trycloudflare.com")
        .body(Body::from(json!({ "once": token }).to_string()))
        .unwrap();
    req = from_loopback(req);
    assert_eq!(
        h.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    // None of those used it up.
    assert_eq!(post(&[]).await, StatusCode::OK);
}

#[tokio::test]
async fn discovery_shows_the_public_url_once_set() {
    let h = common::harness().await;
    let wk = h.get("/.well-known/h2live.json").await.json();
    assert!(wk.get("url").is_none(), "absent until known: {wk}");
    let public = &h.live.policy().public;
    public.set("https://friendly-test-host.trycloudflare.com", true);
    let wk = h.get("/.well-known/h2live.json").await.json();
    assert_eq!(wk["url"], "https://friendly-test-host.trycloudflare.com");
    // The id (tokens' `aud`, author ids) and the API path do not move.
    assert_eq!(wk["id"], "live.test");
    assert_eq!(wk["api"], "/v1");
    // A restarted quick tunnel's new URL replaces it.
    public.set("https://another-test-host.trycloudflare.com", true);
    let wk = h.get("/.well-known/h2live.json").await.json();
    assert_eq!(wk["url"], "https://another-test-host.trycloudflare.com");
}
