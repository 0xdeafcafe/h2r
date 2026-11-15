//! The service core: authentication, write rules and the operations behind
//! the HTTP API. Writes verify, append, then wait until the read model has
//! applied them (read-your-writes), so a successful reply is readable
//! state. Every check here is in memory; the only I/O is the append.

use std::sync::Arc;
use std::time::Duration;

use live_proto::sign::{
    self, AccountRequest, EventBody, Invite, Jwk, MAX_EVENT_SKEW_S, PublicKey, SignError,
    SignedEvent, TOKEN_LEEWAY_S, Token, parse_time,
};
use live_proto::{
    AccountDetail, AccountKeys, AccountSummary, Author, CreatedAccount, Discovery, ErrorCode,
    Gamertag, PRESENCE_STATES, Page, Presence, PublishedFile, TitleId, WireEvent, Xuid, lxm,
    streams, types, valid_file_name,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, watch};

use crate::dispatch::Dispatcher;
use crate::guard::{RateLimiter, ReplayGuard};
use crate::identity::ServerIdentity;
use crate::model::{Model, State, invite_key, nonce_key};
use crate::store::{EventStore, NewEvent, Query, RecordedEvent, StoreError};

/// Title files can be large: `multiplayer_hopper.dat` inflates to 1.4 MB.
pub const MAX_TITLE_FILE: usize = 2 * 1024 * 1024;
/// How long a write waits for the read model to apply it.
const APPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a token's `jti` is remembered (security §3: two minutes).
const JTI_MEMORY_S: i64 = 120;

/// A failed request: an error code (and so an HTTP status) and a message
/// safe to show the caller.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError {
            code,
            message: message.into(),
        }
    }
}

fn invalid(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, m)
}
fn unauthorized(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Unauthorized, m)
}
fn forbidden(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Forbidden, m)
}
fn conflict(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Conflict, m)
}
pub fn not_found(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::NotFound, m)
}

/// Store failures are logged here and reported without detail.
impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        eprintln!("live: store: {e}");
        ApiError::new(ErrorCode::Unavailable, "the event store is unavailable")
    }
}

/// A signature problem is the caller's (401); a malformed body is 400.
fn sign_error(e: SignError) -> ApiError {
    match e {
        SignError::BadSignature | SignError::KidMismatch => unauthorized(e.to_string()),
        _ => invalid(e.to_string()),
    }
}

/// Server switches (docs/live-security.md §3, §6).
#[derive(Debug, Clone)]
pub struct Policy {
    /// Reads need a token too.
    pub private: bool,
    /// Account creation needs a server-signed invite.
    pub invite_only: bool,
    /// Per author and method: requests per second, burst.
    pub write_rate: (f64, u32),
    /// Account creation, server-wide.
    pub create_rate: (f64, u32),
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            private: false,
            invite_only: false,
            write_rate: (10.0, 30),
            create_rate: (1.0, 10),
        }
    }
}

pub struct Live {
    store: Arc<dyn EventStore>,
    model: Arc<Model>,
    identity: ServerIdentity,
    policy: Policy,
    applied: watch::Receiver<u64>,
    feed: broadcast::Sender<Arc<RecordedEvent>>,
    jtis: ReplayGuard,
    writes: RateLimiter,
    creates: RateLimiter,
    dispatcher: tokio::task::AbortHandle,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.dispatcher.abort();
    }
}

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// `0x0009` (the marker real Live account XUIDs carry) over 48 random bits.
fn generate_xuid() -> Xuid {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b[2..]).expect("the OS random source failed");
    Xuid(0x0009_0000_0000_0000 | u64::from_be_bytes(b))
}

fn stream_xuid(stream: &str) -> Option<Xuid> {
    let rest = stream
        .strip_prefix("account-")
        .or_else(|| stream.strip_prefix("presence-"))?;
    Xuid::parse(rest).ok()
}

fn parse_data<'a, T: Deserialize<'a>>(data: &'a Value) -> Result<T, ApiError> {
    T::deserialize(data).map_err(|e| invalid(format!("event data: {e}")))
}

/// Who may append what (security §2): an account writes its own key and
/// presence events; the server anything but the events it only writes
/// through its own endpoints. Games come with the game endpoints.
fn may_write(author: &Author, stream: &str, kind: &str) -> bool {
    match author {
        Author::Server(_) => !matches!(
            kind,
            types::ACCOUNT_CREATED | types::GAMERTAG_RESERVED | types::TITLE_FILE_PUBLISHED
        ),
        Author::Account { xuid, .. } => {
            (stream == streams::account(*xuid)
                && matches!(kind, types::KEY_ADDED | types::KEY_REVOKED))
                || (stream == streams::presence(*xuid)
                    && matches!(
                        kind,
                        types::SIGNED_IN | types::PRESENCE_CHANGED | types::SIGNED_OUT
                    ))
        }
    }
}

/// Checks an event's data against the current state.
fn validate(state: &State, body: &EventBody) -> Result<(), ApiError> {
    #[derive(Deserialize)]
    struct KeyAdded {
        jwk: Jwk,
        kid: Option<String>,
    }
    #[derive(Deserialize)]
    struct KeyRevoked {
        kid: String,
    }
    #[derive(Deserialize)]
    struct PresenceData {
        xuid: Xuid,
        state: Option<String>,
        game_id: Option<String>,
    }
    let xuid = stream_xuid(&body.stream);
    match body.kind.as_str() {
        types::KEY_ADDED => {
            let d: KeyAdded = parse_data(&body.data)?;
            let key = PublicKey::from_jwk(&d.jwk).map_err(|e| invalid(e.to_string()))?;
            if d.kid.as_deref().is_some_and(|k| k != key.kid()) {
                return Err(invalid("kid is not the key's thumbprint"));
            }
            if state.keys.contains_key(key.kid()) {
                return Err(conflict("that key is already registered"));
            }
            if xuid.is_none_or(|x| !state.accounts.contains_key(&x)) {
                return Err(invalid("no such account"));
            }
        }
        types::KEY_REVOKED => {
            let d: KeyRevoked = parse_data(&body.data)?;
            let xuid = xuid.ok_or_else(|| invalid("not an account stream"))?;
            if state.active_key(xuid, &d.kid).is_none() {
                return Err(invalid("no such current key on this account"));
            }
            if state.active_key_count(xuid) < 2 {
                return Err(conflict("cannot revoke an account's last key"));
            }
        }
        types::SIGNED_IN
        | types::PRESENCE_CHANGED
        | types::SIGNED_OUT
        | types::PRESENCE_EXPIRED => {
            let d: PresenceData = parse_data(&body.data)?;
            if Some(d.xuid) != xuid {
                return Err(invalid("presence xuid is not the stream's"));
            }
            if matches!(
                body.kind.as_str(),
                types::SIGNED_IN | types::PRESENCE_CHANGED
            ) {
                let s = d.state.as_deref().unwrap_or("");
                if !PRESENCE_STATES.contains(&s) {
                    return Err(invalid("presence state must be online, away or playing"));
                }
                if (s == "playing") != d.game_id.is_some() {
                    return Err(invalid("game_id is set exactly when playing"));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

impl Live {
    /// Starts the dispatcher and waits until the read model has replayed
    /// the whole log.
    pub async fn start(
        store: Arc<dyn EventStore>,
        identity: ServerIdentity,
        policy: Policy,
    ) -> anyhow::Result<Self> {
        let model = Arc::new(Model::default());
        let mut dispatcher = Dispatcher::new(store.clone());
        dispatcher.subscribe(model.clone());
        let mut applied = dispatcher.applied();
        let feed = dispatcher.feed();
        let wake = store.wake().await?;
        let head = store.head().await?;
        let task = tokio::spawn(dispatcher.run(wake));
        let live = Live {
            store,
            model,
            identity,
            applied: applied.clone(),
            feed,
            jtis: ReplayGuard::default(),
            writes: RateLimiter::new(policy.write_rate.0, policy.write_rate.1),
            creates: RateLimiter::new(policy.create_rate.0, policy.create_rate.1),
            policy,
            dispatcher: task.abort_handle(),
        };
        applied.wait_for(|p| *p >= head).await?;
        Ok(live)
    }

    pub fn id(&self) -> &str {
        &self.identity.id
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The last position the read model has applied: the `position` of
    /// every read response.
    pub fn position(&self) -> u64 {
        *self.applied.borrow()
    }

    pub fn applied(&self) -> watch::Receiver<u64> {
        self.applied.clone()
    }

    pub fn feed(&self) -> broadcast::Receiver<Arc<RecordedEvent>> {
        self.feed.subscribe()
    }

    pub fn store(&self) -> &dyn EventStore {
        self.store.as_ref()
    }

    /// The current key `kid` of `author`, from memory.
    fn key_for(&self, author: &Author, kid: &str) -> Option<PublicKey> {
        if author.server() != self.identity.id {
            return None;
        }
        match author {
            Author::Server(_) => {
                (kid == self.identity.key.kid()).then(|| self.identity.key.public().clone())
            }
            Author::Account { xuid, .. } => self.model.read().active_key(*xuid, kid).cloned(),
        }
    }

    /// Checks a bearer token for `method` (security §3) and returns its
    /// author. No I/O.
    pub fn authenticate(&self, bearer: Option<&str>, method: &str) -> Result<Author, ApiError> {
        let raw = bearer.ok_or_else(|| unauthorized("this needs Authorization: Bearer <token>"))?;
        let token = Token::parse(raw).map_err(|e| unauthorized(format!("token: {e}")))?;
        let c = &token.claims;
        let author =
            Author::parse(&c.iss).ok_or_else(|| unauthorized("token iss is not an author id"))?;
        let kid = token
            .jws
            .kid()
            .ok_or_else(|| unauthorized("token has no kid"))?;
        let key = self
            .key_for(&author, kid)
            .ok_or_else(|| unauthorized("token key is unknown or revoked"))?;
        let now = now();
        token
            .verify(&key, &self.identity.id, method, now)
            .map_err(|e| unauthorized(e.to_string()))?;
        let expires = (c.exp + TOKEN_LEEWAY_S).max(now + JTI_MEMORY_S);
        if !self
            .jtis
            .insert_new(format!("{} {}", c.iss, c.jti), expires, now)
        {
            return Err(unauthorized("token already used"));
        }
        Ok(author)
    }

    /// Reads are public unless the server is `--private`.
    pub fn authorize_read(&self, bearer: Option<&str>) -> Result<(), ApiError> {
        if self.policy.private {
            self.authenticate(bearer, lxm::READ)?;
        }
        Ok(())
    }

    /// The token's expiry, for WebSockets that must close when it passes.
    pub fn authenticate_until(&self, bearer: &str, method: &str) -> Result<i64, ApiError> {
        self.authenticate(Some(bearer), method)?;
        Ok(Token::parse(bearer).map(|t| t.claims.exp).unwrap_or(0))
    }

    fn rate_limit(&self, author: &Author, method: &str) -> Result<(), ApiError> {
        if self.writes.allow(&format!("{author} {method}")) {
            Ok(())
        } else {
            Err(ApiError::new(ErrorCode::RateLimited, "slow down"))
        }
    }

    fn server_event(&self, stream: String, kind: &str, data: Value) -> NewEvent {
        let key = &self.identity.key;
        let body = EventBody::new(
            key,
            &self.identity.author(),
            &stream,
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        );
        NewEvent {
            jws: body.sign(key),
            jws_index: None,
            body,
        }
    }

    async fn wait_applied(&self, position: u64) -> Result<(), ApiError> {
        let mut rx = self.applied.clone();
        match tokio::time::timeout(APPLY_TIMEOUT, rx.wait_for(|p| *p >= position)).await {
            Ok(Ok(_)) => Ok(()),
            _ => Err(ApiError::new(
                ErrorCode::Unavailable,
                "the write is stored but not yet readable",
            )),
        }
    }

    /// `POST /v1/accounts`: a JWS by the new key over `{gamertag, jwk,
    /// invite?}`. Appends, as the server, `GamertagReserved` (expected
    /// version 0: the uniqueness check), `AccountCreated` and `KeyAdded`
    /// (carrying the request as proof of possession), atomically.
    pub async fn create_account(&self, jws: &str) -> Result<CreatedAccount, ApiError> {
        if !self.creates.allow("accounts.create") {
            return Err(ApiError::new(
                ErrorCode::RateLimited,
                "too many new accounts; try again shortly",
            ));
        }
        let (req, key) = AccountRequest::open(jws).map_err(sign_error)?;
        let tag = Gamertag::new(&req.gamertag).map_err(|e| invalid(e.to_string()))?;
        let now = now();
        let invite = match (&req.invite, self.policy.invite_only) {
            (Some(code), true) => {
                let invite = Invite::open(code, self.identity.key.public(), &self.identity.id, now)
                    .map_err(|e| forbidden(format!("invite: {e}")))?;
                Some(invite.jti)
            }
            (None, true) => return Err(forbidden("this server needs an invite code")),
            (_, false) => None,
        };
        {
            let s = self.model.read();
            if s.account_by_tag(&tag).is_some() {
                return Err(conflict("gamertag is taken"));
            }
            if s.keys.contains_key(key.kid()) {
                return Err(conflict("that key is already registered"));
            }
        }
        if let Some(jti) = &invite
            && !self.model.nonces.insert_new(invite_key(jti), i64::MAX, now)
        {
            return Err(forbidden("invite already used"));
        }
        let result = self.create(&tag, &key, jws, invite.as_deref()).await;
        if result.is_err()
            && let Some(jti) = &invite
        {
            self.model.nonces.remove(&invite_key(jti));
        }
        result
    }

    async fn create(
        &self,
        tag: &Gamertag,
        key: &PublicKey,
        proof: &str,
        invite: Option<&str>,
    ) -> Result<CreatedAccount, ApiError> {
        let tag_stream = streams::gamertag(tag);
        // A random XUID can collide; the account stream's version check
        // catches it.
        for _ in 0..3 {
            let xuid = generate_xuid();
            let account = streams::account(xuid);
            let mut created = json!({"xuid": xuid, "gamertag": tag});
            if let Some(jti) = invite {
                created["invite"] = json!(jti);
            }
            let events = vec![
                self.server_event(
                    tag_stream.clone(),
                    types::GAMERTAG_RESERVED,
                    json!({"xuid": xuid}),
                ),
                self.server_event(account.clone(), types::ACCOUNT_CREATED, created),
                self.server_event(
                    account.clone(),
                    types::KEY_ADDED,
                    json!({"kid": key.kid(), "jwk": key.jwk(), "proof": proof}),
                ),
            ];
            match self
                .store
                .append(events, &[(tag_stream.clone(), 0), (account, 0)])
                .await
            {
                Ok(recorded) => {
                    self.wait_applied(recorded.last().map_or(0, |e| e.position))
                        .await?;
                    let id = Author::Account {
                        xuid,
                        server: self.identity.id.clone(),
                    }
                    .to_string();
                    return Ok(CreatedAccount {
                        xuid,
                        gamertag: tag.clone(),
                        id,
                    });
                }
                Err(StoreError::Conflict { stream, .. }) if stream == tag_stream => {
                    return Err(conflict("gamertag is taken"));
                }
                Err(StoreError::Conflict { .. }) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(ApiError::new(
            ErrorCode::Unavailable,
            "could not allocate a XUID",
        ))
    }

    /// `POST /v1/events`: one signed event (security §2), by the token's
    /// author.
    pub async fn append_signed(
        &self,
        bearer: Option<&str>,
        jws: &str,
    ) -> Result<WireEvent, ApiError> {
        let author = self.authenticate(bearer, lxm::EVENTS_WRITE)?;
        self.rate_limit(&author, lxm::EVENTS_WRITE)?;
        let event = SignedEvent::open(jws).map_err(sign_error)?;
        let body = &event.body;
        if body.author != author.to_string() {
            return Err(forbidden("the event's author is not the token's issuer"));
        }
        let key = self
            .key_for(&author, &body.kid)
            .ok_or_else(|| unauthorized("the event's key is unknown or revoked"))?;
        event.verify(&key).map_err(sign_error)?;
        let now = now();
        let at = parse_time(&body.at)
            .ok_or_else(|| invalid("at is not an RFC 3339 time"))?
            .unix_timestamp();
        if (at - now).abs() > MAX_EVENT_SKEW_S {
            return Err(invalid("at is more than 5 minutes from the server's clock"));
        }
        if !may_write(&author, &body.stream, &body.kind) {
            return Err(forbidden(format!(
                "{author} may not write {} to {}",
                body.kind, body.stream
            )));
        }
        validate(&self.model.read(), body)?;
        if !self.model.nonces.insert_new(
            nonce_key(&body.author, &body.nonce),
            at + MAX_EVENT_SKEW_S,
            now,
        ) {
            return Err(conflict("nonce already used"));
        }
        let new = NewEvent {
            jws: jws.to_string(),
            jws_index: None,
            body: event.body,
        };
        let recorded = self.store.append(vec![new], &[]).await?;
        let event = recorded.into_iter().next().expect("one appended");
        self.wait_applied(event.position).await?;
        Ok(event.to_wire())
    }

    /// `PUT /v1/titles/{title}/files/{name}`: the operator (a token signed
    /// by the server key) publishes a title file.
    pub async fn publish_file(
        &self,
        bearer: Option<&str>,
        title: TitleId,
        name: &str,
        bytes: &[u8],
    ) -> Result<PublishedFile, ApiError> {
        let author = self.authenticate(bearer, lxm::TITLES_WRITE)?;
        if !matches!(author, Author::Server(_)) {
            return Err(forbidden("only the server operator publishes title files"));
        }
        self.rate_limit(&author, lxm::TITLES_WRITE)?;
        if !valid_file_name(name) {
            return Err(invalid("file names are [A-Za-z0-9_.-]{1,64}"));
        }
        if bytes.len() > MAX_TITLE_FILE {
            return Err(ApiError::new(
                ErrorCode::TooLarge,
                "title files are at most 2 MiB",
            ));
        }
        let meta = PublishedFile {
            title_id: title,
            name: name.into(),
            sha256: sign::hex(&Sha256::digest(bytes)),
            size: bytes.len(),
        };
        self.store.put_blob(&meta.sha256, bytes).await?;
        let data = serde_json::to_value(&meta).expect("serializes");
        let event = self.server_event(
            streams::title_file(title, name),
            types::TITLE_FILE_PUBLISHED,
            data,
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(meta)
    }

    pub fn discovery(&self) -> Discovery {
        Discovery {
            id: self.identity.id.clone(),
            api: "/v1".into(),
            keys: vec![self.identity.key.public().jwk()],
            software: format!("h2recomp live-server {}", env!("CARGO_PKG_VERSION")),
            federation: false,
        }
    }

    pub async fn events(&self, query: &Query) -> Result<Page<WireEvent>, ApiError> {
        let position = self.position();
        let items: Vec<WireEvent> = self
            .store
            .read(query)
            .await?
            .iter()
            .map(RecordedEvent::to_wire)
            .collect();
        let next = (items.len() == query.limit)
            .then(|| items.last().map(|e| e.position.to_string()))
            .flatten();
        Ok(Page {
            items,
            next,
            position,
        })
    }

    fn summary(&self, s: &State, a: &crate::model::Account) -> AccountSummary {
        AccountSummary {
            xuid: a.xuid,
            gamertag: a.gamertag.clone(),
            created_at: a.created_at.clone(),
            online: s.presence.contains_key(&a.xuid),
        }
    }

    /// Accounts in creation order; the cursor is the last one's position.
    pub fn accounts(&self, after: u64, limit: usize) -> Page<AccountSummary> {
        let position = self.position();
        let s = self.model.read();
        let page: Vec<_> = s.accounts_after(after).take(limit).collect();
        let next = (page.len() == limit)
            .then(|| page.last().map(|a| a.position.to_string()))
            .flatten();
        Page {
            items: page.iter().map(|a| self.summary(&s, a)).collect(),
            next,
            position,
        }
    }

    pub fn account(&self, tag: &Gamertag) -> Result<AccountDetail, ApiError> {
        let position = self.position();
        let s = self.model.read();
        let a = s
            .account_by_tag(tag)
            .ok_or_else(|| not_found("no such account"))?;
        Ok(AccountDetail {
            summary: self.summary(&s, a),
            id: Author::Account {
                xuid: a.xuid,
                server: self.identity.id.clone(),
            }
            .to_string(),
            recent_games: Vec::new(),
            position,
        })
    }

    pub fn keys_by_tag(&self, tag: &Gamertag) -> Result<AccountKeys, ApiError> {
        let xuid = self.model.read().account_by_tag(tag).map(|a| a.xuid);
        self.keys_by_xuid(xuid.ok_or_else(|| not_found("no such account"))?)
    }

    pub fn keys_by_xuid(&self, xuid: Xuid) -> Result<AccountKeys, ApiError> {
        let position = self.position();
        let s = self.model.read();
        if !s.accounts.contains_key(&xuid) {
            return Err(not_found("no such account"));
        }
        Ok(AccountKeys {
            keys: s.keys_of(xuid),
            position,
        })
    }

    pub fn presence(&self) -> Page<Presence> {
        let position = self.position();
        let s = self.model.read();
        let mut items: Vec<Presence> = s
            .presence
            .iter()
            .filter_map(|(xuid, p)| {
                let a = s.accounts.get(xuid)?;
                Some(Presence {
                    xuid: *xuid,
                    gamertag: a.gamertag.clone(),
                    state: p.state.clone(),
                    game_id: p.game_id.clone(),
                    since: p.since.clone(),
                })
            })
            .collect();
        items.sort_by_key(|p| p.gamertag.key());
        Page {
            items,
            next: None,
            position,
        }
    }

    pub async fn file(
        &self,
        title: TitleId,
        name: &str,
    ) -> Result<(PublishedFile, Vec<u8>), ApiError> {
        let meta = self
            .model
            .read()
            .files
            .get(&(title, name.to_string()))
            .map(|f| f.meta.clone());
        let meta = meta.ok_or_else(|| not_found("no such title file"))?;
        let bytes = self
            .store
            .blob(&meta.sha256)
            .await?
            .ok_or_else(|| not_found("title file bytes are missing"))?;
        Ok((meta, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_permissions() {
        let me = Xuid(0x0009_0000_0000_0001);
        let acct = Author::Account {
            xuid: me,
            server: "h".into(),
        };
        let server = Author::Server("h".into());
        assert!(may_write(&acct, &streams::account(me), types::KEY_ADDED));
        assert!(may_write(&acct, &streams::presence(me), types::SIGNED_IN));
        assert!(!may_write(
            &acct,
            &streams::account(me),
            types::ACCOUNT_CREATED
        ));
        assert!(!may_write(
            &acct,
            &streams::presence(me),
            types::PRESENCE_EXPIRED
        ));
        assert!(!may_write(
            &acct,
            &streams::presence(Xuid(2)),
            types::SIGNED_IN
        ));
        assert!(!may_write(&acct, "game-1", "PlayerKilled"));
        assert!(!may_write(
            &acct,
            "title-4D530064-file-x",
            types::TITLE_FILE_PUBLISHED
        ));
        assert!(may_write(
            &server,
            &streams::presence(me),
            types::PRESENCE_EXPIRED
        ));
        assert!(!may_write(
            &server,
            &streams::account(me),
            types::ACCOUNT_CREATED
        ));
    }

    #[test]
    fn xuids_look_like_live_xuids() {
        let a = generate_xuid();
        assert_eq!(a.0 >> 48, 0x0009);
        assert_ne!(a, generate_xuid());
    }
}
