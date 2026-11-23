//! Image generation: a picture made from a prompt and reference pictures,
//! fitted to a bitmap slot and written through the Textures tool's edit
//! path (docs/editor/agent-api.md, "Generating images";
//! docs/editor/editor-guide.md, "Generating an image").
//!
//! - [`ImageSource`]: what makes the picture. [`codex::CodexCli`] drives
//!   the Codex CLI on the user's ChatGPT sign-in; [`fake::FakeSource`]
//!   draws a picture of its own for tests. `H2_IMAGEGEN` picks one
//!   ([`source_from_env`]). An OpenAI API key source (the key in the
//!   Keychain, service `openai`) plugs in as another implementation.
//! - [`ImageGen`]: the jobs. A generation takes minutes, so
//!   [`ImageGen::start`] runs it on a thread of its own and answers with
//!   an id at once; [`ImageGen::poll`] collects what finished. A job
//!   writes the generator's picture, the picture fitted to the slot
//!   ([`fit`]) and a provenance file into its own directory in the cache.
//! - The App side (`app::image_gen`): the slot and the references read
//!   from the open map, and apply, accept and discard, which go through
//!   `Action::Bitmap` like any other texture edit.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tags::PixelFormat;

pub mod codex;
pub mod fake;
pub mod fit;

#[cfg(test)]
pub(crate) mod tests;

pub use fit::Alpha;

/// The environment variable that picks the image source: `codex` (the
/// default) or `fake`.
pub const SOURCE_ENV: &str = "H2_IMAGEGEN";

/// Jobs kept for `image.result`.
pub const KEEP: usize = 16;

/// What an [`ImageSource`] is asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct GenRequest {
    /// The whole prompt: the user's words and the slot's needs
    /// ([`compose_prompt`]).
    pub prompt: String,
    /// PNG files the generator sees as style references, in order.
    pub references: Vec<PathBuf>,
    /// The size wanted, when there is one: the generator picks its
    /// nearest; [`fit`] makes it exact.
    pub size: Option<(u32, u32)>,
    /// A directory of the job's own the source may write into.
    pub work_dir: PathBuf,
}

/// What a source made.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    /// The picture, a PNG file.
    pub png: PathBuf,
    /// The generator, as provenance names it (`codex-cli 0.170.0`).
    pub generator: String,
    /// The prompt as the generator rewrote it, when it says.
    pub revised_prompt: Option<String>,
}

/// Why a source made nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenError {
    /// The generator is not installed.
    Missing(String),
    /// The generator is installed but not signed in.
    Unauthenticated(String),
    /// The account is out of its allowance.
    Quota(String),
    /// It ran and made no picture.
    NoImage(String),
    /// Anything else.
    Failed(String),
}

impl GenError {
    /// `missing`, `unauthenticated`, `quota`, `no_image` or `failed`.
    pub fn kind(&self) -> &'static str {
        match self {
            GenError::Missing(_) => "missing",
            GenError::Unauthenticated(_) => "unauthenticated",
            GenError::Quota(_) => "quota",
            GenError::NoImage(_) => "no_image",
            GenError::Failed(_) => "failed",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            GenError::Missing(m)
            | GenError::Unauthenticated(m)
            | GenError::Quota(m)
            | GenError::NoImage(m)
            | GenError::Failed(m) => m,
        }
    }
}

impl std::fmt::Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// Makes a picture from a prompt and references.
pub trait ImageSource: Send + Sync {
    /// The source's short name: `codex`, `fake`.
    fn name(&self) -> &'static str;
    /// Makes one picture. Blocks for as long as the generator takes; a
    /// job calls it on a thread of its own.
    fn generate(&self, req: &GenRequest) -> Result<Generated, GenError>;
}

/// The source `H2_IMAGEGEN` names: `fake`, else the Codex CLI.
pub fn source_from_env() -> Arc<dyn ImageSource> {
    match std::env::var(SOURCE_ENV).as_deref() {
        Ok("fake") => Arc::new(fake::FakeSource::new()),
        _ => Arc::new(codex::CodexCli::default()),
    }
}

/// The bitmap image a picture is made for.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub tag: map::TagId,
    /// The bitmap tag's path.
    pub bitmap: String,
    pub image: usize,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

impl Slot {
    pub fn json(&self) -> Value {
        json!({
            "bitmap": format!("{:#010x}", self.tag.0),
            "name": self.bitmap,
            "image": self.image,
            "width": self.width,
            "height": self.height,
            "format": format!("{:?}", self.format),
        })
    }
}

/// A reference picture and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    /// The PNG the generator sees.
    pub path: PathBuf,
    /// `file`: a PNG the user named. `map`: an image of the open map's
    /// bitmap the user picked, exported into the job's directory.
    pub origin: RefOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RefOrigin {
    File,
    Map { bitmap: String, image: usize },
}

impl Reference {
    pub fn json(&self) -> Value {
        match &self.origin {
            RefOrigin::File => json!({"path": self.path.display().to_string(), "origin": "file"}),
            RefOrigin::Map { bitmap, image } => json!({
                "path": self.path.display().to_string(),
                "origin": "map",
                "bitmap": bitmap,
                "image": image,
            }),
        }
    }
}

/// One generation, as asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// The user's words.
    pub prompt: String,
    pub references: Vec<Reference>,
    /// The slot the picture is fitted to, when there is one.
    pub slot: Option<Slot>,
    /// The size without a slot: `[w, h]`, fitted as 32-bit RGBA. With
    /// neither, the generator's picture is kept as it came.
    pub size: Option<(u32, u32)>,
    pub alpha: Alpha,
    /// Where the fitted PNG goes besides the job's directory.
    pub out: Option<PathBuf>,
    /// The job this one regenerates or tweaks.
    pub from: Option<u64>,
}

impl Spec {
    /// The target size: the slot's, else `size`.
    pub fn target(&self) -> Option<(u32, u32)> {
        self.slot
            .as_ref()
            .map(|s| (s.width, s.height))
            .or(self.size)
    }
}

/// The prompt a source is given: the user's words, then what the slot
/// needs (its size, a transparent background when it keeps alpha) and how
/// the references are meant.
pub fn compose_prompt(spec: &Spec) -> String {
    let mut out = spec.prompt.trim().to_owned();
    match spec.references.len() {
        0 => {}
        1 => out.push_str(
            "\n\nThe attached image is a style reference: match its art style, palette, line weight, lighting and framing, so the new image sits beside it as one set. Do not copy its subject.",
        ),
        n => out.push_str(&format!(
            "\n\nThe {n} attached images are style references: match their art style, palette, line weight, lighting and framing, so the new image sits beside them as one set. Do not copy their subjects."
        )),
    }
    if let Some((w, h)) = spec.target() {
        out.push_str(&format!(
            "\n\nIt is shown at {w} by {h} pixels: keep the shapes bold and readable at that size, centred, filling the frame with a small margin."
        ));
    }
    let alpha = spec.slot.as_ref().is_none_or(|s| fit::has_alpha(s.format));
    if alpha && spec.alpha != Alpha::Opaque {
        out.push_str("\n\nUse a transparent background.");
    }
    out
}

/// What a finished job made.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// The generator's picture, copied into the job's directory.
    pub raw: PathBuf,
    pub raw_size: (u32, u32),
    /// The picture fitted to the slot or size (the raw picture without
    /// either).
    pub fitted: PathBuf,
    pub fitted_size: (u32, u32),
    /// The alpha used.
    pub alpha: Alpha,
    pub generator: String,
    pub revised_prompt: Option<String>,
    /// The prompt the source was given.
    pub sent_prompt: String,
    /// The fitted PNG's copy at the spec's `out`.
    pub out: Option<PathBuf>,
}

/// The fitted picture written into its slot (`image.apply`).
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub tag: map::TagId,
    pub image: usize,
    /// The bitmap's revision just after the import: a discard undoes the
    /// import while the bitmap is still at it.
    pub rev: u64,
    /// The slot's pixels before, as a PNG: a discard after later edits
    /// imports these back.
    pub original: PathBuf,
}

enum State {
    Running(mpsc::Receiver<Result<Outcome, GenError>>),
    Done(Result<Outcome, GenError>),
}

/// One generation.
pub struct Job {
    pub id: u64,
    pub spec: Spec,
    /// The job's directory: the raw and fitted pictures, exported
    /// references, the slot's original pixels and `provenance.json`.
    pub dir: PathBuf,
    pub generator: &'static str,
    started: Instant,
    state: State,
    pub applied: Option<Applied>,
    /// Where `image.accept` saved the art, once accepted.
    pub accepted: Option<Vec<PathBuf>>,
    pub discarded: bool,
}

impl Job {
    pub fn state_name(&self) -> &'static str {
        match &self.state {
            State::Running(_) => "running",
            State::Done(Err(_)) => "failed",
            State::Done(Ok(_)) => "done",
        }
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        match &self.state {
            State::Done(Ok(o)) => Some(o),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&GenError> {
        match &self.state {
            State::Done(Err(e)) => Some(e),
            _ => None,
        }
    }

    /// `image.result`'s answer.
    pub fn report(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "state": self.state_name(),
            "generator": self.generator,
            "elapsed_ms": self.started.elapsed().as_millis() as u64,
            "prompt": self.spec.prompt,
            "references": self.spec.references.iter().map(Reference::json).collect::<Vec<_>>(),
            "slot": self.spec.slot.as_ref().map(Slot::json),
            "size": self.spec.target().map(|(w, h)| json!([w, h])),
            "dir": self.dir.display().to_string(),
            "from": self.spec.from,
            "applied": self.applied.is_some(),
            "accepted": self.accepted.as_ref().map(|p| p.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()),
            "discarded": self.discarded,
        });
        if let Some(e) = self.error() {
            v["why"] = json!(e.message());
            v["error"] = json!(e.kind());
        }
        if let Some(o) = self.outcome() {
            v["generator"] = json!(o.generator);
            v["raw"] = json!({"path": o.raw.display().to_string(), "width": o.raw_size.0, "height": o.raw_size.1});
            v["fitted"] = json!({
                "path": o.fitted.display().to_string(),
                "width": o.fitted_size.0,
                "height": o.fitted_size.1,
                "format": self.spec.slot.as_ref().map(|s| format!("{:?}", s.format)),
                "alpha": o.alpha.name(),
            });
            v["revised_prompt"] = json!(o.revised_prompt);
            v["out"] = json!(o.out.as_ref().map(|p| p.display().to_string()));
        }
        v
    }

    /// The provenance record: the prompt, the references, the generator,
    /// the slot and what became of the picture.
    pub fn provenance(&self) -> Value {
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let o = self.outcome();
        json!({
            "kind": "h2recomp generated image",
            "note": "Original AI-made art. The references are the images the user picked in Studio.",
            "generator": o.map_or(self.generator.to_owned(), |o| o.generator.clone()),
            "prompt": self.spec.prompt,
            "sent_prompt": o.map(|o| o.sent_prompt.clone()),
            "revised_prompt": o.and_then(|o| o.revised_prompt.clone()),
            "references": self.spec.references.iter().map(Reference::json).collect::<Vec<_>>(),
            "slot": self.spec.slot.as_ref().map(Slot::json),
            "alpha": o.map(|o| o.alpha.name()),
            "from": self.spec.from,
            "studio_job": self.id,
            "recorded_unix": created,
            "accepted": self.accepted.is_some(),
        })
    }

    /// Writes [`Job::provenance`] as `provenance.json` in the job's
    /// directory.
    pub fn write_provenance(&self) -> Result<PathBuf, String> {
        let path = self.dir.join("provenance.json");
        let text = serde_json::to_string_pretty(&self.provenance()).map_err(|e| e.to_string())?;
        std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }
}

/// The jobs and the source that runs them.
pub struct ImageGen {
    source: Arc<dyn ImageSource>,
    /// Where the jobs' directories go.
    root: PathBuf,
    jobs: Vec<Job>,
    next: u64,
}

impl Default for ImageGen {
    fn default() -> Self {
        ImageGen::new(
            source_from_env(),
            agent_proto::cache_dir().join("studio").join("imagegen"),
        )
    }
}

impl ImageGen {
    pub fn new(source: Arc<dyn ImageSource>, root: PathBuf) -> Self {
        ImageGen {
            source,
            root,
            jobs: Vec::new(),
            next: 0,
        }
    }

    /// Replaces the source (tests, `H2_IMAGEGEN`).
    pub fn set_source(&mut self, source: Arc<dyn ImageSource>) {
        self.source = source;
    }

    pub fn set_root(&mut self, root: PathBuf) {
        self.root = root;
    }

    pub fn source_name(&self) -> &'static str {
        self.source.name()
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn job(&self, id: u64) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn job_mut(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn last(&self) -> Option<&Job> {
        self.jobs.last()
    }

    /// The id the next job takes, and its directory.
    pub fn next_dir(&self) -> (u64, PathBuf) {
        let id = self.next + 1;
        (id, self.root.join(format!("{}-{id}", std::process::id())))
    }

    /// Starts `spec` on a thread of its own; the job's id. One job runs
    /// at a time.
    pub fn start(&mut self, spec: Spec) -> Result<u64, String> {
        self.poll();
        if let Some(busy) = self.jobs.iter().find(|j| matches!(j.state, State::Running(_))) {
            return Err(format!(
                "image {} is still being generated: wait for it (image.result)",
                busy.id
            ));
        }
        if spec.prompt.trim().is_empty() {
            return Err("the prompt is empty".into());
        }
        let (id, dir) = self.next_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        self.next = id;
        let (tx, rx) = mpsc::channel();
        let source = Arc::clone(&self.source);
        let job_spec = spec.clone();
        let job_dir = dir.clone();
        std::thread::Builder::new()
            .name(format!("imagegen-{id}"))
            .spawn(move || {
                let _ = tx.send(run(&*source, &job_spec, &job_dir));
            })
            .map_err(|e| format!("could not start the job: {e}"))?;
        self.jobs.push(Job {
            id,
            spec,
            dir,
            generator: self.source.name(),
            started: Instant::now(),
            state: State::Running(rx),
            applied: None,
            accepted: None,
            discarded: false,
        });
        if self.jobs.len() > KEEP {
            self.jobs.remove(0);
        }
        Ok(id)
    }

    /// Collects every finished job; true when one finished.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        for job in &mut self.jobs {
            if let State::Running(rx) = &job.state {
                let done = match rx.try_recv() {
                    Ok(r) => Some(r),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err(GenError::Failed(
                        "the generation's thread stopped without an answer".into(),
                    ))),
                };
                if let Some(r) = done {
                    job.state = State::Done(r);
                    let _ = job.write_provenance();
                    any = true;
                }
            }
        }
        any
    }

    /// Whether any job is still running.
    pub fn busy(&self) -> bool {
        self.jobs.iter().any(|j| matches!(j.state, State::Running(_)))
    }

    /// Waits up to `timeout` for job `id` to finish (tests and scripts).
    pub fn wait(&mut self, id: u64, timeout: std::time::Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll();
            match self.job(id) {
                Some(j) if !matches!(j.state, State::Running(_)) => return true,
                None => return false,
                _ => {}
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// One job's work: generate, keep the picture, fit it, write it.
fn run(source: &dyn ImageSource, spec: &Spec, dir: &Path) -> Result<Outcome, GenError> {
    let sent_prompt = compose_prompt(spec);
    let req = GenRequest {
        prompt: sent_prompt.clone(),
        references: spec.references.iter().map(|r| r.path.clone()).collect(),
        size: spec.target(),
        work_dir: dir.to_owned(),
    };
    let made = source.generate(&req)?;
    let bytes = std::fs::read(&made.png)
        .map_err(|e| GenError::NoImage(format!("{}: {e}", made.png.display())))?;
    let (sw, sh, rgba) = bitmap_edit::decode_png(&bytes).map_err(|e| {
        GenError::NoImage(format!("{} is not a PNG Studio reads: {e}", made.png.display()))
    })?;
    let raw = dir.join("generated.png");
    if made.png != raw {
        std::fs::write(&raw, &bytes).map_err(|e| GenError::Failed(format!("{}: {e}", raw.display())))?;
    }
    let fitted = match spec.target() {
        Some((w, h)) => fit::fit(
            &rgba,
            sw,
            sh,
            w,
            h,
            spec.slot.as_ref().map(|s| s.format),
            spec.alpha,
        )
        .map_err(GenError::Failed)?,
        None => fit::Fitted {
            width: sw,
            height: sh,
            rgba,
            alpha: Alpha::Image,
        },
    };
    let png = bitmap_edit::encode_png(fitted.width, fitted.height, &fitted.rgba)
        .map_err(|e| GenError::Failed(e.to_string()))?;
    let fitted_path = dir.join("fitted.png");
    std::fs::write(&fitted_path, &png)
        .map_err(|e| GenError::Failed(format!("{}: {e}", fitted_path.display())))?;
    if let Some(out) = &spec.out {
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(out, &png).map_err(|e| GenError::Failed(format!("{}: {e}", out.display())))?;
    }
    Ok(Outcome {
        raw,
        raw_size: (sw, sh),
        fitted: fitted_path,
        fitted_size: (fitted.width, fitted.height),
        alpha: fitted.alpha,
        generator: made.generator,
        revised_prompt: made.revised_prompt,
        sent_prompt,
        out: spec.out.clone(),
    })
}
