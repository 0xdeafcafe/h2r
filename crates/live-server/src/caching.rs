//! How long responses may be kept, and by whom (`Cache-Control`), so the
//! CDN in front of the server (Cloudflare, through the tunnel) answers
//! what it can without a round trip to the origin.
//!
//! - **Built assets** (`/assets/*`, named by their content's hash):
//!   `public, max-age=31536000, immutable`. A new build has new names.
//! - **The app's HTML** (`index.html`, and every app route it answers):
//!   `no-cache`, so a new build is picked up at once (it is 1 KB; the
//!   check is a 304).
//! - **Anonymous public reads** (`GET /v1/` of [`PUBLIC_READS`], 2xx, no
//!   `Authorization`): [`PUBLIC`], a few seconds at the edge and served
//!   stale while it refetches, with an `ETag` (a matching `If-None-Match`
//!   gets a 304). An anonymous answer is the same for every anonymous
//!   reader, so sharing it shows no one anything they could not fetch.
//! - **Everything else under `/v1/`**: `private, no-store`. That covers
//!   every request with a token (the answer may hold friends-only games
//!   and presence; the web app also puts `auth=1` on such reads so a CDN
//!   keyed on the URL never hands a reader the anonymous copy), every
//!   error, and every other route.
//!
//! A handler that sets its own `Cache-Control` (title files, art, cloud
//! saves) keeps it.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
pub const REVALIDATE: &str = "no-cache";
pub const PUBLIC: &str = "public, max-age=0, s-maxage=5, stale-while-revalidate=30";
pub const PRIVATE: &str = "private, no-store";

/// The `/v1/` reads an anonymous reader may get from a shared cache: the
/// first path segment.
pub const PUBLIC_READS: &[&str] = &[
    "snapshot", "games", "accounts", "presence", "stats", "events", "teams", "maps",
];

/// The largest public read buffered for its `ETag`; a bigger one goes out
/// without one.
const ETAG_MAX: usize = 8 << 20;

/// What a response to `GET path` may be kept as, if this layer decides.
pub fn policy(
    path: &str,
    authorized: bool,
    status: StatusCode,
    html: bool,
) -> Option<&'static str> {
    if status == StatusCode::SWITCHING_PROTOCOLS {
        return None;
    }
    if let Some(rest) = path.strip_prefix("/v1/") {
        let first = rest.split('/').next().unwrap_or("");
        let public = !authorized && status.is_success() && PUBLIC_READS.contains(&first);
        return Some(if public { PUBLIC } else { PRIVATE });
    }
    // Not the app's HTML, which answers a missing asset too (a year of
    // that under a script's name would break every page that asks).
    if path.starts_with("/assets/") && status.is_success() && !html {
        return Some(IMMUTABLE);
    }
    (html && !path.starts_with("/art/")).then_some(REVALIDATE)
}

/// The middleware: sets [`policy`] on GET and HEAD responses that have no
/// `Cache-Control` of their own, and an `ETag` on public reads.
pub async fn layer(req: Request, next: Next) -> Response {
    let read = matches!(*req.method(), Method::GET | Method::HEAD);
    let path = req.uri().path().to_owned();
    let authorized =
        req.headers().contains_key(header::AUTHORIZATION) || has_auth_param(req.uri().query());
    let if_none_match = req.headers().get(header::IF_NONE_MATCH).cloned();
    let mut resp = next.run(req).await;
    if !read || resp.headers().contains_key(header::CACHE_CONTROL) {
        return resp;
    }
    let html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|t| t.as_bytes().starts_with(b"text/html"));
    let Some(policy) = policy(&path, authorized, resp.status(), html) else {
        return resp;
    };
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    if policy != PUBLIC || resp.status() != StatusCode::OK {
        return resp;
    }
    // A public read: tag it by its bytes.
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, ETAG_MAX).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let digest = Sha256::digest(&bytes);
    let tag = format!("W/\"{}\"", hex16(&digest));
    let tag = HeaderValue::from_str(&tag).expect("hex is a header value");
    if if_none_match.is_some_and(|m| matches(&m, &tag)) {
        let mut r = StatusCode::NOT_MODIFIED.into_response();
        r.headers_mut().insert(header::ETAG, tag);
        r.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(PUBLIC));
        return r;
    }
    parts.headers.insert(header::ETAG, tag);
    Response::from_parts(parts, Body::from(bytes))
}

/// `auth=1` (or any value but `0`) in a query: the web app's mark on a read
/// it sends with a token.
fn has_auth_param(query: Option<&str>) -> bool {
    query.is_some_and(|q| {
        q.split('&').any(|kv| {
            kv.split_once('=')
                .map_or(kv == "auth", |(k, v)| k == "auth" && v != "0")
        })
    })
}

fn hex16(digest: &[u8]) -> String {
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether an `If-None-Match` names `tag` (or is `*`).
fn matches(if_none_match: &HeaderValue, tag: &HeaderValue) -> bool {
    let Ok(list) = if_none_match.to_str() else {
        return false;
    };
    let ours = tag.to_str().unwrap_or("").trim_start_matches("W/");
    list.split(',')
        .map(str::trim)
        .any(|t| t == "*" || t.trim_start_matches("W/") == ours)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_response_gets_its_policy() {
        let ok = StatusCode::OK;
        assert_eq!(
            policy("/assets/index-Bl7_M9u5.js", false, ok, false),
            Some(IMMUTABLE)
        );
        assert_eq!(
            policy("/assets/missing.js", false, StatusCode::NOT_FOUND, false),
            None
        );
        // The app's HTML standing in for a missing asset.
        assert_eq!(
            policy("/assets/missing.js", false, ok, true),
            Some(REVALIDATE)
        );
        assert_eq!(policy("/", false, ok, true), Some(REVALIDATE));
        assert_eq!(policy("/stats/games", false, ok, true), Some(REVALIDATE));
        assert_eq!(policy("/v1/games", false, ok, false), Some(PUBLIC));
        assert_eq!(policy("/v1/snapshot", false, ok, false), Some(PUBLIC));
        assert_eq!(policy("/v1/stats/summary", false, ok, false), Some(PUBLIC));
        // With a token, failing, or not a public read: never shared.
        assert_eq!(policy("/v1/games", true, ok, false), Some(PRIVATE));
        assert_eq!(
            policy("/v1/games/x", false, StatusCode::NOT_FOUND, false),
            Some(PRIVATE)
        );
        assert_eq!(policy("/v1/sessions", false, ok, false), Some(PRIVATE));
        assert_eq!(policy("/v1/admin/server", false, ok, false), Some(PRIVATE));
        assert_eq!(
            policy("/v1/stream", false, StatusCode::SWITCHING_PROTOCOLS, false),
            None
        );
        // Art and other files keep their own (or the default).
        assert_eq!(policy("/art/game/manifest.json", false, ok, false), None);
        assert_eq!(policy("/favicon.svg", false, ok, false), None);
    }

    #[test]
    fn the_auth_mark_is_read_from_the_query() {
        assert!(has_auth_param(Some("auth=1")));
        assert!(has_auth_param(Some("limit=5&auth=1")));
        assert!(has_auth_param(Some("auth")));
        assert!(!has_auth_param(Some("auth=0")));
        assert!(!has_auth_param(Some("author=1")));
        assert!(!has_auth_param(None));
    }
}
