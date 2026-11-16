//! One-use, this-machine-only sign-in links (docs/live-admin.md §4).
//!
//! `h2 play --live` offers the browser entry of the account the game plays
//! as ([`Live::offer_local_signin`]) and prints a link carrying the token in
//! its fragment. The web app's `/signin` page trades the token for the
//! entry at `POST /v1/local/signin`. Offers live in memory only: nothing is
//! logged or persisted, a restart forgets them.
//!
//! The endpoint answers only a loopback peer that is not a proxy or tunnel
//! (cloudflared) forwarding someone else, and only once an offer has been made; anything else gets
//! the API's ordinary "no such endpoint". An unknown, used or expired token
//! all get the same 404.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use live_proto::ErrorCode;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::app::{ApiError, Live, not_found};
use crate::guard::RateLimiter;

/// How long an offer can be redeemed.
pub const OFFER_TTL: Duration = Duration::from_secs(10 * 60);
/// Outstanding offers kept; a new one beyond this drops the oldest.
pub const MAX_OFFERS: usize = 4;
/// The redeem body is `{"once": "<44 chars>"}`.
const MAX_REDEEM_BODY: usize = 1024;

struct Offer {
    digest: [u8; 32],
    entry: Value,
    expires: Instant,
}

#[derive(Default)]
struct Inner {
    /// Whether any offer was ever made: until then the endpoint is absent.
    offered: bool,
    offers: Vec<Offer>,
}

/// The outstanding offers, and a light limit on redeem attempts.
pub struct Offers {
    inner: Mutex<Inner>,
    attempts: RateLimiter,
}

impl Default for Offers {
    fn default() -> Self {
        Offers {
            inner: Mutex::default(),
            // A person clicking a link, not a guesser (256-bit tokens make
            // guessing moot anyway).
            attempts: RateLimiter::new(1.0, 10),
        }
    }
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Equal without an early exit.
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Offers {
    /// Keeps `entry` for one redeem within `ttl`; returns the token: 256
    /// random bits, base64url.
    pub fn offer(&self, entry: Value, ttl: Duration) -> String {
        let token = format!(
            "{}{}",
            live_proto::sign::random_id(),
            live_proto::sign::random_id()
        );
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        inner.offered = true;
        inner.offers.retain(|o| o.expires > now);
        while inner.offers.len() >= MAX_OFFERS {
            inner.offers.remove(0);
        }
        inner.offers.push(Offer {
            digest: digest(&token),
            entry,
            expires: now + ttl,
        });
        token
    }

    pub fn offered(&self) -> bool {
        self.inner.lock().unwrap().offered
    }

    /// The entry offered under `token`, once; `None` for an unknown, used or
    /// expired one.
    pub fn redeem(&self, token: &str) -> Option<Value> {
        self.redeem_at(token, Instant::now())
    }

    fn redeem_at(&self, token: &str, now: Instant) -> Option<Value> {
        let want = digest(token);
        let mut inner = self.inner.lock().unwrap();
        // Compare against every offer, so the time taken says nothing about
        // which (if any) matched.
        let mut hit = None;
        for (i, o) in inner.offers.iter().enumerate() {
            if ct_eq(&o.digest, &want) {
                hit = Some(i);
            }
        }
        let offer = inner.offers.remove(hit?);
        (offer.expires > now).then_some(offer.entry)
    }
}

impl Live {
    /// Offers `entry` (the browser's account entry, as `h2 live account
    /// export` writes it) for one redeem at `POST /v1/local/signin` from
    /// this machine within `ttl` ([`OFFER_TTL`]). Returns the token.
    pub fn offer_local_signin(&self, entry: Value, ttl: Duration) -> String {
        self.local_signin.offer(entry, ttl)
    }
}

fn no_store(mut resp: Response) -> Response {
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// From this machine, not via a proxy, and (when a browser says where the
/// request comes from) from this server's own pages.
fn local(peer: Option<SocketAddr>, headers: &HeaderMap) -> bool {
    if !crate::art::allowed(false, peer, headers) {
        return false;
    }
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    // A name that resolves here (DNS rebinding) is not this machine's page.
    if host.is_some_and(|h| !loopback_host(h)) {
        return false;
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let origin = origin.to_str().ok().and_then(|o| {
        o.strip_prefix("http://")
            .or_else(|| o.strip_prefix("https://"))
    });
    matches!((origin, host), (Some(o), Some(h)) if o.eq_ignore_ascii_case(h))
}

/// `127.0.0.1:port`, `localhost:port`, `[::1]:port` (and the like).
fn loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(n, _)| n),
    };
    name.eq_ignore_ascii_case("localhost")
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
}

/// `POST /v1/local/signin` `{"once": token}`: the offered entry, once.
pub async fn redeem(State(live): State<Arc<Live>>, req: Request) -> Response {
    no_store(match redeem_inner(&live, req).await {
        Ok(entry) => (StatusCode::OK, axum::Json(entry)).into_response(),
        Err(e) => e.into_response(),
    })
}

async fn redeem_inner(live: &Live, req: Request) -> Result<Value, ApiError> {
    let offers = &live.local_signin;
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    if !offers.offered() || req.method() != Method::POST || !local(peer, req.headers()) {
        return Err(not_found("no such endpoint"));
    }
    if !offers.attempts.allow("local-signin") {
        return Err(ApiError::new(ErrorCode::RateLimited, "slow down"));
    }
    // JSON only: a cross-origin page cannot send it without a preflight,
    // which the CORS layer refuses for anything but reads.
    if !is_json(req.headers()) {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "the body must be application/json",
        ));
    }
    let body = axum::body::to_bytes(req.into_body(), MAX_REDEEM_BODY)
        .await
        .map_err(|_| ApiError::new(ErrorCode::Invalid, "bad body"))?;
    let token = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("once")?.as_str().map(str::to_string))
        .ok_or_else(|| ApiError::new(ErrorCode::Invalid, "the body must be {\"once\": token}"))?;
    offers
        .redeem(&token)
        .ok_or_else(|| not_found("this sign-in link is unknown, used or expired"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn offers_redeem_once_and_expire() {
        let o = Offers::default();
        assert!(!o.offered());
        let t = o.offer(json!({"a": 1}), OFFER_TTL);
        assert!(o.offered());
        assert_eq!(t.len(), 44, "two 16-byte ids, base64url");
        assert!(o.redeem("nope").is_none());
        assert_eq!(o.redeem(&t), Some(json!({"a": 1})));
        assert!(o.redeem(&t).is_none(), "used");

        let t = o.offer(json!(2), Duration::from_secs(5));
        assert!(
            o.redeem_at(&t, Instant::now() + Duration::from_secs(6))
                .is_none()
        );
        assert!(o.redeem(&t).is_none(), "an expired offer is gone too");
    }

    #[test]
    fn at_most_a_few_outstanding() {
        let o = Offers::default();
        let tokens: Vec<_> = (0..MAX_OFFERS + 1)
            .map(|i| o.offer(json!(i), OFFER_TTL))
            .collect();
        assert!(o.redeem(&tokens[0]).is_none(), "the oldest was dropped");
        for (i, t) in tokens.iter().enumerate().skip(1) {
            assert_eq!(o.redeem(t), Some(json!(i)));
        }
    }

    #[test]
    fn only_this_machine_and_its_own_pages() {
        let lo: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.20:5000".parse().unwrap();
        let mut h = HeaderMap::new();
        assert!(local(Some(lo), &h));
        assert!(!local(Some(lan), &h));
        assert!(!local(None, &h));
        h.insert(header::HOST, "127.0.0.1:5000".parse().unwrap());
        h.insert(header::ORIGIN, "http://127.0.0.1:5000".parse().unwrap());
        assert!(local(Some(lo), &h));
        h.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!local(Some(lo), &h));
        h.insert(header::ORIGIN, "null".parse().unwrap());
        assert!(!local(Some(lo), &h));
        h.remove(header::ORIGIN);
        h.insert(header::HOST, "rebound.example:5000".parse().unwrap());
        assert!(!local(Some(lo), &h), "DNS rebinding");
        for ok in ["localhost:5000", "[::1]:5000", "127.0.0.1"] {
            h.insert(header::HOST, ok.parse().unwrap());
            assert!(local(Some(lo), &h), "{ok}");
        }
    }

    #[test]
    fn a_tunnelled_request_is_not_this_machine() {
        // cloudflared connects from loopback; what it adds gives it away,
        // even when the origin's Host is kept loopback.
        let lo: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let cloudflared = [
            ("cf-connecting-ip", "203.0.113.9"),
            ("cf-ray", "8f1e2d3c4b5a6978-LHR"),
            ("cf-visitor", "{\"scheme\":\"https\"}"),
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-proto", "https"),
        ];
        for (name, value) in cloudflared {
            let mut h = HeaderMap::new();
            h.insert(header::HOST, "127.0.0.1:5000".parse().unwrap());
            h.insert(name, value.parse().unwrap());
            assert!(!local(Some(lo), &h), "{name}");
        }
        let mut h = HeaderMap::new();
        h.insert(
            header::HOST,
            "happy-otter-1234.trycloudflare.com".parse().unwrap(),
        );
        assert!(!local(Some(lo), &h), "the tunnel's own host name");
    }
}
