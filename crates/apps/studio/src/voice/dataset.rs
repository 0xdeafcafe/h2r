//! A game character's voice as training data, from the user's own disc:
//! their dialogue sounds found by tag path, decoded, cut to clean speech
//! and paired with their subtitle (or marked for a local Whisper), written
//! as 24 kHz mono WAVs and a JSONL manifest under
//! `<data root>/voices/<character>`. docs/editor/voice-cloning.md,
//! "The dataset".
//!
//! Pure over a [`LineSource`]: Studio's is `app::voices::AppLines` (the
//! source's maps through the Tags tool's audition decoder); the tests' is
//! invented. Nothing here reads a map or the network.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use content::Sha256;
use serde_json::{Value, json};

use super::{VoiceError, audio, write_atomic};
use crate::api::sound_measure::meter;

/// The rate the dataset's clips are written at: what F5-TTS and the other
/// clone models take their reference and training audio at.
pub const RATE: u32 = 24_000;

/// The manifest's file name in a character's directory.
pub const MANIFEST: &str = "manifest.jsonl";
/// The last build's summary.
pub const SUMMARY: &str = "dataset.json";
/// The clips' directory in a character's directory.
pub const CLIPS: &str = "dataset";

/// What is left of the silence at a clip's start and end (ms).
const HEAD_PAD_MS: u64 = 40;
const TAIL_PAD_MS: u64 = 80;

/// A game character the clone knows how to find. The rules are matched
/// against `snd!` tag paths under `sound\dialog\` (lowercase, backslashes).
#[derive(Clone, Copy, Debug)]
pub struct Character {
    pub id: &'static str,
    pub name: &'static str,
    /// Paths the character's tags start with.
    pub prefixes: &'static [&'static str],
    /// Directory names under `sound\dialog\` any of which marks the
    /// character's tags.
    pub folders: &'static [&'static str],
    /// Speaker codes: a tag whose last path part ends in `_<code>` (and
    /// digits) is the character's.
    pub codes: &'static [&'static str],
}

/// The characters, first match wins. The folders and codes are the
/// naming Halo 2's dialogue tags are believed to use; they are not yet
/// checked against a disc. `voice.characters` lists the speaker codes and
/// folders a disc has, so this table can be put right from the first live
/// run (docs/editor/voice-cloning.md, "Naming a character").
pub const CHARACTERS: &[Character] = &[
    Character {
        id: "announcer",
        name: "Multiplayer announcer",
        prefixes: &[r"sound\dialog\multiplayer\"],
        folders: &["announcer"],
        codes: &[],
    },
    Character {
        id: "masterchief",
        name: "Master Chief",
        prefixes: &[],
        folders: &["masterchief", "master_chief", "chief", "spartan"],
        codes: &["mas", "chf", "mc"],
    },
    Character {
        id: "arbiter",
        name: "Arbiter",
        prefixes: &[],
        folders: &["arbiter", "dervish"],
        codes: &["arb", "der"],
    },
    Character {
        id: "cortana",
        name: "Cortana",
        prefixes: &[],
        folders: &["cortana"],
        codes: &["cor"],
    },
    Character {
        id: "johnson",
        name: "Sergeant Johnson",
        prefixes: &[],
        folders: &["johnson", "sergeant_johnson"],
        codes: &["jon", "jhn", "joh"],
    },
    Character {
        id: "guiltyspark",
        name: "343 Guilty Spark",
        prefixes: &[],
        folders: &["monitor", "guilty_spark", "343_guilty_spark"],
        codes: &["gsp", "spk", "mon"],
    },
    Character {
        id: "miranda",
        name: "Commander Miranda Keyes",
        prefixes: &[],
        folders: &["miranda", "miranda_keyes"],
        codes: &["mir", "kys"],
    },
    Character {
        id: "hood",
        name: "Lord Hood",
        prefixes: &[],
        folders: &["hood", "lord_hood"],
        codes: &["hod", "hoo"],
    },
    Character {
        id: "tartarus",
        name: "Tartarus",
        prefixes: &[],
        folders: &["tartarus"],
        codes: &["tar"],
    },
    Character {
        id: "truth",
        name: "Prophet of Truth",
        prefixes: &[],
        folders: &["truth", "prophet_of_truth"],
        codes: &["tru", "pot"],
    },
    Character {
        id: "gravemind",
        name: "Gravemind",
        prefixes: &[],
        folders: &["gravemind"],
        codes: &["grv", "gra"],
    },
];

/// The character `id`.
pub fn character(id: &str) -> Option<&'static Character> {
    CHARACTERS.iter().find(|c| c.id == id)
}

/// A character id: lowercase letters, digits, `-` and `_`.
pub fn check_id(id: &str) -> Result<&str, VoiceError> {
    let ok = !id.is_empty()
        && id.len() <= 48
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if ok {
        Ok(id)
    } else {
        Err(VoiceError::Invalid(
            "character: lowercase letters, digits, - and _ (cortana, masterchief)".into(),
        ))
    }
}

/// `name` as the rules see it: lowercase, backslashes.
pub fn norm(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('/', "\\")
}

/// Whether `tag` is a dialogue sound.
pub fn is_dialogue(tag: &str) -> bool {
    norm(tag).starts_with(r"sound\dialog\")
}

/// The leaf of a tag path.
fn leaf(tag: &str) -> &str {
    tag.rsplit('\\').next().unwrap_or(tag)
}

/// A leaf's speaker code: the letters after its last `_`, digits after
/// them allowed (`l01_0010_jon` and `l01_0010_jon2` are `jon`).
pub fn speaker_code(tag: &str) -> Option<String> {
    let n = norm(tag);
    let (_, last) = leaf(&n).rsplit_once('_')?;
    let letters = last.trim_end_matches(|c: char| c.is_ascii_digit());
    (2..=4)
        .contains(&letters.len())
        .then_some(letters)
        .filter(|l| l.chars().all(|c| c.is_ascii_lowercase()))
        .map(str::to_owned)
}

/// The rules a dataset is built by: a character's, or ones given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    pub prefixes: Vec<String>,
    pub folders: Vec<String>,
    pub codes: Vec<String>,
    /// Text any part of the path holds (given rules only).
    pub contains: Vec<String>,
}

impl Rules {
    pub fn of(c: &Character) -> Rules {
        let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
        Rules {
            prefixes: own(c.prefixes),
            folders: own(c.folders),
            codes: own(c.codes),
            contains: Vec::new(),
        }
    }

    /// Rules from `paths`: each a piece of a tag path (`dialog\combat\elite`).
    pub fn paths(paths: &[String]) -> Rules {
        Rules {
            contains: paths
                .iter()
                .map(|p| norm(p))
                .filter(|p| !p.is_empty())
                .collect(),
            ..Rules::default()
        }
    }

    /// Whether `tag` is a dialogue sound these rules pick.
    pub fn matches(&self, tag: &str) -> bool {
        let n = norm(tag);
        let Some(rest) = n.strip_prefix(r"sound\dialog\") else {
            return false;
        };
        if self.prefixes.iter().any(|p| n.starts_with(p.as_str())) {
            return true;
        }
        if self.contains.iter().any(|p| n.contains(p.as_str())) {
            return true;
        }
        let dirs: Vec<&str> = rest.split('\\').collect();
        let dirs = &dirs[..dirs.len().saturating_sub(1)];
        if dirs.iter().any(|d| self.folders.iter().any(|f| f == d)) {
            return true;
        }
        speaker_code(&n).is_some_and(|c| self.codes.iter().any(|x| *x == c))
    }

    pub fn json(&self) -> Value {
        json!({
            "prefixes": self.prefixes,
            "folders": self.folders,
            "codes": self.codes,
            "paths": self.contains,
        })
    }
}

/// A group of dialogue sounds that seem to share a speaker: a speaker code
/// at the end of the tag name, else the first two folders under
/// `sound\dialog\`. For `voice.characters`, to put [`CHARACTERS`] right.
pub fn speaker_of(tag: &str) -> Option<(&'static str, String)> {
    let n = norm(tag);
    let rest = n.strip_prefix(r"sound\dialog\")?;
    if let Some(c) = speaker_code(&n) {
        return Some(("code", c));
    }
    let dirs: Vec<&str> = rest.split('\\').collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    (!dirs.is_empty()).then(|| {
        (
            "folder",
            dirs.iter().take(2).copied().collect::<Vec<_>>().join("\\"),
        )
    })
}

// ---- where the sounds come from ----

/// A dialogue sound of a map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogueSound {
    /// The tag's datum (`map::TagId`'s value).
    pub id: u32,
    /// The tag path.
    pub name: String,
    /// The header's class byte (`snd!` +2), kept in the manifest.
    pub class: u8,
}

/// Decoded audio: interleaved 16-bit samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub rate: u32,
    pub channels: u16,
    pub pcm: Vec<i16>,
}

/// The sounds, subtitles and audio of a source's maps.
pub trait LineSource {
    /// The maps to read, in order.
    fn maps(&mut self) -> Vec<String>;
    /// The map's `snd!` tags under `sound\dialog\`.
    fn dialogue(&mut self, map: &str) -> Result<Vec<DialogueSound>, String>;
    /// The map's subtitles: each string-list entry's string id name and
    /// its English text.
    fn subtitles(&mut self, map: &str) -> Result<Vec<(String, String)>, String>;
    /// How many permutations the sound's first pitch range has.
    fn permutations(&mut self, map: &str, sound: &DialogueSound) -> Result<usize, String>;
    /// Permutation `permutation` of the sound's first pitch range.
    fn decode(
        &mut self,
        map: &str,
        sound: &DialogueSound,
        permutation: usize,
    ) -> Result<Decoded, String>;
}

/// The subtitles of a map: every `unic` tag's English entries, by string
/// id name. Cutscene and mission subtitles are believed to be named as the
/// dialogue sound they caption (docs/editor/voice-cloning.md, "Pairing a
/// line with its text"); an entry whose name does not resolve is left out.
pub fn subtitles_of<R: Read + Seek>(
    map: &mut map::Map<R>,
) -> Result<Vec<(String, String)>, String> {
    use tags::localization::{Language, Localization};
    let loc = Localization::read(map).map_err(|e| format!("the map's strings: {e}"))?;
    let unics: Vec<map::TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == map::FourCC(*b"unic"))
        .map(|t| t.id)
        .collect();
    let mut out = Vec::new();
    for id in unics {
        let Ok(unic) = loc.unic(map, id) else {
            continue;
        };
        let Ok(entries) = loc.entries_for(&unic, Language::English) else {
            continue;
        };
        for e in entries {
            let Ok(Some(name)) = e.id.resolve(map.strings()) else {
                continue;
            };
            if let Ok(Some(text)) = loc.text_in(&unic, e.id, Language::English) {
                out.push((name.to_owned(), text));
            }
        }
    }
    Ok(out)
}

/// A subtitle as training text: whitespace collapsed, `[stage directions]`
/// and a leading `Speaker:` taken off. `None` when nothing is left.
pub fn clean_text(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    let mut t = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // A speaker's name: up to three capitalised words ("Cortana", "Sgt.
    // Johnson"), not the start of a sentence ("Note this: ...").
    if let Some((who, rest)) = t.split_once(": ")
        && who.len() <= 24
        && (1..=3).contains(&who.split(' ').count())
        && who.split(' ').all(|w| {
            w.chars()
                .next()
                .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
        })
        && who
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '\'' | '-'))
    {
        t = rest.trim().to_owned();
    }
    (!t.is_empty()).then_some(t)
}

// ---- the audio ----

/// A biquad, RBJ's cookbook, direct form I.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn new(rate: u32, hz: f64, high: bool) -> Biquad {
        let w = 2.0 * std::f64::consts::PI * hz / f64::from(rate);
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = if high {
            ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0)
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0)
        };
        Biquad {
            b: [b0 / a0, b1 / a0, b2 / a0],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// Interleaved `pcm` as one channel, the channels averaged.
pub fn downmix(pcm: &[i16], channels: u16) -> Vec<i16> {
    let ch = usize::from(channels.max(1));
    if ch == 1 {
        return pcm.to_vec();
    }
    pcm.chunks_exact(ch)
        .map(|f| (f.iter().map(|&s| i32::from(s)).sum::<i32>() / ch as i32) as i16)
        .collect()
}

/// Mono `pcm` from `from` Hz to `to` Hz. Going down, it is low-passed
/// first (two Butterworth sections at 45% of the new rate) so the linear
/// interpolation does not fold the top octave into the speech.
pub fn resample(pcm: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || pcm.is_empty() {
        return pcm.to_vec();
    }
    let mut x: Vec<f32> = pcm.iter().map(|&s| f32::from(s) / 32768.0).collect();
    if to < from {
        let cut = f64::from(to) * 0.45;
        let mut a = Biquad::new(from, cut, false);
        let mut b = Biquad::new(from, cut, false);
        for s in &mut x {
            *s = b.run(a.run(f64::from(*s))) as f32;
        }
    }
    ::audio::resample::resample_linear(&x, 1, from, to)
        .into_iter()
        .map(|v| (f64::from(v) * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// What a clip's spectrum says about how it was recorded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Analysis {
    /// The share of the energy under 250 Hz.
    pub low_share: f64,
    /// The share of the energy over 4 kHz.
    pub high_share: f64,
    /// The share of samples at full scale.
    pub clipped_share: f64,
}

/// Mono `pcm`'s [`Analysis`].
pub fn analyse(pcm: &[i16], rate: u32) -> Analysis {
    let (mut lo0, mut lo1) = (
        Biquad::new(rate, 250.0, false),
        Biquad::new(rate, 250.0, false),
    );
    let high_hz = 4000.0_f64.min(f64::from(rate) * 0.45);
    let (mut hi0, mut hi1) = (
        Biquad::new(rate, high_hz, true),
        Biquad::new(rate, high_hz, true),
    );
    let (mut total, mut low, mut high, mut clipped) = (0.0f64, 0.0f64, 0.0f64, 0usize);
    for &s in pcm {
        if s == i16::MAX || s == i16::MIN {
            clipped += 1;
        }
        let x = f64::from(s) / 32768.0;
        let l = lo1.run(lo0.run(x));
        let h = hi1.run(hi0.run(x));
        total += x * x;
        low += l * l;
        high += h * h;
    }
    let share = |e: f64| if total > 0.0 { e / total } else { 0.0 };
    Analysis {
        low_share: share(low),
        high_share: share(high),
        clipped_share: if pcm.is_empty() {
            0.0
        } else {
            clipped as f64 / pcm.len() as f64
        },
    }
}

/// What a clip must be to be kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filter {
    /// Shortest and longest clip, after trimming (ms).
    pub min_ms: u64,
    pub max_ms: u64,
    /// Keep clips that sound like a radio.
    pub keep_radio: bool,
    /// A clip is radio-filtered when both its low share is under this…
    pub radio_low_share: f64,
    /// …and its high share is under this.
    pub radio_high_share: f64,
    /// A clip with more of its samples at full scale is dropped.
    pub max_clipped_share: f64,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            min_ms: 1_000,
            max_ms: 15_000,
            keep_radio: false,
            radio_low_share: 0.01,
            radio_high_share: 0.004,
            max_clipped_share: 0.001,
        }
    }
}

impl Filter {
    /// Whether `a` is a band-limited (radio, intercom) recording: almost
    /// nothing under 250 Hz and almost nothing over 4 kHz.
    pub fn radio(&self, a: &Analysis) -> bool {
        a.low_share < self.radio_low_share && a.high_share < self.radio_high_share
    }

    pub fn json(&self) -> Value {
        json!({
            "min_ms": self.min_ms,
            "max_ms": self.max_ms,
            "keep_radio": self.keep_radio,
            "radio_low_share": self.radio_low_share,
            "radio_high_share": self.radio_high_share,
            "max_clipped_share": self.max_clipped_share,
        })
    }
}

// ---- the manifest ----

/// Where a line's text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFrom {
    /// The map's subtitle for the sound.
    Subtitle,
    /// The local Whisper's transcript.
    Asr,
    /// Waiting for the local Whisper.
    Pending,
    /// The Whisper heard nothing: not used.
    None,
}

impl TextFrom {
    pub fn id(self) -> &'static str {
        match self {
            TextFrom::Subtitle => "subtitle",
            TextFrom::Asr => "asr",
            TextFrom::Pending => "pending",
            TextFrom::None => "none",
        }
    }

    fn parse(s: &str) -> Option<TextFrom> {
        Some(match s {
            "subtitle" => TextFrom::Subtitle,
            "asr" => TextFrom::Asr,
            "pending" => TextFrom::Pending,
            "none" => TextFrom::None,
            _ => return None,
        })
    }
}

/// One clip of the dataset: a manifest line.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The clip's id: `<tag leaf>-<8 hex>`.
    pub id: String,
    /// The 24 kHz mono WAV.
    pub path: PathBuf,
    pub text: Option<String>,
    pub text_from: TextFrom,
    pub duration_ms: u64,
    /// The `snd!` tag, its map, its permutation and class.
    pub source_tag: String,
    pub map: String,
    pub permutation: usize,
    pub class: u8,
    /// The source permutation's loudness and peak, trimmed, before any
    /// resampling: what a new line is brought to.
    pub lufs: f64,
    pub peak_dbfs: f64,
    pub low_share: f64,
    pub high_share: f64,
}

impl Line {
    /// Whether the line can be trained on or copied from.
    pub fn usable(&self) -> bool {
        matches!(self.text_from, TextFrom::Subtitle | TextFrom::Asr)
            && self.text.as_deref().is_some_and(|t| !t.trim().is_empty())
    }

    pub fn json(&self) -> Value {
        let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
        json!({
            "id": self.id,
            "path": self.path.display().to_string(),
            "text": self.text,
            "text_from": self.text_from.id(),
            "duration_ms": self.duration_ms,
            "source_tag": self.source_tag,
            "map": self.map,
            "permutation": self.permutation,
            "class": self.class,
            "lufs": meter::round1(self.lufs),
            "peak_dbfs": meter::round1(self.peak_dbfs),
            "low_share": r3(self.low_share),
            "high_share": r3(self.high_share),
        })
    }

    pub fn from_json(v: &Value) -> Option<Line> {
        Some(Line {
            id: v["id"].as_str()?.to_owned(),
            path: PathBuf::from(v["path"].as_str()?),
            text: v["text"].as_str().map(str::to_owned),
            text_from: TextFrom::parse(v["text_from"].as_str()?)?,
            duration_ms: v["duration_ms"].as_u64()?,
            source_tag: v["source_tag"].as_str()?.to_owned(),
            map: v["map"].as_str().unwrap_or_default().to_owned(),
            permutation: v["permutation"].as_u64().unwrap_or(0) as usize,
            class: v["class"].as_u64().unwrap_or(0) as u8,
            lufs: v["lufs"].as_f64().unwrap_or(::audio::meter::SILENCE_DB),
            peak_dbfs: v["peak_dbfs"].as_f64().unwrap_or(::audio::meter::SILENCE_DB),
            low_share: v["low_share"].as_f64().unwrap_or(0.0),
            high_share: v["high_share"].as_f64().unwrap_or(0.0),
        })
    }
}

/// The manifest's lines; an error when it is missing or a line does not read.
pub fn read_manifest(path: &Path) -> Result<Vec<Line>, VoiceError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| VoiceError::Io(format!("{}: {e}", path.display())))?;
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str::<Value>(l)
                .ok()
                .as_ref()
                .and_then(Line::from_json)
                .ok_or_else(|| {
                    VoiceError::Io(format!("{}: line {} does not read", path.display(), i + 1))
                })
        })
        .collect()
}

/// Writes `lines` as JSONL, through a temporary file.
pub fn write_manifest(path: &Path, lines: &[Line]) -> Result<(), VoiceError> {
    let mut s = String::new();
    for l in lines {
        s.push_str(&l.json().to_string());
        s.push('\n');
    }
    write_atomic(path, s.as_bytes())
}

/// A dataset's numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub lines: usize,
    pub usable: usize,
    pub usable_ms: u64,
    pub from_subtitle: usize,
    pub from_asr: usize,
    pub pending_asr: usize,
    pub no_text: usize,
    pub lufs: Option<meter::Spread>,
    pub peak: Option<meter::Spread>,
}

pub fn summarize(lines: &[Line]) -> Summary {
    let count = |f: TextFrom| lines.iter().filter(|l| l.text_from == f).count();
    let usable: Vec<&Line> = lines.iter().filter(|l| l.usable()).collect();
    Summary {
        lines: lines.len(),
        usable: usable.len(),
        usable_ms: usable.iter().map(|l| l.duration_ms).sum(),
        from_subtitle: count(TextFrom::Subtitle),
        from_asr: count(TextFrom::Asr),
        pending_asr: count(TextFrom::Pending),
        no_text: count(TextFrom::None),
        lufs: meter::spread(&lines.iter().map(|l| l.lufs).collect::<Vec<_>>()),
        peak: meter::spread(&lines.iter().map(|l| l.peak_dbfs).collect::<Vec<_>>()),
    }
}

impl Summary {
    pub fn minutes(&self) -> f64 {
        (self.usable_ms as f64 / 6_000.0).round() / 10.0
    }

    pub fn json(&self) -> Value {
        let spread = |s: Option<meter::Spread>| match s {
            Some(s) => json!({
                "p10": meter::round1(s.p10),
                "p50": meter::round1(s.p50),
                "p90": meter::round1(s.p90),
                "max": meter::round1(s.max),
            }),
            None => Value::Null,
        };
        json!({
            "lines": self.lines,
            "usable": self.usable,
            "minutes": self.minutes(),
            "from_subtitle": self.from_subtitle,
            "from_asr": self.from_asr,
            "pending_asr": self.pending_asr,
            "no_text": self.no_text,
            "lufs": spread(self.lufs),
            "peak_dbfs": spread(self.peak),
        })
    }
}

/// The lines the clone copies a delivery from: `wanted` (a clip id or a
/// source tag, `#<permutation>` allowed), or the best one: a subtitled
/// line of 3 to 10 s nearest 7 s, else any line with text up to 12 s.
pub fn pick_reference<'a>(lines: &'a [Line], wanted: Option<&str>) -> Result<&'a Line, VoiceError> {
    if let Some(w) = wanted {
        let (tag, perm) = match w.rsplit_once('#') {
            Some((t, p)) => (t, p.parse::<usize>().ok()),
            None => (w, None),
        };
        let found = lines.iter().find(|l| l.id == w).or_else(|| {
            let t = norm(tag);
            lines
                .iter()
                .find(|l| norm(&l.source_tag) == t && perm.is_none_or(|p| l.permutation == p))
        });
        return match found {
            Some(l) if l.usable() => Ok(l),
            Some(l) => Err(VoiceError::Invalid(format!(
                "reference: {} has no text yet ({})",
                l.id,
                l.text_from.id()
            ))),
            None => Err(VoiceError::Invalid(format!(
                "reference: no line {w:?} in the dataset"
            ))),
        };
    }
    let mut best: Vec<&Line> = lines
        .iter()
        .filter(|l| l.usable() && (3_000..=10_000).contains(&l.duration_ms))
        .collect();
    best.sort_by_key(|l| {
        (
            l.text_from != TextFrom::Subtitle,
            l.duration_ms.abs_diff(7_000),
            l.id.clone(),
        )
    });
    if let Some(&l) = best.first() {
        return Ok(l);
    }
    lines
        .iter()
        .filter(|l| l.usable() && l.duration_ms <= 12_000)
        .max_by_key(|l| l.duration_ms)
        .ok_or_else(|| {
            VoiceError::Setup(
                "no line of the dataset has text yet: wait for its transcription (voice.dataset.status)".into(),
            )
        })
}

// ---- the build ----

/// What to read and keep.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Only these maps (default every map of the source).
    pub maps: Option<Vec<String>>,
    pub filter: Filter,
    /// Most clips kept.
    pub limit: Option<usize>,
    /// Most permutations read of one sound.
    pub max_permutations: Option<usize>,
}

/// What a build did.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub dir: PathBuf,
    pub lines: Vec<Line>,
    /// Dialogue sounds the rules picked (each tag once).
    pub tags: usize,
    /// Permutations decoded.
    pub decoded: usize,
    /// Clips left out, by why.
    pub dropped: BTreeMap<&'static str, usize>,
    /// A few of the clips left out: their tag and why.
    pub dropped_examples: Vec<(String, String)>,
    pub maps_read: usize,
    pub maps_failed: Vec<(String, String)>,
    /// Subtitles the maps that had the character's sounds held.
    pub subtitles: usize,
}

impl Report {
    fn left_out(&mut self, why: &'static str, tag: &str, detail: String) {
        *self.dropped.entry(why).or_default() += 1;
        if self.dropped_examples.len() < 12 {
            self.dropped_examples
                .push((tag.to_owned(), format!("{why}: {detail}")));
        }
    }
}

/// The clip id of permutation `p` of `tag`.
pub fn clip_id(tag: &str, p: usize) -> String {
    let n = norm(tag);
    let stem: String = leaf(&n)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    let h = Sha256::of(format!("{n}#{p}").as_bytes()).to_string();
    format!("{stem}-{}", &h[..8])
}

/// Builds `dir`'s dataset (`<data root>/voices/<character>`): reads the
/// maps, keeps the clips the rules and the filter pass, writes them to
/// `dir/dataset/<id>.wav` (24 kHz mono 16-bit), `dir/manifest.jsonl` and
/// `dir/dataset.json`. The clips a previous build made are replaced; a
/// transcript a previous build's Whisper made is kept for the same clip.
pub fn build(
    src: &mut dyn LineSource,
    character: &str,
    rules: &Rules,
    opts: &Options,
    dir: &Path,
) -> Result<Report, VoiceError> {
    let io = |p: &Path, e: std::io::Error| VoiceError::Io(format!("{}: {e}", p.display()));
    let clips = dir.join(CLIPS);
    // Transcripts already made, by clip id.
    let earlier: HashMap<String, String> = read_manifest(&dir.join(MANIFEST))
        .unwrap_or_default()
        .into_iter()
        .filter(|l| l.text_from == TextFrom::Asr)
        .filter_map(|l| Some((l.id, l.text?)))
        .collect();
    if clips.exists() {
        std::fs::remove_dir_all(&clips).map_err(|e| io(&clips, e))?;
    }
    std::fs::create_dir_all(&clips).map_err(|e| io(&clips, e))?;

    let mut r = Report {
        dir: dir.to_path_buf(),
        ..Report::default()
    };
    let maps = match &opts.maps {
        Some(m) => m.clone(),
        None => src.maps(),
    };
    let mut seen: HashSet<String> = HashSet::new();
    'maps: for map in maps {
        let sounds = match src.dialogue(&map) {
            Ok(s) => s,
            Err(e) => {
                r.maps_failed.push((map.clone(), e));
                continue;
            }
        };
        r.maps_read += 1;
        let mine: Vec<DialogueSound> = sounds
            .into_iter()
            .filter(|s| rules.matches(&s.name) && seen.insert(norm(&s.name)))
            .collect();
        if mine.is_empty() {
            continue;
        }
        r.tags += mine.len();
        let subs: HashMap<String, String> = match src.subtitles(&map) {
            Ok(s) => s
                .into_iter()
                .filter_map(|(k, v)| Some((k.to_ascii_lowercase(), clean_text(&v)?)))
                .collect(),
            Err(e) => {
                r.maps_failed.push((map.clone(), format!("subtitles: {e}")));
                HashMap::new()
            }
        };
        r.subtitles += subs.len();
        for s in &mine {
            let perms = match src.permutations(&map, s) {
                Ok(n) => n,
                Err(e) => {
                    r.left_out("decode", &s.name, e);
                    continue;
                }
            };
            let perms = perms.min(opts.max_permutations.unwrap_or(usize::MAX));
            let n = norm(&s.name);
            // A subtitle names the tag, not a permutation: it is the text
            // only of a sound with one.
            let subtitle = if perms == 1 {
                subs.get(leaf(&n)).cloned()
            } else {
                None
            };
            for p in 0..perms {
                if opts.limit.is_some_and(|l| r.lines.len() >= l) {
                    break 'maps;
                }
                let d = match src.decode(&map, s, p) {
                    Ok(d) => d,
                    Err(e) => {
                        r.left_out("decode", &s.name, e);
                        continue;
                    }
                };
                r.decoded += 1;
                let mono = downmix(&d.pcm, d.channels);
                let Some((trimmed, _)) =
                    audio::trim_silence(&mono, d.rate, HEAD_PAD_MS, TAIL_PAD_MS)
                else {
                    r.left_out("silent", &s.name, "no sample over -50 dBFS".into());
                    continue;
                };
                let ms = trimmed.len() as u64 * 1000 / u64::from(d.rate.max(1));
                if ms < opts.filter.min_ms {
                    r.left_out("too_short", &s.name, format!("{ms} ms"));
                    continue;
                }
                if ms > opts.filter.max_ms {
                    r.left_out("too_long", &s.name, format!("{ms} ms"));
                    continue;
                }
                let a = analyse(&trimmed, d.rate);
                if a.clipped_share > opts.filter.max_clipped_share {
                    r.left_out(
                        "clipped",
                        &s.name,
                        format!("{:.2}% at full scale", a.clipped_share * 100.0),
                    );
                    continue;
                }
                let radio =
                    n.split('\\').any(|part| part.contains("radio")) || opts.filter.radio(&a);
                if radio && !opts.filter.keep_radio {
                    r.left_out(
                        "radio",
                        &s.name,
                        format!(
                            "{:.3} under 250 Hz, {:.4} over 4 kHz",
                            a.low_share, a.high_share
                        ),
                    );
                    continue;
                }
                let levels = meter::measure(&trimmed, 1, d.rate);
                let id = clip_id(&s.name, p);
                let path = clips.join(format!("{id}.wav"));
                let out = resample(&trimmed, d.rate, RATE);
                write_atomic(&path, &audio::write_wav(&out, RATE))?;
                let (text, text_from) = match (&subtitle, earlier.get(&id)) {
                    (Some(t), _) => (Some(t.clone()), TextFrom::Subtitle),
                    (None, Some(t)) => (Some(t.clone()), TextFrom::Asr),
                    (None, None) => (None, TextFrom::Pending),
                };
                r.lines.push(Line {
                    id,
                    path,
                    text,
                    text_from,
                    duration_ms: out.len() as u64 * 1000 / u64::from(RATE),
                    source_tag: s.name.clone(),
                    map: map.clone(),
                    permutation: p,
                    class: s.class,
                    lufs: levels.lufs,
                    peak_dbfs: levels.peak_dbfs,
                    low_share: a.low_share,
                    high_share: a.high_share,
                });
            }
        }
    }
    write_manifest(&dir.join(MANIFEST), &r.lines)?;
    let summary = json!({
        "character": character,
        "rules": rules.json(),
        "filter": opts.filter.json(),
        "tags": r.tags,
        "decoded": r.decoded,
        "dropped": r.dropped,
        "maps_read": r.maps_read,
        "maps_failed": r.maps_failed.iter().map(|(m, w)| json!({"map": m, "why": w})).collect::<Vec<_>>(),
        "subtitles": r.subtitles,
        "dataset": summarize(&r.lines).json(),
    });
    write_atomic(&dir.join(SUMMARY), summary.to_string().as_bytes())?;
    Ok(r)
}

#[cfg(test)]
mod tests;
