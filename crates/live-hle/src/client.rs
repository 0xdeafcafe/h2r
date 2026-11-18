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

pub(crate) const MAX_FILE: usize = 2 * 1024 * 1024;
pub(crate) const FILES: [(u32, &str); 2] =
    [(1, "message_of_the_day.dat"), (4, "multiplayer_hopper.dat")];
#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub request_timeout: Duration,
    pub heartbeat_interval: Duration,
    /// Custom maps (docs/game/maps.md §5): sessions on a map that cannot be had
    /// are left out of searches, and finding one by id (a join) downloads
    /// its missing layers first. None: sessions are taken as they come.
    pub maps: Option<Arc<crate::maps::MapGate>>,
}
impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(5),
            heartbeat_interval: Duration::from_secs(30),
            maps: None,
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
    Social(SocialSnapshot),
    InviteAccepted(live_proto::mailbox::Invite),
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
    Create(live_proto::session::NewSession),
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
}
#[derive(Clone)]
pub(crate) enum SocialRequest {
    Refresh,
    Send { to: Vec<Xuid>, session_id: String },
    Answer(live_proto::mailbox::Answered),
    Join { from: Xuid, session_id: String },
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
    Presence { user: u64, value: Presence },
    File(&'static str, u32),
    Teams { user: u64, request: TeamRequest },
    Match(MatchRequest),
    Social { user: u64, request: SocialRequest },
    Wake,
}
struct Command {
    session: Session,
    work: Work,
    result: Pending,
}
pub(crate) struct Service {
    sender: SyncSender<Command>,
    metrics: Arc<Metrics>,
    /// Every session the worker has read, by id: what its host advertised
    /// (the skulls, docs/game/skulls.md "In multiplayer"; a co-op
    /// campaign, docs/live/live-coop.md) and who is in it.
    pub seen: Seen,
}
/// Sessions by session id, as last read from the server.
pub(crate) type Seen = Arc<Mutex<BTreeMap<String, live_proto::session::Session>>>;
impl Service {
    pub fn start(base: String, options: ServerOptions) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Command>(64);
        let metrics = Arc::new(Metrics::default());
        let counters = metrics.clone();
        let seen = Seen::default();
        let seen_by_worker = seen.clone();
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
                    agent,
                    connection: None,
                    active: None,
                    metrics: counters,
                    seen: seen_by_worker,
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
                        Ok(command) => {
                            let result = worker.run(&command);
                            if let Err(error) = result
                                && !command.session.cancelled()
                            {
                                worker.metrics.last_error.store(error, Ordering::Release);
                                if matches!(command.work, Work::Presence { .. })
                                    && matches!(
                                        error,
                                        CANNOT_ACCESS_SERVICE | INVALID_USER | SERVERS_BUSY
                                    )
                                {
                                    command
                                        .session
                                        .status
                                        .store(CONNECTION_LOST, Ordering::Release);
                                }
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
            sender,
            metrics,
            seen,
        })
    }
    fn submit(&self, session: &Session, work: Work) -> Result<Pending, u32> {
        let result = Pending::new();
        self.sender
            .try_send(Command {
                session: session.clone(),
                work,
                result: result.clone(),
            })
            .map_err(|_| TASK_BUSY)?;
        Ok(result)
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
    session: Session,
    users: BTreeMap<u64, Identity>,
    presence: BTreeMap<u64, Presence>,
    matches: BTreeMap<String, u64>,
}
struct Worker {
    base: String,
    maps: Option<Arc<crate::maps::MapGate>>,
    agent: ureq::Agent,
    connection: Option<Connection>,
    active: Option<Active>,
    metrics: Arc<Metrics>,
    seen: Seen,
}
#[derive(Clone, Copy)]
enum Error {
    Status(u16),
    Transport,
    Protocol,
    TooLarge,
    Team(live_proto::team::TeamError),
}
impl Error {
    fn result(self) -> u32 {
        match self {
            Self::Team(error) => error.hresult(),
            Self::Status(401 | 403 | 404) => INVALID_USER,
            Self::Status(429 | 503) => SERVERS_BUSY,
            Self::TooLarge => BUFFER_TOO_SMALL,
            _ => CANNOT_ACCESS_SERVICE,
        }
    }
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
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
                let mut data = Vec::new();
                let _ = response.into_reader().take(4096).read_to_end(&mut data);
                let team = serde_json::from_slice::<serde_json::Value>(&data)
                    .ok()
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
    fn json<T: DeserializeOwned>(
        &self,
        req: ureq::Request,
        body: Option<&str>,
    ) -> Result<T, Error> {
        serde_json::from_slice(&self.response(req, body, 64 * 1024)?).map_err(|_| Error::Protocol)
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
        let discovery: Discovery = self.json(self.agent.get(&url), None)?;
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
            let keys: AccountKeys = self.json(
                self.request(
                    "GET",
                    &format!("/accounts/by-xuid/{}/keys", Xuid(a.xuid)),
                    &user,
                    lxm::READ,
                ),
                None,
            )?;
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
            presence_data(Xuid(user.xuid), presence),
            time::OffsetDateTime::now_utc(),
        )
        .sign(&user.key);
        self.response(
            self.request("POST", "/events", user, lxm::EVENTS_WRITE)
                .set("content-type", "application/jose"),
            Some(&body),
            64 * 1024,
        )?;
        Ok(())
    }
    fn fetch(&self, user: &Identity, name: &'static str, id: u32) -> Result<File, Error> {
        let bytes = self.response(
            self.request(
                "GET",
                &format!("/titles/4D530064/files/{name}"),
                user,
                lxm::READ,
            ),
            None,
            MAX_FILE,
        )?;
        Ok(File {
            id,
            bytes: bytes.into(),
        })
    }
    fn team_json<T: DeserializeOwned>(&self, user: &Identity, path: &str) -> Result<T, Error> {
        serde_json::from_slice(&self.response(
            self.request("GET", path, user, lxm::READ),
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
                self.response(
                    self.request("POST", "/events", user, lxm::EVENTS_WRITE)
                        .set("content-type", "application/jose"),
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
    fn matches_request(
        &self,
        user: &Identity,
        request: MatchRequest,
    ) -> Result<Vec<live_proto::session::Session>, Error> {
        use live_proto::session;
        let items = match request {
            MatchRequest::Create(new) => {
                #[derive(serde::Deserialize)]
                struct Created {
                    session: session::Session,
                }
                let bytes = self.response(
                    self.request("POST", "/sessions", user, lxm::SESSIONS_WRITE)
                        .set("content-type", "application/json"),
                    Some(&serde_json::to_string(&new).map_err(|_| Error::Protocol)?),
                    MAX_FILE,
                )?;
                vec![
                    serde_json::from_slice::<Created>(&bytes)
                        .map_err(|_| Error::Protocol)?
                        .session,
                ]
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
                || item.endpoint.port == 0
                || item.endpoint.port > 65529
                || item.endpoint.addr.is_unspecified()
                || item.endpoint.addr.is_multicast()
                || item.endpoint.addr.is_broadcast()
            {
                return Err(Error::Protocol);
            }
        }
        self.saw(&items);
        Ok(items)
    }
    /// Keeps each session read, for a guest's game start (the host's
    /// skulls) and its co-op join screen (the campaign, the host).
    fn saw(&self, sessions: &[live_proto::session::Session]) {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        for s in sessions {
            seen.insert(s.session_id.clone(), s.clone());
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
        let fetch = crate::maps::HttpFetch {
            agent: &self.agent,
            api: &c.api,
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
    fn match_event(
        &self,
        user: &Identity,
        id: &str,
        kind: &str,
        data: serde_json::Value,
    ) -> Result<(), Error> {
        let body = EventBody::new(
            &user.key,
            &self.author(user),
            &live_proto::session::stream(id),
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        );
        let jws = live_proto::sign::sign_batch(&user.key, &[body]);
        self.response(
            self.request(
                "POST",
                &format!("/sessions/{id}/events"),
                user,
                lxm::SESSIONS_WRITE,
            )
            .set("content-type", "application/jose"),
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
        self.response(
            self.request("POST", "/events", user, lxm::EVENTS_WRITE)
                .set("content-type", "application/jose"),
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
    fn social_request(&self, user: &Identity, request: &SocialRequest) -> Result<Reply, Error> {
        use live_proto::mailbox::{self, Answer, Invite};
        match request {
            SocialRequest::Refresh => {
                let friends: live_proto::Friends = self.team_json(
                    user,
                    &format!(
                        "/accounts/{}/friends",
                        url::form_urlencoded::byte_serialize(user.gamertag.as_bytes())
                            .collect::<String>()
                    ),
                )?;
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
                let friends: live_proto::Friends = self.team_json(
                    user,
                    &format!(
                        "/accounts/{}/friends",
                        url::form_urlencoded::byte_serialize(user.gamertag.as_bytes())
                            .collect::<String>()
                    ),
                )?;
                if !friends.friends.iter().any(|f| f.xuid == *from) {
                    return Err(Error::Status(403));
                }
                let row = self
                    .fetch_match(user, session_id)?
                    .into_iter()
                    .find(|v| v.host_xuid == *from)
                    .ok_or(Error::Status(404))?;
                Ok(Reply::InviteAccepted(Invite {
                    id: 0,
                    from: *from,
                    to: Xuid(user.xuid),
                    gamertag: row.host_gamertag,
                    session_id: session_id.clone(),
                    sent_at: time::OffsetDateTime::now_utc().unix_timestamp(),
                    answer: None,
                }))
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
                    session: c.session.clone(),
                    presence: users.keys().map(|u| (*u, Presence::default())).collect(),
                    users,
                    matches: BTreeMap::new(),
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
                let rows = self
                    .matches_request(&user, request.clone())
                    .map_err(|e| match e {
                        Error::Status(404 | 409) => MATCH_INVALID_SESSION,
                        Error::Status(400 | 413 | 422) => INVALID_ARGUMENT,
                        _ => e.result(),
                    })?;
                let active = self.active.as_mut().unwrap();
                match request {
                    MatchRequest::Create(_) => {
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
    fn ping(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        for user in active.users.values() {
            match self.response(
                self.request("POST", "/presence/ping", user, lxm::EVENTS_WRITE),
                Some(""),
                64 * 1024,
            ) {
                Ok(_) => {
                    self.metrics.pings.fetch_add(1, Ordering::AcqRel);
                }
                Err(e) => {
                    self.metrics.last_error.store(e.result(), Ordering::Release);
                    active
                        .session
                        .status
                        .store(CONNECTION_LOST, Ordering::Release);
                }
            }
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

/// A presence event's data. `activity` is left out when there is none, so a
/// server that predates it reads the same bytes.
pub(crate) fn presence_data(xuid: Xuid, presence: &Presence) -> serde_json::Value {
    let mut data = json!({"xuid": xuid, "state": presence.state, "game_id": presence.game_id});
    if let Some(a) = &presence.activity {
        data["activity"] = json!(a);
    }
    data
}
