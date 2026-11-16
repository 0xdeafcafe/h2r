//! Invites made by admin accounts from the browser (docs/live-admin.md §5,
//! docs/live-security.md §3).
//!
//! The operator mints invites with the server key (`h2 live invite
//! create`, [`crate::Live::create_invite`]); nothing about those is logged.
//! An admin account cannot hold the server key, so it asks the server to
//! mint one with a request it signs itself:
//!
//! - an `invites.write` token, self-issued by the admin's account key, and
//! - as the body, an [`CREATED`] event on the [`STREAM`] stream, signed by
//!   the same key: `{"jti": <its id>, "exp": <unix seconds>}`.
//!
//! The server checks the admin, the rate limit and the event like any
//! signed write, appends the admin's JWS untouched (the audit trail names
//! and is signed by whoever asked), and answers the invite: the same
//! server-signed code the operator's command makes, with that `jti` and
//! `exp`, so account creation verifies it exactly as before. Ed25519 is
//! deterministic, so the same `jti` and `exp` always give the same code;
//! [`list`] re-mints it for the invites still open.
//!
//! [`REVOKED`] (`{"jti"}`) ends one. At redemption an invite the log knows
//! must be open: not revoked, and its creator still an admin (revoking an
//! admin kills the invites it made). Invites the log does not know (the
//! operator's) are checked by their signature alone, as before.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::sign::{Invite, MAX_EVENT_SKEW_S, SignedEvent, TYP_INVITE, parse_time};
use live_proto::{Author, ErrorCode, Xuid, lxm, types};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::{ApiError, Live, forbidden, invalid, not_found, now, sign_error, unauthorized};
use crate::dispatch::Subscriber;
use crate::guard::RateLimiter;
use crate::model::{invite_key, nonce_key};
use crate::store::{NewEvent, RecordedEvent};

/// The stream admin invite events go on.
pub const STREAM: &str = "invites";
/// An admin made an invite: `{jti, exp}`, signed by the admin's key.
pub const CREATED: &str = "InviteCreated";
/// An admin ended an open invite: `{jti}`.
pub const REVOKED: &str = "InviteRevoked";
/// The token method for admin-only reads (`GET /v1/invites`,
/// `GET /v1/admin/status`).
pub const LXM_ADMIN_READ: &str = "admin.read";
/// The longest an admin's invite may last.
pub const MAX_DAYS: i64 = 30;
/// Per author: one invite a minute on average, ten at once.
pub const CREATE_RATE: (f64, u32) = (1.0 / 60.0, 10);

/// One invite the log knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub jti: String,
    /// The author id that made it.
    pub by: String,
    /// When (the event's `at`).
    pub at: String,
    pub exp: i64,
    pub position: u64,
    pub revoked: bool,
    pub used_by: Option<Xuid>,
}

impl Record {
    pub fn state(&self, now: i64) -> &'static str {
        if self.used_by.is_some() {
            "used"
        } else if self.revoked {
            "revoked"
        } else if self.exp < now {
            "expired"
        } else {
            "open"
        }
    }

    fn creator(&self) -> Option<Xuid> {
        match Author::parse(&self.by)? {
            Author::Account { xuid, .. } => Some(xuid),
            Author::Server(_) => None,
        }
    }
}

/// The invites the log knows, projected in memory, plus the admin
/// creation rate limit.
pub struct Invites {
    by_jti: RwLock<HashMap<String, Record>>,
    creates: RateLimiter,
    /// Serializes the check-then-append of invite writes.
    pub(crate) writes: tokio::sync::Mutex<()>,
}

impl Default for Invites {
    fn default() -> Self {
        Invites {
            by_jti: RwLock::default(),
            creates: RateLimiter::new(CREATE_RATE.0, CREATE_RATE.1),
            writes: Default::default(),
        }
    }
}

impl Invites {
    pub fn get(&self, jti: &str) -> Option<Record> {
        self.by_jti.read().unwrap().get(jti).cloned()
    }

    /// Every known invite, newest first.
    pub fn all(&self) -> Vec<Record> {
        let mut v: Vec<Record> = self.by_jti.read().unwrap().values().cloned().collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.position));
        v
    }

    /// How many are in each state.
    pub fn counts(&self, now: i64) -> Value {
        let mut c: HashMap<&str, u64> = HashMap::new();
        for r in self.by_jti.read().unwrap().values() {
            *c.entry(r.state(now)).or_default() += 1;
        }
        json!({
            "open": c.get("open").copied().unwrap_or(0),
            "used": c.get("used").copied().unwrap_or(0),
            "revoked": c.get("revoked").copied().unwrap_or(0),
            "expired": c.get("expired").copied().unwrap_or(0),
        })
    }
}

impl Subscriber for Invites {
    fn name(&self) -> &str {
        "invites"
    }

    fn apply(&self, event: &RecordedEvent) {
        let b = &event.body;
        let jti = b.data.get("jti").and_then(Value::as_str);
        match (b.stream.as_str(), b.kind.as_str(), jti) {
            (STREAM, CREATED, Some(jti)) => {
                let Some(exp) = b.data.get("exp").and_then(Value::as_i64) else {
                    return;
                };
                self.by_jti
                    .write()
                    .unwrap()
                    .entry(jti.to_string())
                    .or_insert(Record {
                        jti: jti.to_string(),
                        by: b.author.clone(),
                        at: b.at.clone(),
                        exp,
                        position: event.position,
                        revoked: false,
                        used_by: None,
                    });
            }
            (STREAM, REVOKED, Some(jti)) => {
                if let Some(r) = self.by_jti.write().unwrap().get_mut(jti) {
                    r.revoked = true;
                }
            }
            (_, types::ACCOUNT_CREATED, _) => {
                let (Some(jti), Some(xuid)) = (
                    b.data.get("invite").and_then(Value::as_str),
                    b.data
                        .get("xuid")
                        .and_then(Value::as_str)
                        .and_then(|s| Xuid::parse(s).ok()),
                ) else {
                    return;
                };
                if let Some(r) = self.by_jti.write().unwrap().get_mut(jti) {
                    r.used_by = Some(xuid);
                }
            }
            _ => {}
        }
    }
}

/// Whether an invite with this `jti` (its signature already checked) may
/// still be used: the log's invites must not be revoked and their creator
/// must still be an admin. Unknown ones (the operator's) may.
pub fn check_redeemable(live: &Live, jti: &str) -> Result<(), ApiError> {
    let Some(r) = live.invites.get(jti) else {
        return Ok(());
    };
    if r.revoked {
        return Err(forbidden("invite: revoked"));
    }
    if let Some(xuid) = r.creator()
        && !live.admins.is_admin_xuid(xuid, &live.model.read())
    {
        return Err(forbidden("invite: whoever made it is no longer an admin"));
    }
    Ok(())
}

/// The invite code for a known invite: what the operator's command makes,
/// with this `jti` and `exp`.
fn code(live: &Live, jti: &str, exp: i64) -> String {
    live.identity.key.sign_json(
        TYP_INVITE,
        &Invite {
            aud: live.identity.id.clone(),
            jti: jti.to_string(),
            exp,
        },
    )
}

/// An admin (the operator counts) authenticated for `method`.
pub(crate) fn admin(live: &Live, bearer: Option<&str>, method: &str) -> Result<Author, ApiError> {
    let author = live.authenticate(bearer, method)?;
    if !live.admins.is_admin(&author, &live.model.read()) {
        return Err(forbidden("this needs an admin"));
    }
    Ok(author)
}

/// Opens `jws` as an event by `author` on [`STREAM`] of type `kind`:
/// signature by the author's current key, `at` within five minutes, a new
/// nonce. Returns the event.
fn open_event(
    live: &Live,
    author: &Author,
    jws: &str,
    kind: &str,
) -> Result<SignedEvent, ApiError> {
    let event = SignedEvent::open(jws).map_err(sign_error)?;
    let body = &event.body;
    if body.author != author.to_string() {
        return Err(forbidden("the event's author is not the token's issuer"));
    }
    let key = live
        .key_for(author, &body.kid)
        .ok_or_else(|| unauthorized("the event's key is unknown or revoked"))?;
    event.verify(&key).map_err(sign_error)?;
    if body.stream != STREAM || body.kind != kind {
        return Err(invalid(format!("expected a {kind} event on {STREAM}")));
    }
    let at = parse_time(&body.at)
        .ok_or_else(|| invalid("at is not an RFC 3339 time"))?
        .unix_timestamp();
    let now = now();
    if (at - now).abs() > MAX_EVENT_SKEW_S {
        return Err(invalid("at is more than 5 minutes from the server's clock"));
    }
    if !live.model.nonces.insert_new(
        nonce_key(&body.author, &body.nonce),
        at + MAX_EVENT_SKEW_S,
        now,
    ) {
        return Err(ApiError::new(ErrorCode::Conflict, "nonce already used"));
    }
    Ok(event)
}

fn valid_jti(jti: &str) -> bool {
    (16..=64).contains(&jti.len())
        && jti
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

async fn append(live: &Live, jws: &str, event: SignedEvent) -> Result<u64, ApiError> {
    let recorded = live
        .store
        .append(
            vec![NewEvent {
                jws: jws.to_string(),
                jws_index: None,
                body: event.body,
            }],
            &[],
        )
        .await?;
    let position = recorded[0].position;
    live.wait_applied(position).await?;
    Ok(position)
}

/// `POST /v1/invites` with a signed [`CREATED`] event: an admin account
/// mints an invite. Returns `{invite, jti, exp, position}`.
pub async fn create(live: &Live, bearer: Option<&str>, jws: &str) -> Result<Value, ApiError> {
    let author = admin(live, bearer, lxm::INVITES_WRITE)?;
    if !live.invites.creates.allow(&author.to_string()) {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            "too many invites; try again in a minute",
        ));
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Data {
        jti: String,
        exp: i64,
    }
    let _guard = live.invites.writes.lock().await;
    let event = open_event(live, &author, jws, CREATED)?;
    let d: Data = serde_json::from_value(event.body.data.clone())
        .map_err(|e| invalid(format!("event data: {e}")))?;
    if !valid_jti(&d.jti) {
        return Err(invalid("jti is 16 to 64 of [A-Za-z0-9_-]"));
    }
    let now = now();
    if d.exp <= now || d.exp > now + MAX_DAYS * 86_400 + MAX_EVENT_SKEW_S {
        return Err(invalid(format!(
            "exp is in the future, at most {MAX_DAYS} days"
        )));
    }
    if live.invites.get(&d.jti).is_some() || live.model.nonces.contains(&invite_key(&d.jti), now) {
        return Err(ApiError::new(ErrorCode::Conflict, "that jti is taken"));
    }
    let position = append(live, jws, event).await?;
    Ok(json!({
        "invite": code(live, &d.jti, d.exp),
        "jti": d.jti,
        "exp": d.exp,
        "position": position,
    }))
}

/// `POST /v1/invites/{jti}/revoke` with a signed [`REVOKED`] event: an
/// admin ends an open invite.
pub async fn revoke(
    live: &Live,
    bearer: Option<&str>,
    jti: &str,
    jws: &str,
) -> Result<Value, ApiError> {
    let author = admin(live, bearer, lxm::INVITES_WRITE)?;
    live.rate_limit(&author, lxm::INVITES_WRITE)?;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Data {
        jti: String,
    }
    let _guard = live.invites.writes.lock().await;
    let event = open_event(live, &author, jws, REVOKED)?;
    let d: Data = serde_json::from_value(event.body.data.clone())
        .map_err(|e| invalid(format!("event data: {e}")))?;
    if d.jti != jti {
        return Err(invalid("the event names another invite"));
    }
    let r = live
        .invites
        .get(jti)
        .ok_or_else(|| not_found("no such invite"))?;
    if r.state(now()) != "open" {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            format!("that invite is {}", r.state(now())),
        ));
    }
    let position = append(live, jws, event).await?;
    Ok(json!({"jti": jti, "state": "revoked", "position": position}))
}

/// The gamertag of an author id or xuid, when it is an account here.
fn tag_of(live: &Live, xuid: Xuid) -> Option<String> {
    live.model
        .read()
        .accounts
        .get(&xuid)
        .map(|a| a.gamertag.to_string())
}

/// `GET /v1/invites`: the invites the log knows, newest first, for admins.
/// Open ones carry their code again.
pub fn list(live: &Live, bearer: Option<&str>) -> Result<Value, ApiError> {
    admin(live, bearer, LXM_ADMIN_READ)?;
    let now = now();
    let items: Vec<Value> = live
        .invites
        .all()
        .into_iter()
        .map(|r| {
            let state = r.state(now);
            let mut v = json!({
                "jti": r.jti,
                "by": r.by,
                "at": r.at,
                "exp": r.exp,
                "state": state,
            });
            if let Some(tag) = r.creator().and_then(|x| tag_of(live, x)) {
                v["by_gamertag"] = json!(tag);
            }
            if let Some(x) = r.used_by {
                v["used_by"] = json!(x);
                if let Some(tag) = tag_of(live, x) {
                    v["used_by_gamertag"] = json!(tag);
                }
            }
            if state == "open" {
                v["invite"] = json!(code(live, &r.jti, r.exp));
            }
            v
        })
        .collect();
    Ok(json!({"items": items, "position": live.position()}))
}

// HTTP handlers.

type Shared = State<Arc<Live>>;

fn jws_text(body: Result<Bytes, BytesRejection>) -> Result<String, ApiError> {
    let bytes = body.map_err(|e| invalid(e.body_text()))?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| invalid("the body must be a compact JWS"))?;
    Ok(text.trim().to_string())
}

pub async fn get_invites(State(live): Shared, headers: HeaderMap) -> Result<Response, ApiError> {
    let v = list(&live, crate::http::bearer(&headers))?;
    Ok(crate::http::json(StatusCode::OK, &v))
}

pub async fn revoke_invite(
    State(live): Shared,
    Path(jti): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let v = revoke(&live, crate::http::bearer(&headers), &jti, &jws_text(body)?).await?;
    Ok(crate::http::json(StatusCode::OK, &v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jti_shape() {
        assert!(valid_jti("abcdefghijklmnop"));
        assert!(valid_jti("A-b_c0123456789xyz"));
        assert!(!valid_jti("short"));
        assert!(!valid_jti("has space in it here"));
        assert!(!valid_jti(&"x".repeat(65)));
    }

    #[test]
    fn states() {
        let mut r = Record {
            jti: "j".into(),
            by: "acct:0009000000000001@live.test".into(),
            at: String::new(),
            exp: 100,
            position: 1,
            revoked: false,
            used_by: None,
        };
        assert_eq!(r.state(50), "open");
        assert_eq!(r.state(101), "expired");
        r.revoked = true;
        assert_eq!(r.state(50), "revoked");
        r.used_by = Some(Xuid(2));
        assert_eq!(r.state(50), "used");
        assert_eq!(r.creator(), Some(Xuid(0x0009_0000_0000_0001)));
    }
}
