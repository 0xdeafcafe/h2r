//! Games (docs/live-api.md): starting one, appending its signed event
//! batches, live telemetry frames, recorded `GameFrame`s, map outlines, and
//! the presence and game managers that write `PresenceExpired` and
//! `GameEnded {reason: "lost"}` when clients go quiet.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use live_proto::game::{
    self, Frame, GameCreated, GameDetail, GameEnded, GameStarted, GameSummary, NewGame, Outline,
};
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

/// Per-game frame bookkeeping for the ingest path.
#[derive(Default)]
pub struct IngestState {
    games: Mutex<HashMap<String, FrameClock>>,
}

struct FrameClock {
    last_sent: Instant,
    /// Unix second of the last recorded `GameFrame`.
    last_recorded: i64,
}

/// What a frame turned into.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Fanned out (and maybe recorded).
    Sent { recorded: bool },
    /// Dropped: faster than 15 Hz.
    Throttled,
}

fn random_game_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the OS random source failed");
    sign::hex(&b)
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
        frame: Frame,
    ) -> Result<FrameOutcome, ApiError> {
        if !game::valid_id(&frame.game_id) {
            return Err(invalid("bad game_id"));
        }
        self.writable_game(&frame.game_id, author)?;
        game::validate_frame(&frame).map_err(invalid)?;
        let now_s = now();
        self.model.touch_game(&frame.game_id, now_s);
        let record = {
            let mut games = self.ingest.games.lock().unwrap();
            let clock = games.entry(frame.game_id.clone()).or_insert(FrameClock {
                last_sent: Instant::now() - FRAME_INTERVAL,
                last_recorded: 0,
            });
            if clock.last_sent.elapsed() < FRAME_INTERVAL {
                return Ok(FrameOutcome::Throttled);
            }
            clock.last_sent = Instant::now();
            let record = clock.last_recorded < now_s;
            if record {
                clock.last_recorded = now_s;
            }
            record
        };
        let frame = Arc::new(frame);
        let _ = self.frames.send(frame.clone());
        if record {
            let mut data = serde_json::to_value(&*frame).expect("serializes");
            data["source"] = json!(author.to_string());
            let event = self.server_event(game::stream(&frame.game_id), game::GAME_FRAME, data);
            self.store.append(vec![event], &[]).await?;
        }
        Ok(FrameOutcome::Sent { recorded: record })
    }

    pub fn frames(&self) -> tokio::sync::broadcast::Receiver<Arc<Frame>> {
        self.frames.subscribe()
    }

    /// `GET /v1/games`: newest first; the cursor is a start position.
    pub fn games(
        &self,
        state: Option<&str>,
        before: Option<u64>,
        limit: usize,
    ) -> Page<GameSummary> {
        let position = self.position();
        let s = self.model.read();
        let page: Vec<&Game> = s
            .games_newest()
            .filter(|g| before.is_none_or(|b| g.position < b))
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
    /// <= to_tick`, oldest first; the cursor is a log position.
    pub async fn recorded_frames(
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
