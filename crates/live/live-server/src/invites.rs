//! Server invites (docs/live/live-admin.md §5 and live-security.md §3).
//!
//! The operator can mint invites directly. An admin or an account with an
//! invite grant sends an `invites.write` token and an [`CREATED`] event
//! on the [`STREAM`] stream signed by the account. The server checks its
//! current permission, rate and cap, then records the event before
//! returning the code.
//!
//! Generated codes come from the invite's id and a server-only key. A
//! custom code is sent beside the event and logged only as a MAC in a
//! server-signed [`CODE_SET`] event. The list can show generated codes for
//! open invites. Older server-signed JWS codes also redeem.
//!
//! The server signs [`GRANTED`] and [`GRANT_REVOKED`] on the target's
//! account stream. The projection closes open invites on admin revocation
//! and when a non-admin loses its grant.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use hmac::{Hmac, Mac};
use live_proto::recovery::invite_code;
use live_proto::sign::{Invite, MAX_EVENT_SKEW_S, SignedEvent, TYP_INVITE, parse_time};
use live_proto::{Author, ErrorCode, Gamertag, InviteGrant, Xuid, lxm, streams, types};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;

use crate::app::{ApiError, Live, forbidden, invalid, not_found, now, sign_error, unauthorized};
use crate::dispatch::Subscriber;
use crate::guard::RateLimiter;
use crate::model::{invite_key, nonce_key};
use crate::store::{NewEvent, RecordedEvent};

/// The stream invite events go on.
pub const STREAM: &str = "invites";
/// An account or operator made an invite: `{jti, exp}`.
pub const CREATED: &str = "InviteCreated";
/// An account or operator ended an open invite: `{jti}`.
pub const REVOKED: &str = "InviteRevoked";
/// The server recorded the MAC of a custom code: `{jti, mac}`.
pub const CODE_SET: &str = "InviteCodeSet";
/// The server granted or updated an account's explicit permission.
pub const GRANTED: &str = "InviteGrantSet";
/// The server ended an account's explicit permission.
pub const GRANT_REVOKED: &str = "InviteGrantRevoked";
/// The label of the invite-code key ([`crate::ServerIdentity::derive`]).
pub const CODE_LABEL: &str = "h2live-invite-code-v1";
/// The token method for admin-only reads (`GET /v1/invites`,
/// `GET /v1/admin/status`).
pub const LXM_ADMIN_READ: &str = "admin.read";
/// The longest an account's invite may last.
pub const MAX_DAYS: i64 = 30;
/// Per author: one invite a minute on average, ten at once.
pub const CREATE_RATE: (f64, u32) = (1.0 / 60.0, 10);
/// Per explicitly granted non-admin: one invite a minute, three at once.
pub const GRANTED_CREATE_RATE: (f64, u32) = (1.0 / 60.0, 3);
const GRANT_REQUIRED: &str = "an admin must enable invite access for this account";

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
    /// Its code was chosen by its creator.
    pub custom: bool,
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

/// Invite grants and records projected from the log, with per-author
/// creation rate limits.
pub struct Invites {
    grants: RwLock<HashMap<Xuid, InviteGrant>>,
    by_jti: RwLock<HashMap<String, Record>>,
    /// MAC of a normalised code -> jti.
    by_code: RwLock<HashMap<String, String>>,
    code_key: [u8; 32],
    creates: RateLimiter,
    granted_creates: RateLimiter,
    /// Serializes invite writes, grant changes and redemption.
    pub(crate) writes: tokio::sync::Mutex<()>,
}

impl Invites {
    pub fn new(code_key: [u8; 32]) -> Self {
        Invites {
            grants: RwLock::default(),
            by_jti: RwLock::default(),
            by_code: RwLock::default(),
            code_key,
            creates: RateLimiter::new(CREATE_RATE.0, CREATE_RATE.1),
            granted_creates: RateLimiter::new(GRANTED_CREATE_RATE.0, GRANTED_CREATE_RATE.1),
            writes: Default::default(),
        }
    }
}

impl Invites {
    fn hmac(&self, label: &[u8], data: &str) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.code_key).expect("any key length");
        mac.update(label);
        mac.update(data.as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// The MAC a code is logged and looked up by.
    pub fn mac(&self, code: &str) -> String {
        live_proto::sign::hex(&self.hmac(b"code\0", &invite_code::normalize(code)))
    }

    /// The generated code of invite `jti`.
    pub fn generated(&self, jti: &str) -> String {
        invite_code::from_bytes(&self.hmac(b"gen\0", jti))
    }

    /// The invite a typed code names.
    pub fn lookup(&self, code: &str) -> Option<Record> {
        let jti = self.by_code.read().unwrap().get(&self.mac(code))?.clone();
        self.get(&jti)
    }

    pub fn get(&self, jti: &str) -> Option<Record> {
        self.by_jti.read().unwrap().get(jti).cloned()
    }

    pub fn grant(&self, xuid: Xuid) -> Option<InviteGrant> {
        self.grants.read().unwrap().get(&xuid).copied()
    }

    fn open_count(&self, by: &str, now: i64) -> usize {
        self.by_jti
            .read()
            .unwrap()
            .values()
            .filter(|r| r.by == by && r.state(now) == "open")
            .count()
    }

    /// Every known invite, newest first.
    pub fn all(&self) -> Vec<Record> {
        self.list_matching(|_| true)
    }

    fn list_matching(&self, include: impl Fn(&Record) -> bool) -> Vec<Record> {
        let mut records: Vec<_> = self
            .by_jti
            .read()
            .unwrap()
            .values()
            .filter(|r| include(r))
            .cloned()
            .collect();
        records.sort_by_key(|r| std::cmp::Reverse(r.position));
        records
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
                let mut by_jti = self.by_jti.write().unwrap();
                if by_jti.contains_key(jti) {
                    return;
                }
                by_jti.insert(
                    jti.to_string(),
                    Record {
                        jti: jti.to_string(),
                        by: b.author.clone(),
                        at: b.at.clone(),
                        exp,
                        position: event.position,
                        revoked: false,
                        used_by: None,
                        custom: false,
                    },
                );
                let mac = self.mac(&self.generated(jti));
                self.by_code.write().unwrap().insert(mac, jti.to_string());
            }
            (STREAM, CODE_SET, Some(jti)) => {
                let Some(mac) = b.data.get("mac").and_then(Value::as_str) else {
                    return;
                };
                if let Some(r) = self.by_jti.write().unwrap().get_mut(jti) {
                    r.custom = true;
                }
                let generated = self.mac(&self.generated(jti));
                let mut by_code = self.by_code.write().unwrap();
                by_code.remove(&generated);
                by_code.insert(mac.to_string(), jti.to_string());
            }
            (STREAM, REVOKED, Some(jti)) => {
                if let Some(r) = self.by_jti.write().unwrap().get_mut(jti) {
                    r.revoked = true;
                }
            }
            (_, GRANTED | GRANT_REVOKED | crate::admin::ADMIN_REVOKED, _) => {
                self.apply_permission_change(event);
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

impl Invites {
    fn apply_permission_change(&self, event: &RecordedEvent) {
        let b = &event.body;
        let Some(Author::Server(server)) = Author::parse(&b.author) else {
            return;
        };
        let Some(xuid) = b.data["xuid"].as_str().and_then(|x| Xuid::parse(x).ok()) else {
            return;
        };
        if b.stream != streams::account(xuid) {
            return;
        }
        if b.kind == GRANTED {
            if let Ok(open_cap) = serde_json::from_value(b.data["open_cap"].clone()) {
                self.grants
                    .write()
                    .unwrap()
                    .insert(xuid, InviteGrant { open_cap });
            }
        } else if let Some(at) = parse_time(&b.at) {
            if b.kind == GRANT_REVOKED {
                self.grants.write().unwrap().remove(&xuid);
                if b.data["keep_open"].as_bool() == Some(true) {
                    return;
                }
            }
            let by = Author::Account { xuid, server }.to_string();
            let at = at.unix_timestamp();
            for r in self.by_jti.write().unwrap().values_mut() {
                if r.by == by && r.state(at) == "open" {
                    r.revoked = true;
                }
            }
        }
    }
}

/// Whether an invite with this `jti` may still be used. Known invites need
/// a currently permitted creator. Legacy operator codes have no log record.
pub fn check_redeemable(live: &Live, jti: &str) -> Result<(), ApiError> {
    let Some(r) = live.invites.get(jti) else {
        return Ok(());
    };
    if r.revoked {
        return Err(forbidden("invite: revoked"));
    }
    if let Some(xuid) = r.creator()
        && !live.admins.is_admin_xuid(xuid, &live.model.read())
        && (live.invites.grant(xuid).is_none() || live.bans.read().in_force(xuid, now()).is_some())
    {
        return Err(forbidden("invite: its creator no longer has invite access"));
    }
    Ok(())
}

/// The open invite a short code names, with a permitted creator. Returns
/// its `jti`.
pub fn redeem_short(live: &Live, code: &str, now: i64) -> Result<String, ApiError> {
    let r = live
        .invites
        .lookup(code)
        .ok_or_else(|| forbidden("invite: no such code"))?;
    match r.state(now) {
        "open" => {}
        state => return Err(forbidden(format!("invite: {state}"))),
    }
    check_redeemable(live, &r.jti)?;
    Ok(r.jti)
}

/// The signed invite code for a known invite: what the operator's command
/// used to make, with this `jti` and `exp`. Still redeemable.
#[allow(dead_code)]
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
                jws: jws.into(),
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

fn permission(live: &Live, author: &Author) -> Result<(bool, Option<InviteGrant>), ApiError> {
    let admin = live.admins.is_admin(author, &live.model.read());
    let grant = match author {
        Author::Account { xuid, .. } => live.invites.grant(*xuid),
        Author::Server(_) => None,
    };
    if admin || grant.is_some() {
        Ok((admin, grant))
    } else {
        Err(forbidden(GRANT_REQUIRED))
    }
}

/// `POST /v1/invites` with a signed [`CREATED`] event: a permitted account
/// mints an invite. Returns `{invite, jti, exp, position}`.
pub async fn create(
    live: &Live,
    bearer: Option<&str>,
    jws: &str,
    custom: Option<&str>,
) -> Result<Value, ApiError> {
    let author = live.authenticate(bearer, lxm::INVITES_WRITE)?;
    let _guard = live.invites.writes.lock().await;
    let (admin, grant) = permission(live, &author)?;
    let by = author.to_string();
    let now = now();
    if !admin
        && let Some(cap) = grant.and_then(|g| g.open_cap)
        && live.invites.open_count(&by, now) >= cap.get() as usize
    {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            format!("open invite limit reached ({cap})"),
        ));
    }
    let rate = if admin {
        &live.invites.creates
    } else {
        &live.invites.granted_creates
    };
    if !rate.allow(&by) {
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
    let event = open_event(live, &author, jws, CREATED)?;
    let d: Data = serde_json::from_value(event.body.data.clone())
        .map_err(|e| invalid(format!("event data: {e}")))?;
    if !valid_jti(&d.jti) {
        return Err(invalid("jti is 16 to 64 of [A-Za-z0-9_-]"));
    }
    if d.exp <= now || d.exp > now + MAX_DAYS * 86_400 + MAX_EVENT_SKEW_S {
        return Err(invalid(format!(
            "exp is in the future, at most {MAX_DAYS} days"
        )));
    }
    if live.invites.get(&d.jti).is_some() || live.model.nonces.contains(&invite_key(&d.jti), now) {
        return Err(ApiError::new(ErrorCode::Conflict, "that jti is taken"));
    }
    check_custom(live, custom, now)?;
    let mut events = vec![NewEvent {
        jws: jws.into(),
        jws_index: None,
        body: event.body,
    }];
    if let Some(c) = custom {
        events.push(code_set(live, &d.jti, c));
    }
    let position = append_all(live, events).await?;
    Ok(reply(live, &d.jti, d.exp, custom, position))
}

/// A custom code must be well formed and not name an open invite already.
fn check_custom(live: &Live, custom: Option<&str>, now: i64) -> Result<(), ApiError> {
    let Some(c) = custom else {
        return Ok(());
    };
    if !invite_code::valid_custom(c) {
        return Err(invalid(format!(
            "a custom code is {} to {} printable characters",
            invite_code::MIN_CUSTOM,
            invite_code::MAX_CUSTOM
        )));
    }
    if live
        .invites
        .lookup(c)
        .is_some_and(|r| r.state(now) == "open")
    {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "an open invite has that code",
        ));
    }
    Ok(())
}

fn code_set(live: &Live, jti: &str, code: &str) -> NewEvent {
    live.server_event(
        STREAM.into(),
        CODE_SET,
        json!({"jti": jti, "mac": live.invites.mac(code)}),
    )
}

fn reply(live: &Live, jti: &str, exp: i64, custom: Option<&str>, position: u64) -> Value {
    json!({
        "invite": custom.map_or_else(|| live.invites.generated(jti), str::to_string),
        "custom": custom.is_some(),
        "jti": jti,
        "exp": exp,
        "position": position,
    })
}

async fn append_all(live: &Live, events: Vec<NewEvent>) -> Result<u64, ApiError> {
    let recorded = live.store.append(events, &[]).await?;
    let position = recorded.last().expect("appended").position;
    live.wait_applied(position).await?;
    Ok(recorded[0].position)
}

/// `POST /v1/invites` by the operator (a server-key token): a logged,
/// server-signed invite lasting `days`, with `custom` as its code if given.
pub async fn create_operator(
    live: &Live,
    days: i64,
    custom: Option<&str>,
) -> Result<Value, ApiError> {
    let _guard = live.invites.writes.lock().await;
    let now = now();
    check_custom(live, custom, now)?;
    let jti = live_proto::sign::random_id();
    let exp = now + days.clamp(1, 365) * 86_400;
    let mut events =
        vec![live.server_event(STREAM.into(), CREATED, json!({"jti": jti, "exp": exp}))];
    if let Some(c) = custom {
        events.push(code_set(live, &jti, c));
    }
    let position = append_all(live, events).await?;
    Ok(reply(live, &jti, exp, custom, position))
}

/// `POST /v1/invites/{jti}/revoke` with a signed [`REVOKED`] event:
/// admins end any open invite, and granted accounts end their own.
pub async fn revoke(
    live: &Live,
    bearer: Option<&str>,
    jti: &str,
    jws: &str,
) -> Result<Value, ApiError> {
    let author = live.authenticate(bearer, lxm::INVITES_WRITE)?;
    let _guard = live.invites.writes.lock().await;
    let (admin, _) = permission(live, &author)?;
    live.rate_limit(&author, lxm::INVITES_WRITE)?;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Data {
        jti: String,
    }
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
    if !admin && r.by != author.to_string() {
        return Err(not_found("no such invite"));
    }
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
    Ok(list_for(live, None))
}

fn list_mine(live: &Live, bearer: Option<&str>) -> Result<Value, ApiError> {
    let author = live.authenticate(bearer, lxm::INVITES_READ)?;
    permission(live, &author)?;
    Ok(list_for(live, Some(&author.to_string())))
}

fn list_for(live: &Live, owner: Option<&str>) -> Value {
    let now = now();
    let records = match owner {
        None => live.invites.all(),
        Some(by) => live.invites.list_matching(|r| r.by == by && r.state(now) == "open"),
    };
    let items: Vec<Value> = records
        .into_iter()
        .map(|r| {
            let state = r.state(now);
            let mut v = json!({
                "jti": r.jti,
                "by": r.by,
                "at": r.at,
                "exp": r.exp,
                "state": state,
                "custom": r.custom,
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
            if state == "open" && !r.custom {
                v["invite"] = json!(live.invites.generated(&r.jti));
            }
            v
        })
        .collect();
    json!({"items": items, "position": live.position()})
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

pub async fn get_my_invites(State(live): Shared, headers: HeaderMap) -> Result<Response, ApiError> {
    let v = list_mine(&live, crate::http::bearer(&headers))?;
    Ok(crate::http::json(StatusCode::OK, &v))
}

async fn change_grant(
    live: &Live,
    author: Author,
    tag: &str,
    grant: Option<InviteGrant>,
) -> Result<Response, ApiError> {
    live.rate_limit(&author, lxm::INVITE_GRANTS_WRITE)?;
    let _guard = live.invites.writes.lock().await;
    if !live.admins.is_admin(&author, &live.model.read()) {
        return Err(forbidden("this needs an admin"));
    }
    let tag = Gamertag::new(tag).map_err(|_| not_found("no such account"))?;
    let xuid = crate::admin::account_xuid(live, &tag)?;
    let mut data = json!({"xuid": xuid, "gamertag": tag, "by": author.to_string()});
    let (kind, status) = if let Some(grant) = grant {
        data["open_cap"] = json!(grant.open_cap);
        (GRANTED, StatusCode::CREATED)
    } else {
        if live.invites.grant(xuid).is_none() {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "this account has no invite grant",
            ));
        }
        // Persist this decision so replay also respects the role held at revocation,
        // including an admin supplied by configuration or the embedded server.
        if live.admins.is_admin_xuid(xuid, &live.model.read()) {
            data["keep_open"] = json!(true);
        }
        (GRANT_REVOKED, StatusCode::OK)
    };
    let event = crate::admin::append_account_event(live, xuid, kind, data).await?;
    Ok(crate::http::json(status, &event))
}

pub async fn grant_invites(
    State(live): Shared,
    Path(tag): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let author = admin(&live, crate::http::bearer(&headers), lxm::INVITE_GRANTS_WRITE)?;
    let grant = serde_json::from_slice(&crate::http::body_bytes(body)?)
        .map_err(|e| invalid(format!("invite grant: {e}")))?;
    change_grant(&live, author, &tag, Some(grant)).await
}

pub async fn revoke_grant(
    State(live): Shared,
    Path(tag): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let author = admin(&live, crate::http::bearer(&headers), lxm::INVITE_GRANTS_WRITE)?;
    change_grant(&live, author, &tag, None).await
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
            custom: false,
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
