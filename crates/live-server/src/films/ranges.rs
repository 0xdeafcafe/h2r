//! Public ranges of one otherwise private, immutable game recording. Upload
//! inspection verifies each time-index entry against an actual chunk. The
//! signed receipt binds that exact index to the game's visibility record.
use std::{
    collections::VecDeque,
    ops::Range,
    sync::{Arc, Mutex},
};

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use live_proto::{
    ErrorCode,
    film::range::{FOOTER_BYTES, PREFIX_BYTES},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ApiError, ApiResult, Shared, bearer, game_id, host_film_key, not_found, visible_game};
use crate::blobs::BlobStore;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct WebRanges {
    /// Set by field-level validation of the front, directory and all TICKs.
    /// Old signed receipts authenticated extents only.
    #[serde(default)]
    pub public_safe: bool,
    pub size: u64,
    /// JSON byte length of the public front summary; reserve padding is private.
    pub summary_bytes: u32,
    pub index_offset: u64,
    /// Packed CIDX length including its eight-byte chunk envelope.
    pub index_size: u32,
    pub index_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks_per_second: Option<u16>,
}

/// Each entry retains only the physically validated byte extents needed by
/// this endpoint, independently of the size of the directory's clock maps.
#[derive(Clone, Copy, Deserialize)]
struct Extent {
    offset: u64,
    size: u32,
}

#[derive(Deserialize)]
struct Extents {
    chunks: Box<[Extent]>,
}

const MAX_CACHED_FILMS: usize = 16;
const MAX_CACHED_BYTES: usize = 4 << 20;

#[derive(Default)]
pub(crate) struct WebIndexCache(Mutex<VecDeque<(String, Arc<Extents>)>>);

impl WebIndexCache {
    fn get(&self, digest: &str) -> Option<Arc<Extents>> {
        let mut lru = self.0.lock().expect("film index cache");
        let at = lru.iter().position(|(key, _)| key == digest)?;
        let entry = lru.remove(at)?;
        let index = entry.1.clone();
        lru.push_front(entry);
        Some(index)
    }

    fn insert(&self, digest: &str, dir: Extents) -> Arc<Extents> {
        let dir = Arc::new(dir);
        let bytes = |dir: &Extents| std::mem::size_of_val(&*dir.chunks);
        if bytes(&dir) > MAX_CACHED_BYTES {
            return dir;
        }
        let mut lru = self.0.lock().expect("film index cache");
        // Two readers may have missed concurrently while fetching from R2.
        lru.retain(|(key, _)| key != digest);
        lru.push_front((digest.into(), dir.clone()));
        let mut used = lru.iter().map(|(_, d)| bytes(d)).sum::<usize>();
        while lru.len() > MAX_CACHED_FILMS || used > MAX_CACHED_BYTES {
            let (_, old) = lru.pop_back().expect("nonempty index cache");
            used -= bytes(&old);
        }
        dir
    }
}

/// A strong validator for a single immutable upload. The CIDX digest
/// identifies offsets, not the bytes at those offsets in different films.
fn revision(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

#[derive(Deserialize, Default)]
pub(crate) struct RangeQuery {
    rev: Option<String>,
    span: Option<String>,
    auth: Option<String>,
}

const PRIVATE: &str = "private, no-store, no-transform";
use crate::caching::IMMUTABLE_FILM_CHUNK as PUBLIC;

fn unavailable() -> ApiError {
    ApiError::new(ErrorCode::Unavailable, "the film range is unavailable")
}

fn parse_range(headers: &HeaderMap, size: u64) -> Option<Range<u64>> {
    let all = headers.get_all(header::RANGE);
    let mut ranges = all.iter();
    let value = ranges.next()?.to_str().ok()?.strip_prefix("bytes=")?;
    if ranges.next().is_some() {
        return None;
    }
    let (start, last) = value.split_once('-')?;
    let number = |n: &str| {
        if n.is_empty() || !n.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        n.parse::<u64>().ok()
    };
    if start.is_empty() {
        if number(last)? != FOOTER_BYTES as u64 {
            return None;
        }
        return Some(size.checked_sub(FOOTER_BYTES as u64)?..size);
    }
    let start = number(start)?;
    let end = number(last)?.checked_add(1)?;
    (start < end && end <= size).then_some(start..end)
}

fn range_response(
    status: StatusCode,
    range: Option<&Range<u64>>,
    total: u64,
    body: Bytes,
    shared: bool,
    rev: Option<&str>,
) -> Response {
    let span = match range {
        Some(r) => format!("bytes {}-{}/{total}", r.start, r.end - 1),
        None => format!("bytes */{total}"),
    };
    let mut response = Response::builder().status(status);
    if let Some(rev) = rev {
        response = response.header(header::ETAG, format!("\"film-{rev}\""));
    }
    response
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_RANGE, span)
        .header(header::CONTENT_LENGTH, body.len().to_string())
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, if shared { PUBLIC } else { PRIVATE })
        .header(header::VARY, "Authorization, Range")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(body))
        .expect("static response headers and validated range")
}

impl WebRanges {
    fn front_or_footer(&self, r: &Range<u64>) -> bool {
        r.end <= PREFIX_BYTES as u64 + u64::from(self.summary_bytes)
            || self
                .size
                .checked_sub(FOOTER_BYTES as u64)
                .is_some_and(|start| r.start >= start && r.end <= self.size)
    }

    async fn fetch(&self, store: &BlobStore, key: &str, range: Range<u64>) -> ApiResult<Bytes> {
        let (bytes, size) = super::parts::get_range(store, key, range)
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(|| not_found("no such film"))?;
        if size != self.size {
            return Err(unavailable());
        }
        Ok(bytes)
    }

    async fn read(
        &self,
        cache: &WebIndexCache,
        store: &BlobStore,
        key: &str,
        range: &Range<u64>,
    ) -> ApiResult<Option<(Bytes, bool)>> {
        if !self.public_safe {
            return Ok(None);
        }
        if self.front_or_footer(range) {
            return self
                .fetch(store, key, range.clone())
                .await
                .map(|b| Some((b, false)));
        }
        let end = self
            .index_offset
            .checked_add(self.index_size.into())
            .ok_or_else(unavailable)?;
        let index_range = self.index_offset..end;
        let in_index = index_range.start <= range.start && range.end <= index_range.end;
        if !in_index
            && (range.start < live_proto::film::range::DATA_OFFSET || range.end > self.index_offset)
        {
            return Ok(None);
        }
        let get_index = || async {
            let bytes = self.fetch(store, key, index_range.clone()).await?;
            if bytes.len() < 8
                || &bytes[..4] != b"CIDX"
                || format!("{:x}", Sha256::digest(&bytes)) != self.index_sha256
            {
                return Err(unavailable());
            }
            Ok(bytes)
        };
        if in_index {
            let bytes = get_index().await?;
            let subrange = (range.start - index_range.start) as usize
                ..(range.end - index_range.start) as usize;
            return Ok(Some((bytes.slice(subrange), false)));
        }
        let dir = if let Some(hit) = cache.get(&self.index_sha256) {
            hit
        } else {
            let index = get_index().await?;
            let raw = super::inspect::inflate_limited(
                &index[8..],
                live_proto::film::range::MAX_INDEX_BYTES,
            )?;
            let dir: Extents = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
            cache.insert(&self.index_sha256, dir)
        };
        let upper = dir.chunks.partition_point(|c| c.offset <= range.start);
        let Some(i) = upper.checked_sub(1) else {
            return Ok(None);
        };
        let chunk = &dir.chunks[i];
        let end = chunk.offset + 8 + u64::from(chunk.size);
        if range.end > end {
            return Ok(None);
        }
        let bytes = self.fetch(store, key, range.clone()).await?;
        if range.start == chunk.offset
            && bytes.len() >= 8
            && (&bytes[..4] != b"TICK" || bytes[4..8] != chunk.size.to_le_bytes())
        {
            return Err(unavailable());
        }
        let complete = range.start == chunk.offset && range.end == end;
        Ok(Some((bytes, complete)))
    }
}

pub(crate) async fn read_film_range(
    State(live): Shared,
    Path(id): Path<String>,
    Query(query): Query<RangeQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    game_id(&id)?;
    let (receipt, immutable_key, ended) = visible_game(&live.model.read(), &id, viewer)
        .and_then(super::Source::range_receipt)
        .filter(|(receipt, _, _)| receipt.public_safe)
        .ok_or_else(|| not_found("no such film"))?;
    let rev = immutable_key.as_deref().map(revision);
    if query.rev.is_some() && query.rev != rev {
        return Err(not_found("no such film revision"));
    }
    let key = immutable_key.unwrap_or_else(|| host_film_key(&id));
    let denied = || {
        range_response(
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
            receipt.size,
            Bytes::new(),
            false,
            None,
        )
    };
    let Some(range) = parse_range(&headers, receipt.size) else {
        return Ok(denied());
    };
    let request_span = headers[header::RANGE]
        .to_str()
        .ok()
        .and_then(|v| v.strip_prefix("bytes="));
    if query.rev.is_some() != query.span.is_some()
        || (query.span.is_some() && query.span.as_deref() != request_span)
    {
        return Ok(denied());
    }
    // read_viewer has authorised anonymous access under this home's read
    // policy and visible_game checked this game's visibility above.
    // An open co-op session can switch from public to friends-only.
    let shareable = ended
        && viewer == crate::sessions::Viewer::Anonymous
        && !headers.contains_key(header::AUTHORIZATION)
        && query.auth.as_deref().is_none_or(|a| a == "0");
    let store = &live.blobs().ok_or_else(unavailable)?.private;
    match receipt
        .read(&live.web_film_index, store, &key, &range)
        .await?
    {
        Some((bytes, complete)) => Ok(range_response(
            StatusCode::PARTIAL_CONTENT,
            Some(&range),
            receipt.size,
            bytes,
            complete && shareable && query.rev.is_some(),
            rev.as_deref(),
        )),
        None => Ok(denied()),
    }
}

#[cfg(test)]
mod tests;
