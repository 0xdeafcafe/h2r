//! Theater lists archived games and tracks their exact host films. Film
//! availability, pins and clips are projected in both homes from signed events.

mod catalogue;
use catalogue::{Source, Standalone};
mod import;
pub(crate) use import::resume as resume_import;
mod inspect;
mod legacy;
pub(crate) use legacy::advance as continue_staged;
pub(crate) mod parts;
mod ranges;
pub(crate) use ranges::{WebIndexCache, WebRanges, read_film};
mod upload;
pub(crate) use upload::{cleanup_published, upload_host_film};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hash;
use std::ops::Bound::{self, Excluded, Unbounded};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::game::{self, Winner};
use live_proto::sign::{parse_time, random_id};
use live_proto::{Author, ErrorCode, Page, Xuid, lxm};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{ApiError, Live, forbidden, invalid, not_found};
use crate::http::{bearer, body_bytes, game_id, json, paging};
use crate::model::{Game, State as Model};
use crate::sessions::Viewer;
use crate::store::RecordedEvent;

pub(crate) const STREAM: &str = "theater";
pub(crate) const IMPORT_PREFIX: &str = "film-import-";
pub(crate) fn import_stream(game: &str) -> String {
    format!("{IMPORT_PREFIX}{game}")
}
const SAVED: &str = "FilmSaved";
const UNSAVED: &str = "FilmUnsaved";
const CLIP_CREATED: &str = "ClipCreated";
const CLIP_DELETED: &str = "ClipDeleted";
const HOST_FILM_STORED: &str = "HostFilmStored";
const HOST_FILM_CLEANED: &str = "HostFilmCleaned";
const HOST_FILM_CLEANUP_REQUESTED: &str = "HostFilmCleanupRequested";
pub(crate) const KINDS: [&str; 11] = [SAVED, UNSAVED, CLIP_CREATED, CLIP_DELETED, HOST_FILM_STORED, import::STARTED, import::ADVANCED, HOST_FILM_CLEANED, legacy::QUEUED, legacy::FINISHED, HOST_FILM_CLEANUP_REQUESTED];
pub(crate) const HOST_FILM_PREFIX: &str = "films/";

const LEGACY_TICKS_PER_SECOND: u16 = 30;
const MAX_TITLE_CHARS: usize = 60;
const DAILY_CLIPS: usize = 20;
const DAY_MS: i64 = 86_400_000;
const RECENT: usize = 25;

type Shared = State<Arc<Live>>;
type ApiResult<T> = Result<T, ApiError>;
type Params = Query<HashMap<String, String>>;

#[derive(Serialize, Deserialize)]
struct Pin {
    game: String,
    by: Xuid,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Clip {
    id: String,
    pub game: String,
    from_tick: u64,
    to_tick: u64,
    pub title: String,
    by: Xuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pov: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    camera: Option<String>,
    created_ms: i64,
}

fn film_tick_rate(ranges: Option<&WebRanges>) -> u16 {
    ranges
        .and_then(|ranges| ranges.ticks_per_second)
        .filter(|&rate| rate > 0)
        .unwrap_or(LEGACY_TICKS_PER_SECOND)
}

impl Clip {
    pub(crate) fn duration_s(&self, game: &Game) -> u64 {
        self.to_tick.saturating_sub(self.from_tick) / u64::from(film_tick_rate(game.film_web_ranges.as_ref()))
    }
}

#[derive(Deserialize)]
struct ClipRequest {
    from_tick: u64,
    to_tick: u64,
    title: String,
    pov: Option<u32>,
    camera: Option<String>,
}

/// Concurrent live hosts only wait for a finalising film of their own game.
/// The durable import-stream version arbitrates separate server processes.
#[derive(Default)]
pub(crate) struct HostGameWrites {
    games: std::sync::Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

impl HostGameWrites {
    pub(crate) async fn lock(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut games = self.games.lock().expect("host event locks");
            if let Some(lock) = games.get(id).and_then(std::sync::Weak::upgrade) {
                lock
            } else {
                games.retain(|_, lock| lock.strong_count() != 0);
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                games.insert(id.into(), Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }
}

#[derive(Default)]
pub(crate) struct Films {
    pub(crate) imports: BTreeMap<String, import::Pending>,
    last_import: std::sync::Mutex<String>,
    next_version: u64,
    standalone: HashMap<String, Standalone>,
    standalone_order: BTreeMap<u64, String>,
    // Signed per-game journal versions survive both successful publication and
    // the later housekeeping event (which has its own stream).
    import_versions: HashMap<String, u64>,
    cleanup: BTreeSet<String>,
    cleanup_versions: HashMap<String, u64>,
    legacy: legacy::Queue,
    last_cleanup: std::sync::Mutex<String>,
    // By player, then by game start position (the same order as recent).
    saved: HashMap<Xuid, BTreeMap<u64, String>>,
    clips: BTreeMap<u64, Clip>,
    by_id: HashMap<String, u64>,
    by_game: HashMap<String, BTreeSet<u64>>,
    by_author: HashMap<Xuid, BTreeSet<u64>>,
    // Most recent UTC day and number of creates, including deleted clips.
    quota: HashMap<Xuid, (i64, usize)>,
}

impl Films {
    pub(crate) fn import_version(&self, id: &str) -> u64 {
        self.import_versions.get(id).copied().unwrap_or(0)
    }

    pub(crate) fn next_import(&self) -> Option<String> {
        let mut last = self.last_import.lock().expect("film import cursor");
        let (id, _) = self.imports.range((Excluded(last.clone()), Unbounded)).next()
            .or_else(|| self.imports.first_key_value())?;
        last.clone_from(id);
        Some(id.clone())
    }

    pub(crate) fn film_work_due(&self) -> Option<i64> {
        self.legacy.due().into_iter().chain(
            (!self.cleanup.is_empty() || !self.imports.is_empty()).then(|| crate::app::now() + 5)
        ).min()
    }

    pub(crate) fn ingesting(&self, id: &str) -> bool {
        self.imports.contains_key(id) || self.legacy.contains(id)
    }

    /// Round-robin keeps a broken object or an unavailable bucket from
    /// starving the other games which still have temporary objects.
    pub(crate) fn next_cleanup(&self) -> Option<String> {
        let mut last = self.last_cleanup.lock().expect("film cleanup cursor");
        let id = self
            .cleanup
            .range((Excluded(last.clone()), Unbounded))
            .next()
            .or_else(|| self.cleanup.first())?
            .clone();
        last.clone_from(&id);
        Some(id)
    }

    fn pinned_by(&self, game: Source<'_>, player: Xuid) -> bool {
        self.saved
            .get(&player)
            .is_some_and(|pins| pins.contains_key(&game.position()))
    }

    fn add_clip(&mut self, position: u64, clip: Clip) {
        if self.by_id.contains_key(&clip.id) {
            return;
        }
        let day = clip.created_ms.div_euclid(DAY_MS);
        let count = self.quota.entry(clip.by).or_insert((day, 0));
        if count.0 != day {
            *count = (day, 0);
        }
        count.1 += 1;
        self.by_id.insert(clip.id.clone(), position);
        self.by_game
            .entry(clip.game.clone())
            .or_default()
            .insert(position);
        self.by_author.entry(clip.by).or_default().insert(position);
        self.clips.insert(position, clip);
    }

    fn delete_clip(&mut self, id: &str) {
        let Some(position) = self.by_id.remove(id) else {
            return;
        };
        let clip = self.clips.remove(&position).expect("indexed clip");
        remove_position(&mut self.by_game, &clip.game, position);
        remove_position(&mut self.by_author, &clip.by, position);
    }

    fn clip(&self, id: &str) -> Option<&Clip> {
        self.by_id.get(id).and_then(|p| self.clips.get(p))
    }
}

fn remove_position<K: Eq + Hash>(index: &mut HashMap<K, BTreeSet<u64>>, key: &K, position: u64) {
    if let Some(positions) = index.get_mut(key) {
        positions.remove(&position);
        if positions.is_empty() {
            index.remove(key);
        }
    }
}

pub(crate) fn project(model: &mut Model, event: &RecordedEvent) {
    let films = &mut model.films;
    if event.body.stream == STREAM {
        films.next_version = event.version + 1;
    } else if let Some(id) = event.body.stream.strip_prefix(IMPORT_PREFIX)
        && game::valid_id(id)
    {
        films.import_versions.insert(id.to_owned(), event.version + 1);
    }
    match event.body.kind.as_str() {
        legacy::QUEUED | legacy::FINISHED => films.legacy.project(event),
        import::STARTED => {
            if let Ok(p) = serde_json::from_value::<import::Pending>(event.body.data.clone())
                && let Some(id) = p.receipt["game"].as_str()
            {
                films.legacy.published(id);
                films.imports.insert(id.to_owned(), p);
            }
        }
        import::ADVANCED => {
            if let Some(id) = event.body.data["game"].as_str()
                && let Some(p) = films.imports.get_mut(id)
                && event.body.data["page"].as_u64() == Some(p.next as u64)
            {
                p.advance(event.body.data["rows"].as_u64());
            }
        }
        HOST_FILM_STORED => {
            let Some(id) = event.body.data["game"].as_str() else { return };
            films.imports.remove(id);
            films.legacy.published(id);
            films.cleanup.insert(id.to_owned());
            if let Some(game) = model.games.get_mut(id) {
                game.has_host_film = true;
                game.film_blob = event.body.data["blob"].as_str().map(str::to_owned);
                game.film_web_ranges = event.body.data.get("web_ranges")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                game.film_summary = event.body.data.get("summary")
                    .and_then(|s| serde_json::from_value(s.clone()).ok());
                if let Some(summary) = &mut game.film_summary {
                    live_proto::film::privacy::public_summary(summary);
                }
                game.film_stats_discrepancy = event.body.data.get("stats_discrepancy")
                    .and_then(|s| serde_json::from_value(s.clone()).ok());
            } else if !films.standalone.contains_key(id)
                && let Some(entry) = Standalone::from_receipt(event)
            {
                films.standalone_order.insert(event.position, id.to_owned());
                films.standalone.insert(id.to_owned(), entry);
            }
        }
        HOST_FILM_CLEANED | HOST_FILM_CLEANUP_REQUESTED => {
            if let Some(id) = event.body.data["game"].as_str() {
                films.cleanup_versions.insert(id.to_owned(), event.version + 1);
                if event.body.kind == HOST_FILM_CLEANED {
                    films.cleanup.remove(id);
                } else {
                    films.cleanup.insert(id.to_owned());
                }
            }
        }
        SAVED | UNSAVED => {
            let Ok(pin) = serde_json::from_value::<Pin>(event.body.data.clone()) else {
                return;
            };
            let save = event.body.kind == SAVED;
            let position = if let Some(game) = model.games.get_mut(&pin.game) {
                if save == films.saved.get(&pin.by).is_some_and(|pins| pins.contains_key(&game.position)) {
                    return;
                }
                if save { game.recording.saved_by += 1 } else { game.recording.saved_by -= 1 }
                game.position
            } else if let Some(entry) = films.standalone.get_mut(&pin.game) {
                if save { entry.saved_by.insert(pin.by); } else { entry.saved_by.remove(&pin.by); }
                entry.position
            } else {
                return;
            };
            if save {
                films.saved.entry(pin.by).or_default().insert(position, pin.game);
            } else if let Some(pins) = films.saved.get_mut(&pin.by) {
                pins.remove(&position);
                if pins.is_empty() { films.saved.remove(&pin.by); }
            }
        }
        CLIP_CREATED => {
            if let Ok(clip) = serde_json::from_value::<Clip>(event.body.data.clone()) {
                films.add_clip(event.position, clip);
            }
        }
        CLIP_DELETED => {
            if let Some(id) = event.body.data["id"].as_str() {
                films.delete_clip(id);
            }
        }
        _ => {}
    }
}

#[derive(Serialize)]
struct Film<'a> {
    game_id: &'a str,
    map: &'a str,
    mode: &'a str,
    variant: Option<&'a str>,
    kind: Option<&'a str>,
    playlist: Option<&'a str>,
    difficulty: Option<&'a str>,
    online: Option<bool>,
    players: Vec<&'a game::Player>,
    scoreboard: &'a [game::PlayerScore],
    started_at: &'a str,
    start_tick: Option<u64>,
    duration_s: Option<u64>,
    ticks_per_second: u16,
    result: Option<&'static str>,
    has_host_film: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a live_proto::film::Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stats_discrepancy: Option<&'a live_proto::film::StatsDiscrepancy>,
}

impl<'a> Film<'a> {
    fn new(game: &'a Game, player: Xuid) -> Self {
        let start = &game.started.game;
        let ticks_per_second = film_tick_rate(game.film_web_ranges.as_ref());
        let duration_s = game.ended.as_ref().and_then(|(end, at)| {
            if let Some(tick) = start.tick {
                return Some(end.tick.saturating_sub(tick) / u64::from(ticks_per_second));
            }
            if end.reason == "lost" {
                return Some(0);
            }
            // Old GameStarted events did not record a tick.
            let wall = parse_time(at)? - parse_time(&game.started_at)?;
            Some(wall.whole_seconds().max(0) as u64)
        });
        let mut players: Vec<_> = game.players.iter().collect();
        for player in &start.players {
            if !players.iter().any(|p| p.index == player.index) {
                players.push(player);
            }
        }
        Self {
            game_id: &game.started.game_id,
            map: &start.map,
            mode: &start.mode,
            variant: start.variant.as_deref(),
            kind: Some(start.kind()),
            playlist: start.playlist.as_deref(),
            difficulty: start.difficulty.as_deref(),
            online: start.online,
            players,
            scoreboard: &game.scoreboard,
            started_at: &game.started_at,
            start_tick: start.tick,
            duration_s: game.film_stats_discrepancy.as_ref()
                .and(game.film_summary.as_ref())
                .map(|s| s.duration_ticks / u64::from(ticks_per_second))
                .or(duration_s),
            ticks_per_second,
            result: game.film_stats_discrepancy.is_none().then(|| game_result(game, player)).flatten(),
            has_host_film: game.has_host_film,
            summary: game.film_summary.as_ref(),
            stats_discrepancy: game.film_stats_discrepancy.as_ref(),
        }
    }
}

fn game_result(game: &Game, player: Xuid) -> Option<&'static str> {
    let (end, _) = game.ended.as_ref()?;
    if game.started.game.kind() == "campaign" || !game.xuids.contains(&player) {
        return None;
    }
    let active = game.players.iter().find(|p| p.xuid == Some(player));
    let wins = match end.winner.as_ref() {
        Some(Winner::Team { team }) => {
            let team_of_player = active
                .map(|p| p.team)
                .or_else(|| game.departed_teams.get(&player).copied());
            team_of_player? == *team
        }
        Some(Winner::Player { player }) => active.is_some_and(|p| p.index == *player),
        None => return (end.reason == "finished").then_some("draw"),
    };
    Some(if wins { "win" } else { "loss" })
}

#[derive(Serialize)]
struct ClipWithFilm<'a> {
    clip: &'a Clip,
    film: Film<'a>,
}

fn visible_game<'a>(model: &'a Model, id: &str, viewer: Viewer) -> Option<Source<'a>> {
    model.games.get(id).map(Source::Game)
        .or_else(|| model.films.standalone.get(id).map(Source::Standalone))
        .filter(|s| s.visible(model, viewer))
}

pub(crate) fn visible_clip<'a>(model: &'a Model, id: &str, viewer: Viewer) -> Option<&'a Clip> {
    let clip = model.films.clip(id)?;
    visible_game(model, &clip.game, viewer)?;
    Some(clip)
}

fn before(cursor: u64) -> Bound<u64> {
    if cursor == 0 {
        Unbounded
    } else {
        Excluded(cursor)
    }
}

fn page<T>(mut rows: impl Iterator<Item = (u64, T)>, limit: usize, position: u64) -> Page<T> {
    let mut items = Vec::new();
    let mut last = 0;
    for (key, row) in rows.by_ref().take(limit) {
        last = key;
        items.push(row);
    }
    Page {
        items,
        next: rows.next().map(|_| last.to_string()),
        position,
    }
}

pub(crate) async fn list(
    State(live): Shared,
    headers: HeaderMap,
    Query(params): Params,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    let player = params
        .get("player")
        .and_then(|s| Xuid::parse(s).ok())
        .ok_or_else(|| invalid("player must be a XUID"))?;
    let position = live.position();
    let model = live.model.read();
    let shown = |id: &str| visible_game(&model, id, viewer);
    match params.get("kind").map(String::as_str) {
        Some("recent") => {
            let items = catalogue::recent(&model, player, viewer);
            Ok(json(
                StatusCode::OK,
                &Page {
                    items,
                    next: None,
                    position,
                },
            ))
        }
        Some("saved") => {
            let paging = paging(&params)?;
            let before = before(paging.after);
            let rows = model
                .films
                .saved
                .get(&player)
                .into_iter()
                .flat_map(|pins| pins.range((Unbounded, before)).rev())
                .filter_map(|(&key, id)| Some((key, shown(id)?.listing(player))));
            Ok(json(StatusCode::OK, &page(rows, paging.limit, position)))
        }
        Some("clips") => {
            let paging = paging(&params)?;
            let before = before(paging.after);
            let rows = model
                .films
                .by_author
                .get(&player)
                .into_iter()
                .flat_map(|clips| clips.range((Unbounded, before)).rev())
                .filter_map(|key| {
                    let clip = &model.films.clips[key];
                    let film = shown(&clip.game)?.listing(player);
                    Some((*key, ClipWithFilm { clip, film }))
                });
            Ok(json(StatusCode::OK, &page(rows, paging.limit, position)))
        }
        _ => Err(invalid("kind must be recent, saved or clips")),
    }
}

impl Live {
    fn film_author(&self, headers: &HeaderMap) -> ApiResult<Author> {
        let author = self.authenticate(bearer(headers), lxm::FILMS_WRITE)?;
        self.rate_limit(&author, lxm::FILMS_WRITE)?;
        Ok(author)
    }

    async fn append_film(&self, version: u64, kind: &str, data: Value) -> ApiResult<()> {
        let event = self.server_event(STREAM.into(), kind, data);
        // The version also protects against a committed write whose handler
        // timed out or was cancelled before the model caught up.
        let recorded = self
            .store
            .append(vec![event], &[(STREAM.into(), version)])
            .await?;
        self.wait_applied(recorded[0].position).await
    }

    async fn set_pin(&self, headers: &HeaderMap, id: String, save: bool) -> ApiResult<StatusCode> {
        let author = self.film_author(headers)?;
        let by = account(&author)?;
        game_id(&id)?;
        let _guard = self.film_writes.lock().await;
        let viewer = self.viewer_of(&author);
        let version = {
            let model = self.model.read();
            let game =
                visible_game(&model, &id, viewer).ok_or_else(|| not_found("no such game"))?;
            let pinned = model.films.pinned_by(game, by);
            if !game.players().contains(&by) && (save || !pinned) {
                return Err(forbidden("only a player in the game can save it"));
            }
            if pinned == save {
                return Ok(StatusCode::NO_CONTENT);
            }
            model.films.next_version
        };
        let pin = Pin { game: id, by };
        self.append_film(version, if save { SAVED } else { UNSAVED }, json!(pin))
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }
}

fn account(author: &Author) -> ApiResult<Xuid> {
    match author {
        Author::Account { xuid, .. } => Ok(*xuid),
        Author::Server(_) => Err(forbidden("a player account is required")),
    }
}

pub(crate) fn host_film_key(id: &str) -> String {
    format!("{HOST_FILM_PREFIX}{id}.h2film")
}

/// Exact playback includes private machine and network state. A catalogue
/// reservation can list the film before a real start establishes who played.
async fn download_host_film(live: &Live, headers: &HeaderMap, id: &str) -> ApiResult<Response> {
    use axum::{body::Body, http::header};

    let author = live.authenticate(bearer(headers), lxm::READ)?;
    let player = account(&author)?;
    let viewer = live.viewer_of(&author);
    let key = {
        let model = live.model.read();
        let Some(Source::Game(g)) = visible_game(&model, id, viewer) else {
            return Err(not_found("no host film for this account"));
        };
        if !g.has_host_film || !g.xuids.contains(&player) {
            return Err(not_found("no host film for this account"));
        }
        g.film_blob.clone().unwrap_or_else(|| host_film_key(id))
    };
    let store = live.blobs().ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "the film store is unavailable"))?;
    let (size, stream) = parts::get_stream(&store.private, &key)
        .await
        .map_err(|error| {
            eprintln!("live: film {id}: {error}");
            ApiError::new(ErrorCode::Unavailable, "the film store cannot read")
        })?
        .filter(|(size, _)| (1..=live_proto::film::MAX_UPLOAD_BYTES).contains(size))
        .ok_or_else(|| not_found("the host film is not stored"))?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::CONTENT_LENGTH, size.to_string())
        .body(Body::from_stream(stream))
        .expect("valid host film response headers"))
}

pub(crate) async fn save(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    live.set_pin(&headers, id, true).await
}

pub(crate) async fn unsave(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    live.set_pin(&headers, id, false).await
}

pub(crate) async fn game_clips(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Params,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    game_id(&id)?;
    let paging = paging(&params)?;
    let before = before(paging.after);
    let position = live.position();
    let model = live.model.read();
    visible_game(&model, &id, viewer).ok_or_else(|| not_found("no such game"))?;
    let rows = model
        .films
        .by_game
        .get(&id)
        .into_iter()
        .flat_map(|clips| clips.range((Unbounded, before)).rev())
        .map(|key| (*key, &model.films.clips[key]));
    Ok(json(StatusCode::OK, &page(rows, paging.limit, position)))
}

fn clip_title(raw: &str) -> ApiResult<String> {
    let text: String = raw
        .chars()
        .filter(|&c| {
            (!c.is_control() || c.is_whitespace())
                && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !(1..=MAX_TITLE_CHARS).contains(&title.chars().count()) {
        return Err(invalid("title must be 1 to 60 characters after sanitising"));
    }
    Ok(title)
}

pub(crate) async fn create_clip(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let author = live.film_author(&headers)?;
    let by = account(&author)?;
    game_id(&id)?;
    let body: ClipRequest =
        serde_json::from_slice(&body_bytes(body)?).map_err(|e| invalid(format!("clip: {e}")))?;
    let title = clip_title(&body.title)?;
    let duration = body.to_tick.saturating_sub(body.from_tick);
    if body.to_tick > (1_u64 << 53) - 1 {
        return Err(invalid("clip ticks must be safe JSON integers"));
    }
    if body.camera.as_deref().is_some_and(|c| !game::valid_id(c)) {
        return Err(invalid(
            "camera must be a name of 1 to 64 letters, digits, underscores or hyphens",
        ));
    }
    let _guard = live.film_writes.lock().await;
    let viewer = live.viewer_of(&author);
    let created_ms = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let version = {
        let model = live.model.read();
        let game = visible_game(&model, &id, viewer).ok_or_else(|| not_found("no such game"))?;
        let rate = u64::from(film_tick_rate(game.ranges()));
        if !(30 * rate..=300 * rate).contains(&duration) {
            return Err(invalid("a clip must be 30 seconds to 5 minutes long"));
        }
        let day = created_ms.div_euclid(DAY_MS);
        if model
            .films
            .quota
            .get(&by)
            .is_some_and(|&(d, n)| d == day && n >= DAILY_CLIPS)
        {
            return Err(ApiError::new(
                ErrorCode::RateLimited,
                "a player can create 20 clips per UTC day",
            ));
        }
        model.films.next_version
    };
    let clip = Clip {
        id: random_id(),
        game: id,
        from_tick: body.from_tick,
        to_tick: body.to_tick,
        title,
        by,
        pov: body.pov,
        camera: body.camera,
        created_ms,
    };
    live.append_film(version, CLIP_CREATED, json!(&clip))
        .await?;
    Ok(json(StatusCode::CREATED, &clip))
}

pub(crate) async fn get_clip(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    let model = live.model.read();
    let clip = visible_clip(&model, &id, viewer).ok_or_else(|| not_found("no such clip"))?;
    Ok(json(StatusCode::OK, clip))
}

pub(crate) async fn delete_clip(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let author = live.film_author(&headers)?;
    let _guard = live.film_writes.lock().await;
    let viewer = live.viewer_of(&author);
    let version = {
        let model = live.model.read();
        let clip = visible_clip(&model, &id, viewer).ok_or_else(|| not_found("no such clip"))?;
        let owns_clip = matches!(&author, Author::Account { xuid, .. } if *xuid == clip.by);
        if !owns_clip && !viewer.is_admin() {
            return Err(forbidden(
                "only the clip's author or an admin can delete it",
            ));
        }
        model.films.next_version
    };
    live.append_film(
        version,
        CLIP_DELETED,
        json!({"id": id, "by": author.to_string()}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
