//! Games (docs/live/live-api.md): starting one, appending its signed event
//! batches, live telemetry frames, recorded frame chunks (and the
//! `GameFrame`s older games recorded), map outlines, and
//! the presence and game managers that write `PresenceExpired` and
//! `GameEnded {reason: "lost"}` when clients go quiet.

use std::collections::{BTreeMap, HashMap};
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
use crate::model::{Game, nonce_key};
use crate::store::{NewEvent, Query, StoreError};

/// Live frames go to readers at most this often per game.
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

struct FrameClock {
    last_sent: Instant,
    /// What throttled frames carried (shots, actions, hits, machines),
    /// handed to the next frame sent so none is lost.
    unsent: game::FrameCarry,
    /// Every machine's latest state, by id: a chunk's first frame carries
    /// them all, so a reader that starts there has every door.
    machines: BTreeMap<String, game::FrameMachine>,
    machine_positions: BTreeMap<u16, game::MachinePosition>,
    machine_reset: bool,
    /// The sent frames being cut into chunks for the recording.
    chunks: ChunkWriter,
    /// The game's packed live stream.
    pack: pack::Encoder,
    /// Its frames since the last keyframe, for readers that join.
    since_key: Vec<Arc<LiveFrame>>,
    /// Its packed frames since the last tick ([`crate::packed`]).
    pending: crate::packed::Pending,
}

impl FrameClock {
    fn new(now: Instant, sid: u64) -> Self {
        FrameClock {
            last_sent: now - FRAME_INTERVAL,
            unsent: game::FrameCarry::default(),
            machines: BTreeMap::new(),
            machine_positions: BTreeMap::new(),
            machine_reset: false,
            chunks: ChunkWriter::default(),
            pack: pack::Encoder::new(sid, STREAM_KEYFRAME_EVERY),
            since_key: Vec::new(),
            pending: Default::default(),
        }
    }

    /// Packs a sent frame into the game's stream and keeps it for readers
    /// that join before the next keyframe.
    fn publish(&mut self, mut frame: Frame) -> Arc<LiveFrame> {
        if self.machine_reset {
            self.pack.force_keyframe();
        }
        let seq = self.pack.next_seq();
        let (packed, keyframe) = match self
            .pack
            .encode_with_machine_baseline(&frame, Some(&self.machine_positions))
        {
            Ok((bytes, key)) => (Some(bytes), key),
            Err(e) => {
                eprintln!("live: frame of {} does not pack: {e}", frame.game_id);
                (None, false)
            }
        };
        if keyframe {
            self.machine_reset = false;
            frame.machine_positions = self.machine_positions.values().copied().collect();
        }
        let live = Arc::new(LiveFrame {
            frame,
            sid: self.pack.sid(),
            seq,
            keyframe,
            packed,
            json: Default::default(),
        });
        if keyframe {
            self.since_key.clear();
        }
        if let Some(packed) = &live.packed {
            self.pending.push(seq, keyframe, packed);
            self.since_key.push(Arc::clone(&live));
        }
        live
    }

    /// Throttles a validated frame: false when it is dropped (what it
    /// carried goes to the next sent frame). A sent frame's shots, actions
    /// and hits cover the frames dropped before it.
    fn admit(&mut self, frame: &mut Frame, now: Instant, machine_baseline: bool) -> bool {
        for m in &frame.machines {
            if self.machines.len() < game::MAX_FRAME_MACHINES || self.machines.contains_key(&m.id) {
                self.machines.insert(m.id.clone(), m.clone());
            }
        }
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
            self.machine_reset = true;
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
        if now.duration_since(self.last_sent) < FRAME_INTERVAL {
            self.unsent.add(frame);
            return false;
        }
        self.last_sent = now;
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
            self.machine_reset,
        )
    }
}

/// What a frame turned into.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Fanned out and added to the recording; `recorded` when it closed a
    /// chunk, which was stored.
    Sent { recorded: bool },
    /// Dropped: faster than 15 Hz.
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
    fn writable_game(&self, id: &str, author: &Author) -> Result<Game, ApiError> {
        let s = self.model.read();
        let g = s.games.get(id).ok_or_else(|| not_found("no such game"))?;
        if g.ended.is_some() {
            return Err(conflict("the game has ended"));
        }
        if !g.admits(author) {
            return Err(forbidden("only the game's host and players write to it"));
        }
        Ok(g.clone())
    }

    /// `POST /v1/games`: the server writes `GameStarted` with a fresh id
    /// and `host` = the token's author.
    pub async fn start_game(
        &self,
        bearer: Option<&str>,
        mut new: NewGame,
    ) -> Result<GameCreated, ApiError> {
        let author = self.authenticate(bearer, lxm::GAMES_WRITE)?;
        self.rate_limit(&author, lxm::GAMES_WRITE)?;
        if new.playlist.is_none() {
            new.playlist = self.playlist_name(new.playlist_id.as_deref()).await;
        }
        game::validate_new(&new).map_err(invalid)?;
        // Bots (docs/game/bots.md §9.3): the session's roster as it is now,
        // which is the one in force when GameStarted lands.
        let host = match &author {
            Author::Account { xuid, .. } => Some(*xuid),
            _ => None,
        };
        crate::bots::check_players(
            &self.model.read(),
            host,
            new.session_id.as_deref(),
            u64::MAX,
            &new.players,
        )
        .map_err(invalid)?;
        let game_id = random_game_id();
        let started = GameStarted {
            game_id: game_id.clone(),
            host: author.to_string(),
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
        let author = self.authenticate(bearer, lxm::GAMES_WRITE)?;
        self.append_game_batch(&author, game_id, jws).await
    }

    /// One batch JWS of game events by `author`, verified once, appended in
    /// order. Returns the last event's position.
    pub async fn append_game_batch(
        &self,
        author: &Author,
        game_id: &str,
        jws: &str,
    ) -> Result<u64, ApiError> {
        self.rate_limit(author, lxm::GAMES_WRITE)?;
        let (parsed, bodies) = SignedEvent::open_batch(jws).map_err(sign_error)?;
        let first = &bodies[0];
        if first.author != author.to_string() {
            return Err(forbidden("the events' author is not the token's issuer"));
        }
        let key = self
            .key_for(author, &first.kid)
            .ok_or_else(|| unauthorized("the events' key is unknown or revoked"))?;
        parsed.verify(&key).map_err(sign_error)?;
        let game = self.writable_game(game_id, author)?;
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
                    let itself = matches!(author, Author::Account { xuid, .. }
                        if p.xuid == Some(*xuid) && !p.is_bot());
                    if !by_host && !itself {
                        return Err(forbidden(format!(
                            "event {i}: a player joins as itself; the host adds others"
                        )));
                    }
                    crate::bots::check_players(
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
        let events: Vec<NewEvent> = bodies
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
            self.flush_frames(game_id).await;
        }
        let recorded = self.store.append(events, &[]).await?;
        let last = recorded.last().map_or(0, |e| e.position);
        self.wait_applied(last).await?;
        if ends {
            // A finished ranked game ranks its players (crate::ranks).
            self.rank_game(game_id).await?;
        }
        Ok(last)
    }

    /// A telemetry frame from the ingest socket: fanned out to stream
    /// readers at up to 15 Hz per game, and every frame sent recorded in
    /// the game's frame chunks ([`live_proto::chunk`]): a chunk is stored
    /// when a frame lands in the next ten seconds, or the game ends.
    pub async fn ingest_frame(
        &self,
        author: &Author,
        frame: Frame,
    ) -> Result<FrameOutcome, ApiError> {
        self.ingest_frame_with_machine_baseline(author, frame, false)
            .await
    }

    pub(crate) async fn ingest_frame_with_machine_baseline(
        &self,
        author: &Author,
        mut frame: Frame,
        machine_baseline: bool,
    ) -> Result<FrameOutcome, ApiError> {
        if !game::valid_id(&frame.game_id) {
            return Err(invalid("bad game_id"));
        }
        self.writable_game(&frame.game_id, author)?;
        game::validate_frame(&frame).map_err(invalid)?;
        game::normalize_frame(&mut frame);
        let now_s = now();
        self.model.touch_game(&frame.game_id, now_s);
        let game_id = frame.game_id.clone();
        let clock = self.ingest.clock(&game_id);
        let closed = {
            let mut clock = clock.lock().unwrap();
            if !clock.admit(&mut frame, Instant::now(), machine_baseline) {
                return Ok(FrameOutcome::Throttled);
            }
            let closed = clock.record(&frame);
            // Sent under the game's lock: its frames go out in order.
            let _ = self.frames.send(clock.publish(frame));
            closed
        };
        let recorded = closed.is_some();
        if let Some(c) = closed {
            self.store_chunk(&game_id, c).await?;
        }
        Ok(FrameOutcome::Sent { recorded })
    }

    /// Stores game `id`'s open chunk, if it has frames: at the game's end
    /// (its `GameEnded`, or the sweep ending it as lost). Failures are
    /// logged; the chunk is lost, the game is not.
    pub async fn flush_frames(&self, id: &str) {
        let clock = self.ingest.games.lock().unwrap().get(id).cloned();
        let Some(clock) = clock else {
            return;
        };
        let chunk = clock.lock().unwrap().chunks.finish();
        if let Some(c) = chunk
            && let Err(e) = self.store_chunk(id, c).await
        {
            eprintln!("live: frame chunk of {id}: {}", e.message);
        }
    }

    /// Puts a chunk's bytes in the private blob store (R2 on the Worker,
    /// the data directory natively; the event store's blobs on a server
    /// with neither), then its index entry in the log as a server-signed
    /// `FrameChunk`.
    async fn store_chunk(&self, id: &str, c: Chunk) -> Result<(), ApiError> {
        match self.blobs() {
            Some(b) => {
                let meta = crate::blobs::Meta {
                    content_type: Some(CHUNK_TYPE.into()),
                    content_encoding: None,
                    cache_control: None,
                };
                b.private
                    .put(&chunk_key(id, c.meta.from_tick), c.bytes.into(), &meta)
                    .await
                    .map_err(|e| {
                        eprintln!("live: frame chunk of {id}: {e}");
                        ApiError::new(
                            live_proto::ErrorCode::Unavailable,
                            "the blob store is unavailable",
                        )
                    })?;
            }
            None => self.store.put_blob(&c.meta.sha256, &c.bytes).await?,
        }
        let data = serde_json::to_value(&c.meta).expect("serializes");
        let event = self.server_event(game::stream(id), chunk::FRAME_CHUNK, data);
        self.store.append(vec![event], &[]).await?;
        Ok(())
    }

    /// `GET /v1/games/{id}/chunks`: the game's frame chunk index, oldest
    /// first: each `FrameChunk` event's data. Empty for a game recorded
    /// before chunks (its frames are `GameFrame`s, read with
    /// [`Live::recorded_frame_data`]).
    pub async fn frame_chunks(&self, id: &str) -> Result<Vec<ChunkMeta>, ApiError> {
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
        out.sort_by_key(|m| m.from_tick);
        Ok(out)
    }

    /// `GET /v1/games/{id}/chunks/{from_tick}`: one chunk's bytes, checked
    /// against the hash its signed index entry holds.
    pub async fn frame_chunk(&self, id: &str, from_tick: u64) -> Result<Bytes, ApiError> {
        let index = self.frame_chunks(id).await?;
        let meta = index
            .iter()
            .find(|m| m.from_tick == from_tick)
            .ok_or_else(|| not_found("no such chunk"))?;
        self.stored_chunk(id, meta).await
    }

    async fn stored_chunk(&self, id: &str, meta: &ChunkMeta) -> Result<Bytes, ApiError> {
        let from_tick = meta.from_tick;
        let mut bytes = None;
        if let Some(b) = self.blobs() {
            match b.private.get(&chunk_key(id, from_tick)).await {
                Ok(found) => bytes = found,
                Err(e) => eprintln!("live: frame chunk of {id}: {e}"),
            }
        }
        if bytes.is_none() {
            bytes = self.store.blob(&meta.sha256).await?.map(Bytes::from);
        }
        let bytes = bytes.ok_or_else(|| not_found("the chunk is not stored"))?;
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
        for (id, clock) in clocks {
            let mut clock = clock.lock().unwrap();
            let sid = clock.pack.sid();
            if let Some(b) = clock.pending.take(&id, sid)
                && readers
            {
                batches.push(b);
            }
        }
        if !batches.is_empty() {
            self.packed.publish(batches);
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
            items: page.iter().map(|g| g.summary()).collect(),
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

    pub fn game(&self, id: &str) -> Result<GameDetail, ApiError> {
        let position = self.position();
        let s = self.model.read();
        let g = s.games.get(id).ok_or_else(|| not_found("no such game"))?;
        let n = &g.started.game;
        Ok(GameDetail {
            summary: g.summary(),
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

    /// `PUT /v1/maps/{map}/outline`: the first upload of a map wins; the
    /// operator may replace it.
    pub async fn put_outline(
        &self,
        bearer: Option<&str>,
        map: &str,
        bytes: &[u8],
    ) -> Result<Value, ApiError> {
        let author = self.authenticate(bearer, lxm::GAMES_WRITE)?;
        self.rate_limit(&author, lxm::GAMES_WRITE)?;
        if !game::valid_id(map) {
            return Err(invalid("map must be [A-Za-z0-9_-]{1,64}"));
        }
        let outline: Outline =
            serde_json::from_slice(bytes).map_err(|e| invalid(format!("outline: {e}")))?;
        if outline.map != map {
            return Err(invalid("the outline's map is not the path's"));
        }
        let operator = matches!(author, Author::Server(_));
        let stream = game::map_stream(map);
        let expect = if operator {
            Vec::new()
        } else {
            vec![(stream.clone(), 0)]
        };
        if !operator && self.model.read().outlines.contains_key(map) {
            return Err(conflict("this map already has an outline"));
        }
        let sha256 = sign::hex(&Sha256::digest(bytes));
        self.store.put_blob(&sha256, bytes).await?;
        let data = json!({"map": map, "sha256": sha256, "size": bytes.len(), "source": author.to_string()});
        let event = self.server_event(stream, game::MAP_OUTLINE_STORED, data.clone());
        let recorded = match self.store.append(vec![event], &expect).await {
            Err(StoreError::Conflict { .. }) => {
                return Err(conflict("this map already has an outline"));
            }
            r => r?,
        };
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
        let games = s
            .games
            .values()
            .filter(|g| g.ended.is_none())
            .map(|g| g.last_activity + policy.game_idle_s);
        let sessions = s
            .sessions
            .values()
            .filter(|v| v.state == "open")
            .map(|v| v.last_activity + policy.session_idle_s);
        presence.chain(games).chain(sessions).min().map(|d| d + 1)
    }

    /// The managers: expire quiet presence and end idle games, as the
    /// server. Deadlines come from the read model, so they survive a
    /// restart (rebuilt from the events' `at`).
    pub async fn sweep(&self) {
        let now = now();
        self.flush_usage().await;
        self.sweep_sessions(now).await;
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
                .filter(|g| g.ended.is_none() && g.last_activity + policy.game_idle_s < now)
                .map(|g| {
                    (
                        g.started.game_id.clone(),
                        g.last_tick.unwrap_or(0),
                        g.scoreboard.clone(),
                    )
                })
                .collect();
            (expired, idle)
        };
        let mut events = Vec::new();
        for xuid in expired {
            events.push(self.server_event(
                streams::presence(xuid),
                types::PRESENCE_EXPIRED,
                json!({"xuid": xuid}),
            ));
        }
        for (id, tick, players) in idle {
            self.flush_frames(&id).await;
            let ended = GameEnded {
                tick,
                reason: "lost".into(),
                winner: None,
                players,
            };
            events.push(self.server_event(
                game::stream(&id),
                game::GAME_ENDED,
                serde_json::to_value(ended).expect("serializes"),
            ));
            self.ingest.games.lock().unwrap().remove(&id);
        }
        if events.is_empty() {
            return;
        }
        match self.store.append(events, &[]).await {
            Ok(recorded) => {
                // Wait, so the next sweep does not write them twice.
                let _ = self
                    .wait_applied(recorded.last().map_or(0, |e| e.position))
                    .await;
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
        assert!(clock.admit(&mut f, at(0), false));
        assert_eq!(shots(&f), (Some(1), None));
        let mut f = frame(4, 0);
        assert!(!clock.admit(&mut f, at(20), false), "throttled");
        let mut f = frame(0, 3);
        assert!(clock.admit(&mut f, at(100), false));
        assert_eq!(shots(&f), (Some(4), Some(3)), "carries the throttled frame");
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
            if clock.admit(&mut f, at(ms), false) {
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
            if !clock.admit(&mut f, start + Duration::from_millis(ms), false) {
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
    fn machine_positions_empty_ingest_baseline_clears_recorded_and_live_state() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        let mut first = frame(0, 0);
        first.tick = 10;
        first.machine_positions = vec![game::MachinePosition { i: 7, p: 200 }];
        assert!(clock.admit(&mut first, start, true));
        assert!(clock.record(&first).is_none());
        clock.publish(first);

        let mut throttled = frame(0, 0);
        throttled.tick = 12;
        throttled.machine_positions = vec![game::MachinePosition { i: 8, p: 128 }];
        assert!(!clock.admit(&mut throttled, start + Duration::from_millis(10), false));

        let mut reset = frame(0, 0);
        reset.tick = 14;
        assert!(clock.admit(&mut reset, start + Duration::from_millis(70), true));
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
        assert!(clock.admit(&mut f, start, false));
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
