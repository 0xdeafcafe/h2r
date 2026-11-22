//! Account recovery by TOTP (docs/live/live-security.md §1, "Recovery").
//!
//! Each account has a TOTP secret ([`live_proto::recovery`]). The server
//! keeps it in the log, sealed: in `AccountCreated` as `recovery`, then in
//! a server-written [`SET`] event on the account stream, `{"sealed": ...}`,
//! when it is set up or reset, under a key derived from the server
//! key ([`ServerIdentity::derive`](crate::identity::ServerIdentity::derive)),
//! so the log (Postgres or files) never holds one in the clear. The
//! account's signed-in devices read it (`recovery.read`) to show the
//! current code or enrol an authenticator, and reset it
//! (`recovery.write`). A new device recovers with the gamertag and a
//! current code: the server checks the code (one step either side, each
//! step used once), applies exponential backoff per gamertag and client IP,
//! and appends a server-signed `KeyAdded` with the device's signed request
//! as proof and `"via": "totp"`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::recovery::{
    self, PERIOD, RecoverBody, RecoverRequest, RecoveryInfo, SECRET_BYTES, TotpSecret, seal,
};
use live_proto::{Author, CreatedAccount, ErrorCode, Gamertag, Xuid, lxm, streams, types};
use serde_json::{Value, json};

use crate::app::{ApiError, Live, forbidden, invalid, not_found, now, sign_error};
use crate::dispatch::Subscriber;
use crate::guard::{Backoff, RateLimiter, ReplayGuard};
use crate::http::ClientIp;
use crate::store::{NewEvent, RecordedEvent};

/// The server's event holding an account's sealed secret; the newest one
/// is current. `{"sealed": null}` turns recovery off.
pub const SET: &str = "RecoverySet";
/// The label of the sealing key.
const SEAL_LABEL: &str = "h2live-recovery-seal-v1";
/// Wrong codes before the lockout starts, its first length and its cap.
pub const FREE_FAILURES: u32 = 5;
pub const LOCK_BASE: Duration = Duration::from_secs(60);
pub const LOCK_MAX: Duration = Duration::from_secs(24 * 3600);

/// The sealed secrets the log holds, plus the guards.
pub struct Recovery {
    sealed: RwLock<HashMap<Xuid, String>>,
    key: [u8; 32],
    issuer: String,
    lockout: Backoff,
    attempts: RateLimiter,
    per_account: RateLimiter,
    /// `(xuid, step)` pairs already used.
    used: ReplayGuard,
}

impl Recovery {
    pub fn new(
        identity: &crate::identity::ServerIdentity,
        lockout: Backoff,
        attempts: RateLimiter,
    ) -> Self {
        Recovery {
            sealed: RwLock::default(),
            key: identity.derive(SEAL_LABEL),
            issuer: identity.id.clone(),
            lockout,
            attempts,
            // A burst allows for a few mistyped codes across devices.
            per_account: RateLimiter::new(1.0 / 300.0, 10),
            used: ReplayGuard::default(),
        }
    }

    /// `xuid`'s current secret, if it has one (and it opens).
    pub fn secret(&self, xuid: Xuid) -> Option<TotpSecret> {
        let sealed = self.sealed.read().unwrap().get(&xuid)?.clone();
        let bytes: [u8; SECRET_BYTES] = seal::open(&self.key, &sealed)?.try_into().ok()?;
        Some(TotpSecret::from_bytes(bytes))
    }

    /// A [`SET`] event's data for `secret`.
    pub fn set_data(&self, secret: &TotpSecret) -> Value {
        json!({"sealed": seal::seal(&self.key, secret.bytes())})
    }

    pub fn info(&self, secret: &TotpSecret, gamertag: &str, now: i64) -> RecoveryInfo {
        RecoveryInfo::new(secret, &self.issuer, gamertag, now)
    }
}

impl Subscriber for Recovery {
    fn name(&self) -> &str {
        "recovery"
    }

    fn apply(&self, event: &RecordedEvent) {
        let b = &event.body;
        let field = match b.kind.as_str() {
            SET => "sealed",
            types::ACCOUNT_CREATED => "recovery",
            _ => return,
        };
        let Some(xuid) = b
            .stream
            .strip_prefix("account-")
            .and_then(|x| Xuid::parse(x).ok())
        else {
            return;
        };
        let mut sealed = self.sealed.write().unwrap();
        match b.data.get(field).and_then(Value::as_str) {
            Some(s) => sealed.insert(xuid, s.to_string()),
            None => sealed.remove(&xuid),
        };
    }
}

fn own_account(
    live: &Live,
    bearer: Option<&str>,
    method: &str,
    xuid: Xuid,
) -> Result<String, ApiError> {
    match live.authenticate(bearer, method)? {
        Author::Account { xuid: me, .. } if me == xuid => {}
        _ => return Err(forbidden("only the account itself")),
    }
    live.model
        .read()
        .accounts
        .get(&xuid)
        .map(|a| a.gamertag.to_string())
        .ok_or_else(|| not_found("no such account"))
}

/// `GET /v1/accounts/by-xuid/{xuid}/recovery`: the secret and current code,
/// for the account's own devices.
pub fn get(live: &Live, bearer: Option<&str>, xuid: Xuid) -> Result<RecoveryInfo, ApiError> {
    let tag = own_account(live, bearer, lxm::RECOVERY_READ, xuid)?;
    let secret = live
        .recovery
        .secret(xuid)
        .ok_or_else(|| not_found("this account has no recovery set up"))?;
    Ok(live.recovery.info(&secret, &tag, now()))
}

/// `POST /v1/accounts/by-xuid/{xuid}/recovery`: a new secret (set up, or
/// reset: the old one stops working).
pub async fn reset(
    live: &Live,
    bearer: Option<&str>,
    xuid: Xuid,
) -> Result<RecoveryInfo, ApiError> {
    let tag = own_account(live, bearer, lxm::RECOVERY_WRITE, xuid)?;
    let author = Author::Account {
        xuid,
        server: live.identity.id.clone(),
    };
    live.rate_limit(&author, lxm::RECOVERY_WRITE)?;
    let secret = TotpSecret::generate();
    let event = live.server_event(streams::account(xuid), SET, live.recovery.set_data(&secret));
    let recorded = live.store.append(vec![event], &[]).await?;
    live.wait_applied(recorded[0].position).await?;
    Ok(live.recovery.info(&secret, &tag, now()))
}

/// `POST /v1/accounts/{gamertag}/recover`.
pub async fn recover(
    live: &Live,
    ip: &str,
    path_tag: &Gamertag,
    body: &RecoverBody,
    device: Option<crate::signins::Device>,
) -> Result<CreatedAccount, ApiError> {
    if !live.recovery.attempts.allow(ip) {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            "too many recoveries; try again shortly",
        ));
    }
    let now = now();
    let (req, key) =
        RecoverRequest::open(&body.request, &live.identity.id, now).map_err(sign_error)?;
    let tag = Gamertag::new(&req.gamertag).map_err(|e| invalid(e.to_string()))?;
    if tag.key() != path_tag.key() {
        return Err(invalid("the request is for another gamertag"));
    }
    let code = recovery::parse_code(&body.code).ok_or_else(|| invalid("the code is six digits"))?;
    let lock = format!("{}|{ip}", tag.key());
    if let Some(left) = live.recovery.lockout.locked(&lock) {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            format!(
                "too many wrong codes for this gamertag from your address; try again in {} s",
                left.as_secs().max(1)
            ),
        ));
    }
    let (xuid, gamertag) = {
        let s = live.model.read();
        let a = s
            .account_by_tag(&tag)
            .ok_or_else(|| not_found("no such gamertag"))?;
        if s.keys.contains_key(key.kid()) {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "that key is already registered",
            ));
        }
        (a.xuid, a.gamertag.clone())
    };
    if !live.recovery.per_account.allow(&xuid.to_string()) {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            "too many recovery attempts for this account",
        ));
    }
    let secret = live
        .recovery
        .secret(xuid)
        .ok_or_else(|| forbidden("this account has no recovery set up"))?;
    let Some(step) = secret.verify(code, now) else {
        // The count lets a client say how many tries are left
        // (docs/live/live-security.md §1, Recovery).
        let left = live.recovery.lockout.fail(&lock);
        return Err(forbidden(format!("wrong code; {left} tries left")));
    };
    if !live
        .recovery
        .used
        .insert_new(format!("{xuid} {step}"), (step + 2) * PERIOD, now)
    {
        return Err(forbidden(
            "that code was used already; wait for the next one",
        ));
    }
    let mut added = json!({"kid": key.kid(), "jwk": key.jwk(), "proof": body.request, "via": "totp"});
    if let Some(d) = &device {
        added["device"] = live.signins.seal(d).into();
    }
    let event: NewEvent = live.server_event(streams::account(xuid), types::KEY_ADDED, added);
    let recorded = live.store.append(vec![event], &[]).await?;
    live.wait_applied(recorded[0].position).await?;
    live.recovery.lockout.clear(&lock);
    Ok(CreatedAccount {
        xuid,
        gamertag,
        id: Author::Account {
            xuid,
            server: live.identity.id.clone(),
        }
        .to_string(),
        recovery: None,
    })
}

// HTTP handlers.

type Shared = State<Arc<Live>>;

fn xuid_of(raw: &str) -> Result<Xuid, ApiError> {
    Xuid::parse(raw).map_err(|_| invalid("not a XUID"))
}

pub async fn get_recovery(
    State(live): Shared,
    headers: HeaderMap,
    Path(xuid): Path<String>,
) -> Result<Response, ApiError> {
    let info = get(&live, crate::http::bearer(&headers), xuid_of(&xuid)?)?;
    Ok(crate::http::json(StatusCode::OK, &info))
}

pub async fn post_recovery(
    State(live): Shared,
    headers: HeaderMap,
    Path(xuid): Path<String>,
) -> Result<Response, ApiError> {
    let info = reset(&live, crate::http::bearer(&headers), xuid_of(&xuid)?).await?;
    Ok(crate::http::json(StatusCode::CREATED, &info))
}

pub(crate) async fn post_recover(
    State(live): Shared,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    headers: HeaderMap,
    Path(tag): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let tag = Gamertag::new(&tag).map_err(|e| invalid(e.to_string()))?;
    let bytes = body.map_err(|e| invalid(e.body_text()))?;
    let body: RecoverBody =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("body: {e}")))?;
    let created = recover(&live, &ip, &tag, &body, crate::signins::device_of(&headers)).await?;
    Ok(crate::http::json(StatusCode::CREATED, &created))
}
