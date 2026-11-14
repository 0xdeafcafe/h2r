//! Halo 2 sound tags: the `snd!` header, the per-map `ugh!` gestalt it indexes,
//! and the raw encoded sample chunks both ultimately point at.
//!
//! This module is deliberately self-contained (hand-written fixed codecs, no
//! schema/build.rs involvement) so it merges independently of other tag groups.
//! Layout evidence and its provenance are recorded in `docs/sound.md`.
//!
//! Model: a retail Xbox `snd!` tag is only 20 bytes. It selects a run of
//! *pitch ranges* inside the map's single `ugh!` tag (the "sound cache file
//! gestalt"); each pitch range owns a run of *permutations*; each permutation
//! owns a run of *chunks*, and a chunk is a `(raw pointer, size)` span of
//! encoded sample data in the current map or one of the shared maps.
use crate::{Error, Result, wire::TagBlock};
use map::{FourCC, Map, RawPointer, TagId};
use std::io::{Read, Seek};

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Sample rate selector (`snd!` +3). Names are source-described (S); the 44.1 kHz
/// value is corroborated by the engine's buffer setup (`0x0021e447`, `0xAC44`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleRate {
    Hz22050,
    Hz44100,
    Hz32000,
    Unknown(u8),
}
impl SampleRate {
    pub fn hz(self) -> Option<u32> {
        match self {
            Self::Hz22050 => Some(22050),
            Self::Hz44100 => Some(44100),
            Self::Hz32000 => Some(32000),
            Self::Unknown(_) => None,
        }
    }
}
/// Channel layout selector (`snd!` +4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Mono,
    Stereo,
    /// Source name "codec". Retail data shows it holds Xbox ADPCM streams whose
    /// channel count is not given by this byte; see `docs/sound.md`.
    Codec,
    Unknown(u8),
}
/// Sample compression selector (`snd!` +5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    NoneBigEndian,
    XboxAdpcm,
    ImaAdpcm,
    NoneLittleEndian,
    Wma,
    Unknown(u8),
}

/// The 20-byte `snd!` tag. Fields not interpreted are retained raw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundHeader {
    pub flags: u16,
    pub class: u8,
    pub sample_rate: SampleRate,
    pub encoding: Encoding,
    pub compression: Compression,
    /// Index into the gestalt's playback-parameters block (+6). Source-only (S).
    pub playback_index: u16,
    /// First pitch range in the gestalt (+8). Disc-checked: ranges chain to
    /// permutations whose chunk sizes sum exactly.
    pub first_pitch_range: u16,
    /// Number of pitch ranges (+0xa).
    pub pitch_range_count: u8,
    /// +0xb..+0x10, uninterpreted.
    pub unknown: [u8; 5],
    /// Maximum play time (+0x10); milliseconds (see docs/sound.md).
    pub max_play_time: u32,
}
impl SoundHeader {
    pub const SIZE: usize = 0x14;
    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() < Self::SIZE {
            return Err(Error::RecordSize { actual: b.len(), expected: Self::SIZE });
        }
        Ok(Self {
            flags: u16_at(b, 0),
            class: b[2],
            sample_rate: match b[3] {
                0 => SampleRate::Hz22050,
                1 => SampleRate::Hz44100,
                2 => SampleRate::Hz32000,
                n => SampleRate::Unknown(n),
            },
            encoding: match b[4] {
                0 => Encoding::Mono,
                1 => Encoding::Stereo,
                2 => Encoding::Codec,
                n => Encoding::Unknown(n),
            },
            compression: match b[5] {
                0 => Compression::NoneBigEndian,
                1 => Compression::XboxAdpcm,
                2 => Compression::ImaAdpcm,
                3 => Compression::NoneLittleEndian,
                4 => Compression::Wma,
                n => Compression::Unknown(n),
            },
            playback_index: u16_at(b, 6),
            first_pitch_range: u16_at(b, 8),
            pitch_range_count: b[10],
            unknown: b[11..16].try_into().expect("slice length"),
            max_play_time: u32_at(b, 0x10),
        })
    }
}

/// `ugh!` pitch range, 12 bytes. Only the permutation run is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PitchRange {
    /// +0..+8 raw (name/parameter indices and encoded-permutation words).
    pub raw: [u8; 8],
    pub first_permutation: u16,
    pub permutation_count: u16,
}
/// `ugh!` permutation, 16 bytes. Only size and chunk run are interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permutation {
    /// +0..+8 raw (name index, skip fraction, gain, permutation-order bytes).
    pub raw: [u8; 8],
    /// Total encoded bytes over this permutation's chunks (+8).
    pub encoded_size: u32,
    pub first_chunk: u16,
    pub chunk_count: u16,
}
/// `ugh!` chunk, 12 bytes: a raw-resource span of encoded samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// +0 raw pointer (file selector in the top two bits).
    pub pointer: RawPointer,
    /// +4 size with the top bit (a flag, meaning unknown) masked off.
    pub size: u32,
    /// +4 bit 31.
    pub flag: bool,
    /// +8, runtime slot (observed 0xffffffff on disc).
    pub runtime: u32,
}

/// The decoded parts of a map's `ugh!` tag this stage needs.
#[derive(Clone, Debug, Default)]
pub struct Gestalt {
    pub pitch_ranges: Vec<PitchRange>,
    pub permutations: Vec<Permutation>,
    pub chunks: Vec<Chunk>,
}

const GESTALT_HEADER: usize = 0x58;
const BLOCK_PITCH_RANGES: usize = 4;
const BLOCK_PERMUTATIONS: usize = 5;
const BLOCK_CHUNKS: usize = 8;

fn block(
    bytes: &[u8],
    base: u32,
    idx: usize,
    stride: usize,
) -> Result<(&[u8], u32)> {
    let hb = &bytes[idx * 8..idx * 8 + 8];
    let b = TagBlock { count: u32_at(hb, 0), address: u32_at(hb, 4) };
    if b.count == 0 {
        return Ok((&[], 0));
    }
    let size = u64::from(b.count) * stride as u64;
    let start = b
        .address
        .checked_sub(base)
        .ok_or(Error::Bounds { address: b.address, size })? as usize;
    let end = start
        .checked_add(size as usize)
        .ok_or(Error::Bounds { address: b.address, size })?;
    let s = bytes.get(start..end).ok_or(Error::Bounds { address: b.address, size })?;
    Ok((s, b.count))
}

impl Gestalt {
    /// Decode from the raw payload bytes of an `ugh!` tag loaded at `base`.
    pub fn decode(bytes: &[u8], base: u32) -> Result<Self> {
        if bytes.len() < GESTALT_HEADER {
            return Err(Error::RecordSize { actual: bytes.len(), expected: GESTALT_HEADER });
        }
        let (r, _) = block(bytes, base, BLOCK_PITCH_RANGES, 12)?;
        let (p, _) = block(bytes, base, BLOCK_PERMUTATIONS, 16)?;
        let (c, _) = block(bytes, base, BLOCK_CHUNKS, 12)?;
        Ok(Self {
            pitch_ranges: r
                .as_chunks::<12>()
                .0
                .iter()
                .map(|e| PitchRange {
                    raw: e[..8].try_into().expect("slice length"),
                    first_permutation: u16_at(e, 8),
                    permutation_count: u16_at(e, 10),
                })
                .collect(),
            permutations: p
                .as_chunks::<16>()
                .0
                .iter()
                .map(|e| Permutation {
                    raw: e[..8].try_into().expect("slice length"),
                    encoded_size: u32_at(e, 8),
                    first_chunk: u16_at(e, 12),
                    chunk_count: u16_at(e, 14),
                })
                .collect(),
            chunks: c
                .as_chunks::<12>()
                .0
                .iter()
                .map(|e| {
                    Ok(Chunk {
                        pointer: RawPointer::decode(u32_at(e, 0))
                            .ok_or(Error::Invalid("null sound chunk pointer"))?,
                        size: u32_at(e, 4) & 0x7fff_ffff,
                        flag: u32_at(e, 4) >> 31 != 0,
                        runtime: u32_at(e, 8),
                    })
                })
                .collect::<Result<_>>()?,
        })
    }

    /// Read the (single) `ugh!` tag of a map.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let tag = map
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"ugh!"))
            .ok_or(Error::Invalid("map has no ugh! sound gestalt"))?;
        let (id, address) = (tag.id, tag.location.ok_or(map::Error::NoTagData)?.address);
        let bytes = map.read_tag_bytes(id)?;
        Self::decode(&bytes, address)
    }

    /// Chunks of a permutation (borrowed; never copied).
    pub fn chunks_of(&self, p: &Permutation) -> &[Chunk] {
        let first = usize::from(p.first_chunk);
        self.chunks.get(first..first + usize::from(p.chunk_count)).unwrap_or(&[])
    }

    /// Resolve a sound's pitch ranges and permutations, checking every index
    /// and that each permutation's chunk sizes sum to its stated size.
    /// Aliased metadata can make many sounds share runs, so the work done per
    /// sound is bounded by [`MAX_PERMUTATIONS_PER_SOUND`] and
    /// [`MAX_CHUNKS_PER_SOUND`] (counted with multiplicity), not by the tag.
    pub fn resolve(&self, h: &SoundHeader) -> Result<Sound> {
        let first = usize::from(h.first_pitch_range);
        let n = usize::from(h.pitch_range_count);
        let ranges = self
            .pitch_ranges
            .get(first..first + n)
            .ok_or(Error::Invalid("pitch range index out of gestalt"))?;
        let (mut perm_budget, mut chunk_budget) = (MAX_PERMUTATIONS_PER_SOUND, MAX_CHUNKS_PER_SOUND);
        let mut out = Vec::with_capacity(ranges.len());
        for r in ranges {
            let fp = usize::from(r.first_permutation);
            let count = usize::from(r.permutation_count);
            perm_budget = perm_budget
                .checked_sub(count)
                .ok_or(Error::Invalid("sound exceeds permutation budget"))?;
            let perms = self
                .permutations
                .get(fp..fp + count)
                .ok_or(Error::Invalid("permutation index out of gestalt"))?;
            for p in perms {
                chunk_budget = chunk_budget
                    .checked_sub(usize::from(p.chunk_count))
                    .ok_or(Error::Invalid("sound exceeds chunk budget"))?;
                if usize::from(p.first_chunk) + usize::from(p.chunk_count) > self.chunks.len() {
                    return Err(Error::Invalid("chunk index out of gestalt"));
                }
                let total: u64 = self.chunks_of(p).iter().map(|c| u64::from(c.size)).sum();
                if total != u64::from(p.encoded_size) {
                    return Err(Error::Invalid("permutation size differs from its chunks"));
                }
            }
            out.push(ResolvedRange { range: *r, permutations: perms.to_vec() });
        }
        Ok(Sound { header: h.clone(), ranges: out })
    }
}

/// Upper bounds on aliased-metadata work for one sound (real data: at most a
/// few dozen permutations and a few hundred chunks).
pub const MAX_PERMUTATIONS_PER_SOUND: usize = 4096;
pub const MAX_CHUNKS_PER_SOUND: usize = 1 << 16;
/// Largest encoded permutation read into memory.
pub const MAX_ENCODED_BYTES: u64 = 256 << 20;

#[derive(Clone, Debug)]
pub struct ResolvedRange {
    pub range: PitchRange,
    pub permutations: Vec<Permutation>,
}
/// A `snd!` with its gestalt links followed. Permutations are small records;
/// their chunks stay in the [`Gestalt`] (see [`Gestalt::chunks_of`]).
#[derive(Clone, Debug)]
pub struct Sound {
    pub header: SoundHeader,
    pub ranges: Vec<ResolvedRange>,
}

/// Source of raw resource bytes. The caller owns file selection (the disc's
/// `mainmenu.map`, `shared.map`, ...); this crate never opens files itself.
pub trait ResourceReader {
    /// Check that the span lies inside its file, without reading or allocating.
    fn check(&self, pointer: RawPointer, size: u32) -> Result<()>;
    fn read_raw(&mut self, pointer: RawPointer, size: u32) -> Result<Vec<u8>>;
}
impl Gestalt {
    /// Concatenate a permutation's encoded chunks. Every chunk is bounds-checked
    /// against its resource file, and the total against [`MAX_ENCODED_BYTES`],
    /// before anything is allocated.
    pub fn read_encoded(&self, p: &Permutation, r: &mut dyn ResourceReader) -> Result<Vec<u8>> {
        let chunks = self.chunks_of(p);
        let mut total = 0u64;
        for c in chunks {
            r.check(c.pointer, c.size)?;
            total += u64::from(c.size);
        }
        if total > MAX_ENCODED_BYTES || total != u64::from(p.encoded_size) {
            return Err(Error::Invalid("encoded permutation size out of bounds"));
        }
        let mut out = Vec::with_capacity(total as usize);
        for c in chunks {
            out.extend(r.read_raw(c.pointer, c.size)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "sound_tests.rs"]
mod tests;

/// Read and decode a `snd!` tag header.
pub fn read_header<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<SoundHeader> {
    if map.tag(id)?.group != FourCC(*b"snd!") {
        return Err(Error::UnsupportedGroup(map.tag(id)?.group));
    }
    SoundHeader::decode(&map.read_tag_bytes(id)?)
}
