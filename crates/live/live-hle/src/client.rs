//! Blocking HTTP lives exclusively on this worker. Commands and completions
//! own host data; neither guest pointers nor a guest/HLE lock cross this boundary.
use crate::{config::Account, status::*};
use live_proto::{
    AccountKeys, Author, Discovery, Xuid, bearer_allowed, lxm,
    sign::{EventBody, KeyPair, PublicKey, issue_token},
    streams, types,
};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};
use url::Url;

mod publication;
pub use publication::HostPublisher;
use publication::Writer;

pub(crate) const MAX_FILE: usize = 2 * 1024 * 1024;
pub(crate) const FILES: [(u32, &str); 2] =
    [(1, "message_of_the_day.dat"), (4, "multiplayer_hopper.dat")];
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Boot credential for a playerless machine. Its only HLE service is session publication.
    pub publisher: Option<HostPublisher>,
    pub request_timeout: Duration,
    pub heartbeat_interval: Duration,
    /// Custom maps (docs/game/maps.md §5): sessions on a map that cannot be had
    /// are left out of searches, and finding one by id (a join) downloads
    /// its missing layers first. None: sessions are taken as they come.
    pub maps: Option<Arc<crate::maps::MapGate>>,
    /// How a request that is safe to repeat is retried through a short
    /// outage: a Worker deploy, a dropped connection (docs/live/live.md
    /// "Outages").
    pub retry: Retry,
    /// How long the heartbeat may keep failing before the persistent
    /// connection is lost. A refused account (a ban, a bad key) ends it at
    /// once.
    pub connection_grace: Duration,
    /// This console's build (its git short hash, [`live_proto::valid_build`])
    /// and what it can do ([`live_proto::capability`]), sent with every
    /// presence set (docs/game/game-options.md section 3, "Older builds").
    /// A build that isn't valid is left out, as are bad capabilities.
    pub build: Option<String>,
    pub capabilities: Vec<String>,
}
impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            publisher: None,
            request_timeout: Duration::from_secs(5),
            heartbeat_interval: Duration::from_secs(30),
            maps: None,
            retry: Retry::default(),
            connection_grace: Duration::from_secs(90),
            build: None,
            capabilities: Vec::new(),
        }
    }
}
/// Retries of a request that is safe to repeat: a read, a keep-alive, a
/// PUT or DELETE, or a signed event (the server takes an event once by its
/// nonce, so a resend of one whose answer was lost is refused as a replay,
/// which is taken as success). Transport failures, 5xx and 429 are retried;
/// any other answer is final.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    /// Attempts in all, the first included: 1 is no retry.
    pub attempts: u32,
    /// The wait before the first retry; it doubles after each.
    pub backoff: Duration,
    /// The longest wait between attempts, a server's `Retry-After` included.
    pub max_wait: Duration,
}
impl Default for Retry {
    fn default() -> Self {
        Self {
            attempts: 3,
            backoff: Duration::from_millis(500),
            max_wait: Duration::from_secs(5),
        }
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct ServerStatus {
    pub acknowledged_pings: u64,
    pub last_error: u32,
}
#[derive(Default)]
struct Metrics {
    pings: AtomicU64,
    last_error: AtomicU32,
}
#[derive(Clone)]
pub(crate) struct File {
    pub id: u32,
    pub bytes: Arc<[u8]>,
}
#[derive(Clone)]
pub(crate) enum Reply {
    Logon,
    File(File),
    Done,
    Teams(Vec<live_proto::team::Team>),
    Matches(Vec<live_proto::session::Session>),
    Matchmaking(Box<crate::matchmaking::Step>),
    MatchmakingUnsupported,
    Social(SocialSnapshot),
    InviteAccepted(live_proto::mailbox::Invite),
    JoinLink(Result<live_proto::mailbox::Invite, String>),
    /// A mute list, oldest first.
    Mutes(Vec<u64>),
    Ranks(Vec<live_proto::rank::LeaderboardRow>),
    /// A bot roster for the host's session (docs/game/bots.md §9.1), not
    /// yet checked.
    Roster(live_proto::bots::Roster),
    BotCard(live_proto::bots::PlayerCard),
    /// Everyone's presence, as `GET /v1/presence` shows this console.
    Presence(Vec<live_proto::Presence>),
    /// A page of the mod repository (docs/game/maps.md §5, "Mod downloads").
    Mods(crate::mods::ModPage),
    /// The served hopper file's clanker hoppers (docs/game/bots.md §8).
    ClankerHoppers(live_proto::hopper::clankers::ClankerHoppers),
    /// Where content is fetched from (the API base) and the map gate that
    /// installs it, for a download on a thread of its own.
    Content {
        api: String,
        gate: Option<Arc<crate::maps::MapGate>>,
        /// Tokens for the counted manifest GET (the download count).
        reader: Option<crate::maps::Reader>,
    },
}
#[derive(Clone)]
pub(crate) struct Pending(Arc<Mutex<Option<Result<Reply, u32>>>>);
impl Pending {
    fn new() -> Self {
        Self(Arc::default())
    }
    pub fn take(&self) -> Option<Result<Reply, u32>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
    fn finish(&self, value: Result<Reply, u32>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
    }
}
#[derive(Clone)]
pub(crate) struct Session {
    pub id: u32,
    cancelled: Arc<AtomicBool>,
    status: Arc<AtomicU32>,
}
impl Session {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            cancelled: Arc::new(AtomicBool::new(false)),
            status: Arc::new(AtomicU32::new(0)),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn same_logon(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
    pub fn status(&self) -> u32 {
        self.status.load(Ordering::Acquire)
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Presence {
    pub state: &'static str,
    pub game_id: Option<String>,
    /// What the player is doing, in words ("Co-op: Outskirts").
    pub activity: Option<String>,
}
impl Default for Presence {
    fn default() -> Self {
        Self {
            state: "online",
            game_id: None,
            activity: None,
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) enum TeamRequest {
    User(Xuid),
    Team(Xuid),
    Change(live_proto::team::Action),
}
#[derive(Clone)]
pub(crate) enum MatchRequest {
    Create {
        session: live_proto::session::NewSession,
        assignment: Option<String>,
    },
    Queue {
        ticket: live_proto::matchmaking::Ticket,
        waiting: Option<live_proto::matchmaking::Assignment>,
    },
    JoinDedicated(Box<crate::matchmaking::DedicatedJoin>),
    Update {
        id: String,
        update: live_proto::session::Update,
    },
    Delete(String),
    Search {
        limit: u32,
        attributes: BTreeMap<u32, live_proto::session::AttributeValue>,
        optimatch: Option<live_proto::optimatch::Query>,
    },
    Find(String),
    /// The hosted session's players changed: `PlayerJoined` and
    /// `PlayerLeft` on `session-<id>`, one signed batch each, so a refused
    /// one (a bot of a replaced roster) holds back no other.
    Players {
        id: String,
        joined: Vec<Xuid>,
        left: Vec<Xuid>,
    },
}
#[derive(Clone)]
pub(crate) enum SocialRequest {
    Refresh,
    Send { to: Vec<Xuid>, session_id: String },
    Answer(live_proto::mailbox::Answered),
    Join { from: Xuid, session_id: String },
    JoinLink(String),
}
/// A player report ([`crate::feedback`]).
#[derive(Clone)]
pub(crate) struct ReportRequest {
    pub target: Xuid,
    pub body: live_proto::reports::ReportRequest,
}
#[derive(Clone, PartialEq)]
pub(crate) struct SocialSnapshot {
    pub friends: live_proto::Friends,
    pub invites: Vec<live_proto::mailbox::Invite>,
    pub sessions: Vec<live_proto::session::Session>,
}
enum Work {
    Logon(Vec<Account>),
    Change(Vec<Account>),
    Presence {
        user: u64,
        value: Presence,
    },
    File(&'static str, u32),
    Teams {
        user: u64,
        request: TeamRequest,
    },
    Match(MatchRequest),
    Social { user: u64, request: SocialRequest },
    Report { user: u64, request: ReportRequest },
    MuteList { user: u64 },
    Mute { user: u64, target: u64, mute: bool },
    Arbitration {
        path: String,
        body: String,
    },
    Ranks {
        leaderboards: Vec<u32>,
        xuids: Vec<u64>,
        game_session: Option<String>,
    },
    /// `POST /games/bots` (docs/game/bots.md §9.1), as the host of the
    /// request's session.
    Bots(live_proto::bots::RosterRequest),
    BotCard(Xuid),
    /// `GET /v1/presence`.
    ReadPresence,
    /// `GET /v1/maps?sort=new` (the mod repository), as the first account
    /// signed in on this console.
    Mods { limit: u32, cursor: Option<String> },
    /// The API base and map gate, for a mod download.
    ContentApi,
    /// `GET /v1/hoppers/clankers` (docs/game/bots.md §8), as the first
    /// account signed in on this console.
    ClankerHoppers,
    Wake,
}
/// Called on the worker with a command's result, once it has one.
pub(crate) type Then = Box<dyn FnOnce(Result<Reply, u32>) + Send>;
struct Command {
    session: Session,
    work: Work,
    result: Pending,
    then: Option<Then>,
}
#[derive(Clone)]
pub(crate) struct Service {
    pub machine: bool,
    sender: SyncSender<Command>,
    metrics: Arc<Metrics>,
    /// The sessions the worker read most recently: what their hosts advertised
    /// (the skulls, docs/game/skulls.md "In multiplayer"; a co-op
    /// campaign, docs/live/live-coop.md) and who is in it.
    pub seen: Seen,
}
pub(crate) type Seen = Arc<Mutex<SessionCache>>;

/// Keep at least a full server page, plus room for individual finds.
const MAX_SEEN_SESSIONS: usize = 1024;

#[derive(Default)]
pub(crate) struct SessionCache {
    by_id: BTreeMap<String, (u64, live_proto::session::Session)>,
    by_age: BTreeMap<u64, String>,
    next: u64,
}

impl SessionCache {
    pub(crate) fn get(&self, id: &str) -> Option<&live_proto::session::Session> {
        self.by_id.get(id).map(|(_, session)| session)
    }

    fn remember(&mut self, session: &live_proto::session::Session) {
        let id = &session.session_id;
        self.next += 1;
        if let Some((old, _)) = self.by_id.insert(id.clone(), (self.next, session.clone())) {
            self.by_age.remove(&old);
        }
        self.by_age.insert(self.next, id.clone());
        if self.by_id.len() > MAX_SEEN_SESSIONS {
            let (_, oldest) = self.by_age.pop_first().expect("the cache has an entry");
            self.by_id.remove(&oldest);
        }
    }
}
impl Service {
    pub fn start(base: String, options: ServerOptions) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Command>(64);
        let metrics = Arc::new(Metrics::default());
        let counters = metrics.clone();
        let seen = Seen::default();
        let seen_by_worker = seen.clone();
        let machine = options.publisher.is_some();
        std::thread::Builder::new()
            .name("h2-live-http".into())
            .spawn(move || {
                let agent = ureq::AgentBuilder::new()
                    .timeout(options.request_timeout)
                    .redirects(0)
                    .build();
                let mut worker = Worker {
                    base,
                    maps: options.maps.clone(),
                    software: Software::from_options(&options),
                    agent,
                    publisher: options.publisher,
                    connection: None,
                    active: None,
                    metrics: counters,
                    seen: seen_by_worker,
                    retry: options.retry,
                    grace: options.connection_grace,
                    retry_after: Cell::new(None),
                };
                let mut next_ping = Instant::now() + options.heartbeat_interval;
                loop {
                    worker.cancelled();
                    if Instant::now() >= next_ping {
                        worker.ping();
                        next_ping = Instant::now() + options.heartbeat_interval;
                    }
                    let wait = next_ping.saturating_duration_since(Instant::now());
                    match receiver.recv_timeout(wait) {
                        Ok(mut command) => {
                            let result = worker.run(&command);
                            if result.is_ok()
                                && let Work::Presence { user, .. } = &command.work
                                && let Some(active) = worker.active.as_mut()
                            {
                                // A newer presence went through: nothing older is owed.
                                active.unsent.remove(user);
                            }
                            if let Err(error) = result
                                && !command.session.cancelled()
                            {
                                worker.metrics.last_error.store(error, Ordering::Release);
                                if let Work::Presence { user, value } = &command.work {
                                    worker.presence_failed(&command.session, *user, value, error);
                                }
                            }
                            if let Some(then) = command.then.take() {
                                then(result.clone());
                            }
                            command.result.finish(result);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            worker.sign_out();
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            machine,
            sender,
            metrics,
            seen,
        })
    }
    fn submit(&self, session: &Session, work: Work) -> Result<Pending, u32> {
        self.submit_then(session, work, None)
    }
    fn submit_then(
        &self,
        session: &Session,
        work: Work,
        then: Option<Then>,
    ) -> Result<Pending, u32> {
        let result = Pending::new();
        self.sender
            .try_send(Command {
                session: session.clone(),
                work,
                result: result.clone(),
                then,
            })
            .map_err(|_| TASK_BUSY)?;
        Ok(result)
    }
    /// Asks the server for a bot roster for the host's session
    /// (`POST /games/bots`, docs/game/bots.md §9.1); `then` gets the
    /// answer on the worker.
    pub fn bots(
        &self,
        s: &Session,
        request: live_proto::bots::RosterRequest,
        then: Then,
    ) -> Result<(), u32> {
        self.submit_then(s, Work::Bots(request), Some(then))
            .map(|_| ())
    }
    pub fn bot_card(&self, s: &Session, xuid: Xuid, then: Then) -> Result<(), u32> {
        self.submit_then(s, Work::BotCard(xuid), Some(then))
            .map(|_| ())
    }
    pub fn logon(&self, s: &Session, accounts: Vec<Account>) -> Result<Pending, u32> {
        self.submit(s, Work::Logon(accounts))
    }
    pub fn change(&self, s: &Session, accounts: Vec<Account>) -> Result<Pending, u32> {
        self.submit(s, Work::Change(accounts))
    }
    pub fn presence(&self, s: &Session, user: u64, value: Presence) -> Result<Pending, u32> {
        self.submit(s, Work::Presence { user, value })
    }
    pub fn file(&self, s: &Session, name: &'static str, id: u32) -> Result<Pending, u32> {
        self.submit(s, Work::File(name, id))
    }
    pub fn teams(&self, s: &Session, user: u64, request: TeamRequest) -> Result<Pending, u32> {
        self.submit(s, Work::Teams { user, request })
    }
    pub fn match_request(&self, s: &Session, request: MatchRequest) -> Result<Pending, u32> {
        self.submit(s, Work::Match(request))
    }
    pub fn social(&self, s: &Session, user: u64, request: SocialRequest) -> Result<Pending, u32> {
        self.submit(s, Work::Social { user, request })
    }
    /// Reports `target` for `user` (docs/live/live-api.md "Player reports").
    pub fn report(
        &self,
        s: &Session,
        user: u64,
        target: Xuid,
        kind: live_proto::reports::Kind,
        reason: String,
        game_id: Option<String>,
    ) -> Result<Pending, u32> {
        let request = ReportRequest {
            target,
            body: live_proto::reports::ReportRequest {
                kind,
                reason,
                game_id,
            },
        };
        self.submit(s, Work::Report { user, request })
    }
    /// An arbitration call (docs/live/live-matchmaking.md §8.4), posted
    /// for every account signed in on this console: `path` under
    /// `/arbitration/`.
    pub fn arbitrate(&self, s: &Session, path: String, body: String) -> Result<Pending, u32> {
        self.submit(s, Work::Arbitration { path, body })
    }
    /// `user`'s mute list (docs/live/live-voice.md §3.6).
    pub fn mute_list(&self, s: &Session, user: u64) -> Result<Pending, u32> {
        self.submit(s, Work::MuteList { user })
    }
    /// Mutes (or unmutes) `target` for `user`.
    pub fn mute(&self, s: &Session, user: u64, target: u64, mute: bool) -> Result<Pending, u32> {
        self.submit(s, Work::Mute { user, target, mute })
    }
    /// The standings the game's stats read asks for: every player on every
    /// leaderboard (docs/live/live-matchmaking.md §8.3). `game_session`,
    /// the Live session this console is in (hex), lets made-up bots answer
    /// with the level their roster showed (§8.4).
    pub fn ranks(
        &self,
        s: &Session,
        leaderboards: Vec<u32>,
        xuids: Vec<u64>,
        game_session: Option<String>,
    ) -> Result<Pending, u32> {
        self.submit(
            s,
            Work::Ranks {
                leaderboards,
                xuids,
                game_session,
            },
        )
    }
    /// Everyone's presence (`GET /v1/presence`), read as the first account
    /// signed in on this console.
    pub fn read_presence(&self, s: &Session) -> Result<Pending, u32> {
        self.submit(s, Work::ReadPresence)
    }
    /// A page of the mod repository; `then` gets it on the worker.
    pub fn mods(
        &self,
        s: &Session,
        limit: u32,
        cursor: Option<String>,
        then: Then,
    ) -> Result<(), u32> {
        self.submit_then(s, Work::Mods { limit, cursor }, Some(then))
            .map(|_| ())
    }
    /// The served hopper file's clanker hoppers; `then` gets them on the
    /// worker.
    pub fn clanker_hoppers(&self, s: &Session, then: Then) -> Result<(), u32> {
        self.submit_then(s, Work::ClankerHoppers, Some(then))
            .map(|_| ())
    }
    /// The API base and map gate; `then` gets them on the worker.
    pub fn content_api(&self, s: &Session, then: Then) -> Result<(), u32> {
        self.submit_then(s, Work::ContentApi, Some(then))
            .map(|_| ())
    }
    pub fn cancel(&self, s: &Session) {
        s.cancel();
        let _ = self.submit(s, Work::Wake);
    }
    pub fn status(&self) -> ServerStatus {
        ServerStatus {
            acknowledged_pings: self.metrics.pings.load(Ordering::Acquire),
            last_error: self.metrics.last_error.load(Ordering::Acquire),
        }
    }
}
struct Connection {
    api: String,
    id: String,
}
#[derive(Clone)]
struct Identity {
    xuid: u64,
    gamertag: String,
    key: Arc<KeyPair>,
}
struct Active {
    published: Option<live_proto::session::Session>,
    session: Session,
    users: BTreeMap<u64, Identity>,
    presence: BTreeMap<u64, Presence>,
    matches: BTreeMap<String, u64>,
    /// Presence the game set that the server has not taken yet (it could
    /// not be reached): sent after the next good heartbeat.
    unsent: BTreeMap<u64, Presence>,
    /// When the heartbeat (or a presence change) began failing; the
    /// connection is lost once this is [`Worker::grace`] old.
    failing_since: Option<Instant>,
}
/// What this console's presence says it runs ([`ServerOptions::build`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Software {
    pub build: Option<String>,
    pub capabilities: Vec<String>,
}
impl Software {
    pub fn from_options(o: &ServerOptions) -> Self {
        Self {
            build: o.build.clone().filter(|b| live_proto::valid_build(b)),
            capabilities: Some(o.capabilities.clone())
                .filter(|c| live_proto::valid_capabilities(c))
                .unwrap_or_default(),
        }
    }
}
struct Worker {
    publisher: Option<HostPublisher>,
    base: String,
    maps: Option<Arc<crate::maps::MapGate>>,
    software: Software,
    agent: ureq::Agent,
    connection: Option<Connection>,
    active: Option<Active>,
    metrics: Arc<Metrics>,
    seen: Seen,
    retry: Retry,
    /// How long the heartbeat may fail before the connection is lost.
    grace: Duration,
    /// The last error answer's `Retry-After`, for [`Worker::retried`].
    retry_after: Cell<Option<Duration>>,
}
#[derive(Clone, Copy)]
enum Error {
    Status(u16),
    Transport,
    Protocol,
    TooLarge,
    Team(live_proto::team::TeamError),
    /// The server refused the account: it is banned
    /// (docs/live/live-admin.md "Bans").
    Banned,
    /// 409 "nonce already used": the server already has this signed event.
    Replayed,
}
impl Error {
    fn result(self) -> u32 {
        match self {
            Self::Team(error) => error.hresult(),
            Self::Banned => ACCOUNT_REQUIRES_MANAGEMENT,
            Self::Status(401 | 403 | 404) => INVALID_USER,
            Self::Status(429 | 503) => SERVERS_BUSY,
            Self::TooLarge => BUFFER_TOO_SMALL,
            _ => CANNOT_ACCESS_SERVICE,
        }
    }
    /// A failure that may pass: the server could not be reached, was
    /// restarting (a deploy), or asked to be called back later.
    fn retryable(self) -> bool {
        matches!(
            self,
            Self::Transport | Self::Status(429 | 500 | 502 | 503 | 504)
        )
    }
}
/// Results that mean the server refused the account itself rather than
/// could not be reached: they end the connection at once, with no grace.
fn refused(result: u32) -> bool {
    matches!(result, INVALID_USER | ACCOUNT_REQUIRES_MANAGEMENT)
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
/// What the persistent connection task reports when a request made while
/// signed in fails: a ban as the account-management result the game
/// shows its account dialog for, anything else as a lost connection.
fn lost(error: u32) -> u32 {
    if error == ACCOUNT_REQUIRES_MANAGEMENT {
        error
    } else {
        CONNECTION_LOST
    }
}
impl Worker {
    fn response(
        &self,
        req: ureq::Request,
        body: Option<&str>,
        limit: usize,
    ) -> Result<Vec<u8>, Error> {
        let response = match body {
            Some(b) => req.send_bytes(b.as_bytes()),
            None => req.call(),
        }
        .map_err(|e| match e {
            ureq::Error::Status(code, response) => {
                self.retry_after.set(
                    response
                        .header("retry-after")
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(Duration::from_secs),
                );
                let mut data = Vec::new();
                let _ = response.into_reader().take(4096).read_to_end(&mut data);
                let body = serde_json::from_slice::<serde_json::Value>(&data).ok();
                if body.as_ref().is_some_and(|v| v["error"] == "banned") {
                    return Error::Banned;
                }
                if code == 409
                    && body
                        .as_ref()
                        .is_some_and(|v| v["message"] == "nonce already used")
                {
                    return Error::Replayed;
                }
                let team = body
                    .and_then(|v| v.get("team_error").cloned())
                    .and_then(|v| serde_json::from_value(v).ok());
                team.map(Error::Team).unwrap_or(Error::Status(code))
            }
            _ => Error::Transport,
        })?;
        let mut data = Vec::new();
        response
            .into_reader()
            .take(limit as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| Error::Transport)?;
        if data.len() > limit {
            return Err(Error::TooLarge);
        }
        Ok(data)
    }
    /// [`Worker::response`] for a request that is safe to repeat, retried
    /// through a short outage under [`Retry`]. `build` makes each attempt's
    /// request afresh: a bearer token is spent by the first.
    fn retried(
        &self,
        build: impl Fn() -> ureq::Request,
        body: Option<&str>,
        limit: usize,
    ) -> Result<Vec<u8>, Error> {
        let attempts = self.retry.attempts.max(1);
        let mut wait = self.retry.backoff;
        // The server's own answer says more than a later failure to reach it.
        let mut answered = None;
        let mut attempt = 1;
        loop {
            self.retry_after.set(None);
            let error = match self.response(build(), body, limit) {
                Ok(bytes) => return Ok(bytes),
                // An earlier attempt landed and its answer was lost.
                Err(Error::Replayed) if attempt > 1 => return Ok(Vec::new()),
                Err(Error::Replayed) => return Err(Error::Status(409)),
                Err(e) if !e.retryable() => return Err(e),
                Err(e) => e,
            };
            if !matches!(error, Error::Transport) {
                answered = Some(error);
            }
            if attempt >= attempts {
                return Err(answered.unwrap_or(error));
            }
            let pause = self.retry_after.get().unwrap_or(wait);
            std::thread::sleep(pause.min(self.retry.max_wait));
            wait = wait.saturating_mul(2);
            attempt += 1;
        }
    }
    /// A GET's JSON, retried.
    fn json<T: DeserializeOwned>(&self, build: impl Fn() -> ureq::Request) -> Result<T, Error> {
        serde_json::from_slice(&self.retried(build, None, 64 * 1024)?).map_err(|_| Error::Protocol)
    }
    fn discover(&mut self) -> Result<(), Error> {
        let base = Url::parse(&self.base).map_err(|_| Error::Protocol)?;
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::Protocol);
        }
        if !bearer_allowed(base.as_str()) {
            return Err(Error::Protocol);
        }
        let url = format!(
            "{}/.well-known/h2live.json",
            self.base.trim_end_matches('/')
        );
        let discovery: Discovery = self.json(|| self.agent.get(&url))?;
        let directory = Url::parse(&format!("{}/", self.base.trim_end_matches('/')))
            .map_err(|_| Error::Protocol)?;
        let api = directory
            .join(&discovery.api)
            .map_err(|_| Error::Protocol)?;
        // Discovery may select an API path, never a new credential destination.
        if api.origin() != base.origin()
            || !api.username().is_empty()
            || api.password().is_some()
            || api.query().is_some()
            || api.fragment().is_some()
        {
            return Err(Error::Protocol);
        }
        if let Some(host) = &self.publisher {
            host.verify(&discovery)?;
        }
        self.connection = Some(Connection {
            api: api.as_str().trim_end_matches('/').into(),
            id: discovery.id,
        });
        Ok(())
    }
    fn author(&self, user: &Identity) -> String {
        Author::Account {
            xuid: Xuid(user.xuid),
            server: self.connection.as_ref().unwrap().id.clone(),
        }
        .to_string()
    }
    fn request(&self, method: &str, path: &str, user: &Identity, lxm: &str) -> ureq::Request {
        let c = self.connection.as_ref().unwrap();
        let token = issue_token(&user.key, &self.author(user), &c.id, lxm, now());
        self.agent
            .request(method, &format!("{}{path}", c.api))
            .set("authorization", &format!("Bearer {token}"))
    }
    fn identities(&self, accounts: &[Account]) -> Result<BTreeMap<u64, Identity>, Error> {
        let mut users = BTreeMap::new();
        for a in accounts {
            if users.contains_key(&a.xuid) {
                continue;
            }
            let user = Identity {
                xuid: a.xuid,
                gamertag: a.gamertag.clone(),
                key: a.key.clone().ok_or(Error::Status(401))?,
            };
            let path = format!("/accounts/by-xuid/{}/keys", Xuid(a.xuid));
            let keys: AccountKeys = self.json(|| self.request("GET", &path, &user, lxm::READ))?;
            if !keys.keys.iter().any(|k| {
                k.valid_to.is_none()
                    && k.kid == user.key.kid()
                    && PublicKey::from_jwk(&k.jwk).is_ok_and(|p| p == *user.key.public())
            }) {
                return Err(Error::Status(401));
            }
            users.insert(a.xuid, user);
        }
        Ok(users)
    }
    fn event(&self, user: &Identity, kind: &str, presence: &Presence) -> Result<(), Error> {
        let body = EventBody::new(
            &user.key,
            &self.author(user),
            &streams::presence(Xuid(user.xuid)),
            kind,
            presence_data(
                Xuid(user.xuid),
                presence,
                (kind != types::SIGNED_OUT).then_some(&self.software),
            ),
            time::OffsetDateTime::now_utc(),
        )
        .sign(&user.key);
        self.retried(
            || {
                self.request("POST", "/events", user, lxm::EVENTS_WRITE)
                    .set("content-type", "application/jose")
            },
            Some(&body),
            64 * 1024,
        )?;
        Ok(())
    }
    fn fetch(&self, user: &Identity, name: &'static str, id: u32) -> Result<File, Error> {
        let path = format!("/titles/4D530064/files/{name}");
        let bytes = self.retried(
            || self.request("GET", &path, user, lxm::READ),
            None,
            MAX_FILE,
        )?;
        Ok(File {
            id,
            bytes: bytes.into(),
        })
    }
    /// Fresh single-use `read` tokens for the first account signed in on
    /// this console, for a GET that counts a mod download
    /// ([`crate::maps::Fetch::get_counted`]); `None` when signed out.
    fn reader(&self) -> Option<crate::maps::Reader> {
        let server = self.connection.as_ref()?.id.clone();
        let user = self.active.as_ref()?.users.values().next()?.clone();
        let author = self.author(&user);
        Some(Arc::new(move || {
            Some(issue_token(&user.key, &author, &server, lxm::READ, now()))
        }))
    }
    fn team_json<T: DeserializeOwned>(&self, user: &Identity, path: &str) -> Result<T, Error> {
        serde_json::from_slice(&self.retried(
            || self.request("GET", path, user, lxm::READ),
            None,
            MAX_FILE,
        )?)
        .map_err(|_| Error::Protocol)
    }
    fn teams_request(
        &self,
        user: &Identity,
        request: TeamRequest,
    ) -> Result<Vec<live_proto::team::Team>, Error> {
        use live_proto::team::{Action, Identity as TeamIdentity};
        let teams = match request {
            TeamRequest::User(uid) => {
                let page: live_proto::Page<live_proto::team::Team> =
                    self.team_json(user, &format!("/accounts/by-xuid/{uid}/teams"))?;
                if page.items.len() > 8
                    || page.next.is_some()
                    || page
                        .items
                        .iter()
                        .any(|t| !t.members.contains_key(&uid) && !t.recruits.contains_key(&uid))
                    || page
                        .items
                        .iter()
                        .map(|t| t.id)
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != page.items.len()
                {
                    return Err(Error::Protocol);
                }
                page.items
            }
            TeamRequest::Team(uid) => self.fetch_team(user, uid)?,
            TeamRequest::Change(mut action) => {
                if let Action::Recruit { target, .. } = &mut action {
                    if target.xuid.0 == 0 {
                        let part = path_part(&target.gamertag);
                        let account: live_proto::AccountDetail = self
                            .team_json(user, &format!("/accounts/{part}"))
                            .map_err(|e| {
                                if matches!(e, Error::Status(404)) {
                                    Error::Team(live_proto::team::TeamError::UserNotFound)
                                } else {
                                    e
                                }
                            })?;
                        *target = TeamIdentity {
                            xuid: account.summary.xuid,
                            gamertag: account.summary.gamertag.to_string(),
                        };
                    } else {
                        *target = self
                            .team_json(user, &format!("/accounts/by-xuid/{}", target.xuid))
                            .map_err(|e| {
                                if matches!(e, Error::Status(404)) {
                                    Error::Team(live_proto::team::TeamError::UserNotFound)
                                } else {
                                    e
                                }
                            })?;
                    }
                }
                let body = EventBody::new(
                    &user.key,
                    &self.author(user),
                    &format!("team-{}", action.id()),
                    action.kind(),
                    serde_json::to_value(&action).map_err(|_| Error::Protocol)?,
                    time::OffsetDateTime::now_utc(),
                )
                .sign(&user.key);
                self.retried(
                    || {
                        self.request("POST", "/events", user, lxm::EVENTS_WRITE)
                            .set("content-type", "application/jose")
                    },
                    Some(&body),
                    MAX_FILE,
                )?;
                if matches!(action, Action::Delete { .. }) {
                    vec![]
                } else {
                    self.fetch_team(user, action.id())?
                }
            }
        };
        if teams
            .iter()
            .any(|t| live_proto::team::validate_team(t).is_err())
        {
            return Err(Error::Protocol);
        }
        Ok(teams)
    }
    fn fetch_team(&self, user: &Identity, id: Xuid) -> Result<Vec<live_proto::team::Team>, Error> {
        #[derive(serde::Deserialize)]
        struct Detail {
            team: live_proto::team::Team,
        }
        let reply: Detail = self.team_json(user, &format!("/teams/{id}"))?;
        if reply.team.id != id {
            return Err(Error::Protocol);
        }
        Ok(vec![reply.team])
    }
    fn matchmaking(
        &self,
        user: &Identity,
        ticket: &live_proto::matchmaking::Ticket,
        waiting: Option<&live_proto::matchmaking::Assignment>,
    ) -> Result<Reply, Error> {
        use crate::matchmaking::{Outcome, QueueAction, Step, queue_action};

        let body = serde_json::to_string(ticket).map_err(|_| Error::Protocol)?;
        let bytes = match self.response(
            self.request("PUT", "/matchmaking/queue", user, lxm::SESSIONS_WRITE)
                .set("content-type", "application/json"),
            Some(&body),
            64 * 1024,
        ) {
            Err(Error::Status(404)) => return Ok(Reply::MatchmakingUnsupported),
            Err(error) if error.retryable() => {
                // Allocation may outlast the HTTP timeout. Let the search
                // resubmit the same ticket with a fresh sessions.write token.
                return Ok(Reply::Matchmaking(Box::new(Step::RefreshQueue)));
            }
            other => other?,
        };
        let me = Xuid(user.xuid);
        let value = serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)?;
        let step = match queue_action(value, ticket.hopper, me).ok_or(Error::Protocol)? {
            QueueAction::Probe(candidates, region_echoes) => {
                let connection = self.connection.as_ref().ok_or(Error::Protocol)?;
                // Echo targets carry a validated region code. Only the discovery
                // origin may receive a bearer, including on a failed upgrade.
                let mut probes = Vec::new();
                for probe in region_echoes {
                    let mut url = Url::parse(&format!("{}/hosts/regions/{}/echo", connection.api, probe.region))
                        .map_err(|_| Error::Protocol)?;
                    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
                    url.set_scheme(scheme).map_err(|_| Error::Protocol)?;
                    let token = issue_token(&user.key, &self.author(user), &connection.id, lxm::SESSIONS_RELAY, now());
                    probes.push((probe, url, token));
                }
                let samples = std::thread::scope(|scope| {
                    let mut pending = Vec::new();
                    for (probe, url, token) in probes {
                        let task = std::thread::Builder::new()
                            .name("live-region-qos".into())
                            .spawn_scoped(scope, move || crate::tunnel_client::region_echo(url.as_str(), &token));
                        pending.push((probe, task));
                    }
                    let mut samples = crate::qos::measure(candidates, Duration::from_millis(450))
                        .map_err(|_| Error::Transport)?;
                    for (probe, task) in pending {
                        samples.push(live_proto::matchmaking::Sample {
                            target: probe.xuid,
                            generation: probe.generation,
                            rtt_ms: task.ok().and_then(|t| t.join().ok()).flatten(),
                        });
                    }
                    Ok::<_, Error>(samples)
                })?;
                Step::Retry(samples)
            }
            QueueAction::Wait => Step::Retry(Vec::new()),
            QueueAction::WaitForHost(assignment) => Step::WaitingForHost(assignment),
            QueueAction::Host(assignment) => Step::Complete(Outcome::Host(assignment)),
            QueueAction::Join {
                session_id,
                frames_from_host,
            } => self.matchmaking_join(user, ticket.hopper, &session_id, frames_from_host, waiting)?,
        };
        Ok(Reply::Matchmaking(Box::new(step)))
    }
    fn matchmaking_join(
        &self,
        user: &Identity,
        hopper: u32,
        session_id: &str,
        frames_from_host: bool,
        waiting: Option<&live_proto::matchmaking::Assignment>,
    ) -> Result<crate::matchmaking::Step, Error> {
        use crate::matchmaking::{DedicatedJoin, HostAction, Outcome, Step, host_action, join_assignment};

        let mut rows = match self.fetch_match(user, session_id) {
            Err(Error::Status(403 | 404 | 410)) if frames_from_host || waiting.is_some() => {
                return Ok(Step::RequeueDedicated);
            }
            Err(e) if e.retryable() && (frames_from_host || waiting.is_some()) => {
                return Ok(Step::RequeueDedicated);
            }
            Err(Error::Status(404)) => return Ok(Step::Retry(Vec::new())),
            result => result?,
        };
        let Some(row) = rows.pop() else {
            return Ok(if frames_from_host || waiting.is_some() {
                Step::RequeueDedicated
            } else {
                Step::Retry(Vec::new())
            });
        };
        row.advertisement.validate().map_err(|_| Error::Protocol)?;
        let Some(assignment) = join_assignment(&row, waiting, hopper, Xuid(user.xuid), frames_from_host)
        else {
            return if frames_from_host {
                Ok(Step::RequeueDedicated)
            } else {
                Err(Error::Protocol)
            };
        };
        if row.xnkid != session_id {
            return Err(Error::Protocol);
        }

        if let Some(lease) = &assignment.host_lease {
            if !crate::matchmaking::valid_host_lease(lease) {
                return Err(Error::Protocol);
            }
            let host = match self.team_json(user, &format!("/hosts/{lease}")) {
                Err(Error::Status(403 | 404 | 410)) => return Ok(Step::RequeueDedicated),
                Err(e) if e.retryable() => return Ok(Step::WaitingForHost(assignment.clone())),
                result => result?,
            };
            let api = &self.connection.as_ref().ok_or(Error::Protocol)?.api;
            return Ok(match host_action(host, assignment, session_id, api) {
                HostAction::Wait => Step::WaitingForHost(assignment.clone()),
                HostAction::Requeue => Step::RequeueDedicated,
                HostAction::Join { route, region } => Step::CheckDedicated(Box::new(DedicatedJoin {
                    row,
                    hopper,
                    route,
                    region,
                    frames_from_host,
                })),
            });
        }

        if frames_from_host {
            // A lease-bearing queue reply can race a peer fallback in Find.
            return Ok(Step::RequeueDedicated);
        }
        if assignment.id != session_id
            || !crate::matchmaking::valid_address(row.endpoint.addr, row.endpoint.port)
        {
            return Err(Error::Protocol);
        }
        let kid = crate::sessions::wire_id(session_id).ok_or(Error::Protocol)?;
        let address = std::net::SocketAddrV4::new(row.endpoint.addr, row.endpoint.port);
        if self.map_ready(&row, true)
            && crate::qos::query_why(address, kid, 1).is_ok_and(|r| !r.data.is_empty())
        {
            Ok(Step::Complete(Outcome::Join {
                rows: vec![row],
                frames_from_host,
                dedicated: None,
                tunnel: None,
            }))
        } else {
            Ok(Step::Retry(Vec::new()))
        }
    }
    fn join_dedicated(
        &self,
        user: &Identity,
        join: &crate::matchmaking::DedicatedJoin,
    ) -> Result<crate::matchmaking::Step, Error> {
        use crate::matchmaking::{DedicatedMatch, Outcome, Step};

        if !self.map_ready(&join.row, true) {
            return Ok(Step::RequeueDedicated);
        }
        let tunnel = if let Some(url) = &join.route.tunnel_url {
            let server = &self.connection.as_ref().ok_or(Error::Protocol)?.id;
            let bearer = issue_token(&user.key, &self.author(user), server, lxm::SESSIONS_RELAY, now());
            match crate::connect_tunnel(url.as_str(), &bearer) {
                Ok(handle) => Some((join.route.address, handle)),
                Err(_) => return Ok(Step::RequeueDedicated),
            }
        } else {
            None
        };
        let kid = crate::sessions::wire_id(&join.row.session_id).ok_or(Error::Protocol)?;
        let qos_base = tunnel
            .as_ref()
            .map_or(join.route.address, |(_, handle)| handle.qos_base());
        if !crate::qos::query_why(qos_base, kid, 1).is_ok_and(|r| !r.data.is_empty()) {
            return Ok(Step::RequeueDedicated);
        }

        let mut row = join.row.clone();
        join.route.apply(&mut row);
        self.saw(std::slice::from_ref(&row));
        Ok(Step::Complete(Outcome::Join {
            rows: vec![row],
            frames_from_host: join.frames_from_host,
            dedicated: Some(DedicatedMatch {
                hopper: join.hopper,
                route: join.route.clone(),
            }),
            tunnel,
        }))
    }
    fn matches_request(
        &self,
        user: &Identity,
        request: MatchRequest,
    ) -> Result<Vec<live_proto::session::Session>, Error> {
        use live_proto::session;
        let items = match request {
            MatchRequest::Create {
                session: new,
                assignment,
            } => {
                vec![self.create_match(Writer::Account(user), &new, assignment.as_deref())?]
            }
            MatchRequest::Queue { .. } | MatchRequest::JoinDedicated(_) => {
                return Err(Error::Protocol);
            }
            MatchRequest::Update { id, update } => {
                self.match_event(
                    user,
                    &id,
                    session::UPDATED,
                    serde_json::to_value(update).map_err(|_| Error::Protocol)?,
                )?;
                self.fetch_match(user, &id)?
            }
            MatchRequest::Delete(id) => {
                self.match_event(user, &id, session::DELETED, json!({"reason":"closed"}))?;
                Vec::new()
            }
            MatchRequest::Players { id, joined, left } => {
                self.match_players(Writer::Account(user), &id, &joined, &left)?;
                Vec::new()
            }
            MatchRequest::Find(id) => match self.fetch_match(user, &id) {
                Err(Error::Status(404)) => Vec::new(),
                // Finding a session by id is how a join starts: its map is
                // made ready first, or the session is not found.
                Ok(items) => items
                    .into_iter()
                    .filter(|s| self.map_ready(s, true))
                    .collect(),
                Err(e) => return Err(e),
            },
            MatchRequest::Search {
                limit,
                attributes,
                optimatch,
            } => {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query.append_pair(
                    "attributes",
                    &serde_json::to_string(&attributes).map_err(|_| Error::Protocol)?,
                );
                if let Some(q) = optimatch {
                    query.append_pair(
                        "optimatch",
                        &serde_json::to_string(&q).map_err(|_| Error::Protocol)?,
                    );
                }
                let filter = query.finish();
                let page: live_proto::Page<session::Session> = self.team_json(
                    user,
                    &format!("/sessions?state=open&limit={}&{filter}", limit.max(1)),
                )?;
                if page.items.len() > limit as usize && limit != 0 {
                    return Err(Error::Protocol);
                }
                if limit == 0 {
                    Vec::new()
                } else {
                    page.items
                        .into_iter()
                        .filter(|s| self.map_ready(s, false))
                        .collect()
                }
            }
        };
        for item in &items {
            item.advertisement.validate().map_err(|_| Error::Protocol)?;
            if !session::valid_id(&item.session_id)
                || item.xnkid != item.session_id
                || !crate::matchmaking::valid_address(item.endpoint.addr, item.endpoint.port)
            {
                return Err(Error::Protocol);
            }
        }
        self.saw(&items);
        Ok(items)
    }
    /// Keeps recent sessions, for a guest's game start (the host's
    /// skulls) and its co-op join screen (the campaign, the host).
    fn saw(&self, sessions: &[live_proto::session::Session]) {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        for s in sessions {
            seen.remember(s);
        }
    }
    /// Whether a session's map can be had (docs/game/maps.md §5): checked for a
    /// search result, made ready (downloaded) for a join. A refusal is
    /// printed with its reason and the session is dropped.
    fn map_ready(&self, s: &live_proto::session::Session, join: bool) -> bool {
        let (Some(gate), Some(c)) = (&self.maps, &self.connection) else {
            return true;
        };
        if s.advertisement.map_id.is_none() {
            return true;
        }
        // A join may download a layer of tens of MB: not under the worker's
        // whole-request timeout ([`crate::maps::download_agent`]).
        let downloads = join.then(crate::maps::download_agent);
        let fetch = crate::maps::HttpFetch {
            agent: downloads.as_ref().unwrap_or(&self.agent),
            api: &c.api,
            // A join that installs the map counts as its download.
            reader: join.then(|| self.reader()).flatten(),
        };
        let result = if join {
            gate.prepare(&fetch, &s.advertisement).map(|dir| {
                if let Some(dir) = dir {
                    eprintln!(
                        "live: {}'s map is ready in {}",
                        s.host_gamertag,
                        dir.display()
                    );
                }
            })
        } else {
            gate.check(&fetch, &s.advertisement).map(|_| ())
        };
        match result {
            Ok(()) => true,
            Err(why) => {
                eprintln!("live: not joining {}'s game: {why}", s.host_gamertag);
                false
            }
        }
    }
    fn fetch_match(
        &self,
        user: &Identity,
        id: &str,
    ) -> Result<Vec<live_proto::session::Session>, Error> {
        #[derive(serde::Deserialize)]
        struct Detail {
            session: live_proto::session::Session,
        }
        let detail: Detail = self.team_json(user, &format!("/sessions/{id}"))?;
        if detail.session.session_id != id {
            return Err(Error::Protocol);
        }
        self.saw(std::slice::from_ref(&detail.session));
        Ok(if detail.session.state == "open" {
            vec![detail.session]
        } else {
            Vec::new()
        })
    }
    fn match_players(
        &self,
        writer: Writer<'_>,
        id: &str,
        joined: &[Xuid],
        left: &[Xuid],
    ) -> Result<(), Error> {
        use live_proto::session;
        let events = left
            .iter()
            .map(|x| (session::LEFT, x))
            .chain(joined.iter().map(|x| (session::JOINED, x)));
        let mut last = Ok(());
        for (kind, xuid) in events {
            if let Err(e) = self.write_match_event(writer, id, kind, json!({ "xuid": xuid })) {
                let why = match e {
                    Error::Status(n) => format!("status {n}"),
                    _ => "no answer".to_owned(),
                };
                eprintln!("live: session {id}: {kind} for {xuid} refused ({why})");
                last = Err(e);
            }
        }
        last
    }

    fn match_event(
        &self,
        user: &Identity,
        id: &str,
        kind: &str,
        data: serde_json::Value,
    ) -> Result<(), Error> {
        self.write_match_event(Writer::Account(user), id, kind, data)
    }

    fn write_match_event(
        &self,
        writer: Writer<'_>,
        id: &str,
        kind: &str,
        data: serde_json::Value,
    ) -> Result<(), Error> {
        let (author, key) = writer.signer(self);
        let body = EventBody::new(
            key,
            &author,
            &live_proto::session::stream(id),
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        );
        let jws = live_proto::sign::sign_batch(key, &[body]);
        let path = format!("/sessions/{id}/events");
        self.retried(
            || {
                writer
                    .post(self, &path)
                    .set("content-type", "application/jose")
            },
            Some(&jws),
            MAX_FILE,
        )?;
        Ok(())
    }

    fn social_event(
        &self,
        user: &Identity,
        recipient: Xuid,
        kind: &str,
        data: serde_json::Value,
    ) -> Result<(), Error> {
        let body = EventBody::new(
            &user.key,
            &self.author(user),
            &live_proto::mailbox::stream(recipient),
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        )
        .sign(&user.key);
        self.retried(
            || {
                self.request("POST", "/events", user, lxm::EVENTS_WRITE)
                    .set("content-type", "application/jose")
            },
            Some(&body),
            MAX_FILE,
        )?;
        Ok(())
    }
    fn mailbox(&self, user: &Identity) -> Result<Vec<live_proto::mailbox::Invite>, Error> {
        let page: live_proto::Page<live_proto::mailbox::Invite> = self.team_json(
            user,
            &format!("/accounts/by-xuid/{}/mailbox", Xuid(user.xuid)),
        )?;
        if page.items.len() > 100
            || page.items.iter().any(|v| {
                v.to != Xuid(user.xuid)
                    || !live_proto::session::valid_id(&v.session_id)
                    || v.gamertag.len() > 15
                    || !v.gamertag.is_ascii()
            })
        {
            return Err(Error::Protocol);
        }
        Ok(page.items)
    }
    fn friends_for(&self, user: &Identity) -> Result<live_proto::Friends, Error> {
        self.team_json(
            user,
            &format!(
                "/accounts/{}/friends",
                url::form_urlencoded::byte_serialize(user.gamertag.as_bytes()).collect::<String>()
            ),
        )
    }
    fn social_request(&self, user: &Identity, request: &SocialRequest) -> Result<Reply, Error> {
        use live_proto::mailbox::{self, Answer};
        match request {
            SocialRequest::Refresh => {
                let friends = self.friends_for(user)?;
                let invites = self.mailbox(user)?;
                let sessions = self.matches_request(
                    user,
                    MatchRequest::Search {
                        limit: 1000,
                        attributes: BTreeMap::new(),
                        optimatch: None,
                    },
                )?;
                Ok(Reply::Social(SocialSnapshot {
                    friends,
                    invites,
                    sessions,
                }))
            }
            SocialRequest::Send { to, session_id } => {
                for recipient in to {
                    self.social_event(
                        user,
                        *recipient,
                        mailbox::SENT,
                        json!({"from":Xuid(user.xuid),"session_id":session_id}),
                    )?;
                }
                Ok(Reply::Done)
            }
            SocialRequest::Answer(answer) => {
                let invite = self
                    .mailbox(user)?
                    .into_iter()
                    .find(|v| v.id == answer.invite_id && v.session_id == answer.session_id)
                    .ok_or(Error::Status(404))?;
                self.social_event(
                    user,
                    Xuid(user.xuid),
                    mailbox::ANSWERED,
                    serde_json::to_value(answer).map_err(|_| Error::Protocol)?,
                )?;
                Ok(if answer.answer == Answer::Accept {
                    Reply::InviteAccepted(invite)
                } else {
                    Reply::Done
                })
            }
            SocialRequest::Join { from, session_id } => {
                let friends = self.friends_for(user)?;
                if !friends.friends.iter().any(|f| f.xuid == *from) {
                    return Err(Error::Status(403));
                }
                let row = self
                    .fetch_match(user, session_id)?
                    .into_iter()
                    .find(|v| v.host_xuid == *from)
                    .ok_or(Error::Status(404))?;
                Ok(Reply::InviteAccepted(crate::join_resolver::friend_join(
                    row,
                    Xuid(user.xuid),
                )))
            }
            SocialRequest::JoinLink(id) => {
                let Some(row) = self.fetch_match(user, id)?.into_iter().next() else {
                    return Ok(Reply::JoinLink(Err(
                        "This game is no longer available.".into()
                    )));
                };
                let friend =
                    if row.advertisement.visibility == live_proto::session::Visibility::Friends {
                        self.friends_for(user)?
                            .friends
                            .iter()
                            .any(|f| f.xuid == row.host_xuid)
                    } else {
                        false
                    };
                let viewer = Xuid(user.xuid);
                if !crate::matchmaking::valid_address(row.endpoint.addr, row.endpoint.port) {
                    return Ok(Reply::JoinLink(Err(
                        "This game has an invalid host address.".into(),
                    )));
                }
                Ok(Reply::JoinLink(
                    row.check_join_link(Some(viewer), |_, _| friend)
                        .map(|()| crate::join_resolver::friend_join(row, viewer)),
                ))
            }
        }
    }
    fn run(&mut self, c: &Command) -> Result<Reply, u32> {
        if matches!(c.work, Work::Wake) {
            self.cancelled();
            return Ok(Reply::Done);
        }
        if c.session.cancelled() {
            return Err(NO_SESSION);
        }
        if self.publisher.is_some() {
            match &c.work {
                Work::Match(request) => return self.publish_host_match(&c.session, request),
                Work::Logon(accounts) if accounts.is_empty() => {}
                _ => return Err(NO_USER),
            }
        }
        match &c.work {
            Work::Logon(accounts) => {
                self.sign_out();
                self.discover().map_err(Error::result)?;
                let users = self.identities(accounts).map_err(Error::result)?;
                let mut signed = Vec::new();
                for user in users.values() {
                    if c.session.cancelled() {
                        break;
                    }
                    if let Err(e) = self.event(user, types::SIGNED_IN, &Presence::default()) {
                        for user in signed {
                            let _ = self.event(user, types::SIGNED_OUT, &Presence::default());
                        }
                        return Err(e.result());
                    }
                    signed.push(user);
                }
                if c.session.cancelled() {
                    for user in signed {
                        let _ = self.event(user, types::SIGNED_OUT, &Presence::default());
                    }
                    return Err(NO_SESSION);
                }
                self.active = Some(Active {
                    published: None,
                    session: c.session.clone(),
                    presence: users.keys().map(|u| (*u, Presence::default())).collect(),
                    users,
                    matches: BTreeMap::new(),
                    unsent: BTreeMap::new(),
                    failing_since: None,
                });
                c.session.status.store(CONNECTED, Ordering::Release);
                Ok(Reply::Logon)
            }
            Work::Change(accounts) => {
                if self
                    .active
                    .as_ref()
                    .is_none_or(|a| a.session.id != c.session.id)
                {
                    return Err(NO_SESSION);
                }
                let users = self.identities(accounts).map_err(Error::result)?;
                let old = self.active.as_ref().unwrap();
                let added: Vec<_> = users
                    .values()
                    .filter(|u| !old.users.contains_key(&u.xuid))
                    .cloned()
                    .collect();
                let removed: Vec<_> = old
                    .users
                    .values()
                    .filter(|u| !users.contains_key(&u.xuid))
                    .cloned()
                    .collect();
                let mut signed = Vec::new();
                let changed = (|| {
                    for user in &added {
                        if c.session.cancelled() {
                            return Err(NO_SESSION);
                        }
                        self.event(user, types::SIGNED_IN, &Presence::default())
                            .map_err(Error::result)?;
                        signed.push(user);
                    }
                    for user in &removed {
                        let ids: Vec<_> = self
                            .active
                            .as_ref()
                            .unwrap()
                            .matches
                            .iter()
                            .filter(|(_, uid)| **uid == user.xuid)
                            .map(|(id, _)| id.clone())
                            .collect();
                        for id in ids {
                            self.match_event(
                                user,
                                &id,
                                live_proto::session::DELETED,
                                json!({"reason":"host_left"}),
                            )
                            .map_err(Error::result)?;
                            self.active.as_mut().unwrap().matches.remove(&id);
                        }
                        if c.session.cancelled() {
                            return Err(NO_SESSION);
                        }
                        self.event(user, types::SIGNED_OUT, &Presence::default())
                            .map_err(Error::result)?;
                    }
                    Ok(())
                })();
                if let Err(error) = changed {
                    // Do not leave newly signed-in accounts orphaned or claim
                    // that a partially applied controller change succeeded.
                    for user in signed {
                        let _ = self.event(user, types::SIGNED_OUT, &Presence::default());
                    }
                    self.sign_out();
                    c.session.status.store(CONNECTION_LOST, Ordering::Release);
                    return Err(error);
                }
                let active = self.active.as_mut().unwrap();
                active.presence.retain(|u, _| users.contains_key(u));
                for user in users.keys() {
                    active.presence.entry(*user).or_default();
                }
                active.users = users;
                Ok(Reply::Done)
            }
            Work::Presence { user, value } => {
                let Some(active) = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                else {
                    return Err(NO_SESSION);
                };
                let identity = active.users.get(user).ok_or(INVALID_USER)?.clone();
                if active.presence.get(user) != Some(value) {
                    self.event(&identity, types::PRESENCE_CHANGED, value)
                        .map_err(Error::result)?;
                    self.active
                        .as_mut()
                        .unwrap()
                        .presence
                        .insert(*user, value.clone());
                }
                Ok(Reply::Done)
            }
            Work::File(name, id) => {
                let Some(active) = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                else {
                    return Err(NO_SESSION);
                };
                let user = active.users.values().next().ok_or(NO_USER)?;
                self.fetch(user, name, *id)
                    .map(Reply::File)
                    .map_err(|e| match e {
                        Error::Status(404) => STORAGE_FILE_NOT_FOUND,
                        _ => e.result(),
                    })
            }
            Work::Match(request) => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.values().next().ok_or(NO_USER)?.clone();
                if let MatchRequest::Queue { ticket, waiting } = request {
                    return self.matchmaking(&user, ticket, waiting.as_ref()).map_err(Error::result);
                }
                if let MatchRequest::JoinDedicated(join) = request {
                    return self
                        .join_dedicated(&user, join)
                        .map(|step| Reply::Matchmaking(Box::new(step)))
                        .map_err(Error::result);
                }
                let rows = self
                    .matches_request(&user, request.clone())
                    .map_err(|e| match e {
                        Error::Status(404 | 409) => MATCH_INVALID_SESSION,
                        Error::Status(400 | 413 | 422) => INVALID_ARGUMENT,
                        _ => e.result(),
                    })?;
                let active = self.active.as_mut().unwrap();
                match request {
                    MatchRequest::Create { .. } => {
                        if let Some(row) = rows.first() {
                            active.matches.insert(row.session_id.clone(), user.xuid);
                        }
                    }
                    MatchRequest::Delete(id) => {
                        active.matches.remove(id);
                    }
                    _ => {}
                }
                Ok(Reply::Matches(rows))
            }
            Work::Social { user, request } => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.get(user).ok_or(NO_USER)?;
                self.social_request(user, request).map_err(|e| match e {
                    Error::Status(404) => FRIEND_NOT_FOUND,
                    Error::Status(400 | 403 | 409) => INVALID_ARGUMENT,
                    _ => e.result(),
                })
            }
            Work::Report { user, request } => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.get(user).ok_or(NO_USER)?;
                let body = serde_json::to_string(&request.body).map_err(|_| INVALID_ARGUMENT)?;
                self.response(
                    self.request(
                        "POST",
                        &format!("/accounts/by-xuid/{}/report", request.target),
                        user,
                        lxm::REPORTS_WRITE,
                    )
                    .set("content-type", "application/json"),
                    Some(&body),
                    64 * 1024,
                )
                .map(|_| Reply::Done)
                .map_err(|e| match e {
                    Error::Status(404) => INVALID_USER,
                    Error::Status(400 | 422) => INVALID_ARGUMENT,
                    _ => e.result(),
                })
            }
            Work::MuteList { user } => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.get(user).ok_or(NO_USER)?;
                let path = format!("/accounts/by-xuid/{}/mutes", Xuid(user.xuid));
                let list: live_proto::mutes::MuteList = self
                    .json(|| self.request("GET", &path, user, lxm::READ))
                    .map_err(Error::result)?;
                Ok(Reply::Mutes(list.items.iter().map(|e| e.xuid.0).collect()))
            }
            Work::Arbitration { path, body } => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                if active.users.is_empty() {
                    return Err(NO_USER);
                }
                for user in active.users.values() {
                    self.response(
                        self.request(
                            "POST",
                            &format!("/arbitration/{path}"),
                            user,
                            lxm::GAMES_WRITE,
                        ),
                        Some(body),
                        64 * 1024,
                    )
                    .map_err(Error::result)?;
                }
                Ok(Reply::Done)
            }
            Work::Mute { user, target, mute } => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.get(user).ok_or(NO_USER)?;
                let path = format!(
                    "/accounts/by-xuid/{}/mutes/{}",
                    Xuid(user.xuid),
                    Xuid(*target)
                );
                let method = if *mute { "PUT" } else { "DELETE" };
                self.retried(
                    || self.request(method, &path, user, lxm::MUTES_WRITE),
                    Some(""),
                    64 * 1024,
                )
                .map(|_| Reply::Done)
                .map_err(Error::result)
            }
            Work::Teams { user, request } => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|s| s.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.get(user).ok_or(NO_USER)?;
                self.teams_request(user, request.clone())
                    .map(Reply::Teams)
                    .map_err(Error::result)
            }
            Work::Ranks {
                leaderboards,
                xuids,
                game_session,
            } => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.values().next().ok_or(NO_USER)?;
                let boards: Vec<String> = leaderboards.iter().map(u32::to_string).collect();
                let xuids: Vec<String> = xuids.iter().map(|x| Xuid(*x).to_string()).collect();
                let mut path = format!(
                    "/rank-stats?leaderboards={}&xuids={}",
                    boards.join(","),
                    xuids.join(","),
                );
                if let Some(id) = game_session {
                    path.push_str(&format!("&session={id}"));
                }
                #[derive(serde::Deserialize)]
                struct Rows {
                    items: Vec<live_proto::rank::LeaderboardRow>,
                }
                self.team_json::<Rows>(user, &path)
                    .map(|r| Reply::Ranks(r.items))
                    .map_err(Error::result)
            }
            Work::Bots(request) => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                // The account that made the session hosts it.
                let user = active
                    .matches
                    .get(&request.session_id)
                    .and_then(|x| active.users.get(x))
                    .or_else(|| active.users.values().next())
                    .ok_or(NO_USER)?;
                let body = serde_json::to_string(request).map_err(|_| INVALID_ARGUMENT)?;
                // Sent once: a roster request is not safe to repeat.
                self.response(
                    self.request("POST", "/games/bots", user, lxm::GAMES_WRITE)
                        .set("content-type", "application/json"),
                    Some(&body),
                    64 * 1024,
                )
                .and_then(|b| {
                    serde_json::from_slice::<live_proto::bots::Roster>(&b)
                        .map_err(|_| Error::Protocol)
                })
                .map(Reply::Roster)
                .map_err(|e| match e {
                    Error::Status(404 | 409) => MATCH_INVALID_SESSION,
                    Error::Status(400 | 413 | 422) => INVALID_ARGUMENT,
                    _ => e.result(),
                })
            }
            Work::BotCard(xuid) => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.values().next().ok_or(NO_USER)?;
                self.team_json(user, &format!("/games/bots/{xuid}"))
                    .map(Reply::BotCard)
                    .map_err(Error::result)
            }
            Work::ReadPresence => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.values().next().ok_or(NO_USER)?;
                self.team_json::<live_proto::Page<live_proto::Presence>>(user, "/presence")
                    .map(|p| Reply::Presence(p.items))
                    .map_err(Error::result)
            }
            Work::Mods { limit, cursor } => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.values().next().ok_or(NO_USER)?;
                let page = match self.team_json::<crate::mods::PackPage>(user, "/mod-packs") {
                    Ok(packs) => Ok(packs.page(*limit, cursor.as_deref())),
                    Err(Error::Status(404)) => {
                        let path = crate::mods::list_path(*limit, cursor.as_deref());
                        self.team_json::<crate::mods::ModPage>(user, &path)
                    }
                    Err(error) => Err(error),
                };
                page.map(Reply::Mods)
                    .map_err(Error::result)
            }
            Work::ClankerHoppers => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let user = active.users.values().next().ok_or(NO_USER)?;
                let path = live_proto::hopper::clankers::PATH;
                self.team_json::<live_proto::hopper::clankers::ClankerHoppers>(user, path)
                    .map(Reply::ClankerHoppers)
                    .map_err(Error::result)
            }
            Work::ContentApi => {
                self.active
                    .as_ref()
                    .filter(|a| a.session.id == c.session.id)
                    .ok_or(NO_SESSION)?;
                let api = self.connection.as_ref().ok_or(NO_SESSION)?.api.clone();
                Ok(Reply::Content {
                    api,
                    gate: self.maps.clone(),
                    reader: self.reader(),
                })
            }
            Work::Wake => Ok(Reply::Done),
        }
    }
    fn cancelled(&mut self) {
        if self.active.as_ref().is_some_and(|a| a.session.cancelled()) {
            self.sign_out();
        }
    }
    fn sign_out(&mut self) {
        if let Some(active) = self.active.take() {
            if let (Some(host), Some(row)) = (&self.publisher, active.published) {
                let _ = self.write_match_event(
                    Writer::Host(host),
                    &row.session_id,
                    live_proto::session::DELETED,
                    json!({"reason":"host_left"}),
                );
            }
            for (id, uid) in &active.matches {
                if let Some(user) = active.users.get(uid) {
                    let _ = self.match_event(
                        user,
                        id,
                        live_proto::session::DELETED,
                        json!({"reason":"host_left"}),
                    );
                }
            }
            for user in active.users.values() {
                let _ = self.event(user, types::SIGNED_OUT, &Presence::default());
            }
        }
    }
    /// The heartbeat. A failed one is not yet a lost connection: a Worker
    /// deploy or a blip passes, and the connection is lost only once the
    /// heartbeat has failed for [`Worker::grace`] (docs/live/live.md
    /// "Outages"). A refused account ends it at once.
    fn ping(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        let users: Vec<Identity> = active.users.values().cloned().collect();
        let mut failure = None;
        for user in &users {
            match self.keep_alive(user) {
                Ok(()) => {
                    self.metrics.pings.fetch_add(1, Ordering::AcqRel);
                }
                Err(e) => {
                    let result = e.result();
                    self.metrics.last_error.store(result, Ordering::Release);
                    if failure.is_none_or(|f| !refused(f)) {
                        failure = Some(result);
                    }
                }
            }
        }
        self.health(failure);
    }
    /// One account's heartbeat, and the presence the game set while the
    /// server could not be reached.
    fn keep_alive(&mut self, user: &Identity) -> Result<(), Error> {
        let ping = self.retried(
            || self.request("POST", "/presence/ping", user, lxm::EVENTS_WRITE),
            Some(""),
            64 * 1024,
        );
        match ping {
            // The server has no presence for the player: it expired while
            // the server could not be reached. The player never left: it
            // signs in again, as it is.
            Err(Error::Status(404)) => self.announce(user)?,
            other => {
                other?;
            }
        }
        let unsent = self
            .active
            .as_mut()
            .and_then(|a| a.unsent.remove(&user.xuid));
        if let Some(presence) = unsent {
            if let Err(e) = self.event(user, types::PRESENCE_CHANGED, &presence) {
                if let Some(a) = self.active.as_mut() {
                    a.unsent.entry(user.xuid).or_insert(presence);
                }
                return Err(e);
            }
            if let Some(a) = self.active.as_mut() {
                a.presence.insert(user.xuid, presence);
            }
        }
        Ok(())
    }
    /// Signs `user` in again with the presence the server last took.
    fn announce(&self, user: &Identity) -> Result<(), Error> {
        let presence = self
            .active
            .as_ref()
            .and_then(|a| a.presence.get(&user.xuid))
            .cloned()
            .unwrap_or_default();
        self.event(user, types::SIGNED_IN, &Presence::default())?;
        if presence != Presence::default() {
            self.event(user, types::PRESENCE_CHANGED, &presence)?;
        }
        Ok(())
    }
    /// A presence change the server did not take: kept for the next good
    /// heartbeat, and counted as a failed heartbeat.
    fn presence_failed(&mut self, session: &Session, user: u64, value: &Presence, error: u32) {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|a| a.session.id == session.id)
        else {
            return;
        };
        if refused(error) {
            return self.health(Some(error));
        }
        // Anything else is the game's own mistake, not an outage.
        if !matches!(error, CANNOT_ACCESS_SERVICE | SERVERS_BUSY) {
            return;
        }
        if active.users.contains_key(&user) {
            active.unsent.insert(user, value.clone());
        }
        self.health(Some(error));
    }
    /// Notes a heartbeat's outcome (`None`: it was answered) and loses the
    /// connection when the account was refused or the grace has run out.
    fn health(&mut self, failure: Option<u32>) {
        let grace = self.grace;
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let Some(result) = failure else {
            active.failing_since = None;
            return;
        };
        let since = *active.failing_since.get_or_insert_with(Instant::now);
        if refused(result) || since.elapsed() >= grace {
            active.session.status.store(lost(result), Ordering::Release);
        }
    }
}

fn path_part(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char)
        } else {
            use std::fmt::Write;
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

/// A presence event's data. `activity`, `build` and `capabilities` are
/// left out when there are none, so a server that predates them reads the
/// same bytes. `software` is None for a sign-out.
pub(crate) fn presence_data(
    xuid: Xuid,
    presence: &Presence,
    software: Option<&Software>,
) -> serde_json::Value {
    let mut data = json!({"xuid": xuid, "state": presence.state, "game_id": presence.game_id});
    if let Some(a) = &presence.activity {
        data["activity"] = json!(a);
    }
    if let Some(s) = software {
        if let Some(b) = &s.build {
            data["build"] = json!(b);
        }
        if !s.capabilities.is_empty() {
            data["capabilities"] = json!(s.capabilities);
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_session_cache_bounds_churn_and_keeps_refreshed_rows() {
        let mut seen = SessionCache::default();
        let mut row: live_proto::session::Session = serde_json::from_value(json!({
            "session_id": "", "host": "acct:0009000000000001@live.test",
            "host_xuid": "0009000000000001", "host_gamertag": "Host", "xnkid": "",
            "endpoint": {"addr": "127.0.0.1", "port": 1000, "source": "observed"},
            "advertisement": {
                "port": 1000, "host_address": [], "attributes": {},
                "slots": live_proto::session::Slots::default()
            },
            "since": "", "last_activity": 0, "state": "open"
        }))
        .unwrap();
        let id = |n: usize| format!("{n:016x}");
        for n in 0..MAX_SEEN_SESSIONS {
            row.session_id = id(n);
            seen.remember(&row);
        }

        row.session_id = id(0);
        row.advertisement.name = Some("Refreshed".into());
        seen.remember(&row);
        row.session_id = id(MAX_SEEN_SESSIONS);
        seen.remember(&row);

        assert_eq!(seen.by_id.len(), MAX_SEEN_SESSIONS);
        assert_eq!(seen.by_age.len(), MAX_SEEN_SESSIONS);
        assert!(seen.get(&id(1)).is_none());
        assert_eq!(
            seen.get(&id(0)).unwrap().advertisement.name.as_deref(),
            Some("Refreshed")
        );
        assert_eq!(seen.get(&id(MAX_SEEN_SESSIONS)), Some(&row));
    }
}
