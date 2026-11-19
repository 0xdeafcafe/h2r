//! One bounded unit of work per call. A signed part advances the durable
//! cursor only after all content-addressed blobs have been read back. Its
//! two index nodes link to the preceding nodes, so final publication is a
//! constant-size, atomic event even for a long game.
use axum::body::Bytes;
use live_proto::{archive, game, sign};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Recording, source};
use crate::Live;
use crate::app::{ApiError, conflict, invalid, not_found};
use crate::store::StoreError;

pub(super) const ARCHIVE: &str = "frames/archive/";
pub(super) const BACKUP: &str = "backup/frames-pre-v7/";
pub(super) const PART: &str = "ReplayArchivePart";
pub(super) const PUBLISHED: &str = "ReplayArchivePublished";
pub(super) const ROLLED_BACK: &str = "ReplayArchiveRolledBack";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Segment {
    pub from_tick: u64,
    pub to_tick: u64,
    pub frames: usize,
    pub bytes: u64,
    pub sha256: String,
    pub keyframes: Vec<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Totals {
    pub frames: u64,
    pub segments: u64,
    pub archive_bytes: u64,
    #[serde(default)]
    pub index_bytes: u64,
    pub source_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Progress {
    pub(super) source_position: u64,
    after: source::Cursor,
    pub archive_sha256: Option<String>,
    pub backup_sha256: Option<String>,
    pub totals: Totals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Part {
    pub segment: Segment,
    pub progress: Progress,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    DryRun,
    Apply,
    Verify,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub mode: Mode,
    pub cursor: Option<String>,
    pub source_position: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ResultPage<'a> {
    game_id: &'a str,
    state: &'static str,
    done: bool,
    next: Option<String>,
    source_position: u64,
    segment: Option<usize>,
    frames: usize,
    source_bytes: u64,
    archive_bytes: u64,
    archive_sha256: Option<String>,
    backup_sha256: Option<String>,
    totals: Totals,
}

fn bytes<T: Serialize>(v: &T) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(v).map_err(|e| invalid(e.to_string()))
}
fn sha(b: &[u8]) -> String {
    sign::hex(&Sha256::digest(b))
}
fn data<T: Serialize>(v: &T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(v).map_err(|e| invalid(e.to_string()))
}

impl Progress {
    fn token(&self) -> Result<String, ApiError> {
        Ok(live_proto::pack::to_base64(&bytes(self)?))
    }
}

pub(super) async fn read_blob(live: &Live, prefix: &str, hash: &str) -> Result<Bytes, ApiError> {
    let found = if let Some(b) = live.blobs() {
        b.private
            .get(&format!("{prefix}{hash}"))
            .await
            .map_err(|e| failure(e.to_string()))?
    } else {
        live.store.blob(hash).await?.map(Bytes::from)
    };
    let bytes = found.ok_or_else(|| not_found("replay archive blob is missing"))?;
    if sha(&bytes) != hash {
        return Err(invalid("replay archive integrity check failed"));
    }
    Ok(bytes)
}

fn failure(msg: impl Into<String>) -> ApiError {
    ApiError::new(live_proto::ErrorCode::Unavailable, msg)
}

async fn blob(live: &Live, prefix: &str, raw: Bytes, mode: Mode) -> Result<String, ApiError> {
    let hash = sha(&raw);
    if mode == Mode::Apply {
        if let Some(b) = live.blobs() {
            b.private
                .put(
                    &format!("{prefix}{hash}"),
                    raw,
                    &crate::blobs::Meta::default(),
                )
                .await
                .map_err(|e| failure(e.to_string()))?;
        } else {
            live.store.put_blob(&hash, &raw).await?;
        }
    }
    if mode != Mode::DryRun {
        read_blob(live, prefix, &hash).await?;
    }
    Ok(hash)
}

impl Recording {
    fn base(&self) -> Progress {
        Progress {
            source_position: self.position,
            ..Default::default()
        }
    }
    pub(super) fn progress(&self) -> Progress {
        self.parts
            .last()
            .map(|p| p.progress.clone())
            .unwrap_or_else(|| self.base())
    }
    pub(super) fn state(&self, ended: bool) -> &'static str {
        if !ended {
            "live"
        } else if self.published {
            "published"
        } else if self.rolled_back {
            "rolled_back"
        } else if !self.parts.is_empty() {
            "staging"
        } else if self.chunks == 0 && self.legacy_frames == 0 {
            "empty"
        } else {
            "raw"
        }
    }
}

fn page<'a>(
    id: &'a str,
    state: &'static str,
    p: &Progress,
    done: bool,
    part: Option<&Part>,
    prev_bytes: u64,
) -> Result<ResultPage<'a>, ApiError> {
    Ok(ResultPage {
        game_id: id,
        state,
        done,
        source_position: p.source_position,
        next: if done { None } else { Some(p.token()?) },
        segment: part.map(|_| (p.totals.segments - 1) as usize),
        source_bytes: p.totals.source_bytes.saturating_sub(prev_bytes),
        archive_bytes: part.map_or(0, |x| x.segment.bytes),
        frames: part.map_or(0, |x| x.segment.frames),
        totals: p.totals.clone(),
        archive_sha256: p.archive_sha256.clone(),
        backup_sha256: p.backup_sha256.clone(),
    })
}

// CAS on the actual game stream checks the entire read/write interval,
// including a delayed FrameChunk and concurrent processors on another host.
pub(super) async fn append(
    live: &Live,
    id: &str,
    revision: u64,
    writes: Vec<(&str, serde_json::Value)>,
) -> Result<(), ApiError> {
    let stream = game::stream(id);
    let events = writes
        .into_iter()
        .map(|(kind, d)| live.server_event(stream.clone(), kind, d))
        .collect();
    let rows = live
        .store
        .append(events, &[(stream, revision)])
        .await
        .map_err(|e| match e {
            StoreError::Conflict { .. } => conflict("recording changed; retry the step"),
            other => ApiError::from(other),
        })?;
    live.wait_applied(rows.last().expect("a nonempty batch").position)
        .await?;
    Ok(())
}

fn snapshot(live: &Live, id: &str, expected: Option<u64>) -> Result<Recording, ApiError> {
    let m = live.model.read();
    let g = m.games.get(id).ok_or_else(|| not_found("no such game"))?;
    if g.ended.is_none() {
        return Err(conflict("game is still live"));
    }
    if expected.is_some_and(|n| n != g.recording.position) {
        return Err(conflict("recording changed"));
    }
    Ok(g.recording.clone())
}

pub(crate) async fn step<'a>(
    live: &Live,
    id: &'a str,
    req: Request,
) -> Result<ResultPage<'a>, ApiError> {
    let r = snapshot(live, id, req.source_position)?;
    let mode = req.mode;
    if mode == Mode::Verify && !r.published {
        return Err(conflict("archive has not been published"));
    }
    let p = if let Some(t) = req.cursor {
        if t.len() > 16384 {
            return Err(invalid("invalid archive cursor"));
        }
        let b = live_proto::pack::from_base64(&t).map_err(|_| invalid("invalid archive cursor"))?;
        serde_json::from_slice(&b).map_err(|_| invalid("invalid archive cursor"))?
    } else if mode == Mode::Apply && !r.rolled_back {
        r.progress()
    } else {
        r.base()
    };
    if p.source_position != r.position {
        return Err(conflict("archive cursor is stale"));
    }
    let i = usize::try_from(p.totals.segments).map_err(|_| invalid("invalid archive cursor"))?;
    if mode == Mode::Apply && r.rolled_back && i != 0 {
        return Err(conflict("restart rolled back archive at its first segment"));
    }
    if mode != Mode::DryRun {
        let expected = if i == 0 {
            r.base()
        } else {
            r.parts
                .get(i - 1)
                .ok_or_else(|| conflict("archive cursor is ahead of progress"))?
                .progress
                .clone()
        };
        if expected != p {
            return Err(conflict("archive cursor does not match stored progress"));
        }
    }
    if mode == Mode::Apply && !r.rolled_back {
        if let Some(part) = r.parts.get(i) {
            return page(
                id,
                r.state(true),
                &part.progress,
                part.done,
                Some(part),
                p.totals.source_bytes,
            );
        }
        if r.published {
            return page(id, "published", &p, true, None, p.totals.source_bytes);
        }
    }
    let batch = source::batch(live, id, r.position, p.after).await?;
    if batch.frames.is_empty() {
        if r.state(true) != "empty" {
            return Err(conflict("archive cursor missed the source"));
        }
        return page(id, "empty", &p, true, None, 0);
    }

    let stored = if mode == Mode::Verify {
        let part = r
            .parts
            .get(i)
            .ok_or_else(|| conflict("published segment is missing"))?;
        read_blob(live, ARCHIVE, &part.segment.sha256).await?
    } else {
        archive::encode(&batch.frames)
            .map_err(|e| invalid(e.to_string()))?
            .into()
    };
    archive::verify(&batch.frames, &stored).map_err(|e| invalid(e.to_string()))?;
    let seg = Segment {
        from_tick: batch.frames[0].tick,
        to_tick: batch.frames.last().unwrap().tick,
        bytes: stored.len() as u64,
        frames: batch.frames.len(),
        sha256: blob(live, ARCHIVE, stored, mode).await?,
        keyframes: batch.keyframes,
    };
    let mut rows = Vec::with_capacity(batch.originals.len());
    for original in batch.originals {
        if let Some((signed, original_bytes)) = original.blob {
            if blob(live, BACKUP, original_bytes, mode).await? != signed {
                return Err(invalid("source backup hash mismatch"));
            }
        }
        rows.push(original.event);
    }
    // The original signed events retain both the index metadata and legacy
    // JSON/packed payloads. Each FrameChunk hash names its exact backup blob.
    let backup = bytes(&(
        "H2 original recording",
        id,
        r.position,
        &p.backup_sha256,
        &rows,
    ))?;
    let index = bytes(&(
        "H2 replay archive v1",
        id,
        r.position,
        &p.archive_sha256,
        &seg,
    ))?;
    let index_bytes = index.len() as u64;
    let progress = Progress {
        source_position: r.position,
        after: batch.next.unwrap_or_default(),
        archive_sha256: Some(blob(live, ARCHIVE, index.into(), mode).await?),
        backup_sha256: Some(blob(live, BACKUP, backup.into(), mode).await?),
        totals: Totals {
            index_bytes: p
                .totals
                .index_bytes
                .checked_add(index_bytes)
                .ok_or_else(|| invalid("index size overflow"))?,
            segments: p
                .totals
                .segments
                .checked_add(1)
                .ok_or_else(|| invalid("segment count overflow"))?,
            frames: p
                .totals
                .frames
                .checked_add(seg.frames as u64)
                .ok_or_else(|| invalid("frame count overflow"))?,
            source_bytes: p
                .totals
                .source_bytes
                .checked_add(batch.source_bytes)
                .ok_or_else(|| invalid("source size overflow"))?,
            archive_bytes: p
                .totals
                .archive_bytes
                .checked_add(seg.bytes)
                .ok_or_else(|| invalid("archive size overflow"))?,
        },
    };
    let part = Part {
        segment: seg,
        progress,
        done: batch.next.is_none(),
    };
    if mode == Mode::Verify && r.parts.get(i) != Some(&part) {
        return Err(invalid("published index differs from source"));
    }
    if mode == Mode::Apply {
        let mut events = vec![(PART, data(&part)?)];
        if part.done {
            events.push((PUBLISHED, data(&part.progress)?));
        }
        append(live, id, r.next_version, events).await?;
    } else {
        let current = snapshot(live, id, Some(r.position))?;
        if mode == Mode::Verify && (!current.published || current.progress() != r.progress()) {
            return Err(conflict("archive was replaced during verification"));
        }
    }
    let state = if mode == Mode::Apply {
        if part.done { "published" } else { "staging" }
    } else {
        r.state(true)
    };
    page(
        id,
        state,
        &part.progress,
        part.done,
        Some(&part),
        p.totals.source_bytes,
    )
}

#[derive(Deserialize)]
pub(crate) struct Rollback {
    archive_sha256: String,
    backup_sha256: String,
}

pub(super) async fn rollback(live: &Live, id: &str, input: Rollback) -> Result<(), ApiError> {
    let r = snapshot(live, id, None)?;
    let p = r.progress();
    if !(r.published || r.rolled_back)
        || p.archive_sha256.as_ref() != Some(&input.archive_sha256)
        || p.backup_sha256.as_ref() != Some(&input.backup_sha256)
    {
        return Err(conflict("archive or backup pointer differs"));
    }
    if r.rolled_back {
        return Ok(());
    }
    read_blob(live, BACKUP, &input.backup_sha256).await?;
    append(live, id, r.next_version, vec![(ROLLED_BACK, data(&p)?)]).await
}
