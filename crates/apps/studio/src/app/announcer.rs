//! The Announcer: a line of text said by a voice (Fish Audio), trimmed,
//! brought to the loudness of the retail announcer and saved as a clip.
//! The Announcer panel (`ui::announcer`) and the agent API's `voice.say`,
//! `voice.list` and `voice.commit` (`api::voice`) both call these, so the
//! window and the API are one path. docs/editor/announcer.md.
//!
//! A line is made in steps, each a pure piece in [`crate::voice`]:
//! 1. **Ask**: the cache (`<data root>/announcer/cache`) is read first; a
//!    line the voice has already said is not asked for again. Only a miss
//!    calls the [`VoiceSource`], and so needs the key. Only an answer that
//!    passes the trim and the normalise below is kept in the cache.
//! 2. **Trim** the silence off both ends.
//! 3. **Measure** against the retail announcer lines of the open map
//!    (`sound.measure`'s reference set, `sound\dialog\multiplayer\flavor`),
//!    and **normalise** with one gain to their median loudness, the peak
//!    held at [`audio::CEILING_DBFS`]. With no such map open the target is
//!    our own announcer clips' median, and with none readable the
//!    [`FALLBACK_LUFS`] they were mixed to.
//! 4. **Save** as 44.1 kHz mono 16-bit PCM, the format of
//!    `art/announcer/*.wav`, under the data root: never in the checkout.
//! 5. **Play** it through the Sound tool's audition output.
//!
//! [`App::voice_commit`] is the separate, explicit step that copies a line
//! into `art/announcer/<clip>.wav` for `git add`.
//! [`App::voice_commit_sound`] is the same step into a package: the line
//! becomes a `sound` item of the edit target (`app::sound_import`), the way
//! `import.sound` adds a WAV. A path in a character's dataset or fine-tune
//! is refused ([`check_committable`]): only a line the voice said is a
//! clip.
//!
//! With [`Engine::Clone`] the voice is a game character's local clone
//! (`voice::clone`, `app::voices`): the cache and the lines are under
//! `<data root>/voices/<character>`, a miss needs the sidecar installed
//! instead of a key, and the target loudness is the median of the
//! character's own lines in its dataset ([`App::character_target`]).
//! docs/editor/voice-cloning.md.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use super::App;
use super::audition::{Clip, SoundError};
use crate::api::sound_measure::{self, meter};
use crate::voice::{
    self, Cache, DEFAULT_VOICE, Engine, Request, VoiceError, VoiceSource, audio, dataset,
};

/// What the retail announcer's median loudness was measured at, when no
/// map is open to measure it from (`art/announcer/README.md`, LUFS).
pub const FALLBACK_LUFS: f64 = -14.5;

/// Retail lines measured for the target, at most.
const REFERENCE_LINES: usize = 16;

/// Fewest retail lines a target is taken from.
const MIN_RETAIL: usize = 3;

/// The Announcer's state: where lines come from, and the last one made.
pub struct Announcer {
    pub source: Box<dyn VoiceSource>,
    /// The last line saved, for `voice.commit`.
    pub last: Option<PathBuf>,
}

impl Default for Announcer {
    fn default() -> Self {
        Announcer {
            source: default_source(),
            last: None,
        }
    }
}

/// Fish Audio, with its key from the Keychain.
#[cfg(not(test))]
fn default_source() -> Box<dyn VoiceSource> {
    Box::new(voice::fish::Fish::new(Box::new(voice::key::Keychain)))
}

/// In a test, never the service: the fake voice.
#[cfg(test)]
fn default_source() -> Box<dyn VoiceSource> {
    Box::new(voice::fake::Fake::default())
}

/// What `voice.say` is asked for.
#[derive(Clone, Debug, Default)]
pub struct SayParams {
    pub text: String,
    pub voice: Option<String>,
    pub model: Option<String>,
    pub speed: Option<f32>,
    /// Where to save the WAV; relative to the engine's directory
    /// (`<data root>/announcer`, or `<data root>/voices/<character>`).
    pub out: Option<PathBuf>,
    /// The clip id the line is for (`superbounce`): names the file and
    /// says how to add it.
    pub clip: Option<String>,
    pub play: bool,
    /// A loudness to use instead of the retail announcer's (LUFS).
    pub target_lufs: Option<f64>,
    /// Ask the voice again even when the cache has the line.
    pub fresh: bool,
    /// Who says it. With [`Engine::Clone`], `voice` is the character's id
    /// and the line is saved under `<data root>/voices/<character>`.
    pub engine: Engine,
    /// The clone's reference line (a dataset clip id or source tag).
    pub reference: Option<String>,
}

/// The loudness a line is brought to, and where it came from.
#[derive(Clone, Debug)]
pub struct Target {
    pub lufs: f64,
    /// `given`, `retail`, `our clips`, `character` or `fallback`.
    pub from: &'static str,
    /// Lines the target is the median of.
    pub lines: usize,
    pub lufs_spread: Option<meter::Spread>,
    pub peak_spread: Option<meter::Spread>,
    /// Why not retail, when it is not.
    pub why: Option<String>,
}

/// How a line was played.
#[derive(Clone, Debug)]
pub enum Playback {
    Playing { output: &'static str, duration_ms: u64 },
    /// Why it was not: `play: false`, or no audio output.
    Silent(String),
}

/// How to add a line as a clip.
#[derive(Clone, Debug)]
pub struct Ready {
    pub clip: String,
    /// `art/announcer/<clip>.wav`, in the checkout.
    pub repo_path: String,
    /// The line to add to `boot::medal_announcer::CLIPS` when the clip is
    /// a new one.
    pub clips_entry: String,
    /// The clip already exists: `voice.commit` needs `replace`.
    pub exists: bool,
}

/// A line made.
#[derive(Clone, Debug)]
pub struct Said {
    pub text: String,
    pub voice: String,
    /// The cache key (a digest of the request).
    pub key: String,
    /// The voice was not asked: the line was in the cache.
    pub cached: bool,
    /// The WAV as the voice said it.
    pub raw: PathBuf,
    /// The saved clip.
    pub file: PathBuf,
    pub raw_levels: meter::Levels,
    pub levels: meter::Levels,
    pub trimmed: audio::Trimmed,
    pub gain_db: f64,
    /// The peak ceiling kept the line under the target.
    pub limited: bool,
    pub target: Target,
    pub verdict: meter::Verdict,
    pub playback: Playback,
    pub ready: Option<Ready>,
    pub pcm: Arc<[i16]>,
}

/// A clip id: lowercase letters, digits and `-`, as the medals' are.
pub fn check_clip_id(id: &str) -> Result<&str, VoiceError> {
    let ok = !id.is_empty()
        && id.len() <= 48
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(id)
    } else {
        Err(VoiceError::Invalid(
            "clip: lowercase letters, digits and - (superbounce, double-bounce)".into(),
        ))
    }
}

/// Why `from` is not a line to commit, when it is not: a file inside a
/// character's dataset (`voices/<character>/dataset`, which holds the
/// user's disc audio) or fine-tune (`voices/<character>/model`, the
/// weights), or anything but a `.wav`.
pub fn check_committable(from: &Path) -> Result<(), VoiceError> {
    let real = from.canonicalize().unwrap_or_else(|_| from.to_path_buf());
    for path in [from, real.as_path()] {
        let part = path.components().find_map(|c| {
            let c = c.as_os_str().to_str()?;
            (c == dataset::CLIPS || c == "model").then_some(c.to_owned())
        });
        if let Some(part) = part {
            return Err(VoiceError::Setup(format!(
                "from: {} is in a {part} folder: a dataset or a fine-tune never goes into a package; commit a line the voice said (a file under announcer/ or voices/<character>/lines or clips)",
                from.display()
            )));
        }
    }
    if from.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() != Some("wav") {
        return Err(VoiceError::Invalid(format!(
            "from: {} is not a .wav line",
            from.display()
        )));
    }
    Ok(())
}

/// `text` as a file name: lowercase words joined by `-`, at most 40.
fn slug(text: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let s: String = words.join("-").chars().take(40).collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "line".into() } else { s }
}

/// The checkout's `art/announcer` above `from`, when `from` is in one.
fn find_repo(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|d| d.join("art/announcer").is_dir())
        .map(Path::to_path_buf)
}

impl App {
    /// `<data root>/announcer`: the cache, the lines and the clips made.
    pub fn announcer_dir(&self) -> PathBuf {
        self.data_root().join("announcer")
    }

    /// The loudness a line is brought to: `given`, else the median of the
    /// open map's retail announcer lines, else of our announcer clips,
    /// else [`FALLBACK_LUFS`].
    pub fn announcer_target(&mut self, given: Option<f64>) -> Target {
        let retail = sound_measure::retail_announcer(self, REFERENCE_LINES);
        let (lufs_spread, peak_spread) = (retail.lufs(), retail.peak());
        let of_retail = (retail.measured.len() >= MIN_RETAIL)
            .then_some(lufs_spread)
            .flatten();
        let why = if retail.measured.len() >= MIN_RETAIL {
            None
        } else {
            Some(retail.why.clone().unwrap_or_else(|| {
                format!(
                    "{} of the open map's retail announcer lines read (at least {MIN_RETAIL} are needed)",
                    retail.measured.len()
                )
            }))
        };
        let ours: Vec<meter::Levels> = sound_measure::our_clips()
            .iter()
            .filter(|(id, _)| id.starts_with("announcer/"))
            .filter_map(|(_, b)| sound_measure::wav_levels(b).ok())
            .collect();
        let ours_lufs = meter::spread(&ours.iter().map(|l| l.lufs).collect::<Vec<_>>());
        let ours_peak = meter::spread(&ours.iter().map(|l| l.peak_dbfs).collect::<Vec<_>>());
        let (lufs, from, lines, ls, ps) = if let Some(s) = of_retail {
            (s.p50, "retail", retail.measured.len(), lufs_spread, peak_spread)
        } else if let Some(s) = ours_lufs {
            (s.p50, "our clips", ours.len(), ours_lufs, ours_peak)
        } else {
            (FALLBACK_LUFS, "fallback", 0, None, None)
        };
        match given {
            Some(g) => Target {
                lufs: g,
                from: "given",
                lines: 0,
                lufs_spread: ls,
                peak_spread: ps,
                why: None,
            },
            None => Target {
                lufs,
                from,
                lines,
                lufs_spread: ls,
                peak_spread: ps,
                why: if from == "retail" { None } else { why },
            },
        }
    }

    /// The loudness a character's line is brought to: `given`, else the
    /// median of the character's own lines in its dataset (each measured
    /// from the disc's permutation, trimmed, by `sound.measure`'s meter).
    pub fn character_target(&self, character: &str, given: Option<f64>) -> Target {
        let lines = dataset::read_manifest(
            &self.voices_dir().join(character).join(dataset::MANIFEST),
        )
        .unwrap_or_default();
        let s = dataset::summarize(&lines);
        let (lufs, from, why) = match (given, s.lufs) {
            (Some(g), _) => (g, "given", None),
            (None, Some(l)) => (l.p50, "character", None),
            (None, None) => (
                FALLBACK_LUFS,
                "fallback",
                Some(format!("{character} has no dataset to measure")),
            ),
        };
        Target {
            lufs,
            from,
            lines: if from == "character" { s.lines } else { 0 },
            lufs_spread: s.lufs,
            peak_spread: s.peak,
            why,
        }
    }

    /// Makes a line, in the steps of this module's header.
    pub fn voice_say(&mut self, p: SayParams) -> Result<Said, VoiceError> {
        let clone = p.engine == Engine::Clone;
        let voice = if clone {
            let c = p.voice.as_deref().ok_or_else(|| {
                VoiceError::Invalid("character: whose voice says it (cortana, masterchief)".into())
            })?;
            dataset::check_id(c)?.to_owned()
        } else {
            p.voice.clone().unwrap_or_else(|| DEFAULT_VOICE.to_owned())
        };
        let req = Request {
            text: p.text.trim().to_owned(),
            model: if clone {
                Some(self.voices.clone.model_tag(&voice))
            } else {
                p.model.clone()
            },
            voice,
            speed: p.speed.unwrap_or(1.0),
            reference: if clone { p.reference.clone() } else { None },
        }
        .checked()?;
        let clip_id = p.clip.as_deref().map(check_clip_id).transpose()?;
        if let Some(t) = p.target_lufs
            && !(-60.0..=0.0).contains(&t)
        {
            return Err(VoiceError::Invalid("target_lufs: -60 to 0".into()));
        }
        let dir = if clone {
            self.voices_dir().join(&req.voice)
        } else {
            self.announcer_dir()
        };
        let cache = Cache::new(&dir);
        let key = req.key();
        let raw_file = cache.wav(&key);

        // Where it will be saved, checked before anything is asked.
        let file = match (&p.out, clip_id) {
            (Some(o), _) => {
                if o.extension().and_then(|x| x.to_str()) != Some("wav") {
                    return Err(VoiceError::Invalid("out: a .wav file".into()));
                }
                if o.is_absolute() { o.clone() } else { dir.join(o) }
            }
            (None, Some(c)) => dir.join("clips").join(format!("{c}.wav")),
            (None, None) => dir
                .join("lines")
                .join(format!("{}-{}.wav", slug(&req.text), &key[..8])),
        };
        // 1. Ask, unless the voice has said it already.
        let kept = if p.fresh { None } else { cache.get(&key) };
        let (raw, cached) = match kept {
            Some(pcm) => (pcm, true),
            None => {
                let pcm = if clone {
                    self.voice_clone_ready()?;
                    self.voices.clone.say(&req)?
                } else {
                    self.announcer.source.say(&req)?
                };
                (pcm, false)
            }
        };
        if raw.rate != audio::RATE {
            return Err(VoiceError::Audio(format!(
                "the voice said it at {} Hz, not {}",
                raw.rate,
                audio::RATE
            )));
        }

        // 2. Trim, 3. measure and normalise.
        let raw_levels = meter::measure(&raw.samples, 1, raw.rate);
        let (trimmed_pcm, trimmed) =
            audio::trim_silence(&raw.samples, raw.rate, audio::HEAD_PAD_MS, audio::TAIL_PAD_MS)
                .ok_or_else(|| VoiceError::Audio("the line is silent".into()))?;
        let target = if clone {
            self.character_target(&req.voice, p.target_lufs)
        } else {
            self.announcer_target(p.target_lufs)
        };
        let n = audio::normalise(&trimmed_pcm, raw.rate, target.lufs, audio::CEILING_DBFS)?;
        // Only an answer that passed the trim and the normalise is kept:
        // a silent or broken line is never served from the cache again.
        if !cached {
            cache.put(&req, &raw)?;
        }
        let levels = meter::measure(&n.pcm, 1, raw.rate);
        let character_lines = format!("{}'s lines on the disc", req.voice);
        let verdict = meter::verdict(
            &levels,
            target.lufs_spread,
            target.peak_spread,
            match target.from {
                _ if clone => character_lines.as_str(),
                "retail" => "the retail announcer lines",
                "our clips" => "our announcer clips",
                _ => "the announcer's mix",
            },
        );

        // 4. Save.
        voice::write_atomic(&file, &audio::write_wav(&n.pcm, raw.rate))?;
        self.announcer.last = Some(file.clone());

        // 5. Play.
        let pcm: Arc<[i16]> = n.pcm.into();
        let playback = if p.play {
            let clip = Clip {
                rate: raw.rate,
                channels: 1,
                pcm: pcm.clone(),
            };
            match self.sound_play_clip(&clip) {
                Ok(s) => Playback::Playing {
                    output: s.output,
                    duration_ms: s.duration_ms,
                },
                Err(SoundError::Refused(w) | SoundError::Failed(w)) => Playback::Silent(w),
            }
        } else {
            Playback::Silent("play was off".into())
        };

        let ready = clip_id.map(|c| {
            let repo_path = format!("art/announcer/{c}.wav");
            let exists = crate::api::sound_measure::our_clips()
                .iter()
                .any(|(id, _)| id == &format!("announcer/{c}"));
            Ready {
                clip: c.to_owned(),
                clips_entry: format!(
                    "(\"{c}\", include_bytes!(\"../../../../art/announcer/{c}.wav\")),"
                ),
                repo_path,
                exists,
            }
        });
        Ok(Said {
            text: req.text,
            voice: req.voice,
            key,
            cached,
            raw: raw_file,
            file,
            raw_levels,
            levels,
            trimmed,
            gain_db: n.gain_db,
            limited: n.limited,
            target,
            verdict,
            playback,
            ready,
            pcm,
        })
    }

    /// The source's state and the lines it has said.
    pub fn voice_list(&mut self) -> Value {
        let cache = Cache::new(&self.announcer_dir());
        let lines = cache.lines();
        let key = match self.announcer.source.ready() {
            Ok(()) => json!({"state": "present"}),
            Err(VoiceError::NoKey) => json!({
                "state": "missing",
                "service": voice::key::SERVICE,
                "command": voice::key::ADD_COMMAND,
            }),
            Err(e) => json!({"state": "unreadable", "why": e.to_string()}),
        };
        let mut voices: Vec<(String, usize)> = vec![(DEFAULT_VOICE.to_owned(), 0)];
        for (_, r) in &lines {
            match voices.iter_mut().find(|(v, _)| *v == r.voice) {
                Some(v) => v.1 += 1,
                None => voices.push((r.voice.clone(), 1)),
            }
        }
        json!({
            "source": self.announcer.source.name(),
            "default": DEFAULT_VOICE,
            "voices": voices.iter().map(|(id, n)| json!({
                "id": id,
                "default": id == DEFAULT_VOICE,
                "url": format!("https://fish.audio/m/{id}/"),
                "lines": n,
            })).collect::<Vec<_>>(),
            "key": key,
            "cache": {"dir": cache.dir().display().to_string(), "lines": lines.len()},
            "recent": lines.iter().take(8).map(|(k, r)| json!({
                "key": k, "text": r.text, "voice": r.voice,
            })).collect::<Vec<_>>(),
        })
    }

    /// Copies a saved line (`from`, else the last one made) to
    /// `<repo>/art/announcer/<clip>.wav` in the checkout (`repo`, else the
    /// one the editor runs in). It does not `git add` it, and it does not
    /// touch `boot::medal_announcer::CLIPS`: the reply says what to add.
    /// A clip that already exists is only replaced with `replace`.
    pub fn voice_commit(
        &mut self,
        clip: &str,
        from: Option<&Path>,
        repo: Option<&Path>,
        replace: bool,
    ) -> Result<(PathBuf, Ready), VoiceError> {
        let clip = check_clip_id(clip)?;
        let from = from
            .map(Path::to_path_buf)
            .or_else(|| self.announcer.last.clone())
            .ok_or_else(|| VoiceError::Invalid("from: no line has been made yet".into()))?;
        let bytes = std::fs::read(&from)
            .map_err(|e| VoiceError::Io(format!("{}: {e}", from.display())))?;
        let c = boot::medal_announcer::parse_wav(&bytes)
            .map_err(|e| VoiceError::Audio(format!("{}: {e}", from.display())))?;
        if (c.channels, c.rate) != (1, audio::RATE) {
            return Err(VoiceError::Audio(format!(
                "{}: our announcer clips are 44.1 kHz mono, this is {} Hz in {} channels",
                from.display(),
                c.rate,
                c.channels
            )));
        }
        let root = match repo {
            Some(r) if r.join("art/announcer").is_dir() => r.to_path_buf(),
            Some(r) => {
                return Err(VoiceError::Invalid(format!(
                    "repo: {} has no art/announcer",
                    r.display()
                )));
            }
            None => std::env::current_dir()
                .ok()
                .and_then(|d| find_repo(&d))
                .ok_or_else(|| {
                    VoiceError::Invalid(
                        "repo: the editor is not in a checkout (no art/announcer above it); pass repo"
                            .into(),
                    )
                })?,
        };
        let to = root.join("art/announcer").join(format!("{clip}.wav"));
        let exists = to.exists();
        if exists && !replace {
            return Err(VoiceError::Invalid(format!(
                "{} exists: pass replace to overwrite it",
                to.display()
            )));
        }
        voice::write_atomic(&to, &bytes)?;
        Ok((
            to,
            Ready {
                clip: clip.to_owned(),
                repo_path: format!("art/announcer/{clip}.wav"),
                clips_entry: format!(
                    "(\"{clip}\", include_bytes!(\"../../../../art/announcer/{clip}.wav\")),"
                ),
                exists,
            },
        ))
    }

    /// Adds a saved line (`from`, else the last one made) to the local
    /// package `package` as a `sound` item that writes the `snd!` `tag`
    /// (`class` as `import.sound`'s). The WAV is copied into the package
    /// unchanged; the package's build resamples and encodes it. A path in
    /// a dataset or fine-tune is refused first.
    pub fn voice_commit_sound(
        &mut self,
        tag: &str,
        from: Option<&Path>,
        package: &str,
        class: Option<&str>,
    ) -> Result<Value, VoiceError> {
        let from = from
            .map(Path::to_path_buf)
            .or_else(|| self.announcer.last.clone())
            .ok_or_else(|| VoiceError::Invalid("from: no line has been made yet".into()))?;
        check_committable(&from)?;
        super::sound_import::import_sound(&self.data_root(), &from, package, tag, class).map_err(
            |e| match e {
                super::assets::CopyRefusal::Params(m) => VoiceError::Invalid(m),
                super::assets::CopyRefusal::Refused(m) => VoiceError::Setup(m),
                super::assets::CopyRefusal::Failed(m) => VoiceError::Io(m),
            },
        )
    }
}
