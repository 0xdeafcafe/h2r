//! Web Theater. Films point to existing game recordings. Pins and clips
//! are server-signed events shared by the native and Worker stores.

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
const SAVED: &str = "FilmSaved";
const UNSAVED: &str = "FilmUnsaved";
const CLIP_CREATED: &str = "ClipCreated";
const CLIP_DELETED: &str = "ClipDeleted";
pub(crate) const KINDS: [&str; 4] = [SAVED, UNSAVED, CLIP_CREATED, CLIP_DELETED];

const TICKS_PER_SECOND: u64 = 30;
const MIN_CLIP_TICKS: u64 = 30 * TICKS_PER_SECOND;
const MAX_CLIP_TICKS: u64 = 300 * TICKS_PER_SECOND;
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

impl Clip {
    pub(crate) fn duration_s(&self) -> u64 {
        self.to_tick.saturating_sub(self.from_tick) / TICKS_PER_SECOND
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

#[derive(Default)]
pub(crate) struct Films {
    next_version: u64,
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
    fn pinned_by(&self, game: &Game, player: Xuid) -> bool {
        self.saved
            .get(&player)
            .is_some_and(|pins| pins.contains_key(&game.position))
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
    films.next_version = event.version + 1;
    match event.body.kind.as_str() {
        SAVED | UNSAVED => {
            let Ok(pin) = serde_json::from_value::<Pin>(event.body.data.clone()) else {
                return;
            };
            let Some(game) = model.games.get_mut(&pin.game) else {
                return;
            };
            if event.body.kind == SAVED {
                let pins = films.saved.entry(pin.by).or_default();
                if pins.insert(game.position, pin.game).is_none() {
                    game.recording.saved_by += 1;
                }
            } else if let Some(pins) = films.saved.get_mut(&pin.by) {
                if pins.remove(&game.position).is_some() {
                    game.recording.saved_by -= 1;
                }
                if pins.is_empty() {
                    films.saved.remove(&pin.by);
                }
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
    started_at: &'a str,
    duration_s: Option<u64>,
    result: Option<&'static str>,
}

impl<'a> Film<'a> {
    fn new(game: &'a Game, player: Xuid) -> Self {
        let start = &game.started.game;
        let duration_s = game.ended.as_ref().and_then(|(end, at)| {
            if let Some(tick) = start.tick {
                return Some(end.tick.saturating_sub(tick) / TICKS_PER_SECOND);
            }
            if end.reason == "lost" {
                return Some(0);
            }
            // Old GameStarted events did not record a tick.
            let wall = parse_time(at)? - parse_time(&game.started_at)?;
            Some(wall.whole_seconds().max(0) as u64)
        });
        Self {
            game_id: &game.started.game_id,
            map: &start.map,
            mode: &start.mode,
            variant: start.variant.as_deref(),
            started_at: &game.started_at,
            duration_s,
            result: game_result(game, player),
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

fn visible_game<'a>(model: &'a Model, id: &str, viewer: Viewer) -> Option<&'a Game> {
    model
        .games
        .get(id)
        .filter(|g| g.shows_to(viewer, |a, b| model.is_friends(a, b)))
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
            let items = model
                .games_newest()
                .filter(|g| g.started.game.sim.is_none())
                .filter(|g| g.xuids.contains(&player))
                .filter(|g| g.shows_to(viewer, |a, b| model.is_friends(a, b)))
                .take(RECENT)
                .map(|g| Film::new(g, player))
                .collect();
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
                .filter_map(|(&key, id)| Some((key, Film::new(shown(id)?, player))));
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
                    let film = Film::new(shown(&clip.game)?, player);
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
            if !game.xuids.contains(&by) {
                return Err(forbidden("only a player in the game can save it"));
            }
            if model.films.pinned_by(game, by) == save {
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
    if !(MIN_CLIP_TICKS..=MAX_CLIP_TICKS).contains(&duration) {
        return Err(invalid("a clip must be 30 seconds to 5 minutes long"));
    }
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
        visible_game(&model, &id, viewer).ok_or_else(|| not_found("no such game"))?;
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
