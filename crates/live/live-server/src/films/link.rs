//! A catalogue row can be visible to more viewers than the exact machine
//! recording. Check the latter's participant rule before every range read.
use super::*;
use crate::http::bearer;
use axum::http::header;

pub(crate) async fn film_detail(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    game_id(&id)?;
    let (viewer, player) = live.read_viewer_account(bearer(&headers))?;
    let model = live.model.read();
    if let Some(share) = model.films.shared.get(&id) {
        if !share.visible(player) {
            return Err(not_found("no such film"));
        }
        return Ok(shared::private_json(StatusCode::OK, &share.row()));
    }
    let film = visible_game(&model, &id, viewer).ok_or_else(|| not_found("no such film"))?;
    let mut response = json(StatusCode::OK, &film.listing(player.unwrap_or(Xuid(0))));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    Ok(response)
}

#[derive(Deserialize)]
pub(crate) struct ObjectQuery {
    object: Option<String>,
    #[serde(flatten)]
    ranges: ranges::RangeQuery,
}

pub(crate) async fn film_object(
    State(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ObjectQuery>,
) -> ApiResult<Response> {
    game_id(&id)?;
    if let Some(share) = {
        let model = live.model.read();
        model.films.shared.get(&id).cloned()
    } {
        return shared::read_object(
            &live,
            &headers,
            share,
            query.object.as_deref(),
            &query.ranges,
        )
        .await;
    }
    match query.object.as_deref() {
        None => {
            return ranges::read_film(State(live), Path(id), Query(query.ranges), headers).await;
        }
        Some("film") => {}
        _ => return Err(invalid("unknown Theater object")),
    }
    if !headers.contains_key(header::RANGE) {
        return download_host_film(&live, &headers, &id).await;
    }
    let author = live.authenticate(bearer(&headers), lxm::READ)?;
    let player = account(&author)?;
    let (key, size, digest) = {
        let viewer = live.viewer_of(&author);
        let model = live.model.read();
        let Some(Source::Game(game)) = visible_game(&model, &id, viewer) else {
            return Err(not_found("no host film for this account"));
        };
        if !game.has_host_film || !game.xuids.contains(&player) {
            return Err(not_found("no host film for this account"));
        }
        let Some((size, digest)) = model.films.downloads.get(&id) else {
            // Older receipts have no content digest. Their authenticated full GET remains available.
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "film predates resumable downloads",
            ));
        };
        (
            game.film_blob.clone().unwrap_or_else(|| host_film_key(&id)),
            *size,
            digest.clone(),
        )
    };
    if let Some(tag) = headers.get(header::IF_MATCH)
        && tag.to_str().ok() != Some(&format!("\"film-{digest}\""))
    {
        return Ok(Response::builder()
            .status(StatusCode::PRECONDITION_FAILED)
            .body(axum::body::Body::empty())
            .unwrap());
    }
    let range = ranges::parse_range(&headers, size)
        .filter(|r| r.end - r.start <= live_proto::film::PART_BYTES);
    let Some(range) = range else {
        return Ok(ranges::range_response(
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
            size,
            Bytes::new(),
            false,
            Some(&digest),
        ));
    };
    let store = live
        .blobs()
        .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "film store unavailable"))?;
    let (bytes, total) = parts::get_range(&store.private, &key, range.clone())
        .await
        .map_err(|_| ApiError::new(ErrorCode::Unavailable, "could not read film"))?
        .ok_or_else(|| not_found("no such film"))?;
    if total != size || bytes.len() as u64 != range.end - range.start {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "stored film length differs from receipt",
        ));
    }
    Ok(ranges::range_response(
        StatusCode::PARTIAL_CONTENT,
        Some(&range),
        size,
        bytes,
        false,
        Some(&digest),
    ))
}
