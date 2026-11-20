use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Method, StatusCode, header},
};
use futures_util::StreamExt;
use live_proto::film::{MAX_UPLOAD_BYTES, PART_BYTES, SINGLE_UPLOAD_BYTES};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{ApiResult, HOST_FILM_STORED, Shared, import::Spool, inspect::Inspector};
use crate::app::{ApiError, Live, forbidden, invalid, not_found};
use crate::blobs::{BlobError, BlobStore, BlobWriter, Meta};
use crate::host_identity::{HostClaims, TELEMETRY_INGEST, lease_id};
use crate::http::{bearer, game_id};
use live_proto::ErrorCode;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadQuery {
    part: Option<u64>,
    complete: Option<u64>,
    sha256: Option<String>,
}

fn staging_key(id: &str, part: u64) -> String {
    format!("film-staging/{id}/{part}")
}

fn blob_error(error: BlobError) -> ApiError {
    eprintln!("live: film store: {error}");
    ApiError::new(ErrorCode::Unavailable, "the film store is unavailable")
}

fn too_large() -> ApiError {
    ApiError::new(
        ErrorCode::TooLarge,
        "host film upload exceeds the size limit",
    )
}

fn version(live: &Live, id: &str, lease: &str, grant: &HostClaims) -> ApiResult<Option<u64>> {
    let model = live.model.read();
    match model.games.get(id) {
        Some(game) if lease_id(&game.started.host) != Some(lease) => {
            Err(forbidden("the game belongs to another host"))
        }
        Some(game) if game.has_host_film => Ok(None),
        Some(_) => Ok(Some(model.films.next_version)),
        None if id == grant.canonical_film_id() => Ok(Some(model.films.next_version)),
        None => Err(not_found("no such assigned game")),
    }
}

pub(crate) async fn upload_host_film(
    State(live): Shared,
    headers: HeaderMap,
    method: Method,
    Path(id): Path<String>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> ApiResult<StatusCode> {
    let grant = live.host_claims(bearer(&headers), TELEMETRY_INGEST)?;
    let publisher = grant.publisher();
    let lease = publisher.host_lease_id().expect("verified host");
    game_id(&id)?;
    if version(&live, &id, lease, &grant)?.is_none() {
        live.rank_game(&id).await?;
        cleanup_published(&live, &id).await;
        return Ok(StatusCode::NO_CONTENT);
    }
    if live.model.read().films.imports.contains_key(&id) {
        super::resume_import(&live, &id, true).await?;
        cleanup_published(&live, &id).await;
        return Ok(StatusCode::NO_CONTENT);
    }
    let store = &live
        .blobs()
        .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "the film store is unavailable"))?
        .private;
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());

    let part = query.part;
    if (method == Method::PUT) != part.is_some() {
        return Err(invalid(
            "use PUT for a numbered film part and POST to publish",
        ));
    }
    let complete = match (part, query.complete, query.sha256) {
        (None, None, None) => None,
        (Some(n), None, None) if n < MAX_UPLOAD_BYTES.div_ceil(PART_BYTES) => None,
        (None, Some(size), Some(hash))
            if size > 0
                && size <= MAX_UPLOAD_BYTES
                && hash.len() == 64
                && hash.bytes().all(|c| c.is_ascii_hexdigit()) =>
        {
            Some((size, hash))
        }
        _ => return Err(invalid("invalid host film upload parameters")),
    };
    let limit = if complete.is_some() {
        0
    } else if part.is_some() {
        PART_BYTES
    } else {
        SINGLE_UPLOAD_BYTES
    };
    if length.is_some_and(|n| n > limit) {
        return Err(too_large());
    }

    // The winning receipt selects one immutable object. Concurrent servers
    // and interrupted requests never overwrite the selected match bytes.
    let key = part.map_or_else(
        || {
            format!(
                "{}{id}/{}.h2film",
                super::HOST_FILM_PREFIX,
                live_proto::sign::random_id()
            )
        },
        |n| staging_key(&id, n),
    );
    let meta = Meta {
        content_type: Some("application/octet-stream".into()),
        ..Default::default()
    };
    let mut writer = Some(store.writer(&key, &meta).map_err(blob_error)?);
    let mut spool = Spool::default();
    let mut keep_pages = false;
    let mut finished_candidate = false;
    let mut keep_candidate = false;
    let result = async {
        let out = writer.as_mut().expect("writer is open");
        let mut inspector = Inspector::default();
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| invalid("the film upload was cut off"))?;
            if out.size().saturating_add(chunk.len() as u64) > limit {
                return Err(too_large());
            }
            if part.is_none() && complete.is_none() {
                inspect_bytes(&chunk, &mut inspector, &mut spool, store, &key).await?;
            }
            out.write(&chunk).await.map_err(blob_error)?;
        }
        if length.is_some_and(|n| out.size() != n) {
            return Err(invalid("the film upload was cut off"));
        }
        if let Some((size, ref hash)) = complete {
            assemble(
                store,
                &id,
                size,
                hash,
                &mut inspector,
                &mut spool,
                &key,
                out,
            )
            .await?;
        }
        if out.size() == 0 {
            return Err(invalid("empty host film"));
        }
        let inspected = if part.is_none() {
            Some(inspector.finish()?)
        } else {
            None
        };
        // Parallel retries can stream freely. Only the first publication
        // wins. A late part cannot recreate staging after publication.
        'selection: {
            let _guard = live.film_writes.lock().await;
            let selecting = !live.model.read().films.imports.contains_key(&id);
            if let Some(v) = version(&live, &id, lease, &grant)?
                && selecting
            {
                let _host_guard = if inspected.is_some() {
                    Some(live.host_game_writes.lock(&id).await)
                } else {
                    None
                };
                let publication = if let Some(film) = inspected {
                    let mut receipt = json!({"game": id, "blob": key});
                    let v3 = film.web_ranges.is_some();
                    if let Some(summary) = film.summary {
                        receipt["summary"] = json!(summary);
                    }
                    if let Some(ranges) = film.web_ranges {
                        receipt["web_ranges"] = json!(ranges);
                    }
                    Some(
                        spool
                            .prepare(
                                &live,
                                &id,
                                &publisher,
                                &grant,
                                store,
                                receipt,
                                film.events.started_ms,
                                film.events.tick_rate.unwrap_or(30),
                                v3,
                            )
                            .await?,
                    )
                } else {
                    None
                };
                writer
                    .take()
                    .expect("writer is open")
                    .finish()
                    .await
                    .map_err(blob_error)?;
                if let Some((events, game_version, pending)) = publication {
                    finished_candidate = true;
                    keep_candidate = true;
                    keep_pages = pending;
                    let fence = if pending {
                        (super::import_stream(&id), 0)
                    } else {
                        (super::STREAM.into(), v)
                    };
                    let expect = [(live_proto::game::stream(&id), game_version), fence];
                    let stored = match live.store.append(events, &expect).await {
                        Ok(rows) => rows,
                        Err(e @ crate::store::StoreError::Conflict { .. }) => {
                            // A failed compare/exchange never committed our receipt.
                            keep_candidate = false;
                            keep_pages = false;
                            live.wait_applied(live.store.head().await?).await?;
                            let state = live.model.read();
                            if state.films.imports.contains_key(&id)
                                || state.games.get(&id).is_some_and(|g| g.has_host_film)
                            {
                                break 'selection;
                            }
                            return Err(e.into());
                        }
                        Err(e) => return Err(e.into()),
                    };
                    live.wait_applied(stored.last().expect("film receipt").position)
                        .await?;
                }
            }
        }
        if part.is_none() {
            super::resume_import(&live, &id, true).await?;
            live.rank_game(&id).await?;
        }
        Ok(StatusCode::NO_CONTENT)
    }
    .await;
    if let Some(writer) = writer {
        writer.abort().await;
    }
    if !keep_pages {
        spool.discard(store, &key).await;
    }
    if finished_candidate && !keep_candidate {
        let _ = store.delete(&key).await;
    }
    if result.is_ok() && part.is_none() {
        cleanup_published(&live, &id).await;
    }
    result
}

/// Once the immutable receipt exists, an interrupted writer can never win a
/// later selection. Drop its object and pages as well as superseded part tails.
pub(super) async fn cleanup_published(live: &Live, id: &str) {
    let Some(chosen) = live
        .model
        .read()
        .games
        .get(id)
        .filter(|g| g.has_host_film)
        .map(|g| {
            g.film_blob
                .clone()
                .unwrap_or_else(|| super::host_film_key(id))
        })
    else {
        return;
    };
    let Some(blobs) = live.blobs() else {
        return;
    };
    for prefix in [
        format!("film-staging/{id}/"),
        format!("{}{id}/", super::HOST_FILM_PREFIX),
    ] {
        match blobs.private.list(&prefix).await {
            Ok(items) => {
                for item in items {
                    if item.key != chosen
                        && let Err(e) = blobs.private.delete(&item.key).await
                    {
                        eprintln!("live: film staging cleanup: {e}");
                    }
                }
            }
            Err(e) => eprintln!("live: film staging cleanup: {e}"),
        }
    }
}

async fn inspect_bytes(
    mut bytes: &[u8],
    inspector: &mut Inspector,
    spool: &mut Spool,
    store: &BlobStore,
    key: &str,
) -> ApiResult<()> {
    while !bytes.is_empty() {
        let consumed = inspector.consume_one(bytes)?;
        spool.drain(&mut inspector.events, store, key).await?;
        bytes = &bytes[consumed..];
    }
    Ok(())
}

async fn assemble(
    store: &BlobStore,
    id: &str,
    size: u64,
    expected: &str,
    inspector: &mut Inspector,
    spool: &mut Spool,
    key: &str,
    out: &mut BlobWriter,
) -> ApiResult<()> {
    let mut hash = Sha256::new();
    for n in 0..size.div_ceil(PART_BYTES) {
        let (len, mut stream) = store
            .get_stream(&staging_key(id, n))
            .await
            .map_err(blob_error)?
            .ok_or_else(|| invalid("missing host film part"))?;
        if len != (size - n * PART_BYTES).min(PART_BYTES) {
            return Err(invalid("wrong host film part length"));
        }
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(blob_error)?;
            hash.update(&chunk);
            inspect_bytes(&chunk, inspector, spool, store, key).await?;
            out.write(&chunk).await.map_err(blob_error)?;
        }
    }
    if out.size() != size || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected) {
        return Err(invalid("host film digest mismatch"));
    }
    Ok(())
}
