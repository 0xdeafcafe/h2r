//! Halo 1 `snd!` -> Halo 2 `snd!` plus `ugh!` gestalt entries.
//!
//! A Halo 2 `snd!` is 20 bytes of indices into the map's gestalt
//! ([`crate::sounds::gestalt`]). Converting a sound therefore has two halves:
//!
//! 1. [`convert_sound`] reads everything a Halo 2 sound needs from the Halo
//!    1 tag into a [`ConvertedSound`], independent of any map: the header
//!    fields, one playback and one scale element, and per pitch range its
//!    parameters and permutations, each permutation's samples already cut
//!    into chunks.
//! 2. [`ConvertedSound::add_to`] appends those entries to a
//!    [`SoundGestalt`], adds the chunks to the [`TagSet`] as raw resources,
//!    and returns the `snd!` as a [`Converted`] whose indices point at them.
//!    [`install_sounds`] does this for a list of sounds and writes the
//!    gestalt back.
//!
//! Samples are copied as stored. A Halo 1 permutation chain (a permutation
//! and the ones its `next` index continues with) becomes one Halo 2
//! permutation whose chunks are those samples, in order, cut at
//! [`CHUNK_LEN`] (a whole number of blocks for mono and stereo).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::models::Converted;
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, TagId};
use tags::sound::h1_promotion;

use crate::sounds::gestalt::{Chunk, ChunkPointer, SoundGestalt, blocks};
use crate::sounds::h1::{Compression, Sound};
use crate::sounds::{Error, Result, class};

/// Largest chunk: retail Halo 2's streaming chunk size, 910 mono or 455
/// stereo ADPCM blocks.
pub const CHUNK_LEN: usize = 0x7ff8;
/// File alignment of a chunk's raw resource.
pub const CHUNK_ALIGN: u32 = 0x200;

/// Halo 2 `snd!` fields (20 bytes).
pub mod snd {
    /// `u16` flags. Bits 0 and 1 are Halo 1's; the engine reads the three
    /// below ([`CLASS_MIN_DISTANCE`], [`CLASS_MAX_DISTANCE`],
    /// [`NO_CONE`]).
    pub const FLAGS: usize = 0;
    /// Flag: the minimum distance is the class's (class `+0x18`), not the
    /// playback element's `+0x00` (E `0x00124fc0`, `0x0012ac20`). **V:**
    /// set on every retail sound whose minimum distance is 0, and on no
    /// other (65,309 sounds, all 30 maps).
    pub const CLASS_MIN_DISTANCE: u16 = 0x0400;
    /// Flag: the maximum distance is the class's (class `+0x1c`), not the
    /// playback element's `+0x04` (E `0x00125010`, `0x00218d30`). The
    /// same retail rule as [`CLASS_MIN_DISTANCE`]. Without both, zeros
    /// are taken as they stand: the minimum is raised to 0.001 and the
    /// distance gain is `min / distance` times a fade that a maximum of 0
    /// leaves at 1 (E `0x0012ac20`, `0x0012aff0`), so a positioned sound
    /// is at -60 dB one world unit away: inaudible.
    pub const CLASS_MAX_DISTANCE: u16 = 0x0800;
    /// Flag: skip the cone, so the source's facing is never looked up
    /// (E `0x0012adc2`, `0x00125060`). **V:** set on every retail sound;
    /// every retail cone is the whole sphere.
    pub const NO_CONE: u16 = 0x1000;
    /// `u8` class.
    pub const CLASS: usize = 2;
    /// `u8` sample rate: 0 22,050 Hz, 1 44,100 Hz, 2 32,000 Hz.
    pub const SAMPLE_RATE: usize = 3;
    /// `u8` encoding: 0 mono, 1 stereo, 2 codec.
    pub const ENCODING: usize = 4;
    /// `u8` compression: 0 big-endian PCM, 1 Xbox ADPCM, 2 IMA, 3
    /// little-endian PCM, 4 WMA.
    pub const COMPRESSION: usize = 5;
    /// `u16` playback index.
    pub const PLAYBACK: usize = 6;
    /// `u16` first pitch range.
    pub const FIRST_PITCH_RANGE: usize = 8;
    /// `u8` pitch range count.
    pub const PITCH_RANGE_COUNT: usize = 0xa;
    /// `u8` scale index.
    pub const SCALE: usize = 0xb;
    /// Nonnegative signed byte promotion index. `ff` means none.
    pub const PROMOTION: usize = 0xc;
    /// `u8` custom playback index, `ff` none.
    pub const CUSTOM_PLAYBACK: usize = 0xd;
    /// `u16` extra info index, `ffff` none.
    pub const EXTRA_INFO: usize = 0xe;
    /// `u32` maximum play time, milliseconds.
    pub const MAX_PLAY_TIME: usize = 0x10;
    /// Bytes.
    pub const LEN: usize = 0x14;
}

/// One converted permutation.
#[derive(Debug, Clone, PartialEq)]
pub struct PermutationOut {
    /// Name, for the import names.
    pub name: String,
    /// `+2`: skip fraction, encoded as a fraction of 32767 (Design).
    pub skip_fraction: i16,
    /// `+4`: gain in decibels.
    pub gain: i8,
    /// `+6`: how long it plays, in milliseconds (`u16`, saturating). Retail
    /// fills it for dialogue (classes 19 and 45 to 48) with the line's
    /// length in its longest language; the engine reads it as the length
    /// of a playing script impulse in campaign games with the language
    /// timing byte set (E `0x00218c60`, from `0x0020b3c0`). Every
    /// converted permutation gets its own length, so a script's wait on
    /// its sound (`sound_impulse_language_time`) never ends at once.
    pub time_ms: u16,
    /// The authored 30 Hz mouth aperture curve.
    pub mouth: Vec<u8>,
    /// The samples, cut into chunks of at most [`CHUNK_LEN`].
    pub chunks: Vec<Vec<u8>>,
}

impl PermutationOut {
    /// Bytes over all chunks.
    pub fn size(&self) -> usize {
        self.chunks.iter().map(Vec::len).sum()
    }
}

/// One converted pitch range.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeOut {
    /// Name, for the import names.
    pub name: String,
    /// The pitch range parameters element (10 bytes).
    pub parameters: [u8; 10],
    /// Permutations, one per Halo 1 chain.
    pub permutations: Vec<PermutationOut>,
}

/// A Halo 1 sound ready to join a Halo 2 map.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertedSound {
    /// The `snd!` bytes, gestalt indices still unset.
    pub header: [u8; snd::LEN],
    /// The playback parameters element (`0x38`).
    pub playback: [u8; 0x38],
    /// The scale element (`0x14`).
    pub scale: [u8; 0x14],
    /// Pitch ranges.
    pub ranges: Vec<RangeOut>,
    pub promotion: Option<Promotion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub count: u16,
    pub millis: u32,
    /// Another authored sound, or none for a rate-limited sound.
    pub target: Option<String>,
}

/// Decibels from a linear gain; 0 and 1 are 0 dB.
pub fn decibels(linear: f32) -> f32 {
    if linear <= 0.0 || linear == 1.0 {
        0.0
    } else {
        20.0 * linear.log10()
    }
}

/// Cents from a pitch multiplier; 0 and 1 are 0 cents.
pub fn cents(multiplier: f32) -> i16 {
    if multiplier <= 0.0 || multiplier == 1.0 {
        0
    } else {
        (1200.0 * multiplier.log2())
            .round()
            .clamp(-32768.0, 32767.0) as i16
    }
}

fn put_f32(b: &mut [u8], at: usize, v: f32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// The Halo 2 `snd!` flags for `sound`: Halo 1's bits 0 and 1 (fit to
/// ADPCM blocks, split into permutations), and the engine's class
/// distance and cone bits. A Halo 1 distance of 0 means the class's
/// default (S); Halo 2 says that with [`snd::CLASS_MIN_DISTANCE`] and
/// [`snd::CLASS_MAX_DISTANCE`], as every retail sound with no distance
/// of its own does. A sound whose cones are the whole sphere (Halo 1's
/// `2π`, the default) gets [`snd::NO_CONE`].
pub fn flags(sound: &Sound) -> u16 {
    let mut f = (sound.flags & 3) as u16;
    if sound.distance[0] <= 0.0 {
        f |= snd::CLASS_MIN_DISTANCE;
    }
    if sound.distance[1] <= 0.0 {
        f |= snd::CLASS_MAX_DISTANCE;
    }
    let whole = |a: f32| a >= std::f32::consts::TAU - 1e-3;
    if whole(sound.cone_angles[0]) && whole(sound.cone_angles[1]) {
        f |= snd::NO_CONE;
    }
    f
}

/// Reads what a Halo 2 sound needs out of `sound`, whose samples are in
/// `file` (the decompressed Halo 1 map).
pub fn convert_sound(sound: &Sound, file: &[u8]) -> Result<ConvertedSound> {
    let channels = sound.channels()?;
    let compression = match sound.compression {
        Compression::XboxAdpcm => 1u8,
        Compression::None => 3,
        c => return Err(Error::Unsupported(format!("compression {c:?}"))),
    };
    let block = match sound.compression {
        Compression::XboxAdpcm => 36 * channels,
        _ => 2 * channels,
    };
    let rate = sound.hz()?;
    if sound.pitch_ranges.len() > 255 {
        return Err(Error::Unsupported(format!(
            "{} pitch ranges; Halo 2 counts them in a byte",
            sound.pitch_ranges.len()
        )));
    }

    let mut header = [0u8; snd::LEN];
    put_u16(&mut header, snd::FLAGS, flags(sound));
    header[snd::CLASS] = class::to_h2(sound.class)?;
    header[snd::SAMPLE_RATE] = sound.sample_rate as u8;
    header[snd::ENCODING] = sound.encoding as u8;
    header[snd::COMPRESSION] = compression;
    header[snd::PITCH_RANGE_COUNT] = sound.pitch_ranges.len() as u8;
    header[snd::PROMOTION] = 0xff;
    header[snd::CUSTOM_PLAYBACK] = 0xff;
    put_u16(&mut header, snd::EXTRA_INFO, 0xffff);

    let mut playback = [0u8; 0x38];
    put_f32(&mut playback, 0x00, sound.distance[0]);
    put_f32(&mut playback, 0x04, sound.distance[1]);
    put_f32(&mut playback, 0x08, sound.skip_fraction);
    put_f32(&mut playback, 0x0c, sound.max_bend_per_second);
    put_f32(&mut playback, 0x10, decibels(sound.gain_modifier));
    put_u16(
        &mut playback,
        0x18,
        cents(sound.random_pitch_bounds[0]) as u16,
    );
    put_u16(
        &mut playback,
        0x1a,
        cents(sound.random_pitch_bounds[1]) as u16,
    );
    // Halo 1 stores whole cone angles (2π: no cone), Halo 2 half angles.
    put_f32(&mut playback, 0x1c, sound.cone_angles[0] / 2.0);
    put_f32(&mut playback, 0x20, sound.cone_angles[1] / 2.0);
    put_f32(&mut playback, 0x24, decibels(sound.outer_cone_gain));

    let [zs, zg, zp] = sound.zero_modifiers;
    let [os, og, op] = sound.one_modifiers;
    let mut scale = [0u8; 0x14];
    put_f32(&mut scale, 0x00, decibels(zg));
    put_f32(&mut scale, 0x04, decibels(og));
    put_u16(&mut scale, 0x08, cents(zp) as u16);
    put_u16(&mut scale, 0x0a, cents(op) as u16);
    put_f32(&mut scale, 0x0c, if zs == 0.0 { 1.0 } else { zs });
    put_f32(&mut scale, 0x10, if os == 0.0 { 1.0 } else { os });

    if sound.promotion_id().is_some() && sound.promotion_name.is_none() {
        return Err(Error::Invalid("unresolved promotion sound datum".into()));
    }
    if sound.promotion_count != 0
        && (sound.promotion_count > i16::MAX as u16
            || sound
                .promotion_millis
                .checked_mul(u32::from(sound.promotion_count) + 1)
                .is_none_or(|v| v > i32::MAX as u32))
    {
        return Err(Error::Invalid("invalid promotion count or duration".into()));
    }
    let promotion =
        (sound.promotion_count != 0 || sound.promotion_name.is_some()).then(|| Promotion {
            count: sound.promotion_count,
            millis: sound.promotion_millis,
            target: sound.promotion_name.clone(),
        });
    let single = sound.pitch_ranges.len() == 1;
    let mut ranges = Vec::with_capacity(sound.pitch_ranges.len());
    for (r, range) in sound.pitch_ranges.iter().enumerate() {
        let mut parameters = [0u8; 10];
        if !single {
            let lo = cents(range.bend_bounds[0]);
            let hi = cents(range.bend_bounds[1]);
            put_u16(&mut parameters, 0, cents(range.natural_pitch) as u16);
            for (i, v) in [lo, hi, lo, hi].into_iter().enumerate() {
                put_u16(&mut parameters, 2 + 2 * i, v as u16);
            }
        }
        let mut permutations = Vec::new();
        for chain in sound.chains(r)? {
            let first = &range.permutations[chain[0]];
            for &i in &chain {
                let p = &range.permutations[i];
                if p.mouth.size as usize != p.mouth_samples.len() {
                    return Err(Error::Invalid(format!(
                        "{}: mouth data length mismatch",
                        p.name
                    )));
                }
            }
            // Retail Xbox dialogue has one envelope per playable permutation.
            // A split sound would need to rebase segment-local sample times.
            if chain.len() > 1 && chain.iter().any(|&i| range.permutations[i].mouth.size > 0) {
                return Err(Error::Unsupported(format!(
                    "{}: mouth data on a split sound",
                    first.name
                )));
            }
            let bytes = crate::sounds::decode::chain_bytes(sound, file, r, &chain)?;
            if bytes.len() % block != 0 {
                return Err(Error::Invalid(format!(
                    "{}: {} bytes is not whole {block}-byte blocks",
                    first.name,
                    bytes.len()
                )));
            }
            let samples = bytes.len() / block * if compression == 1 { 64 } else { 1 };
            permutations.push(PermutationOut {
                time_ms: u16::try_from(samples as u64 * 1000 / u64::from(rate)).unwrap_or(u16::MAX),
                name: first.name.clone(),
                skip_fraction: (first.skip_fraction.clamp(0.0, 1.0) * 32767.0).round() as i16,
                gain: decibels(first.gain).round().clamp(-128.0, 127.0) as i8,
                mouth: first.mouth_samples.clone(),
                chunks: bytes.chunks(CHUNK_LEN).map(<[u8]>::to_vec).collect(),
            });
        }
        ranges.push(RangeOut {
            name: range.name.clone(),
            parameters,
            permutations,
        });
    }
    let longest = ranges
        .iter()
        .flat_map(|r| &r.permutations)
        .map(|p| p.size() / block * if compression == 1 { 64 } else { 1 })
        .max()
        .unwrap_or(0);
    header[snd::MAX_PLAY_TIME..]
        .copy_from_slice(&((longest as u64 * 1000 / u64::from(rate)) as u32).to_le_bytes());
    Ok(ConvertedSound {
        header,
        playback,
        scale,
        ranges,
        promotion,
    })
}

fn index16(what: &str, n: usize) -> Result<u16> {
    u16::try_from(n)
        .ok()
        .filter(|&v| v != u16::MAX)
        .ok_or_else(|| Error::Unsupported(format!("{what} index {n} doesn't fit 16 bits")))
}

impl ConvertedSound {
    /// Appends this sound's entries to `gestalt`, its chunks to `set` as raw
    /// resources, and returns its `snd!`.
    pub fn add_to(self, gestalt: &mut SoundGestalt, set: &mut TagSet) -> Result<Converted> {
        let snd_group = FourCC(*b"snd!");
        let target = self
            .promotion
            .as_ref()
            .and_then(|p| p.target.as_ref())
            .map(|target| {
                set.converted(snd_group, target)
                    .ok_or_else(|| Error::Invalid(format!("uninstalled promotion sound {target}")))
            })
            .transpose()?;
        let promotion = self.promotion.clone();
        let mut sound = self.add_samples(gestalt, set)?;
        if let Some(promotion) = promotion {
            write_promotion(&mut sound.root, &promotion, target);
        }
        Ok(sound)
    }

    fn add_samples(self, gestalt: &mut SoundGestalt, set: &mut TagSet) -> Result<Converted> {
        let mut header = self.header;
        let playback = gestalt.push(blocks::PLAYBACKS, &self.playback, true);
        put_u16(&mut header, snd::PLAYBACK, index16("playback", playback)?);
        let scale = gestalt.push(blocks::SCALES, &self.scale, true);
        header[snd::SCALE] = u8::try_from(scale)
            .ok()
            .filter(|&s| s != 0xff)
            .ok_or_else(|| Error::Unsupported(format!("scale index {scale}")))?;
        let first_range = gestalt.count(blocks::PITCH_RANGES);
        put_u16(
            &mut header,
            snd::FIRST_PITCH_RANGE,
            index16("pitch range", first_range)?,
        );

        let name = |gestalt: &mut SoundGestalt, set: &mut TagSet, s: &str| -> Result<u16> {
            let id = set.intern(if s.is_empty() { "default" } else { s })?;
            let n = gestalt.push(blocks::IMPORT_NAMES, &id.to_le_bytes(), true);
            index16("import name", n)
        };
        let curves: Vec<_> = self
            .ranges
            .iter()
            .flat_map(|r| r.permutations.iter())
            .map(|p| p.mouth.as_slice())
            .collect();
        let has_mouth = curves.iter().any(|curve| !curve.is_empty());
        if has_mouth {
            let index = crate::sounds::mouth::add(&curves, gestalt, set)?;
            put_u16(&mut header, snd::EXTRA_INFO, index);
        }
        let mut descriptor = 0;
        for range in self.ranges {
            let range_name = name(gestalt, set, &range.name)?;
            let params = gestalt.push(blocks::PITCH_RANGE_PARAMETERS, &range.parameters, true);
            let first_perm = gestalt.count(blocks::PERMUTATIONS);
            let n = range.permutations.len();
            // One permutation: +4 is 0 and +6 none; more: +4 none and +6 the
            // bit offset of the range's runtime flags, a byte per eight.
            let (encoded, flags) = if n == 1 {
                (0u16, 0xffffu16)
            } else {
                let at = gestalt.flat[blocks::RUNTIME_PERMUTATION_FLAGS].len();
                gestalt.flat[blocks::RUNTIME_PERMUTATION_FLAGS]
                    .extend(std::iter::repeat_n(0, n.div_ceil(8)));
                (0xffff, index16("runtime permutation flag", at * 8)?)
            };
            for (i, p) in range.permutations.into_iter().enumerate() {
                let perm_name = name(gestalt, set, &p.name)?;
                let first_chunk = index16("chunk", gestalt.chunks.len())?;
                let size = p.size() as u32;
                let count = index16("chunk count", p.chunks.len())?;
                for c in p.chunks {
                    let len = c.len() as u32;
                    let raw = set.add_raw(c, CHUNK_ALIGN);
                    gestalt.chunks.push(Chunk {
                        pointer: ChunkPointer::Local(raw),
                        size: len,
                        runtime: u32::MAX,
                    });
                }
                let mut e = [0u8; 16];
                put_u16(&mut e, 0, perm_name);
                put_u16(&mut e, 2, p.skip_fraction as u16);
                e[4] = p.gain as u8;
                e[5] = if has_mouth {
                    let index = descriptor;
                    descriptor += 1;
                    index as u8
                } else {
                    u8::try_from(i).unwrap_or(u8::MAX)
                };
                put_u16(&mut e, 6, p.time_ms);
                e[8..12].copy_from_slice(&size.to_le_bytes());
                put_u16(&mut e, 12, first_chunk);
                put_u16(&mut e, 14, count);
                gestalt.push(blocks::PERMUTATIONS, &e, false);
            }
            let mut e = [0u8; 12];
            put_u16(&mut e, 0, range_name);
            put_u16(&mut e, 2, index16("pitch range parameters", params)?);
            put_u16(&mut e, 4, encoded);
            put_u16(&mut e, 6, flags);
            put_u16(&mut e, 8, index16("permutation", first_perm)?);
            put_u16(&mut e, 10, index16("permutation count", n)?);
            gestalt.push(blocks::PITCH_RANGES, &e, false);
        }
        Ok(Converted {
            group: FourCC(*b"snd!"),
            root: Block::new(header.to_vec()),
            raw: Vec::new(),
        })
    }
}

/// Adds `sounds` (name, sound) to `set`: decodes its `ugh!` (or starts an
/// empty one), appends every sound's entries, installs each `snd!`, and
/// writes the gestalt back in place. Returns the new sounds' datums.
pub fn install_sounds(
    set: &mut TagSet,
    sounds: Vec<(String, ConvertedSound)>,
) -> Result<Vec<TagId>> {
    if sounds.is_empty() {
        return Ok(Vec::new());
    }
    let snd_group = FourCC(*b"snd!");
    let names: HashSet<_> = sounds.iter().map(|(name, _)| name.as_str()).collect();
    for (name, sound) in &sounds {
        if let Some(target) = sound.promotion.as_ref().and_then(|p| p.target.as_deref())
            && !names.contains(target)
            && set.converted(snd_group, target).is_none()
        {
            return Err(Error::Invalid(format!(
                "{name}: uninstalled promotion sound {target}"
            )));
        }
    }

    let mut gestalt = SoundGestalt::from_set(set)?;
    let mut ids = Vec::with_capacity(sounds.len());
    let mut promotions = Vec::new();
    for (name, mut sound) in sounds {
        let id = match set.converted(snd_group, &name) {
            Some(id) => id,
            None => {
                let promotion = sound.promotion.take();
                let converted = sound
                    .add_samples(&mut gestalt, set)
                    .map_err(|e| Error::Invalid(format!("{name}: {e}")))?;
                let id = install_named(converted, set, &name)?;
                if let Some(promotion) = promotion {
                    promotions.push((id, promotion));
                }
                id
            }
        };
        ids.push(id);
    }
    // The retail rock and metal chips promote in cycles. Assign every
    // datum first so all edges are resolved, including backward edges.
    for (id, promotion) in promotions {
        let target = promotion.target.as_ref().map(|name| {
            set.converted(snd_group, name)
                .expect("validated promotion target installed")
        });
        let Some(TagPayload::Built(root)) = set.tag_mut(id).map(|tag| &mut tag.payload) else {
            return Err(Error::Invalid(format!(
                "{id:?}: promotion sound is not built"
            )));
        };
        write_promotion(root, &promotion, target);
    }
    gestalt.install(set, crate::sounds::gestalt::DEFAULT_NAME)?;
    Ok(ids)
}

fn write_promotion(root: &mut Block, promotion: &Promotion, target: Option<TagId>) {
    use h1_promotion::*;
    root.bytes.resize(SIZE as usize, 0);
    root.put_u32(SIGNATURE as usize, MAGIC);
    root.put_u32(COUNT as usize, u32::from(promotion.count));
    root.put_u32(MILLIS as usize, promotion.millis);
    root.put_u32(TARGET as usize, TagId::NULL.0);
    if let Some(target) = target {
        root.tag_id(TARGET, target);
    }
}

fn source_tag<'a>(map: &'a h1map::H1Map, name: &str) -> Result<&'a h1map::Tag> {
    let mut matching = map
        .tags()
        .iter()
        .filter(|tag| tag.group == crate::sounds::h1::SND && tag.name.eq_ignore_ascii_case(name));
    let invalid = |reason| Error::Invalid(format!("{}: {reason} snd! {name}", map.header().name));
    let tag = matching.next().ok_or_else(|| invalid("missing"))?;
    if matching.next().is_some() {
        return Err(invalid("ambiguous"));
    }
    Ok(tag)
}

/// Read the distinct source sounds reachable from the named roots, including
/// promotion targets. An already installed source sound owns its entire chain.
pub fn read_source_sounds(
    h1: &h1map::H1Map,
    names: impl IntoIterator<Item = String>,
    mut installed: impl FnMut(&str) -> bool,
) -> Result<Vec<(String, ConvertedSound)>> {
    let mut pending: VecDeque<_> = names.into_iter().collect();
    let mut seen = HashSet::new();
    let mut sounds = Vec::new();
    while let Some(name) = pending.pop_front() {
        let tag = source_tag(h1, &name)?;
        if !seen.insert(tag.id) || installed(&tag.name) {
            continue;
        }
        let sound = Sound::read(h1, tag)
            .and_then(|s| convert_sound(&s, h1.bytes()))
            .map_err(|e| Error::Invalid(format!("{}: {name}: {e}", h1.header().name)))?;
        if let Some(target) = sound.promotion.as_ref().and_then(|p| p.target.as_ref()) {
            pending.push_back(target.clone());
        }
        sounds.push((tag.name.clone(), sound));
    }
    Ok(sounds)
}

/// Install the requested source sounds and their promotion targets together.
pub fn install_source_sounds(
    h1: &h1map::H1Map,
    set: &mut TagSet,
    names: impl IntoIterator<Item = String>,
) -> Result<HashMap<String, TagId>> {
    let snd_group = FourCC(*b"snd!");
    let names: Vec<_> = names.into_iter().collect();
    let sounds = read_source_sounds(h1, names.iter().cloned(), |name| {
        set.converted(snd_group, name).is_some()
    })?;
    install_sounds(set, sounds)?;
    names
        .into_iter()
        .map(|name| {
            let source = source_tag(h1, &name)?;
            let id = set.converted(snd_group, &source.name).ok_or_else(|| {
                Error::Invalid(format!(
                    "{}: missing installed snd! {name}",
                    h1.header().name
                ))
            })?;
            Ok((name, id))
        })
        .collect()
}

pub(crate) fn install_named(tag: Converted, set: &mut TagSet, name: &str) -> Result<TagId> {
    match set.find(tag.group, name) {
        Some(id) => tag.install_at(set, id, name),
        None => tag.install(set, name),
    }
    .map_err(|e| Error::Invalid(format!("{name}: {e}")))
}

/// Lets a set decomposed from a retail map ([`TagSet::from_map`]) gain
/// tags. The builder never writes over the uninterpreted bytes after a
/// pinned map's tag entries, so a pinned index can't grow; this zeroes
/// them (as [`map::build::PinnedImage::clear_index_tail`] does for a map
/// "built from scratch") and lets the entries use the whole index region.
/// What the game does with those bytes is not known: see
/// `docs/halo1/sounds.md`, "Open questions". Returns the nonzero
/// bytes cleared.
pub fn make_index_room(set: &mut TagSet) -> usize {
    let Some(p) = set.pinned.as_mut() else {
        return 0;
    };
    let cleared = p.clear_index_tail();
    p.index_entries_end = p.index.len() as u32;
    cleared
}
