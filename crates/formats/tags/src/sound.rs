//! Halo 2 sound tags: the `snd!` header, the per-map `ugh!` gestalt it indexes,
//! and the raw encoded sample chunks both ultimately point at.
//!
//! This module is deliberately self-contained (hand-written fixed codecs, no
//! schema/build.rs involvement) so it merges independently of other tag groups.
//! Layout evidence and its provenance are recorded in `docs/formats/sound.md`.
//!
//! Model: a retail Xbox `snd!` tag is only 20 bytes. It selects a run of
//! *pitch ranges* inside the map's single `ugh!` tag (the "sound cache file
//! gestalt"); each pitch range owns a run of *permutations*; each permutation
//! owns a run of *chunks*, and a chunk is a `(raw pointer, size)` span of
//! encoded sample data in the current map or one of the shared maps. A sound
//! whose dialogue is lip-synced also names an *extra info* element of the
//! gestalt, whose raw resource holds the mouth curves: one jaw-aperture byte
//! per 30 Hz tick per permutation, read by [`Gestalt::mouth_curve`].
use crate::{Error, Result, wire::TagBlock};
use map::{FourCC, Map, RawPointer, TagId};
use std::io::{Read, Seek};

/// Converted Halo 1 impulse admission rule appended to the native snd! header.
/// The final two words are map-local mixer state, initially zero.
pub mod h1_promotion {
    pub const SIGNATURE: u32 = 0x14;
    pub const TARGET: u32 = 0x18;
    pub const COUNT: u32 = 0x1c;
    pub const MILLIS: u32 = 0x20;
    pub const CREDIT: u32 = 0x24;
    pub const LAST_TIME: u32 = 0x28;
    pub const SIZE: u32 = 0x2c;
    pub const MAGIC: u32 = u32::from_le_bytes(*b"H1PR");

    pub fn present(bytes: &[u8]) -> bool {
        bytes.len() == SIZE as usize
            && bytes[SIGNATURE as usize..TARGET as usize] == MAGIC.to_le_bytes()
    }
}

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
    /// channel count is not given by this byte; see `docs/formats/sound.md`.
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
    /// +0xb..+0xe, uninterpreted (scale, promotion, custom playback).
    pub unknown: [u8; 3],
    /// Index into the gestalt's extra-info block (+0xe): the lip-sync
    /// resource of this sound's permutations. `0xffff` is none.
    pub extra_info: u16,
    /// Maximum play time (+0x10); milliseconds (see docs/formats/sound.md).
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
            unknown: b[11..14].try_into().expect("slice length"),
            extra_info: u16_at(b, 0xe),
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
    /// +0..+8 raw (name index, skip fraction, gain, permutation-order bytes;
    /// `+5` is the extra-info descriptor while the sound has one).
    pub raw: [u8; 8],
    /// Total encoded bytes over this permutation's chunks (+8).
    pub encoded_size: u32,
    pub first_chunk: u16,
    pub chunk_count: u16,
}

impl Permutation {
    /// The signed extra-info descriptor index at `+5`: which element of the
    /// sound's extra-info resource holds this permutation's mouth curve. A
    /// negative index is no descriptor. Meaningless while the sound has no
    /// extra info, where the byte is the permutation order instead.
    pub fn extra_info_descriptor(&self) -> Option<usize> {
        usize::try_from(i8::from_le_bytes([self.raw[5]])).ok()
    }
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

/// `ugh!` extra info (block 10, element `0x2c`): the lip-sync resource one
/// sound's `snd!` `+0xe` index names. Held as one `hklb` raw resource
/// (docs/halo1/sounds.md, "Dialogue mouth curves").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtraInfo {
    /// `+8`: the resource's raw pointer (file selector in the top two bits).
    pub pointer: RawPointer,
    /// `+0xc`: the resource's whole byte length.
    pub size: u32,
    /// The `+0x18` child block: one element per buffer of the resource.
    pub locators: Vec<Locator>,
}

/// One buffer of an extra-info resource (a `0x10` element of the `+0x18`
/// child block): 8 head bytes the loader reads, then the buffer's length
/// and where it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Locator {
    /// `+0`: kind and fields, raw.
    pub head: [u8; 8],
    /// `+8`: the buffer's byte length.
    pub size: u32,
    /// `+0xc`: the offset of the buffer from the resource's data section
    /// (its first `crsr`, at `+0x18`).
    pub offset: u32,
}

/// The decoded parts of a map's `ugh!` tag this stage needs.
#[derive(Clone, Debug, Default)]
pub struct Gestalt {
    pub pitch_ranges: Vec<PitchRange>,
    pub permutations: Vec<Permutation>,
    pub chunks: Vec<Chunk>,
    pub extra_infos: Vec<ExtraInfo>,
}

const GESTALT_HEADER: usize = 0x58;
const BLOCK_PITCH_RANGES: usize = 4;
const BLOCK_PERMUTATIONS: usize = 5;
const BLOCK_CHUNKS: usize = 8;
const BLOCK_EXTRA_INFOS: usize = 10;
/// An extra-info element (`ugh!` block 10).
const EXTRA_INFO_LEN: usize = 0x2c;
/// A locator element of an extra-info's `+0x18` child block.
const LOCATOR_LEN: usize = 0x10;
/// The mouth curve's rate: one aperture byte per tick at 30 Hz (E, the
/// sampler at `0x218e50` scales it by 1/255; docs/halo1/sounds.md,
/// "Dialogue mouth curves").
pub const MOUTH_HZ: u32 = 30;
/// The head of the locator that finds an extra-info's curve data buffer
/// (V, retail 01b; as the converter writes, h1-assets `sounds::mouth`).
const DATA_LOCATOR: [u8; 8] = [1, 2, 0, 0, 0, 0, 0xff, 0xff];
/// The head of the locator that finds the 16-byte mouth descriptors (V,
/// retail 01b; as the converter writes).
const DESCRIPTOR_LOCATOR: [u8; 8] = [0, 2, 8, 0, 8, 0, 16, 0];

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

/// A run of `count` elements of `stride` bytes at a map `address`.
fn run(bytes: &[u8], base: u32, count: u32, address: u32, stride: usize) -> Result<&[u8]> {
    if count == 0 {
        return Ok(&[]);
    }
    let size = u64::from(count) * stride as u64;
    let start = address.checked_sub(base).ok_or(Error::Bounds { address, size })? as usize;
    let end = start
        .checked_add(size as usize)
        .ok_or(Error::Bounds { address, size })?;
    bytes.get(start..end).ok_or(Error::Bounds { address, size })
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
        let (x, _) = block(bytes, base, BLOCK_EXTRA_INFOS, EXTRA_INFO_LEN)?;
        let extra_infos = x
            .as_chunks::<EXTRA_INFO_LEN>()
            .0
            .iter()
            .map(|e| {
                // The child block at +0 has no element on any retail map and
                // no known layout: present is refused, not skipped.
                if u32_at(e, 0) != 0 {
                    return Err(Error::Invalid("an extra info has a child block of unknown layout"));
                }
                let locators = run(bytes, base, u32_at(e, 0x18), u32_at(e, 0x1c), LOCATOR_LEN)?
                    .as_chunks::<LOCATOR_LEN>()
                    .0
                    .iter()
                    .map(|l| Locator {
                        head: l[..8].try_into().expect("slice length"),
                        size: u32_at(l, 8),
                        offset: u32_at(l, 12),
                    })
                    .collect();
                Ok(ExtraInfo {
                    pointer: RawPointer::decode(u32_at(e, 8))
                        .ok_or(Error::Invalid("null extra-info resource pointer"))?,
                    size: u32_at(e, 0xc),
                    locators,
                })
            })
            .collect::<Result<_>>()?;
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
            extra_infos,
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

    /// The mouth curve of permutation `p` of the sound with header `h`: one
    /// jaw-aperture byte per 30 Hz tick ([`MOUTH_HZ`]), read out of the
    /// extra-info resource `h`'s index names, as the loader follows it: both
    /// locators (the curve data, then the 16-byte descriptors), the
    /// descriptor the permutation's signed `+5` index picks, then that
    /// descriptor's span of the data. `Ok(None)` when the permutation is
    /// unvoiced: the sound has no extra info, the descriptor index is
    /// negative, or the descriptor's mouth length is zero.
    pub fn mouth_curve(
        &self,
        h: &SoundHeader,
        p: &Permutation,
        r: &mut dyn ResourceReader,
    ) -> Result<Option<Vec<u8>>> {
        if h.extra_info == u16::MAX {
            return Ok(None);
        }
        let Some(descriptor) = p.extra_info_descriptor() else {
            return Ok(None);
        };
        let e = self
            .extra_infos
            .get(usize::from(h.extra_info))
            .ok_or(Error::Invalid("extra info index out of gestalt"))?;
        r.check(e.pointer, e.size)?;
        let raw = r.read_raw(e.pointer, e.size)?;
        // The resource is one `hklb` block: a 0x18-byte header, then the
        // data section (each buffer preceded by `crsr`), ending in `fklb`.
        if raw.len() < 0x20 || raw[..4] != *b"hklb" || raw[raw.len() - 4..] != *b"fklb" {
            return Err(Error::Invalid("the extra-info resource is not an hklb block"));
        }
        if raw.len() != 0x1c + u32_at(&raw, 4) as usize {
            return Err(Error::Invalid("the extra-info resource disagrees with its size word"));
        }
        if e.locators.len() != 2 {
            return Err(Error::Invalid(
                "the extra-info resource does not have two locators (the data and the descriptor)",
            ));
        }
        let buffer = |l: &Locator, head: &[u8; 8]| -> Result<&[u8]> {
            if &l.head != head {
                return Err(Error::Invalid(
                    "an extra-info locator is not the data or descriptor one the loader follows",
                ));
            }
            let start = 0x18 + l.offset as usize;
            let end = start.saturating_add(l.size as usize);
            let s = raw
                .get(start..end)
                .ok_or(Error::Invalid("an extra-info buffer runs outside the resource"))?;
            if raw.get(start - 4..start) != Some(&b"crsr"[..]) {
                return Err(Error::Invalid("an extra-info buffer does not follow a crsr"));
            }
            Ok(s)
        };
        let data = buffer(&e.locators[0], &DATA_LOCATOR)?;
        let descriptors = buffer(&e.locators[1], &DESCRIPTOR_LOCATOR)?;
        let d = descriptors
            .as_chunks::<16>()
            .0
            .get(descriptor)
            .ok_or(Error::Invalid("extra info descriptor index out of the resource"))?;
        let start = u32_at(d, 0) as usize;
        let curve = data
            .get(start..start.saturating_add(u32_at(d, 4) as usize))
            .ok_or(Error::Invalid("a mouth curve runs outside the extra-info data"))?;
        Ok((!curve.is_empty()).then(|| curve.to_vec()))
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
