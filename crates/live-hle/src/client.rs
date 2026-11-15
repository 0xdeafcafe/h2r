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
}
impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(5),
            heartbeat_interval: Duration::from_secs(30),
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
    pub name: &'static str,
    pub bytes: Arc<[u8]>,
    pub sent_time: u64,
    version: u64,
}
impl File {
    pub fn version(&self) -> u64 {
        self.version
    }
}
#[derive(Clone)]
pub(crate) enum Reply {
    Logon { files: Vec<File>, files_error: u32 },
    File(File),
    Done,
    Teams(Vec<live_proto::team::Team>),
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
}
impl Default for Presence {
    fn default() -> Self {
        Self {
            state: "online",
            game_id: None,
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) enum TeamRequest {
    User(Xuid),
    Team(Xuid),
    Change(live_proto::team::Action),
}
enum Work {
    Logon(Vec<Account>),
    Change(Vec<Account>),
    Presence { user: u64, value: Presence },
    File(&'static str, u32),
    Teams { user: u64, request: TeamRequest },
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
}
impl Service {
    pub fn start(base: String, options: ServerOptions) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Command>(64);
        let metrics = Arc::new(Metrics::default());
        let counters = metrics.clone();
        std::thread::Builder::new()
            .name("h2-live-http".into())
            .spawn(move || {
                let agent = ureq::AgentBuilder::new()
                    .timeout(options.request_timeout)
                    .redirects(0)
                    .build();
                let mut worker = Worker {
                    base,
                    agent,
                    connection: None,
                    active: None,
                    metrics: counters,
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
        Ok(Self { sender, metrics })
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
    key: Arc<KeyPair>,
}
struct Active {
    session: Session,
    users: BTreeMap<u64, Identity>,
    presence: BTreeMap<u64, Presence>,
}
struct Worker {
    base: String,
    agent: ureq::Agent,
    connection: Option<Connection>,
    active: Option<Active>,
    metrics: Arc<Metrics>,
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
            json!({"xuid":Xuid(user.xuid),"state":presence.state,"game_id":presence.game_id}),
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
        // Compute metadata here, not in the game's enumeration/property loop.
        let version = bytes
            .iter()
            .fold(0xcbf29ce484222325u64, |v, b| {
                (v ^ *b as u64).wrapping_mul(0x100000001b3)
            })
            .max(1);
        let sent_time = (now() + 11644473600) as u64 * 10000000;
        Ok(File {
            id,
            name,
            bytes: bytes.into(),
            sent_time,
            version,
        })
    }
    fn files(&self, user: &Identity) -> (Vec<File>, u32) {
        let mut files = Vec::new();
        let mut error = 0;
        for (id, name) in FILES {
            match self.fetch(user, name, id) {
                Ok(file) => files.push(file),
                Err(Error::Status(404)) => {}
                Err(e) => error = e.result(),
            }
        }
        (files, error)
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
                let (files, files_error) = users
                    .values()
                    .next()
                    .map(|u| self.files(u))
                    .unwrap_or_default();
                self.active = Some(Active {
                    session: c.session.clone(),
                    presence: users.keys().map(|u| (*u, Presence::default())).collect(),
                    users,
                });
                c.session.status.store(CONNECTED, Ordering::Release);
                Ok(Reply::Logon { files, files_error })
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
                        Error::Status(404) => MESSAGE_INVALID_ID,
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
