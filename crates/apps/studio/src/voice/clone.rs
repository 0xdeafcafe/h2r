//! The local clone of a game character as a [`VoiceSource`]: the line is
//! said by the sidecar's model ([`super::sidecar`]) from one of the
//! character's own lines as the reference (its delivery is what the clone
//! copies), with the character's fine-tuned weights when
//! `voice.clone.train` has made them. The request's `voice` is the
//! character's id. docs/editor/voice-cloning.md, "How it works".
//!
//! The model answers at the dataset's 24 kHz; the line is brought to
//! [`audio::RATE`] here, so the rest of the Announcer's pipeline (trim,
//! normalise, save, play, `voice.commit`) is the one Fish Audio's lines go
//! through.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use content::Sha256;
use serde_json::{Value, json};

use super::dataset::{self, Line};
use super::sidecar::{Backend, Launcher, Sidecar};
use super::{Pcm, Request, VoiceError, VoiceSource, audio};

/// A character's fine-tune record, in `<character>/model/`.
pub const TRAINED: &str = "trained.json";

/// The clone.
pub struct CloneVoice {
    launcher: Arc<dyn Launcher>,
    pub backend: Backend,
    /// `<data root>/voices`: the characters' directories.
    pub voices_dir: PathBuf,
    /// The running sidecar, kept between lines (the model stays loaded).
    sidecar: Option<(Backend, Box<dyn Sidecar>)>,
    /// The reference the last line was said from.
    pub last_reference: Option<Line>,
}

impl CloneVoice {
    pub fn new(launcher: Arc<dyn Launcher>, backend: Backend, voices_dir: PathBuf) -> CloneVoice {
        CloneVoice {
            launcher,
            backend,
            voices_dir,
            sidecar: None,
            last_reference: None,
        }
    }

    pub fn launcher(&self) -> &Arc<dyn Launcher> {
        &self.launcher
    }

    /// The character's fine-tune, when it was made for this backend and
    /// its weights are there: the record and the weights' path.
    pub fn trained(&self, character: &str) -> Option<(Value, PathBuf)> {
        trained(&self.voices_dir.join(character), self.backend)
    }

    /// What says a character's lines, for the cache key: the backend, and
    /// a digest of the fine-tune's record when there is one.
    pub fn model_tag(&self, character: &str) -> String {
        match self.trained(character) {
            Some((v, _)) => {
                let h = Sha256::of(v.to_string().as_bytes()).to_string();
                format!("{}.ft-{}", self.backend.id(), &h[..8])
            }
            None => self.backend.id().to_owned(),
        }
    }

    fn sidecar(&mut self) -> Result<&mut Box<dyn Sidecar>, VoiceError> {
        if self
            .sidecar
            .as_ref()
            .is_none_or(|(b, _)| *b != self.backend)
        {
            self.sidecar = Some((self.backend, self.launcher.launch(self.backend)?));
        }
        Ok(&mut self.sidecar.as_mut().expect("launched").1)
    }
}

/// The fine-tune in `dir/model` for `backend`.
pub fn trained(dir: &Path, backend: Backend) -> Option<(Value, PathBuf)> {
    let bytes = std::fs::read(dir.join("model").join(TRAINED)).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    if v["backend"].as_str() != Some(backend.id()) {
        return None;
    }
    let weights = PathBuf::from(v["checkpoint"].as_str()?);
    weights.exists().then_some((v, weights))
}

/// The request the sidecar's `say` is sent.
pub fn say_params(
    backend: Backend,
    req: &Request,
    reference: &Line,
    checkpoint: Option<&Path>,
    out: &Path,
) -> Value {
    json!({
        "backend": backend.id(),
        "text": req.text,
        "speed": req.speed,
        "reference": {
            "path": reference.path.display().to_string(),
            "text": reference.text,
        },
        "checkpoint": checkpoint.map(|p| p.display().to_string()),
        "out": out.display().to_string(),
    })
}

impl VoiceSource for CloneVoice {
    fn say(&mut self, req: &Request) -> Result<Pcm, VoiceError> {
        dataset::check_id(&req.voice)?;
        if !self.launcher.installed(self.backend) {
            return Err(VoiceError::Setup(format!(
                "the local voice model ({}) is not installed",
                self.backend.id()
            )));
        }
        let dir = self.voices_dir.join(&req.voice);
        let lines = dataset::read_manifest(&dir.join(dataset::MANIFEST)).map_err(|_| {
            VoiceError::Setup(format!(
                "{} has no dataset yet: build it first (voice.dataset.build {{character: \"{}\"}})",
                req.voice, req.voice
            ))
        })?;
        let reference = dataset::pick_reference(&lines, req.reference.as_deref())?.clone();
        let checkpoint = self.trained(&req.voice).map(|(_, p)| p);
        let out = dir.join("tmp").join(format!("{}.wav", req.key()));
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d)
                .map_err(|e| VoiceError::Io(format!("{}: {e}", d.display())))?;
        }
        let params = say_params(self.backend, req, &reference, checkpoint.as_deref(), &out);
        let answer = self.sidecar()?.call("say", params, &mut |_| {});
        if answer.is_err() {
            // A sidecar that failed may be gone: start a new one next time.
            self.sidecar = None;
        }
        answer?;
        let bytes = std::fs::read(&out).map_err(|e| {
            VoiceError::Model(format!(
                "the voice sidecar wrote no audio to {}: {e}",
                out.display()
            ))
        })?;
        let _ = std::fs::remove_file(&out);
        let said = audio::read_wav(&bytes)?;
        if said.samples.is_empty() {
            return Err(VoiceError::Audio("the local voice said nothing".into()));
        }
        self.last_reference = Some(reference);
        Ok(Pcm {
            rate: audio::RATE,
            samples: dataset::resample(&said.samples, said.rate, audio::RATE),
        })
    }

    fn ready(&mut self) -> Result<(), VoiceError> {
        if self.launcher.installed(self.backend) {
            Ok(())
        } else {
            Err(VoiceError::Setup(format!(
                "the local voice model ({}) is not installed",
                self.backend.id()
            )))
        }
    }

    fn name(&self) -> &'static str {
        "local clone"
    }
}
