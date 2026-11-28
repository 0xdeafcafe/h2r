//! Importing a WAV as a `snd!`: what the `sound` builder writes for an item
//! with a `tag` (docs/editor/studio-redesign.md, "The build").
//!
//! A clip of any rate is resampled to the nearest rate the `snd!`'s format
//! field names that is not below it ([`target_rate`]: 22,050, 32,000 or
//! 44,100 Hz), kept mono or stereo, and encoded as Xbox ADPCM
//! (`audio::adpcm::encode`) in whole blocks. The bytes are cut into
//! [`CHUNK_LEN`] chunks that join the map as raw resources, and the sound's
//! rows join the map's one `ugh!` gestalt: a playback, a scale, a pitch
//! range, a permutation and its chunks. The gestalt is read from the set
//! (or started empty), extended and written back in the same slot, so the
//! rows of every sound already in it, and the raw their chunks point at, are
//! kept as they are.
//!
//! The row-appending is `h1_assets::sounds`: the Halo 1 converter's, which
//! is the one path that writes a `snd!`'s indices. [`merge_sound`], which
//! takes a `snd!` and its rows from another map's gestalt, is re-exported
//! here for the import of a retail sound; the gestalt model
//! (`h1_assets::sounds::gestalt`) stays in that crate, which this one
//! depends on through `content-kit` already.
use audio::resample::resample_sinc;
use h1_assets::sounds::convert::{CHUNK_LEN, PermutationOut, RangeOut, snd};
use h1_assets::sounds::gestalt::{DEFAULT_NAME, SoundGestalt, UGH};
use h1_assets::sounds::{ConvertedSound, class, install_sounds};
use map::FourCC;
pub use h1_assets::sounds::{MergeMap, merge_sound};
use tags::repo::TagKey;

use crate::build::Cx;

const SND: FourCC = FourCC(*b"snd!");

/// The class of a `sound` item's `snd!`: it takes the class's distances and
/// plays at full volume anywhere, which suits a sound that is heard rather
/// than placed.
pub const DEFAULT_CLASS: u8 = 51;
/// Its name.
pub const DEFAULT_CLASS_NAME: &str = "ui";

/// Samples of a clip: 16-bit, interleaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// 1 or 2.
    pub channels: u16,
    /// Frames per second.
    pub rate: u32,
    pub pcm: Vec<i16>,
}

impl Clip {
    pub fn frames(&self) -> usize {
        self.pcm.len() / usize::from(self.channels.max(1))
    }
}

/// A clip as the game reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    /// `snd!` +3: 0 22,050 Hz, 1 44,100 Hz, 2 32,000 Hz.
    pub rate_code: u8,
    pub rate: u32,
    pub channels: u16,
    /// Frames of the clip at `rate`, before the last block was padded.
    pub frames: usize,
    /// Xbox ADPCM, whole blocks.
    pub adpcm: Vec<u8>,
}

/// The sidecar of a `sound` item whose WAV is `source`: `<stem>.sound.yaml`
/// beside it. It holds `class:` (a name or number) and is a `data` item of
/// the package.
pub fn sidecar_path(source: &str) -> String {
    let stem = match source.rsplit_once('.') {
        Some((stem, ext)) if !ext.contains('/') => stem,
        _ => source,
    };
    format!("{stem}.sound.yaml")
}

/// A 16-bit PCM WAV file as a clip.
pub fn read_wav(bytes: &[u8]) -> Result<Clip, String> {
    crate::sound::read_clip(bytes)
}

/// The class named `text`, or a number, as `snd!` +2.
pub fn class_of(text: &str) -> Result<u8, String> {
    let t = text.trim();
    if let Ok(n) = t.parse::<u8>() {
        return match class::H2_NAMES.get(usize::from(n)).copied().flatten() {
            Some(_) => Ok(n),
            None => Err(format!("{n} is not a sound class")),
        };
    }
    class::H2_NAMES
        .iter()
        .position(|n| n.is_some_and(|n| n.eq_ignore_ascii_case(t)))
        .map(|i| i as u8)
        .ok_or_else(|| format!("{t:?} is not a sound class: one of {}", class_names()))
}

/// Every class name, for an error.
pub fn class_names() -> String {
    class::H2_NAMES
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

/// The rate a clip of `rate` Hz is converted to, with its `snd!` code: the
/// nearest of the three the format holds that is not below it, or the
/// highest.
pub fn target_rate(rate: u32) -> (u8, u32) {
    if rate <= 22_050 {
        (0, 22_050)
    } else if rate <= 32_000 {
        (2, 32_000)
    } else {
        (1, 44_100)
    }
}

/// Resample and encode `clip`.
pub fn encode(clip: &Clip) -> Result<Encoded, String> {
    if !(1..=2).contains(&clip.channels) {
        return Err(format!("{} channels: a sound is mono or stereo", clip.channels));
    }
    if clip.rate == 0 || clip.frames() == 0 {
        return Err("the clip is empty".into());
    }
    let (rate_code, rate) = target_rate(clip.rate);
    let channels = usize::from(clip.channels);
    let pcm: Vec<i16> = if clip.rate == rate {
        clip.pcm.clone()
    } else {
        let src: Vec<f32> = clip.pcm.iter().map(|&s| f32::from(s) / 32768.0).collect();
        resample_sinc(&src, channels, clip.rate, rate)
            .into_iter()
            .map(|v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
            .collect()
    };
    let frames = pcm.len() / channels;
    if frames == 0 {
        return Err("the clip is shorter than one frame at the sound's rate".into());
    }
    let adpcm = audio::adpcm::encode(&pcm, channels).map_err(|e| e.to_string())?;
    Ok(Encoded {
        rate_code,
        rate,
        channels: clip.channels,
        frames,
        adpcm,
    })
}

/// The sound as the converter's model of one: a `snd!` header with its
/// gestalt indices unset, one pitch range of one permutation, called `name`.
pub fn converted(name: &str, class: u8, e: &Encoded) -> ConvertedSound {
    let millis = e.frames as u64 * 1000 / u64::from(e.rate);
    let mut header = [0u8; snd::LEN];
    let flags = snd::CLASS_MIN_DISTANCE | snd::CLASS_MAX_DISTANCE | snd::NO_CONE;
    header[snd::FLAGS..snd::FLAGS + 2].copy_from_slice(&flags.to_le_bytes());
    header[snd::CLASS] = class;
    header[snd::SAMPLE_RATE] = e.rate_code;
    header[snd::ENCODING] = (e.channels - 1) as u8;
    header[snd::COMPRESSION] = 1;
    header[snd::PITCH_RANGE_COUNT] = 1;
    header[snd::PROMOTION] = 0xff;
    header[snd::CUSTOM_PLAYBACK] = 0xff;
    header[snd::EXTRA_INFO..snd::EXTRA_INFO + 2].copy_from_slice(&0xffffu16.to_le_bytes());
    header[snd::MAX_PLAY_TIME..].copy_from_slice(&(millis as u32).to_le_bytes());
    // Distances come from the class (the flags above); no gain, no bend.
    let playback = [0u8; 0x38];
    let mut scale = [0u8; 0x14];
    scale[0x0c..0x10].copy_from_slice(&1.0f32.to_le_bytes());
    scale[0x10..0x14].copy_from_slice(&1.0f32.to_le_bytes());
    ConvertedSound {
        header,
        playback,
        scale,
        ranges: vec![RangeOut {
            name: String::new(),
            parameters: [0; 10],
            permutations: vec![PermutationOut {
                name: name.to_owned(),
                skip_fraction: 0,
                gain: 0,
                time_ms: u16::try_from(millis).unwrap_or(u16::MAX),
                mouth: Vec::new(),
                chunks: e.adpcm.chunks(CHUNK_LEN).map(<[u8]>::to_vec).collect(),
            }],
        }],
        promotion: None,
    }
}

/// Write `clip` into the build as the `snd!` at `path`, and its rows into
/// the map's gestalt (made if the map has none).
pub fn install(cx: &mut Cx<'_>, path: &str, clip: &Clip, class: u8) -> Result<(), String> {
    let key = Cx::key(SND, path);
    cx.check_free(&key)?;
    let encoded = encode(clip)?;
    let leaf = key.path.rsplit('\\').next().unwrap_or(&key.path);
    let sound = converted(leaf, class, &encoded);
    // The gestalt this sound joins, as the build first found it.
    let before = SoundGestalt::slot(cx.set).and_then(|slot| {
        let slot = &cx.set.slots[slot];
        Some((slot.tag.as_ref()?.id, slot.name.clone()))
    });
    if let Some((id, _)) = &before {
        cx.touch(*id)?;
    }
    let ids = install_sounds(cx.set, vec![(key.path.clone(), sound)])
        .map_err(|e| format!("writing sound {key}: {e}"))?;
    let id = *ids.first().ok_or("the sound was not installed")?;
    cx.note_added(&key, id);
    match before {
        Some((_, Some(name))) => cx.note_changed(&TagKey::new(UGH, name)),
        Some((_, None)) => {}
        None => {
            if let Some(ugh) = SoundGestalt::slot(cx.set)
                .and_then(|s| cx.set.slots[s].tag.as_ref().map(|t| t.id))
            {
                cx.note_added(&TagKey::new(UGH, DEFAULT_NAME), ugh);
            }
        }
    }
    Ok(())
}
