//! The Voices batch: many lines said by game characters' clones as one
//! job. Rows (a line, whose voice, an output name and optionally the sound
//! tag it replaces) are added, pasted or imported; "Say all" queues them;
//! [`App::poll_voice_batch`] says one row per call (the UI calls it every
//! frame, `voice.say_batch` with `wait` runs it to the end), so each row
//! has its own state: idle, queued, running, done or failed with why. A
//! done row plays again, re-runs, and is kept into the checkout (its output
//! name) or the edit target package (its tag) with the others in one step.
//! docs/editor/voice-cloning.md, "The batch"; the agent API's
//! `voice.say_batch`, `voice.batch.*`.
//!
//! A row is said exactly as `voice.clone.say` says a line
//! ([`App::voice_say`] with [`Engine::Clone`], not played), so the cache,
//! the trim and the loudness are the same. Nothing is written outside
//! `<data root>/voices` until a save.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::App;
use super::announcer::{SayParams, Said};
use crate::voice::{Engine, VoiceError};

/// The most a text file imported into the batch may be.
const MAX_IMPORT: u64 = 1 << 20;

/// The columns a said line covers when it is drawn under an original on
/// one time axis: the original (`axis_ms` long) fills `width` columns, so
/// a line of `line_ms` covers its share of them, and a longer one is cut
/// where the axis ends. The sound document's Lines pane draws with it.
pub fn axis_columns(axis_ms: u64, line_ms: u64, width: usize) -> usize {
    if axis_ms == 0 {
        return 0;
    }
    let cols = (u128::from(line_ms) * width as u128 / u128::from(axis_ms)) as usize;
    cols.min(width)
}

/// Where a row is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    /// Written, not asked for yet.
    Idle,
    Queued,
    Running,
    Done,
    Failed(String),
}

impl RowState {
    pub fn name(&self) -> &'static str {
        match self {
            RowState::Idle => "idle",
            RowState::Queued => "queued",
            RowState::Running => "running",
            RowState::Done => "done",
            RowState::Failed(_) => "failed",
        }
    }

    pub fn why(&self) -> Option<&str> {
        match self {
            RowState::Failed(w) => Some(w),
            _ => None,
        }
    }
}

/// One line of the batch.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: u64,
    pub text: String,
    /// Whose voice; `None` is the batch's default character.
    pub character: Option<String>,
    /// The output name (a clip id: lowercase letters, digits and -), or
    /// empty. Names the saved file and the checkout clip.
    pub clip: String,
    /// The `snd!` tag this line replaces in the edit target package, or
    /// `None`. A row with a tag is saved there, not under `art/announcer`.
    pub tag: Option<String>,
    pub state: RowState,
    pub said: Option<Said>,
    /// What the last save said.
    pub saved: Option<Result<String, String>>,
    /// Ask the model again (a re-run), instead of the cache.
    fresh: bool,
}

/// The batch.
#[derive(Debug, Default)]
pub struct Batch {
    pub rows: Vec<Row>,
    next_id: u64,
    /// The voice of a row with none of its own: a character id, or empty.
    pub default_character: String,
    /// A save may replace a clip the checkout has.
    pub replace: bool,
}

/// How many rows are in each state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub idle: usize,
    pub queued: usize,
    pub running: usize,
    pub done: usize,
    pub failed: usize,
}

impl Batch {
    /// Adds a row; its id.
    pub fn add(&mut self, text: &str, character: Option<&str>, clip: &str) -> u64 {
        self.next_id += 1;
        self.rows.push(Row {
            id: self.next_id,
            text: text.trim().to_owned(),
            character: character.filter(|c| !c.is_empty()).map(str::to_owned),
            clip: clip.trim().to_owned(),
            tag: None,
            state: RowState::Idle,
            said: None,
            saved: None,
            fresh: false,
        });
        self.next_id
    }

    /// One row per non-empty line of `text`; a tab splits an output name
    /// from the text (`name<TAB>text`). The rows added.
    pub fn paste(&mut self, text: &str, character: Option<&str>) -> usize {
        let mut n = 0;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match line.split_once('\t') {
                Some((name, t)) if !t.trim().is_empty() => self.add(t, character, name),
                _ => self.add(line, character, ""),
            };
            n += 1;
        }
        n
    }

    /// Adds the lines of the text file at `path`, as [`Batch::paste`].
    pub fn import(&mut self, path: &Path, character: Option<&str>) -> Result<usize, VoiceError> {
        let io = |e: std::io::Error| VoiceError::Io(format!("{}: {e}", path.display()));
        if std::fs::metadata(path).map_err(io)?.len() > MAX_IMPORT {
            return Err(VoiceError::Invalid(format!(
                "{}: a text file of at most 1 MB",
                path.display()
            )));
        }
        let text = std::fs::read_to_string(path).map_err(io)?;
        Ok(self.paste(&text, character))
    }

    pub fn remove(&mut self, id: u64) {
        self.rows.retain(|r| r.id != id || r.state == RowState::Running);
    }

    pub fn row(&self, id: u64) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Drops the rows that are done.
    pub fn clear_done(&mut self) {
        self.rows.retain(|r| r.state != RowState::Done);
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for r in &self.rows {
            match r.state {
                RowState::Idle => c.idle += 1,
                RowState::Queued => c.queued += 1,
                RowState::Running => c.running += 1,
                RowState::Done => c.done += 1,
                RowState::Failed(_) => c.failed += 1,
            }
        }
        c
    }

    /// Takes the rows that wait back to idle; the one being said ends.
    pub fn cancel(&mut self) {
        for r in &mut self.rows {
            if r.state == RowState::Queued {
                r.state = RowState::Idle;
            }
        }
    }

    /// Rows are waiting or being said.
    pub fn active(&self) -> bool {
        let c = self.counts();
        c.queued + c.running > 0
    }

    /// The character that says `r`.
    pub fn voice_of(&self, r: &Row) -> Option<String> {
        r.character
            .clone()
            .or_else(|| (!self.default_character.is_empty()).then(|| self.default_character.clone()))
    }

    /// Why `r` cannot be said, or `None`.
    pub fn problem(&self, r: &Row) -> Option<String> {
        if r.text.trim().is_empty() {
            return Some("no text".into());
        }
        if r.text.chars().count() > 500 {
            return Some("the text is over 500 characters".into());
        }
        if self.voice_of(r).is_none() {
            return Some("no voice: pick one for the batch or for this line".into());
        }
        if !r.clip.is_empty()
            && let Err(e) = super::announcer::check_clip_id(&r.clip)
        {
            return Some(e.to_string());
        }
        None
    }

    /// Why the window will not say `r` yet, or `None`: a problem of its
    /// own ([`Batch::problem`]), or no output name and no game line to
    /// replace, so nothing could name or place the result. The API's
    /// `queue` does not ask for a name; the Voices window does.
    pub fn blocked(&self, r: &Row) -> Option<String> {
        if let Some(p) = self.problem(r) {
            return Some(p);
        }
        if r.clip.is_empty() && r.tag.is_none() {
            return Some(
                "give the line an output name, or pick the game line it replaces".into(),
            );
        }
        None
    }

    /// Queues rows: with `ids`, those (a done row is asked again); else
    /// every idle or failed row. A row with a problem fails at once with
    /// it, and so does one whose voice and output name another row has.
    /// How many are queued.
    pub fn queue(&mut self, ids: Option<&[u64]>) -> usize {
        let pick: Vec<u64> = self
            .rows
            .iter()
            .filter(|r| match ids {
                Some(ids) => ids.contains(&r.id),
                None => matches!(r.state, RowState::Idle | RowState::Failed(_)),
            })
            .filter(|r| r.state != RowState::Running)
            .map(|r| r.id)
            .collect();
        for id in &pick {
            let i = self.rows.iter().position(|r| r.id == *id).expect("picked");
            let problem = self.problem(&self.rows[i]);
            let r = &mut self.rows[i];
            r.fresh = r.state == RowState::Done;
            r.saved = None;
            match problem {
                Some(p) => r.state = RowState::Failed(p),
                None => r.state = RowState::Queued,
            }
        }
        // One file per voice and output name.
        let mut seen: Vec<(String, String, u64)> = Vec::new();
        for i in 0..self.rows.len() {
            let r = &self.rows[i];
            if r.clip.is_empty() || matches!(r.state, RowState::Idle | RowState::Failed(_)) {
                continue;
            }
            let v = self.voice_of(r).unwrap_or_default();
            match seen.iter().find(|(sv, sc, _)| *sv == v && *sc == r.clip) {
                Some((_, _, first)) => {
                    let why = format!("the output name {} is line {first}'s too", r.clip);
                    if pick.contains(&r.id) {
                        self.rows[i].state = RowState::Failed(why);
                    }
                }
                None => seen.push((v, r.clip.clone(), r.id)),
            }
        }
        self.rows.iter().filter(|r| r.state == RowState::Queued).count()
    }

    /// A row, as the API gives it.
    pub fn row_json(&self, r: &Row) -> Value {
        let mut v = json!({
            "id": r.id,
            "text": r.text,
            "character": self.voice_of(r),
            "clip": r.clip,
            "tag": r.tag,
            "state": r.state.name(),
        });
        if let Some(w) = r.state.why() {
            v["why"] = json!(w);
        }
        if let Some(s) = &r.said {
            v["file"] = json!(s.file.display().to_string());
            v["cached"] = json!(s.cached);
            v["levels"] = json!({
                "lufs": s.levels.lufs,
                "peak_dbfs": s.levels.peak_dbfs,
                "duration_ms": s.levels.duration_ms,
            });
            v["verdict"] = json!({"level": s.verdict.level, "text": s.verdict.text});
        }
        match &r.saved {
            Some(Ok(to)) => v["saved"] = json!(to),
            Some(Err(w)) => v["save_failed"] = json!(w),
            None => {}
        }
        v
    }

    pub fn json(&self) -> Value {
        let c = self.counts();
        json!({
            "default_character": (!self.default_character.is_empty()).then_some(&self.default_character),
            "active": self.active(),
            "counts": {
                "idle": c.idle, "queued": c.queued, "running": c.running,
                "done": c.done, "failed": c.failed,
            },
            "rows": self.rows.iter().map(|r| self.row_json(r)).collect::<Vec<_>>(),
        })
    }
}

impl App {
    /// Moves the batch on by one step: a running row is said; else the
    /// next queued row starts running (so a frame shows it so before the
    /// model is asked). True while rows are left to do.
    pub fn poll_voice_batch(&mut self) -> bool {
        let b = &mut self.voices.batch;
        if let Some(i) = b.rows.iter().position(|r| r.state == RowState::Running) {
            let voice = b.voice_of(&b.rows[i]).unwrap_or_default();
            let r = &b.rows[i];
            let params = SayParams {
                text: r.text.clone(),
                voice: Some(voice),
                clip: (!r.clip.is_empty()).then(|| r.clip.clone()),
                engine: Engine::Clone,
                play: false,
                fresh: r.fresh,
                ..SayParams::default()
            };
            let said = self.voice_say(params);
            let r = &mut self.voices.batch.rows[i];
            r.fresh = false;
            match said {
                Ok(s) => {
                    r.said = Some(s);
                    r.state = RowState::Done;
                }
                Err(e) => {
                    r.said = None;
                    r.state = RowState::Failed(e.to_string());
                }
            }
        } else if let Some(r) = b.rows.iter_mut().find(|r| r.state == RowState::Queued) {
            r.state = RowState::Running;
        }
        self.voices.batch.active()
    }

    /// Says every queued row, now.
    pub fn voice_batch_run(&mut self) {
        while self.poll_voice_batch() {}
    }

    /// Plays a done row.
    pub fn voice_batch_play(&mut self, id: u64) -> Result<(), String> {
        let s = self
            .voices
            .batch
            .row(id)
            .and_then(|r| r.said.clone())
            .ok_or_else(|| format!("line {id} has not been said"))?;
        let clip = super::audition::Clip {
            rate: s.levels.rate,
            channels: 1,
            pcm: s.pcm.clone(),
        };
        self.sound_play_clip(&clip)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Why nothing could be saved, or `None` when some row can be.
    pub fn voice_batch_save_problem(&self) -> Option<String> {
        let b = &self.voices.batch;
        let done: Vec<&Row> = b.rows.iter().filter(|r| r.state == RowState::Done).collect();
        if done.is_empty() {
            return Some("no line is done yet".into());
        }
        if done.iter().all(|r| r.tag.is_none() && r.clip.is_empty()) {
            return Some("give the lines an output name (or a target sound) first".into());
        }
        None
    }

    /// Keeps the done rows: a row with a tag becomes a `sound` item of
    /// `package` (default the edit target); one with an output name is
    /// copied to `art/announcer/<name>.wav` of `repo` (default the
    /// checkout the editor runs in), replacing a clip only with the
    /// batch's `replace`. `ids` limits it. A row's result is its `saved`.
    pub fn voice_batch_save(
        &mut self,
        ids: Option<&[u64]>,
        repo: Option<&Path>,
        package: Option<&str>,
    ) -> Vec<(u64, Result<String, String>)> {
        let replace = self.voices.batch.replace;
        let todo: Vec<(u64, PathBuf, String, Option<String>)> = self
            .voices
            .batch
            .rows
            .iter()
            .filter(|r| r.state == RowState::Done && ids.is_none_or(|i| i.contains(&r.id)))
            .filter_map(|r| {
                r.said
                    .as_ref()
                    .map(|s| (r.id, s.file.clone(), r.clip.clone(), r.tag.clone()))
            })
            .collect();
        let mut out = Vec::new();
        for (id, file, clip, tag) in todo {
            let r: Result<String, String> = if let Some(tag) = tag {
                match package.map(str::to_owned).or_else(|| self.edit_target()) {
                    None => Err(super::edit_target::NO_TARGET.to_owned()),
                    Some(p) => self
                        .voice_commit_sound(&tag, Some(&file), &p, None)
                        .map(|_| format!("{tag} in {p}"))
                        .map_err(|e| e.to_string()),
                }
            } else if clip.is_empty() {
                Err("no output name and no target sound".into())
            } else {
                self.voice_commit(&clip, Some(&file), repo, replace)
                    .map(|(to, _)| to.display().to_string())
                    .map_err(|e| e.to_string())
            };
            if let Some(row) = self.voices.batch.rows.iter_mut().find(|x| x.id == id) {
                row.saved = Some(r.clone());
            }
            out.push((id, r));
        }
        out
    }
}
