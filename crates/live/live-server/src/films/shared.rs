//! Account file shares. The signed reservation charges both immutable objects
//! before accepting any bytes. The game ledger never consumes these events.
mod library;
mod verify;
use super::*;
use crate::blobs::{BlobStore, Meta};
use axum::{body::Body, http::header};
pub(crate) use library::sweep;
pub(super) use library::{commit, evict, host_size, library};
use live_proto::film::{
    PART_BYTES, Summary,
    sharing::{ACCOUNT_QUOTA, Compat, Object, Upload, Visibility},
};
use sha2::{Digest, Sha256};

pub(super) const PURGED: &str = "FilmSharePurged";
pub(super) const REMOVED: &str = "FilmLibraryRemoved";
pub(super) const RESERVED: &str = "FilmShareReserved";
pub(super) const PUBLISHED: &str = "FilmSharePublished";
pub(super) const VISIBILITY: &str = "FilmShareVisibility";
pub(super) const DELETED: &str = "FilmShareDeleted";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Publication {
    compat: Compat,
    summary: Summary,
    ranges: WebRanges,
    ticks: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Share {
    pub(super) owner: Xuid,
    #[serde(default)]
    pub(super) pinned: bool,
    id: String,
    upload: Upload,
    #[serde(default)]
    publication: Option<Publication>,
    #[serde(default)]
    pub(super) position: u64,
    #[serde(default)]
    started_at: String,
}

impl Share {
    pub(super) fn visible(&self, reader: Option<Xuid>) -> bool {
        self.publication.is_some()
            && (reader == Some(self.owner)
                || match self.upload.visibility {
                    Visibility::Public => true,
                    Visibility::Link => reader.is_some(),
                    Visibility::Private => false,
                })
    }
    pub(super) fn listed(&self, reader: Option<Xuid>) -> bool {
        self.visible(reader)
            && (self.upload.visibility == Visibility::Public || reader == Some(self.owner))
    }
    pub(super) fn length(&self) -> u64 {
        self.publication.as_ref().map_or(0, |p| p.ticks.into())
    }
    pub(super) fn row(&self) -> Value {
        let p = self.publication.as_ref().expect("published film row");
        let hz = p.ranges.ticks_per_second.unwrap_or(30).max(1);
        json!({"game_id": self.id, "map":p.summary.level, "mode":p.summary.mode,
            "variant":p.summary.variant_name, "started_at":self.started_at, "ticks_per_second":hz,
            "duration_s":p.summary.duration_ticks / u64::from(hz), "length_ticks":p.ticks,
            "has_host_film":true, "shared":true, "summary":p.summary, "compat":p.compat,
            "saved":self.pinned, "visibility":self.upload.visibility})
    }
    fn object(&self, name: &str) -> ApiResult<&Object> {
        match name {
            "film" => Ok(&self.upload.film),
            "stats" => Ok(&self.upload.stats),
            _ => Err(invalid("object must be film or stats")),
        }
    }
    fn key(&self, name: &str) -> String {
        let prefix = if name == "stats" { "stats" } else { "films" };
        format!(
            "{prefix}/{}/{}.parts",
            self.id,
            self.object(name).expect("known object").sha256
        )
    }
    fn receipt(&self) -> Value {
        json!({"film":self.id, "url":format!("/theater/{}",self.id)})
    }
}

pub(super) fn project(films: &mut Films, event: &RecordedEvent) {
    let data = &event.body.data;
    match event.body.kind.as_str() {
        RESERVED => {
            if let Ok(mut share) = serde_json::from_value::<Share>(data.clone()) {
                share.position = event.position;
                share.started_at = event.body.at.clone();
                films.shared.insert(share.id.clone(), share);
            }
        }
        kind => {
            if let Some(id) = data["film"].as_str() {
                if kind == DELETED {
                    if let Some(s) = films.shared.remove(id) {
                        if let Some(pins) = films.saved.get_mut(&s.owner) {
                            pins.remove(&s.position);
                        }
                        films.garbage.insert(id.into(), s);
                    }
                } else if kind == PURGED {
                    films.garbage.remove(id);
                } else if kind == REMOVED {
                    if let Ok(by) = serde_json::from_value::<Xuid>(data["by"].clone()) {
                        films.removed.entry(by).or_default().insert(id.into());
                        if let Some(pins) = films.saved.get_mut(&by) {
                            pins.retain(|_, pin| pin != id);
                        }
                    }
                } else if let Some(share) = films.shared.get_mut(id) {
                    if kind == PUBLISHED {
                        share.publication =
                            serde_json::from_value(data["publication"].clone()).ok();
                    } else if let Ok(v) = serde_json::from_value(data["visibility"].clone()) {
                        share.upload.visibility = v;
                    }
                }
            }
        }
    }
}

pub(super) fn private_json(status: StatusCode, value: &impl Serialize) -> Response {
    let mut r = json(status, value);
    r.headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    r
}
fn unavailable(_: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Unavailable, "film store unavailable")
}
fn store(live: &Live) -> ApiResult<&BlobStore> {
    live.blobs()
        .map(|s| &s.private)
        .ok_or_else(|| unavailable("store"))
}
fn selected(live: &Live, id: &str) -> ApiResult<Share> {
    game_id(id)?;
    live.model
        .read()
        .films
        .shared
        .get(id)
        .cloned()
        .ok_or_else(|| not_found("no such file share"))
}
fn owner(live: &Live, headers: &HeaderMap, share: &Share) -> ApiResult<()> {
    let author = live.film_author(headers)?;
    if account(&author)? != share.owner {
        return Err(forbidden("only the film uploader can change it"));
    }
    Ok(())
}

pub(crate) async fn reserve(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let by = account(&live.film_author(&headers)?)?;
    let upload: Upload = serde_json::from_slice(&body_bytes(body)?)
        .map_err(|_| invalid("invalid film upload description"))?;
    if !upload.film.valid() || !upload.stats.valid() {
        return Err(invalid("invalid object sizes or SHA-256 part list"));
    }
    let _guard = live.film_writes.lock().await;
    let version = {
        let model = live.model.read();
        for share in model.films.shared.values().filter(|s| s.owner == by) {
            if share.upload.film == upload.film && share.upload.stats == upload.stats {
                return Ok(private_json(
                    if share.publication.is_some() {
                        StatusCode::CREATED
                    } else {
                        StatusCode::ACCEPTED
                    },
                    &share.receipt(),
                ));
            }
        }
        evict(&model, by, upload.film.size + upload.stats.size, true)?;
        model.films.next_version
    };
    let share = Share {
        id: format!("share-{}", random_id()),
        owner: by,
        pinned: false,
        upload,
        publication: None,
        position: 0,
        started_at: String::new(),
    };
    live.append_film(version, RESERVED, json!(&share)).await?;
    Ok(private_json(StatusCode::ACCEPTED, &share.receipt()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Part {
    object: String,
    part: usize,
}

pub(crate) async fn part(
    State(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<Part>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<StatusCode> {
    let _guard = live.film_uploads.lock(&id).await;
    let share = selected(&live, &id)?;
    owner(&live, &headers, &share)?;
    let obj = share.object(&query.object)?;
    let hash = obj
        .parts
        .get(query.part)
        .ok_or_else(|| invalid("invalid film part number"))?;
    let bytes = body_bytes(body)?;
    if bytes.len() as u64 != (obj.size - query.part as u64 * PART_BYTES).min(PART_BYTES)
        || format!("{:x}", Sha256::digest(&bytes)) != *hash
    {
        return Err(invalid("film part length or digest mismatch"));
    }
    if share.publication.is_none() {
        let key = parts::part_key(&share.key(&query.object), query.part as u64, hash);
        store(&live)?
            .put(&key, bytes, &Meta::default())
            .await
            .map_err(unavailable)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Continuation {
    #[serde(rename = "continue")]
    more: u8,
}

pub(crate) async fn complete(
    State(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<Continuation>,
) -> ApiResult<Response> {
    if query.more != 1 {
        return Err(invalid("invalid upload continuation"));
    }
    let _guard = live.film_uploads.lock(&id).await;
    let share = selected(&live, &id)?;
    owner(&live, &headers, &share)?;
    if share.publication.is_none() {
        let Some(publication) = verify::advance(store(&live)?, &share).await? else {
            return Ok(private_json(StatusCode::ACCEPTED, &share.receipt()));
        };
        let _guard = live.film_writes.lock().await;
        let (version, evictions) = {
            let model = live.model.read();
            if !model.films.shared.contains_key(&id) {
                return Err(not_found("upload was removed"));
            }
            (
                model.films.next_version,
                evict(
                    &model,
                    share.owner,
                    share.upload.film.size + share.upload.stats.size,
                    false,
                )?,
            )
        };
        commit(
            &live,
            version,
            evictions,
            PUBLISHED,
            json!({"film":id,"publication":publication}),
        )
        .await?;
        store(&live)?
            .delete(&verify::cursor_key(&share))
            .await
            .map_err(unavailable)?;
    }
    Ok(private_json(StatusCode::CREATED, &share.receipt()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    visibility: Visibility,
}

pub(crate) async fn visibility(
    State(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<StatusCode> {
    let _guard = live.film_writes.lock().await;
    let share = selected(&live, &id)?;
    owner(&live, &headers, &share)?;
    let change: Change =
        serde_json::from_slice(&body_bytes(body)?).map_err(|_| invalid("invalid visibility"))?;
    let version = live.model.read().films.next_version;
    live.append_film(
        version,
        VISIBILITY,
        json!({"film":id,"visibility":change.visibility}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete(
    State(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let author = live.film_author(&headers)?;
    let _guard = live.film_uploads.lock(&id).await;
    let exists = live.model.read().films.shared.contains_key(&id);
    if !exists {
        return library::remove_game(&live, &author, &id).await;
    }
    let share = selected(&live, &id)?;
    if !matches!(&author, Author::Account {xuid,..} if *xuid == share.owner)
        && !live.viewer_of(&author).is_admin()
    {
        return Err(forbidden("only the uploader or admin can delete a film"));
    }
    let _guard = live.film_writes.lock().await;
    let version = live.model.read().films.next_version;
    live.append_film(version, DELETED, json!({"film":id}))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn read_object(
    live: &Live,
    headers: &HeaderMap,
    share: Share,
    object: Option<&str>,
    query: &ranges::RangeQuery,
) -> ApiResult<Response> {
    let (_, player) = live.read_viewer_account(bearer(headers))?;
    if !share.visible(player) {
        return Err(not_found("no such film"));
    }
    let p = share.publication.as_ref().expect("visible share");
    let ranged = headers.contains_key(header::RANGE);
    let name = object.unwrap_or(if ranged { "stats" } else { "film" });
    let obj = share.object(name)?;
    if name == "film" && player.is_none() {
        return Err(crate::app::unauthorized("sign in to download a film"));
    }
    let store = store(live)?;
    let key = share.key(name);
    if name == "film" && !ranged {
        let (size, stream) = parts::get_stream(store, &key)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| not_found("no such film"))?;
        return Ok(Response::builder()
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CACHE_CONTROL, "private, no-store")
            .header(header::CONTENT_LENGTH, size)
            .body(Body::from_stream(stream))
            .unwrap());
    }
    let denied = || {
        ranges::range_response(
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
            obj.size,
            Bytes::new(),
            false,
            None,
        )
    };
    let Some(range) = ranges::parse_range(headers, obj.size) else {
        return Ok(denied());
    };
    if query.rev.as_ref().is_some_and(|rev| rev != &obj.sha256) {
        return Err(not_found("no such film revision"));
    }
    let request_span = headers
        .get(header::RANGE)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("bytes="));
    if query.rev.is_some() != query.span.is_some()
        || query
            .span
            .as_ref()
            .is_some_and(|span| Some(span.as_str()) != request_span)
    {
        return Ok(denied());
    }
    let bytes = if name == "stats" {
        let Some((b, _)) = p
            .ranges
            .read(&live.web_film_index, store, &key, &range)
            .await?
        else {
            return Ok(denied());
        };
        b
    } else {
        if range.end - range.start > PART_BYTES {
            return Ok(denied());
        }
        if headers.get(header::IF_MATCH).is_some_and(|tag| {
            tag.to_str().ok() != Some(format!("\"film-{}\"", obj.sha256).as_str())
        }) {
            return Ok(Response::builder()
                .status(StatusCode::PRECONDITION_FAILED)
                .body(Body::empty())
                .unwrap());
        }
        parts::get_range(store, &key, range.clone())
            .await
            .map_err(unavailable)?
            .ok_or_else(|| not_found("no such film"))?
            .0
    };
    Ok(ranges::range_response(
        StatusCode::PARTIAL_CONTENT,
        Some(&range),
        obj.size,
        bytes,
        false,
        Some(&obj.sha256),
    ))
}
