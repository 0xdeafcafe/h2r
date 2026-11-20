//! Games (docs/live/live-api.md): starting one, appending its signed event
//! batches, live telemetry frames, recorded frame chunks (and the
//! `GameFrame`s older games recorded), map outlines, and
//! the presence and game managers that write `PresenceExpired` and
//! `GameEnded {reason: "lost"}` when clients go quiet.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::rt::Instant;
use bytes::Bytes;

use live_proto::game::{
    self, Frame, GameCreated, GameDetail, GameEnded, GameStarted, GameSummary, NewGame, Outline,
};
use live_proto::chunk::{self, Chunk, ChunkMeta, ChunkWriter};
use live_proto::pack;
use live_proto::sign::{self, MAX_EVENT_SKEW_S, SignedEvent, parse_time};
use live_proto::{Author, Page, lxm, streams, types};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::{
    ApiError, Live, conflict, forbidden, invalid, not_found, now, sign_error, unauthorized,
};
use crate::host_identity::{Publisher, TELEMETRY_INGEST}; // dh-identity
use crate::model::{Game, nonce_key};
use crate::store::{NewEvent, Query};

/// Ordinary live frames are admitted at this average interval per game.
const FRAME_INTERVAL: Duration = Duration::from_millis(1000 / 15);
/// `GET /v1/games/{id}/frames` page size.
pub const FRAMES_PAGE: usize = 600;

/// Where a game's frame chunks live in the private blob store:
/// `frames/<game id>/<first tick>` (docs/live/live-api.md, "Frame chunks").
pub fn chunk_key(game_id: &str, from_tick: u64) -> String {
    format!("{}{game_id}/{from_tick}", crate::blobs::FRAMES_PREFIX)
}

/// A chunk's content type (`GET /v1/games/{id}/chunks/{tick}`).
pub const CHUNK_TYPE: &str = "application/x-h2-chunk";

/// A keyframe on the packed live stream at least this often (one a second
/// at 15 Hz): what a reader that joins or falls behind waits for at most.
const STREAM_KEYFRAME_EVERY: u64 = 15;

/// Per-game frame bookkeeping for the ingest path. The map's lock is held
/// only to find a game's clock; each game's frames are packed under its
/// own lock, so games ingest side by side.
#[derive(Default)]
pub struct IngestState {
    games: Mutex<HashMap<String, Arc<Mutex<FrameClock>>>>,
    /// The next packed stream id (one per game while it sends frames).
    next_sid: std::sync::atomic::AtomicU64,
}

/// A live frame as stream readers get it: the frame for JSON readers, and
/// its record in the game's packed stream (docs/live/live-telemetry.md, Packed
/// frames) for packed ones.
#[derive(Debug)]
pub struct LiveFrame {
    pub frame: Frame,
    /// The game's packed stream and this frame's place in it.
    pub sid: u64,
    pub seq: u64,
    pub keyframe: bool,
    /// `None` when the frame would not pack (the stream skips it).
    pub packed: Option<Vec<u8>>,
    /// The JSON stream message, made by the first JSON reader that sends
    /// the frame and shared by the rest ([`LiveFrame::json_message`]).
    json: std::sync::OnceLock<WsText>,
}

/// A WebSocket text message, shared without copying: axum's on a native
/// server; the text itself on a Worker, whose sockets are JavaScript's.
#[cfg(feature = "native")]
pub type WsText = axum::extract::ws::Utf8Bytes;
#[cfg(not(feature = "native"))]
pub type WsText = std::sync::Arc<str>;

impl LiveFrame {
    /// The frame as a JSON reader's `frame` stream message: serialised once
    /// however many JSON readers there are (it is the same for all of them;
    /// visibility only decides who gets it).
    pub fn json_message(&self) -> WsText {
        self.json
            .get_or_init(|| {
                let message = live_proto::StreamMessage::Frame {
                    frame: Box::new(self.frame.clone()),
                };
                serde_json::to_string(&message).expect("serializes").into()
            })
            .clone()
    }

    /// A frame of stream `sid` with these packed bytes, for tests.
    #[cfg(test)]
    pub(crate) fn packed_for_test(sid: u64, seq: u64, keyframe: bool, packed: Vec<u8>) -> Self {
        LiveFrame {
            frame: Frame::default(),
            sid,
            seq,
            keyframe,
            packed: Some(packed),
            json: Default::default(),
        }
    }
}

impl IngestState {
    /// Game `id`'s clock, made (with the next stream id) on its first frame.
    fn clock(&self, id: &str) -> Arc<Mutex<FrameClock>> {
        let mut games = self.games.lock().unwrap();
        if let Some(clock) = games.get(id) {
            return Arc::clone(clock);
        }
        let sid = self
            .next_sid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let clock = Arc::new(Mutex::new(FrameClock::new(Instant::now(), sid)));
        games.insert(id.to_owned(), Arc::clone(&clock));
        clock
    }
}

/// One trained v8 writer per game, shared by its negotiated subscribers.
struct CausalStream {
    encoder: pack::Encoder,
    history: crate::packed::Pending,
    pending: crate::packed::Pending,
}

impl CausalStream {
    fn new(sid: u64) -> Self {
        Self {
            encoder: pack::Encoder::with_version(sid, 150, pack::LIVE_VERSION)
                .expect("supported live version"),
            history: Default::default(),
            pending: Default::default(),
        }
    }

    fn encode(
        &mut self,
        frame: &Frame,
        baseline: &BTreeMap<u16, game::MachinePosition>,
    ) -> (u64, bool, Option<Vec<u8>>) {
        let seq = self.encoder.next_seq();
        match self
            .encoder
            .encode_with_machine_baseline(frame, Some(baseline))
        {
            Ok((bytes, key)) => {
                if key {
                    self.history = Default::default();
                }
                self.history.push(seq, key, &bytes);
                self.pending.push(seq, key, &bytes);
                (seq, key, Some(bytes))
            }
            Err(e) => {
                eprintln!("live: v8 frame of {} does not pack: {e}", frame.game_id);
                (seq, false, None)
            }
        }
    }
}

struct FrameClock {
    last_sent: Option<Instant>,
    /// Virtual time advances one frame interval for each admitted frame.
    next_frame_credit: Instant,
    sim_tick: Option<u64>,
    /// Private live training samples, replaced in seat order. Never encoded
    /// in a frame, a stream broadcast or an archive chunk.
    sim_combat: BTreeMap<u8, game::SimCombatSample>,
    /// What throttled frames carried (shots, actions, hits, machines),
    /// handed to the next frame sent so none is lost.
    unsent: game::FrameCarry,
    /// Every machine's latest state, by id: a chunk's first frame carries
    /// them all, so a reader that starts there has every door.
    machines: BTreeMap<String, game::FrameMachine>,
    machine_positions: BTreeMap<u16, game::MachinePosition>,
    record_machine_reset: bool,
    /// The sent frames being cut into chunks for the recording.
    chunks: ChunkWriter,
    // Retain closed chunks until their signed index is durable.
    writes: VecDeque<(ChunkMeta, Bytes)>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    causal: Option<CausalStream>,
    /// A new version can start at the last published tick even if ingest
    /// has already observed newer, throttled machine changes.
    latest: Option<Arc<LiveFrame>>,
    published_positions: BTreeMap<u16, game::MachinePosition>,
    /// The game's legacy packed live stream.
    pack: pack::Encoder,
    /// Its frames since the last keyframe, for readers that join.
    since_key: Vec<Arc<LiveFrame>>,
    /// Its packed frames since the last tick ([`crate::packed`]).
    pending: crate::packed::Pending,
}

impl FrameClock {
    fn new(now: Instant, sid: u64) -> Self {
        FrameClock {
            last_sent: None,
            next_frame_credit: now,
            sim_tick: None,
            sim_combat: BTreeMap::new(),
            unsent: game::FrameCarry::default(),
            machines: BTreeMap::new(),
            machine_positions: BTreeMap::new(),
            record_machine_reset: false,
            chunks: ChunkWriter::default(),
            writes: VecDeque::new(),
            write_lock: Default::default(),
            causal: None,
            latest: None,
            published_positions: BTreeMap::new(),
            pack: pack::Encoder::new(sid, STREAM_KEYFRAME_EVERY),
            since_key: Vec::new(),
            pending: Default::default(),
        }
    }

    /// Packs a sent frame into the game's stream and keeps it for readers
    /// that join before the next keyframe.
    #[cfg(test)]
    fn publish(&mut self, frame: Frame) -> Arc<LiveFrame> {
        self.publish_versions(frame, [true, false], &Default::default())
    }

    fn publish_versions(
        &mut self,
        mut frame: Frame,
        versions: [bool; 2],
        next_sid: &std::sync::atomic::AtomicU64,
    ) -> Arc<LiveFrame> {
        let [v7, v8] = versions;
        let legacy = v7 || !v8;
        let causal = if v8 {
            let stream = self.causal.get_or_insert_with(|| {
                CausalStream::new(next_sid.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
            });
            let (seq, key, bytes) = stream.encode(&frame, &self.machine_positions);
            Some((stream.encoder.sid(), seq, key, bytes))
        } else {
            self.causal = None;
            None
        };
        let (sid, seq, keyframe, packed) = if legacy {
            let seq = self.pack.next_seq();
            let (packed, key) = match self
                .pack
                .encode_with_machine_baseline(&frame, Some(&self.machine_positions))
            {
                Ok((bytes, key)) => (Some(bytes), key),
                Err(e) => {
                    eprintln!("live: frame of {} does not pack: {e}", frame.game_id);
                    (None, false)
                }
            };
            (self.pack.sid(), seq, key, packed)
        } else {
            self.since_key.clear();
            self.pack.force_keyframe();
            causal.expect("v8 requested")
        };
        if keyframe || frame.cut.is_some() {
            self.published_positions.clone_from(&self.machine_positions);
        } else {
            for m in &frame.machine_positions {
                self.published_positions.insert(m.i, *m);
            }
        }
        if keyframe {
            frame.machine_positions = self.published_positions.values().copied().collect();
        }
        let live = Arc::new(LiveFrame {
            frame,
            sid,
            seq,
            keyframe,
            packed,
            json: Default::default(),
        });
        if legacy {
            if keyframe {
                self.since_key.clear();
            }
            if let Some(bytes) = &live.packed {
                self.pending.push(seq, keyframe, bytes);
                self.since_key.push(Arc::clone(&live));
            }
        }
        self.latest = Some(Arc::clone(&live));
        live
    }

    fn join(
        &mut self,
        v8: bool,
        next_sid: &std::sync::atomic::AtomicU64,
        follow: &mut crate::packed::Follow,
        out: &mut Vec<u8>,
    ) {
        let Some(latest) = &self.latest else { return };
        if v8 {
            let causal = self.causal.get_or_insert_with(|| {
                let sid = next_sid.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut stream = CausalStream::new(sid);
                stream.encode(&latest.frame, &self.published_positions);
                stream
            });
            causal.history.replay(causal.encoder.sid(), follow, out);
        } else {
            if self.since_key.is_empty() {
                self.pack.force_keyframe();
                let seq = self.pack.next_seq();
                if let Ok((bytes, true)) = self
                    .pack
                    .encode_with_machine_baseline(&latest.frame, Some(&self.published_positions))
                {
                    let mut frame = latest.frame.clone();
                    frame.machine_positions = self.published_positions.values().copied().collect();
                    self.pending.push(seq, true, &bytes);
                    self.since_key.push(Arc::new(LiveFrame {
                        frame,
                        sid: self.pack.sid(),
                        seq,
                        keyframe: true,
                        packed: Some(bytes),
                        json: Default::default(),
                    }));
                }
            }
            for frame in &self.since_key {
                follow.take_frame(frame, out);
            }
        }
    }

    fn observe_machines(&mut self, frame: &Frame) {
        for m in &frame.machines {
            if self.machines.len() < game::MAX_FRAME_MACHINES || self.machines.contains_key(&m.id) {
                self.machines.insert(m.id.clone(), m.clone());
            }
        }
    }

    fn observe_travel(&mut self, frame: &mut Frame, machine_baseline: bool) {
        if frame.cut.is_some() {
            self.machine_positions.clear();
        }
        if machine_baseline
            && self.machine_positions.keys().any(|i| {
                frame
                    .machine_positions
                    .binary_search_by_key(i, |m| m.i)
                    .is_err()
            })
        {
            // Publish a complete baseline so consumers forget removed keys.
            // This also supersedes travel waiting in throttled frames.
            self.machine_positions.clear();
            self.unsent.clear_machine_positions();
            // Sim recording and live fan-out can consume the reset on different frames.
            self.record_machine_reset = true;
            self.pack.force_keyframe();
            if let Some(stream) = &mut self.causal {
                stream.encoder.force_keyframe();
            }
        }
        frame.machine_positions.retain(|m| {
            if self.machine_positions.get(&m.i) == Some(m) {
                return false;
            }
            if self.machine_positions.len() < game::MAX_MACHINE_POSITIONS
                || self.machine_positions.contains_key(&m.i)
            {
                self.machine_positions.insert(m.i, *m);
                return true;
            }
            false
        });
    }

    fn admit(
        &mut self,
        frame: &mut Frame,
        now: Instant,
        machine_baseline: bool,
        batch_frames: bool,
    ) -> bool {
        self.observe_travel(frame, machine_baseline);
        self.admit_live(frame, now, batch_frames)
    }

    /// Throttles the wall-clock fan-out, carrying actions and hits forward.
    fn admit_live(&mut self, frame: &mut Frame, now: Instant, batch_frames: bool) -> bool {
        let burst = FRAME_INTERVAL * (live_proto::MAX_INGEST_BATCH_FRAMES as u32 - 1);
        // All publishers of this game spend the same budget. A batched
        // stream can use its accumulated credit in a single message.
        if self.next_frame_credit > now + burst
            || (!batch_frames
                && self
                    .last_sent
                    .is_some_and(|last| now.duration_since(last) < FRAME_INTERVAL))
        {
            self.unsent.add(frame);
            return false;
        }
        self.last_sent = Some(now);
        self.next_frame_credit = self.next_frame_credit.max(now) + FRAME_INTERVAL;
        frame.absorb(std::mem::take(&mut self.unsent));
        true
    }

    /// Adds a sent frame to the recording; the chunk it closed, if any. A
    /// chunk's first frame lists every machine seen so far.
    fn record(&mut self, frame: &Frame) -> Option<Chunk> {
        let first = self.chunks.starts_chunk(frame).then(|| {
            let mut first = frame.clone();
            first.machines = self.machines.values().cloned().collect();
            first
        });
        self.chunks.push_with_machine_baseline(
            first.as_ref().unwrap_or(frame),
            Some(&self.machine_positions),
            std::mem::take(&mut self.record_machine_reset),
        )
    }
}

/// What a frame turned into.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Fanned out and added to the recording; `recorded` when it closed a
    /// chunk, which was stored.
    Sent { recorded: bool },
    /// Dropped: exceeded the frame rate budget.
    Throttled,
}

/// A recorded frame's packed bytes (packing an old JSON recording now);
/// `None` when it holds neither.
pub fn recorded_packed(data: &Value) -> Option<Vec<u8>> {
    match data.get("packed").and_then(Value::as_str) {
        Some(text) => pack::from_base64(text).ok(),
        None => {
            let frame: Frame = serde_json::from_value(data.clone()).ok()?;
            pack::encode_keyframe(&frame).ok()
        }
    }
}

fn random_game_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the OS random source failed");
    sign::hex(&b)
}

/// Ended games whose detail `GET /v1/snapshot` includes.
pub const SNAPSHOT_DETAILS: usize = 40;
/// How many log positions back `GET /v1/snapshot`'s events reach.
pub const SNAPSHOT_TAIL: u64 = 300;
/// The most accounts `GET /v1/snapshot` lists.
const SNAPSHOT_LIST: usize = 1000;

/// Which map `GET /v1/games` keeps (docs/live/live-api.md).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameFilter {
    /// The map's name: games on the base map and every mod of it.
    pub map: Option<String>,
    /// `Some(None)`: only games with no `MapId`; `Some(Some(id))`: that map.
    pub map_id: Option<Option<live_proto::maps::MapId>>,
    /// Only games that had every one of these skulls on (`?skulls=4,6`).
    pub skulls: Vec<u8>,
}

impl GameFilter {
    pub fn matches(&self, g: &live_proto::game::NewGame) -> bool {
        let on = g.skulls.as_deref().unwrap_or_default();
        self.map
            .as_ref()
            .is_none_or(|m| g.map.eq_ignore_ascii_case(m))
            && self.map_id.is_none_or(|id| g.map_id == id)
            && self.skulls.iter().all(|s| on.contains(s))
    }
}

/// `?skulls=` as `GET /v1/games` takes it: comma-separated skull indices.
pub fn parse_skulls(v: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part.parse::<u8>() {
            Ok(n) if n < live_proto::skulls::LIMIT => out.push(n),
            _ => {
                return Err(format!(
                    "skulls is a comma-separated list of 0 to {}",
                    live_proto::skulls::LIMIT - 1
                ));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

impl Live {
    /// The live game `id`, if `author` may write to it.
    fn writable_game(&self, id: &str, author: &Publisher, frame: bool) -> Result<Game, ApiError> {
        self.writable_game_in(&self.model.read(), id, author, frame).cloned()
    }

    fn writable_game_in<'a>(
        &self,
        s: &'a crate::model::State,
        id: &str,
        author: &Publisher,
        frame: bool,
    ) -> Result<&'a Game, ApiError> {
        let g = s.games.get(id).ok_or_else(|| not_found("no such game"))?;
        if g.started.game.sim.is_some()
            && author.peer().is_some_and(|a| !self.admins.is_admin(a, s))
        {
            return Err(forbidden("simulation recording requires an admin"));
        }
        if g.ended.is_some() {
            return Err(conflict("the game has ended"));
        }
        // Even a participant writing to its own secondary record cannot
        // publish a competing frame stream for a dedicated session.
        if frame && author.peer().is_some()
            && g.started.game.session_id.as_ref().and_then(|id| s.sessions.get(id))
                .and_then(|s| s.matchmaking.as_ref()).is_some_and(|a| a.host_lease.is_some())
        {
            return Err(forbidden("the assigned host supplies this session's frames"));
        }
        if !author.may_write(g) {
            return Err(forbidden("only the game's host and players write to it"));
        }
        Ok(g)
    }

    /// `POST /v1/games`: the server writes `GameStarted` with a fresh id
    /// and `host` = the token's author.
    pub async fn start_game(
        &self,
        bearer: Option<&str>,
        mut new: NewGame,
    ) -> Result<GameCreated, ApiError> {
        let (author, grant, _) = self.publisher(bearer, TELEMETRY_INGEST)?;
        if let Some(grant) = &grant {
            grant.require_active(self)?;
            grant.bind_game(&self.model.read(), &mut new)?;
        }
        self.rate_limit(&author, lxm::GAMES_WRITE)?;
        let _sim_guard = if let Some(sim) = &new.sim {
            if let Some(peer) = author.peer() {
                crate::admin::require_admin(self, peer)?;
                crate::sim_runs::validate_start(sim, &new)?;
            } else {
                crate::sim_runs::validate_hosted_start(sim, &new)?;
            }
            Some(self.sim_writes.lock().await)
        } else {
            None
        };
        if new.playlist.is_none() {
            new.playlist = self.playlist_name(new.playlist_id.as_deref()).await;
        }
        game::validate_new(&new).map_err(invalid)?;
        // Bots (docs/game/bots.md §9.3): the session's roster as it is now,
        // which is the one in force when GameStarted lands.
        author.check_players(
            &self.model.read(),
            author.xuid(),
            new.session_id.as_deref(),
            u64::MAX,
            &new.players,
        )
        .map_err(invalid)?;
        // dh-identity: one game per lease. Sim recordings keep their existing
        // stable run id. A retried identical start returns the original game.
        let stable_id = new
            .sim
            .as_ref()
            .map(|sim| game::sim_game_id(&sim.run_id))
            .or_else(|| author.host_lease_id().map(str::to_owned));
        if let Some(id) = &stable_id
            && let Some(g) = self.model.read().games.get(id)
        {
            return if g.started.host == author.to_string() && g.started.game == new {
                Ok(GameCreated {
                    game_id: id.clone(),
                    position: g.position,
                })
            } else {
                Err(conflict("that game id is already in use"))
            };
        }
        let game_id = stable_id.unwrap_or_else(random_game_id);
        let host = author.to_string();
        let started = GameStarted {
            game_id: game_id.clone(),
            dedicated: self.dedicated_host_fields(&host),
            host,
            game: new,
        };
        let stream = game::stream(&game_id);
        let event = self.server_event(
            stream.clone(),
            game::GAME_STARTED,
            serde_json::to_value(&started).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[(stream, 0)]).await?;
        let position = recorded[0].position;
        self.wait_applied(position).await?;
        Ok(GameCreated { game_id, position })
    }

    /// `POST /v1/games/{id}/events` (after [`Live::authenticate`]).
    pub async fn append_game_events(
        &self,
        bearer: Option<&str>,
        game_id: &str,
        jws: &str,
    ) -> Result<u64, ApiError> {
        let (author, _, _) = self.publisher(bearer, TELEMETRY_INGEST)?;
        self.append_publisher_batch(&author, game_id, jws).await
    }

    /// One batch JWS of game events by `author`, verified once, appended in
    /// order. Returns the last event's position.
    pub async fn append_game_batch(
        &self,
        author: &Author,
        game_id: &str,
        jws: &str,
    ) -> Result<u64, ApiError> {
        self.append_publisher_batch(&Publisher::Peer(author.clone()), game_id, jws)
            .await
    }

    pub(crate) async fn append_publisher_batch(
        &self,
        author: &Publisher,
        game_id: &str,
        jws: &str,
    ) -> Result<u64, ApiError> {
        self.rate_limit(author, lxm::GAMES_WRITE)?;
        let (parsed, bodies) = SignedEvent::open_batch(jws).map_err(sign_error)?;
        let first = &bodies[0];
        if first.author != author.to_string() {
            return Err(forbidden("the events' author is not the token's issuer"));
        }
        let key = author
            .key(self, &first.kid)
            .ok_or_else(|| unauthorized("the events' key is unknown or revoked"))?;
        parsed.verify(&key).map_err(sign_error)?;
        // A film can finish an interrupted telemetry stream. Hold the same lock
        // while checking the roster and committing this host's live batch.
        let _film_guard = if author.host_lease_id().is_some() {
            Some(self.host_game_writes.lock(game_id).await)
        } else {
            None
        };
        let film_expect = if author.host_lease_id().is_some() {
            let model = self.model.read();
            if model.films.imports.contains_key(game_id) {
                return Err(conflict("the host film is finalising this game"));
            }
            vec![(
                crate::films::import_stream(game_id),
                model.films.import_version(game_id),
            )]
        } else {
            Vec::new()
        };
        let game = self.writable_game(game_id, author, false)?;
        let stream = game::stream(game_id);
        let now = now();
        // A replay is a conflict, whatever the game looks like now.
        if bodies.iter().any(|b| {
            self.model
                .nonces
                .contains(&nonce_key(&b.author, &b.nonce), now)
        }) {
            return Err(conflict("nonce already used"));
        }
        // Validate in order against the players as the batch changes them.
        // The host's record is trusted (docs/live/live-matchmaking.md
        // §8.4): it adds anyone and ends the game. A player only joins as
        // itself, and leaves rather than ending the game.
        let by_host = author.to_string() == game.started.host;
        let host = match Author::parse(&game.started.host) {
            Some(Author::Account { xuid, .. }) => Some(xuid),
            _ => None,
        };
        let mut players = game.players.clone();
        for (i, b) in bodies.iter().enumerate() {
            let at = parse_time(&b.at)
                .ok_or_else(|| invalid("at is not an RFC 3339 time"))?
                .unix_timestamp();
            if (at - now).abs() > MAX_EVENT_SKEW_S {
                return Err(invalid("at is more than 5 minutes from the server's clock"));
            }
            if b.stream != stream {
                return Err(forbidden(format!(
                    "event {i} is for {}, not {stream}",
                    b.stream
                )));
            }
            if !game::CLIENT_GAME_EVENTS.contains(&b.kind.as_str()) {
                return Err(forbidden(format!(
                    "{} is not a game event a console writes",
                    b.kind
                )));
            }
            game::validate(&b.kind, &b.data, &players)
                .map_err(|m| invalid(format!("event {i}: {m}")))?;
            match b.kind.as_str() {
                game::PLAYER_JOINED => {
                    let p: game::Player =
                        serde_json::from_value(b.data["player"].clone()).expect("validated");
                    let itself = author
                        .xuid()
                        .is_some_and(|x| p.xuid == Some(x) && !p.is_bot());
                    if !by_host && !itself {
                        return Err(forbidden(format!(
                            "event {i}: a player joins as itself; the host adds others"
                        )));
                    }
                    if game.started.game.sim.is_some() && (!p.is_bot() || p.xuid.is_some()) {
                        return Err(invalid("simulation players must be bots without an account ID"));
                    }
                    author.check_players(
                        &self.model.read(),
                        host,
                        game.started.game.session_id.as_deref(),
                        game.position,
                        [&p],
                    )
                    .map_err(|m| invalid(format!("event {i}: {m}")))?;
                    players.push(p);
                }
                game::PLAYER_LEFT => {
                    players.retain(|p| Some(p.index as u64) != b.data["index"].as_u64())
                }
                game::GAME_ENDED if !by_host => {
                    return Err(forbidden(format!(
                        "event {i}: only the host ends the game; a player leaves with PlayerLeft"
                    )));
                }
                game::GAME_ENDED if i + 1 != bodies.len() => {
                    return Err(invalid("GameEnded must be the batch's last event"));
                }
                _ => {}
            }
        }
        for b in &bodies {
            let at = parse_time(&b.at).map_or(now, |t| t.unix_timestamp());
            if !self.model.nonces.insert_new(
                nonce_key(&b.author, &b.nonce),
                at + MAX_EVENT_SKEW_S,
                now,
            ) {
                return Err(conflict("nonce already used"));
            }
        }
        // The batch's events share its JWS (one copy, not one an event).
        let jws: Arc<str> = jws.into();
        let mut events: Vec<NewEvent> = bodies
            .into_iter()
            .enumerate()
            .map(|(i, body)| NewEvent {
                jws: Arc::clone(&jws),
                jws_index: Some(i),
                body,
            })
            .collect();
        let ends = events_end(&events);
        if ends {
            // The last frames' chunk goes in before the end.
            self.flush_recording(game_id).await?;
            events.insert(events.len() - 1, self.server_event(stream, crate::replay_archive::REQUESTED, json!({})));
        }
        let recorded = self.store.append(events, &film_expect).await?;
        let last = recorded.last().map_or(0, |e| e.position);
        self.wait_applied(last).await?;
        if ends {
            if game.started.game.sim.is_some() {
                self.ingest.games.lock().unwrap().remove(game_id);
            } else {
                // A finished ranked game ranks its players (crate::ranks).
                self.rank_game(game_id).await?;
            }
        }
        Ok(last)
    }

    /// A telemetry frame with the single-frame rate limit: fanned out to
    /// stream readers at up to 15 Hz per game and recorded in
    /// the game's frame chunks ([`live_proto::chunk`]): a chunk is stored
    /// when a frame lands in the next ten seconds, or the game ends.
    pub async fn ingest_frame(
        &self,
        author: &Author,
        frame: Frame,
    ) -> Result<FrameOutcome, ApiError> {
        self.ingest_frame_from_socket(&Publisher::Peer(author.clone()), frame, false, false)
            .await
    }

    pub(crate) async fn ingest_frame_from_socket(
        &self,
        author: &Publisher,
        mut frame: Frame,
        machine_baseline: bool,
        batch_frames: bool,
    ) -> Result<FrameOutcome, ApiError> {
        if !game::valid_id(&frame.game_id) {
            return Err(invalid("bad game_id"));
        }
        let game = self.writable_game(&frame.game_id, author, true)?;
        let sim = game.started.game.sim.as_ref();
        if sim.is_some()
            && frame
                .tick
                .saturating_sub(game.started.game.tick.unwrap_or(0))
                > crate::sim_runs::MAX_TICKS
        {
            return Err(ApiError::new(
                live_proto::ErrorCode::TooLarge,
                "simulation recording exceeds one in-game hour",
            ));
        }
        game::validate_frame(&frame).map_err(invalid)?;
        game::normalize_frame(&mut frame);
        let now_s = now();
        self.model.touch_game(&frame.game_id, now_s);
        let game_id = frame.game_id.clone();
        let clock = self.ingest.clock(&game_id);
        // Apply backpressure after a failed write. Ordinary live frames
        // never acquire the async storage lock while its queue is empty.
        let recorded = loop {
            {
                let mut state = clock.lock().unwrap();
                if state.writes.is_empty() {
                    state.observe_machines(&frame);
                    let now = Instant::now();
                    let closed = if let Some(sim) = sim {
                        // Record on game time. Wall time only limits live fan-out.
                        if state
                            .sim_tick
                            .is_some_and(|tick| frame.tick < tick.saturating_add(2))
                        {
                            return Ok(FrameOutcome::Throttled);
                        }
                        state.sim_tick = Some(frame.tick);
                        state.observe_travel(&mut frame, machine_baseline);
                        let chunk = if game.sim_recording_limited {
                            None
                        } else {
                            state.record(&frame)
                        };
                        if !sim.deferred && state.admit_live(&mut frame, now, false) {
                            let _ = self.frames.send(state.publish_versions(
                                frame,
                                self.packed.versions(&game_id),
                                &self.ingest.next_sid,
                            ));
                        }
                        chunk
                    } else {
                        if !state.admit(&mut frame, now, machine_baseline, batch_frames) {
                            return Ok(FrameOutcome::Throttled);
                        }
                        let closed = state.record(&frame);
                        // Sent under the game's lock: its frames go out in order.
                        let _ = self.frames.send(state.publish_versions(
                            frame,
                            self.packed.versions(&game_id),
                            &self.ingest.next_sid,
                        ));
                        closed
                    };
                    if let Some(c) = closed {
                        state.writes.push_back((c.meta, c.bytes.into()));
                        break true;
                    }
                    break false;
                }
            }
            self.write_pending(&game_id, &clock).await?;
        };
        if recorded {
            self.write_pending(&game_id, &clock).await?;
        }
        Ok(FrameOutcome::Sent { recorded })
    }

    pub(crate) fn ingest_sim_combat(
        &self,
        author: &Publisher,
        id: &str,
        sample: game::SimCombatSample,
    ) -> Result<(), ApiError> {
        if !game::valid_id(id) {
            return Err(invalid("bad game_id"));
        }
        // Hold the projection read guard through the insert so the end of
        // a run cannot drop its clock and have a late message recreate it.
        let state = self.model.read();
        let game = self.writable_game_in(&state, id, author, true)?;
        if author.to_string() != game.started.host {
            return Err(forbidden("only the run host supplies combat samples"));
        }
        if !game.started.game.sim.as_ref().is_some_and(|s| !s.deferred) {
            return Err(invalid("combat samples require a live simulation"));
        }
        if u64::from(sample.tick).saturating_sub(game.started.game.tick.unwrap_or(0))
            > crate::sim_runs::MAX_TICKS
        {
            return Err(invalid("combat sample exceeds the simulation tick limit"));
        }
        if !sample.valid() {
            return Err(invalid("invalid combat sample"));
        }
        if !game.players.iter().any(|p| p.index == u32::from(sample.index) && p.is_bot()) {
            return Err(invalid("unknown combat sample bot seat"));
        }
        let clock = self.ingest.clock(id);
        let mut clock = clock.lock().unwrap();
        if clock.sim_combat.get(&sample.index).is_none_or(|old| sample.tick > old.tick) {
            clock.sim_combat.insert(sample.index, sample);
        }
        Ok(())
    }

    pub(crate) fn sim_combat(&self, id: &str) -> Vec<game::SimCombatSample> {
        let clock = self.ingest.games.lock().unwrap().get(id).cloned();
        clock.map_or_else(Vec::new, |clock| {
            clock.lock().unwrap().sim_combat.values().cloned().collect()
        })
    }

    pub(crate) fn sim_tick(&self, id: &str) -> Option<u64> {
        let clock = self.ingest.games.lock().unwrap().get(id).cloned()?;
        clock.lock().unwrap().sim_tick
    }

    /// Flush explicitly without closing the game (also used by parity tests).
    pub async fn flush_frames(&self, id: &str) {
        if let Err(e) = self.flush_recording(id).await {
            eprintln!("live: frame chunk of {id}: {}", e.message);
        }
    }

    pub(crate) async fn flush_recording(&self, id: &str) -> Result<(), ApiError> {
        let clock = self.ingest.games.lock().unwrap().get(id).cloned();
        let Some(clock) = clock else { return Ok(()) };
        {
            let mut state = clock.lock().unwrap();
            if let Some(c) = state.chunks.finish() {
                state.writes.push_back((c.meta, c.bytes.into()));
            }
        }
        self.write_pending(id, &clock).await
    }

    async fn write_pending(&self, id: &str, clock: &Mutex<FrameClock>) -> Result<(), ApiError> {
        let writer = clock.lock().unwrap().write_lock.clone();
        let _guard = writer.lock().await;
        loop {
            let chunk = clock.lock().unwrap().writes.front().cloned();
            let Some((meta, bytes)) = chunk else { return Ok(()) };
            self.store_chunk(id, &meta, bytes).await?;
            clock.lock().unwrap().writes.pop_front();
        }
    }

    /// Puts a chunk's bytes in the private blob store (R2 on the Worker,
    /// the data directory natively; the event store's blobs on a server
    /// with neither), then its index entry in the log as a server-signed
    /// `FrameChunk`.
    async fn store_chunk(&self, id: &str, chunk: &ChunkMeta, bytes: Bytes) -> Result<(), ApiError> {
        let sim = crate::sim_runs::is_sim_id(id);
        let _guard = if sim {
            // Serialize the archive quota with its accounting event. Both
            // come back from the log after a Worker wake.
            let guard = self.sim_writes.lock().await;
            let (bytes, limited) = self.model.read().games.get(id).map_or((0, false), |g| {
                (g.sim_recorded_bytes, g.sim_recording_limited)
            });
            if limited {
                return Ok(());
            }
            if bytes.saturating_add(chunk.bytes) > crate::sim_runs::MAX_ARCHIVE_BYTES {
                let event = self.server_event(
                    game::stream(id),
                    crate::sim_runs::RECORDING_LIMITED,
                    json!({}),
                );
                let stored = self.store.append(vec![event], &[]).await?;
                self.wait_applied(stored[0].position).await?;
                return Ok(());
            }
            Some(guard)
        } else {
            None
        };
        match self.blobs() {
            Some(b) => {
                let meta = crate::blobs::Meta {
                    content_type: Some(CHUNK_TYPE.into()),
                    content_encoding: None,
                    cache_control: None,
                };
                b.private
                    .put(&format!("{}-{}", chunk_key(id, chunk.from_tick), chunk.sha256), bytes, &meta)
                    .await
                    .map_err(|e| {
                        eprintln!("live: frame chunk of {id}: {e}");
                        ApiError::new(
                            live_proto::ErrorCode::Unavailable,
                            "the blob store is unavailable",
                        )
                    })?;
            }
            None => self.store.put_blob(&chunk.sha256, &bytes).await?,
        }
        let data = serde_json::to_value(chunk).expect("serializes");
        let event = self.server_event(game::stream(id), chunk::FRAME_CHUNK, data);
        let stored = self.store.append(vec![event], &[]).await?;
        if sim {
            self.wait_applied(stored[0].position).await?;
        }
        Ok(())
    }

    /// `GET /v1/games/{id}/chunks`: the game's frame chunk index, oldest
    /// first: each `FrameChunk` event's data. Empty for a game recorded
    /// before chunks (its frames are `GameFrame`s, read with
    /// [`Live::recorded_frame_data`]).
    pub async fn frame_chunks(&self, id: &str) -> Result<Vec<ChunkMeta>, ApiError> {
        let mut out = self.source_chunks(id).await?;
        out.sort_by_key(|m| m.from_tick);
        Ok(out)
    }

    pub(crate) async fn source_chunks(&self, id: &str) -> Result<Vec<ChunkMeta>, ApiError> {
        if !self.model.read().games.contains_key(id) {
            return Err(not_found("no such game"));
        }
        let mut query = Query {
            after: 0,
            limit: FRAMES_PAGE,
            stream: Some(game::stream(id)),
            types: vec![chunk::FRAME_CHUNK.into()],
            exclude: Vec::new(),
        };
        let mut out = Vec::new();
        loop {
            let batch = self.store.read(&query).await?;
            for e in &batch {
                match serde_json::from_value::<ChunkMeta>(e.body.data.clone()) {
                    Ok(meta) => out.push(meta),
                    Err(err) => eprintln!("live: frame chunk {} of {id}: {err}", e.position),
                }
            }
            match batch.last() {
                Some(last) if batch.len() == query.limit => query.after = last.position,
                _ => break,
            }
        }
        Ok(out)
    }

    /// `GET /v1/games/{id}/chunks/{from_tick}`: one chunk's bytes, checked
    /// against the hash its signed index entry holds.
    pub async fn frame_chunk(&self, id: &str, from_tick: u64) -> Result<Bytes, ApiError> {
        self.frame_chunk_named(id, from_tick, None).await
    }

    pub(crate) async fn frame_chunk_named(&self, id: &str, from_tick: u64, digest: Option<&str>) -> Result<Bytes, ApiError> {
        let index = self.frame_chunks(id).await?;
        let meta = index.iter().find(|m| m.from_tick == from_tick && digest.is_none_or(|hash| hash == m.sha256))
            .ok_or_else(|| not_found("no such chunk"))?;
        self.stored_chunk(id, meta).await
    }

    pub(crate) async fn stored_chunk(&self, id: &str, meta: &ChunkMeta) -> Result<Bytes, ApiError> {
        let from_tick = meta.from_tick;
        let mut bytes = None;
        let mut unavailable = None;
        if let Some(b) = self.blobs() {
            let old = chunk_key(id, from_tick);
            for key in [format!("{old}-{}", meta.sha256), old] {
                match b.private.get(&key).await {
                    Ok(Some(found)) => { bytes = Some(found); break; }
                    Ok(None) => {}
                    Err(e) => unavailable = Some(ApiError::new(live_proto::ErrorCode::Unavailable, e.to_string())),
                }
            }
        }
        if bytes.is_none() {
            bytes = self.store.blob(&meta.sha256).await?.map(Bytes::from);
        }
        let bytes = bytes.ok_or_else(|| unavailable.unwrap_or_else(|| not_found("the chunk is not stored")))?;
        if sign::hex(&Sha256::digest(&bytes)) != meta.sha256 {
            eprintln!("live: frame chunk {from_tick} of {id} does not match its hash");
            return Err(not_found("the chunk is not stored"));
        }
        Ok(bytes)
    }

    /// Every recorded frame of a game in chunks, decoded: for tests and
    /// the parity suite.
    pub async fn chunk_frames(&self, id: &str) -> Result<Vec<Frame>, ApiError> {
        let mut out = Vec::new();
        for meta in self.frame_chunks(id).await? {
            let bytes = self.stored_chunk(id, &meta).await?;
            out.extend(chunk::frames(&bytes).map_err(|e| invalid(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn frames(&self) -> tokio::sync::broadcast::Receiver<Arc<LiveFrame>> {
        self.frames.subscribe()
    }

    /// Takes every game's packed frames since the last tick and publishes
    /// them to packed readers ([`crate::packed`]); with none, drops them.
    pub fn publish_packed(&self) {
        let clocks: Vec<_> = {
            let all = self.ingest.games.lock().unwrap();
            all.iter()
                .map(|(id, clock)| (id.clone(), Arc::clone(clock)))
                .collect()
        };
        let readers = self.packed.has_readers();
        let mut batches = Vec::new();
        let mut causal_batches = Vec::new();
        for (id, clock) in clocks {
            let mut clock = clock.lock().unwrap();
            let sid = clock.pack.sid();
            if let Some(b) = clock.pending.take(&id, sid)
                && readers
            {
                batches.push(b);
            }
            if let Some(stream) = &mut clock.causal
                && let Some(mut b) = stream.pending.take(&id, stream.encoder.sid())
                && readers
            {
                b.version = pack::LIVE_VERSION;
                causal_batches.push(b);
            }
        }
        if !batches.is_empty() {
            self.packed.publish(batches);
        }
        if !causal_batches.is_empty() {
            self.packed.publish(causal_batches);
        }
    }

    /// Whether a live game has an ingest clock, so packed frames may come
    /// and [`Live::publish_packed`] has a tick to run.
    pub fn packed_active(&self) -> bool {
        !self.ingest.games.lock().unwrap().is_empty()
    }

    /// The packed fan-out, for stream readers.
    pub fn packed(&self) -> &crate::packed::Hub {
        &self.packed
    }

    pub(crate) fn join_packed(
        &self,
        games: &[String],
        v8: bool,
        viewer: crate::sessions::Viewer,
        follow: &mut crate::packed::Follow,
        out: &mut Vec<u8>,
    ) {
        let clocks: Vec<_> = self
            .ingest
            .games
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| {
                (games.is_empty() && !crate::sim_runs::is_sim_id(id)) || games.contains(id)
            })
            .map(|(id, clock)| (id.clone(), Arc::clone(clock)))
            .collect();
        let visible = self.games_show(clocks.iter().map(|(id, _)| id.as_str()), viewer);
        for ((_, clock), show) in clocks.into_iter().zip(visible) {
            if show {
                clock
                    .lock()
                    .unwrap()
                    .join(v8, &self.ingest.next_sid, follow, out);
            }
        }
    }

    /// Each live game's packed frames since its last keyframe (all games,
    /// or those in `games`): what a packed reader that joins or fell
    /// behind is sent first.
    pub fn frames_since_keyframe(&self, games: &[String]) -> Vec<Arc<LiveFrame>> {
        let clocks: Vec<_> = {
            let all = self.ingest.games.lock().unwrap();
            all.iter()
                .filter(|(id, _)| games.is_empty() || games.contains(id))
                .map(|(_, clock)| Arc::clone(clock))
                .collect()
        };
        clocks
            .iter()
            .flat_map(|clock| clock.lock().unwrap().since_key.clone())
            .collect()
    }

    /// `GET /v1/games`: newest first; the cursor is a start position.
    pub fn games(
        &self,
        state: Option<&str>,
        before: Option<u64>,
        limit: usize,
    ) -> Page<GameSummary> {
        self.games_where(
            state,
            &GameFilter::default(),
            crate::sessions::Viewer::Admin,
            before,
            limit,
        )
    }

    /// [`Live::games`] narrowed to one map: by name (`?map=`, every game on
    /// it, modded or not) and by `MapId` (`?map_id=`: one exact map, or
    /// `none` for games that name no `MapId`, the retail ones).
    /// `GET /v1/snapshot`: what the web app reads on load, in one round
    /// trip instead of a list each and a detail a game. The accounts, the
    /// presence `viewer` may see, the live and last ended games (a hundred
    /// each), the detail of every live game and the [`SNAPSHOT_DETAILS`]
    /// newest ended ones, and the log's last events before `position`
    /// (the 1 Hz `GameFrame`s left out, as the stream leaves them). Each
    /// part is as its own endpoint would answer; `position` is the oldest
    /// of theirs, where the reader's stream starts.
    pub async fn snapshot(
        &self,
        viewer: crate::sessions::Viewer,
    ) -> Result<serde_json::Value, ApiError> {
        let accounts = self.accounts(0, SNAPSHOT_LIST);
        let presence = self.presence(viewer);
        let any = GameFilter::default();
        let live = self.games_where(Some("live"), &any, viewer, None, SNAPSHOT_LIST.min(100));
        let ended = self.games_where(Some("ended"), &any, viewer, None, SNAPSHOT_LIST.min(100));
        let details: Vec<GameDetail> = live
            .items
            .iter()
            .chain(ended.items.iter().take(SNAPSHOT_DETAILS))
            .filter_map(|g| self.game(&g.game_id).ok())
            .collect();
        let position = [
            accounts.position,
            presence.position,
            live.position,
            ended.position,
        ]
        .into_iter()
        .chain(details.iter().map(|d| d.position))
        .min()
        .expect("not empty");
        let query = crate::store::Query {
            after: position.saturating_sub(SNAPSHOT_TAIL),
            limit: SNAPSHOT_TAIL as usize,
            exclude: vec![game::GAME_FRAME.to_string()],
            ..crate::store::Query::after(0, 0)
        };
        let mut tail = self.events_for(&query, viewer).await?.items;
        tail.retain(|e| e.position <= position);
        Ok(serde_json::json!({
            "position": position,
            "accounts": accounts,
            "presence": presence,
            "live": live,
            "ended": ended,
            "details": details,
            "tail": tail,
        }))
    }

    pub fn games_where(
        &self,
        state: Option<&str>,
        filter: &GameFilter,
        viewer: crate::sessions::Viewer,
        before: Option<u64>,
        limit: usize,
    ) -> Page<GameSummary> {
        let position = self.position();
        let s = self.model.read();
        let page: Vec<&Game> = s
            .games_newest()
            .filter(|g| g.started.game.sim.is_none())
            .filter(|g| before.is_none_or(|b| g.position < b))
            .filter(|g| filter.matches(&g.started.game))
            .filter(|g| g.shows_to(viewer, |a, b| s.is_friends(a, b)))
            .filter(|g| match state {
                Some("live") => g.ended.is_none(),
                Some("ended") => g.ended.is_some(),
                _ => true,
            })
            .take(limit)
            .collect();
        let next = (page.len() == limit)
            .then(|| page.last().map(|g| g.position.to_string()))
            .flatten();
        Page {
            items: page.iter().map(|g| self.game_summary(g)).collect(),
            next,
            position,
        }
    }

    /// `Err(not_found)` unless `viewer` may see game `id`
    /// ([`Game::shows_to`]): a hidden co-op game reads as no game at all.
    pub fn check_game_shows(
        &self,
        id: &str,
        viewer: crate::sessions::Viewer,
    ) -> Result<(), ApiError> {
        let s = self.model.read();
        match s.games.get(id) {
            Some(g) if !g.shows_to(viewer, |a, b| s.is_friends(a, b)) => {
                Err(not_found("no such game"))
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn game_summary(&self, game: &Game) -> GameSummary {
        let mut summary = game.summary();
        // Older GameStarted events can still use the lease in their host subject.
        if summary.dedicated.host_lease.is_none() {
            summary.dedicated = self.dedicated_host_fields(&game.started.host);
        }
        summary
    }

    pub fn game(&self, id: &str) -> Result<GameDetail, ApiError> {
        let position = self.position();
        let s = self.model.read();
        let g = s.games.get(id).ok_or_else(|| not_found("no such game"))?;
        let n = &g.started.game;
        Ok(GameDetail {
            stats_discrepancy: g.film_stats_discrepancy.clone(),
            summary: self.game_summary(g),
            console: n.console.clone(),
            host: g.started.host.clone(),
            score_limit: n.score_limit,
            time_limit_s: n.time_limit_s,
            tick: n.tick,
            last_tick: g.last_tick,
            scoreboard: g.scoreboard.clone(),
            kills: g.kills.iter().cloned().collect(),
            winner: g.ended.as_ref().and_then(|e| e.0.winner.clone()),
            end_reason: g.ended.as_ref().map(|e| e.0.reason.clone()),
            position,
        })
    }

    /// `GET /v1/games/{id}/frames`: a game's recorded `GameFrame`s with
    /// `from_tick <= tick <= to_tick`, oldest first (the cursor is a log
    /// position): each one's data, packed (`{tick, source, packed}`) or,
    /// for the oldest games, the JSON frame. Games recorded before frame
    /// chunks have these; newer ones have [`Live::frame_chunks`].
    pub async fn recorded_frame_data(
        &self,
        id: &str,
        from_tick: Option<u64>,
        to_tick: Option<u64>,
        after: u64,
        limit: usize,
    ) -> Result<Page<Value>, ApiError> {
        let position = self.position();
        if !self.model.read().games.contains_key(id) {
            return Err(not_found("no such game"));
        }
        let limit = limit.min(FRAMES_PAGE);
        let mut query = Query {
            after,
            limit: FRAMES_PAGE,
            stream: Some(game::stream(id)),
            types: vec![game::GAME_FRAME.into()],
            exclude: Vec::new(),
        };
        let mut items = Vec::new();
        let mut last = after;
        loop {
            let batch = self.store.read(&query).await?;
            for e in &batch {
                let tick = e.body.data.get("tick").and_then(Value::as_u64).unwrap_or(0);
                if to_tick.is_some_and(|t| tick > t) {
                    return Ok(Page {
                        items,
                        next: None,
                        position,
                    });
                }
                last = e.position;
                if from_tick.is_none_or(|f| tick >= f) {
                    items.push(e.body.data.clone());
                    if items.len() == limit {
                        return Ok(Page {
                            items,
                            next: Some(last.to_string()),
                            position,
                        });
                    }
                }
            }
            if batch.len() < query.limit {
                return Ok(Page {
                    items,
                    next: None,
                    position,
                });
            }
            query.after = last;
        }
    }

    /// `PUT /v1/maps/{map}/outline`: admin-managed geometry shared by every
    /// game on the map.
    pub async fn put_outline(
        &self,
        bearer: Option<&str>,
        map: &str,
        bytes: &[u8],
    ) -> Result<Value, ApiError> {
        let author = self.authenticate(bearer, lxm::GAMES_WRITE)?;
        self.rate_limit(&author, lxm::GAMES_WRITE)?;
        if !self.admins.is_admin(&author, &self.model.read()) {
            return Err(forbidden("only an admin may upload a map outline"));
        }
        if !game::valid_id(map) {
            return Err(invalid("map must be [A-Za-z0-9_-]{1,64}"));
        }
        let outline: Outline =
            serde_json::from_slice(bytes).map_err(|e| invalid(format!("outline: {e}")))?;
        if outline.map != map {
            return Err(invalid("the outline's map is not the path's"));
        }
        let sha256 = sign::hex(&Sha256::digest(bytes));
        self.store.put_blob(&sha256, bytes).await?;
        let data = json!({"map": map, "sha256": sha256, "size": bytes.len(), "source": author.to_string()});
        let event = self.server_event(
            game::map_stream(map),
            game::MAP_OUTLINE_STORED,
            data.clone(),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(data)
    }

    pub async fn outline(&self, map: &str) -> Result<Vec<u8>, ApiError> {
        let sha = self
            .model
            .read()
            .outlines
            .get(map)
            .map(|o| o.sha256.clone());
        let sha = sha.ok_or_else(|| not_found("no outline for this map yet"))?;
        self.store
            .blob(&sha)
            .await?
            .ok_or_else(|| not_found("outline bytes are missing"))
    }

    /// When [`Live::sweep`] next has something to do (unix seconds): the
    /// earliest presence to expire, game to end as lost or co-op session
    /// to expire; `None` with nothing to watch.
    pub fn sweep_due(&self) -> Option<i64> {
        let policy = self.policy();
        let s = self.model.read();
        // The sweep acts once a deadline is strictly behind it.
        let presence = s
            .presence
            .values()
            .map(|p| p.last_seen + policy.presence_ttl_s);
        let games = s.games.values().filter(|g| g.ended.is_none() && !s.films.ingesting(&g.started.game_id)).map(|g| {
            g.last_activity
                + crate::sim_runs::idle_seconds(g.started.game.sim.as_ref(), policy.game_idle_s)
        });
        let sessions = s
            .sessions
            .values()
            .filter(|v| v.state == "open")
            .map(|v| v.last_activity + policy.session_idle_s);
        let archive = s.games.values().filter_map(|g| g.recording.due(g.ended.is_some())).min();
        presence.chain(games).chain(sessions).min().map(|d| d + 1)
            .into_iter().chain(archive).chain(s.films.film_work_due()).min()
    }

    /// The managers: expire quiet presence and end idle games, as the
    /// server. Deadlines come from the read model, so they survive a
    /// restart (rebuilt from the events' `at`).
    pub async fn sweep(&self) {
        let now = now();
        crate::films::continue_staged(self).await;
        let cleanup = self.model.read().films.next_cleanup();
        if let Some(id) = cleanup {
            crate::films::cleanup_published(self, &id).await;
        }
        self.flush_usage().await;
        self.sweep_sessions(now).await;
        crate::replay_archive::advance(self, now).await;
        let import = self.model.read().films.next_import();
        if let Some(id) = import {
            match crate::films::resume_import(self, &id, 1).await {
                Ok(true) => crate::films::cleanup_published(self, &id).await,
                Ok(false) => {}
                Err(e) => eprintln!("live: resuming film stats for {id}: {}", e.message),
            }
        }
        let policy = self.policy();
        let (expired, idle): (Vec<_>, Vec<_>) = {
            let s = self.model.read();
            let expired = s
                .presence
                .iter()
                .filter(|(_, p)| p.last_seen + policy.presence_ttl_s < now)
                .map(|(x, _)| *x)
                .collect();
            let idle = s
                .games
                .values()
                .filter(|g| {
                    g.ended.is_none() && !s.films.ingesting(&g.started.game_id)
                        && g.last_activity
                            + crate::sim_runs::idle_seconds(
                                g.started.game.sim.as_ref(),
                                policy.game_idle_s,
                            )
                            < now
                })
                .map(|g| {
                    (
                        g.started.game_id.clone(),
                        g.last_tick.unwrap_or(0),
                        g.scoreboard.clone(),
                        s.films.import_version(&g.started.game_id),
                    )
                })
                .collect();
            (expired, idle)
        };
        let mut expect = Vec::new();
        let mut events = Vec::new();
        let mut finished_games = Vec::new();
        for xuid in expired {
            events.push(self.server_event(
                streams::presence(xuid),
                types::PRESENCE_EXPIRED,
                json!({"xuid": xuid}),
            ));
        }
        for (id, tick, players, import_version) in idle {
            if let Err(e) = self.flush_recording(&id).await {
                eprintln!("live: retrying the last chunk of {id}: {}", e.message);
                continue;
            }
            expect.push((crate::films::import_stream(&id), import_version));
            let ended = GameEnded {
                tick,
                reason: "lost".into(),
                winner: None,
                players,
            };
            events.push(self.server_event(game::stream(&id), crate::replay_archive::REQUESTED, json!({})));
            events.push(self.server_event(
                game::stream(&id),
                game::GAME_ENDED,
                serde_json::to_value(ended).expect("serializes"),
            ));
            finished_games.push(id);
        }
        if events.is_empty() {
            return;
        }
        match self.store.append(events, &expect).await {
            Ok(recorded) => {
                // Wait, so the next sweep does not write them twice.
                if self.wait_applied(recorded.last().map_or(0, |e| e.position)).await.is_ok() {
                    let mut clocks = self.ingest.games.lock().unwrap();
                    for id in finished_games {
                        clocks.remove(&id);
                    }
                }
            }
            Err(e) => eprintln!("live: sweep: {e}"),
        }
    }
}

fn events_end(events: &[NewEvent]) -> bool {
    events
        .last()
        .is_some_and(|e| e.body.kind == game::GAME_ENDED)
}

/// An ingest refusal as the socket reports it.
pub fn reply_error(e: &ApiError) -> live_proto::IngestReply {
    live_proto::IngestReply::Error {
        error: e.code,
        message: e.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::game::{FrameActor, FramePlayer};

    fn frame(player_shots: u32, actor_shots: u32) -> Frame {
        Frame {
            game_id: "g".into(),
            players: vec![FramePlayer {
                index: 0,
                alive: true,
                shots: (player_shots > 0).then_some(player_shots),
                ..FramePlayer::default()
            }],
            actors: vec![FrameActor {
                id: "90070007".into(),
                species: "grunt".into(),
                shots: (actor_shots > 0).then_some(actor_shots),
                ..FrameActor::default()
            }],
            ..Frame::default()
        }
    }

    fn shots(frame: &Frame) -> (Option<u32>, Option<u32>) {
        (frame.players[0].shots, frame.actors[0].shots)
    }

    #[test]
    fn sent_frames_carry_what_throttled_frames_had() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        let at = |ms: u64| start + Duration::from_millis(ms);

        let mut f = frame(1, 0);
        assert!(clock.admit(&mut f, at(0), false, false));
        assert_eq!(shots(&f), (Some(1), None));
        let mut f = frame(4, 0);
        assert!(!clock.admit(&mut f, at(20), false, false), "throttled");
        let mut f = frame(0, 3);
        assert!(clock.admit(&mut f, at(100), false, false));
        assert_eq!(shots(&f), (Some(4), Some(3)), "carries the throttled frame");
    }

    #[test]
    fn batched_frames_can_spend_at_most_one_burst_of_credit() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        for _ in 0..live_proto::MAX_INGEST_BATCH_FRAMES {
            assert!(clock.admit(&mut frame(0, 0), start, false, true));
        }
        assert!(!clock.admit(&mut frame(3, 0), start, false, true));
        // A legacy frame spends the same credit that a batch would use.
        let next = start + FRAME_INTERVAL;
        assert!(clock.admit(&mut frame(0, 0), next, false, false));
        let mut f = frame(0, 0);
        assert!(!clock.admit(&mut f, next, false, true));
        assert!(clock.admit(&mut f, next + FRAME_INTERVAL, false, true));
    }

    fn machine(id: &str, position: f64) -> live_proto::game::FrameMachine {
        live_proto::game::FrameMachine {
            node: None,
            id: id.into(),
            kind: "door".into(),
            pos: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            position: Some(position),
        }
    }

    #[test]
    fn every_sent_frame_is_recorded_and_a_chunks_first_frame_lists_every_machine() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut chunks = Vec::new();
        let mut send = |clock: &mut FrameClock, mut f: Frame, ms: u64| {
            clock.observe_machines(&f);
            if clock.admit(&mut f, at(ms), false, false) {
                chunks.extend(clock.record(&f));
            }
        };

        // Window 0: both doors in the first frame, door a moving after.
        let mut f = frame(0, 0);
        f.tick = 10;
        f.machines = vec![machine("a", 0.0), machine("b", 0.0)];
        send(&mut clock, f, 0);
        let mut f = frame(0, 0);
        f.tick = 12;
        f.machines = vec![machine("a", 1.0)];
        send(&mut clock, f, 100);
        // Throttled: its shots go to the next sent frame.
        let mut f = frame(2, 0);
        f.tick = 14;
        send(&mut clock, f, 120);
        let mut f = frame(0, 0);
        f.tick = 16;
        send(&mut clock, f, 200);
        // The next window: closes the first chunk; its first frame lists
        // both doors though neither moved in it.
        let mut f = frame(0, 0);
        f.tick = chunk::CHUNK_TICKS + 2;
        send(&mut clock, f, 300);
        assert_eq!(chunks.len(), 1);
        let first = chunk::frames(&chunks[0].bytes).unwrap();
        assert_eq!(first.iter().map(|f| f.tick).collect::<Vec<_>>(), [10, 12, 16]);
        assert_eq!(first[2].players[0].shots, Some(2));
        assert_eq!(chunks[0].meta.shots, 2);
        let next = clock.chunks.finish().unwrap();
        let next = chunk::frames(&next.bytes).unwrap();
        let machines: Vec<_> = next[0]
            .machines
            .iter()
            .map(|m| (m.id.as_str(), m.position))
            .collect();
        // Device positions travel in 1023 steps: 0 and 1 are exact.
        assert_eq!(machines, [("a", Some(1.0)), ("b", Some(0.0))]);
    }

    #[test]
    fn machine_positions_survive_throttling_keyframes_chunks_and_cuts() {
        use game::MachinePosition;

        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        clock.pack = pack::Encoder::new(0, 3);
        let mut decoder = pack::Decoder::new();
        let device = |i, p| MachinePosition { i, p };
        let send = |clock: &mut FrameClock, tick, ms, positions: &[MachinePosition], cut| {
            let mut f = frame(0, 0);
            f.tick = tick;
            f.cut = cut;
            f.machine_positions.extend_from_slice(positions);
            if !clock.admit(&mut f, start + Duration::from_millis(ms), false, false) {
                return None;
            }
            let _ = clock.record(&f);
            Some(clock.publish(f))
        };
        let mut decode = |f: Arc<LiveFrame>| decoder.decode(f.packed.as_ref().unwrap()).unwrap();

        let first = send(&mut clock, 10, 0, &[device(0, 0), device(1, 20)], None).unwrap();
        assert_eq!(
            decode(first).frame.machine_positions,
            [device(0, 0), device(1, 20)]
        );
        assert!(send(&mut clock, 12, 30, &[device(0, 128), device(1, 50)], None).is_none());
        let next = send(&mut clock, 14, 70, &[device(0, 255)], None).unwrap();
        assert_eq!(
            decode(next).frame.machine_positions,
            [device(0, 255), device(1, 50)]
        );
        let quiet = send(&mut clock, 16, 140, &[device(0, 255)], None).unwrap();
        assert!(decode(quiet).frame.machine_positions.is_empty());

        let key = send(&mut clock, chunk::CHUNK_TICKS + 2, 210, &[], None).unwrap();
        let from_key = pack::Decoder::new()
            .decode(key.packed.as_ref().unwrap())
            .unwrap();
        assert!(from_key.keyframe);
        assert_eq!(
            from_key.frame.machine_positions,
            [device(0, 255), device(1, 50)]
        );
        let chunk = clock.chunks.finish().unwrap();
        assert_eq!(
            chunk::frames(&chunk.bytes).unwrap()[0].machine_positions,
            from_key.frame.machine_positions
        );

        let cut_tick = chunk::CHUNK_TICKS + 4;
        assert!(send(&mut clock, cut_tick, 240, &[device(0, 10)], Some(4)).is_none());
        let after_cut = send(&mut clock, cut_tick + 2, 280, &[], None).unwrap();
        assert_eq!(after_cut.frame.cut, Some(4));
        assert_eq!(after_cut.frame.machine_positions, [device(0, 10)]);
    }

    #[test]
    fn cold_subscriber_joins_use_the_published_machine_state_for_each_version() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        let next_sid = std::sync::atomic::AtomicU64::new(1);
        let mut f = frame(0, 0);
        f.tick = 10;
        let at = |p| game::MachinePosition { i: 9, p };
        f.machine_positions.push(at(20));
        assert!(clock.admit(&mut f, start, true, false));
        clock.publish_versions(f, [false, true], &next_sid);
        assert_eq!(clock.pack.next_seq(), 0, "v8 alone only needs its own writer");

        // The join is at tick 10 while ingest already knows about tick 11.
        let mut throttled = frame(0, 0);
        throttled.tick = 11;
        throttled.machine_positions.push(at(40));
        assert!(!clock.admit(&mut throttled, start + Duration::from_millis(5), false, false));
        for v8 in [false, true] {
            let mut out = Vec::new();
            let mut follow = crate::packed::Follow::default();
            clock.join(v8, &next_sid, &mut follow, &mut out);
            let records: Vec<_> = pack::records(&out).collect::<Result<_, _>>().unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0][0] & 15, if v8 { 8 } else { 7 });
            let key = pack::Decoder::new().decode(records[0]).unwrap();
            assert!(key.machine_baseline);
            assert_eq!(key.frame.tick, 10);
            assert_eq!(key.frame.machine_positions, [at(20)]);
            out.clear();
            clock.join(v8, &next_sid, &mut follow, &mut out);
            assert!(out.is_empty());
        }
    }

    #[test]
    fn machine_positions_empty_ingest_baseline_clears_recorded_and_live_state() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        let mut first = frame(0, 0);
        first.tick = 10;
        first.machine_positions = vec![game::MachinePosition { i: 7, p: 200 }];
        assert!(clock.admit(&mut first, start, true, false));
        assert!(clock.record(&first).is_none());
        clock.publish(first);

        let mut throttled = frame(0, 0);
        throttled.tick = 12;
        throttled.machine_positions = vec![game::MachinePosition { i: 8, p: 128 }];
        assert!(!clock.admit(&mut throttled, start + Duration::from_millis(10), false, false));

        let mut reset = frame(0, 0);
        reset.tick = 14;
        assert!(clock.admit(&mut reset, start + Duration::from_millis(70), true, false));
        assert!(reset.machine_positions.is_empty());
        assert!(clock.record(&reset).is_none());
        let live = clock.publish(reset);
        let d = pack::Decoder::new()
            .decode(live.packed.as_ref().unwrap())
            .unwrap();
        assert!(d.machine_baseline);
        assert!(d.frame.machine_positions.is_empty());

        let chunk = clock.chunks.finish().unwrap();
        let container = chunk::container(&chunk.bytes).unwrap();
        let last = pack::records(&container).last().unwrap().unwrap();
        let d = pack::Decoder::new().decode(last).unwrap();
        assert!(d.machine_baseline);
        assert!(d.frame.machine_positions.is_empty());
    }

    #[test]
    fn machine_positions_baseline_keeps_more_devices_than_world_poses() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        let mut f = frame(0, 0);
        f.machine_positions = (0..195)
            .map(|i| game::MachinePosition { i, p: 127 })
            .collect();
        let expected = f.machine_positions.clone();
        assert!(clock.admit(&mut f, start, false, false));
        assert_eq!(clock.machine_positions.len(), 195);
        let live = clock.publish(f);
        let mut decoder = pack::Decoder::new();
        assert_eq!(
            decoder
                .decode(live.packed.as_ref().unwrap())
                .unwrap()
                .frame
                .machine_positions,
            expected
        );
    }

    #[test]
    fn old_recordings_stay_readable() {
        let mut frame = live_proto::samples::campaign(1).remove(0);
        game::normalize_frame(&mut frame);
        // A GameFrame as servers before packed frames stored it.
        let mut old = serde_json::to_value(&frame).unwrap();
        old["source"] = json!("acct:0009a1b2c3d4e5f6@live.test");
        let packed = recorded_packed(&old).expect("old frames pack on the way out");
        assert_eq!(
            pack::decode_keyframe(&packed).unwrap(),
            pack::quantize(&frame).unwrap()
        );
        // A packed one, as servers before chunks stored it.
        let new = json!({
            "tick": frame.tick,
            "source": "acct:0009a1b2c3d4e5f6@live.test",
            "packed": pack::to_base64(&packed),
        });
        assert_eq!(recorded_packed(&new), Some(packed));
        // Damaged data is passed through, not invented.
        let broken = json!({"tick": 1, "packed": "***"});
        assert_eq!(recorded_packed(&broken), None);
    }
}
