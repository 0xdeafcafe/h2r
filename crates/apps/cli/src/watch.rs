//! `h2 play --watch`: keep playing while the checkout changes.
//!
//! The sibling of `--follow` (follow.rs): the supervisor runs the game as a
//! child, but watches the user's own checkout (its working tree and git
//! state, through FSEvents on macOS) instead of a branch. A burst of file
//! events, such as a `git pull`, is debounced ([`Debounce`]) until the tree
//! has been quiet for a while and no git operation is in progress; the
//! change set is then classified ([`classify`], [`Plan`]):
//!
//! - rules scripts go into the running game with `rules.reload`, and the
//!   live web app is rebuilt in place (hot, no restart);
//! - host Rust and baked data rebuild `h2` in the checkout
//!   (`cargo build --release -p cli`), the generator's inputs as well, and
//!   the new `h2`'s `h2 build --check` says whether the game code must be
//!   generated again; either then offers the restart banner `--follow`
//!   uses ([`follow::RESTART_EXIT`]);
//! - docs and tests do nothing.
//!
//! [`Supervisor`] is the pure state machine; [`drive`] runs it against a
//! [`Builder`], a [`HotRunner`], a [`Launcher`] and a [`StatusSink`], which
//! the tests replace with fakes. The process plumbing (the child, the
//! status file, build steps, signals and the lock) is `--follow`'s. See
//! docs/runtime/app.md, "Watching the checkout".

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use app::{Notice, NoticeKind};

use crate::follow::{
    self, Build, ChildLauncher, Commit, Launcher, Log, Owned, RESTART_EXIT, StatusFile, StatusSink,
    Step,
};

const USAGE: &str = "usage:
  h2 play --watch [--repo PATH] [--quiet MS] [--fresh-data] [--cache DIR] <h2 play arguments>

Runs `h2 play` as a child and watches your checkout (default: the one
containing the current directory). Once the files have been quiet for
--quiet milliseconds (1500) and no git operation is in progress (a pull,
merge, rebase or cherry-pick), it looks at what changed:
  rules scripts      reloaded into the running game (rules.reload)
  web/               the live web app rebuilt (with --live-local)
  crates/, assets/   h2 rebuilt in the checkout (cargo build --release -p cli),
                     and the game code generated again when the new h2 says so
  docs, tests        nothing
A rebuild shows \"Update ready\" in the game window: F5 restarts into it,
F6 hides the banner. A failed build shows its first error and the game keeps
running. Logs and builds live in the cache dir (default `watch/<checkout>`
in the cache directory, docs/runtime/storage.md).";

/// How long the files must be quiet before a change set is taken.
pub const QUIET: Duration = Duration::from_millis(1500);
/// How often a git operation in progress is looked at again.
const BUSY_POLL: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// What a changed path means.

/// What one changed path asks for. Paths are relative to the checkout's
/// top, `/`-separated; the checkout's git dir is `.git/...` (also when it
/// is a linked worktree's, elsewhere on disk).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Not watched: build outputs, other worktrees, editor droppings.
    Ignored,
    /// Git's own state (HEAD, refs, the index): it holds the settle timer
    /// open, and builds nothing.
    Git,
    /// Docs, tests and tooling outside `h2`.
    Nothing,
    /// A rules script: `rules.reload` into the running game.
    Rules,
    /// A built-in rules script: reloaded, and baked into `h2`, so `h2` is
    /// rebuilt for the next start too.
    BuiltinRules,
    /// The live web app: `bun run build`, served at the next page load.
    Web,
    /// Host Rust and the data baked into `h2` (UI screens, HUD kit).
    Host,
    /// The generator's inputs: `h2`, then the game code generated again
    /// when the new `h2 build --check` fails.
    Image,
}

const BUILTIN_RULES: &str = "crates/formats/rules/src/builtin/";

/// Path components that are never sources.
const IGNORED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "buck-out",
    "__pycache__",
    ".direnv",
];

/// Prefixes of what the tree holds that no build reads (gitignored outputs,
/// other worktrees and git's object store).
const IGNORED_PREFIXES: &[&str] = &[
    ".claude/worktrees/",
    "web/dist/",
    "out/",
    "extracted/",
    "disc/",
    ".tools/",
    ".git/objects/",
    ".git/logs/",
    ".git/worktrees/",
    ".git/modules/",
    ".git/lfs/",
    ".git/fsmonitor--daemon/",
];

/// What turns the XBE into the game code: the directories
/// `crates/runtime/recomp/build.rs` hashes for `GENERATOR_SOURCE_HASH` and
/// `FAST_TIER_SOURCE_HASH` (keep the two lists in step).
const IMAGE_PREFIXES: &[&str] = &[
    "crates/formats/xbe/src/",
    "crates/recompiler/",
    "crates/runtime/runtime/include/",
    "crates/runtime/recomp/c/",
    "crates/runtime/recomp/build.rs",
    "crates/runtime/recomp/src/render_entries.rs",
];

/// What `cargo build -p cli` reads beyond the generator.
const HOST_PREFIXES: &[&str] = &["crates/", "assets/", ".cargo/"];
const HOST_FILES: &[&str] = &["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"];

/// The web app's tests, benchmarks and fixtures.
const WEB_NOTHING: &[&str] = &["test/", "bench/", "mock/"];

fn editor_temp(name: &str) -> bool {
    name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swo")
        || name.ends_with(".swx")
        || name.starts_with(".#")
        || name == "4913"
        || name == ".DS_Store"
}

fn is_test(parts: &[&str], name: &str) -> bool {
    parts
        .iter()
        .any(|c| matches!(*c, "tests" | "benches" | "examples"))
        || name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name.ends_with("_test.rs")
}

/// What a changed path asks for (the table in docs/runtime/app.md,
/// "Watching the checkout").
pub fn classify(rel: &str) -> Kind {
    let rel = rel.trim_start_matches("./");
    if rel.is_empty() {
        return Kind::Ignored;
    }
    let ignored_prefix = IGNORED_PREFIXES.iter().any(|p| rel.starts_with(p));
    if rel == ".git" || rel.starts_with(".git/") {
        return if ignored_prefix {
            Kind::Ignored
        } else {
            Kind::Git
        };
    }
    let parts: Vec<&str> = rel.split('/').collect();
    let name = parts.last().copied().unwrap_or("");
    if ignored_prefix
        || editor_temp(name)
        || parts
            .iter()
            .any(|c| IGNORED_DIRS.contains(c) || c.starts_with(".devenv"))
    {
        return Kind::Ignored;
    }
    if name.ends_with(".rules") {
        return if rel.starts_with(BUILTIN_RULES) {
            Kind::BuiltinRules
        } else {
            Kind::Rules
        };
    }
    if rel.starts_with("docs/") || name.ends_with(".md") {
        return Kind::Nothing;
    }
    if let Some(web) = rel.strip_prefix("web/") {
        return if WEB_NOTHING.iter().any(|p| web.starts_with(p)) {
            Kind::Nothing
        } else {
            Kind::Web
        };
    }
    if is_test(&parts, name) {
        return Kind::Nothing;
    }
    if IMAGE_PREFIXES.iter().any(|p| rel.starts_with(p)) {
        return Kind::Image;
    }
    if HOST_PREFIXES.iter().any(|p| rel.starts_with(p)) || HOST_FILES.contains(&rel) {
        return Kind::Host;
    }
    Kind::Nothing
}

/// What one change set asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Rules scripts to reload, relative paths.
    pub rules: BTreeSet<String>,
    /// Rebuild the live web app.
    pub web: bool,
    /// Rebuild `h2`.
    pub host: bool,
    /// The generator changed: `h2`, then the game code if its stamp moved.
    pub image: bool,
}

impl Plan {
    pub fn of<'a>(paths: impl IntoIterator<Item = &'a str>) -> Plan {
        let mut p = Plan::default();
        for rel in paths {
            match classify(rel) {
                Kind::Rules => {
                    p.rules.insert(rel.to_string());
                }
                Kind::BuiltinRules => {
                    p.rules.insert(rel.to_string());
                    p.host = true;
                }
                Kind::Web => p.web = true,
                Kind::Host => p.host = true,
                Kind::Image => p.image = true,
                Kind::Ignored | Kind::Git | Kind::Nothing => {}
            }
        }
        p
    }

    /// A settled change set's plan. A rescan (the watcher lost events)
    /// rebuilds: `h2 build --check` still decides about the game code.
    pub fn of_changes(c: &Changes) -> Plan {
        let mut p = Plan::of(c.paths.iter().map(String::as_str));
        if c.rescan {
            p.host = true;
            p.image = true;
        }
        p
    }

    pub fn merge(&mut self, o: Plan) {
        self.rules.extend(o.rules);
        self.web |= o.web;
        self.host |= o.host;
        self.image |= o.image;
    }

    pub fn builds(&self) -> bool {
        self.host || self.image
    }

    pub fn has_hot(&self) -> bool {
        !self.rules.is_empty() || self.web
    }

    pub fn is_empty(&self) -> bool {
        !self.builds() && !self.has_hot()
    }

    /// The part a hot runner does.
    pub fn hot(&self) -> Plan {
        Plan {
            rules: self.rules.clone(),
            web: self.web,
            ..Plan::default()
        }
    }

    /// The part a build does.
    pub fn build(&self) -> Plan {
        Plan {
            host: self.host,
            image: self.image,
            ..Plan::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Settling a burst of events.

/// A settled change set: the paths that changed (not ignored, not git's
/// own), and whether the watcher lost events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub paths: BTreeSet<String>,
    pub rescan: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Settle {
    /// Nothing pending: wait for the next event.
    Idle,
    /// Look again after this long (or at the next event).
    Wait(Duration),
    /// A git operation is in progress: look again soon.
    Busy,
    /// Quiet for long enough: this is the change set.
    Ready(Changes),
}

/// The debounce: every event (git's own included) restarts the quiet
/// period, and so does a git operation in progress, so a pull settles
/// once, after its last write and its lock are gone.
#[derive(Debug)]
pub struct Debounce {
    quiet: Duration,
    last: Option<Instant>,
    pending: Changes,
}

impl Debounce {
    pub fn new(quiet: Duration) -> Self {
        Self {
            quiet,
            last: None,
            pending: Changes::default(),
        }
    }

    /// A path changed at `now`.
    pub fn event(&mut self, now: Instant, rel: &str) {
        match classify(rel) {
            Kind::Ignored => {}
            Kind::Git => self.last = Some(now),
            _ => {
                self.last = Some(now);
                self.pending.paths.insert(rel.to_string());
            }
        }
    }

    /// The watcher lost events (FSEvents' rescan flag).
    pub fn rescan(&mut self, now: Instant) {
        self.last = Some(now);
        self.pending.rescan = true;
    }

    /// What to do at `now`; `busy`: a git operation is in progress.
    pub fn poll(&mut self, now: Instant, busy: bool) -> Settle {
        let Some(last) = self.last else {
            return Settle::Idle;
        };
        if busy {
            self.last = Some(now);
            return Settle::Busy;
        }
        let since = now.saturating_duration_since(last);
        if since < self.quiet {
            return Settle::Wait(self.quiet - since);
        }
        self.last = None;
        let changes = std::mem::take(&mut self.pending);
        if changes.paths.is_empty() && !changes.rescan {
            Settle::Idle
        } else {
            Settle::Ready(changes)
        }
    }
}

/// Files in a git dir that mean an operation is in progress.
pub const GIT_BUSY: &[&str] = &[
    "index.lock",
    "HEAD.lock",
    "MERGE_HEAD",
    "rebase-merge",
    "rebase-apply",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
];

/// Whether a git operation is in progress, given which git dir entries
/// exist.
pub fn git_busy_with(exists: impl Fn(&str) -> bool) -> bool {
    GIT_BUSY.iter().any(|n| exists(n))
}

fn git_busy(git_dir: &Path) -> bool {
    git_busy_with(|n| fs::symlink_metadata(git_dir.join(n)).is_ok())
}

/// `path` relative to the checkout, `/`-separated, with the git dir
/// (wherever it is) as `.git`; `None` outside both.
pub fn relative(path: &Path, root: &Path, git_dir: &Path) -> Option<String> {
    let join = |p: &Path| {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    };
    if let Ok(r) = path.strip_prefix(git_dir) {
        let r = join(r);
        return Some(if r.is_empty() {
            ".git".into()
        } else {
            format!(".git/{r}")
        });
    }
    path.strip_prefix(root).ok().map(join)
}

// ---------------------------------------------------------------------------
// The supervisor.

#[derive(Debug)]
pub enum Event {
    /// A settled change set's plan.
    Changes(Plan),
    /// Build `id` finished: a new build, `None` when it made the same `h2`
    /// and game code as the running ones, or its first error line.
    Built(Result<Option<Build>, String>),
    /// The hot runner finished: what to show, if anything.
    Hot(Option<Notice>),
    /// The game exited with this code.
    ChildExited(i32),
    /// Ctrl-C or SIGTERM to the supervisor.
    Interrupt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Reload into the running game / rebuild the web app.
    Hot(Plan),
    /// Build `plan` (its host and image parts); `keep`: the build and gen
    /// dirs still in use.
    StartBuild {
        id: u64,
        plan: Plan,
        keep: Vec<PathBuf>,
    },
    Status(Option<Notice>),
    Launch(Build),
    TerminateChild,
    CancelBuild,
    Exit(i32),
}

/// The running build, at most one build in progress (changes that arrive
/// meanwhile are merged into one queued plan), the newest ready build, the
/// last failure (whose plan is carried into the next build) and the last
/// hot result.
#[derive(Debug)]
pub struct Supervisor {
    running: Build,
    next: u64,
    building: Option<(u64, Plan)>,
    queued: Plan,
    carry: Plan,
    ready: Option<Build>,
    failed: Option<String>,
    hot: Option<Notice>,
    stopping: bool,
}

fn building_text(p: &Plan) -> String {
    if p.image {
        "Rebuilding h2 and the game code\u{2026}".into()
    } else {
        "Rebuilding h2\u{2026}".into()
    }
}

impl Supervisor {
    pub fn new(running: Build) -> Self {
        Self {
            running,
            next: 0,
            building: None,
            queued: Plan::default(),
            carry: Plan::default(),
            ready: None,
            failed: None,
            hot: None,
            stopping: false,
        }
    }

    pub fn start(&self) -> Vec<Action> {
        vec![Action::Status(None), Action::Launch(self.running.clone())]
    }

    /// What the banner says now: a ready update, else a build in progress,
    /// else a failure, else the last hot result.
    pub fn notice(&self) -> Option<Notice> {
        if let Some(r) = &self.ready {
            let mut text = format!(
                "Update ready: {} \u{2014} press F5 to restart",
                follow::clip(&r.commit.subject, 60)
            );
            if self.building.is_some() || self.queued.builds() {
                text += " (rebuilding\u{2026})";
            } else if self.failed.is_some() {
                text += " (newer changes failed to build)";
            }
            return Some(Notice {
                kind: NoticeKind::Ready,
                text,
            });
        }
        if let Some((_, p)) = &self.building {
            return Some(Notice {
                kind: NoticeKind::Info,
                text: building_text(p),
            });
        }
        if let Some(e) = &self.failed {
            return Some(Notice {
                kind: NoticeKind::Error,
                text: format!("Local build failed: {}", follow::clip(e, 70)),
            });
        }
        self.hot.clone()
    }

    fn keep(&self) -> Vec<PathBuf> {
        let mut keep = Vec::new();
        for b in std::iter::once(&self.running).chain(&self.ready) {
            keep.extend(b.exe.parent().map(Path::to_path_buf));
            keep.push(b.out.clone());
        }
        keep
    }

    fn start_build(&mut self, plan: Plan) -> Action {
        let mut plan = plan.build();
        plan.merge(std::mem::take(&mut self.carry));
        self.next += 1;
        self.failed = None;
        self.building = Some((self.next, plan.clone()));
        Action::StartBuild {
            id: self.next,
            plan,
            keep: self.keep(),
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::Changes(plan) => {
                if self.stopping || plan.is_empty() {
                    return vec![];
                }
                let mut actions = Vec::new();
                if plan.has_hot() {
                    actions.push(Action::Hot(plan.hot()));
                }
                if plan.builds() {
                    if self.building.is_some() {
                        self.queued.merge(plan.build());
                    } else {
                        actions.push(self.start_build(plan));
                    }
                }
                actions.push(Action::Status(self.notice()));
                actions
            }
            Event::Built(result) => {
                let plan = self.building.take().map(|(_, p)| p).unwrap_or_default();
                if self.stopping {
                    return vec![];
                }
                match result {
                    Ok(Some(b)) => {
                        self.failed = None;
                        self.ready = Some(b);
                    }
                    Ok(None) => self.failed = None,
                    Err(e) => {
                        self.failed = Some(e);
                        // Whatever it was for is still to build.
                        self.carry.merge(plan);
                    }
                }
                let mut actions = Vec::new();
                if self.queued.builds() {
                    let q = std::mem::take(&mut self.queued);
                    actions.push(self.start_build(q));
                }
                actions.push(Action::Status(self.notice()));
                actions
            }
            Event::Hot(n) => {
                if self.stopping {
                    return vec![];
                }
                match n {
                    Some(n) => {
                        self.hot = Some(n);
                        vec![Action::Status(self.notice())]
                    }
                    None => vec![],
                }
            }
            Event::ChildExited(code) => {
                if self.stopping || code != RESTART_EXIT {
                    self.stopping = true;
                    return vec![Action::CancelBuild, Action::Exit(code)];
                }
                // F5 with nothing new (a stale banner) just reboots.
                if let Some(b) = self.ready.take() {
                    self.running = b;
                }
                self.hot = None;
                vec![
                    Action::Status(self.notice()),
                    Action::Launch(self.running.clone()),
                ]
            }
            Event::Interrupt => {
                if self.stopping {
                    return vec![Action::CancelBuild, Action::Exit(130)];
                }
                self.stopping = true;
                vec![Action::CancelBuild, Action::TerminateChild]
            }
        }
    }
}

/// Builds in the background; results come back as [`Event::Built`].
pub trait Builder {
    fn start(&mut self, id: u64, plan: &Plan, keep: &[PathBuf]);
    fn cancel(&mut self);
    /// Cancel, and return once the build and everything it started are gone.
    fn shutdown(&mut self) {}
}

/// Reloads into the running game (`game_pid`, 0 if unknown) in the
/// background; the result comes back as [`Event::Hot`].
pub trait HotRunner {
    fn run(&mut self, plan: &Plan, game_pid: u32);
    /// Stop what runs and return once it is gone.
    fn shutdown(&mut self) {}
}

/// Runs the supervisor until it exits; returns the exit code. The build,
/// the hot runner and the game are shut down before it returns.
pub fn drive(
    mut sup: Supervisor,
    events: &Receiver<Event>,
    builder: &mut dyn Builder,
    hot: &mut dyn HotRunner,
    launcher: &mut dyn Launcher,
    status: &mut dyn StatusSink,
) -> i32 {
    let mut actions = sup.start();
    loop {
        for a in actions {
            match a {
                Action::Hot(plan) => hot.run(&plan, launcher.pid()),
                Action::StartBuild { id, plan, keep } => builder.start(id, &plan, &keep),
                Action::Status(n) => status.set(n.as_ref()),
                Action::Launch(b) => {
                    if let Err(e) = launcher.launch(&b) {
                        eprintln!("watch: starting {}: {e:#}", b.exe.display());
                        builder.shutdown();
                        hot.shutdown();
                        return 1;
                    }
                }
                Action::TerminateChild => launcher.terminate(),
                Action::CancelBuild => builder.cancel(),
                Action::Exit(code) => {
                    builder.shutdown();
                    hot.shutdown();
                    launcher.shutdown();
                    return code;
                }
            }
        }
        let Ok(event) = events.recv() else {
            builder.shutdown();
            hot.shutdown();
            launcher.shutdown();
            return 1;
        };
        actions = sup.handle(event);
    }
}

// ---------------------------------------------------------------------------
// The real thing: the file watcher, cargo, h2 build and the agent socket.

#[derive(Debug, PartialEq)]
struct WatchArgs {
    repo: Option<PathBuf>,
    cache: Option<PathBuf>,
    quiet: Duration,
    fresh_data: bool,
    /// The rest: the child's `h2 play` arguments.
    play: Vec<String>,
}

fn parse_args(args: Vec<String>) -> Result<WatchArgs> {
    let mut w = WatchArgs {
        repo: None,
        cache: None,
        quiet: QUIET,
        fresh_data: false,
        play: Vec::new(),
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--watch" => {}
            "--repo" => w.repo = Some(PathBuf::from(val()?)),
            "--cache" => w.cache = Some(PathBuf::from(val()?)),
            "--quiet" => w.quiet = Duration::from_millis(val()?.parse::<u64>()?.max(200)),
            "--fresh-data" => w.fresh_data = true,
            "--follow" => bail!("--watch and --follow are separate supervisors; give one"),
            "--follow-status" => bail!("--follow-status is for the supervisor's child"),
            "-h" | "--help" => bail!("{USAGE}"),
            _ if follow::PLAY_VALUE_FLAGS.contains(&a.as_str()) => {
                let v = val()?;
                w.play.extend([a, v]);
            }
            _ => w.play.push(a),
        }
    }
    Ok(w)
}

/// FNV-1a, for a stable short name per checkout.
fn fnv64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// `<cache>/watch/<hash of the checkout's path>`: one cache, and so one
/// lock, per checkout.
fn default_cache(repo: &Path) -> Result<PathBuf> {
    Ok(host_dirs::cache()
        .context("no cache directory: HOME is not set")?
        .join("watch")
        .join(format!(
            "{:016x}",
            fnv64(repo.as_os_str().to_string_lossy().as_bytes())
        )))
}

/// The checkout's cargo target dir: `CARGO_TARGET_DIR`, else `target`.
fn cargo_target(repo: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(t) if !t.is_empty() => {
            let t = PathBuf::from(t);
            if t.is_absolute() { t } else { repo.join(t) }
        }
        _ => repo.join("target"),
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

enum Raw {
    Paths(Vec<PathBuf>),
    Rescan,
    Error(String),
}

/// Watches `root` (and `git_dir` when it lies outside it) and sends one
/// [`Event::Changes`] per settled change set. Drop the returned watcher,
/// then join the handle: dropping it ends the thread.
fn spawn_watcher(
    root: PathBuf,
    git_dir: PathBuf,
    quiet: Duration,
    log: Log,
    tx: Sender<Event>,
) -> Result<(notify::RecommendedWatcher, std::thread::JoinHandle<()>)> {
    use notify::Watcher as _;
    let (raw_tx, raw) = channel::<Raw>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let msg = match res {
            Ok(ev) if ev.need_rescan() => Raw::Rescan,
            Ok(ev) if matches!(ev.kind, notify::EventKind::Access(_)) => return,
            Ok(ev) => Raw::Paths(ev.paths),
            Err(e) => Raw::Error(e.to_string()),
        };
        let _ = raw_tx.send(msg);
    })
    .context("starting the file watcher")?;
    watcher
        .watch(&root, notify::RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", root.display()))?;
    if !git_dir.starts_with(&root) {
        // A linked worktree's own git dir: HEAD, the index and the
        // operation markers are there.
        watcher
            .watch(&git_dir, notify::RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", git_dir.display()))?;
    }
    let handle = std::thread::Builder::new()
        .name("watch-files".into())
        .spawn(move || {
            let mut deb = Debounce::new(quiet);
            let mut wait: Option<Duration> = None;
            let mut was_busy = false;
            loop {
                let got = match wait {
                    None => raw.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    Some(d) => raw.recv_timeout(d),
                };
                match got {
                    Ok(Raw::Paths(paths)) => {
                        let now = Instant::now();
                        for p in paths {
                            if let Some(rel) = relative(&p, &root, &git_dir) {
                                deb.event(now, &rel);
                            }
                        }
                    }
                    Ok(Raw::Rescan) => {
                        log.line("the file watcher lost events; rebuilding to be safe");
                        deb.rescan(Instant::now());
                    }
                    Ok(Raw::Error(e)) => log.line(&format!("file watcher: {e}")),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                let settle = deb.poll(Instant::now(), git_busy(&git_dir));
                let busy = settle == Settle::Busy;
                if busy && !was_busy {
                    log.line("a git operation is in progress; waiting for it to finish");
                }
                was_busy = busy;
                wait = match settle {
                    Settle::Idle => None,
                    Settle::Wait(d) => Some(d),
                    Settle::Busy => Some(BUSY_POLL),
                    Settle::Ready(changes) => {
                        let plan = Plan::of_changes(&changes);
                        log.line(&format!(
                            "{} changed{}: {}",
                            changes.paths.len(),
                            if changes.rescan { " (rescan)" } else { "" },
                            describe(&plan)
                        ));
                        if !plan.is_empty() && tx.send(Event::Changes(plan)).is_err() {
                            return;
                        }
                        None
                    }
                };
            }
        })
        .context("spawning the watcher")?;
    Ok((watcher, handle))
}

fn describe(p: &Plan) -> String {
    let mut parts = Vec::new();
    if !p.rules.is_empty() {
        parts.push(format!(
            "reload {}",
            p.rules.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if p.web {
        parts.push("rebuild the web app".into());
    }
    if p.image {
        parts.push("rebuild h2 and check the game code".into());
    } else if p.host {
        parts.push("rebuild h2".into());
    }
    if parts.is_empty() {
        "nothing to do".into()
    } else {
        parts.join("; ")
    }
}

/// Builds the checkout in place, one build at a time, under `nice`.
struct LocalBuilder {
    repo: PathBuf,
    cache: PathBuf,
    disc: PathBuf,
    log: Log,
    tx: Sender<Event>,
    /// The newest generated game's `--out` (the first child's to begin with).
    last_gen: Arc<Mutex<PathBuf>>,
    /// The mtime of the `h2` the running or ready build was copied from.
    last_h2: Arc<Mutex<Option<SystemTime>>>,
    owned: Arc<Owned>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct BuildCtx<'a> {
    repo: &'a Path,
    cache: &'a Path,
    disc: &'a Path,
    last_gen: &'a Mutex<PathBuf>,
    last_h2: &'a Mutex<Option<SystemTime>>,
    step: &'a Step<'a>,
    log: &'a Log,
}

impl LocalBuilder {
    fn build(c: &BuildCtx, id: u64, plan: &Plan) -> Result<Option<Build>, String> {
        let log = c.log;
        log.line(&format!(
            "build {id} ({}): cargo build --release -p cli in {}",
            describe(plan),
            c.repo.display()
        ));
        let target = cargo_target(c.repo);
        let mut cargo = follow::niced("cargo");
        cargo
            .args(["build", "--release", "-p", "cli"])
            .current_dir(c.repo);
        if std::env::var_os("CARGO_BUILD_JOBS").is_none() {
            let half = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1));
            cargo.env("CARGO_BUILD_JOBS", half.to_string());
        }
        c.step.run("cargo", &mut cargo)?;
        let built = target.join("release/h2");
        let built_at = mtime(&built).ok_or_else(|| format!("{} is missing", built.display()))?;

        // The new h2 says whether the last generated game is still its own.
        let prev = c.last_gen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut check = std::process::Command::new(&built);
        check
            .arg("build")
            .arg(c.disc)
            .arg("--out")
            .arg(&prev)
            .arg("--check");
        let (out, regenerated) = if c.step.run("h2 build --check", &mut check).is_ok() {
            (prev, false)
        } else {
            // A new dir, not the running game's: its dylib is mapped. The
            // emit starts from a clone of the last one's C and takes every
            // unchanged object from H2_OBJ_CACHE.
            let out = c.cache.join("gen").join(id.to_string());
            log.line(&format!(
                "build {id}: the game code is out of date, h2 build --out {}",
                out.display()
            ));
            follow::seed_sources(&prev, &out, log);
            let mut emit = follow::niced(&built);
            emit.arg("build").arg(c.disc).arg("--out").arg(&out);
            c.step.run("h2 build", &mut emit)?;
            *c.last_gen.lock().unwrap_or_else(|e| e.into_inner()) = out.clone();
            (out, true)
        };

        let mut last = c.last_h2.lock().unwrap_or_else(|e| e.into_inner());
        if !regenerated && *last == Some(built_at) {
            log.line(&format!("build {id}: h2 and the game code are unchanged"));
            return Ok(None);
        }
        *last = Some(built_at);
        let dir = c.cache.join("builds").join(id.to_string());
        fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let exe = dir.join("h2");
        fs::copy(&built, &exe).map_err(|e| format!("copying {}: {e}", built.display()))?;
        let what = if regenerated {
            "h2 and the game code"
        } else {
            "h2"
        };
        Ok(Some(Build {
            commit: Commit {
                hash: format!("local-{id}"),
                subject: format!("local changes ({what})"),
            },
            exe,
            out,
            subjects: Vec::new(),
        }))
    }
}

impl Builder for LocalBuilder {
    fn start(&mut self, id: u64, plan: &Plan, keep: &[PathBuf]) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.owned.reset();
        let mut keep = keep.to_vec();
        keep.push(
            self.last_gen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        );
        let (repo, cache, disc, log, tx) = (
            self.repo.clone(),
            self.cache.clone(),
            self.disc.clone(),
            self.log.clone(),
            self.tx.clone(),
        );
        let (last_gen, last_h2, owned) = (
            Arc::clone(&self.last_gen),
            Arc::clone(&self.last_h2),
            Arc::clone(&self.owned),
        );
        let plan = plan.clone();
        self.thread = Some(
            std::thread::Builder::new()
                .name("watch-build".into())
                .spawn(move || {
                    follow::prune(&cache.join("builds"), &keep, &log);
                    follow::prune(&cache.join("gen"), &keep, &log);
                    let log_path = cache.join("logs").join(format!("build-{id}.log"));
                    let _ = fs::remove_file(&log_path);
                    let step = Step {
                        log_path: &log_path,
                        owned: &owned,
                    };
                    let ctx = BuildCtx {
                        repo: &repo,
                        cache: &cache,
                        disc: &disc,
                        last_gen: &last_gen,
                        last_h2: &last_h2,
                        step: &step,
                        log: &log,
                    };
                    let r = Self::build(&ctx, id, &plan);
                    match &r {
                        Ok(Some(b)) => log.line(&format!("build {id} ready: {}", b.exe.display())),
                        Ok(None) => {}
                        Err(e) => log.line(&format!(
                            "build {id} failed: {e} (log: {})",
                            log_path.display()
                        )),
                    }
                    let _ = tx.send(Event::Built(r));
                })
                .expect("spawning the builder"),
        );
    }

    fn cancel(&mut self) {
        self.owned.cancel();
    }

    fn shutdown(&mut self) {
        self.owned.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The running game's agent socket: `$H2_AGENT` when it names one, none
/// when it is off, else the per-process socket `h2 play` opens
/// (`agents/<pid>.sock`, docs/runtime/agent-api.md, "Finding a game").
fn game_endpoint(pid: u32) -> Option<agent_proto::Endpoint> {
    use agent_proto::discover;
    match std::env::var(agent_proto::ENV) {
        Ok(v) if discover::is_off(&v) => None,
        Ok(v) if !v.trim().is_empty() => agent_proto::Endpoint::parse(v.trim()).ok(),
        _ => (pid != 0)
            .then(|| agent_proto::Endpoint::from(discover::socket_in(&discover::dir(), pid))),
    }
}

/// Puts a changed rules script into the running game: compiles it (an
/// error is the result), then, when the game runs a script of the same
/// name, `rules.reload` with its source (mode `auto`). `Ok(None)`: the
/// game runs no script of that name, or has no socket.
fn reload_rules(
    repo: &Path,
    rel: &str,
    endpoint: Option<&agent_proto::Endpoint>,
) -> Result<Option<String>, String> {
    use agent_proto::rules::{ReloadParams, ReloadResult, State, method};
    let path = repo.join(rel);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{rel}: {e}")),
    };
    let sources = if rel.starts_with(BUILTIN_RULES) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        rules::builtin::sources_of(name.as_deref().unwrap_or(""), &text)
    } else {
        vec![text]
    };
    let mut compiled = Vec::new();
    for src in sources {
        let ir = rules::compile(&src).map_err(|e| format!("{rel}:{e}"))?;
        compiled.push((ir.header.name, src));
    }
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };
    let rpc = |e: agent_proto::CallError| format!("the game's agent: {e}");
    let mut client =
        agent_proto::Client::connect_with_timeout(endpoint, Some(Duration::from_secs(2)))
            .map_err(|e| format!("the game's agent socket: {e}"))?;
    client
        .set_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let state: State = serde_json::from_value(
        client
            .call(method::STATE, serde_json::json!({}))
            .map_err(rpc)?,
    )
    .map_err(|e| format!("rules.state: {e}"))?;
    let Some(running) = state.script.filter(|_| state.running) else {
        return Ok(None);
    };
    let Some((_, src)) = compiled.into_iter().find(|(name, _)| *name == running) else {
        return Ok(None);
    };
    let params = ReloadParams {
        source: Some(src),
        ..ReloadParams::default()
    };
    let r: ReloadResult = serde_json::from_value(
        client
            .call(
                method::RELOAD,
                serde_json::to_value(params).map_err(|e| e.to_string())?,
            )
            .map_err(rpc)?,
    )
    .map_err(|e| format!("rules.reload: {e}"))?;
    let mut msg = format!("Reloaded {running} ({})", r.mode.name().replace('_', " "));
    if let Some(why) = r.reasons.first() {
        msg += &format!(": {why}");
    }
    Ok(Some(msg))
}

/// The banner for a hot run: the first error, else what was reloaded.
pub fn hot_notice(done: &[String], errors: &[String]) -> Option<Notice> {
    if let Some(e) = errors.first() {
        return Some(Notice {
            kind: NoticeKind::Error,
            text: format!("Reload failed: {}", follow::clip(e, 70)),
        });
    }
    (!done.is_empty()).then(|| Notice {
        kind: NoticeKind::Info,
        text: follow::clip(&done.join("; "), 90),
    })
}

/// Runs hot work on one thread, in order.
struct AgentHot {
    jobs: Option<Sender<(Plan, u32)>>,
    owned: Arc<Owned>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AgentHot {
    /// `serve_web`: the game serves the checkout's web app
    /// (`--live-local`, no `H2_LIVE_WEB`), so rebuilding it is worth it.
    fn start(repo: PathBuf, cache: PathBuf, serve_web: bool, log: Log, tx: Sender<Event>) -> Self {
        let (jobs, rx) = channel::<(Plan, u32)>();
        let owned = Arc::new(Owned::new(follow::BUILD_GRACE));
        let o = Arc::clone(&owned);
        let thread = std::thread::Builder::new()
            .name("watch-hot".into())
            .spawn(move || {
                for (plan, pid) in rx {
                    let (mut done, mut errors) = (Vec::new(), Vec::new());
                    let endpoint = game_endpoint(pid);
                    for rel in &plan.rules {
                        match reload_rules(&repo, rel, endpoint.as_ref()) {
                            Ok(Some(m)) => {
                                log.line(&format!("{rel}: {m}"));
                                done.push(m);
                            }
                            Ok(None) => log.line(&format!(
                                "{rel}: compiles; the game is not running it, nothing to reload"
                            )),
                            Err(e) => {
                                log.line(&format!("{rel}: {e}"));
                                errors.push(e);
                            }
                        }
                    }
                    if plan.web && !serve_web {
                        log.line("web/ changed; the game serves no web app from this checkout (--live-local), so it is not rebuilt");
                    } else if plan.web {
                        match build_web(&repo, &cache, &o, &log) {
                            Ok(()) => done.push("Web app rebuilt \u{2014} reload the page".into()),
                            Err(e) => errors.push(format!("web app: {e}")),
                        }
                    }
                    if o.cancelled() {
                        return;
                    }
                    if tx.send(Event::Hot(hot_notice(&done, &errors))).is_err() {
                        return;
                    }
                }
            })
            .expect("spawning the hot runner");
        Self {
            jobs: Some(jobs),
            owned,
            thread: Some(thread),
        }
    }
}

/// `bun install --frozen-lockfile` and `bun run build` in the checkout's
/// `web/`; the embedded server reads `web/dist` at each request.
fn build_web(repo: &Path, cache: &Path, owned: &Owned, log: &Log) -> Result<(), String> {
    let web = repo.join("web");
    let bun = crate::web_build::find_bun().ok_or("bun is not installed")?;
    let log_path = cache.join("logs/web.log");
    let _ = fs::remove_file(&log_path);
    let step = Step {
        log_path: &log_path,
        owned,
    };
    log.line("web/ changed: bun run build");
    let mut install = std::process::Command::new(&bun);
    install
        .args(["install", "--frozen-lockfile"])
        .current_dir(&web);
    step.run("bun install", &mut install)?;
    let mut build = std::process::Command::new(&bun);
    build.args(["run", "build"]).current_dir(&web);
    step.run("bun run build", &mut build)
}

impl HotRunner for AgentHot {
    fn run(&mut self, plan: &Plan, game_pid: u32) {
        if let Some(j) = &self.jobs {
            let _ = j.send((plan.clone(), game_pid));
        }
    }

    fn shutdown(&mut self) {
        self.owned.cancel();
        self.jobs = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn run(args: Vec<String>) -> Result<()> {
    let w = parse_args(args)?;
    let (disc, out, data) = follow::play_paths(&w.play);
    let disc = disc.with_context(|| format!("no disc given and H2_DISC is not set\n\n{USAGE}"))?;
    let repo = match &w.repo {
        Some(r) => r.clone(),
        None => PathBuf::from(
            follow::git(Path::new("."), &["rev-parse", "--show-toplevel"])
                .context("--repo is needed outside a checkout")?,
        ),
    };
    let repo = fs::canonicalize(&repo).with_context(|| format!("{}", repo.display()))?;
    let git_dir = follow::git(&repo, &["rev-parse", "--absolute-git-dir"])?;
    let git_dir = fs::canonicalize(&git_dir).with_context(|| git_dir.clone())?;
    let cache = match w.cache {
        Some(c) => c,
        None => default_cache(&repo)?,
    };
    for sub in ["logs", "builds", "gen"] {
        let d = cache.join(sub);
        fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
        if !follow::real_dir(&d) {
            bail!(
                "{} is not a plain directory (a symlink?); refusing to use the cache",
                d.display()
            );
        }
    }
    let _lock = follow::lock(&cache, "--watch")?;
    let log = Log::open(&cache.join("logs/watch.log"));
    // The shared object cache, so a new emit only compiles changed C.
    if std::env::var_os("H2_OBJ_CACHE").is_none()
        && let Some(c) = host_dirs::cache()
    {
        // SAFETY: no other thread exists yet.
        unsafe { std::env::set_var("H2_OBJ_CACHE", c.join("obj")) };
    }

    let (tx, rx) = channel();
    follow::watch_signals(tx.clone(), || Event::Interrupt);
    let exe = std::env::current_exe().context("finding this h2")?;
    // Started from the checkout's own h2: a build that leaves it as it is
    // is nothing new.
    let built = cargo_target(&repo).join("release/h2");
    let same = fs::canonicalize(&exe).ok() == fs::canonicalize(&built).ok();
    let last_h2 = if same { mtime(&built) } else { None };
    log.line(&format!(
        "watching {} (quiet {} ms); cache {}",
        repo.display(),
        w.quiet.as_millis(),
        cache.display()
    ));
    let running = Build {
        commit: Commit {
            hash: "local-0".into(),
            subject: "the h2 you started".into(),
        },
        exe,
        out: out.clone(),
        subjects: Vec::new(),
    };
    let (fs_watcher, settle_thread) =
        spawn_watcher(repo.clone(), git_dir, w.quiet, log.clone(), tx.clone())?;
    let serve_web =
        w.play.iter().any(|a| a == "--live-local") && std::env::var_os("H2_LIVE_WEB").is_none();

    let status = cache.join("status");
    let mut builder = LocalBuilder {
        repo: repo.clone(),
        cache: cache.clone(),
        disc,
        log: log.clone(),
        tx: tx.clone(),
        last_gen: Arc::new(Mutex::new(out)),
        last_h2: Arc::new(Mutex::new(last_h2)),
        owned: Arc::new(Owned::new(follow::BUILD_GRACE)),
        thread: None,
    };
    let mut hot = AgentHot::start(repo, cache.clone(), serve_web, log.clone(), tx.clone());
    let mut launcher = ChildLauncher {
        play: w.play,
        data,
        fresh_data: w.fresh_data.then(|| cache.join("data")),
        status: status.clone(),
        log: log.clone(),
        tx,
        exited: Event::ChildExited,
        owned: Arc::new(Owned::new(follow::GAME_GRACE)),
        waiter: None,
        launches: 0,
        last_pid: 0,
    };
    let mut sink = StatusFile {
        path: status,
        log: log.clone(),
    };
    let code = drive(
        Supervisor::new(running),
        &rx,
        &mut builder,
        &mut hot,
        &mut launcher,
        &mut sink,
    );
    // Dropping the watcher closes its channel; the settle thread then ends.
    drop(fs_watcher);
    let _ = settle_thread.join();
    log.line(&format!("exiting ({code})"));
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
