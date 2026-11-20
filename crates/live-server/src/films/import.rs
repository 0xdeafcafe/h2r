//! A completed film materialises the existing Live game stream. Only the six
//! telemetry-compatible match events enter stats. Temporary, bounded pages let
//! either Live home finish an arbitrarily long import after a process restart.
use std::{collections::VecDeque, sync::Arc};

use live_proto::{
    ErrorCode,
    film::{
        events::{Event, Record},
        stream,
    },
    game,
    sign::{EventBody, KeyPair, parse_time, sign_batch},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{ApiResult, HOST_FILM_STORED, STREAM};
use crate::{
    app::{ApiError, Live, conflict, forbidden, invalid, now},
    blobs::{BlobStore, Meta},
    host_identity::{HostClaims, Publisher},
    store::{NewEvent, Query, RecordedEvent},
};

pub(super) const STARTED: &str = "HostFilmImportStarted";
pub(super) const ADVANCED: &str = "HostFilmImportAdvanced";
const PAGE_BYTES: usize = 256 << 10;
const PAGE_ROWS: usize = 128;

#[derive(Serialize, Deserialize)]
struct Row {
    tick: u64,
    kind: String,
    data: Box<serde_json::value::RawValue>,
}

#[derive(Default)]
pub(super) struct FilmEvents {
    pub(super) started_ms: Option<u64>,
    pub(super) tick_rate: Option<u16>,
    rows: Vec<Row>,
}

impl FilmEvents {
    pub(super) fn chunk(&mut self, raw: &[u8]) -> ApiResult<(u64, u64)> {
        let chunk = stream::Chunk::parse(raw).map_err(invalid)?;
        for record in chunk.records() {
            let record = record.map_err(invalid)?;
            if record.tag == stream::EVENT
                && let Some((&stream::STATS_EVENT, payload)) = record.payload.split_first()
            {
                if let Some((tick, kind, data)) =
                    Record::decode_telemetry(payload).map_err(invalid)?
                {
                    self.rows.push(Row {
                        tick,
                        kind: kind.into(),
                        data: serde_json::value::to_raw_value(&data).expect("canonical film event"),
                    });
                }
            }
        }
        Ok((
            chunk.first_tick,
            chunk.first_tick + u64::from(chunk.tick_count - 1),
        ))
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Page {
    sha256: String,
    count: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub(super) receipt: Value,
    pages: Arc<[Page]>,
    pub(super) next: usize,
    pub(super) skip: usize,
    origin: u64,
    hz: u16,
    start: i64,
    sealed: i64,
}

impl Pending {
    pub(super) fn page_count(&self) -> usize {
        self.pages[self.next].count
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn page_key(blob: &str, page: usize) -> String {
    format!("{blob}.import/{page}")
}

fn unavailable(e: impl std::fmt::Display) -> ApiError {
    eprintln!("live: film stats import: {e}");
    ApiError::new(
        ErrorCode::Unavailable,
        "the film import store is unavailable",
    )
}

/// Pages live only from upload until the winning receipt and game end commit.
/// Their hashes in the signed import selection bind recovery to validated data.
#[derive(Default)]
pub(super) struct Spool {
    start: Option<(u64, game::GameStarted)>,
    ended: Option<u64>,
    pages: Vec<Page>,
    rows: Vec<Row>,
    bytes: usize,
}

impl Spool {
    pub(super) async fn drain(
        &mut self,
        events: &mut FilmEvents,
        store: &BlobStore,
        blob: &str,
    ) -> ApiResult<()> {
        for row in events.rows.drain(..) {
            if self.start.is_none() {
                if row.kind != game::GAME_STARTED {
                    return Err(invalid("film stats must start with GameStarted"));
                }
                self.start = Some((
                    row.tick,
                    serde_json::from_str(row.data.get()).expect("typed GameStarted"),
                ));
                continue;
            }
            if self.ended.is_some() || row.kind == game::GAME_STARTED {
                return Err(invalid("a film must contain exactly one match"));
            }
            if row.kind == game::GAME_ENDED {
                self.ended = Some(row.tick);
            }
            let size = row.data.get().len() + row.kind.len() + 64;
            if !self.rows.is_empty()
                && (self.bytes + size > PAGE_BYTES || self.rows.len() == PAGE_ROWS)
            {
                self.flush(store, blob).await?;
            }
            self.bytes += size;
            self.rows.push(row);
        }
        Ok(())
    }

    async fn flush(&mut self, store: &BlobStore, blob: &str) -> ApiResult<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&self.rows).expect("serializes");
        let page = Page {
            sha256: digest(&bytes),
            count: self.rows.len(),
        };
        store
            .put(
                &page_key(blob, self.pages.len()),
                bytes.into(),
                &Meta::default(),
            )
            .await
            .map_err(unavailable)?;
        self.pages.push(page);
        self.bytes = 0;
        self.rows.clear();
        Ok(())
    }

    pub(super) async fn discard(&self, store: &BlobStore, blob: &str) {
        for n in 0..self.pages.len() {
            let _ = store.delete(&page_key(blob, n)).await;
        }
    }

    /// Validate the *whole* roster, event sequence and live prefix before
    /// recording any part of the new match. The caller holds host_game_writes.
    pub(super) async fn prepare(
        &mut self,
        live: &Live,
        id: &str,
        publisher: &Publisher,
        grant: &HostClaims,
        store: &BlobStore,
        receipt: Value,
        started_ms: Option<u64>,
        hz: u16,
        v3: bool,
    ) -> ApiResult<(Vec<NewEvent>, u64, bool)> {
        let blob = receipt["blob"].as_str().expect("our private object key");
        self.flush(store, blob).await?;
        if v3
            && live
                .model
                .read()
                .games
                .get(id)
                .is_some_and(|g| g.ended.is_none())
        {
            live.flush_recording(id).await?;
        }
        let existing = live.model.read().games.get(id).cloned();
        let version = existing.as_ref().map_or(0, |g| g.recording.next_version);
        if existing.as_ref().is_some_and(|g| !publisher.may_write(g)) {
            return Err(forbidden("the game belongs to another host"));
        }
        let Some((origin, recorded)) = &self.start else {
            if !v3 && let Some(g) = existing {
                return Ok((
                    vec![live.server_event(STREAM.into(), HOST_FILM_STORED, receipt)],
                    g.recording.next_version,
                    false,
                ));
            }
            return Err(invalid("publishing a unified film requires match events"));
        };
        let end = self
            .ended
            .ok_or_else(|| invalid("film stats require GameEnded"))?;
        if recorded.game_id != id || id != grant.canonical_film_id() {
            return Err(forbidden("the film is outside the assigned game"));
        }
        let mut new = recorded.game.clone();
        if new.session_id.is_none() {
            new.session_id = existing
                .as_ref()
                .and_then(|g| g.started.game.session_id.clone());
        }
        grant.bind_film(&live.model.read(), &mut new)?;
        if let Some(sim) = &new.sim {
            if game::sim_game_id(&sim.run_id) != id {
                return Err(forbidden("simulation belongs to another lease"));
            }
            crate::sim_runs::validate_hosted_start(sim, &new)?;
        } else if crate::sim_runs::is_sim_id(id) {
            return Err(forbidden("the assignment requires a simulation film"));
        }
        if new.playlist.is_none() {
            new.playlist = match &existing {
                Some(g) => g.started.game.playlist.clone(),
                None => live.playlist_name(new.playlist_id.as_deref()).await,
            };
        }
        game::validate_new(&new).map_err(invalid)?;
        if existing.as_ref().is_some_and(|g| g.started.game != new) {
            return Err(conflict(
                "film and live telemetry describe different game starts",
            ));
        }
        let before = existing.as_ref().map_or(u64::MAX, |g| g.position);
        let check_players = |players: &[game::Player]| {
            if new.session_id.is_none()
                && grant.assignment_id.starts_with("custom-")
                && !grant.roster.is_empty()
                && players
                    .iter()
                    .filter_map(|p| p.xuid)
                    .any(|x| !grant.roster.contains(&x))
            {
                return Err(forbidden("a player is outside the custom reservation"));
            }
            publisher
                .check_players(
                    &live.model.read(),
                    None,
                    new.session_id.as_deref(),
                    before,
                    players,
                )
                .map_err(invalid)
        };
        let mut roster = new.players.clone();
        check_players(&roster)?;
        let mut prefix = Prefix::new(id, existing.as_ref().map(|g| g.position));
        let mut matched = 0;
        for (n, page) in self.pages.iter().enumerate() {
            for row in read_page(store, blob, n, page).await? {
                let data: Value = serde_json::from_str(row.data.get()).map_err(unavailable)?;
                game::validate(&row.kind, &data, &roster).map_err(invalid)?;
                if let Some(committed) = prefix.next(live).await? {
                    // Re-canonicalise old senders' sparse JSON/defaults exactly
                    // as the recorder does before comparing the event bodies.
                    let old: Event = serde_json::from_value(
                        json!({"kind":committed.body.kind, "data":committed.body.data}),
                    )
                    .map_err(|_| conflict("invalid stored telemetry"))?;
                    if old
                        .telemetry()
                        .is_none_or(|(kind, old)| kind != row.kind || old != data)
                    {
                        return Err(conflict("the film disagrees with committed game events"));
                    }
                    matched += 1;
                }
                match row.kind.as_str() {
                    game::PLAYER_JOINED => {
                        let p: game::PlayerJoined =
                            serde_json::from_value(data).expect("validated");
                        if new.sim.is_some() && (!p.player.is_bot() || p.player.xuid.is_some()) {
                            return Err(invalid("simulation players must be local bots"));
                        }
                        check_players(std::slice::from_ref(&p.player))?;
                        roster.push(p.player);
                    }
                    game::PLAYER_LEFT => {
                        let p: game::PlayerLeft = serde_json::from_value(data).expect("validated");
                        roster.retain(|player| player.index != p.index);
                    }
                    _ => {}
                }
            }
        }
        if prefix.next(live).await?.is_some() {
            return Err(conflict("telemetry extends beyond the film"));
        }
        let complete = existing.as_ref().is_some_and(|g| g.ended.is_some());
        if complete && matched != self.pages.iter().map(|p| p.count).sum::<usize>() {
            return Err(conflict("the telemetry game ended before the film"));
        }
        if complete {
            return Ok((
                vec![live.server_event(STREAM.into(), HOST_FILM_STORED, receipt)],
                version,
                false,
            ));
        }
        let sealed = now();
        let lease_floor = live
            .host_view(publisher.host_lease_id().expect("host"))
            .map_or(sealed, |h| h.lease.created_ms / 1000);
        let start = existing
            .as_ref()
            .and_then(|g| parse_time(&g.started_at))
            .map(|t| t.unix_timestamp())
            .or_else(|| started_ms.and_then(|ms| i64::try_from(ms / 1000).ok()))
            .unwrap_or_else(|| {
                sealed.saturating_sub(
                    i64::try_from(end.saturating_sub(*origin) / u64::from(hz)).unwrap_or(i64::MAX),
                )
            })
            .clamp(lease_floor.min(sealed), sealed);
        let mut events = Vec::new();
        if existing.is_none() {
            let host = publisher.to_string();
            let started = game::GameStarted {
                game_id: id.into(),
                dedicated: live.dedicated_host_fields(&host),
                host,
                game: new,
            };
            let key = &live.identity.key;
            let body = EventBody::at_unix(
                key,
                &live.identity.author(),
                &game::stream(id),
                game::GAME_STARTED,
                json!(started),
                start,
            );
            events.push(NewEvent {
                jws: body.sign(key).into(),
                jws_index: None,
                body,
            });
        } else {
            events.push(live.server_event(
                game::stream(id),
                crate::replay_archive::REQUESTED,
                json!({}),
            ));
        }
        let pending = Pending {
            receipt,
            pages: self.pages.clone().into(),
            next: 0,
            skip: matched,
            origin: *origin,
            hz,
            start,
            sealed,
        };
        events.push(live.server_event(super::import_stream(id), STARTED, json!(pending)));
        Ok((events, version, true))
    }
}

struct Prefix {
    query: Query,
    done: bool,
    buffer: VecDeque<RecordedEvent>,
}

impl Prefix {
    fn new(id: &str, start: Option<u64>) -> Self {
        Self {
            query: Query {
                stream: Some(game::stream(id)),
                after: start.unwrap_or(0),
                types: game::CLIENT_GAME_EVENTS
                    .iter()
                    .map(|k| k.to_string())
                    .collect(),
                limit: 32,
                ..Default::default()
            },
            done: start.is_none(),
            buffer: VecDeque::new(),
        }
    }

    async fn next(&mut self, live: &Live) -> ApiResult<Option<RecordedEvent>> {
        if !self.done && self.buffer.is_empty() {
            let events = live.store.read(&self.query).await?;
            self.done = events.len() < self.query.limit;
            if let Some(last) = events.last() {
                self.query.after = last.position;
            }
            self.buffer = events.into();
        }
        Ok(self.buffer.pop_front())
    }
}

async fn read_page(store: &BlobStore, blob: &str, n: usize, page: &Page) -> ApiResult<Vec<Row>> {
    let bytes = store
        .get(&page_key(blob, n))
        .await
        .map_err(unavailable)?
        .ok_or_else(|| unavailable("missing stats page"))?;
    if digest(&bytes) != page.sha256 {
        return Err(unavailable("stats page digest mismatch"));
    }
    let rows: Vec<Row> = serde_json::from_slice(&bytes).map_err(unavailable)?;
    if rows.len() != page.count {
        return Err(unavailable("stats page length mismatch"));
    }
    Ok(rows)
}

/// Every append includes the cursor and a checked ledger version. A crash,
/// double finalise or lost HTTP response can only resume at the first unapplied
/// event page. Workers and native sweepers also recover without a second upload.
pub(crate) async fn resume(live: &Live, id: &str, all: bool) -> ApiResult<()> {
    loop {
        let _film_guard = live.film_writes.lock().await;
        let _host_guard = live.host_game_writes.lock(id).await;
        let (pending, host, game_version, theater_version) = {
            let m = live.model.read();
            let Some(p) = m.films.imports.get(id).cloned() else {
                return Ok(());
            };
            let g = m
                .games
                .get(id)
                .ok_or_else(|| unavailable("missing import game"))?;
            (
                p,
                g.started.host.clone(),
                g.recording.next_version,
                m.films.next_version,
            )
        };
        let store = &live
            .blobs()
            .ok_or_else(|| unavailable("no film store"))?
            .private;
        let blob = pending.receipt["blob"]
            .as_str()
            .ok_or_else(|| unavailable("no selected blob"))?;
        let n = pending.next;
        let page = pending
            .pages
            .get(n)
            .ok_or_else(|| unavailable("no selected stats page"))?;
        let rows = match read_page(store, blob, n, page).await {
            Ok(rows) => rows,
            Err(e) => {
                // Another Live process may have committed and removed this
                // page after our local projection supplied its old cursor.
                live.wait_applied(live.store.head().await?).await?;
                if live
                    .model
                    .read()
                    .films
                    .imports
                    .get(id)
                    .is_none_or(|p| p.next != n)
                {
                    continue;
                }
                return Err(e);
            }
        };
        let key = crate::host_identity::film_event_key(live, &host)?;
        let stream = game::stream(id);
        let mut events = Vec::with_capacity(rows.len() + 1);
        let bodies: Vec<_> = rows
            .into_iter()
            .skip(pending.skip)
            .map(|r| {
                let at = pending
                    .start
                    .saturating_add(
                        i64::try_from(
                            r.tick.saturating_sub(pending.origin) / u64::from(pending.hz),
                        )
                        .unwrap_or(i64::MAX),
                    )
                    .min(pending.sealed);
                EventBody::at_unix(
                    &key,
                    &host,
                    &stream,
                    &r.kind,
                    serde_json::from_str(r.data.get()).expect("validated page"),
                    at,
                )
            })
            .collect();
        append_signed(&mut events, &key, bodies);
        let last = n + 1 == pending.pages.len();
        let (kind, data) = if last {
            (HOST_FILM_STORED, pending.receipt.clone())
        } else {
            (ADVANCED, json!({"game":id,"page":n}))
        };
        let cursor = super::import_stream(id);
        events.push(live.server_event(
            if last { STREAM.into() } else { cursor.clone() },
            kind,
            data,
        ));
        let mut expected = vec![(stream, game_version), (cursor, n as u64 + 1)];
        if last {
            expected.push((STREAM.into(), theater_version));
        }
        let recorded = match live.store.append(events, &expected).await {
            Ok(rows) => rows,
            Err(crate::store::StoreError::Conflict { .. }) => {
                live.wait_applied(live.store.head().await?).await?;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        live.wait_applied(recorded.last().expect("cursor").position)
            .await?;
        if let Err(e) = store.delete(&page_key(blob, n)).await {
            eprintln!("live: film stats page cleanup: {e}");
        }
        if last {
            if !all {
                super::upload::cleanup_published(live, id).await;
            }
            live.rank_game(id).await?;
            return Ok(());
        }
        if !all {
            return Ok(());
        }
    }
}

fn append_signed(events: &mut Vec<NewEvent>, key: &KeyPair, bodies: Vec<EventBody>) {
    for chunk in bodies.chunks(16) {
        let jws: Arc<str> = sign_batch(key, chunk).into();
        events.extend(chunk.iter().enumerate().map(|(i, b)| NewEvent {
            body: b.clone(),
            jws: Arc::clone(&jws),
            jws_index: Some(i),
        }));
    }
}
