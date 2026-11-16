//! Games (docs/live-api.md): starting one, appending its signed event
//! batches, live telemetry frames, recorded `GameFrame`s, map outlines, and
//! the presence and game managers that write `PresenceExpired` and
//! `GameEnded {reason: "lost"}` when clients go quiet.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use live_proto::game::{
    self, Frame, GameCreated, GameDetail, GameEnded, GameStarted, GameSummary, NewGame, Outline,
};
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

/// A keyframe on the packed live stream at least this often (one a second
/// at 15 Hz): what a reader that joins or falls behind waits for at most.
const STREAM_KEYFRAME_EVERY: u64 = 15;

/// Per-game frame bookkeeping for the ingest path.
#[derive(Default)]
pub struct IngestState {
    games: Mutex<HashMap<String, FrameClock>>,
    /// The next packed stream id (one per game while it sends frames).
    next_sid: std::sync::atomic::AtomicU64,
}

/// A live frame as stream readers get it: the frame for JSON readers, and
/// its record in the game's packed stream (docs/live-telemetry.md, Packed
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
}

struct FrameClock {
    last_sent: Instant,
    /// Unix second of the last recorded `GameFrame`.
    last_recorded: i64,
    /// What throttled frames carried (shots, actions, hits, machines),
    /// handed to the next frame sent so none is lost.
    unsent: game::FrameCarry,
    /// What sent frames carried since the last recorded one, handed to the
    /// next recorded frame so a replay covers its whole second.
    unrecorded: game::FrameCarry,
    /// Every machine's latest state, by id: recorded frames carry them all.
    machines: BTreeMap<String, game::FrameMachine>,
    /// The game's packed live stream.
    pack: pack::Encoder,
    /// Its frames since the last keyframe, for readers that join.
    since_key: Vec<Arc<LiveFrame>>,
}

impl FrameClock {
    fn new(now: Instant, sid: u64) -> Self {
        FrameClock {
            last_sent: now - FRAME_INTERVAL,
            last_recorded: 0,
            unsent: game::FrameCarry::default(),
            unrecorded: game::FrameCarry::default(),
            machines: BTreeMap::new(),
            pack: pack::Encoder::new(sid, STREAM_KEYFRAME_EVERY),
            since_key: Vec::new(),
        }
    }

    /// Packs a sent frame into the game's stream and keeps it for readers
    /// that join before the next keyframe.
    fn publish(&mut self, frame: Frame) -> Arc<LiveFrame> {
        let seq = self.pack.next_seq();
        let (packed, keyframe) = match self.pack.encode(&frame) {
            Ok((bytes, key)) => (Some(bytes), key),
            Err(e) => {
                eprintln!("live: frame of {} does not pack: {e}", frame.game_id);
                (None, false)
            }
        };
        let live = Arc::new(LiveFrame {
            frame,
            sid: self.pack.sid(),
            seq,
            keyframe,
            packed,
        });
        if keyframe {
            self.since_key.clear();
        }
        if live.packed.is_some() {
            self.since_key.push(Arc::clone(&live));
        }
        live
    }

    /// Throttles a validated frame: `None` when it is dropped (what it
    /// carried goes to the next sent frame), else `Some` with the frame to
    /// record if this is the first sent in a new second. The sent frame's
    /// shots, actions and hits cover the frames dropped before it; the
    /// recorded one's every frame since the last recorded, and it lists
    /// every machine seen so far.
    fn admit(&mut self, frame: &mut Frame, now: Instant, now_s: i64) -> Option<Option<Frame>> {
        for m in &frame.machines {
            if self.machines.len() < game::MAX_FRAME_MACHINES || self.machines.contains_key(&m.id) {
                self.machines.insert(m.id.clone(), m.clone());
            }
        }
        if now.duration_since(self.last_sent) < FRAME_INTERVAL {
            self.unsent.add(frame);
            return None;
        }
        self.last_sent = now;
        frame.absorb(std::mem::take(&mut self.unsent));
        if self.last_recorded >= now_s {
            self.unrecorded.add(frame);
            return Some(None);
        }
        self.last_recorded = now_s;
        let mut recorded = frame.clone();
        recorded.absorb(std::mem::take(&mut self.unrecorded));
        recorded.machines = self.machines.values().cloned().collect();
        Some(Some(recorded))
    }
}

/// What a frame turned into.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Fanned out (and maybe recorded).
    Sent { recorded: bool },
    /// Dropped: faster than 15 Hz.
    Throttled,
}

/// A recorded `GameFrame`'s data: its tick, who sent it and the frame
/// packed on its own as base64url (the signature covers those bytes).
/// Older recordings hold the JSON frame itself with `source`.
fn recorded_data(frame: &Frame, author: &Author) -> Value {
    match pack::encode_keyframe(frame) {
        Ok(bytes) => json!({
            "tick": frame.tick,
            "source": author.to_string(),
            "packed": pack::to_base64(&bytes),
        }),
        Err(e) => {
            eprintln!(
                "live: recorded frame of {} does not pack: {e}",
                frame.game_id
            );
            let mut data = serde_json::to_value(frame).expect("serializes");
            data["source"] = json!(author.to_string());
            data
        }
    }
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

/// A recorded frame as JSON (`GET /v1/games/{id}/frames`): the frame plus
/// `source`, whichever form it was stored in.
pub fn recorded_json(data: &Value) -> Value {
    let Some(text) = data.get("packed").and_then(Value::as_str) else {
        return data.clone();
    };
    let frame = pack::from_base64(text).and_then(|bytes| pack::decode_keyframe(&bytes));
    match frame {
        Ok(frame) => {
            let mut out = serde_json::to_value(frame).expect("serializes");
            if let Some(source) = data.get("source") {
                out["source"] = source.clone();
            }
            out
        }
        Err(_) => data.clone(),
    }
}

fn random_game_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the OS random source failed");
    sign::hex(&b)
}

/// Which map `GET /v1/games` keeps (docs/live-api.md).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameFilter {
    /// The map's name: games on the base map and every mod of it.
    pub map: Option<String>,
    /// `Some(None)`: only games with no `MapId`; `Some(Some(id))`: that map.
    pub map_id: Option<Option<live_proto::maps::MapId>>,
}

impl GameFilter {
    pub fn matches(&self, g: &live_proto::game::NewGame) -> bool {
        self.map
            .as_ref()
            .is_none_or(|m| g.map.eq_ignore_ascii_case(m))
            && self.map_id.is_none_or(|id| g.map_id == id)
    }
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
        new: NewGame,
    ) -> Result<GameCreated, ApiError> {
        let author = self.authenticate(bearer, lxm::GAMES_WRITE)?;
        self.rate_limit(&author, lxm::GAMES_WRITE)?;
        game::validate_new(&new).map_err(invalid)?;
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
                game::PLAYER_JOINED => players
                    .push(serde_json::from_value(b.data["player"].clone()).expect("validated")),
                game::PLAYER_LEFT => {
                    players.retain(|p| Some(p.index as u64) != b.data["index"].as_u64())
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
        let events = bodies
            .into_iter()
            .enumerate()
            .map(|(i, body)| NewEvent {
                jws: jws.to_string(),
                jws_index: Some(i),
                body,
            })
            .collect();
        let recorded = self.store.append(events, &[]).await?;
        let last = recorded.last().map_or(0, |e| e.position);
        self.wait_applied(last).await?;
        Ok(last)
    }

    /// A telemetry frame from the ingest socket: fanned out to stream
    /// readers at up to 15 Hz per game, and recorded as a server-signed
    /// `GameFrame` (with `data.source`) once a second.
    pub async fn ingest_frame(
        &self,
        author: &Author,
        mut frame: Frame,
    ) -> Result<FrameOutcome, ApiError> {
        if !game::valid_id(&frame.game_id) {
            return Err(invalid("bad game_id"));
        }
        self.writable_game(&frame.game_id, author)?;
        game::validate_frame(&frame).map_err(invalid)?;
        game::normalize_frame(&mut frame);
        let now_s = now();
        self.model.touch_game(&frame.game_id, now_s);
        let (record, live) = {
            let mut games = self.ingest.games.lock().unwrap();
            let now = Instant::now();
            let clock = games.entry(frame.game_id.clone()).or_insert_with(|| {
                let sid = self
                    .ingest
                    .next_sid
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                FrameClock::new(now, sid)
            });
            let record = match clock.admit(&mut frame, now, now_s) {
                None => return Ok(FrameOutcome::Throttled),
                Some(record) => record,
            };
            (record, clock.publish(frame))
        };
        let recorded = record.is_some();
        let _ = self.frames.send(live);
        if let Some(frame) = record {
            let data = recorded_data(&frame, author);
            let event = self.server_event(game::stream(&frame.game_id), game::GAME_FRAME, data);
            self.store.append(vec![event], &[]).await?;
        }
        Ok(FrameOutcome::Sent { recorded })
    }

    pub fn frames(&self) -> tokio::sync::broadcast::Receiver<Arc<LiveFrame>> {
        self.frames.subscribe()
    }

    /// Each live game's packed frames since its last keyframe (all games,
    /// or those in `games`): what a packed reader that joins or fell
    /// behind is sent first.
    pub fn frames_since_keyframe(&self, games: &[String]) -> Vec<Arc<LiveFrame>> {
        let all = self.ingest.games.lock().unwrap();
        all.iter()
            .filter(|(id, _)| games.is_empty() || games.contains(id))
            .flat_map(|(_, clock)| clock.since_key.iter().cloned())
            .collect()
    }

    /// `GET /v1/games`: newest first; the cursor is a start position.
    pub fn games(
        &self,
        state: Option<&str>,
        before: Option<u64>,
        limit: usize,
    ) -> Page<GameSummary> {
        self.games_where(state, &GameFilter::default(), before, limit)
    }

    /// [`Live::games`] narrowed to one map: by name (`?map=`, every game on
    /// it, modded or not) and by `MapId` (`?map_id=`: one exact map, or
    /// `none` for games that name no `MapId`, the retail ones).
    pub fn games_where(
        &self,
        state: Option<&str>,
        filter: &GameFilter,
        before: Option<u64>,
        limit: usize,
    ) -> Page<GameSummary> {
        let position = self.position();
        let s = self.model.read();
        let page: Vec<&Game> = s
            .games_newest()
            .filter(|g| before.is_none_or(|b| g.position < b))
            .filter(|g| filter.matches(&g.started.game))
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

    /// `GET /v1/games/{id}/frames`: recorded frames with `from_tick <= tick
    /// <= to_tick` as JSON (each the frame plus `source`, however it was
    /// stored), oldest first; the cursor is a log position.
    pub async fn recorded_frames(
        &self,
        id: &str,
        from_tick: Option<u64>,
        to_tick: Option<u64>,
        after: u64,
        limit: usize,
    ) -> Result<Page<Value>, ApiError> {
        let mut page = self
            .recorded_frame_data(id, from_tick, to_tick, after, limit)
            .await?;
        page.items = page.items.iter().map(recorded_json).collect();
        Ok(page)
    }

    /// [`Live::recorded_frames`] as stored: each `GameFrame`'s data, packed
    /// (`{tick, source, packed}`) or, for older games, the JSON frame.
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

    /// The managers: expire quiet presence and end idle games, as the
    /// server. Deadlines come from the read model, so they survive a
    /// restart (rebuilt from the events' `at`).
    pub async fn sweep(&self) {
        let now = now();
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
    fn recorded_frames_sum_the_shots_of_their_second() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        let at = |ms: u64| start + Duration::from_millis(ms);

        // Second 10: sent at 0, 100, 200 ms (one throttled at 120 ms).
        let mut f = frame(1, 0);
        let recorded = clock.admit(&mut f, at(0), 10).unwrap().unwrap();
        assert_eq!(shots(&recorded), (Some(1), None));
        let mut f = frame(2, 1);
        assert_eq!(clock.admit(&mut f, at(100), 10), Some(None));
        assert_eq!(shots(&f), (Some(2), Some(1)));
        let mut f = frame(4, 0);
        assert_eq!(clock.admit(&mut f, at(120), 10), None, "throttled");
        let mut f = frame(0, 3);
        assert_eq!(clock.admit(&mut f, at(200), 10), Some(None));
        assert_eq!(shots(&f), (Some(4), Some(3)), "carries the throttled frame");

        // Second 11: the recorded frame covers everything since the last one.
        let mut f = frame(1, 1);
        let recorded = clock.admit(&mut f, at(1000), 11).unwrap().unwrap();
        assert_eq!(shots(&f), (Some(1), Some(1)), "live frame: its own shots");
        assert_eq!(shots(&recorded), (Some(2 + 4 + 1), Some(1 + 3 + 1)));

        // Capped at 64 per shooter.
        for i in 0..5 {
            let mut f = frame(30, 30);
            assert!(clock.admit(&mut f, at(1100 + i * 100), 11).is_some());
        }
        let mut f = frame(0, 0);
        let recorded = clock.admit(&mut f, at(2000), 12).unwrap().unwrap();
        assert_eq!(shots(&recorded), (Some(64), Some(64)));
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
    fn recorded_frames_carry_the_seconds_actions_hits_and_every_machine() {
        use live_proto::game::FrameHit;
        let start = Instant::now();
        let mut clock = FrameClock::new(start, 0);
        let at = |ms: u64| start + Duration::from_millis(ms);
        let hit = |amount| FrameHit {
            amount: Some(amount),
            ..FrameHit::default()
        };

        // Second 10: the first frame is recorded with both machines.
        let mut f = frame(0, 0);
        f.machines = vec![machine("a", 0.0), machine("b", 0.0)];
        let recorded = clock.admit(&mut f, at(0), 10).unwrap().unwrap();
        assert_eq!(recorded.machines.len(), 2);
        // A sent frame with a reload, one hit and door a moving.
        let mut f = frame(0, 0);
        f.players[0].actions = Some(vec!["reload".into()]);
        f.hits = vec![hit(0.1)];
        f.machines = vec![machine("a", 0.5)];
        assert_eq!(clock.admit(&mut f, at(100), 10), Some(None));
        // A throttled one with a jump and a hit: carried into the next sent.
        let mut f = frame(0, 0);
        f.actors[0].actions = Some(vec!["jump".into()]);
        f.hits = vec![hit(0.2)];
        assert_eq!(clock.admit(&mut f, at(120), 10), None);
        let mut f = frame(0, 0);
        assert_eq!(clock.admit(&mut f, at(200), 10), Some(None));
        assert_eq!(f.actors[0].actions, Some(vec!["jump".into()]));
        assert_eq!(f.hits, [hit(0.2)]);

        // Second 11's recorded frame has all of it, and both machines' latest
        // state though neither moved in it.
        let mut f = frame(0, 0);
        f.hits = vec![hit(0.3)];
        let recorded = clock.admit(&mut f, at(1000), 11).unwrap().unwrap();
        assert_eq!(f.hits, [hit(0.3)], "the live frame keeps its own");
        assert_eq!(recorded.players[0].actions, Some(vec!["reload".into()]));
        assert_eq!(recorded.actors[0].actions, Some(vec!["jump".into()]));
        assert_eq!(recorded.hits, [hit(0.1), hit(0.2), hit(0.3)]);
        let machines: Vec<_> = recorded
            .machines
            .iter()
            .map(|m| (m.id.as_str(), m.position))
            .collect();
        assert_eq!(machines, [("a", Some(0.5)), ("b", Some(0.0))]);
    }

    fn author() -> Author {
        Author::parse("acct:0009a1b2c3d4e5f6@live.test").unwrap()
    }

    #[test]
    fn old_json_recordings_stay_readable() {
        let mut frame = live_proto::samples::campaign(1).remove(0);
        game::normalize_frame(&mut frame);
        // A GameFrame as servers before packed frames stored it.
        let mut old = serde_json::to_value(&frame).unwrap();
        old["source"] = json!("acct:0009a1b2c3d4e5f6@live.test");
        assert_eq!(recorded_json(&old), old, "JSON stays as it was");
        let packed = recorded_packed(&old).expect("old frames pack on the way out");
        assert_eq!(
            pack::decode_keyframe(&packed).unwrap(),
            pack::quantize(&frame).unwrap()
        );

        // A packed recording: tick and source in the clear, the frame packed.
        let new = recorded_data(&frame, &author());
        assert_eq!(new["tick"], frame.tick);
        assert!(new.get("players").is_none());
        let back = recorded_json(&new);
        assert_eq!(back["source"], old["source"]);
        let decoded: Frame = serde_json::from_value(back).unwrap();
        assert_eq!(decoded, pack::quantize(&frame).unwrap());
        assert_eq!(recorded_packed(&new), Some(packed));
        // Damaged data is passed through, not invented.
        let broken = json!({"tick": 1, "packed": "***"});
        assert_eq!(recorded_json(&broken), broken);
        assert_eq!(recorded_packed(&broken), None);
    }

    /// What one recorded frame costs at rest: its signed event's JWS, the
    /// string the store keeps.
    fn stored_bytes(data: Value) -> usize {
        let key = live_proto::sign::KeyPair::generate();
        let body = live_proto::sign::EventBody::new(
            &key,
            "server:live.test",
            "game-c4a1d2e3f4a5b6c7d8e9f0a1b2c3d4e5",
            game::GAME_FRAME,
            data,
            time::OffsetDateTime::now_utc(),
        );
        body.sign(&key).len()
    }

    #[test]
    fn recorded_frames_cost_less_at_rest() {
        for (name, frames) in [
            ("multiplayer", live_proto::samples::multiplayer(450)),
            ("campaign", live_proto::samples::campaign(450)),
        ] {
            let recorded: Vec<Frame> = frames.into_iter().step_by(15).collect();
            let json: usize = recorded
                .iter()
                .map(|f| {
                    let mut d = serde_json::to_value(f).unwrap();
                    d["source"] = json!(author().to_string());
                    stored_bytes(d)
                })
                .sum();
            let packed: usize = recorded
                .iter()
                .map(|f| stored_bytes(recorded_data(f, &author())))
                .sum();
            let n = recorded.len();
            println!(
                "{name}: stored JWS per recorded frame: JSON {} B, packed {} B",
                json / n,
                packed / n
            );
            assert!(packed * 4 < json, "{name}: {packed} vs {json}");
        }
    }
}
