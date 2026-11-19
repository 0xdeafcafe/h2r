//! Read original recording rows in signed log order. A continuation can
//! resume inside a chunk. Rewinds and checkpoint cuts start independent
//! segments so a seek never interpolates through either boundary.
use axum::body::Bytes;
use live_proto::game::{self, Frame};
use live_proto::{WireEvent, chunk, pack};
use serde::{Deserialize, Serialize};

use crate::Live;
use crate::app::{ApiError, conflict, invalid};
use crate::store::{Query, RecordedEvent};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Cursor {
    pub after: u64,
    pub offset: usize,
}

pub(super) struct Original {
    pub event: WireEvent,
    pub blob: Option<(String, Bytes)>,
}

#[derive(Default)]
pub(super) struct Batch {
    pub frames: Vec<Frame>,
    pub originals: Vec<Original>,
    pub source_bytes: u64,
    pub json_bytes: usize,
    pub next: Option<Cursor>,
    pub keyframes: Vec<u16>,
}

impl Batch {
    fn accept(&mut self, frame: Frame, id: &str, keyframe: bool) -> Result<bool, ApiError> {
        if frame.game_id != id {
            return Err(invalid("recording contains a frame from another game"));
        }
        let json = serde_json::to_vec(&frame).map_err(|e| invalid(e.to_string()))?;
        if live_proto::archive::starts_segment(&self.frames, self.json_bytes, &frame, json.len()) {
            return Ok(false);
        }
        self.json_bytes += json.len();
        if keyframe {
            self.keyframes.push(self.frames.len() as u16);
        }
        self.frames.push(frame);
        Ok(true)
    }

    fn legacy(&mut self, row: &RecordedEvent, id: &str) -> Result<bool, ApiError> {
        let (frame, bytes, baseline) = match row.body.data.get("packed") {
            Some(packed) => {
                let text = packed
                    .as_str()
                    .ok_or_else(|| invalid("invalid recorded keyframe"))?;
                let bytes = pack::from_base64(text).map_err(|e| invalid(e.to_string()))?;
                let frame = pack::decode_keyframe(&bytes).map_err(|e| invalid(e.to_string()))?;
                (
                    frame,
                    bytes.len(),
                    bytes.first().is_some_and(|b| b & 15 == 7),
                )
            }
            None => {
                let frame = serde_json::from_value(row.body.data.clone())
                    .map_err(|e| invalid(format!("recorded JSON frame: {e}")))?;
                let bytes =
                    serde_json::to_vec(&row.body.data).map_err(|e| invalid(e.to_string()))?;
                (frame, bytes.len(), false)
            }
        };
        if !self.accept(frame, id, baseline)? {
            return Ok(false);
        }
        self.source_bytes += bytes as u64;
        self.originals.push(Original {
            event: row.to_wire(),
            blob: None,
        });
        Ok(true)
    }

    async fn packed(
        &mut self,
        live: &Live,
        row: &RecordedEvent,
        id: &str,
        start: Cursor,
    ) -> Result<Option<Cursor>, ApiError> {
        let meta: chunk::ChunkMeta = serde_json::from_value(row.body.data.clone())
            .map_err(|e| invalid(format!("chunk metadata: {e}")))?;
        let bytes = live.stored_chunk(id, &meta).await?;
        let (header, _) = chunk::header(&bytes).map_err(|e| invalid(e.to_string()))?;
        if header.frames != u64::from(meta.frames)
            || header.from_tick != meta.from_tick
            || header.to_tick != meta.to_tick
            || header.raw_bytes != meta.raw_bytes
            || header.codec != meta.codec
            || bytes.len() as u64 != meta.bytes
            || meta.frames == 0
        {
            return Err(invalid("original chunk disagrees with signed index"));
        }
        if start.offset >= meta.frames as usize {
            return Err(invalid("source cursor is outside the chunk"));
        }
        let raw = chunk::container(&bytes).map_err(|e| invalid(e.to_string()))?;
        let mut decoder = pack::Decoder::new();
        let mut count = 0;
        let mut next = None;
        for record in pack::records(&raw) {
            let record = record.map_err(|e| invalid(e.to_string()))?;
            let decoded = decoder.decode(record).map_err(|e| invalid(e.to_string()))?;
            let baseline = decoded.keyframe && (record[0] & 15) == 7;
            let frame = decoded.frame;
            if count == 0 && frame.tick != meta.from_tick
                || count + 1 == meta.frames as usize && frame.tick != meta.to_tick
            {
                return Err(invalid("original chunk tick disagrees with signed index"));
            }
            if count >= start.offset && !self.accept(frame, id, baseline)? {
                next = Some(Cursor {
                    after: start.after,
                    offset: count,
                });
                break;
            }
            count += 1;
        }
        if next.is_none() && count != meta.frames as usize {
            return Err(invalid(
                "original chunk frame count disagrees with signed index",
            ));
        }
        if start.offset == 0 && count > 0 {
            self.source_bytes += meta.bytes;
            self.originals.push(Original {
                event: row.to_wire(),
                blob: Some((meta.sha256, bytes)),
            });
        }
        Ok(next)
    }
}

pub(super) async fn batch(
    live: &Live,
    id: &str,
    revision: u64,
    mut cursor: Cursor,
) -> Result<Batch, ApiError> {
    if cursor.after > revision {
        return Err(conflict("source cursor is past the sealed recording"));
    }
    let mut batch = Batch::default();
    let mut query = Query {
        after: cursor.after,
        limit: 8,
        stream: Some(game::stream(id)),
        types: vec![game::GAME_FRAME.into(), chunk::FRAME_CHUNK.into()],
        exclude: Vec::new(),
    };
    loop {
        let rows = live.store.read(&query).await?;
        let last_page = rows.len() < query.limit;
        for row in rows {
            if row.position > revision {
                return Err(conflict("recording changed"));
            }
            if row.body.kind == chunk::FRAME_CHUNK {
                if let Some(next) = batch.packed(live, &row, id, cursor).await? {
                    batch.next = Some(next);
                    return Ok(batch);
                }
            } else {
                if cursor.offset != 0 {
                    return Err(invalid("invalid legacy frame cursor"));
                }
                if !batch.legacy(&row, id)? {
                    batch.next = Some(cursor);
                    return Ok(batch);
                }
            }
            cursor = Cursor {
                after: row.position,
                offset: 0,
            };
        }
        if last_page {
            return Ok(batch);
        }
        query.after = cursor.after;
    }
}
