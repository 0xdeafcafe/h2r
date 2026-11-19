//! What the fetch handler does when a call to a Durable Object throws
//! (docs/live/live-worker.md "Deploys and retries").
//!
//! A deploy resets every Durable Object: calls in flight throw ("Durable
//! Object reset because its code was updated", "Network connection lost"),
//! with `retryable` set on the thrown error. Such a call is made again, a
//! couple of times with a short pause, if the request is safe to repeat; if
//! it still fails, or may not be repeated, the caller is told 503 with a
//! `Retry-After`, not a bare 500, so a client knows to come back. An
//! overloaded object is not called again at once: that would only add load.
//!
//! The decisions are here, in plain Rust, for tests on this machine;
//! `entry` makes the calls.

use std::time::Duration;

/// Calls in all, the first included.
pub const ATTEMPTS: u32 = 3;

/// The `Retry-After` (seconds) of a 503 for an object that is restarting
/// or overloaded.
pub const RETRY_AFTER_S: u64 = 2;

/// Messages the runtime throws when an object was reset under a call; the
/// `retryable` property is the first test, these the fallback.
const RESET: [&str; 3] = [
    "code was updated",
    "Network connection lost",
    "Durable Object reset",
];

/// What a call to a Durable Object threw.
#[derive(Debug, Clone, Default)]
pub struct Thrown {
    /// The error's `retryable` property.
    pub retryable: Option<bool>,
    /// The error's `overloaded` property.
    pub overloaded: Option<bool>,
    pub message: String,
}

/// What to do about a throw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Call again after this pause.
    Retry(Duration),
    /// Answer 503 with [`RETRY_AFTER_S`].
    Unavailable,
    /// Not a passing failure: let it surface as before.
    Fail,
}

impl Thrown {
    fn overloaded(&self) -> bool {
        self.overloaded == Some(true) || self.message.contains("overloaded")
    }

    /// Whether the object may answer if called again: it was reset (a
    /// deploy) or the connection to it dropped.
    pub fn transient(&self) -> bool {
        !self.overloaded()
            && (self.retryable == Some(true) || RESET.iter().any(|m| self.message.contains(m)))
    }
}

/// Whether a request may be sent to the object twice: it reads, or it
/// writes the same thing again (PUT, DELETE, a heartbeat), or it carries
/// signed events, which the hub takes once by their nonce.
pub fn repeatable(method: &str, path: &str) -> bool {
    match method {
        // Upgrades consume the account token's nonce at the hub.
        "GET" if crate::relayroom::session_of(path).is_some() => false,
        "GET" | "HEAD" | "OPTIONS" | "PUT" | "DELETE" => true,
        "POST" => {
            path == "/v1/events"
                || path == "/v1/presence/ping"
                || path
                    .strip_prefix("/v1/sessions/")
                    .and_then(|rest| rest.strip_suffix("/events"))
                    .is_some_and(|id| !id.is_empty() && !id.contains('/'))
        }
        _ => false,
    }
}

/// The largest body kept for a second call. A retry needs its own copy of
/// the body, made by teeing the request, and the runtime holds every byte
/// of the branch kept back in the isolate's memory (128 MB) until the
/// first call finishes: a 95 MiB content upload would not fit beside the
/// rest. Title files (2 MiB) and cloud saves (about 4 MiB) are under it.
pub const KEEP_BODY_BYTES: u64 = 8 << 20;

/// Whether a request's body may be kept for a retry: it has none, or says
/// it is at most [`KEEP_BODY_BYTES`]. One without a length (streamed in
/// chunks) could be any size, so it is sent once.
pub fn keeps_body(has_body: bool, content_length: Option<u64>) -> bool {
    !has_body || content_length.is_some_and(|n| n <= KEEP_BODY_BYTES)
}

/// What to do after call `attempt` (1-based) of a `method` `path` request
/// threw `thrown`.
pub fn decide(thrown: &Thrown, method: &str, path: &str, attempt: u32) -> Outcome {
    if thrown.overloaded() {
        return Outcome::Unavailable;
    }
    if !thrown.transient() {
        return Outcome::Fail;
    }
    if attempt < ATTEMPTS && repeatable(method, path) {
        // 100 ms, then 400 ms: a reset object is back within a second.
        Outcome::Retry(Duration::from_millis(100 << (2 * (attempt - 1))))
    } else {
        Outcome::Unavailable
    }
}

/// [`decide`] for a request whose body was or was not kept
/// ([`keeps_body`]): one sent without a copy cannot be sent again, so a
/// retry becomes the 503, and the client sends it again itself.
pub fn decide_kept(thrown: &Thrown, method: &str, path: &str, attempt: u32, kept: bool) -> Outcome {
    match decide(thrown, method, path, attempt) {
        Outcome::Retry(_) if !kept => Outcome::Unavailable,
        o => o,
    }
}

/// The body of a 503 for an object that could not be reached.
pub fn unavailable_body(message: &str) -> String {
    serde_json::to_string(&live_proto::ErrorBody::new(
        live_proto::ErrorCode::Unavailable,
        format!("the server is restarting, try again: {message}"),
    ))
    .expect("an error body serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thrown(retryable: Option<bool>, overloaded: Option<bool>, message: &str) -> Thrown {
        Thrown {
            retryable,
            overloaded,
            message: message.into(),
        }
    }

    #[test]
    fn a_deploy_reset_is_retried_with_a_growing_pause_then_503() {
        let reset = thrown(
            Some(true),
            None,
            "Durable Object reset because its code was updated.",
        );
        assert_eq!(
            decide(&reset, "GET", "/v1/presence", 1),
            Outcome::Retry(Duration::from_millis(100))
        );
        assert_eq!(
            decide(&reset, "GET", "/v1/presence", 2),
            Outcome::Retry(Duration::from_millis(400))
        );
        assert_eq!(
            decide(&reset, "GET", "/v1/presence", ATTEMPTS),
            Outcome::Unavailable
        );
    }

    #[test]
    fn the_message_is_the_fallback_when_the_property_is_missing() {
        for message in [
            "Durable Object reset because its code was updated.",
            "Network connection lost.",
        ] {
            assert!(thrown(None, None, message).transient(), "{message}");
        }
        assert!(!thrown(None, None, "TypeError: x is undefined").transient());
        assert!(!thrown(Some(false), None, "nope").transient());
    }

    #[test]
    fn an_overloaded_object_is_not_called_again_but_told_to_come_back() {
        let busy = thrown(Some(true), Some(true), "Durable Object is overloaded.");
        assert!(!busy.transient());
        assert_eq!(
            decide(&busy, "GET", "/v1/presence", 1),
            Outcome::Unavailable
        );
        let by_message = thrown(
            None,
            None,
            "Durable Object is overloaded. Too many requests",
        );
        assert_eq!(
            decide(&by_message, "GET", "/v1/presence", 1),
            Outcome::Unavailable
        );
    }

    #[test]
    fn a_write_that_may_not_be_repeated_is_answered_503_at_once() {
        let reset = thrown(Some(true), None, "Network connection lost.");
        for (method, path) in [
            ("POST", "/v1/sessions"),
            ("POST", "/v1/accounts"),
            ("POST", "/v1/arbitration/register"),
            ("POST", "/v1/accounts/by-xuid/0009000000000001/report"),
        ] {
            assert_eq!(
                decide(&reset, method, path, 1),
                Outcome::Unavailable,
                "{method} {path}"
            );
        }
    }

    #[test]
    fn relay_upgrade_is_not_retried_with_a_spent_nonce() {
        assert!(!repeatable("GET", "/v1/sessions/0123456789abcdef/relay"));
        assert!(repeatable("GET", "/v1/sessions/0123456789abcdef"));
    }

    #[test]
    fn reads_puts_deletes_heartbeats_and_signed_events_are_repeatable() {
        for (method, path) in [
            ("GET", "/v1/sessions?state=open"),
            ("PUT", "/v1/accounts/by-xuid/1/mutes/2"),
            ("DELETE", "/v1/accounts/by-xuid/1/mutes/2"),
            ("POST", "/v1/presence/ping"),
            ("POST", "/v1/events"),
            ("POST", "/v1/sessions/0123456789abcdef/events"),
        ] {
            assert!(repeatable(method, path), "{method} {path}");
        }
        assert!(!repeatable("POST", "/v1/sessions//events"));
        assert!(!repeatable("POST", "/v1/sessions/a/b/events"));
        assert!(!repeatable("PATCH", "/v1/events"));
    }

    #[test]
    fn a_big_or_unsized_body_is_sent_once_and_a_reset_under_it_is_a_503() {
        assert!(keeps_body(false, None), "a GET: nothing to keep");
        assert!(keeps_body(true, Some(0)));
        assert!(keeps_body(true, Some(KEEP_BODY_BYTES)));
        assert!(!keeps_body(true, Some(KEEP_BODY_BYTES + 1)));
        assert!(!keeps_body(true, None), "chunked: any size");
        let reset = thrown(Some(true), None, "Network connection lost.");
        let path = "/v1/content/00";
        assert_eq!(
            decide_kept(&reset, "PUT", path, 1, true),
            Outcome::Retry(Duration::from_millis(100))
        );
        assert_eq!(
            decide_kept(&reset, "PUT", path, 1, false),
            Outcome::Unavailable
        );
        // Not a reset: as before, kept or not.
        let bug = thrown(None, None, "RangeError: out of bounds");
        assert_eq!(decide_kept(&bug, "PUT", path, 1, false), Outcome::Fail);
    }

    #[test]
    fn any_other_failure_surfaces_as_before() {
        let bug = thrown(None, None, "RangeError: out of bounds");
        assert_eq!(decide(&bug, "GET", "/v1/presence", 1), Outcome::Fail);
    }

    #[test]
    fn the_503_body_is_the_apis_error_shape() {
        let body: serde_json::Value =
            serde_json::from_str(&unavailable_body("Network connection lost.")).unwrap();
        assert_eq!(body["error"], "unavailable");
        assert!(body["message"].as_str().unwrap().contains("restarting"));
    }
}
