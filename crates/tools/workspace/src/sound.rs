//! The `sound` and `data` builders.
//!
//! A `sound` item is a 16-bit PCM WAV clip. It is checked here (a clip the
//! feature could not play is refused by name at `h2 package check`). With no
//! `tag` it adds nothing to a map: the feature that plays it reads the file
//! from the package, as the Pogo Stick's clips do
//! (`crates/extensions/game/src/pogo_sound.rs`). With a `tag` it is written
//! as a `snd!` of that path: resampled, encoded as Xbox ADPCM and merged into
//! the map's gestalt ([`crate::import`]), in its sound class (the `class` of its sidecar, `<source>.sound.yaml`, a
//! `data` item beside it; the default of [`crate::import::DEFAULT_CLASS`] with
//! none). The sidecar's `edits` (`sound-edit`) are applied to the decoded
//! WAV first; the WAV file is never rewritten.
//!
//! A `data` item is any file, which adds no tag: a feature reads it (the
//! Pogo's pose set) or another builder reads it beside its source (a
//! model's atlas).
use crate::build::{Cx, Failure};
use crate::builders::Job;
use crate::import::Clip;

/// What a WAV file says about its clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wav {
    pub channels: u16,
    pub rate: u32,
    pub frames: u32,
}

fn word(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn half(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// A 16-bit PCM WAV's format and length, or why it is not one.
#[cfg(test)]
pub fn read_wav(bytes: &[u8]) -> Result<Wav, String> {
    parse(bytes).map(|(w, _)| w)
}

/// A 16-bit PCM WAV's samples, interleaved, with its format.
pub fn read_clip(bytes: &[u8]) -> Result<Clip, String> {
    let (wav, data) = parse(bytes)?;
    Ok(Clip {
        channels: wav.channels,
        rate: wav.rate,
        pcm: data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect(),
    })
}

/// The format, and the bytes of the data chunk's whole frames.
fn parse(bytes: &[u8]) -> Result<(Wav, &[u8]), String> {
    if bytes.starts_with(b"version https://git-lfs") {
        return Err("a Git LFS pointer: run `git lfs pull`".into());
    }
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a RIFF WAVE file".into());
    }
    let mut fmt = None;
    let mut data = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = word(bytes, at + 4).ok_or("a chunk with no length")? as usize;
        let body = at + 8;
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => {
                data = Some((body, len.min(bytes.len().saturating_sub(body))));
            }
            _ => {}
        }
        at = body.saturating_add(len).saturating_add(len & 1);
    }
    let f = fmt.ok_or("no fmt chunk")?;
    let (tag, channels, rate, bits) = (
        half(bytes, f).ok_or("a short fmt chunk")?,
        half(bytes, f + 2).ok_or("a short fmt chunk")?,
        word(bytes, f + 4).ok_or("a short fmt chunk")?,
        half(bytes, f + 14).ok_or("a short fmt chunk")?,
    );
    if tag != 1 || bits != 16 {
        return Err(format!("format {tag}, {bits} bits: a clip is 16-bit PCM"));
    }
    if !(1..=2).contains(&channels) || rate == 0 {
        return Err(format!("{channels} channels at {rate} Hz"));
    }
    let (start, len) = data.ok_or("no data chunk")?;
    let width = 2 * usize::from(channels);
    let frames = len / width;
    if frames == 0 {
        return Err("the clip is empty".into());
    }
    Ok((
        Wav {
            channels,
            rate,
            frames: frames as u32,
        },
        &bytes[start..start + frames * width],
    ))
}

/// The path an item writes a `snd!` at, if it names one.
fn path_of<'a>(job: &Job<'a>) -> Option<&'a str> {
    job.item.tag.as_deref().filter(|t| !t.is_empty())
}

/// The item's sidecar (`<source>.sound.yaml`, a `data` item of the
/// package): its class and its edit list; empty with none.
fn doc_of(job: &Job<'_>) -> Result<sound_edit::SoundDoc, String> {
    let Some(source) = job.item.source.as_deref() else {
        return Ok(Default::default());
    };
    let name = crate::import::sidecar_path(source);
    let Some(bytes) = job.files.get(&name) else {
        return Ok(Default::default());
    };
    let text = std::str::from_utf8(bytes).map_err(|e| format!("{name}: {e}"))?;
    sound_edit::SoundDoc::from_yaml(text).map_err(|e| format!("{name}: {e}"))
}

/// The sound class the item's sidecar names, else the default.
fn class_of(doc: &sound_edit::SoundDoc) -> Result<u8, String> {
    match &doc.class {
        None => Ok(crate::import::DEFAULT_CLASS),
        Some(c) => crate::import::class_of(c).map_err(|e| format!("class: {e}")),
    }
}

/// The item's WAV through the sidecar's edit list. The file is not
/// changed: the edits are applied to the decoded samples.
fn edited(clip: Clip, doc: &sound_edit::SoundDoc) -> Result<Clip, String> {
    if doc.edits.is_empty() {
        return Ok(clip);
    }
    let wave = sound_edit::Wave {
        channels: clip.channels,
        rate: clip.rate,
        pcm: clip.pcm,
    };
    let wave = sound_edit::render(&wave, &doc.edits).map_err(|e| e.to_string())?;
    Ok(Clip {
        channels: wave.channels,
        rate: wave.rate,
        pcm: wave.pcm,
    })
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    let clip = read_clip(job.source)?;
    if path_of(job).is_some() {
        let doc = doc_of(job)?;
        class_of(&doc)?;
        crate::import::encode(&edited(clip, &doc)?)?;
    } else {
        // A clip a feature plays: its edits still have to apply.
        edited(clip, &doc_of(job)?)?;
    }
    Ok(())
}

/// An item with no `tag` adds nothing: the feature reads the clip from the
/// package. With one, the clip is edited, then written as a `snd!` and its
/// gestalt rows.
pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let clip = read_clip(job.source)?;
    match path_of(job) {
        None => Ok(()),
        Some(path) => {
            let doc = doc_of(job)?;
            let class = class_of(&doc)?;
            let clip = edited(clip, &doc)?;
            crate::import::install(cx, path, &clip, class).map_err(Failure::from)
        }
    }
}

pub fn data_check(_: &Job<'_>) -> Result<(), Failure> {
    Ok(())
}

/// Adds nothing: the file is for a feature, or another item.
pub fn data_apply(_: &mut Cx<'_>, _: &Job<'_>) -> Result<(), Failure> {
    Ok(())
}
