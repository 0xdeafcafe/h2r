//! Public ranges of one otherwise private, immutable game recording. Upload
//! inspection verifies each time-index entry against an actual chunk. The
//! signed receipt binds that exact index to the game's visibility record.
use std::{
    ops::Range,
    sync::{Arc, Mutex},
};

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use live_proto::{
    ErrorCode,
    film::range::{Directory, FOOTER_BYTES, PREFIX_BYTES},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ApiError, ApiResult, Shared, bearer, game_id, host_film_key, not_found, visible_game};
use crate::blobs::BlobStore;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct WebRanges {
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

/// Keep the most recently decoded time index across seeks. The signed index
/// digest is its identity. Bound residency to one directory per Live home.
#[derive(Default)]
pub(crate) struct WebIndexCache(Mutex<Option<(String, Arc<Directory>)>>);

impl WebIndexCache {
    fn get(&self, digest: &str) -> Option<Arc<Directory>> {
        self.0
            .lock()
            .expect("film index cache")
            .as_ref()
            .filter(|(key, _)| key == digest)
            .map(|(_, dir)| Arc::clone(dir))
    }

    fn insert(&self, digest: &str, dir: Directory) -> Arc<Directory> {
        let dir = Arc::new(dir);
        *self.0.lock().expect("film index cache") = Some((digest.into(), Arc::clone(&dir)));
        dir
    }
}

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
) -> Response {
    let span = match range {
        Some(r) => format!("bytes {}-{}/{total}", r.start, r.end - 1),
        None => format!("bytes */{total}"),
    };
    Response::builder()
        .status(status)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_RANGE, span)
        .header(header::CONTENT_LENGTH, body.len().to_string())
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "private, no-store, no-transform")
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
        let (bytes, size) = store
            .get_range(key, range)
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
    ) -> ApiResult<Option<Bytes>> {
        if self.front_or_footer(range) {
            return self.fetch(store, key, range.clone()).await.map(Some);
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
            return Ok(Some(bytes.slice(
                (range.start - index_range.start) as usize
                    ..(range.end - index_range.start) as usize,
            )));
        }
        let dir = if let Some(hit) = cache.get(&self.index_sha256) {
            hit
        } else {
            let index = get_index().await?;
            let raw = super::inspect::inflate_limited(
                &index[8..],
                live_proto::film::range::MAX_INDEX_BYTES,
            )?;
            let dir: Directory = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
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
        Ok(Some(bytes))
    }
}

pub(crate) async fn read_film_range(
    State(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    game_id(&id)?;
    let (receipt, key) = visible_game(&live.model.read(), &id, viewer)
        .and_then(|game| {
            Some((
                game.film_web_ranges.clone()?,
                game.film_blob.clone().unwrap_or_else(|| host_film_key(&id)),
            ))
        })
        .ok_or_else(|| not_found("no such film"))?;
    let denied = || {
        range_response(
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
            receipt.size,
            Bytes::new(),
        )
    };
    let Some(range) = parse_range(&headers, receipt.size) else {
        return Ok(denied());
    };
    let store = &live.blobs().ok_or_else(unavailable)?.private;
    match receipt
        .read(&live.web_film_index, store, &key, &range)
        .await?
    {
        Some(bytes) => Ok(range_response(
            StatusCode::PARTIAL_CONTENT,
            Some(&range),
            receipt.size,
            bytes,
        )),
        None => Ok(denied()),
    }
}

#[cfg(test)]
mod tests;
