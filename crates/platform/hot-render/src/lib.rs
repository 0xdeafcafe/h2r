//! Hot iteration on the host renderer (docs/graphics/hot-iteration.md).
//!
//! The [`Hub`] is where the agent API (`render.*`) and the renderer meet,
//! without either knowing the other:
//!
//! * **Overrides.** A directory of WGSL files named after the translated
//!   program they replace (`<program id, 16 hex digits>[_note].wgsl`; the
//!   id is `gfx::pipeline_store::program_id`, the same one the pipeline
//!   cache records). [`Hub::reload`] reads them; [`watch`] does it when
//!   the directory changes. The shader provider polls [`Hub::generation`]
//!   once a frame and, when it moved, swaps each program's module for its
//!   override (or back), after checking the override fits the program's
//!   layout ([`iface::check`]). Re-registering a program drops the cached
//!   pipelines, so the next draw builds new ones.
//! * **Parameters.** `{{name=default}}` in an override is replaced by the
//!   parameter's value ([`params`]); [`Hub::set_param`] recompiles the
//!   overrides that use it. Host passes can declare their own and read
//!   them each frame ([`Hub::declare_param`], [`Hub::param`]).
//! * **Toggles.** Named switches host code declares and reads
//!   ([`Hub::declare_toggle`], [`Hub::toggle`]); `overrides` is built in
//!   and switches every override off at once, to compare.
//! * **Programs and dumps.** The provider publishes the programs it has
//!   ([`Hub::publish`]) and writes a program's translated WGSL on request
//!   ([`Hub::request_dump`]), the starting point for an override.
//!
//! Nothing here touches the GPU: this crate builds and tests without one.

pub mod iface;
pub mod params;
mod watch;

pub use watch::{signature, watch};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Serialize;

/// The built-in toggle that switches every override off (`false`) or on.
pub const OVERRIDES: &str = "overrides";

/// The process's hub: the one the renderer and the agent API share.
pub fn global() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(Hub::new)
}

/// A program id as written in file names and the API: 16 hex digits.
pub fn program_text(id: u64) -> String {
    format!("{id:016x}")
}

/// A program id from 16 hex digits (an optional `0x` first).
pub fn parse_program(s: &str) -> Option<u64> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    (h.len() == 16)
        .then(|| u64::from_str_radix(h, 16).ok())
        .flatten()
}

/// The program an override file is for, from its name
/// (`<16 hex digits>.wgsl` or `<16 hex digits>_<anything>.wgsl`).
pub fn program_of_file(name: &str) -> Option<u64> {
    let stem = name.strip_suffix(".wgsl")?;
    let (id, rest) = stem.split_at_checked(16)?;
    (rest.is_empty() || rest.starts_with('_'))
        .then(|| u64::from_str_radix(id, 16).ok())
        .flatten()
}

fn fnv(text: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in text.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    // 0 means "the translation" to providers.
    h.max(1)
}

/// What became of an override in a provider.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Applied {
    /// Loaded; no provider has the program (yet), or it has not looked.
    Pending,
    /// Compiled and registered: draws of the program run it.
    Applied,
    /// The provider refused it and kept the translation.
    Rejected { error: String },
}

/// A file the reload could not take.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    pub file: String,
    pub error: String,
}

/// A loaded override.
#[derive(Clone, Debug)]
struct Override {
    file: PathBuf,
    text: String,
    params: Vec<String>,
    applied: Applied,
}

/// What a program is, as its provider publishes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProgramInfo {
    /// The renderer's shader id.
    pub shader: u32,
    /// `microcode` (a guest vertex program) or `pretransformed` (the
    /// synthesized one for `XYZRHW` vertices).
    pub vertex: String,
    /// Texture stages the pixel side samples.
    pub textures: u8,
    /// Whether the pixel side may discard.
    pub discards: bool,
    /// Translator warnings (approximations).
    pub warnings: usize,
}

/// A parameter.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamInfo {
    pub name: String,
    pub value: f32,
    pub default: f32,
    /// What it does (host parameters), or the override files that use it.
    pub about: String,
    /// Set through the API (survives reloads) rather than defaulted.
    pub set: bool,
}

/// A toggle.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToggleInfo {
    pub name: String,
    pub on: bool,
    pub default: bool,
    pub about: String,
}

/// A request to write translated programs' WGSL.
#[derive(Clone, Debug, PartialEq)]
pub struct DumpRequest {
    /// One program, or every program the provider has.
    pub program: Option<u64>,
    /// The directory to write `<id>.wgsl` into.
    pub to: PathBuf,
    /// Frames it has gone unserved (providers re-queue a program they
    /// don't have, for another to take).
    pub tries: u32,
}

/// What a provider did with a [`DumpRequest`].
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DumpReport {
    pub dir: String,
    pub written: Vec<String>,
    pub errors: Vec<Problem>,
}

/// [`Hub::reload`]'s answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ReloadReport {
    pub dir: Option<String>,
    /// Program ids with an override now.
    pub loaded: Vec<String>,
    /// Added, changed or removed since the last reload.
    pub changed: Vec<String>,
    pub problems: Vec<Problem>,
    /// [`Hub::generation`] after it.
    pub generation: u64,
}

/// One override in [`Status`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverrideStatus {
    pub program: String,
    pub file: String,
    pub params: Vec<String>,
    #[serde(flatten)]
    pub applied: Applied,
}

/// `render.overrides`'s answer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Status {
    pub dir: Option<String>,
    pub watching: bool,
    pub enabled: bool,
    pub generation: u64,
    pub overrides: Vec<OverrideStatus>,
    pub problems: Vec<Problem>,
    /// Programs the renderer has translated.
    pub programs: usize,
    pub last_dump: Option<DumpReport>,
}

#[derive(Default)]
struct Inner {
    dir: Option<PathBuf>,
    overrides: BTreeMap<u64, Override>,
    problems: Vec<Problem>,
    params: BTreeMap<String, ParamInfo>,
    toggles: BTreeMap<String, ToggleInfo>,
    programs: BTreeMap<u64, ProgramInfo>,
    dumps: Vec<DumpRequest>,
    last_dump: Option<DumpReport>,
}

/// Hot-iteration state shared by the API and the renderer (crate docs).
pub struct Hub {
    generation: AtomicU64,
    /// Overrides are loaded and the toggle is on: providers have work.
    active: AtomicBool,
    dumps: AtomicBool,
    watching: AtomicBool,
    inner: Mutex<Inner>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    /// An empty hub (tests; the process uses [`global`]).
    pub fn new() -> Hub {
        let mut inner = Inner::default();
        inner.toggles.insert(
            OVERRIDES.into(),
            ToggleInfo {
                name: OVERRIDES.into(),
                on: true,
                default: true,
                about: "use the WGSL overrides (off: every program runs its translation)".into(),
            },
        );
        Hub {
            generation: AtomicU64::new(0),
            active: AtomicBool::new(false),
            dumps: AtomicBool::new(false),
            watching: AtomicBool::new(false),
            inner: Mutex::new(inner),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bumped whenever what a program should run may have changed: a
    /// reload that changed something, a parameter an override uses, the
    /// `overrides` toggle. Cheap: providers read it once a frame.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn bump(&self, inner: &Inner) -> u64 {
        let on = inner.toggles.get(OVERRIDES).is_none_or(|t| t.on);
        self.active
            .store(on && !inner.overrides.is_empty(), Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Whether any override is loaded and switched on.
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Where overrides come from (`None`: nowhere). Takes effect at the
    /// next [`reload`](Self::reload).
    pub fn set_dir(&self, dir: Option<PathBuf>) {
        self.lock().dir = dir;
    }

    pub fn dir(&self) -> Option<PathBuf> {
        self.lock().dir.clone()
    }

    pub(crate) fn set_watching(&self, on: bool) {
        self.watching.store(on, Ordering::Release);
    }

    /// Reads every override in the directory, replacing what was loaded.
    /// A file that can't be read or whose parameters don't parse is a
    /// problem, not an override; one that doesn't parse as WGSL too (the
    /// provider checks the rest against the program).
    pub fn reload(&self) -> ReloadReport {
        let dir = self.dir();
        let mut found: BTreeMap<u64, Override> = BTreeMap::new();
        let mut problems = Vec::new();
        if let Some(dir) = &dir {
            match std::fs::read_dir(dir) {
                Ok(entries) => {
                    let mut files: Vec<PathBuf> = entries
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().is_some_and(|x| x == "wgsl"))
                        .collect();
                    files.sort();
                    for path in files {
                        load_file(&path, &mut found, &mut problems);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => problems.push(Problem {
                    file: dir.display().to_string(),
                    error: e.to_string(),
                }),
            }
        }
        let mut inner = self.lock();
        let mut changed = Vec::new();
        for (id, o) in &found {
            if inner.overrides.get(id).is_none_or(|old| old.text != o.text) {
                changed.push(program_text(*id));
            }
        }
        for id in inner.overrides.keys() {
            if !found.contains_key(id) {
                changed.push(program_text(*id));
            }
        }
        // Parameters the files declare, keeping values set through the API.
        let mut declared: BTreeMap<String, (Option<f32>, Vec<String>)> = BTreeMap::new();
        for o in found.values() {
            let file = file_name(&o.file);
            if let Ok(uses) = params::scan(&o.text) {
                for u in uses {
                    let d = declared.entry(u.name).or_default();
                    d.0 = d.0.or(u.default);
                    if !d.1.contains(&file) {
                        d.1.push(file.clone());
                    }
                }
            }
        }
        for (name, (default, files)) in declared {
            let default = default.unwrap_or(0.0);
            let about = format!("override parameter ({})", files.join(", "));
            let p = inner.params.entry(name.clone()).or_insert(ParamInfo {
                name,
                value: default,
                default,
                about: about.clone(),
                set: false,
            });
            if p.about.starts_with("override parameter") {
                p.about = about;
                p.default = default;
                if !p.set {
                    p.value = default;
                }
            }
        }
        // Keep what providers said of overrides that didn't change.
        for (id, o) in found.iter_mut() {
            if let Some(old) = inner.overrides.get(id)
                && old.text == o.text
            {
                o.applied = old.applied.clone();
            }
        }
        inner.overrides = found;
        inner.problems = problems.clone();
        // Always bumped: a provider that rejected an override retries it
        // (the file may be the same but the fault was elsewhere).
        let generation = self.bump(&inner);
        ReloadReport {
            dir: dir.map(|d| d.display().to_string()),
            loaded: inner.overrides.keys().map(|id| program_text(*id)).collect(),
            changed,
            problems,
            generation,
        }
    }

    /// The source `program` should run now, with parameters substituted,
    /// and its hash (never 0); `None`: its translation.
    pub fn source(&self, program: u64) -> Option<(String, u64)> {
        if !self.active() {
            return None;
        }
        let inner = self.lock();
        let o = inner.overrides.get(&program)?;
        let text = params::substitute(&o.text, |n| inner.params.get(n).map(|p| p.value)).ok()?;
        let hash = fnv(&text);
        Some((text, hash))
    }

    /// A provider's verdict on `program`'s override.
    pub fn report(&self, program: u64, result: Result<(), String>) {
        let mut inner = self.lock();
        if let Some(o) = inner.overrides.get_mut(&program) {
            o.applied = match result {
                Ok(()) => Applied::Applied,
                Err(error) => Applied::Rejected { error },
            };
        }
    }

    /// A provider has `program` (registered as `info.shader`).
    pub fn publish(&self, program: u64, info: ProgramInfo) {
        self.lock().programs.insert(program, info);
    }

    /// Every program providers have published, by id.
    pub fn programs(&self) -> Vec<(String, ProgramInfo)> {
        let inner = self.lock();
        inner
            .programs
            .iter()
            .map(|(id, p)| (program_text(*id), p.clone()))
            .collect()
    }

    /// Whether `program` has been published.
    pub fn has_program(&self, program: u64) -> bool {
        self.lock().programs.contains_key(&program)
    }

    pub fn status(&self) -> Status {
        let inner = self.lock();
        Status {
            dir: inner.dir.as_ref().map(|d| d.display().to_string()),
            watching: self.watching.load(Ordering::Acquire),
            enabled: inner.toggles.get(OVERRIDES).is_none_or(|t| t.on),
            generation: self.generation(),
            overrides: inner
                .overrides
                .iter()
                .map(|(id, o)| OverrideStatus {
                    program: program_text(*id),
                    file: file_name(&o.file),
                    params: o.params.clone(),
                    applied: o.applied.clone(),
                })
                .collect(),
            problems: inner.problems.clone(),
            programs: inner.programs.len(),
            last_dump: inner.last_dump.clone(),
        }
    }

    // ---- parameters ----

    /// Declares a host parameter (a native pass's knob). An existing one
    /// keeps its value.
    pub fn declare_param(&self, name: &str, default: f32, about: &str) {
        let mut inner = self.lock();
        inner.params.entry(name.into()).or_insert(ParamInfo {
            name: name.into(),
            value: default,
            default,
            about: about.into(),
            set: false,
        });
    }

    /// A parameter's value now.
    pub fn param(&self, name: &str) -> Option<f32> {
        self.lock().params.get(name).map(|p| p.value)
    }

    pub fn params(&self) -> Vec<ParamInfo> {
        self.lock().params.values().cloned().collect()
    }

    /// Sets a parameter (`None`: back to its default). Overrides that use
    /// it are recompiled at the next frame.
    pub fn set_param(&self, name: &str, value: Option<f32>) -> Result<ParamInfo, String> {
        if let Some(v) = value
            && !v.is_finite()
        {
            return Err(format!("{name}: {v} is not finite"));
        }
        let mut inner = self.lock();
        let p = inner
            .params
            .get_mut(name)
            .ok_or_else(|| format!("no parameter {name:?} (render.param lists them)"))?;
        p.value = value.unwrap_or(p.default);
        p.set = value.is_some();
        let p = p.clone();
        let used = inner
            .overrides
            .values()
            .any(|o| o.params.iter().any(|n| n == name));
        if used {
            self.bump(&inner);
        }
        Ok(p)
    }

    // ---- toggles ----

    /// Declares a host toggle. An existing one keeps its state.
    pub fn declare_toggle(&self, name: &str, default: bool, about: &str) {
        let mut inner = self.lock();
        inner.toggles.entry(name.into()).or_insert(ToggleInfo {
            name: name.into(),
            on: default,
            default,
            about: about.into(),
        });
    }

    /// A toggle's state now.
    pub fn toggle(&self, name: &str) -> Option<bool> {
        self.lock().toggles.get(name).map(|t| t.on)
    }

    pub fn toggles(&self) -> Vec<ToggleInfo> {
        self.lock().toggles.values().cloned().collect()
    }

    /// Sets a toggle (`None`: back to its default).
    pub fn set_toggle(&self, name: &str, on: Option<bool>) -> Result<ToggleInfo, String> {
        let mut inner = self.lock();
        let t = inner
            .toggles
            .get_mut(name)
            .ok_or_else(|| format!("no toggle {name:?} (render.toggle lists them)"))?;
        t.on = on.unwrap_or(t.default);
        let t = t.clone();
        self.bump(&inner);
        Ok(t)
    }

    // ---- dumps ----

    /// Asks the provider to write translated WGSL (served at its next frame).
    pub fn request_dump(&self, req: DumpRequest) {
        self.lock().dumps.push(req);
        self.dumps.store(true, Ordering::Release);
    }

    /// Whether a dump is waiting. Cheap.
    pub fn dumps_pending(&self) -> bool {
        self.dumps.load(Ordering::Acquire)
    }

    /// The dumps waiting, for a provider to serve.
    pub fn take_dumps(&self) -> Vec<DumpRequest> {
        let mut inner = self.lock();
        self.dumps.store(false, Ordering::Release);
        std::mem::take(&mut inner.dumps)
    }

    /// A provider served a dump.
    pub fn dump_done(&self, report: DumpReport) {
        self.lock().last_dump = Some(report);
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn load_file(path: &Path, found: &mut BTreeMap<u64, Override>, problems: &mut Vec<Problem>) {
    let name = file_name(path);
    let problem = |error: String| Problem {
        file: name.clone(),
        error,
    };
    let Some(id) = program_of_file(&name) else {
        problems.push(problem(
            "not named <program id, 16 hex digits>[_note].wgsl; ignored".into(),
        ));
        return;
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return problems.push(problem(e.to_string())),
    };
    let uses = match params::scan(&text) {
        Ok(u) => u,
        Err(e) => return problems.push(problem(e)),
    };
    // Parse it now (with defaults) so a typo is reported by the reload.
    match params::substitute(&text, |_| None) {
        Ok(s) => {
            if let Err(e) = shaders::validate(&s) {
                return problems.push(problem(e.to_string()));
            }
        }
        Err(e) => return problems.push(problem(e)),
    }
    if let Some(prev) = found.get(&id) {
        return problems.push(problem(format!(
            "a second override of {} ({} is used)",
            program_text(id),
            file_name(&prev.file)
        )));
    }
    let mut params: Vec<String> = uses.into_iter().map(|u| u.name).collect();
    params.sort();
    params.dedup();
    found.insert(
        id,
        Override {
            file: path.to_owned(),
            text,
            params,
            applied: Applied::Pending,
        },
    );
}

#[cfg(test)]
mod tests;
