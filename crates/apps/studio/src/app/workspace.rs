//! Studio's side of the tag workspace (docs/editor/tag-workspace.md):
//! where the tags tree lives, the plan that keeps it current with the
//! disc, extraction on a worker thread (the first start's every map, and
//! the stale parts of a later start in the background), the sizes the
//! status line and `workspace.status` account, and prune. The start page
//! (`ui/workspace.rs`) and the agent API's `workspace.*`
//! (`api/workspace.rs`) are this state's two views.
//!
//! The tree is the extractor's cache (docs/formats/tag-repo.md): game
//! data under the data root (the `tags_root` setting, `<data>/tags/`
//! unless set), never inside a Git checkout, and nothing of it is
//! committed. It is read-only to every tool but the extractor, and is
//! never pruned in part: it is deleted whole and extracted again.
use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tags::repo::extract::{MapWork, Plan, Why};
use tags::repo::store::PutReport;
use tags::repo::store::Manifest;
use tags::repo::TagRepo;
use tags::schema::Schema;

use crate::App;

/// A build cache is unused after this long
/// (docs/editor/tag-workspace.md, "Size and cleanup").
const UNUSED_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The workspace state in the [`App`]: the tree's root and disc, the plan
/// as last computed, the extraction running (or waiting to be polled) and
/// what the last one did.
#[derive(Default)]
pub struct Workspace {
    /// The `tags_root` setting: where the tree lives, `<data>/tags/`
    /// unless set (docs/editor/tag-workspace.md, "The tree and its
    /// versions"). Saved with the layout.
    root: Option<PathBuf>,
    /// The disc the tree is kept current with: the one last extracted
    /// (or being extracted), recorded so a later start runs the plan
    /// against it. Saved with the layout.
    disc: Option<PathBuf>,
    /// The plan as last computed: what is stale. `None` until a plan ran.
    stale: Option<Plan>,
    /// The extraction running, or finished and not yet polled.
    job: Option<Job>,
    /// What the last extraction did, or why it wrote nothing.
    last: Option<Result<Value, String>>,
    /// The sizes as last walked; recomputed after an extraction or a
    /// prune (`App::workspace_status`).
    sizes: Option<Sizes>,
}


/// The ini section the settings are saved in, after Dear ImGui's own and
/// the field panel's (`[Studio][Tags]`).
const INI_SECTION: &str = "[Studio][Workspace]";

impl Workspace {
    /// The settings as an ini section.
    pub fn ini(&self) -> String {
        let mut out = format!("{INI_SECTION}\n");
        if let Some(r) = &self.root {
            out.push_str(&format!("TagsRoot={}\n", r.display()));
        }
        if let Some(d) = &self.disc {
            out.push_str(&format!("Disc={}\n", d.display()));
        }
        out
    }

    /// The settings in an ini text (unset where it has none).
    pub fn from_ini(text: &str) -> Self {
        let mut out = Self::default();
        let mut inside = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                inside = line == INI_SECTION;
                continue;
            }
            if !inside {
                continue;
            }
            if let Some(v) = line.strip_prefix("TagsRoot=").filter(|v| !v.is_empty()) {
                out.root = Some(PathBuf::from(v));
            } else if let Some(v) = line.strip_prefix("Disc=").filter(|v| !v.is_empty()) {
                out.disc = Some(PathBuf::from(v));
            }
        }
        out
    }

    /// The bar the status line shows while an extraction runs: how far
    /// along, and the line of the map it last finished.
    pub fn progress(&self) -> Option<(f32, String)> {
        let job = self.job.as_ref()?;
        let s = job.shared.lock().unwrap_or_else(|e| e.into_inner());
        let frac = if s.total == 0 {
            0.0
        } else {
            s.done as f32 / s.total as f32
        };
        let text = s
            .last
            .clone()
            .unwrap_or_else(|| "reading the disc...".to_owned());
        Some((frac.min(1.0), text))
    }

    /// An extraction has begun this session (running or finished).
    fn begun(&self) -> bool {
        self.job.is_some() || self.last.is_some()
    }
}

/// The extraction's state, shared with its thread: the plan once computed,
/// the maps done of total, the last map's line, and the outcome.
#[derive(Default)]
struct Shared {
    plan: Option<Plan>,
    total: usize,
    done: usize,
    last: Option<String>,
    outcome: Option<Result<Value, String>>,
}

/// An extraction running on its own thread.
struct Job {
    pub disc: PathBuf,
    pub root: PathBuf,
    shared: Arc<Mutex<Shared>>,
}

impl Job {
    fn done(&self) -> bool {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outcome
            .is_some()
    }
}

/// What starting an extraction answered: the disc, the tree's root, the
/// maps it reads and whether it verifies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extracting {
    pub disc: PathBuf,
    pub root: PathBuf,
    pub maps: Vec<String>,
    pub verify: bool,
}

/// Why an extraction cannot start ([`App::workspace_extract`]).
#[derive(Debug)]
pub enum ExtractRefusal {
    /// The tree's root would sit inside a Git checkout: the tree is game
    /// data, and none of it is committed.
    Checkout { root: PathBuf, repo: PathBuf },
    /// Another extraction is already running.
    Busy,
    /// The disc or the map names: no disc to read, no maps in it, or a
    /// name that is not one of its maps.
    Params(String),
}

impl std::fmt::Display for ExtractRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractRefusal::Checkout { root, repo } => write!(
                f,
                "{} is inside the Git checkout {}: the tree is game data, never in a \
                 repository",
                root.display(),
                repo.display()
            ),
            ExtractRefusal::Busy => {
                write!(
                    f,
                    "an extraction is already running; its progress is in the status line"
                )
            }
            ExtractRefusal::Params(m) => write!(f, "{m}"),
        }
    }
}

/// What the tree's record says (docs/formats/tag-repo.md, "Versions"):
/// the maps extracted, the tags, lossy and refused counts, the variants,
/// and the versions it was extracted under.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeStats {
    pub maps: usize,
    pub tags: u32,
    pub lossy: u32,
    pub refused: u32,
    pub variants: usize,
    pub extractor: u32,
    pub format: u16,
}

/// The tree's record, when a tree is there.
fn tree_stats(root: &Path) -> Option<TreeStats> {
    let repo = TagRepo::open(root).ok()?;
    let record = repo.extract_record().ok()??;
    let mut stats = TreeStats {
        maps: repo.maps().ok()?.len(),
        extractor: record.extractor,
        format: record.format,
        ..Default::default()
    };
    for m in record.maps.values() {
        for g in m.groups.values() {
            stats.tags += g.tags;
            stats.lossy += g.lossy;
            stats.refused += g.refused;
        }
    }
    stats.variants = std::fs::read_to_string(root.join("conflicts.txt"))
        .map(|t| t.lines().filter(|l| !l.is_empty()).count())
        .unwrap_or(0);
    Some(stats)
}

/// One map file of the disc: its stem, the file it is read from, and its
/// region (an ISO entry) or the whole file.
#[derive(Clone)]
struct Source {
    stem: String,
    path: PathBuf,
    base: u64,
    len: u64,
}

fn shared_file(stem: &str) -> Option<map::ResourceFile> {
    match stem {
        "mainmenu" => Some(map::ResourceFile::MainMenu),
        "shared" => Some(map::ResourceFile::Shared),
        "single_player_shared" => Some(map::ResourceFile::SinglePlayerShared),
        _ => None,
    }
}

/// The maps of a disc: an image's `.map` entries, a lone `.map` file
/// (with the shared caches beside it), or a folder of `.map` files (an
/// extracted disc: its `maps\` folder, or the folder itself).
fn sources(disc: &Path) -> Result<Vec<Source>, String> {
    let mut out: Vec<Source> = Vec::new();
    let maps_in = |dir: &Path| -> Result<Vec<PathBuf>, String> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("map")))
            .collect();
        files.sort();
        Ok(files)
    };
    let mut add = |p: &Path| -> Result<(), String> {
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("{}: not a map file name", p.display()))?
            .to_owned();
        if out.iter().any(|o: &Source| o.stem == stem) {
            return Ok(());
        }
        let len = std::fs::metadata(p)
            .map_err(|e| format!("{}: {e}", p.display()))?
            .len();
        out.push(Source {
            stem,
            path: p.to_path_buf(),
            base: 0,
            len,
        });
        Ok(())
    };
    if disc.is_dir() {
        let mut files = maps_in(disc)?;
        if files.is_empty() {
            // An extracted disc: its maps are under `maps\`.
            let inner = disc.join("maps");
            if inner.is_dir() {
                files = maps_in(&inner)?;
            }
        }
        for f in &files {
            add(f)?;
        }
    } else if disc
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("map"))
    {
        add(disc)?;
        // The shared caches beside a lone map resolve its shared pointers.
        for s in ["mainmenu", "shared", "single_player_shared"] {
            let q = disc.with_file_name(format!("{s}.map"));
            if q != *disc && q.exists() {
                add(&q)?;
            }
        }
    } else {
        let mut iso = xiso::Xiso::open(BufReader::new(
            File::open(disc).map_err(|e| format!("{}: {e}", disc.display()))?,
        ))
        .map_err(|e| format!("{}: {e}", disc.display()))?;
        let mut entries = iso
            .entries()
            .map_err(|e| format!("{}: {e}", disc.display()))?
            .into_iter()
            .filter(|e| !e.is_dir)
            .filter(|e| {
                e.path
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|f| f.ends_with(".map"))
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        for e in entries {
            let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
            let stem = file.strip_suffix(".map").unwrap_or("").to_owned();
            if out.iter().any(|o| o.stem == stem) {
                continue;
            }
            out.push(Source {
                stem,
                path: disc.to_path_buf(),
                base: iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR,
                len: e.size.into(),
            });
        }
    }
    if out.is_empty() {
        return Err(format!("{}: no maps in it", disc.display()));
    }
    Ok(out)
}

/// The disc an extraction reads: the one given, else the one recorded,
/// else `H2_DISC`, else `disc/halo2.iso` in the repository.
fn resolve_disc(given: Option<&Path>, workspace: &Workspace) -> Option<PathBuf> {
    given
        .map(Path::to_path_buf)
        .or_else(|| workspace.disc.clone())
        .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
        .or_else(|| editor_core::default_disc(None))
}

/// Run the plan and extract, on the thread of the job that owns
/// `shared`, through `tags::repo::compile::prepare` (the call `h2 build`
/// makes): the map files' hashes from their stamps, the plan against the
/// tree's record, what moved extracted, then settled. Progress and the
/// outcome go into `shared`.
fn extract_thread(
    disc: PathBuf,
    root: PathBuf,
    wanted: Option<Vec<String>>,
    verify: bool,
    shared: Arc<Mutex<Shared>>,
) {
    let run = || -> Result<Value, String> {
        let all = sources(&disc)?;
        if let Some(w) = &wanted {
            for name in w {
                if !all.iter().any(|s| &s.stem == name) {
                    return Err(format!("{name}.map is not in {}", disc.display()));
                }
            }
        }
        let mut maps: Vec<&Source> = all
            .iter()
            .filter(|s| wanted.as_ref().is_none_or(|w| w.contains(&s.stem)))
            .collect();
        // Shared caches first, so their tags are the main contents.
        maps.sort_by_key(|s| (shared_file(&s.stem).is_none(), s.stem.clone()));
        if maps.is_empty() {
            return Err(format!("no maps to extract from {}", disc.display()));
        }
        // The same call `h2 build` and the pre-boot step make
        // (tags::repo::compile): the stamps, the plan, the extraction.
        let discs: Vec<tags::repo::compile::DiscMap> = maps
            .iter()
            .map(|s| tags::repo::compile::DiscMap {
                stem: s.stem.clone(),
                path: s.path.clone(),
                base: s.base,
                len: s.len,
            })
            .collect();
        let mut totals = (0usize, 0usize, 0usize, 0usize, 0usize, 0u64);
        let started = std::time::Instant::now();
        let mut on_plan = |plan: &Plan, _: tags::repo::compile::Input| {
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            s.plan = Some(plan.clone());
            s.total = plan.maps.len();
        };
        let mut on_map = |w: &MapWork, r: &PutReport| {
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            s.done += 1;
            s.last = Some(format!(
                "{}: {} tags ({} lossy, {} refused)",
                w.map,
                r.new + r.same + r.variants.len(),
                r.lossy.len(),
                r.refused.len()
            ));
            totals.0 += r.new;
            totals.1 += r.same;
            totals.2 += r.variants.len();
            totals.3 += r.lossy.len();
            totals.4 += r.refused.len();
            totals.5 += r.raw_new.1;
        };
        let prepared = tags::repo::compile::prepare(
            &root,
            Schema::builtin(),
            Some(&discs),
            &tags::repo::compile::Options {
                verify,
                force: false,
                wanted: wanted.clone(),
            },
            &mut |_| {},
            &mut on_plan,
            &mut on_map,
        )
        .map_err(|e| format!("extracting into {}: {e}", root.display()))?;
        let done = prepared.updated.unwrap_or_default();
        Ok(json!({
            "maps": done.maps,
            "reset": done.reset,
            "tags_new": totals.0,
            "tags_same": totals.1,
            "variants": totals.2,
            "lossy": totals.3,
            "refused": totals.4,
            "raw_bytes": totals.5,
            "settled": done.settled.promoted,
            "conflicts": done.settled.conflicts,
            "took_ms": started.elapsed().as_millis() as u64,
        }))
    };
    let r = run();
    shared.lock().unwrap_or_else(|e| e.into_inner()).outcome = Some(r);
}

impl App {
    /// The tags tree's root: the `tags_root` setting, `<data>/tags/`
    /// unless set.
    pub fn tags_root(&self) -> PathBuf {
        self.workspace
            .root
            .clone()
            .unwrap_or_else(|| self.data_root().join("tags"))
    }

    /// The first start's page shows (docs/editor/tag-workspace.md, "First
    /// start"): no tree, nothing extracted this session, and no source
    /// open other than a disc image (the disc the page asks for; a loose
    /// map or a folder of them is worked on without a tree).
    pub fn workspace_start_page(&self) -> bool {
        !self.workspace.begun()
            && !self.tags_root().join("repo.txt").exists()
            && self
                .source()
                .is_none_or(|s| matches!(s, editor_core::MapSource::Disc(_)))
    }

    /// The background plan of a later start (docs/editor/tag-workspace.md,
    /// "First start"): with a tree and a disc, run the plan and extract
    /// what is stale while the window keeps working. With no tree the
    /// start page shows instead, and nothing runs.
    pub fn start_workspace(&mut self) {
        if !self.tags_root().join("repo.txt").exists() || self.workspace.job.is_some() {
            return;
        }
        let Some(disc) = resolve_disc(None, &self.workspace) else {
            return;
        };
        if let Err(e) = self.workspace_extract(Some(disc), None, false, None) {
            self.info(format!("tags: the plan did not run: {e}"));
        }
    }

    /// Start an extraction ([`extract_thread`] on its own thread): the
    /// plan against `disc` (the one given, else the recorded one, else
    /// `H2_DISC`, else `disc/halo2.iso`), only `maps` of it, with
    /// verification when `verify`. `root` sets the tree's root (the
    /// `tags_root` setting). Answers what it started at once; the
    /// progress is in [`App::workspace_status`] and the status line.
    pub fn workspace_extract(
        &mut self,
        disc: Option<PathBuf>,
        maps: Option<Vec<String>>,
        verify: bool,
        root: Option<PathBuf>,
    ) -> Result<Extracting, ExtractRefusal> {
        if self.workspace.job.is_some() {
            return Err(ExtractRefusal::Busy);
        }
        let root_given = root.is_some();
        let root = root.unwrap_or_else(|| self.tags_root());
        if let Some(repo) = crate::app::ui_screens::git_checkout(&root).or_else(|| {
            crate::app::ui_screens::git_checkout(root.parent().unwrap_or(Path::new(".")))
        }) {
            return Err(ExtractRefusal::Checkout { root, repo });
        }
        let disc = resolve_disc(disc.as_deref(), &self.workspace).ok_or_else(|| {
            ExtractRefusal::Params(
                "no disc: pass disc (an image, a .map or a folder of maps), set H2_DISC, \
                 or put disc/halo2.iso in the repository"
                    .to_owned(),
            )
        })?;
        let all = sources(&disc).map_err(ExtractRefusal::Params)?;
        let mut stems: Vec<String> = all.iter().map(|s| s.stem.clone()).collect();
        tags::repo::store::extraction_order(&mut stems);
        if let Some(w) = &maps {
            for name in w {
                if !stems.contains(name) {
                    return Err(ExtractRefusal::Params(format!(
                        "{name}.map is not in {} ({})",
                        disc.display(),
                        stems.join(", ")
                    )));
                }
            }
        }
        let job = Job {
            disc: disc.clone(),
            root: root.clone(),
            shared: Arc::default(),
        };
        let shared = Arc::clone(&job.shared);
        let wanted = maps.clone();
        let spawned = std::thread::Builder::new()
            .name("tag extract".into())
            .spawn(move || extract_thread(disc, root, wanted, verify, shared));
        if let Err(e) = spawned {
            return Err(ExtractRefusal::Params(format!(
                "starting the extraction: {e}"
            )));
        }
        if root_given {
            // The `tags_root` setting: remembered with the layout.
            self.workspace.root = Some(job.root.clone());
        }
        self.workspace.disc = Some(job.disc.clone());
        self.workspace.job = Some(job);
        self.workspace.sizes = None;
        Ok(Extracting {
            disc: self.workspace.disc.clone().unwrap(),
            root: self.tags_root(),
            maps: stems
                .into_iter()
                .filter(|s| maps.as_ref().is_none_or(|w| w.contains(s)))
                .collect(),
            verify,
        })
    }

    /// Pick up a finished extraction (each frame, and between a served
    /// editor's requests): log what it did, or why it wrote nothing.
    pub fn poll_workspace(&mut self) {
        if !self.workspace.job.as_ref().is_some_and(|j| j.done()) {
            return;
        }
        let job = self.workspace.job.take().expect("checked");
        let shared = job.shared.lock().unwrap_or_else(|e| e.into_inner());
        let outcome = shared.outcome.clone().expect("done");
        let plan = shared.plan.clone();
        drop(shared);
        match &outcome {
            Ok(v) => self.info(format!(
                "tags: {} maps in {:.1}s ({} tags new, {} same, {} variants; {:.1} MB raw)",
                v["maps"],
                v["took_ms"].as_f64().unwrap_or(0.0) / 1000.0,
                v["tags_new"],
                v["tags_same"],
                v["variants"],
                v["raw_bytes"].as_f64().unwrap_or(0.0) / 1048576.0,
            )),
            Err(e) => self.error(format!("tags: {}: {e}", job.disc.display())),
        }
        // What the run leaves stale: nothing when it wrote everything it
        // planned; the plan as it was when a failure stopped it.
        self.workspace.stale = Some(match &outcome {
            Ok(_) => Plan::default(),
            Err(_) => plan.unwrap_or_default(),
        });
        self.workspace.last = Some(outcome);
        self.workspace.sizes = None;
    }

    /// Wait for the running extraction (tests and headless runs), then
    /// pick it up.
    pub fn wait_workspace(&mut self) {
        while self.workspace.job.as_ref().is_some_and(|j| !j.done()) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.poll_workspace();
    }

    /// `workspace.status` (docs/editor/agent-api.md): the tree, the
    /// plan's stale parts, an extraction in progress, the last one's
    /// outcome, and the sizes of everything the workspace holds. Sizes
    /// are walked once and kept until the next extraction or prune.
    pub fn workspace_status(&mut self) -> Value {
        let root = self.tags_root();
        let has_tree = root.join("repo.txt").exists();
        let tree = has_tree.then(|| {
            tree_stats(&root).map(|t| {
                json!({
                    "maps": t.maps,
                    "tags": t.tags,
                    "lossy": t.lossy,
                    "refused": t.refused,
                    "variants": t.variants,
                    "extractor": t.extractor,
                    "format": t.format,
                })
            })
        });
        let why = |w: &Why| match w {
            Why::New => "new",
            Why::FileChanged => "file_changed",
            Why::Versions => "versions",
            Why::Forced => "forced",
            Why::Layouts => "layouts",
            Why::Skeleton => "skeleton",
        };
        let plan_of = |p: &Plan| {
            json!({
                "reset": p.reset,
                "maps": p.maps.iter().map(|w| json!({
                    "map": w.map,
                    "why": why(&w.why),
                    "groups": w.groups.as_ref().map(|g| g.iter()
                        .map(|g| String::from_utf8_lossy(&g.0).into_owned())
                        .collect::<Vec<_>>()),
                })).collect::<Vec<_>>(),
            })
        };
        let (extracting, stale) = match &self.workspace.job {
            Some(job) => {
                let s = job.shared.lock().unwrap_or_else(|e| e.into_inner());
                let extracting = json!({
                    "disc": job.disc.display().to_string(),
                    "root": job.root.display().to_string(),
                    "maps_total": s.total,
                    "maps_done": s.done,
                    "last": s.last,
                });
                let stale = s.plan.as_ref().map(plan_of);
                (Some(extracting), stale)
            }
            None => (None, self.workspace.stale.as_ref().map(plan_of)),
        };
        let (ws, maps_dir) = (
            self.data_root().join("workspace"),
            self.data_root().join("maps"),
        );
        let sizes = self
            .workspace
            .sizes
            .get_or_insert_with(|| Sizes::walk(&root, &ws, &maps_dir));
        json!({
            "root": root.display().to_string(),
            "has_tree": has_tree,
            "tree": tree,
            "disc": self.workspace.disc.as_ref().map(|d| d.display().to_string()),
            "stale": stale,
            "extracting": extracting,
            "last": self.workspace.last.as_ref().map(|r| match r {
                Ok(v) => v.clone(),
                Err(e) => json!({"error": e}),
            }),
            "sizes": sizes.json(),
            "start_page": self.workspace_start_page(),
        })
    }

    /// `workspace.prune` (docs/editor/tag-workspace.md, "Size and
    /// cleanup"): raw resources nothing names (no tag, manifest or extent table), generated layers no
    /// project asks for, and build caches unused for a week. The tree
    /// itself is never pruned in part. With `dry_run`, what would go.
    /// Project outputs are the projects' (W2) and are not pruned yet.
    pub fn workspace_prune(&mut self, dry_run: bool) -> Result<Value, String> {
        let root = self.tags_root();
        let workspace = self.data_root().join("workspace");
        let mut bytes = 0u64;
        let mut removed: Vec<String> = Vec::new();
        // Raw resources nothing names: a tag file or a manifest's
        // low-detail region.
        if root.join("repo.txt").exists() {
            let named = named_raws(&root)?;
            for (file, size) in files_under(&root.join("raw")) {
                let hash = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned();
                if named.contains(&hash) {
                    continue;
                }
                bytes += size;
                removed.push(hash);
                if !dry_run {
                    let _ = std::fs::remove_file(&file);
                }
            }
        }
        // Generated layers no project asks for: with no project files,
        // nothing asks for any of them. The layers are derived data, made
        // again when a kit runs; which fingerprint a project asks for is
        // the project file's (W2's projects).
        let projects = std::fs::read_dir(workspace.join("projects"))
            .map(|d| {
                d.flatten().any(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("toml"))
                })
            })
            .unwrap_or(false);
        let mut generated_removed = Vec::new();
        let mut generated_kept = Vec::new();
        for (name, size) in subdirs(&workspace.join("generated")) {
            if projects {
                generated_kept.push(name);
                continue;
            }
            bytes += size;
            if !dry_run {
                let _ = std::fs::remove_dir_all(workspace.join("generated").join(&name));
            }
            generated_removed.push(name);
        }
        // Build caches unused for a week.
        let mut caches = Vec::new();
        for (name, size) in subdirs(&workspace.join("build")) {
            let dir = workspace.join("build").join(&name);
            let Some(used) = newest(&dir) else { continue };
            let unused = SystemTime::now().duration_since(used).unwrap_or_default();
            if unused < UNUSED_AFTER {
                continue;
            }
            bytes += size;
            caches.push(json!({
                "name": name,
                "unused_days": unused.as_secs() / 86_400,
            }));
            if !dry_run {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        self.workspace.sizes = None;
        Ok(json!({
            "dry_run": dry_run,
            "raw_removed": removed.len(),
            "raw": removed,
            "generated_removed": generated_removed,
            "generated_kept": generated_kept,
            "caches_removed": caches,
            // Project outputs (docs/editor/tag-workspace.md) are named by
            // the project files that build them (W2): none are pruned.
            "outputs_removed": [],
            "bytes": bytes,
        }))
    }
}

/// The maps of a disc and the bytes of their files: what the start page
/// shows before an extraction begins.
pub fn disc_summary(disc: &Path) -> Result<(Vec<String>, u64), String> {
    let all = sources(disc)?;
    let bytes = all.iter().map(|s| s.len).sum();
    let mut stems: Vec<String> = all.iter().map(|s| s.stem.clone()).collect();
    tags::repo::store::extraction_order(&mut stems);
    Ok((stems, bytes))
}

/// The bytes of everything the workspace accounts
/// (docs/editor/tag-workspace.md, "Size and cleanup"): the tree, each
/// layer under `<data>/workspace/`, each project's build cache, and each
/// package under `<data>/maps/`.
#[derive(Clone, Debug, Default)]
pub struct Sizes {
    pub tree: u64,
    /// The authored layer, `<data>/workspace/generated/`.
    pub generated: Vec<(String, u64)>,
    /// The imported layer, `<data>/workspace/imports/`.
    pub imports: Vec<(String, u64)>,
    /// The edit files, `<data>/workspace/edits/`.
    pub edits: Vec<(String, u64)>,
    /// Each project's build cache, `<data>/workspace/build/`.
    pub projects: Vec<(String, u64)>,
    /// Each package under `<data>/maps/`.
    pub outputs: Vec<(String, u64)>,
}

impl Sizes {
    /// Walk them once.
    pub fn walk(tree: &Path, workspace: &Path, maps: &Path) -> Self {
        Sizes {
            tree: dir_bytes(tree),
            generated: subdirs(&workspace.join("generated")),
            imports: subdirs(&workspace.join("imports")),
            edits: subdirs(&workspace.join("edits")),
            projects: subdirs(&workspace.join("build")),
            outputs: subdirs(maps),
        }
    }

    /// Everything together.
    pub fn total(&self) -> u64 {
        self.tree
            + self.layers().map(|(_, b)| b).sum::<u64>()
            + self.projects.iter().map(|(_, b)| b).sum::<u64>()
            + self.outputs.iter().map(|(_, b)| b).sum::<u64>()
    }

    fn layers(&self) -> impl Iterator<Item = &(String, u64)> {
        self.generated
            .iter()
            .chain(&self.imports)
            .chain(&self.edits)
    }

    fn json(&self) -> Value {
        let map = |v: &[(String, u64)]| {
            serde_json::Map::from_iter(v.iter().map(|(n, b)| (n.clone(), json!(b))))
        };
        json!({
            "tree": self.tree,
            "layers": {
                "generated": map(&self.generated),
                "imports": map(&self.imports),
                "edits": map(&self.edits),
            },
            "projects": map(&self.projects),
            "outputs": map(&self.outputs),
            "total": self.total(),
        })
    }
}

/// The bytes of the files under `dir`, recursively; symlinks are not
/// followed.
fn dir_bytes(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            total += dir_bytes(&e.path());
        } else if let Ok(m) = e.metadata() {
            total += m.len();
        }
    }
    total
}

/// `(name, bytes)` of `dir`'s subdirectories, by name; dot-prefixed ones
/// (hidden siblings of atomic writes) are skipped.
fn subdirs(dir: &Path) -> Vec<(String, u64)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, u64)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            Some((name, dir_bytes(&e.path())))
        })
        .collect();
    out.sort();
    out
}

/// Every file under `dir` with its bytes: `(path, len)`, sorted.
fn files_under(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                todo.push(p);
            } else if let Ok(m) = e.metadata() {
                out.push((p, m.len()));
            }
        }
    }
    out.sort();
    out
}

/// The newest modification of a file under `dir`: when it was last
/// written. The directory's own time moves whenever an entry is added or
/// removed, so it stands in only for a directory with no files.
fn newest(dir: &Path) -> Option<SystemTime> {
    let files = files_under(dir)
        .into_iter()
        .filter_map(|(p, _)| std::fs::metadata(p).ok().and_then(|m| m.modified().ok()))
        .max();
    files.or_else(|| std::fs::metadata(dir).ok().and_then(|m| m.modified().ok()))
}

/// The hashes every tag file, manifest and extent table in the tree names: the raw
/// resources that stay.
fn named_raws(root: &Path) -> Result<HashSet<String>, String> {
    let mut named = HashSet::new();
    for (file, _) in files_under(&root.join("tags")) {
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let tag =
            tags::repo::format::decode(&bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        if let Some(t) = tag.tree() {
            for raw in &t.raws {
                named.insert(raw.hex());
            }
        }
    }
    for (file, _) in files_under(&root.join("maps")) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if let Ok(m) = Manifest::parse(&text) {
            if let Some(h) = m.low_detail {
                named.insert(h);
            }
        }
        // A map's extent table names every raw file its map file is
        // served from.
        if let Ok(x) = tags::repo::skeleton::Extents::parse(&text) {
            named.extend(x.extents.into_iter().map(|e| e.hash));
        }
    }
    Ok(named)
}

#[cfg(test)]
pub(crate) mod tests;
