//! The Live sink (docs/live/live-telemetry.md, "Server ingest" and
//! "Reconnect"): games are created with `POST /v1/games`, frames and signed
//! event batches go over the `/v1/ingest` WebSocket.
//!
//! The socket can drop at any time (a Worker deploy closes every socket to
//! its Durable Objects). A drop is never fatal: the sink reconnects with
//! backoff, the new connection starts a new packed stream with a keyframe,
//! and frames made while down are dropped. Game events are not: they wait in
//! a bounded outbox, in order, and each batch leaves it only once the server
//! has answered for it. A batch is kept signed, so a resend after an
//! unanswered send carries the same nonces and the server refuses the copy
//! ("nonce already used"), which the sink counts as delivered.
//!
//! Only the account being refused stops the sink: a ban, or the server
//! closing the socket at auth [`MAX_AUTH_REFUSALS`] times in a row.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use live_proto::game::{self, Frame, GameCreated, MachinePosition, NewGame};
use live_proto::pack;
use live_proto::sign::{EventBody, KeyPair, MAX_EVENT_SKEW_S, issue_token, sign_batch};
use live_proto::{ClientMessage, Discovery, ErrorBody, ErrorCode, IngestReply, lxm};
use time::OffsetDateTime;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::config::{HostToken, TelemetryAccount};
use crate::event::GameEvent;
use crate::sink::Sink;

const MAX_INGEST_MESSAGE: usize = 16 * 1024;
const MAX_BATCH_EVENTS: usize = 4;
/// A packed keyframe every this many frames (ten seconds at 15 a second);
/// the frames between are deltas (docs/live/live-telemetry.md, Packed frames).
pub(crate) const FRAME_KEYFRAME_EVERY: u64 = 150;
/// Game starts and events waiting for the server, at most. Past it the
/// oldest event of a [`DROPPABLE`] kind goes; starts, ends, joins and
/// leaves are never dropped.
pub(crate) const MAX_OUTBOX: usize = 4096;
/// Event kinds the outbox may drop when full: a later `ScoreChanged` and
/// the `GameEnded` scoreboard carry the scores again.
const DROPPABLE: [&str; 3] = [game::ACTOR_KILLED, game::PLAYER_KILLED, game::SCORE_CHANGED];
/// A batch signed longer ago than this is signed again before it is sent
/// (the server refuses an `at` more than five minutes off). New nonces: if
/// the old copy did land unanswered, the server keeps both.
const RESIGN_AFTER_S: i64 = MAX_EVENT_SKEW_S - 60;
/// The server's close code for a refused or expired token.
const CLOSE_UNAUTHORIZED: u16 = 4401;
/// Auth refusals in a row (no batch accepted between) before the sink
/// gives up on the account.
pub(crate) const MAX_AUTH_REFUSALS: u32 = 3;
/// The server's reply to a batch it already has.
const NONCE_USED: &str = "nonce already used";
/// The server's reply to a write to a game that has ended.
const GAME_OVER: &str = "the game has ended";

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

const SIM_FRAME_BATCH_DELAY: Duration = Duration::from_millis(100);
const SIM_KEYFRAME_EVERY: u64 = 30;
const MAX_DEFERRED_BYTES: usize = 32 * 1024 * 1024;
const SIM_SETTLE_TIMEOUT: Duration = Duration::from_secs(60);

struct SimCapture {
    combat: Option<crate::hooks::CombatFeed>,
    next_combat: Option<Instant>,
    game_id: String,
    deferred: Option<Deferred>,
    pending: Vec<u8>,
    since: Instant,
}

struct Deferred {
    lock: PathBuf,
    encoder: pack::Encoder,
    items: Vec<DeferredItem>,
    size: usize,
}

enum DeferredItem {
    Event(GameEvent),
    Frame(Message),
}

impl Deferred {
    fn push(&mut self, item: DeferredItem, size: usize) -> Result<(), String> {
        self.size += size + std::mem::size_of::<DeferredItem>();
        if self.size > MAX_DEFERRED_BYTES {
            return Err("queued sim recording exceeds 32 MiB".into());
        }
        self.items.push(item);
        Ok(())
    }
}

/// The sink's clocks; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// The first reconnect waits this long, each failure after twice the
    /// last, up to `backoff_max`; each wait is jittered by up to a fifth.
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// How long a sent batch waits for its answer before the connection
    /// counts as lost (and the batch is sent again on the next).
    pub ack_timeout: Duration,
    /// After a refusal, how long to wait for an `appended` that would show
    /// the refusal was an earlier frame's (refusals carry no id).
    pub settle: Duration,
    /// A connection up this long before it drops resets the backoff.
    pub steady: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            backoff_min: Duration::from_millis(250),
            backoff_max: Duration::from_secs(30),
            ack_timeout: Duration::from_secs(10),
            settle: Duration::from_millis(500),
            steady: Duration::from_secs(30),
        }
    }
}

struct Connection {
    socket: Socket,
    last_auth: Instant,
    opened: Instant,
    /// The connection's packed frame stream: a new connection starts a new
    /// stream with a keyframe.
    frames: pack::Encoder,
}

/// What waits in the outbox, in the order it happened.
enum Item {
    /// A game to create; its events wait behind it.
    Start {
        local: String,
        game: Box<NewGame>,
    },
    Event(GameEvent),
    /// Events signed into one batch, sent or about to be. Kept as sent, so
    /// a resend repeats the nonces.
    Batch(Batch),
}

struct Batch {
    server_id: String,
    events: Vec<GameEvent>,
    text: String,
    signed_at: i64,
}

/// Why a delivery stopped.
#[derive(Debug)]
enum Failure {
    /// The connection or the server is down or busy: back off, retry.
    Transport(String),
    /// The server refused the token (close 4401 or HTTP 401).
    Auth(String),
    /// The account may not write at all (banned).
    Fatal(String),
}

/// The server's answer to a batch.
enum Answer {
    Appended,
    Refused(ErrorCode, String),
}

enum Uploader {
    Account(live_proto::Xuid),
    Host {
        token: HostToken,
        subject: String,
        audience: String,
    },
}

impl Uploader {
    fn author(&self, server: &str) -> String {
        match self {
            Self::Account(xuid) => format!("acct:{xuid}@{server}"),
            Self::Host { subject, .. } => subject.clone(),
        }
    }
}

pub struct ServerSink {
    base_url: String,
    uploader: Uploader,
    key: KeyPair,
    connection: Option<Connection>,
    /// The last discovery; its id names the server in tokens and authors.
    discovery: Option<Discovery>,
    /// Local game ids to the server's.
    games: HashMap<String, String>,
    /// Travel survives frame drops and is sent again on the next keyframe.
    machine_game: String,
    machine_positions: BTreeMap<u16, MachinePosition>,
    machine_cut: Option<u64>,
    /// Local games the server would not create: their events are dropped.
    refused: HashSet<String>,
    outbox: VecDeque<Item>,
    timing: Timing,
    /// The next reconnect's wait.
    delay: Duration,
    /// No connection attempt before this.
    retry_at: Option<Instant>,
    /// Logged that the server is unreachable, not yet that it is back.
    down: bool,
    auth_refusals: u32,
    fatal: Option<String>,
    dropped_events: u64,
    dropped_frames: u64,
    sim: Option<SimCapture>,
}

impl ServerSink {
    pub fn new(base_url: String, account: TelemetryAccount) -> Result<Self, String> {
        let key = KeyPair::from_private_jwk(&account.key)
            .map_err(|e| format!("telemetry account key: {e}"))?;
        let xuid = live_proto::Xuid::parse(&account.xuid)
            .map_err(|e| format!("telemetry account XUID: {e}"))?;
        Self::with_uploader(base_url, Uploader::Account(xuid), key)
    }

    pub(crate) fn for_host(base_url: String, token: HostToken) -> Result<Self, String> {
        use live_proto::sign::{Jws, TYP_TOKEN};

        #[derive(serde::Deserialize)]
        struct Claims {
            sub: String,
            aud: String,
            event_key: String,
        }

        let bad = || "invalid host telemetry credential".to_owned();
        let jws = Jws::parse(&token.0).map_err(|_| bad())?;
        let payload = jws.json_payload(TYP_TOKEN).map_err(|_| bad())?;
        let claims: Claims = serde_json::from_value(payload).map_err(|_| bad())?;
        let id = claims.sub.strip_prefix("host:").ok_or_else(bad)?;
        if !id.starts_with("hl-") || !game::valid_id(id) {
            return Err(bad());
        }
        let key = KeyPair::from_secret_b64(&claims.event_key).map_err(|_| bad())?;
        let uploader = Uploader::Host {
            token,
            subject: claims.sub,
            audience: claims.aud,
        };
        Self::with_uploader(base_url, uploader, key)
    }

    fn with_uploader(base_url: String, uploader: Uploader, key: KeyPair) -> Result<Self, String> {
        if !live_proto::bearer_allowed(&base_url) {
            return Err("server credentials require HTTPS or a loopback URL".into());
        }
        let timing = Timing::default();
        Ok(Self {
            base_url,
            uploader,
            key,
            connection: None,
            discovery: None,
            games: HashMap::new(),
            machine_game: String::new(),
            machine_positions: BTreeMap::new(),
            machine_cut: None,
            refused: HashSet::new(),
            outbox: VecDeque::new(),
            timing,
            delay: timing.backoff_min,
            retry_at: None,
            down: false,
            auth_refusals: 0,
            fatal: None,
            dropped_events: 0,
            dropped_frames: 0,
            sim: None,
        })
    }

    pub(crate) fn with_sim(
        mut self, sim: &game::SimGame, deferred_lock: Option<PathBuf>, combat: crate::hooks::CombatFeed,
    ) -> Self {
        self.sim = Some(SimCapture {
            combat: (!sim.deferred && deferred_lock.is_none()).then_some(combat),
            next_combat: None,
            game_id: game::sim_game_id(&sim.run_id),
            deferred: deferred_lock.map(|lock| Deferred {
                lock,
                encoder: pack::Encoder::new(0, SIM_KEYFRAME_EVERY),
                items: Vec::new(),
                size: 0,
            }),
            pending: Vec::new(),
            since: Instant::now(),
        });
        self
    }

    #[cfg(test)]
    pub(crate) fn with_timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self.delay = timing.backoff_min;
        self
    }

    /// Starts and events not yet answered for.
    #[cfg(test)]
    pub fn waiting_events(&self) -> usize {
        self.outbox.len()
    }

    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(5))
            .timeout_write(Duration::from_secs(5))
            .build()
    }

    fn discover(&self) -> Result<Discovery, Failure> {
        let url = format!(
            "{}/.well-known/h2live.json",
            self.base_url.trim_end_matches('/')
        );
        let discovery = Self::agent()
            .get(&url)
            .call()
            .map_err(|e| Failure::Transport(format!("telemetry discovery: {e}")))?
            .into_json::<Discovery>()
            .map_err(|e| Failure::Transport(format!("telemetry discovery JSON: {e}")))?;
        self.check_host_destination(&discovery)
            .map_err(Failure::Fatal)?;
        Ok(discovery)
    }

    /// A host bearer can authorize a whole match. Check the issuer and keep
    /// it on the configured origin before sending it, including on reconnect.
    fn check_host_destination(&self, discovery: &Discovery) -> Result<(), String> {
        use live_proto::sign::{Jws, PublicKey};
        use tungstenite::http::Uri;

        let Uploader::Host {
            token, audience, ..
        } = &self.uploader else {
            return Ok(());
        };
        let bad = || "telemetry discovery does not match the host credential".to_owned();
        let base = self.base_url.parse::<Uri>().map_err(|_| bad())?;
        let api = resolve_api(&self.base_url, &discovery.api);
        let target = api.parse::<Uri>().map_err(|_| bad())?;
        let jws = Jws::parse(&token.0).map_err(|_| bad())?;
        let issuer_matches = discovery.keys.iter()
            .filter_map(|jwk| PublicKey::from_jwk(jwk).ok())
            .any(|key| jws.verify(&key).is_ok());
        if &discovery.id != audience
            || base.scheme() != target.scheme()
            || base.authority() != target.authority()
            || api.contains('\\')
            || !issuer_matches
        {
            return Err(bad());
        }
        Ok(())
    }

    fn discovery(&self) -> &Discovery {
        self.discovery.as_ref().expect("connected")
    }

    fn token(&self) -> String {
        if let Uploader::Host { token, .. } = &self.uploader {
            return token.0.clone();
        }
        let server_id = &self.discovery().id;
        let author = self.uploader.author(server_id);
        issue_token(
            &self.key,
            &author,
            server_id,
            lxm::GAMES_WRITE,
            OffsetDateTime::now_utc().unix_timestamp(),
        )
    }

    /// Whether a reconnect must wait.
    fn backing_off(&self) -> bool {
        self.retry_at.is_some_and(|at| Instant::now() < at)
    }

    /// Opens the ingest socket (discovery first, each time: a redeployed
    /// server may name a new API) and sends the auth message with a fresh
    /// token. A new connection is a new packed stream: its first frame is
    /// a keyframe, which is what the server's new decoder needs.
    fn connect(&mut self) -> Result<(), Failure> {
        if self.connection.is_some() {
            return Ok(());
        }
        let discovery = self.discover()?;
        let api = resolve_api(&self.base_url, &discovery.api);
        let ingest = format!("{}/ingest", api.trim_end_matches('/'));
        let socket_url = ingest
            .strip_prefix("http://")
            .map(|rest| format!("ws://{rest}"))
            .or_else(|| {
                ingest
                    .strip_prefix("https://")
                    .map(|rest| format!("wss://{rest}"))
            })
            .ok_or_else(|| Failure::Fatal("telemetry API URL must be HTTP(S)".to_owned()))?;
        if !live_proto::bearer_allowed(&socket_url) {
            return Err(Failure::Fatal(
                "telemetry ingest credentials require WSS or loopback WS".into(),
            ));
        }
        let (mut socket, _) = connect_bounded(&socket_url).map_err(Failure::Transport)?;
        set_timeouts(socket.get_mut()).map_err(Failure::Transport)?;
        let frames = ingest_frames(&discovery, self.keyframe_every());
        self.discovery = Some(discovery);
        let now = Instant::now();
        self.connection = Some(Connection {
            socket,
            last_auth: now,
            opened: now,
            frames,
        });
        self.authenticate()
    }

    fn authenticate(&mut self) -> Result<(), Failure> {
        let token = self.token();
        self.connection.as_mut().unwrap().last_auth = Instant::now();
        let text = serde_json::to_string(&ClientMessage::Auth { token }).expect("serializes");
        self.write(Message::Text(text.into()))
    }

    /// A fresh token before the last one runs out.
    fn refresh_auth(&mut self) -> Result<(), Failure> {
        // The fixed lease bearer lasts through the match and cannot be renewed.
        if matches!(self.uploader, Uploader::Account(_))
            && self.connection.as_ref()
                .is_some_and(|c| c.last_auth.elapsed() >= Duration::from_secs(45))
        {
            self.authenticate()?;
        }
        Ok(())
    }

    fn write(&mut self, message: Message) -> Result<(), Failure> {
        let connection = self.connection.as_mut().expect("connected");
        connection
            .socket
            .send(message)
            .map_err(|e| Failure::Transport(format!("telemetry ingest send: {e}")))
    }

    /// One reply, if one is waiting (the socket's read timeout is short).
    fn read_reply(&mut self) -> Result<Option<IngestReply>, Failure> {
        let connection = self.connection.as_mut().expect("connected");
        loop {
            match connection.socket.read() {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(&text)
                        .map(Some)
                        .map_err(|e| Failure::Transport(format!("telemetry ingest reply: {e}")));
                }
                Ok(Message::Ping(bytes)) => connection
                    .socket
                    .send(Message::Pong(bytes))
                    .map_err(|e| Failure::Transport(format!("telemetry ingest pong: {e}")))?,
                Ok(Message::Close(frame)) => {
                    let (code, reason) = frame
                        .map(|f| (u16::from(f.code), f.reason.as_str().to_owned()))
                        .unwrap_or((u16::from(CloseCode::Status), String::new()));
                    return Err(closed(code, reason));
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(Failure::Transport(format!("telemetry ingest receive: {e}"))),
            }
        }
    }

    /// Replies already waiting, none of them a batch's: frame refusals.
    fn drain(&mut self) -> Result<(), Failure> {
        for _ in 0..8 {
            match self.read_reply()? {
                Some(IngestReply::Error { message, .. }) => self.frame_refused(&message),
                Some(IngestReply::Appended { .. }) => {}
                None => break,
            }
        }
        Ok(())
    }

    /// A refused frame that did not decode (the server lost its decoder,
    /// as after a hibernation wake): the next frame is a keyframe.
    fn frame_refused(&mut self, message: &str) {
        if message.starts_with("frame:")
            && let Some(connection) = &mut self.connection
        {
            connection.frames.force_keyframe();
        }
    }

    /// Sends a signed batch and waits for its answer. Replies come in the
    /// order the server took the messages; frames are answered only when
    /// refused, a batch always. So an `appended` is this batch's, and a
    /// refusal is this batch's once [`Timing::settle`] passes with nothing
    /// after it.
    fn send_batch(&mut self, text: &str) -> Result<Answer, Failure> {
        if text.len() > MAX_INGEST_MESSAGE {
            return Ok(Answer::Refused(
                ErrorCode::TooLarge,
                format!(
                    "telemetry ingest message is {} bytes (limit {MAX_INGEST_MESSAGE})",
                    text.len()
                ),
            ));
        }
        self.send_pending_sim_frames()?;
        self.drain()?;
        self.refresh_auth()?;
        self.write(Message::Text(text.to_owned().into()))?;
        let deadline = Instant::now() + self.timing.ack_timeout;
        let mut refused = None;
        let mut settle = deadline;
        loop {
            match self.read_reply()? {
                Some(IngestReply::Appended { .. }) => return Ok(Answer::Appended),
                Some(IngestReply::Error { error, message }) => {
                    self.frame_refused(&message);
                    refused = Some((error, message));
                    settle = (Instant::now() + self.timing.settle).min(deadline);
                }
                None => {
                    let now = Instant::now();
                    if now >= settle
                        && let Some((error, message)) = refused.take()
                    {
                        return Ok(Answer::Refused(error, message));
                    }
                    if now >= deadline {
                        return Err(Failure::Transport(
                            "telemetry ingest: no answer to an events batch".into(),
                        ));
                    }
                }
            }
        }
    }

    /// Queues an item, dropping the oldest droppable event past
    /// [`MAX_OUTBOX`] (or this one, if it is droppable and nothing older is).
    fn enqueue(&mut self, item: Item) {
        let droppable =
            |item: &Item| matches!(item, Item::Event(e) if DROPPABLE.contains(&e.kind.as_str()));
        if self.outbox.len() >= MAX_OUTBOX {
            match self.outbox.iter().position(droppable) {
                Some(i) => {
                    self.outbox.remove(i);
                    self.dropped_events += 1;
                }
                None if droppable(&item) => {
                    self.dropped_events += 1;
                    return;
                }
                // Only starts, ends, joins and leaves: kept, past the bound.
                None => {}
            }
        }
        self.outbox.push_back(item);
    }

    /// Sends what the outbox holds, oldest first, until it is empty or the
    /// server is out of reach. `Err` only when the account is refused.
    fn deliver(&mut self) -> Result<(), String> {
        if let Some(error) = &self.fatal {
            return Err(error.clone());
        }
        while !self.outbox.is_empty() {
            if self.connection.is_none() && self.backing_off() {
                return Ok(());
            }
            let step = match self.outbox.front() {
                Some(Item::Start { .. }) => Self::deliver_start,
                Some(Item::Event(_)) => Self::seal,
                Some(Item::Batch(_)) => Self::deliver_batch,
                None => break,
            };
            let step = step(self);
            if let Err(failure) = step {
                return self.failed(failure);
            }
        }
        Ok(())
    }

    /// `POST /v1/games` for the start at the front. If the answer is lost
    /// after the server made the game, the retry makes a second one; the
    /// first gets no writes and the server ends it as `lost` once idle.
    fn deliver_start(&mut self) -> Result<(), Failure> {
        if self.sim.as_ref().is_some_and(|sim| sim.deferred.is_some()) {
            if self.discovery.is_none() {
                let discovery = self.discover()?;
                if let Some(deferred) = self.sim.as_mut().and_then(|sim| sim.deferred.as_mut()) {
                    deferred.encoder = ingest_frames(&discovery, SIM_KEYFRAME_EVERY);
                }
                self.discovery = Some(discovery);
            }
        } else {
            self.connect()?;
        }
        let Some(Item::Start { local, game }) = self.outbox.front() else {
            unreachable!()
        };
        let api = resolve_api(&self.base_url, &self.discovery().api);
        let path = if game.sim.is_some() {
            "admin/sim-runs"
        } else {
            "games"
        };
        let url = format!("{}/{path}", api.trim_end_matches('/'));
        if !live_proto::bearer_allowed(&url) {
            return Err(Failure::Fatal(
                "telemetry API credentials require HTTPS or loopback HTTP".into(),
            ));
        }
        let token = self.token();
        let result = Self::agent()
            .post(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_json(game);
        let local = local.clone();
        match result {
            Ok(response) => {
                let created = response
                    .into_json::<GameCreated>()
                    .map_err(|e| Failure::Transport(format!("create telemetry game JSON: {e}")))?;
                if let Some(sim) = &self.sim
                    && created.game_id != sim.game_id
                {
                    return Err(Failure::Fatal(
                        "Live returned an unexpected simulation game id".into(),
                    ));
                }
                self.games.insert(local, created.game_id);
                self.outbox.pop_front();
                self.reached();
                Ok(())
            }
            Err(ureq::Error::Status(status, response)) => {
                let body = response.into_json::<ErrorBody>().ok();
                let message = body
                    .as_ref()
                    .map_or_else(|| format!("HTTP {status}"), |b| b.message.clone());
                match (status, body.map(|b| b.error)) {
                    (_, Some(ErrorCode::Banned)) => Err(Failure::Fatal(format!(
                        "telemetry account refused: {message}"
                    ))),
                    (401, _) => Err(Failure::Auth(message)),
                    (429, _) | (500.., _) => Err(Failure::Transport(format!(
                        "create telemetry game: {message}"
                    ))),
                    _ if self.sim.is_some() => Err(Failure::Fatal(format!(
                        "Live refused the simulation: {message}"
                    ))),
                    _ => {
                        eprintln!("live telemetry: the server refused a game: {message}");
                        self.outbox.pop_front();
                        self.refused.insert(local);
                        Ok(())
                    }
                }
            }
            Err(e) => Err(Failure::Transport(format!("create telemetry game: {e}"))),
        }
    }

    /// Signs the events at the front (up to [`MAX_BATCH_EVENTS`] of one
    /// game; a `GameEnded` alone, so no other event's refusal takes it
    /// down) into a batch in their place.
    fn seal(&mut self) -> Result<(), Failure> {
        let Some(Item::Event(first)) = self.outbox.front() else {
            unreachable!()
        };
        let local = first.game_id.clone();
        let Some(server_id) = self.games.get(&local).cloned() else {
            // A game the server refused, or one never started: nowhere to
            // write.
            if !self.refused.contains(&local) {
                eprintln!("live telemetry: {} for unstarted game {local}", first.kind);
            }
            self.outbox.pop_front();
            self.dropped_events += 1;
            return Ok(());
        };
        self.connect()?;
        let mut events: Vec<GameEvent> = Vec::new();
        while events.len() < MAX_BATCH_EVENTS {
            let Some(Item::Event(next)) = self.outbox.front() else {
                break;
            };
            let ends = next.kind == game::GAME_ENDED;
            if next.game_id != local || (ends && !events.is_empty()) {
                break;
            }
            let Some(Item::Event(next)) = self.outbox.pop_front() else {
                unreachable!()
            };
            events.push(next);
            if ends {
                break;
            }
        }
        let batch = self.sign(server_id, events)?;
        self.outbox.push_front(Item::Batch(batch));
        Ok(())
    }

    fn sign(&self, server_id: String, events: Vec<GameEvent>) -> Result<Batch, Failure> {
        let now = OffsetDateTime::now_utc();
        let author = self.uploader.author(&self.discovery().id);
        let message = signed_events_message(&self.key, &author, &server_id, &events, now)
            .map_err(Failure::Fatal)?;
        Ok(Batch {
            server_id,
            events,
            text: serde_json::to_string(&message).expect("serializes"),
            signed_at: now.unix_timestamp(),
        })
    }

    /// Sends the batch at the front and settles it by the server's answer.
    fn deliver_batch(&mut self) -> Result<(), Failure> {
        self.connect()?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let Some(Item::Batch(batch)) = self.outbox.front_mut() else {
            unreachable!()
        };
        if now - batch.signed_at > RESIGN_AFTER_S {
            let events = std::mem::take(&mut batch.events);
            let server_id = batch.server_id.clone();
            let fresh = self.sign(server_id, events)?;
            self.outbox[0] = Item::Batch(fresh);
        }
        let Some(Item::Batch(batch)) = self.outbox.front() else {
            unreachable!()
        };
        let text = batch.text.clone();
        let answer = self.send_batch(&text)?;
        let Some(Item::Batch(batch)) = self.outbox.pop_front() else {
            unreachable!()
        };
        match answer {
            Answer::Appended => self.reached(),
            // Sent before, unanswered, and taken: this copy is the repeat.
            Answer::Refused(ErrorCode::Conflict, message) if message == NONCE_USED => {
                self.reached()
            }
            // The game is over: this batch's GameEnded landed unanswered
            // before, or the server ended it as lost while we were away.
            Answer::Refused(ErrorCode::Conflict, message) if message == GAME_OVER => {
                self.dropped_events += batch.events.len() as u64;
                self.reached()
            }
            Answer::Refused(error, message) => {
                let failure = match error {
                    ErrorCode::RateLimited | ErrorCode::Unavailable => Some(Failure::Transport(
                        format!("telemetry ingest busy: {message}"),
                    )),
                    ErrorCode::Banned => Some(Failure::Fatal(format!(
                        "telemetry account refused: {message}"
                    ))),
                    ErrorCode::Unauthorized => Some(Failure::Auth(message.clone())),
                    _ => None,
                };
                if let Some(failure) = failure {
                    self.outbox.push_front(Item::Batch(batch));
                    return Err(failure);
                }
                // Refused for good (the game has ended or is unknown, an
                // event the server will not take): it can never land.
                let kinds: Vec<&str> = batch.events.iter().map(|e| e.kind.as_str()).collect();
                eprintln!(
                    "live telemetry: the server refused {} for {}: {message}",
                    kinds.join(", "),
                    batch.server_id
                );
                self.dropped_events += batch.events.len() as u64;
            }
        }
        Ok(())
    }

    /// The server took something: the backoff and auth count start over.
    fn reached(&mut self) {
        self.delay = self.timing.backoff_min;
        self.retry_at = None;
        self.auth_refusals = 0;
        if self.down {
            self.down = false;
            eprintln!(
                "live telemetry: ingest back; {} waiting, {} events and {} frames dropped so far",
                self.outbox.len(),
                self.dropped_events,
                self.dropped_frames
            );
        }
    }

    /// A delivery stopped: drop the connection and back off, or give up on
    /// a refused account. `Err` only for the latter.
    fn failed(&mut self, failure: Failure) -> Result<(), String> {
        let error = match failure {
            Failure::Fatal(error) => {
                self.disconnect();
                self.fatal = Some(error.clone());
                return Err(error);
            }
            Failure::Auth(reason) => {
                self.auth_refusals += 1;
                if self.auth_refusals >= MAX_AUTH_REFUSALS {
                    let error = format!(
                        "telemetry ingest refused the account {} times: {reason}",
                        self.auth_refusals
                    );
                    self.disconnect();
                    self.fatal = Some(error.clone());
                    return Err(error);
                }
                format!("telemetry ingest refused the token: {reason}")
            }
            Failure::Transport(error) => error,
        };
        let steady = self
            .connection
            .as_ref()
            .is_some_and(|c| c.opened.elapsed() >= self.timing.steady);
        self.disconnect();
        if steady {
            self.delay = self.timing.backoff_min;
        }
        let wait = jitter(self.delay);
        if !self.down {
            self.down = true;
            eprintln!("live telemetry: {error}; reconnecting");
        }
        self.retry_at = Some(Instant::now() + wait);
        self.delay = (self.delay * 2).min(self.timing.backoff_max);
        Ok(())
    }

    fn disconnect(&mut self) {
        if let Some(sim) = &mut self.sim {
            sim.pending.clear();
        }
        if let Some(mut connection) = self.connection.take() {
            let _ = connection.socket.close(None);
            let _ = connection.socket.flush();
        }
    }

    fn send_frame(&mut self, mut frame: Frame, server_id: String) -> Result<(), Failure> {
        self.connect()?;
        frame.game_id = server_id;
        self.refresh_auth()?;
        frame.cut = frame.cut.or(self.machine_cut);
        let frames = &mut self.connection.as_mut().unwrap().frames;
        let message = frame_message(frames, frame, Some(&self.machine_positions))
            .map_err(Failure::Transport)?;
        if message.len() > MAX_INGEST_MESSAGE {
            frames.force_keyframe();
            self.dropped_frames += 1;
            return Ok(());
        }
        if self.sim.is_some() {
            self.queue_sim_frame(message)?;
            self.send_combat()?;
            // Retain a cut across connection failures until its batch is sent.
            if self.machine_cut.is_some() {
                self.send_pending_sim_frames()?;
            }
        } else {
            self.write(message)?;
            // Drain available replies without waiting on the guest.
            self.drain()?;
        }
        self.machine_cut = None;
        Ok(())
    }

    fn send_combat(&mut self) -> Result<(), Failure> {
        if !self.discovery().sim_combat {
            return Ok(());
        }
        let sim = self.sim.as_mut().expect("sim capture");
        let Some(read) = sim.combat.as_ref().and_then(|feed| feed.get()) else {
            return Ok(());
        };
        let now = Instant::now();
        if sim.next_combat.is_some_and(|due| due > now) {
            return Ok(());
        }
        let game_id = sim.game_id.clone();
        let samples = read();
        // The first frame can precede binding or spawning the sim roster.
        sim.next_combat = Some(now + Duration::from_secs(if samples.is_empty() { 1 } else { 5 }));
        for sample in samples {
            let message = ClientMessage::SimCombat { game_id: game_id.clone(), sample: Box::new(sample) };
            let text = serde_json::to_string(&message).expect("finite sim combat sample");
            if text.len() <= MAX_INGEST_MESSAGE {
                self.write(Message::Text(text.into()))?;
            }
        }
        Ok(())
    }

    fn keyframe_every(&self) -> u64 {
        if self.sim.is_some() {
            SIM_KEYFRAME_EVERY
        } else {
            FRAME_KEYFRAME_EVERY
        }
    }

    /// The usual packed stream, with multiple records per WebSocket
    /// message and no read timeout paid per frame at unpaced sim speeds.
    fn queue_sim_frame(&mut self, message: Message) -> Result<(), Failure> {
        let Message::Binary(bytes) = message else {
            self.send_pending_sim_frames()?;
            return self.write(message);
        };
        let sim = self.sim.as_ref().expect("sim capture");
        if sim.pending.len() + bytes.len() > MAX_INGEST_MESSAGE
            || sim.since.elapsed() >= SIM_FRAME_BATCH_DELAY
        {
            self.send_pending_sim_frames()?;
        }
        let sim = self.sim.as_mut().expect("sim capture");
        if sim.pending.is_empty() {
            sim.since = Instant::now();
        }
        sim.pending.extend_from_slice(&bytes);
        Ok(())
    }

    fn send_pending_sim_frames(&mut self) -> Result<(), Failure> {
        if let Some(sim) = &mut self.sim
            && !sim.pending.is_empty()
        {
            let bytes = std::mem::take(&mut sim.pending);
            self.refresh_auth()?;
            self.write(Message::Binary(bytes.into()))?;
        }
        Ok(())
    }

    fn settle_sim_events(&mut self) -> Result<(), String> {
        let until = Instant::now() + SIM_SETTLE_TIMEOUT;
        while !self.outbox.is_empty() {
            self.deliver()?;
            if self.outbox.is_empty() {
                break;
            }
            if Instant::now() >= until {
                return Err("Live did not acknowledge simulation telemetry".into());
            }
            if let Some(delay) = self
                .retry_at
                .and_then(|t| t.checked_duration_since(Instant::now()))
            {
                std::thread::sleep(delay.min(Duration::from_secs(2)));
            }
        }
        Ok(())
    }

    fn upload_sim_report(&mut self, report: serde_json::Value) -> Result<(), String> {
        self.settle_sim_events()?;
        let capture = self.sim.as_mut().ok_or("no sim capture")?;
        let id = capture.game_id.clone();
        if self.games.get(&id) != Some(&id) {
            return Err(
                "Live did not create the simulation game (check the selected account's admin role)"
                    .into(),
            );
        }
        let mut guard = None;
        if let Some(deferred) = capture.deferred.take() {
            let lock = OpenOptions::new()
                .read(true)
                .create(true)
                .truncate(false)
                .write(true)
                .open(&deferred.lock)
                .map_err(|e| format!("open deferred sim upload lock: {e}"))?;
            lock.lock()
                .map_err(|e| format!("lock deferred sim upload: {e}"))?;
            guard = Some(lock);
            self.connect()
                .map_err(|e| format!("connect deferred sim upload: {e:?}"))?;
            for item in deferred.items {
                match item {
                    DeferredItem::Event(event) => {
                        self.enqueue(Item::Event(event));
                        self.settle_sim_events()?;
                    }
                    DeferredItem::Frame(message) => {
                        self.queue_sim_frame(message)
                            .map_err(|e| format!("deferred sim frame: {e:?}"))?;
                    }
                }
            }
        }
        self.send_pending_sim_frames()
            .map_err(|e| format!("final sim frames: {e:?}"))?;
        self.settle_sim_events()?;
        let result = self.put_report(&id, report);
        drop(guard);
        result
    }

    pub(crate) fn report_after_film(
        mut self, id: &str, report: serde_json::Value,
    ) -> Result<(), String> {
        self.discovery = Some(self.discover().map_err(|e| format!("sim report discovery: {e:?}"))?);
        self.put_report(id, report)
    }

    fn put_report(&self, id: &str, report: serde_json::Value) -> Result<(), String> {
        let api = resolve_api(&self.base_url, &self.discovery().api);
        let url = format!("{}/admin/sim-runs/{id}/report", api.trim_end_matches('/'));
        if !live_proto::bearer_allowed(&url) {
            return Err("sim report credentials require HTTPS or loopback HTTP".into());
        }
        let bytes = encode_sim_report(report)?;
        let mut result = Ok(());
        for delay in [0, 1, 2] {
            if delay != 0 {
                std::thread::sleep(Duration::from_secs(delay));
            }
            let token = self.token();
            let response = Self::agent()
                .put(&url)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Content-Type", "application/json")
                .send_bytes(&bytes);
            match response {
                Ok(_) => {
                    eprintln!("sim: Live recording {id}");
                    return Ok(());
                }
                Err(e @ ureq::Error::Status(status, _)) if status < 500 && status != 429 => {
                    return Err(format!("uploading sim report: {e}"));
                }
                Err(e) => result = Err(format!("uploading sim report: {e}")),
            }
        }
        result
    }
}

/// Both immediate and deferred sim uploads use the same capped payload.
fn encode_sim_report(mut report: serde_json::Value) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; game::MAX_SIM_REPORT_BYTES];
    let write = |bytes: &mut [u8], value: &serde_json::Value| {
        let mut out = bytes;
        serde_json::to_writer(&mut out, value)?;
        Ok::<_, serde_json::Error>(game::MAX_SIM_REPORT_BYTES - out.len())
    };
    let len = match write(&mut bytes, &report) {
        Ok(n) => n,
        Err(error) if error.io_error_kind() == Some(ErrorKind::WriteZero) => {
            let obj = report.as_object_mut().ok_or("expected a sim report object")?;
            // The runner saves the full per-tick diff locally before upload.
            obj.remove("state_components_v1");
            obj.remove("state_hashes_v1");
            if let Some(notes) = obj.entry("notes").or_insert_with(|| serde_json::json!([])).as_array_mut() {
                notes.push("The per-tick determinism traces exceed the Live report limit and are omitted.".into());
            }
            write(&mut bytes, &report).map_err(|e| format!("sim report exceeds 512 KiB or failed to encode: {e}"))?
        }
        Err(e) => return Err(format!("encoding sim report: {e}")),
    };
    bytes.truncate(len);
    Ok(bytes)
}

/// A close from the server: 4401 is the token refused (or expired), a
/// reason naming a ban is the account refused; anything else is a drop.
fn closed(code: u16, reason: String) -> Failure {
    if reason.to_ascii_lowercase().contains("banned") {
        Failure::Fatal(format!("telemetry account refused: {reason}"))
    } else if code == CLOSE_UNAUTHORIZED {
        Failure::Auth(reason)
    } else {
        Failure::Transport(format!("telemetry ingest closed ({code}): {reason}"))
    }
}

/// `delay` give or take a fifth, so many games do not reconnect at once.
fn jitter(delay: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    // 0.8 to 1.2.
    let factor = 0.8 + f64::from(nanos % 1000) / 1000.0 * 0.4;
    delay.mul_f64(factor)
}

impl Sink for ServerSink {
    fn event(&mut self, event: GameEvent) -> Result<(), String> {
        if let Some(error) = &self.fatal {
            return Err(error.clone());
        }
        if event.kind == game::GAME_STARTED {
            let started: game::GameStarted = serde_json::from_value(event.data.clone())
                .map_err(|e| format!("GameStarted data: {e}"))?;
            self.enqueue(Item::Start {
                local: event.game_id,
                game: Box::new(started.game),
            });
            // Now, so the frames that follow have a server game to go to.
            self.deliver()
        } else if let Some(deferred) = self.sim.as_mut().and_then(|sim| sim.deferred.as_mut()) {
            let size = event.kind.len() + event.game_id.len() + event.data.to_string().len();
            deferred.push(DeferredItem::Event(event), size)
        } else {
            self.enqueue(Item::Event(event));
            Ok(())
        }
    }

    /// A frame goes now or not at all: while the server is out of reach,
    /// or its game is not created yet, it is dropped.
    fn frame(&mut self, frame: Frame) -> Result<(), String> {
        if let Some(error) = &self.fatal {
            return Err(error.clone());
        }
        if self.machine_game != frame.game_id {
            self.machine_game.clone_from(&frame.game_id);
            self.machine_positions.clear();
            self.machine_cut = None;
        }
        if frame.cut.is_some() {
            self.machine_positions.clear();
            self.machine_cut = frame.cut;
        }
        for m in &frame.machine_positions {
            if self.machine_positions.len() < game::MAX_MACHINE_POSITIONS
                || self.machine_positions.contains_key(&m.i)
            {
                self.machine_positions.insert(m.i, *m);
            }
        }
        if let Some(sim) = &mut self.sim
            && let Some(deferred) = &mut sim.deferred
        {
            let mut frame = frame;
            frame.game_id.clone_from(&sim.game_id);
            frame.cut = frame.cut.or(self.machine_cut);
            let message =
                frame_message(&mut deferred.encoder, frame, Some(&self.machine_positions))?;
            let size = message.len();
            if size > MAX_INGEST_MESSAGE {
                return Err("a deferred sim frame exceeds the ingest message limit".into());
            }
            deferred.push(DeferredItem::Frame(message), size)?;
            self.machine_cut = None;
            return Ok(());
        }
        let Some(server_id) = self.games.get(&frame.game_id).cloned() else {
            self.dropped_frames += 1;
            return Ok(());
        };
        if self.connection.is_none() && self.backing_off() {
            self.dropped_frames += 1;
            return Ok(());
        }
        match self.send_frame(frame, server_id) {
            Ok(()) => Ok(()),
            Err(failure) => {
                self.dropped_frames += 1;
                self.failed(failure)
            }
        }
    }

    fn flush(&mut self) -> Result<(), String> {
        self.deliver()
    }

    fn finish_sim(&mut self, report: serde_json::Value) -> Result<(), String> {
        self.upload_sim_report(report)
    }
}

impl Drop for ServerSink {
    fn drop(&mut self) {
        // One last go on a live connection; no reconnect on the way out.
        if self.connection.is_some() && self.fatal.is_none() {
            let _ = self.deliver();
        }
        if !self.outbox.is_empty() {
            eprintln!(
                "live telemetry: {} game starts and events not delivered",
                self.outbox.len()
            );
        }
        self.disconnect();
    }
}

/// A frame as an ingest message: one packed record in a binary message, or
/// (a frame the format cannot carry, which leaves the stream as it was)
/// the JSON `Frame` message; the server takes both.
fn frame_message(
    frames: &mut pack::Encoder,
    frame: Frame,
    baseline: Option<&BTreeMap<u16, MachinePosition>>,
) -> Result<Message, String> {
    match frames.encode_with_machine_baseline(&frame, baseline) {
        Ok((record, _)) => {
            let mut bytes = Vec::with_capacity(record.len() + 3);
            pack::push_record(&mut bytes, &record);
            Ok(Message::Binary(bytes.into()))
        }
        Err(_) => {
            let message = ClientMessage::Frame {
                frame: Box::new(frame),
            };
            let text = serde_json::to_string(&message).map_err(|e| e.to_string())?;
            Ok(Message::Text(text.into()))
        }
    }
}

fn ingest_frames(discovery: &Discovery, every: u64) -> pack::Encoder {
    if discovery
        .ingest_frame_versions
        .contains(&pack::LIVE_VERSION)
    {
        pack::Encoder::with_version(0, every, pack::LIVE_VERSION)
            .expect("local live encoder version")
    } else {
        pack::Encoder::new(0, every)
    }
}

fn connect_bounded(
    url: &str,
) -> Result<(Socket, tungstenite::handshake::client::Response), String> {
    let url = url.to_owned();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("h2-telemetry-connect".into())
        .spawn(move || {
            let _ = sender.send(connect_bounded_io(&url));
        })
        .map_err(|e| format!("spawn telemetry connector: {e}"))?;
    receiver
        .recv_timeout(Duration::from_secs(20))
        .map_err(|_| "telemetry connect timed out".to_owned())?
}

fn connect_bounded_io(
    url: &str,
) -> Result<(Socket, tungstenite::handshake::client::Response), String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| "telemetry ingest URL has no scheme".to_owned())?;
    let authority = rest.split('/').next().unwrap_or("");
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let secure = match scheme {
        "ws" => false,
        "wss" => true,
        _ => return Err("telemetry ingest URL must use ws or wss".into()),
    };
    let default_port = if secure { 443 } else { 80 };
    let (host, address) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or_else(|| "invalid IPv6 telemetry host".to_owned())?;
        let address = if suffix.is_empty() {
            format!("{host_port}:{default_port}")
        } else {
            host_port.to_owned()
        };
        (host, address)
    } else if host_port.contains(':') {
        (
            host_port.split(':').next().unwrap_or(""),
            host_port.to_owned(),
        )
    } else {
        (host_port, format!("{host_port}:{default_port}"))
    };
    let addresses: Vec<_> = address
        .to_socket_addrs()
        .map_err(|e| format!("resolve telemetry ingest: {e}"))?
        .collect();
    if addresses.is_empty() {
        return Err("telemetry ingest resolved no address".into());
    }
    let mut last_error = "telemetry ingest TCP connect failed".to_owned();
    for socket_addr in addresses {
        match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(5)) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|e| e.to_string())?;
                tcp.set_write_timeout(Some(Duration::from_secs(5)))
                    .map_err(|e| e.to_string())?;
                return if secure {
                    let mut roots = rustls::RootCertStore::empty();
                    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                    let config = Arc::new(
                        rustls::ClientConfig::builder_with_provider(
                            rustls::crypto::ring::default_provider().into(),
                        )
                        .with_safe_default_protocol_versions()
                        .map_err(|e| format!("telemetry TLS versions: {e}"))?
                        .with_root_certificates(roots)
                        .with_no_client_auth(),
                    );
                    let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())
                        .map_err(|e| format!("telemetry TLS server name: {e}"))?;
                    let connection = rustls::ClientConnection::new(config, server_name)
                        .map_err(|e| format!("telemetry TLS setup: {e}"))?;
                    tungstenite::client::client(
                        url,
                        MaybeTlsStream::Rustls(rustls::StreamOwned::new(connection, tcp)),
                    )
                    .map_err(|e| format!("telemetry ingest handshake: {e}"))
                } else {
                    tungstenite::client::client(url, MaybeTlsStream::Plain(tcp))
                        .map_err(|e| format!("telemetry ingest handshake: {e}"))
                };
            }
            Err(error) => last_error = format!("telemetry ingest TCP connect: {error}"),
        }
    }
    Err(last_error)
}

fn set_timeouts(stream: &mut MaybeTlsStream<TcpStream>) -> Result<(), String> {
    let tcp = match stream {
        MaybeTlsStream::Plain(tcp) => tcp,
        MaybeTlsStream::Rustls(tls) => &mut tls.sock,
        _ => return Err("unsupported telemetry TLS stream".into()),
    };
    tcp.set_read_timeout(Some(Duration::from_millis(50)))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())
}

fn resolve_api(base_url: &str, api: &str) -> String {
    if api.starts_with("http://") || api.starts_with("https://") {
        return api.trim_end_matches('/').to_owned();
    }
    if api.starts_with('/') {
        let Some(scheme) = base_url.find("://") else {
            return api.trim_end_matches('/').to_owned();
        };
        let authority = scheme + 3;
        let end = base_url[authority..]
            .find('/')
            .map_or(base_url.len(), |i| authority + i);
        return format!("{}{}", &base_url[..end], api.trim_end_matches('/'));
    }
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        api.trim_matches('/')
    )
}

pub(crate) fn signed_events_message(
    key: &KeyPair,
    author: &str,
    game_id: &str,
    events: &[GameEvent],
    now: OffsetDateTime,
) -> Result<ClientMessage, String> {
    if events.is_empty() {
        return Err("cannot sign an empty telemetry batch".into());
    }
    let bodies: Vec<EventBody> = events
        .iter()
        .map(|event| {
            EventBody::new(
                key,
                author,
                &live_proto::game::stream(game_id),
                &event.kind,
                event.data.clone(),
                now,
            )
        })
        .collect();
    Ok(ClientMessage::Events {
        game_id: game_id.to_owned(),
        jws: sign_batch(key, &bodies),
    })
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod reconnect;

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::sign::{KeyPair, SignedEvent, Token};

    #[test]
    fn signed_ingest_batch_verifies_with_live_proto() {
        let key = KeyPair::from_secret([7; 32]);
        let author = "acct:0009000000000001@localhost";
        let events = vec![
            GameEvent::new(
                "server-game",
                "PlayerJoined",
                live_proto::game::PlayerJoined {
                    player: live_proto::game::Player {
                        index: 0,
                        name: "A".into(),
                        xuid: None,
                        team: 0,
                        local: Some(true),
                        appearance: None,
                        bot: None,
                    },
                },
            ),
            GameEvent::new(
                "server-game",
                "ScoreChanged",
                live_proto::game::ScoreChanged {
                    tick: 30,
                    players: vec![],
                    teams: None,
                },
            ),
        ];
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let message = signed_events_message(&key, author, "server-game", &events, now).unwrap();
        let ClientMessage::Events { game_id, jws } = message else {
            panic!()
        };
        assert_eq!(game_id, "server-game");
        let (parsed, bodies) = SignedEvent::open_batch(&jws).unwrap();
        parsed.verify(key.public()).unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].kind, "PlayerJoined");
        assert_eq!(bodies[1].kind, "ScoreChanged");
        assert!(
            bodies
                .iter()
                .all(|b| b.stream == "game-server-game" && b.author == author)
        );
    }

    #[test]
    fn host_telemetry_uses_the_lease_bearer_and_batch_key() {
        let issuer = KeyPair::from_secret([8; 32]);
        let key = KeyPair::from_secret([9; 32]);
        let subject = "host:hl-abc";
        let raw = issuer.sign_json(live_proto::sign::TYP_TOKEN, &serde_json::json!({
            "sub": subject, "aud": "local", "event_key": key.secret_b64(),
        }));
        let mut sink =
            ServerSink::for_host("http://localhost:8074".into(), HostToken(raw.clone())).unwrap();
        let mut discovery = Discovery {
            id: "local".into(),
            api: "/v1".into(),
            keys: vec![issuer.public().jwk()],
            software: "test".into(),
            federation: false,
            url: None,
            sim_combat: false,
            ingest_frame_versions: vec![],
        };
        assert!(sink.check_host_destination(&discovery).is_ok());
        discovery.api = "https://other.example/v1".into();
        assert!(sink.check_host_destination(&discovery).is_err());
        discovery.api = "/v1".into();
        discovery.id = "wrong".into();
        assert!(sink.check_host_destination(&discovery).is_err());
        discovery.id = "local".into();
        discovery.keys = vec![key.public().jwk()];
        assert!(sink.check_host_destination(&discovery).is_err());
        discovery.keys = vec![issuer.public().jwk()];
        sink.discovery = Some(discovery);

        assert!(sink.token() == raw);
        let event = GameEvent::new("local", game::GAME_ENDED, serde_json::json!({
            "tick": 1800, "reason": "finished", "players": [],
        }));
        let batch = sink.sign("leased-game".into(), vec![event]).unwrap();
        let ClientMessage::Events { jws, .. } = serde_json::from_str(&batch.text).unwrap() else {
            panic!("expected an event batch")
        };
        let (signed, bodies) = SignedEvent::open_batch(&jws).unwrap();
        signed.verify(key.public()).unwrap();
        assert!(signed.verify(issuer.public()).is_err());
        assert_eq!(bodies[0].author, subject);
        assert_eq!(bodies[0].stream, game::stream("leased-game"));
    }

    #[test]
    fn frames_go_as_packed_records_the_server_decodes() {
        let mut frames = pack::Encoder::new(0, FRAME_KEYFRAME_EVERY);
        let mut decoder = pack::Decoder::new();
        let sample = live_proto::samples::multiplayer(4);
        for (n, frame) in sample.iter().enumerate() {
            let Message::Binary(bytes) = frame_message(&mut frames, frame.clone(), None).unwrap()
            else {
                panic!("frame {n} not packed");
            };
            assert!(bytes.len() < MAX_INGEST_MESSAGE);
            let records: Vec<_> = pack::records(&bytes).collect::<Result<_, _>>().unwrap();
            assert_eq!(records.len(), 1);
            let decoded = decoder.decode(records[0]).unwrap();
            assert_eq!(decoded.keyframe, n == 0);
            assert_eq!(decoded.frame, pack::quantize(frame).unwrap());
        }
        // A frame the format cannot carry goes as JSON, and the stream
        // carries on.
        let mut odd = sample[0].clone();
        odd.players[0].pos[0] = f64::NAN;
        assert!(matches!(
            frame_message(&mut frames, odd, None).unwrap(),
            Message::Text(_)
        ));
        let Message::Binary(bytes) = frame_message(&mut frames, sample[3].clone(), None).unwrap()
        else {
            panic!("not packed");
        };
        let record = pack::records(&bytes).next().unwrap().unwrap();
        assert_eq!(
            decoder.decode(record).unwrap().frame,
            pack::quantize(&sample[3]).unwrap()
        );
    }

    #[test]
    fn request_token_verifies_with_live_proto() {
        let key = KeyPair::from_secret([9; 32]);
        let now = 1_800_000_000;
        let token = issue_token(&key, "acct:a@localhost", "localhost", lxm::GAMES_WRITE, now);
        Token::parse(&token)
            .unwrap()
            .verify(key.public(), "localhost", lxm::GAMES_WRITE, now + 1)
            .unwrap();
    }

    #[test]
    fn account_xuids_are_normalized_for_signed_authors() {
        let account = TelemetryAccount {
            xuid: "0009A1B2C3D4E5F6".into(),
            gamertag: "A".into(),
            key: KeyPair::from_secret([3; 32]).private_jwk(),
        };
        let sink = ServerSink::new("http://localhost:8074".into(), account).unwrap();
        assert_eq!(
            sink.uploader.author("localhost"),
            "acct:0009a1b2c3d4e5f6@localhost"
        );
    }

    #[test]
    fn api_resolution_handles_absolute_relative_and_root_paths() {
        assert_eq!(
            resolve_api("http://localhost:8074", "/v1"),
            "http://localhost:8074/v1"
        );
        assert_eq!(
            resolve_api("http://localhost:8074/base", "v1"),
            "http://localhost:8074/base/v1"
        );
        assert_eq!(
            resolve_api("http://localhost:8074/base", "https://live.test/v1"),
            "https://live.test/v1"
        );
    }
}

#[cfg(test)]
mod version_negotiation {
    use super::*;

    #[test]
    fn ingest_selects_only_advertised_v8_and_resets_on_a_key() {
        let mut doc = serde_json::json!({"id":"local","api":"/v1","keys":[],"software":"test","federation":false});
        for (advertised, want) in [(None, 7), (Some(vec![9]), 7), (Some(vec![7, 8]), 8)] {
            if let Some(v) = advertised {
                doc["ingest_frame_versions"] = serde_json::json!(v);
            }
            let discovery: Discovery = serde_json::from_value(doc.clone()).unwrap();
            let mut enc = ingest_frames(&discovery, 150);
            let frame = Frame::default();
            let (record, key) = enc.encode(&frame).unwrap();
            assert!(key);
            assert_eq!(record[0] & 15, want);
            enc.force_keyframe();
            let (record, key) = enc.encode(&frame).unwrap();
            assert!(key);
            assert_eq!(pack::Decoder::new().decode(&record).unwrap().seq, 1);
        }
    }
}
