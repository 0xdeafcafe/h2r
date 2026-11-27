//! The Forge record a built map carries (docs/game/forge-map-variants.md,
//! "The Forge record"): the properties with no retail field that the game
//! acts on in play, in a table at the map's tail, as the areas record is
//! (`game_markers::record`).
//!
//! A match on a built variant has no document, so the build writes what
//! the game needs from it here: each item spawner's spare clips and place
//! at start, and each starting point's weight. The rows name entries by
//! their index in the built map, which the record belongs to.
//!
//! ```text
//! record  = "H2FR" version:u8 count:u16le row*count crc:u32le
//! row     = field:u8 index:u16le value:u8
//! field   = 0 spare clips  (an item spawner; value the clips, 0..=SPARE_CLIPS_MAX)
//!           1 weight       (a starting point; value 0 Low, 1 High)
//!           2 held back    (an item spawner placed at start: No; value 0)
//!           3 body refused (a Forge body the kit left out, index its
//!                          `bodies::Body` code; value its `bodies::Refused`)
//! trailer = length:u32le "H2FE"                ; after the record
//! ```
//!
//! `crc` is the FNV-1a of the bytes before it, `length` the record's own.
//! The build writes the Forge record before the areas record; a reader
//! walks the tail's records from the end (`game_markers::record::tail_record`).
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek};

use crate::bodies::{Body, Refused};
use crate::{BlockKind, ForgeDoc, Result, SPARE_CLIPS_MAX, SpawnWeight, resolve};

pub const MAGIC: [u8; 4] = *b"H2FR";
pub const END: [u8; 4] = game_markers::record::TAIL_ENDS[1];
pub const VERSION: u8 = 1;
/// A row's field.
pub const SPARE_CLIPS: u8 = 0;
pub const WEIGHT: u8 = 1;
pub const HELD_BACK: u8 = 2;
pub const BODY_REFUSED: u8 = 3;
/// Bytes per row.
const ROW: usize = 4;

/// What a built map's game reads from its document, by entry index in the
/// built map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForgeRecord {
    /// Each item spawner's spare clips.
    pub spare_clips: BTreeMap<u32, u8>,
    /// Each starting point's weight other than Normal.
    pub weights: BTreeMap<u32, SpawnWeight>,
    /// The item spawners that are not placed at a round's start.
    pub held_back: BTreeSet<u32>,
    /// The Forge bodies a kit build left out, and why (`forge_build`'s
    /// `bodies`). A kit map with none here carries all three.
    pub bodies_refused: BTreeMap<Body, Refused>,
}

/// A record that does not read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    #[error("the Forge record is cut short")]
    Truncated,
    #[error("the Forge record has the wrong magic")]
    Magic,
    #[error("the Forge record is version {0}, not {VERSION}")]
    Version(u8),
    #[error("the Forge record's checksum does not match")]
    Checksum,
    #[error("the Forge record has a row of field {0} it does not know")]
    Field(u8),
    #[error("the Forge record's row of field {field} has the value {value}, out of range")]
    Value { field: u8, value: u8 },
}

fn fnv(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0x811c_9dc5u32, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

impl ForgeRecord {
    pub fn is_empty(&self) -> bool {
        self.spare_clips.is_empty()
            && self.weights.is_empty()
            && self.held_back.is_empty()
            && self.bodies_refused.is_empty()
    }

    /// The record of `doc` over the map it builds: each property resolved to
    /// its entry's final index. A removed entry has none.
    pub fn of(doc: &ForgeDoc, base_counts: &BTreeMap<BlockKind, u32>) -> Result<Self> {
        let resolved = resolve(&doc.ops, base_counts)?;
        let at = |end| resolved.index(end);
        Ok(Self {
            spare_clips: crate::spare_clips(&doc.ops)
                .into_iter()
                .filter_map(|(end, clips)| Some((at(end)?, clips)))
                .collect(),
            weights: crate::spawn_weights(&doc.ops)
                .into_iter()
                .filter_map(|(end, weight)| Some((at(end)?, weight)))
                .collect(),
            held_back: crate::held_at_start(&doc.ops)
                .into_iter()
                .filter_map(at)
                .collect(),
            bodies_refused: BTreeMap::new(),
        })
    }

    /// The record with its trailer, ready to append to a map. An index past
    /// a `u16` is no entry of a retail block and is not written.
    pub fn encode(&self) -> Vec<u8> {
        let mut rows = Vec::new();
        let mut row = |field: u8, index: u32, value: u8| {
            if let Ok(index) = u16::try_from(index) {
                rows.push(field);
                rows.extend_from_slice(&index.to_le_bytes());
                rows.push(value);
            }
        };
        for (&i, &clips) in &self.spare_clips {
            row(SPARE_CLIPS, i, clips);
        }
        for (&i, &weight) in &self.weights {
            row(WEIGHT, i, u8::from(weight == SpawnWeight::High));
        }
        for &i in &self.held_back {
            row(HELD_BACK, i, 0);
        }
        for (&body, &why) in &self.bodies_refused {
            row(BODY_REFUSED, u32::from(body.code()), why.code());
        }
        let mut w = Vec::with_capacity(rows.len() + 15);
        w.extend_from_slice(&MAGIC);
        w.push(VERSION);
        w.extend_from_slice(&((rows.len() / ROW) as u16).to_le_bytes());
        w.extend_from_slice(&rows);
        let crc = fnv(&w);
        w.extend_from_slice(&crc.to_le_bytes());
        let len = w.len() as u32;
        w.extend_from_slice(&len.to_le_bytes());
        w.extend_from_slice(&END);
        w
    }

    /// One record (without its trailer).
    pub fn decode(record: &[u8]) -> std::result::Result<Self, RecordError> {
        if record.len() < 11 {
            return Err(RecordError::Truncated);
        }
        let (body, crc) = record.split_at(record.len() - 4);
        if fnv(body).to_le_bytes() != crc {
            return Err(RecordError::Checksum);
        }
        if body[..4] != MAGIC {
            return Err(RecordError::Magic);
        }
        if body[4] != VERSION {
            return Err(RecordError::Version(body[4]));
        }
        let n = usize::from(u16::from_le_bytes([body[5], body[6]]));
        let rows = &body[7..];
        if rows.len() != n * ROW {
            return Err(RecordError::Truncated);
        }
        let mut out = Self::default();
        for r in rows.chunks_exact(ROW) {
            let (field, index, value) = (r[0], u32::from(u16::from_le_bytes([r[1], r[2]])), r[3]);
            let bad = RecordError::Value { field, value };
            match field {
                SPARE_CLIPS if value <= SPARE_CLIPS_MAX => {
                    out.spare_clips.insert(index, value);
                }
                WEIGHT if value <= 1 => {
                    let weight = if value == 1 { SpawnWeight::High } else { SpawnWeight::Low };
                    out.weights.insert(index, weight);
                }
                HELD_BACK if value == 0 => {
                    out.held_back.insert(index);
                }
                BODY_REFUSED => {
                    let body = u8::try_from(index).ok().and_then(Body::from_code);
                    match (body, Refused::from_code(value)) {
                        (Some(body), Some(why)) => {
                            out.bodies_refused.insert(body, why);
                        }
                        _ => return Err(bad),
                    }
                }
                SPARE_CLIPS | WEIGHT | HELD_BACK => return Err(bad),
                other => return Err(RecordError::Field(other)),
            }
        }
        Ok(out)
    }

    /// The last Forge record at the end of `file`, or none when its tail
    /// has none.
    pub fn read_tail<R: Read + Seek>(file: &mut R) -> std::result::Result<Option<Self>, RecordError> {
        game_markers::record::tail_record(file, END)
            .map_err(|_| RecordError::Truncated)?
            .map(|r| Self::decode(&r))
            .transpose()
    }
}
