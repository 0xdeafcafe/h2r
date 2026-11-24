//! The `sound` and `data` builders: files a game feature reads, which add
//! no tag to a map.
//!
//! A `sound` item is a 16-bit PCM WAV clip. It is checked here (a clip the
//! feature could not play is refused by name at `h2 package check`), and the
//! feature that plays it reads the file from the package; the Pogo Stick's
//! clips are this (`crates/extensions/game/src/pogo_sound.rs`). Writing a
//! clip as a `snd!` tag in the map (resample, ADPCM, the gestalt merge) is
//! the sound editor's import (SN2 of docs/editor/studio-redesign.md), so an
//! item that names a `tag` is refused until then.
//!
//! A `data` item is any file: a feature reads it (the Pogo's pose set) or
//! another builder reads it beside its source (a model's atlas).
use crate::build::{Cx, Failure};
use crate::builders::Job;

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
pub fn read_wav(bytes: &[u8]) -> Result<Wav, String> {
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
                data = Some(len.min(bytes.len().saturating_sub(body)));
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
    let data = data.ok_or("no data chunk")?;
    if data == 0 {
        return Err("the clip is empty".into());
    }
    Ok(Wav {
        channels,
        rate,
        frames: (data / (2 * usize::from(channels))) as u32,
    })
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    if job.item.tag.is_some() {
        return Err(
            "a sound item with a `tag` would write a snd! into the map, which comes with the sound editor's import; leave `tag` out for a clip a game feature plays"
                .into(),
        );
    }
    read_wav(job.source)?;
    Ok(())
}

/// Adds nothing: the feature reads the clip from the package.
pub fn apply(_: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    check(job)
}

pub fn data_check(_: &Job<'_>) -> Result<(), Failure> {
    Ok(())
}

/// Adds nothing: the file is for a feature, or another item.
pub fn data_apply(_: &mut Cx<'_>, _: &Job<'_>) -> Result<(), Failure> {
    Ok(())
}
