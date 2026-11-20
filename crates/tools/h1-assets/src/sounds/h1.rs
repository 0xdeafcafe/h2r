//! Halo 1 Xbox `snd!` tags: the header, pitch ranges and permutations, and
//! where each permutation's samples are in the map file.
//!
//! On Xbox the samples are in the map itself, like bitmap pixels: a
//! permutation's samples data reference holds a **file offset in the
//! decompressed map** and a size. Layouts and their evidence are in
//! `docs/halo1/sounds.md`; offsets below are the ones the disc
//! confirms.

use h1map::{Block, Group, H1Map, Tag, TagId, TagRef};

use crate::sounds::{Error, Result};

/// `snd!`.
pub const SND: Group = Group::new(b"snd!");
/// Bytes in the `snd!` header struct.
pub const HEADER_LEN: usize = 0xa4;
/// Bytes per pitch range.
pub const PITCH_RANGE_LEN: usize = 0x48;
/// Bytes per permutation.
pub const PERMUTATION_LEN: usize = 0x7c;
/// The pitch ranges block in the header.
pub const PITCH_RANGES: usize = 0x98;
/// The permutations block in a pitch range.
pub const PERMUTATIONS: usize = 0x3c;
/// "No next permutation".
pub const NO_NEXT: u16 = 0xffff;

/// Sample compression (`+0x6e` in the header, `+0x28` per permutation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Compression {
    /// Uncompressed 16-bit PCM.
    None,
    /// Xbox ADPCM: 36-byte blocks per channel, 64 samples each.
    XboxAdpcm,
    /// IMA ADPCM (PC).
    ImaAdpcm,
    /// Ogg Vorbis (PC).
    Ogg,
    /// Anything else, as stored.
    Other(u16),
}

impl Compression {
    /// From the stored value.
    pub fn from_raw(v: u16) -> Self {
        match v {
            0 => Self::None,
            1 => Self::XboxAdpcm,
            2 => Self::ImaAdpcm,
            3 => Self::Ogg,
            v => Self::Other(v),
        }
    }
}

/// A data reference (20 bytes): size, flags, file offset, address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DataRef {
    /// Bytes.
    pub size: u32,
    /// Flags (`+4`); zero on every Xbox sound.
    pub flags: u32,
    /// `+8`: for samples, a file offset in the decompressed map.
    pub file_offset: u32,
    /// `+0xc`: a tag data address, when the bytes are in the tag data.
    pub address: u32,
}

impl DataRef {
    fn read(b: &[u8], at: usize) -> Self {
        Self {
            size: u32_at(b, at),
            flags: u32_at(b, at + 4),
            file_offset: u32_at(b, at + 8),
            address: u32_at(b, at + 0xc),
        }
    }
}

/// One permutation (`0x7c` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Permutation {
    /// `+0x00`, up to 31 characters.
    pub name: String,
    /// `+0x20`.
    pub skip_fraction: f32,
    /// `+0x24`, linear.
    pub gain: f32,
    /// `+0x28`.
    pub compression: Compression,
    /// `+0x2a`: the permutation that continues this one's samples, or
    /// [`NO_NEXT`].
    pub next: u16,
    /// `+0x40`: the encoded samples.
    pub samples: DataRef,
    /// `+0x54`: lip-sync (mouth) data.
    pub mouth: DataRef,
    /// One mouth-aperture byte per tick, resident in H1 tag memory.
    pub mouth_samples: Vec<u8>,
    /// `+0x68`: subtitle data.
    pub subtitles: DataRef,
}

/// One pitch range (`0x48` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct PitchRange {
    /// `+0x00`, up to 31 characters.
    pub name: String,
    /// `+0x20`: playback rate multiplier at which the samples sound natural.
    pub natural_pitch: f32,
    /// `+0x24`: the pitch interval this range plays over.
    pub bend_bounds: [f32; 2],
    /// `+0x2c`: permutations that start a sound. Those after it continue
    /// another ([`Permutation::next`]).
    pub actual_permutation_count: u16,
    /// `+0x3c`.
    pub permutations: Vec<Permutation>,
}

/// A Halo 1 `snd!` tag.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    /// `+0x00`.
    pub flags: u32,
    /// `+0x04`.
    pub class: u16,
    /// `+0x06`: 0 is 22,050 Hz, 1 is 44,100 Hz.
    pub sample_rate: u16,
    /// `+0x08`, `+0x0c`: world units; 0 takes the class default.
    pub distance: [f32; 2],
    /// `+0x10`.
    pub skip_fraction: f32,
    /// `+0x14`: random pitch multiplier bounds.
    pub random_pitch_bounds: [f32; 2],
    /// `+0x1c`, `+0x20`: cone angles in radians (`2π` is no cone).
    pub cone_angles: [f32; 2],
    /// `+0x24`.
    pub outer_cone_gain: f32,
    /// `+0x28`.
    pub gain_modifier: f32,
    /// `+0x2c`.
    pub max_bend_per_second: f32,
    /// `+0x3c`, `+0x40`, `+0x44`: skip fraction, gain and pitch modifiers
    /// at scale 0.
    pub zero_modifiers: [f32; 3],
    /// `+0x54`, `+0x58`, `+0x5c`: the same at scale 1.
    pub one_modifiers: [f32; 3],
    /// `+0x6c`: 0 mono, 1 stereo.
    pub encoding: u16,
    /// `+0x6e`.
    pub compression: Compression,
    /// `+0x70`: the sound that replaces this one past the promotion count.
    pub promotion: TagRef,
    /// `+0x80`.
    pub promotion_count: u16,
    /// `+0x84`: milliseconds credited to the promotion counter per request.
    pub promotion_millis: u32,
    /// Path resolved from the authored datum by `read`.
    pub promotion_name: Option<String>,
    /// `+0x98`.
    pub pitch_ranges: Vec<PitchRange>,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}
fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}
fn name_at(b: &[u8], at: usize) -> String {
    let s = &b[at..at + 32];
    let n = s.iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf8_lossy(&s[..n]).into_owned()
}

impl Sound {
    /// Reads the `snd!` tag `tag` of `map`.
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        if tag.group != SND {
            return Err(Error::Invalid(format!(
                "{} is a {}, not a snd!",
                tag.name, tag.group
            )));
        }
        let h = map.read(tag.data_address, HEADER_LEN)?;
        let mut sound = Self::parse(h, |block, len| Ok(map.block(block, len)?))?;
        if let Some(id) = sound.promotion_id() {
            let target = map.tag(id)?;
            if sound.promotion.group != SND || target.group != SND {
                return Err(Error::Invalid(format!(
                    "{}: promotion {id} is not a snd!",
                    tag.name
                )));
            }
            sound.promotion_name = Some(target.name.clone());
        }
        Ok(sound)
    }

    /// Parses a header whose blocks `block(b, elem_len)` resolves.
    pub fn parse<'a>(h: &[u8], block: impl Fn(Block, usize) -> Result<&'a [u8]>) -> Result<Self> {
        if h.len() < HEADER_LEN {
            return Err(Error::Invalid(format!("{} header bytes", h.len())));
        }
        let pr = Block::read(h, PITCH_RANGES).expect("in the header");
        let mut pitch_ranges = Vec::with_capacity(pr.count as usize);
        for r in block(pr, PITCH_RANGE_LEN)?.chunks_exact(PITCH_RANGE_LEN) {
            let pb = Block::read(r, PERMUTATIONS).expect("in the pitch range");
            let permutations = block(pb, PERMUTATION_LEN)?
                .chunks_exact(PERMUTATION_LEN)
                .map(|p| -> Result<Permutation> {
                    let mouth = DataRef::read(p, 0x54);
                    let mouth_samples = if mouth.size == 0 {
                        Vec::new()
                    } else {
                        block(
                            Block {
                                count: mouth.size,
                                address: mouth.address,
                            },
                            1,
                        )?
                        .to_vec()
                    };
                    Ok(Permutation {
                        name: name_at(p, 0),
                        skip_fraction: f32_at(p, 0x20),
                        gain: f32_at(p, 0x24),
                        compression: Compression::from_raw(u16_at(p, 0x28)),
                        next: u16_at(p, 0x2a),
                        samples: DataRef::read(p, 0x40),
                        mouth,
                        mouth_samples,
                        subtitles: DataRef::read(p, 0x68),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            pitch_ranges.push(PitchRange {
                name: name_at(r, 0),
                natural_pitch: f32_at(r, 0x20),
                bend_bounds: [f32_at(r, 0x24), f32_at(r, 0x28)],
                actual_permutation_count: u16_at(r, 0x2c),
                permutations,
            });
        }
        Ok(Self {
            flags: u32_at(h, 0),
            class: u16_at(h, 4),
            sample_rate: u16_at(h, 6),
            distance: [f32_at(h, 8), f32_at(h, 0xc)],
            skip_fraction: f32_at(h, 0x10),
            random_pitch_bounds: [f32_at(h, 0x14), f32_at(h, 0x18)],
            cone_angles: [f32_at(h, 0x1c), f32_at(h, 0x20)],
            outer_cone_gain: f32_at(h, 0x24),
            gain_modifier: f32_at(h, 0x28),
            max_bend_per_second: f32_at(h, 0x2c),
            zero_modifiers: [f32_at(h, 0x3c), f32_at(h, 0x40), f32_at(h, 0x44)],
            one_modifiers: [f32_at(h, 0x54), f32_at(h, 0x58), f32_at(h, 0x5c)],
            encoding: u16_at(h, 0x6c),
            compression: Compression::from_raw(u16_at(h, 0x6e)),
            promotion: TagRef::read(h, 0x70).expect("in the header"),
            promotion_count: u16_at(h, 0x80),
            promotion_millis: u32_at(h, 0x84),
            promotion_name: None,
            pitch_ranges,
        })
    }

    /// The promotion sound's ID, when there is one.
    pub fn promotion_id(&self) -> Option<TagId> {
        (!self.promotion.id.is_none()).then_some(self.promotion.id)
    }

    /// The sample rate in hertz.
    pub fn hz(&self) -> Result<u32> {
        match self.sample_rate {
            0 => Ok(22050),
            1 => Ok(44100),
            n => Err(Error::Unsupported(format!("sample rate {n}"))),
        }
    }

    /// Channels: 1 or 2.
    pub fn channels(&self) -> Result<usize> {
        match self.encoding {
            0 => Ok(1),
            1 => Ok(2),
            n => Err(Error::Unsupported(format!("encoding {n}"))),
        }
    }

    /// The permutations of range `r` that start a sound, each with the run
    /// of permutation indices whose samples play in order: itself, then
    /// its [`Permutation::next`] chain. Fails on a chain that leaves the
    /// range or loops.
    pub fn chains(&self, r: usize) -> Result<Vec<Vec<usize>>> {
        let range = &self.pitch_ranges[r];
        let n = range.permutations.len();
        let actual = usize::from(range.actual_permutation_count);
        if actual > n {
            return Err(Error::Invalid(format!(
                "{actual} actual permutations of {n}"
            )));
        }
        let mut out = Vec::with_capacity(actual);
        for first in 0..actual {
            let mut chain = vec![first];
            let mut at = first;
            while range.permutations[at].next != NO_NEXT {
                at = usize::from(range.permutations[at].next);
                if at >= n || chain.len() > n {
                    return Err(Error::Invalid(format!(
                        "range {r} permutation {first}: chain leaves the range or loops"
                    )));
                }
                chain.push(at);
            }
            out.push(chain);
        }
        Ok(out)
    }
}

/// A permutation's encoded samples in a decompressed map file.
pub fn samples<'f>(file: &'f [u8], p: &Permutation) -> Result<&'f [u8]> {
    let at = p.samples.file_offset as usize;
    file.get(at..at + p.samples.size as usize)
        .ok_or(Error::H1(h1map::Error::Bounds {
            what: "sound samples",
            offset: u64::from(p.samples.file_offset),
            size: u64::from(p.samples.size),
            limit: file.len() as u64,
        }))
}
