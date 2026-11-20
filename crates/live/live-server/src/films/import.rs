//! A completed film materialises the existing Live game stream. Only the six
//! telemetry-compatible match events enter stats. Temporary, bounded pages let
//! either Live home finish an arbitrarily long import after a process restart.
mod pages;

use pages::{Page, PageIndex, digest, page_key, read_page};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};

use live_proto::{
    ErrorCode, Xuid,
    film::{
        DiscrepancyCause, StatsDiscrepancy,
        events::{Event, Record},
        stream,
    },
    game,
    sign::{EventBody, KeyPair, parse_time, sign_batch},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ApiResult, HOST_FILM_STORED};
use crate::{
    app::{ApiError, Live, forbidden, invalid, now},
    blobs::{BlobStore, Meta},
    host_identity::{FilmReservation, Publisher},
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

#[derive(Default, Serialize, Deserialize)]
pub(super) struct FilmEvents {
    pub(super) started_ms: Option<u64>,
    pub(super) tick_rate: Option<u16>,
    rows: Vec<Row>,
}

impl FilmEvents {
    pub(super) fn chunk(&mut self, raw: &[u8]) -> ApiResult<usize> {
        let before = self.rows.len();
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
        Ok(self.rows.len() - before)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub(super) receipt: Value,
    // Earlier Live receipts embedded all page descriptors directly.
    #[serde(default, skip_serializing_if = "pages_empty")]
    pages: Arc<[Page]>,
    #[serde(default)]
    index: Arc<PageIndex>,
    pub(super) next: usize,
    pub(super) skip: usize,
    origin: u64,
    hz: u16,
    start: i64,
    sealed: i64,
}

fn pages_empty(pages: &Arc<[Page]>) -> bool {
    pages.is_empty()
}

impl Pending {
    pub(super) fn advance(&mut self, row_count: Option<u64>) {
        let Some(applied) = row_count
            .and_then(|n| usize::try_from(n).ok())
            .or_else(|| self.pages.get(self.next).map(|page| page.count))
        else {
            return;
        };
        self.skip = self.skip.saturating_sub(applied);
        self.next += 1;
    }

    fn page_count(&self) -> usize {
        if self.pages.is_empty() {
            self.index.len
        } else {
            self.pages.len()
        }
    }

    async fn page(&self, store: &BlobStore, blob: &str, n: usize) -> ApiResult<Page> {
        if self.pages.is_empty() {
            self.index.page(store, blob, n).await
        } else {
            self.pages
                .get(n)
                .cloned()
                .ok_or_else(|| unavailable("missing legacy stats page"))
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Comparison {
    start_position: u64,
    page: usize,
    row: usize,
    matched: usize,
    after: u64,
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
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Spool {
    start: Option<(u64, game::GameStarted)>,
    ended: Option<u64>,
    index: PageIndex,
    rows: Vec<Row>,
    bytes: usize,
    roster: Vec<game::Player>,
    comparison: Option<Comparison>,
    issue: Option<StatsDiscrepancy>,
    event_count: usize,
    participants: BTreeSet<Xuid>,
}

impl Spool {
    pub(super) async fn drain(
        &mut self,
        events: &mut FilmEvents,
        store: &BlobStore,
        blob: &str,
        live: &Live,
        id: &str,
        publisher: &Publisher,
        grant: &FilmReservation,
    ) -> ApiResult<()> {
        for row in events.rows.drain(..) {
            let index = self.event_count;
            self.event_count += 1;
            if row.kind == game::GAME_STARTED {
                let mut recorded: game::GameStarted =
                    serde_json::from_str(row.data.get()).expect("typed GameStarted");
                if recorded.game_id != id || id != grant.canonical_film_id() {
                    return Err(forbidden("the film is outside the assigned game"));
                }
                let new = &mut recorded.game;
                {
                    let model = live.model.read();
                    let existing = model.games.get(id);
                    if existing.is_some_and(|g| !publisher.may_write(g)) {
                        return Err(forbidden("the game belongs to another host"));
                    }
                    if new.session_id.is_none() {
                        new.session_id = existing.and_then(|g| g.started.game.session_id.clone());
                    }
                    grant.bind_film(&model, new)?;
                }
                if let Some(sim) = &new.sim {
                    if game::sim_game_id(&sim.run_id) != id {
                        return Err(forbidden("simulation belongs to another lease"));
                    }
                    crate::sim_runs::validate_hosted_start(sim, new)?;
                } else if crate::sim_runs::is_sim_id(id) {
                    return Err(forbidden("the assignment requires a simulation film"));
                }
                check_roster(live, publisher, grant, id, new, &new.players)?;
                if game::validate_new(new).is_err() || self.start.is_some() {
                    self.flag(DiscrepancyCause::InvalidEventSequence, index, row.tick);
                    continue;
                }
                self.participants.extend(
                    new.players
                        .iter()
                        .filter_map(|p| p.xuid)
                        .filter(|id| grant.roster.contains(id)),
                );
                self.roster = new.players.clone();
                self.start = Some((row.tick, recorded));
                continue;
            }
            let Some((_, start)) = &self.start else {
                self.flag(DiscrepancyCause::MissingStart, index, row.tick);
                continue;
            };
            let data: Value = serde_json::from_str(row.data.get()).expect("typed event");
            if row.kind == game::PLAYER_JOINED {
                let joined: game::PlayerJoined =
                    serde_json::from_value(data.clone()).expect("typed PlayerJoined");
                if start.game.sim.is_some()
                    && (!joined.player.is_bot() || joined.player.xuid.is_some())
                {
                    return Err(forbidden("simulation players must be local bots"));
                }
                check_roster(
                    live,
                    publisher,
                    grant,
                    id,
                    &start.game,
                    std::slice::from_ref(&joined.player),
                )?;
            }
            if self.ended.is_some() || game::validate(&row.kind, &data, &self.roster).is_err() {
                self.flag(DiscrepancyCause::InvalidEventSequence, index, row.tick);
                continue;
            }
            match row.kind.as_str() {
                game::PLAYER_JOINED => {
                    let p: game::PlayerJoined = serde_json::from_value(data).expect("validated");
                    self.participants
                        .extend(p.player.xuid.filter(|id| grant.roster.contains(id)));
                    self.roster.push(p.player);
                }
                game::PLAYER_LEFT => {
                    let p: game::PlayerLeft = serde_json::from_value(data).expect("validated");
                    self.roster.retain(|player| player.index != p.index);
                }
                game::GAME_ENDED => self.ended = Some(row.tick),
                _ => {}
            }
            if self.issue.is_some() {
                continue;
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

    fn flag(&mut self, cause: DiscrepancyCause, event_index: usize, tick: u64) {
        self.issue.get_or_insert(StatsDiscrepancy {
            cause,
            event_index,
            film_tick: Some(tick),
            telemetry_position: None,
        });
    }

    async fn flush(&mut self, store: &BlobStore, blob: &str) -> ApiResult<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&self.rows).expect("serializes");
        let page = Page {
            sha256: digest(&bytes),
            count: self.rows.len(),
            hash_key: true,
        };
        store
            .put(
                &page_key(blob, self.index.len, &page),
                bytes.into(),
                &Meta::default(),
            )
            .await
            .map_err(unavailable)?;
        self.index.push(store, blob, page).await?;
        self.bytes = 0;
        self.rows.clear();
        Ok(())
    }

    /// Validate the *whole* roster, event sequence and live prefix before
    /// recording any part of the new match. The caller holds host_game_writes.
    pub(super) async fn prepare(
        &mut self,
        live: &Live,
        id: &str,
        publisher: &Publisher,
        grant: &FilmReservation,
        store: &BlobStore,
        mut receipt: Value,
        started_ms: Option<u64>,
        hz: u16,
        v3: bool,
        page_budget: usize,
    ) -> ApiResult<Option<(Vec<NewEvent>, u64, bool)>> {
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
        // The archived game's kill log can be large. Only its start, status
        // and ledger cursor are needed to reconcile the paginated events.
        struct Existing {
            new: game::NewGame,
            started_at: String,
            position: u64,
            version: u64,
            ended: bool,
        }
        let existing = {
            let m = live.model.read();
            match m.games.get(id) {
                Some(g) if !publisher.may_write(g) => {
                    return Err(forbidden("the game belongs to another host"));
                }
                Some(g) => Some(Existing {
                    new: g.started.game.clone(),
                    started_at: g.started_at.clone(),
                    position: g.position,
                    version: g.recording.next_version,
                    ended: g.ended.is_some(),
                }),
                None => None,
            }
        };
        let version = existing.as_ref().map_or(0, |g| g.version);
        let cursor = super::import_stream(id);
        if !v3 {
            return if existing.is_some() {
                Ok(Some((
                    vec![live.server_event(cursor, HOST_FILM_STORED, receipt)],
                    version,
                    false,
                )))
            } else {
                Err(invalid("legacy films require an existing match"))
            };
        }
        if id != grant.canonical_film_id() {
            return Err(forbidden("the film is outside the assigned game"));
        }
        let missing = if self.start.is_none() {
            Some(DiscrepancyCause::MissingStart)
        } else if self.ended.is_none() {
            Some(DiscrepancyCause::MissingEnd)
        } else {
            let drops = &receipt["summary"]["dropped_samples"];
            [
                "stats_event",
                "stats_encoding",
                "stats_budget",
                "match_start",
                "tick_snapshot",
                "final_scoreboard",
                "kill",
                "kill_clock",
            ]
            .iter()
            .any(|key| drops[*key].as_u64().unwrap_or(0) != 0)
            .then_some(DiscrepancyCause::IncompleteObservations)
        };
        if let Some(problem) = self.issue.clone().or_else(|| {
            missing.map(|cause| StatsDiscrepancy {
                cause,
                event_index: if cause == DiscrepancyCause::MissingStart {
                    0
                } else {
                    self.event_count
                },
                film_tick: None,
                telemetry_position: None,
            })
        }) {
            if existing.is_none() && self.start.is_none() {
                grant.check_catalogue_map(receipt["summary"]["level"].as_str())?;
            }
            receipt["stats_discrepancy"] = json!(problem);
            if existing.is_none() {
                receipt["catalogue_host"] = json!(publisher.to_string());
                receipt["catalogue_started_ms"] =
                    json!(started_ms.unwrap_or_else(|| now().max(0) as u64 * 1000));
                receipt["catalogue_players"] = if self.start.is_some() {
                    json!(self.participants)
                } else {
                    json!(grant.roster)
                };
            }
            return Ok(Some((
                vec![live.server_event(cursor, HOST_FILM_STORED, receipt)],
                version,
                false,
            )));
        }
        let (origin, recorded) = self.start.as_ref().expect("complete film stats");
        let end = self.ended.expect("complete film stats");
        let mut new = recorded.game.clone();
        if new.playlist.is_none() {
            new.playlist = match &existing {
                Some(g) => g.new.playlist.clone(),
                None => live.playlist_name(new.playlist_id.as_deref()).await,
            };
        }
        let mut discrepancy = None;
        let mut matched = 0;
        let complete = existing.as_ref().is_some_and(|g| g.ended);
        if let Some(g) = &existing {
            if g.new != new {
                discrepancy = Some(StatsDiscrepancy {
                    cause: DiscrepancyCause::DifferentStart,
                    event_index: 0,
                    film_tick: Some(*origin),
                    telemetry_position: Some(g.position),
                });
            } else {
                let progress = self.comparison.get_or_insert_with(Comparison::default);
                if progress.start_position != g.position {
                    *progress = Comparison {
                        start_position: g.position,
                        after: g.position,
                        ..Default::default()
                    };
                }
                let mut prefix = Prefix::new(id, Some(progress.after));
                let limit = progress
                    .page
                    .saturating_add(page_budget)
                    .min(self.index.len);
                'pages: while progress.page < limit {
                    // Film-first or an interrupted live log need no more page reads
                    // once the live prefix is exhausted.
                    let Some(mut old) = prefix.next(live).await? else {
                        break;
                    };
                    let n = progress.page;
                    let page = self.index.page(store, blob, n).await?;
                    let rows = read_page(store, blob, n, &page).await?;
                    for (i, row) in rows.into_iter().skip(progress.row).enumerate() {
                        if i > 0 {
                            let Some(next) = prefix.next(live).await? else {
                                break 'pages;
                            };
                            old = next;
                        }
                        let canonical: Option<Event> = serde_json::from_value(json!({
                            "kind": old.body.kind, "data": old.body.data,
                        }))
                        .ok();
                        let data: Value =
                            serde_json::from_str(row.data.get()).expect("validated film event");
                        if canonical
                            .and_then(|e| e.telemetry())
                            .is_none_or(|(kind, body)| kind != row.kind || body != data)
                        {
                            let idle = old.body.kind == game::GAME_ENDED
                                && old.body.data["reason"] == "lost"
                                && old.body.author == live.identity.author();
                            discrepancy = Some(StatsDiscrepancy {
                                cause: if idle {
                                    DiscrepancyCause::IdleTimeout
                                } else {
                                    DiscrepancyCause::DifferentEvent
                                },
                                event_index: progress.matched + 1,
                                film_tick: Some(row.tick),
                                telemetry_position: Some(old.position),
                            });
                            break 'pages;
                        }
                        progress.row += 1;
                        progress.matched += 1;
                        progress.after = old.position;
                    }
                    progress.row = 0;
                    progress.page += 1;
                }
                matched = progress.matched;
                if discrepancy.is_none() {
                    if let Some(extra) = prefix.next(live).await? {
                        if progress.page < self.index.len {
                            // An unconsumed row will be read again from the last
                            // matched position after restoring the checkpoint.
                            return Ok(None);
                        }
                        discrepancy = Some(StatsDiscrepancy {
                            cause: DiscrepancyCause::TelemetryExtendsPastFilm,
                            event_index: matched + 1,
                            film_tick: None,
                            telemetry_position: Some(extra.position),
                        });
                    } else if complete && matched != self.event_count.saturating_sub(1) {
                        discrepancy = Some(StatsDiscrepancy {
                            cause: DiscrepancyCause::TelemetryEndedEarly,
                            event_index: matched + 1,
                            film_tick: None,
                            telemetry_position: None,
                        });
                    }
                }
            }
        }
        if let Some(problem) = &discrepancy {
            receipt["stats_discrepancy"] = json!(problem);
        }
        if complete || discrepancy.is_some() {
            return Ok(Some((
                vec![live.server_event(cursor, HOST_FILM_STORED, receipt)],
                version,
                false,
            )));
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
            index: Arc::new(self.index.clone()),
            pages: Arc::default(),
            next: 0,
            skip: matched,
            origin: *origin,
            hz,
            start,
            sealed,
        };
        events.push(live.server_event(super::import_stream(id), STARTED, json!(pending)));
        Ok(Some((events, version, true)))
    }
}

fn check_roster(
    live: &Live,
    publisher: &Publisher,
    grant: &FilmReservation,
    id: &str,
    new: &game::NewGame,
    players: &[game::Player],
) -> ApiResult<()> {
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
    let model = live.model.read();
    let before = model.games.get(id).map_or(u64::MAX, |g| g.position);
    publisher
        .check_players(&model, None, new.session_id.as_deref(), before, players)
        .map_err(invalid)
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

/// Every append includes the cursor and a checked ledger version. A crash,
/// double finalise or lost HTTP response can only resume at the first unapplied
/// event page. Workers and native sweepers also recover without a second upload.
pub(crate) async fn resume(live: &Live, id: &str, page_budget: usize) -> ApiResult<bool> {
    for _ in 0..page_budget {
        let _host_guard = live.host_game_writes.lock(id).await;
        let (pending, host, game_version) = {
            let m = live.model.read();
            let Some(p) = m.films.imports.get(id).cloned() else {
                return Ok(true);
            };
            let g = m
                .games
                .get(id)
                .ok_or_else(|| unavailable("missing import game"))?;
            (p, g.started.host.clone(), g.recording.next_version)
        };
        let store = &live
            .blobs()
            .ok_or_else(|| unavailable("no film store"))?
            .private;
        let blob = pending.receipt["blob"]
            .as_str()
            .ok_or_else(|| unavailable("no selected blob"))?;
        let n = pending.next;
        let page_and_rows = async {
            let page = pending.page(store, blob, n).await?;
            let rows = read_page(store, blob, n, &page).await?;
            Ok((page, rows))
        }
        .await;
        let (page, rows) = match page_and_rows {
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
        let last = n + 1 == pending.page_count();
        let (kind, data) = if last {
            (HOST_FILM_STORED, pending.receipt.clone())
        } else {
            (ADVANCED, json!({"game":id,"page":n,"rows":page.count}))
        };
        let cursor = super::import_stream(id);
        events.push(live.server_event(cursor.clone(), kind, data));
        let expected = [(stream, game_version), (cursor, n as u64 + 1)];
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
        if let Err(e) = store.delete(&page_key(blob, n, &page)).await {
            eprintln!("live: film stats page cleanup: {e}");
        }
        if last {
            live.rank_game(id).await?;
            return Ok(true);
        }
    }
    Ok(!live.model.read().films.imports.contains_key(id))
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
