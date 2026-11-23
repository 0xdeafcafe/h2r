//! Voices: game characters cloned from the user's own disc. The dataset
//! builder over the source's maps ([`AppLines`], the Tags tool's audition
//! decoder), the clone's install, transcription and training as background
//! jobs over the sidecar, and the state the Voices window and the agent
//! API's `voice.characters`, `voice.dataset.*` and `voice.clone.*` share.
//! Saying a line is `App::voice_say` with `Engine::Clone`
//! (`app::announcer`). docs/editor/voice-cloning.md.
//!
//! Everything is written under `<data root>/voices/<character>/`
//! (datasets, fine-tunes, lines) or the caches dir (the sidecar's venv and
//! base weights): never in a checkout, never uploaded.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use map::TagId;
use serde_json::{Value, json};

use super::App;
use crate::voice::clone::{CloneVoice, TRAINED};
use crate::voice::dataset::{
    self, CHARACTERS, Decoded, DialogueSound, LineSource, Options, Rules, TextFrom,
};
use crate::voice::sidecar::{Backend, Launcher, Progress};
use crate::voice::{VoiceError, write_atomic};

/// Clips transcribed per sidecar request.
const ASR_BATCH: usize = 16;

/// The fewest minutes of text-bearing speech a fine-tune starts from.
pub const MIN_TRAIN_MINUTES: f64 = 10.0;

/// A background job's state, shared with its thread.
#[derive(Debug, Default)]
struct Shared {
    progress: Progress,
    log: Vec<String>,
    outcome: Option<Result<Value, String>>,
    ended_ms: Option<u64>,
}

/// A background job: install, transcribe or train.
struct Job {
    id: u64,
    kind: &'static str,
    character: Option<String>,
    backend: Backend,
    started: Instant,
    shared: Arc<Mutex<Shared>>,
}

impl Job {
    fn running(&self) -> bool {
        self.shared
            .lock()
            .map(|s| s.outcome.is_none())
            .unwrap_or(false)
    }

    fn json(&self) -> Value {
        let s = self.shared.lock().expect("job state");
        let elapsed = s
            .ended_ms
            .unwrap_or_else(|| self.started.elapsed().as_millis() as u64);
        let mut v = json!({
            "id": self.id,
            "kind": self.kind,
            "character": self.character,
            "backend": self.backend.id(),
            "state": match &s.outcome {
                None => "running",
                Some(Ok(_)) => "done",
                Some(Err(_)) => "failed",
            },
            "stage": s.progress.stage,
            "done": s.progress.done,
            "total": s.progress.total,
            "text": s.progress.text,
            "elapsed_ms": elapsed,
            "log": s.log,
        });
        match &s.outcome {
            Some(Ok(r)) => v["result"] = r.clone(),
            Some(Err(w)) => v["why"] = json!(w),
            None => {}
        }
        v
    }
}

/// The Voices state in the [`App`].
pub struct Voices {
    launcher: Arc<dyn Launcher>,
    /// The clone that says lines (`voice_say` with [`Engine::Clone`]).
    pub clone: CloneVoice,
    /// Where the dataset builder reads from instead of the source's maps
    /// (the tests' invented maps).
    pub lines: Option<Box<dyn LineSource>>,
    jobs: Vec<Job>,
    next_job: u64,
}

impl Default for Voices {
    fn default() -> Self {
        Voices::with_launcher(default_launcher())
    }
}

#[cfg(not(test))]
fn default_launcher() -> Arc<dyn Launcher> {
    Arc::new(crate::voice::sidecar::Managed::default())
}

/// In a test, never Python: the fake sidecar, not installed.
#[cfg(test)]
fn default_launcher() -> Arc<dyn Launcher> {
    Arc::new(crate::voice::fake::FakeLauncher::default())
}

impl Voices {
    pub fn with_launcher(launcher: Arc<dyn Launcher>) -> Voices {
        Voices {
            clone: CloneVoice::new(launcher.clone(), Backend::default(), PathBuf::new()),
            launcher,
            lines: None,
            jobs: Vec::new(),
            next_job: 1,
        }
    }

    /// Uses `launcher` from now on (the tests' fake).
    pub fn set_launcher(&mut self, launcher: Arc<dyn Launcher>) {
        let backend = self.clone.backend;
        *self = Voices {
            lines: self.lines.take(),
            jobs: std::mem::take(&mut self.jobs),
            next_job: self.next_job,
            ..Voices::with_launcher(launcher)
        };
        self.clone.backend = backend;
    }

    pub fn backend(&self) -> Backend {
        self.clone.backend
    }

    fn busy(&self) -> Option<&Job> {
        self.jobs.iter().find(|j| j.running())
    }

    fn job(&self, id: u64) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Starts `work` on a thread as job `kind`. One job runs at a time.
    fn start<F>(
        &mut self,
        kind: &'static str,
        character: Option<&str>,
        work: F,
    ) -> Result<u64, VoiceError>
    where
        F: FnOnce(&mut dyn FnMut(&Progress)) -> Result<Value, VoiceError> + Send + 'static,
    {
        if let Some(j) = self.busy() {
            return Err(VoiceError::Busy(format!(
                "job {} ({}) is still running: voice.dataset.status shows it",
                j.id, j.kind
            )));
        }
        let id = self.next_job;
        self.next_job += 1;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let started = Instant::now();
        let s = shared.clone();
        std::thread::Builder::new()
            .name(format!("voice-{kind}"))
            .spawn(move || {
                let mut on = |p: &Progress| {
                    if let Ok(mut st) = s.lock() {
                        if let Some(t) = &p.text {
                            st.log.push(t.clone());
                            if st.log.len() > 20 {
                                st.log.remove(0);
                            }
                        }
                        st.progress = p.clone();
                    }
                };
                let r = work(&mut on);
                if let Ok(mut st) = s.lock() {
                    st.ended_ms = Some(started.elapsed().as_millis() as u64);
                    st.outcome = Some(r.map_err(|e| e.to_string()));
                }
            })
            .map_err(|e| VoiceError::Io(format!("a job thread: {e}")))?;
        // Keep the last few jobs for status.
        if self.jobs.len() >= 8 {
            self.jobs.remove(0);
        }
        self.jobs.push(Job {
            id,
            kind,
            character: character.map(str::to_owned),
            backend: self.clone.backend,
            started,
            shared,
        });
        Ok(id)
    }

    /// Starts installing the backend, unless it is installed or a job
    /// runs. The job's id.
    pub fn start_install(&mut self) -> Result<Option<u64>, VoiceError> {
        let b = self.clone.backend;
        if self.launcher.installed(b) {
            return Ok(None);
        }
        if let Some(j) = self.busy() {
            return Ok(Some(j.id));
        }
        let l = self.launcher.clone();
        self.start("install", None, move |on| {
            l.install(b, on)?;
            Ok(json!({"installed": b.id()}))
        })
        .map(Some)
    }

    /// The sidecar's state.
    pub fn sidecar_json(&self) -> Value {
        let b = self.clone.backend;
        let installing = self.jobs.iter().any(|j| j.kind == "install" && j.running());
        json!({
            "backend": b.id(),
            "backends": Backend::ALL.iter().map(|b| b.id()).collect::<Vec<_>>(),
            "state": if self.launcher.installed(b) {
                "installed"
            } else if installing {
                "installing"
            } else {
                "missing"
            },
            "dir": self.launcher.dir(b).display().to_string(),
            "trains": b.trains(),
        })
    }
}

/// Installs `backend` when it is not, inside a job.
fn ensure_installed(
    l: &Arc<dyn Launcher>,
    b: Backend,
    on: &mut dyn FnMut(&Progress),
) -> Result<(), VoiceError> {
    if !l.installed(b) {
        l.install(b, on)?;
    }
    Ok(())
}

/// The source's maps, read through the Tags tool: each map is browsed in
/// turn (as `tag.browse` does) and its sounds decoded by the audition
/// (`App::sound_clip`). The map shown before is shown again after.
pub struct AppLines<'a> {
    app: &'a mut App,
    before: Option<String>,
}

impl<'a> AppLines<'a> {
    pub fn new(app: &'a mut App) -> AppLines<'a> {
        let before = app.browsing().map(str::to_owned);
        AppLines { app, before }
    }

    fn show(&mut self, map: &str) -> Result<(), String> {
        if self
            .app
            .shown_map_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(map))
        {
            return Ok(());
        }
        self.app.browse_tags(Some(map));
        if self
            .app
            .shown_map_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(map))
        {
            Ok(())
        } else {
            Err(format!("{map} did not open"))
        }
    }
}

impl Drop for AppLines<'_> {
    fn drop(&mut self) {
        let before = self.before.take();
        self.app.browse_tags(before.as_deref());
    }
}

impl LineSource for AppLines<'_> {
    fn maps(&mut self) -> Vec<String> {
        let maps = self.app.maps().to_vec();
        if maps.is_empty() {
            self.app
                .shown_map_name()
                .map(str::to_owned)
                .into_iter()
                .collect()
        } else {
            maps
        }
    }

    fn dialogue(&mut self, map: &str) -> Result<Vec<DialogueSound>, String> {
        self.show(map)?;
        let tags = self.app.sound_tags().map_err(|e| e.to_string())?;
        Ok(tags
            .into_iter()
            .filter(|t| dataset::is_dialogue(&t.name))
            .map(|t| DialogueSound {
                id: t.id.0,
                name: t.name,
                class: t.class,
            })
            .collect())
    }

    fn subtitles(&mut self, map: &str) -> Result<Vec<(String, String)>, String> {
        self.show(map)?;
        let mut m = self.app.open_shown_map()?;
        dataset::subtitles_of(&mut m)
    }

    fn permutations(&mut self, map: &str, s: &DialogueSound) -> Result<usize, String> {
        self.show(map)?;
        let info = self
            .app
            .sound_info(TagId(s.id))
            .map_err(|e| e.to_string())?;
        Ok(info.ranges.first().map_or(0, Vec::len))
    }

    fn decode(&mut self, map: &str, s: &DialogueSound, p: usize) -> Result<Decoded, String> {
        self.show(map)?;
        let c = self
            .app
            .sound_clip(TagId(s.id), 0, p)
            .map_err(|e| e.to_string())?;
        Ok(Decoded {
            rate: c.rate,
            channels: c.channels,
            pcm: c.pcm.to_vec(),
        })
    }
}

/// What `voice.dataset.build` is asked for.
#[derive(Clone, Debug, Default)]
pub struct BuildParams {
    pub character: String,
    /// Tag path pieces to use instead of the character's rules.
    pub paths: Option<Vec<String>>,
    pub options: Options,
    /// Transcribe the clips with no subtitle (a background job).
    pub asr: bool,
}

impl App {
    /// `<data root>/voices`.
    pub fn voices_dir(&self) -> PathBuf {
        self.data_root().join("voices")
    }

    /// The rules for `character`: the given paths, else the table's.
    fn voice_rules(character: &str, paths: Option<&[String]>) -> Result<Rules, VoiceError> {
        dataset::check_id(character)?;
        match (paths, dataset::character(character)) {
            (Some(p), _) if !p.is_empty() => Ok(Rules::paths(p)),
            (_, Some(c)) => Ok(Rules::of(c)),
            _ => Err(VoiceError::Invalid(format!(
                "character: {character:?} is not one Studio knows ({}); pass paths to name its tags",
                CHARACTERS
                    .iter()
                    .map(|c| c.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// Runs `f` over the dataset source: the tests' invented maps, else
    /// the source's maps through the Tags tool.
    fn with_lines<T>(&mut self, f: impl FnOnce(&mut dyn LineSource) -> T) -> T {
        if let Some(mut l) = self.voices.lines.take() {
            let r = f(l.as_mut());
            self.voices.lines = Some(l);
            return r;
        }
        let mut lines = AppLines::new(self);
        f(&mut lines)
    }

    /// The known characters, with how many tags each has in `map` (or
    /// every map of the source), and the speaker codes and folders found.
    pub fn voice_characters(&mut self, map: Option<&str>) -> Value {
        let voices = self.voices_dir();
        let maps_given = map.map(|m| vec![m.to_owned()]);
        let (tags, failed, read) = self.with_lines(|src| {
            let maps = maps_given.unwrap_or_else(|| src.maps());
            let mut tags: BTreeMap<String, String> = BTreeMap::new();
            let mut failed = Vec::new();
            let mut read = 0;
            for m in maps {
                match src.dialogue(&m) {
                    Ok(s) => {
                        read += 1;
                        for d in s {
                            tags.entry(dataset::norm(&d.name)).or_insert(m.clone());
                        }
                    }
                    Err(e) => failed.push(json!({"map": m, "why": e})),
                }
            }
            (tags, failed, read)
        });
        let mut speakers: BTreeMap<(&'static str, String), (usize, String)> = BTreeMap::new();
        for t in tags.keys() {
            if let Some(k) = dataset::speaker_of(t) {
                let e = speakers.entry(k).or_insert((0, t.clone()));
                e.0 += 1;
            }
        }
        let mut speakers: Vec<_> = speakers.into_iter().collect();
        speakers.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
        let characters: Vec<Value> = CHARACTERS
            .iter()
            .map(|c| {
                let rules = Rules::of(c);
                let n = tags.keys().filter(|t| rules.matches(t)).count();
                let dir = voices.join(c.id);
                let ds = dataset::read_manifest(&dir.join(dataset::MANIFEST))
                    .ok()
                    .map(|l| dataset::summarize(&l).json());
                json!({
                    "id": c.id,
                    "name": c.name,
                    "rules": rules.json(),
                    "tags": n,
                    "dataset": ds,
                    "trained": crate::voice::clone::trained(&dir, self.voices.backend()).is_some(),
                })
            })
            .collect();
        let mut v = json!({
            "characters": characters,
            "dialogue_tags": tags.len(),
            "speakers": speakers.iter().take(60).map(|((kind, key), (n, sample))| json!({
                "kind": kind, "key": key, "tags": n, "sample": sample,
            })).collect::<Vec<_>>(),
            "maps_read": read,
            "maps_failed": failed,
        });
        if read == 0 {
            v["why"] = json!("no map was read: open a disc or a folder of maps first");
        }
        v
    }

    /// Builds `p.character`'s dataset, and starts transcribing its clips
    /// without a subtitle when `p.asr`.
    pub fn voice_dataset_build(&mut self, p: BuildParams) -> Result<Value, VoiceError> {
        let rules = App::voice_rules(&p.character, p.paths.as_deref())?;
        if let Some(j) = self.voices.busy() {
            return Err(VoiceError::Busy(format!(
                "job {} ({}) is still running: build when it is done",
                j.id, j.kind
            )));
        }
        let dir = self.voices_dir().join(&p.character);
        let character = p.character.clone();
        let options = p.options.clone();
        let report =
            self.with_lines(|src| dataset::build(src, &character, &rules, &options, &dir))?;
        if report.maps_read == 0 {
            return Err(VoiceError::Setup(
                "no map was read: open a disc or a folder of maps first".into(),
            ));
        }
        let summary = dataset::summarize(&report.lines);
        let asr_job = if p.asr && summary.pending_asr > 0 {
            Some(self.start_asr(&p.character)?)
        } else {
            None
        };
        Ok(json!({
            "character": p.character,
            "dir": dir.display().to_string(),
            "manifest": dir.join(dataset::MANIFEST).display().to_string(),
            "rules": rules.json(),
            "tags": report.tags,
            "decoded": report.decoded,
            "kept": report.lines.len(),
            "dropped": report.dropped,
            "dropped_examples": report.dropped_examples.iter().map(|(t, w)| json!({"tag": t, "why": w})).collect::<Vec<_>>(),
            "maps_read": report.maps_read,
            "maps_failed": report.maps_failed.iter().map(|(m, w)| json!({"map": m, "why": w})).collect::<Vec<_>>(),
            "subtitles": report.subtitles,
            "dataset": summary.json(),
            "asr_job": asr_job,
        }))
    }

    /// Starts the job that transcribes `character`'s clips with no text.
    fn start_asr(&mut self, character: &str) -> Result<u64, VoiceError> {
        let l = self.voices.launcher.clone();
        let b = self.voices.backend();
        let manifest = self.voices_dir().join(character).join(dataset::MANIFEST);
        self.voices.start("transcribe", Some(character), move |on| {
            ensure_installed(&l, b, on)?;
            let mut side = l.launch(b)?;
            let mut lines = dataset::read_manifest(&manifest)?;
            let pending: Vec<usize> = (0..lines.len())
                .filter(|&i| lines[i].text_from == TextFrom::Pending)
                .collect();
            let total = pending.len() as u64;
            let (mut heard, mut empty) = (0, 0);
            for (n, batch) in pending.chunks(ASR_BATCH).enumerate() {
                let files: Vec<String> = batch
                    .iter()
                    .map(|&i| lines[i].path.display().to_string())
                    .collect();
                let done = (n * ASR_BATCH) as u64;
                let r = side.call(
                    "transcribe",
                    json!({"files": files, "language": "en"}),
                    &mut |p| {
                        on(&Progress {
                            stage: "transcribe".into(),
                            done: done + p.done,
                            total,
                            text: p.text.clone(),
                        })
                    },
                )?;
                let texts = r["texts"].as_array().cloned().unwrap_or_default();
                for &i in batch {
                    let path = lines[i].path.display().to_string();
                    let text = texts
                        .iter()
                        .find(|t| t["path"].as_str() == Some(path.as_str()))
                        .and_then(|t| t["text"].as_str())
                        .and_then(dataset::clean_text);
                    match text {
                        Some(t) => {
                            lines[i].text = Some(t);
                            lines[i].text_from = TextFrom::Asr;
                            heard += 1;
                        }
                        None => {
                            lines[i].text = None;
                            lines[i].text_from = TextFrom::None;
                            empty += 1;
                        }
                    }
                }
                // Kept after every batch: a stopped job loses one batch.
                dataset::write_manifest(&manifest, &lines)?;
                on(&Progress::new(
                    "transcribe",
                    done + batch.len() as u64,
                    total,
                ));
            }
            Ok(json!({"transcribed": heard, "heard_nothing": empty}))
        })
    }

    /// The sidecar, the jobs, and `character`'s dataset (or every
    /// character's that has one).
    pub fn voice_dataset_status(&self, character: Option<&str>) -> Result<Value, VoiceError> {
        if let Some(c) = character {
            dataset::check_id(c)?;
        }
        let voices = self.voices_dir();
        let ids: Vec<String> = match character {
            Some(c) => vec![c.to_owned()],
            None => std::fs::read_dir(&voices)
                .map(|rd| {
                    let mut v: Vec<String> = rd
                        .flatten()
                        .filter(|e| e.path().join(dataset::MANIFEST).is_file())
                        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                        .collect();
                    v.sort();
                    v
                })
                .unwrap_or_default(),
        };
        let datasets: Vec<Value> = ids
            .iter()
            .map(|c| {
                let dir = voices.join(c);
                let mut v = json!({
                    "character": c,
                    "dir": dir.display().to_string(),
                    "built": dir.join(dataset::MANIFEST).is_file(),
                });
                if let Ok(lines) = dataset::read_manifest(&dir.join(dataset::MANIFEST)) {
                    v["dataset"] = dataset::summarize(&lines).json();
                }
                if let Some(s) = std::fs::read(dir.join(dataset::SUMMARY))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                {
                    v["build"] = json!({
                        "tags": s["tags"], "decoded": s["decoded"], "dropped": s["dropped"],
                        "maps_read": s["maps_read"], "maps_failed": s["maps_failed"],
                    });
                }
                v["model"] = match crate::voice::clone::trained(&dir, self.voices.backend()) {
                    Some((t, w)) => {
                        json!({"trained": true, "checkpoint": w.display().to_string(), "record": t})
                    }
                    None => json!({"trained": false}),
                };
                v
            })
            .collect();
        let jobs: Vec<Value> = self.voices.jobs.iter().rev().map(Job::json).collect();
        Ok(json!({
            "sidecar": self.voices.sidecar_json(),
            "job": jobs.first().cloned(),
            "jobs": jobs,
            "datasets": datasets,
        }))
    }

    /// One job's state.
    pub fn voice_job(&self, id: u64) -> Option<Value> {
        self.voices.job(id).map(Job::json)
    }

    /// Starts fine-tuning `character`'s clone on its dataset. `steps`
    /// overrides the sidecar's default; `min_minutes` the least speech.
    pub fn voice_clone_train(
        &mut self,
        character: &str,
        steps: Option<u64>,
        min_minutes: Option<f64>,
    ) -> Result<Value, VoiceError> {
        dataset::check_id(character)?;
        let b = self.voices.backend();
        if !b.trains() {
            return Err(VoiceError::Invalid(format!(
                "backend {} has no fine-tune here: it clones from a reference only",
                b.id()
            )));
        }
        if let Some(j) = self.voices.busy() {
            return Err(VoiceError::Busy(format!(
                "job {} ({}) is still running: voice.dataset.status shows it",
                j.id, j.kind
            )));
        }
        let dir = self.voices_dir().join(character);
        let lines = dataset::read_manifest(&dir.join(dataset::MANIFEST)).map_err(|_| {
            VoiceError::Setup(format!(
                "{character} has no dataset yet: build it first (voice.dataset.build)"
            ))
        })?;
        let s = dataset::summarize(&lines);
        let need = min_minutes.unwrap_or(MIN_TRAIN_MINUTES);
        if s.minutes() < need {
            return Err(VoiceError::Setup(format!(
                "{character}'s dataset has {} minutes of speech with text ({} lines, {} waiting for transcription); a fine-tune needs {need}",
                s.minutes(),
                s.usable,
                s.pending_asr
            )));
        }
        // The lines it trains on, as their own manifest.
        let usable: Vec<_> = lines.into_iter().filter(|l| l.usable()).collect();
        let train = dir.join("model").join("train.jsonl");
        dataset::write_manifest(&train, &usable)?;
        let out = dir.join("model");
        let l = self.voices.launcher.clone();
        let (minutes, count) = (s.minutes(), s.usable);
        let id = self.voices.start("train", Some(character), move |on| {
            ensure_installed(&l, b, on)?;
            let mut side = l.launch(b)?;
            let mut params = json!({
                "backend": b.id(),
                "manifest": train.display().to_string(),
                "out": out.display().to_string(),
            });
            if let Some(n) = steps {
                params["steps"] = json!(n);
            }
            let r = side.call("train", params, on)?;
            let checkpoint = r["checkpoint"]
                .as_str()
                .ok_or_else(|| VoiceError::Model("the fine-tune named no checkpoint".into()))?
                .to_owned();
            let record = json!({
                "backend": b.id(),
                "checkpoint": checkpoint,
                "steps": r["steps"],
                "lines": count,
                "minutes": minutes,
            });
            write_atomic(&out.join(TRAINED), record.to_string().as_bytes())?;
            Ok(record)
        })?;
        Ok(json!({
            "job": id,
            "state": "running",
            "character": character,
            "backend": b.id(),
            "lines": count,
            "minutes": minutes,
        }))
    }

    /// Before the clone says a line that is not cached: installed (else
    /// the install starts and the line is refused), and the character's
    /// clone pointed at the data root.
    pub(crate) fn voice_clone_ready(&mut self) -> Result<(), VoiceError> {
        self.voices.clone.voices_dir = self.voices_dir();
        if self.voices.launcher.installed(self.voices.backend()) {
            return Ok(());
        }
        let job = self.voices.start_install()?;
        Err(VoiceError::Setup(format!(
            "the local voice model ({}) is being installed{}: watch voice.dataset.status and ask again when its state is installed",
            self.voices.backend().id(),
            job.map(|j| format!(" (job {j})")).unwrap_or_default()
        )))
    }
}
