//! One log line for every refused request (docs/live/live-worker.md,
//! "Observability").
//!
//! The line names the method, the path (ids kept, query string dropped),
//! the status, and the error code and message of the [`ApiError`] body
//! (`{"error":"…","message":"…"}`), so a log can tell "no such endpoint"
//! from "no session of yours by that id". It never holds headers, tokens,
//! query strings or request bodies.
//!
//! The Worker calls [`log_refusals`] around its fetch handler with
//! `console_log!`/`console_error!`; `h2 live serve` layers [`native_layer`]
//! over the router and writes the same line to stderr.
//!
//! [`ApiError`]: crate::app::ApiError

use axum::body::{Body, to_bytes};
use axum::http::Response;

/// The longest message kept.
pub const MAX_MESSAGE: usize = 300;
/// The most of an error body read; a longer body is not an `ApiError`.
const MAX_BODY: usize = 16 * 1024;

/// The line for one response, or `None` when it is not a refusal (status
/// below 400). `path` may carry a query string; it is cut. A body that is
/// not an `ApiError` JSON object gives the line without a code or message.
pub fn refusal_line(method: &str, path: &str, status: u16, body: &[u8]) -> Option<String> {
    if status < 400 {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut line = format!("refused {method} {path} {status}");
    if let Ok(serde_json::Value::Object(o)) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(code) = o.get("error").and_then(|v| v.as_str()) {
            line.push_str(&format!(" code={code}"));
        }
        if let Some(message) = o.get("message").and_then(|v| v.as_str()) {
            line.push_str(&format!(" message={:?}", truncate(message)));
        }
    }
    Some(line)
}

fn truncate(s: &str) -> String {
    match s.char_indices().nth(MAX_MESSAGE) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_string(),
    }
}

/// Passes `response` through, calling `sink(status, line)` first when it is
/// a refusal. Only an error response's body is read (up to 16 KiB) and it
/// is put back unchanged; one that is too large or fails to read is
/// logged without a body.
pub async fn log_refusals(
    method: &str,
    path: &str,
    response: Response<Body>,
    sink: impl FnOnce(u16, &str),
) -> Response<Body> {
    let status = response.status().as_u16();
    if status < 400 {
        return response;
    }
    let (parts, body) = response.into_parts();
    let (body, bytes) = match to_bytes(body, MAX_BODY).await {
        Ok(bytes) => (Body::from(bytes.clone()), bytes),
        Err(_) => (Body::empty(), Default::default()),
    };
    if let Some(line) = refusal_line(method, path, status, &bytes) {
        sink(status, &line);
    }
    Response::from_parts(parts, body)
}

/// The native server's layer: the same line on stderr.
#[cfg(feature = "native")]
pub async fn native_layer(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let response = next.run(req).await;
    log_refusals(&method, &path, response, |_, line| {
        eprintln!("live: {line}")
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_body_gives_code_and_message() {
        let body = br#"{"error":"not_found","message":"no such session"}"#;
        assert_eq!(
            refusal_line("POST", "/v1/games/abc/bots", 404, body).as_deref(),
            Some(r#"refused POST /v1/games/abc/bots 404 code=not_found message="no such session""#)
        );
    }

    #[test]
    fn query_string_is_dropped() {
        let line = refusal_line("GET", "/v1/x/7?token=secret&a=1", 401, b"{}").unwrap();
        assert_eq!(line, "refused GET /v1/x/7 401");
        assert!(!line.contains("secret"));
    }

    #[test]
    fn non_json_and_non_error_bodies_keep_the_status() {
        assert_eq!(
            refusal_line("GET", "/a", 502, b"<html>bad gateway</html>").as_deref(),
            Some("refused GET /a 502")
        );
        assert_eq!(
            refusal_line("GET", "/a", 500, b"").as_deref(),
            Some("refused GET /a 500")
        );
        assert_eq!(
            refusal_line("GET", "/a", 400, b"[1]").as_deref(),
            Some("refused GET /a 400")
        );
    }

    #[test]
    fn success_and_redirects_are_not_logged() {
        for status in [200, 204, 301, 304] {
            assert_eq!(refusal_line("GET", "/a", status, b"{}"), None);
        }
    }

    #[test]
    fn long_message_is_truncated() {
        let body = format!(r#"{{"error":"invalid","message":"{}"}}"#, "é".repeat(1000));
        let line = refusal_line("PUT", "/a", 400, body.as_bytes()).unwrap();
        assert!(line.ends_with("…\""));
        assert!(line.chars().count() < 400);
    }

    #[test]
    fn body_is_put_back_and_sink_called_once() {
        let response = Response::builder()
            .status(404)
            .body(Body::from(
                r#"{"error":"not_found","message":"no such endpoint"}"#,
            ))
            .unwrap();
        let mut seen = Vec::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let response = rt.block_on(log_refusals("GET", "/v1/nope?q=1", response, |s, l| {
            seen.push((s, l.to_string()));
        }));
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, 404);
        assert!(seen[0].1.contains("code=not_found"));
        let bytes = rt.block_on(to_bytes(response.into_body(), 1024)).unwrap();
        assert!(bytes.starts_with(b"{\"error\""));
    }
}
