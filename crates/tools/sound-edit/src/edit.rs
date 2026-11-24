//! The edits, applied one after another to decoded samples.
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Wave;

/// The peak ceiling `normalise` keeps (dBFS) when the edit names none: a
/// sample peak at -1 dBFS leaves room for the codec.
pub const DEFAULT_CEILING_DBFS: f64 = -1.0;

/// Gains past this many dB either way are refused.
pub const MAX_GAIN_DB: f64 = 60.0;

fn ceiling() -> f64 {
    DEFAULT_CEILING_DBFS
}

/// One edit. Times are milliseconds from the start of what the edits
/// before it left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    /// Keep `start_ms` up to `end_ms` (the end of the clip when absent).
    Trim {
        start_ms: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_ms: Option<f64>,
    },
    /// A linear ramp from silence over the first `ms`.
    FadeIn { ms: f64 },
    /// A linear ramp to silence over the last `ms`.
    FadeOut { ms: f64 },
    /// `db` more (or less) level.
    Gain { db: f64 },
    /// One gain so the integrated loudness is `lufs`, or as near as keeps
    /// the sample peak under `ceiling_dbfs`.
    Normalise {
        lufs: f64,
        #[serde(default = "ceiling")]
        ceiling_dbfs: f64,
    },
    /// Zero the samples from `start_ms` up to `end_ms`.
    Silence { start_ms: f64, end_ms: f64 },
}

impl Edit {
    pub fn name(&self) -> &'static str {
        match self {
            Edit::Trim { .. } => "trim",
            Edit::FadeIn { .. } => "fade_in",
            Edit::FadeOut { .. } => "fade_out",
            Edit::Gain { .. } => "gain",
            Edit::Normalise { .. } => "normalise",
            Edit::Silence { .. } => "silence",
        }
    }
}

/// An edit that did not apply: its place in the list and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderError {
    pub index: usize,
    pub message: String,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "edit {}: {}", self.index, self.message)
    }
}

impl std::error::Error for RenderError {}

fn frame_at(ms: f64, rate: u32) -> usize {
    (ms * f64::from(rate) / 1000.0).round() as usize
}

fn time(name: &str, ms: f64) -> Result<f64, String> {
    if ms.is_finite() && ms >= 0.0 {
        Ok(ms)
    } else {
        Err(format!("{name}: {ms} is not a time of zero or more"))
    }
}

fn scaled(pcm: &mut [i16], at: usize, k: f64) {
    pcm[at] = (f64::from(pcm[at]) * k).round().clamp(-32768.0, 32767.0) as i16;
}

fn gained(pcm: &[i16], db: f64) -> Vec<i16> {
    let k = 10f64.powf(db / 20.0);
    pcm.iter()
        .map(|&s| (f64::from(s) * k).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// `wave` with one edit applied.
pub fn apply(wave: &Wave, edit: &Edit) -> Result<Wave, String> {
    let ch = usize::from(wave.channels.max(1));
    let frames = wave.frames();
    let mut out = wave.clone();
    match *edit {
        Edit::Trim { start_ms, end_ms } => {
            let start = frame_at(time("start_ms", start_ms)?, wave.rate);
            let end = match end_ms {
                Some(e) => frame_at(time("end_ms", e)?, wave.rate).min(frames),
                None => frames,
            };
            if start >= end {
                return Err(format!("trim leaves nothing of {frames} frames"));
            }
            out.pcm = wave.pcm[start * ch..end * ch].to_vec();
        }
        Edit::FadeIn { ms } => {
            let n = frame_at(time("ms", ms)?, wave.rate).min(frames);
            for f in 0..n {
                for c in 0..ch {
                    scaled(&mut out.pcm, f * ch + c, f as f64 / n as f64);
                }
            }
        }
        Edit::FadeOut { ms } => {
            let n = frame_at(time("ms", ms)?, wave.rate).min(frames);
            for f in frames - n..frames {
                let k = (frames - 1 - f) as f64 / n as f64;
                for c in 0..ch {
                    scaled(&mut out.pcm, f * ch + c, k);
                }
            }
        }
        Edit::Gain { db } => {
            if !db.is_finite() || db.abs() > MAX_GAIN_DB {
                return Err(format!("db: {db} is not within {MAX_GAIN_DB} dB"));
            }
            out.pcm = gained(&wave.pcm, db);
        }
        Edit::Normalise { lufs, ceiling_dbfs } => {
            if !lufs.is_finite() || !ceiling_dbfs.is_finite() || ceiling_dbfs > 0.0 {
                return Err("lufs and ceiling_dbfs: finite, the ceiling at 0 dBFS or under".into());
            }
            let l = wave.levels();
            if l.lufs <= audio::meter::SILENCE_DB + 1.0
                || l.peak_dbfs <= audio::meter::SILENCE_DB + 1.0
            {
                return Err("the clip is silent".into());
            }
            let mut db = lufs - l.lufs;
            if l.peak_dbfs + db > ceiling_dbfs {
                db = ceiling_dbfs - l.peak_dbfs;
            }
            out.pcm = gained(&wave.pcm, db);
        }
        Edit::Silence { start_ms, end_ms } => {
            let start = frame_at(time("start_ms", start_ms)?, wave.rate);
            let end = frame_at(time("end_ms", end_ms)?, wave.rate).min(frames);
            if start >= end {
                return Err(format!("silence: {start_ms} to {end_ms} ms holds no frame"));
            }
            out.pcm[start * ch..end * ch].fill(0);
        }
    }
    Ok(out)
}

/// `source` through every edit in order. The source is not changed.
pub fn render(source: &Wave, edits: &[Edit]) -> Result<Wave, RenderError> {
    let mut wave = source.clone();
    for (index, e) in edits.iter().enumerate() {
        wave = apply(&wave, e).map_err(|message| RenderError { index, message })?;
    }
    Ok(wave)
}

/// Put `edit` at `at` (the end by default); its place.
pub fn insert(list: &mut Vec<Edit>, edit: Edit, at: Option<usize>) -> Result<usize, String> {
    let at = at.unwrap_or(list.len());
    if at > list.len() {
        return Err(format!("at {at}: the list has {}", list.len()));
    }
    list.insert(at, edit);
    Ok(at)
}

/// Take the edit at `index` out.
pub fn remove(list: &mut Vec<Edit>, index: usize) -> Result<Edit, String> {
    if index >= list.len() {
        return Err(format!("index {index}: the list has {}", list.len()));
    }
    Ok(list.remove(index))
}

/// Move the edit at `from` so it is at `to`.
pub fn reorder<T>(list: &mut [T], from: usize, to: usize) -> Result<(), String> {
    let n = list.len();
    if from >= n || to >= n {
        return Err(format!("from {from}, to {to}: the list has {n}"));
    }
    if from < to {
        list[from..=to].rotate_left(1);
    } else {
        list[to..=from].rotate_right(1);
    }
    Ok(())
}

/// Drop the last edit; it, or `None` for an empty list.
pub fn undo(list: &mut Vec<Edit>) -> Option<Edit> {
    list.pop()
}
