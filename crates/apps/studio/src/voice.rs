//! Announcer lines from text: the voice source (Fish Audio) behind the
//! [`VoiceSource`] trait, its key in the macOS Keychain ([`key`]), the
//! audio clean-up ([`audio`]) and the cache of what a voice already said.
//! The pipeline that ties them to the open map's sounds is
//! `app::announcer`; the agent API's `voice.*` and the Announcer panel
//! both go through it. docs/editor/announcer.md.
//!
//! Nothing here touches the network except [`fish`], and nothing in a
//! test reaches it: the tests use [`fake::Fake`].

pub mod audio;
pub mod fish;
pub mod key;

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::{Path, PathBuf};

use content::Sha256;

/// The voice announcer lines use unless one is asked for: a Fish Audio
/// model, https://fish.audio/m/86126c4c4dad4911979556b4569802cd/.
pub const DEFAULT_VOICE: &str = "86126c4c4dad4911979556b4569802cd";

/// Longest line the voice is asked to say (characters).
pub const MAX_TEXT: usize = 500;

/// Slowest and fastest the voice's speed may be set to (Fish Audio's range).
pub const SPEED_RANGE: (f32, f32) = (0.5, 2.0);

/// What a voice is asked to say. Everything here is in the cache key:
/// the same request is never sent twice.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub text: String,
    /// The voice's model id.
    pub voice: String,
    /// The TTS model; `None` is the service's default.
    pub model: Option<String>,
    /// 1.0 is the voice's own pace.
    pub speed: f32,
}

impl Request {
    /// A request for `text` in the default voice, checked.
    pub fn new(text: &str) -> Result<Request, VoiceError> {
        Request {
            text: text.trim().to_owned(),
            voice: DEFAULT_VOICE.to_owned(),
            model: None,
            speed: 1.0,
        }
        .checked()
    }

    /// The request, or why it is not one.
    pub fn checked(self) -> Result<Request, VoiceError> {
        let n = self.text.chars().count();
        if n == 0 {
            return Err(VoiceError::Invalid("text: there is nothing to say".into()));
        }
        if n > MAX_TEXT {
            return Err(VoiceError::Invalid(format!(
                "text: {n} characters, at most {MAX_TEXT}"
            )));
        }
        let word = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        };
        if !word(&self.voice) {
            return Err(VoiceError::Invalid(
                "voice: a Fish Audio model id (letters, digits, - and _)".into(),
            ));
        }
        if self.model.as_deref().is_some_and(|m| !word(m)) {
            return Err(VoiceError::Invalid(
                "model: a Fish Audio TTS model name such as s2.1-pro".into(),
            ));
        }
        if !(SPEED_RANGE.0..=SPEED_RANGE.1).contains(&self.speed) {
            return Err(VoiceError::Invalid(format!(
                "speed: {} to {}",
                SPEED_RANGE.0, SPEED_RANGE.1
            )));
        }
        Ok(self)
    }

    /// The cache key: a digest of every setting that changes the audio.
    pub fn key(&self) -> String {
        let canon = format!(
            "h2-announcer-line\0{}\0{}\0{:.2}\0pcm16-mono-{}\0{}",
            self.voice,
            self.model.as_deref().unwrap_or("default"),
            self.speed,
            audio::RATE,
            self.text
        );
        Sha256::of(canon.as_bytes()).to_string()
    }
}

/// What a voice said: mono 16-bit samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pcm {
    pub rate: u32,
    pub samples: Vec<i16>,
}

/// Where lines come from. Fish Audio in Studio ([`fish::Fish`]), a
/// synthetic signal in the tests (`fake::Fake`).
pub trait VoiceSource {
    /// Says `req`. This is the only call that costs anything.
    fn say(&mut self, req: &Request) -> Result<Pcm, VoiceError>;
    /// Whether the source can be used now (its key is there), without
    /// reading the secret.
    fn ready(&mut self) -> Result<(), VoiceError>;
    /// A name for the source, for `voice.list`.
    fn name(&self) -> &'static str;
}

/// Why a line was not made. None of these carries the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceError {
    /// No key in the Keychain.
    NoKey,
    /// The Keychain did not give the key (denied, or no `security`).
    Key(String),
    /// A param is wrong.
    Invalid(String),
    /// The service answered with an error status.
    Api { status: u16, message: String },
    /// The service could not be reached.
    Network(String),
    /// What came back is not audio, or is silent.
    Audio(String),
    /// A file could not be read or written.
    Io(String),
}

impl fmt::Display for VoiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VoiceError::NoKey => f.write_str(&key::add_key_help()),
            VoiceError::Key(s)
            | VoiceError::Invalid(s)
            | VoiceError::Network(s)
            | VoiceError::Audio(s)
            | VoiceError::Io(s) => f.write_str(s),
            VoiceError::Api { status, message } => {
                write!(f, "Fish Audio answered {status}: {message}")
            }
        }
    }
}

impl std::error::Error for VoiceError {}

// ---- the cache ----

/// The lines a voice has said, as files under `<data root>/announcer/cache`:
/// `<key>.wav` (44.1 kHz mono 16-bit, as the voice said it) and
/// `<key>.json` (what was asked for).
#[derive(Clone, Debug)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(announcer_dir: &Path) -> Cache {
        Cache {
            dir: announcer_dir.join("cache"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn wav(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.wav"))
    }

    /// The earlier line for `key`, when it is there and reads.
    pub fn get(&self, key: &str) -> Option<Pcm> {
        let bytes = std::fs::read(self.wav(key)).ok()?;
        let pcm = audio::read_wav(&bytes).ok()?;
        (!pcm.samples.is_empty()).then_some(pcm)
    }

    /// Keeps `pcm` as the line for `req`.
    pub fn put(&self, req: &Request, pcm: &Pcm) -> Result<(), VoiceError> {
        let key = req.key();
        write_atomic(&self.wav(&key), &audio::write_wav(&pcm.samples, pcm.rate))?;
        let meta = serde_json::json!({
            "text": req.text,
            "voice": req.voice,
            "model": req.model,
            "speed": req.speed,
        });
        write_atomic(
            &self.dir.join(format!("{key}.json")),
            meta.to_string().as_bytes(),
        )
    }

    /// Every line kept, newest first: its key and request.
    pub fn lines(&self) -> Vec<(String, Request)> {
        let mut found: Vec<(std::time::SystemTime, String, Request)> = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(key) = p.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
                continue;
            };
            let Some(v) = std::fs::read(&p)
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            else {
                continue;
            };
            let (Some(text), Some(voice)) = (v["text"].as_str(), v["voice"].as_str()) else {
                continue;
            };
            let req = Request {
                text: text.to_owned(),
                voice: voice.to_owned(),
                model: v["model"].as_str().map(str::to_owned),
                speed: v["speed"].as_f64().unwrap_or(1.0) as f32,
            };
            let at = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            found.push((at, key, req));
        }
        found.sort_by_key(|a| std::cmp::Reverse(a.0));
        found.into_iter().map(|(_, k, r)| (k, r)).collect()
    }
}

/// Writes `bytes` to `path` through a temporary file beside it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), VoiceError> {
    let io = |e: std::io::Error| VoiceError::Io(format!("{}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}
