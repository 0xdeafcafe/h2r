//! A part is acknowledged only after its bytes and ingestion cursor are
//! durable. Every cursor update compares the prior object generation.
use bytes::Bytes;
use live_proto::film::PART_BYTES;
use serde::{Deserialize, Serialize};

use super::{ApiResult, BlobStore, Inspector, Meta, Spool, blob_error, invalid};
use crate::{
    app::conflict,
    blobs::{BlobError, Seen},
};

#[derive(Serialize, Deserialize)]
pub(super) struct Candidate {
    pub blob: String,
    pub reservation: crate::host_identity::FilmReservation,
    pub size: u64,
    /// Empty for a single POST. Multipart keys include their content digest.
    pub hashes: Vec<String>,
    pub spool: Spool,
    pub seal: Option<String>,
}

impl Candidate {
    pub fn new(id: &str, multipart: bool, grant: &crate::host_identity::FilmReservation) -> Self {
        let suffix = if multipart { "parts" } else { "h2film" };
        let nonce = live_proto::sign::random_id();
        Self {
            blob: format!("{}{id}/{nonce}.{suffix}", super::super::HOST_FILM_PREFIX),
            reservation: grant.clone(),
            size: 0,
            hashes: Vec::new(),
            spool: Spool::default(),
            seal: None,
        }
    }

    pub fn multipart(&self) -> bool {
        self.blob.ends_with(".parts")
    }

    pub async fn read(&self, store: &BlobStore, pos: u64, max: u64) -> ApiResult<Bytes> {
        let (key, offset, size) = if self.multipart() {
            let n = pos / PART_BYTES;
            let hash = self
                .hashes
                .get(n as usize)
                .ok_or_else(|| invalid("missing film part"))?;
            (
                super::super::parts::part_key(&self.blob, n, hash),
                pos % PART_BYTES,
                (self.size - n * PART_BYTES).min(PART_BYTES),
            )
        } else {
            (self.blob.clone(), pos, self.size)
        };
        let end = offset.saturating_add(max).min(size);
        let (bytes, total) = store
            .get_range(&key, offset..end)
            .await
            .map_err(blob_error)?
            .ok_or_else(|| invalid("missing staged host film bytes"))?;
        if total != size || bytes.len() as u64 != end - offset {
            return Err(invalid("staged film length changed"));
        }
        Ok(bytes)
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Ready {
    pub candidate: Candidate,
    pub receipt: serde_json::Value,
    #[serde(default)]
    pub unified: bool,
    pub started_ms: Option<u64>,
    pub hz: u16,
    pub sha256: String,
}

pub(super) enum Progress {
    Reading(Inspector, Candidate),
    Ready(Ready),
}

impl Progress {
    pub fn candidate(&self) -> &Candidate {
        match self {
            Self::Reading(_, candidate) => candidate,
            Self::Ready(ready) => &ready.candidate,
        }
    }

    pub fn blob(&self) -> &str {
        &self.candidate().blob
    }
}

pub(super) struct Slot {
    key: String,
    seen: Seen,
}

impl Slot {
    pub async fn load(store: &BlobStore, id: &str) -> ApiResult<(Self, Option<Progress>)> {
        Self::at(store, format!("film-staging/{id}/state")).await
    }

    /// A signed legacy job refers to this candidate-specific cursor. Another
    /// process may concurrently replace the open upload, even between our
    /// cursor write and the commit that acknowledges the legacy job.
    pub async fn legacy(
        store: &BlobStore,
        id: &str,
        blob: &str,
    ) -> ApiResult<(Self, Option<Progress>)> {
        let nonce = blob
            .rsplit('/')
            .next()
            .ok_or_else(|| invalid("invalid candidate key"))?;
        Self::at(store, format!("film-staging/{id}/{nonce}.legacy")).await
    }

    async fn at(store: &BlobStore, key: String) -> ApiResult<(Self, Option<Progress>)> {
        let (progress, seen) = match store.get_seen(&key).await.map_err(blob_error)? {
            Some((bytes, seen)) => {
                let p = match bytes.split_last() {
                    Some((&1, payload)) => {
                        let (inspector, candidate) = Inspector::restore(payload)?;
                        Progress::Reading(inspector, candidate)
                    }
                    Some((&2, payload)) => Progress::Ready(
                        serde_json::from_slice(payload)
                            .map_err(|_| invalid("invalid film publication cursor"))?,
                    ),
                    _ => return Err(invalid("invalid film ingestion cursor")),
                };
                (Some(p), seen)
            }
            None => (None, Seen::default()),
        };
        Ok((Self { key, seen }, progress))
    }

    pub async fn save(&mut self, store: &BlobStore, progress: &Progress) -> ApiResult<()> {
        let (mut bytes, kind) = match progress {
            Progress::Reading(inspector, candidate) => (inspector.checkpoint(candidate)?, 1),
            Progress::Ready(ready) => (
                serde_json::to_vec(ready).map_err(|_| invalid("encoding publication cursor"))?,
                2,
            ),
        };
        bytes.push(kind);
        self.seen = store
            .put_if(&self.key, bytes.into(), &self.seen, &Meta::default())
            .await
            .map_err(|e| match e {
                BlobError::Changed(_) => {
                    conflict("film ingestion advanced on another server; retry")
                }
                e => blob_error(e),
            })?;
        Ok(())
    }
}
