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
    Friend, FriendEventData, FriendPresence, FriendRemovedData, FriendRequest, Friends, Gamertag,
    PRESENCE_STATES, Page, Presence, PublishedFile, TitleId, WireEvent, Xuid, lxm, streams, types,
    valid_file_name,
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
/// How many games `GET /v1/accounts/{gamertag}` lists.
const RECENT_GAMES: usize = 10;
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

pub(crate) fn invalid(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, m)
}
pub(crate) fn unauthorized(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Unauthorized, m)
}
pub(crate) fn forbidden(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Forbidden, m)
}
pub(crate) fn conflict(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Conflict, m)
}
pub fn not_found(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::NotFound, m)
}

/// Store failures are logged here and reported without detail.
impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        if matches!(e, StoreError::Held) {
            return ApiError::new(ErrorCode::Unavailable, e.to_string());
        }
        eprintln!("live: store: {e}");
        ApiError::new(ErrorCode::Unavailable, "the event store is unavailable")
    }
}

/// A signature problem is the caller's (401); a malformed body is 400.
pub(crate) fn sign_error(e: SignError) -> ApiError {
    match e {
        SignError::BadSignature | SignError::KidMismatch => unauthorized(e.to_string()),
        _ => invalid(e.to_string()),
    }
}

/// Server switches (docs/live/live-security.md §3, §6).
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
    /// Presence with no event or ping for this long expires.
    pub presence_ttl_s: i64,
    /// A live game with no event or frame for this long is ended as lost.
    pub game_idle_s: i64,
    /// Idle advertisements expire; presence pings refresh their host deadline.
    pub session_idle_s: i64,
    /// Co-op relay, per socket: messages per second, burst.
    pub relay_rate: (f64, u32),
    /// How often the presence and game managers look.
    pub sweep_every: Duration,
    /// Admin gamertags from --admin/H2_LIVE_ADMINS (docs/live/live-admin.md).
    pub admins: Vec<Gamertag>,
    /// The public base URL, when served through a tunnel or proxy
    /// (docs/live/live-hosting.md); set while running, shown in discovery.
    pub public: crate::public::PublicUrl,
    /// Custom content: caps, quota and upload rate (docs/live/live-admin.md).
    pub content: crate::content::ContentLimits,
    /// Cloud saves: item cap, quota and item count per account
    /// (docs/live/live-cloud-saves.md).
    pub storage: crate::cloud_saves::StorageLimits,
    /// Account recovery lockout per gamertag: wrong codes allowed, the
    /// first lockout, its cap (live-security.md §1, Recovery).
    pub recover_lockout: (u32, Duration, Duration),
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            private: false,
            invite_only: false,
            write_rate: (10.0, 30),
            create_rate: (1.0, 10),
            presence_ttl_s: 90,
            game_idle_s: 300,
            session_idle_s: 300,
            relay_rate: (240.0, 480),
            sweep_every: Duration::from_secs(5),
            admins: Vec::new(),
            public: Default::default(),
            content: Default::default(),
            storage: Default::default(),
            recover_lockout: (
                crate::recovery::FREE_FAILURES,
                crate::recovery::LOCK_BASE,
                crate::recovery::LOCK_MAX,
            ),
        }
    }
}

pub struct Live {
    pub(crate) store: Arc<dyn EventStore>,
    /// `store`, which holds writes while a log is copied in.
    pub(crate) held: Arc<crate::hold::HeldStore>,
    pub(crate) model: Arc<Model>,
    pub(crate) admins: Arc<crate::admin::Admins>,
    pub(crate) identity: ServerIdentity,
    policy: Policy,
    applied: watch::Receiver<u64>,
    feed: broadcast::Sender<Arc<RecordedEvent>>,
    /// Every sent frame, for JSON stream readers.
    pub(crate) frames: broadcast::Sender<Arc<crate::games::LiveFrame>>,
    /// Packed frames, batched a tick at a time for packed readers.
    pub(crate) packed: crate::packed::Hub,
    pub(crate) ingest: crate::games::IngestState,
    jtis: ReplayGuard,
    writes: RateLimiter,
    creates: RateLimiter,
    team_writes: tokio::sync::Mutex<()>,
    pub(crate) session_writes: tokio::sync::Mutex<()>,
    /// Serializes ranking, so a game is ranked once.
    pub(crate) rank_writes: tokio::sync::Mutex<()>,
    /// Serializes arbitration writes, so a round is registered once per account.
    pub(crate) round_writes: tokio::sync::Mutex<()>,
    pub(crate) local_signin: crate::local_signin::Offers,
    pub(crate) relays: crate::relay::Relays,
    pub(crate) invites: Arc<crate::invites::Invites>,
    /// Recovery secrets and the wrong-code lockout.
    pub(crate) recovery: Arc<crate::recovery::Recovery>,
    /// Custom maps and their content (docs/game/maps.md §5).
    pub(crate) catalog: Arc<crate::content::Catalog>,
    /// Server-wide stats for the stats home (`GET /v1/stats/summary`).
    pub(crate) stats: Arc<crate::stats::Stats>,
    pub(crate) usage: Arc<crate::usage::Usage>,
    pub(crate) uploads: RateLimiter,
    pub(crate) content_writes: tokio::sync::Mutex<()>,
    /// Player reports and their reviews (docs/live/live-admin.md).
    pub(crate) reports: Arc<crate::reports::Reports>,
    /// Reports per reporting account ([`crate::reports::RATE`]).
    pub(crate) report_rate: RateLimiter,
    /// Holds a report's duplicate check and its append together.
    pub(crate) report_writes: tokio::sync::Mutex<()>,
    /// Account bans (docs/live/live-admin.md "Bans").
    pub(crate) bans: Arc<crate::bans::Bans>,
    /// Holds a ban's checks and its append together.
    pub(crate) ban_writes: tokio::sync::Mutex<()>,
    /// Mute lists (docs/live/live-voice.md §3.6).
    pub(crate) mutes: Arc<crate::mutes::Mutes>,
    /// Holds a mute's checks and its append together.
    pub(crate) mute_writes: tokio::sync::Mutex<()>,
    /// Each account's own cloud saves (docs/live/live-cloud-saves.md).
    pub(crate) cloud: crate::cloud_saves::CloudSaves,
    /// Unix seconds the service started.
    pub(crate) started_at: i64,
    data_dir: crate::status::DataDir,
    /// Where content, cloud saves and published art are kept
    /// ([`crate::blobs`]); the data directory unless [`Live::set_blobs`].
    blobs: std::sync::OnceLock<crate::blobs::Blobs>,
    /// Shared game art in the public blob store, redirected to once it is
    /// uploaded ([`crate::art::PublishedArt`]).
    published_art: std::sync::OnceLock<crate::art::PublishedArt>,
    dispatcher: crate::rt::Task,
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

/// Who may append what (security §2): an account writes its own key,
/// friend and presence events; the server anything but the events it only
/// writes through its own endpoints, and the friend events, which are the
/// accounts' own. Games come with the game endpoints.
fn may_write(author: &Author, stream: &str, kind: &str) -> bool {
    if stream.starts_with("mailbox-") || live_proto::mailbox::is_kind(kind) {
        return matches!(author, Author::Account { .. })
            && stream.starts_with("mailbox-")
            && live_proto::mailbox::is_kind(kind);
    }
    if stream.starts_with("session-")
        || stream.starts_with("reports-")
        || stream.starts_with("rank-")
        || stream.starts_with("round-")
        || stream.starts_with("bot-")
        || stream.starts_with("bots-")
        || stream.starts_with("bans-")
        || stream.starts_with("mutes-")
        || kind == live_proto::rank::CHANGED
        || live_proto::arbitration::KINDS.contains(&kind)
        || live_proto::bots::KINDS.contains(&kind)
        || live_proto::maps::KINDS.contains(&kind)
        || live_proto::reports::KINDS.contains(&kind)
        || live_proto::bans::KINDS.contains(&kind)
        || live_proto::mutes::KINDS.contains(&kind)
    {
        return false;
    }
    if live_proto::team::Action::is_kind(kind) {
        return matches!(author, Author::Account { .. }) && stream.starts_with("team-");
    }
    match author {
        Author::Server(_) => !matches!(
            kind,
            types::ACCOUNT_CREATED
                | types::GAMERTAG_RESERVED
                | types::TITLE_FILE_PUBLISHED
                | types::FRIEND_REQUESTED
                | types::FRIEND_ACCEPTED
                | types::FRIEND_DECLINED
                | types::FRIEND_REMOVED
        ),
        Author::Account { xuid, .. } => {
            (stream == streams::account(*xuid)
                && matches!(
                    kind,
                    types::KEY_ADDED
                        | types::KEY_REVOKED
                        | types::FRIEND_REQUESTED
                        | types::FRIEND_ACCEPTED
                        | types::FRIEND_DECLINED
                        | types::FRIEND_REMOVED
                ))
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
    if live_proto::mailbox::is_kind(&body.kind) {
        return crate::mailbox::validate(state, body);
    }
    if live_proto::team::Action::is_kind(&body.kind) {
        return crate::teams::validate(state, body);
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
            let d: live_proto::PresenceData = parse_data(&body.data)?;
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
                if d.activity
                    .as_deref()
                    .is_some_and(|a| !live_proto::valid_activity(a))
                {
                    return Err(invalid(
                        "presence activity is empty, too long or has control characters",
                    ));
                }
                if d.build.as_deref().is_some_and(|b| !live_proto::valid_build(b)) {
                    return Err(invalid("presence build must be 7 to 12 lowercase hex digits"));
                }
                if !live_proto::valid_capabilities(&d.capabilities) {
                    return Err(invalid(
                        "presence capabilities are at most 16 of name/version, \
                         lowercase, at most 32 characters each",
                    ));
                }
            }
        }
        types::FRIEND_REQUESTED | types::FRIEND_ACCEPTED | types::FRIEND_DECLINED => {
            let d: FriendEventData = parse_data(&body.data)?;
            let xuid = xuid.ok_or_else(|| invalid("not an account stream"))?;
            if body.kind == types::FRIEND_REQUESTED {
                // Sent from the writer's own account to another one that
                // exists, with nothing between the two already.
                if d.from != xuid {
                    return Err(invalid(
                        "a friend request is sent from the writer's own account",
                    ));
                }
                if d.to == d.from {
                    return Err(invalid("a friend request needs another account"));
                }
                if !state.accounts.contains_key(&d.to) {
                    return Err(invalid("no such account"));
                }
                if state.is_friends(d.from, d.to) {
                    return Err(conflict("already friends"));
                }
                if state.pending(d.from, d.to) || state.pending(d.to, d.from) {
                    return Err(conflict("a friend request is already pending"));
                }
            } else {
                // Answered by the account it was addressed to, while it is
                // pending.
                if d.to != xuid {
                    return Err(invalid(
                        "a friend request is answered on the recipient's account",
                    ));
                }
                if !state.pending(d.from, d.to) {
                    return Err(conflict("no pending friend request from that account"));
                }
            }
        }
        types::FRIEND_REMOVED => {
            let d: FriendRemovedData = parse_data(&body.data)?;
            let xuid = xuid.ok_or_else(|| invalid("not an account stream"))?;
            if d.by != xuid {
                return Err(invalid(
                    "a friend removal is written from one of the two accounts",
                ));
            }
            if !(state.is_friends(d.by, d.friend)
                || state.pending(d.by, d.friend)
                || state.pending(d.friend, d.by))
            {
                return Err(conflict(
                    "no friendship or pending request with that account",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Who runs the server's timed work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timers {
    /// The server spawns its own loops: the dispatcher's poll, the
    /// managers' sweep every `sweep_every`, the packed tick.
    Spawned,
    /// Nothing runs on a timer. The host calls [`Live::sweep`] by
    /// [`Live::sweep_due`] and [`Live::publish_packed`] every
    /// [`crate::packed::TICK`] while [`Live::packed_active`]: a Durable
    /// Object, with an alarm, so an idle one can hibernate.
    Driven,
}

impl Live {
    /// Starts the dispatcher and waits until the read model has replayed
    /// the whole log.
    pub async fn start(
        store: Arc<dyn EventStore>,
        identity: ServerIdentity,
        policy: Policy,
    ) -> anyhow::Result<Arc<Self>> {
        Self::start_with(store, identity, policy, Timers::Spawned).await
    }

    /// [`Live::start`], with the timed work run as `timers` says.
    pub async fn start_with(
        store: Arc<dyn EventStore>,
        identity: ServerIdentity,
        policy: Policy,
        timers: Timers,
    ) -> anyhow::Result<Arc<Self>> {
        let held = Arc::new(crate::hold::HeldStore::open(store).await?);
        let store: Arc<dyn EventStore> = held.clone();
        let model = Arc::new(Model::default());
        let admins = Arc::new(crate::admin::Admins::new(policy.admins.clone()));
        let mut dispatcher = Dispatcher::new(store.clone());
        dispatcher.subscribe(model.clone());
        dispatcher.subscribe(admins.clone());
        let invites = Arc::new(crate::invites::Invites::new(
            identity.derive(crate::invites::CODE_LABEL),
        ));
        dispatcher.subscribe(invites.clone());
        let (free, base, max) = policy.recover_lockout;
        let recovery = Arc::new(crate::recovery::Recovery::new(
            &identity,
            crate::guard::Backoff::new(free, base, max),
        ));
        dispatcher.subscribe(recovery.clone());
        let catalog = Arc::new(crate::content::Catalog::default());
        dispatcher.subscribe(catalog.clone());
        let reports = Arc::new(crate::reports::Reports::default());
        dispatcher.subscribe(reports.clone());
        let bans = Arc::new(crate::bans::Bans::default());
        dispatcher.subscribe(bans.clone());
        let mutes = Arc::new(crate::mutes::Mutes::default());
        dispatcher.subscribe(mutes.clone());
        let stats = Arc::new(crate::stats::Stats::with_model(model.clone()));
        dispatcher.subscribe(stats.clone());
        let usage = Arc::new(crate::usage::Usage::default());
        usage.load(&store.usage().await?);
        dispatcher.subscribe(usage.clone());
        let mut applied = dispatcher.applied();
        let feed = dispatcher.feed();
        let wake = store.wake().await?;
        let head = store.head().await?;
        let poll = (timers == Timers::Spawned).then_some(crate::dispatch::POLL);
        let dispatcher = crate::rt::spawn(dispatcher.run_with(wake, poll));
        let live = Live {
            store,
            held,
            model,
            admins,
            identity,
            applied: applied.clone(),
            feed,
            frames: broadcast::channel(256).0,
            packed: Default::default(),
            ingest: Default::default(),
            jtis: ReplayGuard::default(),
            writes: RateLimiter::new(policy.write_rate.0, policy.write_rate.1),
            creates: RateLimiter::new(policy.create_rate.0, policy.create_rate.1),
            team_writes: Default::default(),
            session_writes: Default::default(),
            rank_writes: Default::default(),
            round_writes: Default::default(),
            local_signin: Default::default(),
            relays: Default::default(),
            invites,
            recovery,
            catalog,
            stats,
            usage,
            uploads: RateLimiter::new(policy.content.upload_rate.0, policy.content.upload_rate.1),
            content_writes: Default::default(),
            reports,
            report_rate: RateLimiter::new(crate::reports::RATE.0, crate::reports::RATE.1),
            report_writes: Default::default(),
            bans,
            ban_writes: Default::default(),
            mutes,
            mute_writes: Default::default(),
            cloud: Default::default(),
            started_at: now(),
            data_dir: Default::default(),
            blobs: Default::default(),
            published_art: Default::default(),
            policy,
            dispatcher,
        };
        applied.wait_for(|p| *p >= head).await?;
        let live = Arc::new(live);
        if timers == Timers::Driven {
            return Ok(live);
        }
        // The presence and game managers; they stop with the service.
        let weak = Arc::downgrade(&live);
        let every = live.policy.sweep_every;
        crate::rt::spawn(async move {
            loop {
                crate::rt::sleep(every).await;
                let Some(live) = weak.upgrade() else { return };
                live.sweep().await;
            }
        });
        // Packed readers' batches, a tick at a time.
        let weak = Arc::downgrade(&live);
        crate::rt::spawn(async move {
            let mut tick = crate::rt::interval(crate::packed::TICK);
            loop {
                tick.tick().await;
                let Some(live) = weak.upgrade() else { return };
                live.publish_packed();
            }
        });
        Ok(live)
    }

    pub fn id(&self) -> &str {
        &self.identity.id
    }

    /// The co-op relay rooms.
    pub fn relays(&self) -> &crate::relay::Relays {
        &self.relays
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Records where the server keeps its key and log, for the admin
    /// status (shown only to callers on this machine). Set once.
    pub fn set_data_dir(&self, dir: &std::path::Path) {
        let _ = self.data_dir.set(dir.to_path_buf());
        #[cfg(feature = "native")]
        if self.blobs.get().is_none() {
            match crate::blobs::Blobs::dir(dir) {
                Ok(b) => {
                    let _ = self.blobs.set(b);
                }
                Err(e) => eprintln!("live: blobs in {}: {e}", dir.display()),
            }
        }
    }

    /// Keeps blobs in `blobs` rather than the data directory. Before
    /// [`Live::set_data_dir`]; set once.
    pub fn set_blobs(&self, blobs: crate::blobs::Blobs) {
        let _ = self.blobs.set(blobs);
    }

    pub fn blobs(&self) -> Option<&crate::blobs::Blobs> {
        self.blobs.get()
    }

    /// Redirects `/art/game/*` to `art`'s public copies once it has
    /// published them. Before the router is built; set once.
    pub fn set_published_art(&self, art: crate::art::PublishedArt) {
        let _ = self.published_art.set(art);
    }

    pub fn published_art(&self) -> Option<&crate::art::PublishedArt> {
        self.published_art.get()
    }

    pub fn data_dir(&self) -> Option<&std::path::Path> {
        self.data_dir.get().map(|p| p.as_path())
    }

    /// Makes an account an admin until the process stops: the embedded
    /// server's playing account, on the owner's own machine
    /// (docs/live/live-admin.md).
    pub fn grant_admin_runtime(&self, xuid: Xuid, tag: &str) {
        self.admins.grant_runtime(xuid, tag);
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
    pub(crate) fn key_for(&self, author: &Author, kid: &str) -> Option<PublicKey> {
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
        if let Author::Account { xuid, .. } = &author
            && live_proto::bans::bars(method)
        {
            let refused = self
                .bans
                .read()
                .in_force(*xuid, now)
                .map(crate::bans::banned_error);
            if let Some(e) = refused
                && !self.admins.is_admin_xuid(*xuid, &self.model.read())
            {
                return Err(e);
            }
        }
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

    /// Who is reading: an account when a `read` bearer is presented (always
    /// checked, even on a public server, so a bad token is an error rather
    /// than a silently anonymous read), None for an anonymous public read.
    /// Lists whose answer depends on the reader (friends-only sessions) use it.
    pub fn read_caller(&self, bearer: Option<&str>) -> Result<Option<Xuid>, ApiError> {
        if bearer.is_none() {
            self.authorize_read(None)?;
            return Ok(None);
        }
        Ok(match self.authenticate(bearer, lxm::READ)? {
            Author::Account { xuid, .. } => Some(xuid),
            Author::Server(_) => None,
        })
    }

    /// The token's expiry, for WebSockets that must close when it passes.
    pub fn authenticate_until(&self, bearer: &str, method: &str) -> Result<i64, ApiError> {
        self.authenticate(Some(bearer), method)?;
        Ok(Token::parse(bearer).map(|t| t.claims.exp).unwrap_or(0))
    }

    pub(crate) fn rate_limit(&self, author: &Author, method: &str) -> Result<(), ApiError> {
        if self.writes.allow(&format!("{author} {method}")) {
            Ok(())
        } else {
            Err(ApiError::new(ErrorCode::RateLimited, "slow down"))
        }
    }

    pub(crate) fn server_event(&self, stream: String, kind: &str, data: Value) -> NewEvent {
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
            jws: body.sign(key).into(),
            jws_index: None,
            body,
        }
    }

    pub(crate) async fn wait_applied(&self, position: u64) -> Result<(), ApiError> {
        let mut rx = self.applied.clone();
        match crate::rt::timeout(APPLY_TIMEOUT, rx.wait_for(|p| *p >= position)).await {
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
        let (req, key) = AccountRequest::open(jws, &self.identity.id, now()).map_err(sign_error)?;
        let tag = Gamertag::new(&req.gamertag).map_err(|e| invalid(e.to_string()))?;
        let now = now();
        let invite = match (&req.invite, self.policy.invite_only) {
            // A signed code (the older, long form) or a short one.
            (Some(code), true) if live_proto::sign::Jws::parse(code.trim()).is_ok() => {
                let invite = Invite::open(
                    code.trim(),
                    self.identity.key.public(),
                    &self.identity.id,
                    now,
                )
                .map_err(|e| forbidden(format!("invite: {e}")))?;
                crate::invites::check_redeemable(self, &invite.jti)?;
                Some(invite.jti)
            }
            (Some(code), true) => Some(crate::invites::redeem_short(self, code, now)?),
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
        let secret = live_proto::recovery::TotpSecret::generate();
        // A random XUID can collide; the account stream's version check
        // catches it.
        for _ in 0..3 {
            let xuid = generate_xuid();
            let account = streams::account(xuid);
            let mut created = json!({"xuid": xuid, "gamertag": tag});
            created["recovery"] = self.recovery.set_data(&secret)["sealed"].clone();
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
                        recovery: Some(self.recovery.info(&secret, tag.as_str(), now())),
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

    /// The server-wide limit on account creation, shared by recovery.
    pub(crate) fn allow_create(&self, what: &str) -> bool {
        self.creates.allow(what)
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
        // Serialize mailbox validation with host delete/expiry and other
        // mailbox mutations so two concurrent answers cannot both commit.
        let _mailbox_guard = if live_proto::mailbox::is_kind(&body.kind) {
            Some(self.session_writes.lock().await)
        } else {
            None
        };
        let _team_guard = if live_proto::team::Action::is_kind(&body.kind) {
            Some(self.team_writes.lock().await)
        } else {
            None
        };
        validate(&self.model.read(), body)?;
        if !self.model.nonces.insert_new(
            nonce_key(&body.author, &body.nonce),
            at + MAX_EVENT_SKEW_S,
            now,
        ) {
            return Err(conflict("nonce already used"));
        }
        let new = NewEvent {
            jws: jws.into(),
            jws_index: None,
            body: event.body,
        };
        let mut writes = vec![new];
        if writes[0].body.kind == "TeamMemberRecruited"
            && let Ok(live_proto::team::Action::Recruit { id, target, .. }) =
                serde_json::from_value(writes[0].body.data.clone())
        {
            writes.push(self.server_event(
                streams::account(target.xuid),
                "TeamRecruitReceived",
                json!({"team_id":id,"to":target.xuid,"from":author.to_string()}),
            ));
        }
        let recorded = self.store.append(writes, &[]).await?;
        let end = recorded.last().expect("at least one append").position;
        self.wait_applied(end).await?;
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
        crate::admin::require_admin(self, &author)?;
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

    /// The operator (a token signed by the server key) for `method`.
    pub fn authorize_operator(&self, bearer: Option<&str>, method: &str) -> Result<(), ApiError> {
        let author = self.authenticate(bearer, method)?;
        if !matches!(author, Author::Server(_)) {
            return Err(forbidden("only the server operator may do this"));
        }
        Ok(())
    }

    /// `POST /v1/presence/ping`: keeps the caller's presence alive without
    /// writing anything. Returns seconds until it would expire.
    pub fn ping(&self, bearer: Option<&str>) -> Result<i64, ApiError> {
        let author = self.authenticate(bearer, lxm::EVENTS_WRITE)?;
        self.ping_as(&author)
    }

    pub(crate) fn ping_as(&self, author: &Author) -> Result<i64, ApiError> {
        let Author::Account { xuid, .. } = author else {
            return Err(forbidden("only accounts have presence"));
        };
        if !self.model.touch_presence(*xuid, now()) {
            return Err(not_found("not signed in: append SignedIn first"));
        }
        self.model.touch_host_sessions(*xuid, now());
        Ok(self.policy.presence_ttl_s)
    }

    pub fn discovery(&self) -> Discovery {
        Discovery {
            id: self.identity.id.clone(),
            api: "/v1".into(),
            keys: vec![self.identity.key.public().jwk()],
            software: format!("h2recomp live-server {}", env!("CARGO_PKG_VERSION")),
            federation: false,
            url: self.policy.public.get(),
        }
    }

    pub async fn events(&self, query: &Query) -> Result<Page<WireEvent>, ApiError> {
        self.events_for(query, crate::sessions::Viewer::Operator)
            .await
    }

    /// `GET /v1/events` as `viewer` reads it: the events it may not see
    /// ([`crate::sessions::event_shows_to`]) are left out, but the cursor
    /// still moves past them, so a page can be short, or empty with a
    /// `next`: read on until `next` is null.
    pub async fn events_for(
        &self,
        query: &Query,
        viewer: crate::sessions::Viewer,
    ) -> Result<Page<WireEvent>, ApiError> {
        let position = self.position();
        let read = self.store.read(query).await?;
        let next = (read.len() == query.limit)
            .then(|| read.last().map(|e| e.position.to_string()))
            .flatten();
        let items: Vec<WireEvent> = {
            let s = self.model.read();
            read.iter()
                .filter_map(|e| match viewer {
                    crate::sessions::Viewer::Operator => Some(e.to_wire()),
                    _ => crate::sessions::wire_for(&s, e, viewer),
                })
                .collect()
        };
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
            admin: self.admins.is_admin_of(&a.gamertag, a.xuid),
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

    /// One account and its recent games, those `viewer` may see
    /// ([`crate::model::Game::shows_to`]).
    pub fn account(
        &self,
        tag: &Gamertag,
        viewer: crate::sessions::Viewer,
    ) -> Result<AccountDetail, ApiError> {
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
            recent_games: s
                .games_newest()
                .filter(|g| g.xuids.contains(&a.xuid))
                .filter(|g| g.shows_to(viewer, |x, y| s.is_friends(x, y)))
                .take(RECENT_GAMES)
                .map(|g| g.summary())
                .collect(),
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

    /// Who is online, with each player's game id and activity as `viewer`
    /// may see them ([`crate::sessions::presence_for`]). Build and
    /// capabilities show to every reader: they name the console's software,
    /// not where its player is, and a host reads its guests' before a game
    /// whether or not they are friends (docs/game/game-options.md section 3).
    pub fn presence(&self, viewer: crate::sessions::Viewer) -> Page<Presence> {
        let position = self.position();
        let s = self.model.read();
        let mut items: Vec<Presence> = s
            .presence
            .iter()
            .filter_map(|(xuid, p)| {
                let a = s.accounts.get(xuid)?;
                let (game_id, activity) = presence_of(&s, *xuid, p, viewer);
                Some(Presence {
                    xuid: *xuid,
                    gamertag: a.gamertag.clone(),
                    state: p.state.clone(),
                    game_id,
                    activity,
                    since: p.since.clone(),
                    build: p.build.clone(),
                    capabilities: p.capabilities.clone(),
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

    /// `GET /v1/accounts/{gamertag}/friends`: the account's friends (each
    /// with its current presence, absent when offline) and its pending
    /// requests in and out, sorted by gamertag — one document, so the
    /// game's friends enumeration needs just this call.
    pub fn friends(
        &self,
        tag: &Gamertag,
        viewer: crate::sessions::Viewer,
    ) -> Result<Friends, ApiError> {
        let position = self.position();
        let s = self.model.read();
        let a = s
            .account_by_tag(tag)
            .ok_or_else(|| not_found("no such account"))?;
        let relations = s.relations_of(a.xuid);
        let request = |(xuid, at): &(Xuid, String)| -> Option<FriendRequest> {
            let a = s.accounts.get(xuid)?;
            Some(FriendRequest {
                xuid: *xuid,
                gamertag: a.gamertag.clone(),
                at: at.clone(),
            })
        };
        let mut friends: Vec<Friend> = relations
            .friends
            .iter()
            .filter_map(|(xuid, since)| {
                let a = s.accounts.get(xuid)?;
                Some(Friend {
                    xuid: *xuid,
                    gamertag: a.gamertag.clone(),
                    friends_since: since.clone(),
                    presence: s.presence.get(xuid).map(|p| {
                        let (game_id, activity) = presence_of(&s, *xuid, p, viewer);
                        FriendPresence {
                            state: p.state.clone(),
                            game_id,
                            activity,
                            since: p.since.clone(),
                            build: p.build.clone(),
                            capabilities: p.capabilities.clone(),
                        }
                    }),
                })
            })
            .collect();
        // Friendships are public; pending requests are the player's own
        // (and the admins'), so anyone else, anonymous included, gets none.
        let own = viewer.is_admin() || viewer == crate::sessions::Viewer::Account(a.xuid);
        let pending = |v: &[(Xuid, String)]| -> Vec<FriendRequest> {
            if own {
                v.iter().filter_map(&request).collect()
            } else {
                Vec::new()
            }
        };
        let mut requests_in = pending(&relations.incoming);
        let mut requests_out = pending(&relations.outgoing);
        friends.sort_by_key(|f| f.gamertag.key());
        requests_in.sort_by_key(|r| r.gamertag.key());
        requests_out.sort_by_key(|r| r.gamertag.key());
        Ok(Friends {
            friends,
            requests_in,
            requests_out,
            position,
        })
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
        let Some(meta) = meta else {
            // Halo 2's playlists until one is published (default_hopper).
            return crate::default_hopper::fallback(title, name)
                .ok_or_else(|| not_found("no such title file"));
        };
        let bytes = self
            .store
            .blob(&meta.sha256)
            .await?
            .ok_or_else(|| not_found("title file bytes are missing"))?;
        Ok((meta, bytes))
    }
}

/// A presence's game id and activity as `viewer` may see them: the private
/// sessions it was set in and the ones its player is in now both count.
fn presence_of(
    s: &State,
    xuid: Xuid,
    p: &crate::model::Presence,
    viewer: crate::sessions::Viewer,
) -> (Option<String>, Option<String>) {
    let mut sessions = p.private_sessions.clone();
    for id in crate::sessions::private_sessions_of(s, xuid) {
        if !sessions.contains(&id) {
            sessions.push(id);
        }
    }
    crate::sessions::presence_for(
        s,
        xuid,
        p.game_id.as_ref(),
        p.activity.as_ref(),
        &sessions,
        viewer,
    )
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
        assert!(may_write(
            &acct,
            &streams::account(me),
            types::FRIEND_REQUESTED
        ));
        assert!(may_write(
            &acct,
            &streams::account(me),
            types::FRIEND_REMOVED
        ));
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
            &streams::presence(me),
            types::FRIEND_REQUESTED
        ));
        assert!(!may_write(
            &acct,
            &streams::account(Xuid(2)),
            types::FRIEND_REQUESTED
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
        // Friendships are the accounts' own; the server does not forge them.
        for kind in [
            types::FRIEND_REQUESTED,
            types::FRIEND_ACCEPTED,
            types::FRIEND_DECLINED,
            types::FRIEND_REMOVED,
        ] {
            assert!(!may_write(&server, &streams::account(me), kind), "{kind}");
        }
    }

    #[test]
    fn friend_event_rules() {
        let me = Xuid(0x0009_0000_0000_0001);
        let other = Xuid(0x0009_0000_0000_0002);
        let nobody = Xuid(0x0009_0000_0000_0003);
        let account = |x: Xuid| crate::model::Account {
            xuid: x,
            gamertag: Gamertag::new("Someone").unwrap(),
            created_at: String::new(),
            position: 1,
        };
        let mut state = crate::model::State::default();
        state.accounts.insert(me, account(me));
        state.accounts.insert(other, account(other));

        let body = |stream: String, kind: &str, data: Value| EventBody {
            stream,
            kind: kind.into(),
            data,
            author: format!("acct:{me}@h"),
            kid: String::new(),
            at: String::new(),
            nonce: String::new(),
        };
        let stream_of = |x: Xuid| streams::account(x);

        // A request is sent from the writer's own account to another that
        // exists, with nothing between the two already.
        assert_eq!(
            validate(
                &state,
                &body(
                    stream_of(me),
                    types::FRIEND_REQUESTED,
                    json!({"from": me, "to": other})
                )
            ),
            Ok(())
        );
        for (stream, data) in [
            (stream_of(me), json!({"from": me, "to": me})),
            (stream_of(me), json!({"from": me, "to": nobody})),
            (stream_of(me), json!({"from": other, "to": me})),
            (stream_of(other), json!({"from": me, "to": other})),
        ] {
            assert!(
                validate(&state, &body(stream, types::FRIEND_REQUESTED, data)).is_err(),
                "a friend request like this must be refused"
            );
        }

        // Once one is pending, it is answered by its recipient only, and
        // no second request fits in either direction.
        state.friends.insert(
            me,
            crate::model::Relations {
                outgoing: vec![(other, String::new())],
                ..Default::default()
            },
        );
        state.friends.insert(
            other,
            crate::model::Relations {
                incoming: vec![(me, String::new())],
                ..Default::default()
            },
        );
        assert_eq!(
            validate(
                &state,
                &body(
                    stream_of(other),
                    types::FRIEND_ACCEPTED,
                    json!({"from": me, "to": other})
                )
            ),
            Ok(())
        );
        assert_eq!(
            validate(
                &state,
                &body(
                    stream_of(other),
                    types::FRIEND_DECLINED,
                    json!({"from": me, "to": other})
                )
            ),
            Ok(())
        );
        for (stream, kind, data) in [
            // Answered on the requester's own stream instead.
            (
                stream_of(me),
                types::FRIEND_ACCEPTED,
                json!({"from": me, "to": other}),
            ),
            // No pending request the other way, either.
            (
                stream_of(me),
                types::FRIEND_ACCEPTED,
                json!({"from": other, "to": me}),
            ),
            // No such pending request.
            (
                stream_of(other),
                types::FRIEND_DECLINED,
                json!({"from": nobody, "to": other}),
            ),
            // A second request, either direction.
            (
                stream_of(me),
                types::FRIEND_REQUESTED,
                json!({"from": me, "to": other}),
            ),
            (
                stream_of(other),
                types::FRIEND_REQUESTED,
                json!({"from": other, "to": me}),
            ),
        ] {
            assert!(
                validate(&state, &body(stream, kind, data)).is_err(),
                "{kind}"
            );
        }

        // Removal needs a friendship or a pending request between the two,
        // written from one of their accounts.
        let mut friends = crate::model::State::default();
        friends.accounts.insert(me, account(me));
        friends.accounts.insert(other, account(other));
        assert!(
            validate(
                &friends,
                &body(
                    stream_of(me),
                    types::FRIEND_REMOVED,
                    json!({"by": me, "friend": other})
                )
            )
            .is_err()
        );
        friends.friends.insert(
            me,
            crate::model::Relations {
                friends: vec![(other, String::new())],
                ..Default::default()
            },
        );
        assert!(
            validate(
                &friends,
                &body(
                    stream_of(other),
                    types::FRIEND_REMOVED,
                    json!({"by": me, "friend": other})
                )
            )
            .is_err()
        );
        assert_eq!(
            validate(
                &friends,
                &body(
                    stream_of(me),
                    types::FRIEND_REMOVED,
                    json!({"by": me, "friend": other})
                )
            ),
            Ok(())
        );
    }

    #[test]
    fn xuids_look_like_live_xuids() {
        let a = generate_xuid();
        assert_eq!(a.0 >> 48, 0x0009);
        assert_ne!(a, generate_xuid());
    }
}
