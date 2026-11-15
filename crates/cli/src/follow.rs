//! `h2 play --follow`: keep playing while the branch moves.
//!
//! The supervisor runs the game as a child (`h2 play` with the same
//! arguments), watches a branch, and builds each new commit in a worktree
//! it owns under the user cache dir, never the user's checkout. While a
//! build runs or once it is ready, it tells the child through a small
//! status file; the child shows that as a banner (app's overlay). F5 on a
//! ready banner makes the child exit with [`RESTART_EXIT`], and the
//! supervisor starts the new binary, booting fresh. Any other child exit
//! ends the supervisor.
//!
//! [`Supervisor`] is the pure state machine (idle, building, ready, failed,
//! restart); [`drive`] runs it against a [`Builder`], a [`Launcher`] and a
//! [`StatusSink`], which the tests replace with fakes. See docs/app.md
//! "Following a branch".

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use app::{Notice, NoticeKind};

/// The child's exit code for "restart into the ready build" (EX_TEMPFAIL).
pub const RESTART_EXIT: i32 = 75;

/// Paths whose changes mean the generated game must be emitted again: the
/// generator's crates and the runtime ABI header the C is compiled against.
/// Everything else (kernel, HLE, d3d, app, ...) only needs the new `h2`.
pub const GEN_INPUTS: &[&str] = &[
    "crates/xbe/",
    "crates/analyze/",
    "crates/lift/",
    "crates/ir/",
    "crates/emit/",
    "crates/x87/",
    "crates/simd/",
    "crates/runtime/include/",
];

const USAGE: &str = "usage:
  h2 play --follow [--repo PATH] [--branch master] [--remote origin] [--no-fetch]
          [--poll SECS] [--fresh-data] [--cache DIR] <h2 play arguments>

Runs `h2 play` as a child and follows a branch: every --poll seconds (45)
it fetches <remote>/<branch> (or reads the local <branch> with --no-fetch),
builds each new commit in its own worktree under the cache dir (default
~/Library/Caches/h2recomp/follow), and shows a banner in the game window.
F5 on \"Update ready\" restarts into the new build; F6 hides the banner.
The game's data dir stays the same unless --fresh-data. Closing the window
ends it.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.hash[..self.hash.len().min(9)]
    }
}

/// A runnable build: the `h2` binary and the `--out` dir holding the dylib.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub commit: Commit,
    pub exe: PathBuf,
    pub out: PathBuf,
    /// Subjects of the commits since the running build, newest first.
    pub subjects: Vec<String>,
}

#[derive(Debug)]
pub enum Event {
    /// The watched branch's head (sent on every poll; repeats are ignored).
    Head(Commit),
    /// A build finished: the build, or the commit and its first error line.
    Built(Result<Build, (Commit, String)>),
    /// The game exited with this code.
    ChildExited(i32),
    /// Ctrl-C or SIGTERM to the supervisor.
    Interrupt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Build `commit`; `since` is the running commit (for the summary) and
    /// `keep` the build and gen dirs still in use (the rest may be pruned).
    StartBuild {
        commit: Commit,
        since: Commit,
        keep: Vec<PathBuf>,
    },
    Status(Option<Notice>),
    Launch(Build),
    TerminateChild,
    CancelBuild,
    Exit(i32),
}

/// The supervisor's state: the running build, at most one build in
/// progress (newer heads queue, latest wins), the newest ready build and
/// the latest failure.
#[derive(Debug)]
pub struct Supervisor {
    running: Build,
    seen: String,
    building: Option<Commit>,
    queued: Option<Commit>,
    ready: Option<Build>,
    failed: Option<(Commit, String)>,
    stopping: bool,
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "\u{2026}"
    }
}

impl Supervisor {
    /// `running` is what the first child runs; its commit is the baseline,
    /// so only commits after it build.
    pub fn new(running: Build) -> Self {
        Self {
            seen: running.commit.hash.clone(),
            running,
            building: None,
            queued: None,
            ready: None,
            failed: None,
            stopping: false,
        }
    }

    pub fn start(&self) -> Vec<Action> {
        vec![Action::Status(None), Action::Launch(self.running.clone())]
    }

    /// What the banner says now.
    pub fn notice(&self) -> Option<Notice> {
        if let Some(r) = &self.ready {
            let what = match r.subjects.as_slice() {
                [one] => clip(one, 60),
                [] => r.commit.short().to_string(),
                many => format!("{} commits", many.len()),
            };
            let mut text = format!("Update ready: {what} \u{2014} press F5 to restart");
            if let Some(b) = &self.building {
                text += &format!(" (building {}\u{2026})", b.short());
            } else if let Some((c, _)) = &self.failed {
                text += &format!(" ({} failed to build)", c.short());
            }
            return Some(Notice {
                kind: NoticeKind::Ready,
                text,
            });
        }
        if let Some(b) = &self.building {
            return Some(Notice {
                kind: NoticeKind::Info,
                text: format!("Building {}\u{2026}", b.short()),
            });
        }
        self.failed.as_ref().map(|(_, e)| Notice {
            kind: NoticeKind::Error,
            text: format!("Update failed to build: {}", clip(e, 70)),
        })
    }

    fn keep(&self) -> Vec<PathBuf> {
        let mut keep = Vec::new();
        for b in std::iter::once(&self.running).chain(&self.ready) {
            keep.extend(b.exe.parent().map(Path::to_path_buf));
            keep.push(b.out.clone());
        }
        keep
    }

    fn start_build(&mut self, commit: Commit) -> Vec<Action> {
        self.failed = None;
        self.building = Some(commit.clone());
        vec![
            Action::StartBuild {
                commit,
                since: self.running.commit.clone(),
                keep: self.keep(),
            },
            Action::Status(self.notice()),
        ]
    }

    fn after_build(&mut self) -> Vec<Action> {
        match self.queued.take() {
            Some(q) => self.start_build(q),
            None => vec![Action::Status(self.notice())],
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::Head(c) => {
                if self.stopping || c.hash == self.seen {
                    return vec![];
                }
                self.seen = c.hash.clone();
                if c.hash == self.running.commit.hash
                    || self.ready.as_ref().is_some_and(|r| r.commit.hash == c.hash)
                {
                    return vec![];
                }
                if self.building.is_some() {
                    self.queued = Some(c);
                    return vec![];
                }
                self.start_build(c)
            }
            Event::Built(result) => {
                self.building = None;
                if self.stopping {
                    return vec![];
                }
                match result {
                    Ok(b) => {
                        self.failed = None;
                        self.ready = Some(b);
                    }
                    Err((c, e)) => self.failed = Some((c, e)),
                }
                self.after_build()
            }
            Event::ChildExited(code) => {
                if self.stopping {
                    return vec![Action::CancelBuild, Action::Exit(code)];
                }
                if code != RESTART_EXIT {
                    self.stopping = true;
                    return vec![Action::CancelBuild, Action::Exit(code)];
                }
                // F5 with nothing new (the banner was stale) just reboots.
                if let Some(b) = self.ready.take() {
                    self.running = b;
                }
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

/// Starts builds in the background; results come back as [`Event::Built`].
pub trait Builder {
    fn start(&mut self, commit: &Commit, since: &Commit, keep: &[PathBuf]);
    fn cancel(&mut self);
}

/// Runs the game; its exit comes back as [`Event::ChildExited`].
pub trait Launcher {
    fn launch(&mut self, build: &Build) -> Result<()>;
    fn terminate(&mut self);
}

/// Where the banner text goes (the child's status file).
pub trait StatusSink {
    fn set(&mut self, notice: Option<&Notice>);
}

/// Runs the supervisor until it exits; returns the exit code.
pub fn drive(
    mut sup: Supervisor,
    events: &Receiver<Event>,
    builder: &mut dyn Builder,
    launcher: &mut dyn Launcher,
    status: &mut dyn StatusSink,
) -> i32 {
    let mut actions = sup.start();
    loop {
        for a in actions {
            match a {
                Action::StartBuild {
                    commit,
                    since,
                    keep,
                } => builder.start(&commit, &since, &keep),
                Action::Status(n) => status.set(n.as_ref()),
                Action::Launch(b) => {
                    if let Err(e) = launcher.launch(&b) {
                        eprintln!("follow: starting {}: {e:#}", b.exe.display());
                        builder.cancel();
                        return 1;
                    }
                }
                Action::TerminateChild => launcher.terminate(),
                Action::CancelBuild => builder.cancel(),
                Action::Exit(code) => return code,
            }
        }
        let Ok(event) = events.recv() else { return 1 };
        actions = sup.handle(event);
    }
}

/// The status file's text for `notice`: `<kind>\t<text>`, or empty.
pub fn format_status(notice: Option<&Notice>) -> String {
    match notice {
        None => String::new(),
        Some(n) => {
            let kind = match n.kind {
                NoticeKind::Info => "info",
                NoticeKind::Ready => "ready",
                NoticeKind::Error => "error",
            };
            format!("{kind}\t{}\n", n.text.replace(['\n', '\t'], " "))
        }
    }
}

/// Reads [`format_status`] back; anything else is no notice.
pub fn parse_status(text: &str) -> Option<Notice> {
    let (kind, text) = text.lines().next()?.split_once('\t')?;
    let kind = match kind {
        "info" => NoticeKind::Info,
        "ready" => NoticeKind::Ready,
        "error" => NoticeKind::Error,
        _ => return None,
    };
    Some(Notice {
        kind,
        text: text.to_string(),
    })
}

/// The first line of a build log worth showing: rustc's or clang's first
/// error, else the last non-empty line.
pub fn first_error(log: &str) -> String {
    let lines = || log.lines().map(str::trim).filter(|l| !l.is_empty());
    lines()
        .find(|l| l.starts_with("error") || l.contains(": error:") || l.starts_with("Error:"))
        .or_else(|| lines().next_back())
        .unwrap_or("no output")
        .to_string()
}

// ---------------------------------------------------------------------------
// The real thing: git, cargo, the child process and the status file.

/// Flags of `h2 play` that take a value (to find the disc among the args).
const PLAY_VALUE_FLAGS: &[&str] = &[
    "--out",
    "--data",
    "--slices",
    "--transfers",
    "--aspect",
    "--scaling",
    "--input-timeline",
    "--bindings",
    "--follow-status",
];

#[derive(Debug, PartialEq, Eq)]
struct FollowArgs {
    repo: Option<PathBuf>,
    branch: String,
    remote: String,
    fetch: bool,
    poll: Duration,
    fresh_data: bool,
    cache: Option<PathBuf>,
    /// The rest: the child's `h2 play` arguments.
    play: Vec<String>,
}

fn parse_args(args: Vec<String>) -> Result<FollowArgs> {
    let mut f = FollowArgs {
        repo: None,
        branch: "master".into(),
        remote: "origin".into(),
        fetch: true,
        poll: Duration::from_secs(45),
        fresh_data: false,
        cache: None,
        play: Vec::new(),
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--follow" => {}
            "--repo" => f.repo = Some(PathBuf::from(val()?)),
            "--branch" => f.branch = val()?,
            "--remote" => f.remote = val()?,
            "--no-fetch" => f.fetch = false,
            "--poll" => f.poll = Duration::from_secs(val()?.parse::<u64>()?.max(5)),
            "--fresh-data" => f.fresh_data = true,
            "--cache" => f.cache = Some(PathBuf::from(val()?)),
            "--follow-status" => bail!("--follow-status is for the supervisor's child"),
            "-h" | "--help" => bail!("{USAGE}"),
            _ if PLAY_VALUE_FLAGS.contains(&a.as_str()) => {
                let v = val()?;
                f.play.extend([a, v]);
            }
            _ => f.play.push(a),
        }
    }
    Ok(f)
}

/// The disc, `--out` and `--data` the child's arguments name (last wins,
/// as in `h2 play`), with play's defaults.
fn play_paths(play: &[String]) -> (Option<PathBuf>, PathBuf, Option<PathBuf>) {
    let (mut disc, mut out, mut data) = (None, std::env::temp_dir().join("h2boot"), None);
    let mut it = play.iter();
    while let Some(a) = it.next() {
        if PLAY_VALUE_FLAGS.contains(&a.as_str()) {
            let v = it.next().map(PathBuf::from);
            match a.as_str() {
                "--out" => out = v.unwrap_or(out),
                "--data" => data = v,
                _ => {}
            }
        } else if !a.starts_with('-') && disc.is_none() {
            disc = Some(PathBuf::from(a));
        }
    }
    let disc = disc.or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from));
    (disc, out, data)
}

fn default_cache() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    Ok(if cfg!(target_os = "macos") {
        home.join("Library/Caches/h2recomp/follow")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
            .join("h2recomp/follow")
    })
}

/// Appends a line to `<cache>/logs/follow.log` and prints it.
#[derive(Clone)]
struct Log(Arc<Mutex<Option<fs::File>>>);

impl Log {
    fn open(path: &Path) -> Self {
        let f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok();
        Self(Arc::new(Mutex::new(f)))
    }
    fn line(&self, msg: &str) {
        eprintln!("follow: {msg}");
        if let Some(f) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let _ = writeln!(f, "{t} {msg}");
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// What the watcher resolves each poll.
#[derive(Clone)]
struct Watch {
    repo: PathBuf,
    remote: String,
    branch: String,
    fetch: bool,
}

impl Watch {
    fn reference(&self) -> String {
        if self.fetch {
            format!("refs/remotes/{}/{}", self.remote, self.branch)
        } else {
            format!("refs/heads/{}", self.branch)
        }
    }

    /// Fetches (updating only the remote-tracking ref) and reads the head.
    fn head(&self, log: &Log) -> Result<Commit> {
        if self.fetch
            && let Err(e) = git(
                &self.repo,
                &["fetch", "--quiet", &self.remote, &self.branch],
            )
        {
            log.line(&format!(
                "fetch failed ({e:#}); using the last fetched {}",
                self.reference()
            ));
        }
        let hash = git(
            &self.repo,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", self.reference()),
            ],
        )?;
        let subject = git(&self.repo, &["log", "-1", "--format=%s", &hash])?;
        Ok(Commit { hash, subject })
    }

    fn spawn(self, poll: Duration, log: Log, tx: Sender<Event>) {
        std::thread::Builder::new()
            .name("follow-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(poll);
                    match self.head(&log) {
                        Ok(c) => {
                            if tx.send(Event::Head(c)).is_err() {
                                return;
                            }
                        }
                        Err(e) => log.line(&format!("reading {}: {e:#}", self.reference())),
                    }
                }
            })
            .expect("spawning the watcher");
    }
}

/// Builds commits in `<cache>/checkout` (a detached worktree of the repo)
/// with `<cache>/target`, one at a time, under `nice`.
struct GitCargoBuilder {
    repo: PathBuf,
    cache: PathBuf,
    disc: PathBuf,
    log: Log,
    tx: Sender<Event>,
    /// The commit and dir of the newest generated game.
    last_gen: Arc<Mutex<(String, PathBuf)>>,
    /// Process group of the step running now (0: none), for cancel.
    pgid: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
}

struct Step<'a> {
    log_path: &'a Path,
    pgid: &'a AtomicU32,
    cancelled: &'a AtomicBool,
}

impl Step<'_> {
    /// Runs `cmd` niced in its own process group, output to the build log.
    fn run(&self, what: &str, cmd: &mut Command) -> Result<(), String> {
        use std::os::unix::process::CommandExt;
        if self.cancelled.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path)
            .map_err(|e| format!("opening {}: {e}", self.log_path.display()))?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log2)
            .process_group(0)
            .spawn()
            .map_err(|e| format!("{what}: {e}"))?;
        self.pgid.store(child.id(), Ordering::Relaxed);
        let status = child.wait();
        self.pgid.store(0, Ordering::Relaxed);
        match status {
            Ok(s) if s.success() => Ok(()),
            _ if self.cancelled.load(Ordering::Relaxed) => Err("cancelled".into()),
            Ok(_) => Err(first_error(
                &fs::read_to_string(self.log_path).unwrap_or_default(),
            )),
            Err(e) => Err(format!("{what}: {e}")),
        }
    }
}

fn niced(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new("nice");
    c.args(["-n", "15"]).arg(program);
    c
}

/// Removes entries of `dir` not in `keep`.
fn prune(dir: &Path, keep: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if !keep.iter().any(|k| k == &p) {
            let _ = fs::remove_dir_all(&p);
        }
    }
}

impl GitCargoBuilder {
    #[allow(clippy::too_many_arguments)]
    fn build(
        repo: &Path,
        cache: &Path,
        disc: &Path,
        commit: &Commit,
        since: &Commit,
        last_gen: &Mutex<(String, PathBuf)>,
        step: &Step,
        log: &Log,
    ) -> Result<Build, String> {
        let short = commit.short();
        let checkout = cache.join("checkout");
        if checkout.join(".git").exists() {
            git(
                &checkout,
                &["checkout", "--quiet", "--detach", "--force", &commit.hash],
            )
            .map_err(|e| format!("{e:#}"))?;
            git(&checkout, &["clean", "-ffdq"]).map_err(|e| format!("{e:#}"))?;
        } else {
            let _ = fs::remove_dir_all(&checkout);
            git(repo, &["worktree", "prune"]).map_err(|e| format!("{e:#}"))?;
            git(
                repo,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "--detach",
                    &checkout.to_string_lossy(),
                    &commit.hash,
                ],
            )
            .map_err(|e| format!("{e:#}"))?;
        }
        let subjects: Vec<String> = git(
            repo,
            &[
                "log",
                "--format=%s",
                "-n",
                "50",
                &format!("{}..{}", since.hash, commit.hash),
            ],
        )
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();

        log.line(&format!("building {short}: cargo build --release -p cli"));
        let target = cache.join("target");
        let mut cargo = niced("cargo");
        cargo
            .args(["build", "--release", "-p", "cli"])
            .current_dir(&checkout)
            .env("CARGO_TARGET_DIR", &target);
        if std::env::var_os("CARGO_BUILD_JOBS").is_none() {
            let half = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1));
            cargo.env("CARGO_BUILD_JOBS", half.to_string());
        }
        step.run("cargo", &mut cargo)?;
        let built = target.join("release/h2");

        let (gen_commit, gen_out) = last_gen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut diff = vec!["diff", "--name-only", &gen_commit, &commit.hash, "--"];
        diff.extend(GEN_INPUTS);
        let out = match git(repo, &diff) {
            Ok(changed) if changed.is_empty() && gen_out.join(boot::DYLIB).exists() => gen_out,
            _ => {
                let out = cache.join("gen").join(short);
                log.line(&format!(
                    "building {short}: generated code changed, h2 build --out {}",
                    out.display()
                ));
                let mut emit = niced(&built);
                emit.arg("build").arg(disc).arg("--out").arg(&out);
                step.run("h2 build", &mut emit)?;
                *last_gen.lock().unwrap_or_else(|e| e.into_inner()) =
                    (commit.hash.clone(), out.clone());
                out
            }
        };

        let dir = cache.join("builds").join(short);
        fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let exe = dir.join("h2");
        fs::copy(&built, &exe).map_err(|e| format!("copying {}: {e}", built.display()))?;
        Ok(Build {
            commit: commit.clone(),
            exe,
            out,
            subjects,
        })
    }
}

impl Builder for GitCargoBuilder {
    fn start(&mut self, commit: &Commit, since: &Commit, keep: &[PathBuf]) {
        self.cancelled.store(false, Ordering::Relaxed);
        let mut keep = keep.to_vec();
        keep.push(
            self.last_gen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .1
                .clone(),
        );
        let (repo, cache, disc, log, tx) = (
            self.repo.clone(),
            self.cache.clone(),
            self.disc.clone(),
            self.log.clone(),
            self.tx.clone(),
        );
        let (last_gen, pgid, cancelled) = (
            Arc::clone(&self.last_gen),
            Arc::clone(&self.pgid),
            Arc::clone(&self.cancelled),
        );
        let (commit, since) = (commit.clone(), since.clone());
        std::thread::Builder::new()
            .name("follow-build".into())
            .spawn(move || {
                prune(&cache.join("builds"), &keep);
                prune(&cache.join("gen"), &keep);
                let log_path = cache
                    .join("logs")
                    .join(format!("build-{}.log", commit.short()));
                let _ = fs::remove_file(&log_path);
                let step = Step {
                    log_path: &log_path,
                    pgid: &pgid,
                    cancelled: &cancelled,
                };
                let r = Self::build(
                    &repo, &cache, &disc, &commit, &since, &last_gen, &step, &log,
                );
                match &r {
                    Ok(b) => log.line(&format!("{} ready: {}", commit.short(), b.exe.display())),
                    Err(e) => log.line(&format!(
                        "{} failed: {e} (log: {})",
                        commit.short(),
                        log_path.display()
                    )),
                }
                let _ = tx.send(Event::Built(r.map_err(|e| (commit, e))));
            })
            .expect("spawning the builder");
    }

    fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let pgid = self.pgid.load(Ordering::Relaxed);
        if pgid != 0 {
            // SAFETY: signalling a process group we started.
            unsafe { libc::kill(-(pgid as i32), libc::SIGTERM) };
        }
    }
}

/// Starts `h2 play` children with the user's arguments, plus the build's
/// `--out`, the data dir and the status file.
struct ChildLauncher {
    play: Vec<String>,
    data: Option<PathBuf>,
    fresh_data: Option<PathBuf>,
    status: PathBuf,
    log: Log,
    tx: Sender<Event>,
    pid: Arc<AtomicU32>,
    launches: u32,
}

impl Launcher for ChildLauncher {
    fn launch(&mut self, build: &Build) -> Result<()> {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        self.launches += 1;
        let mut cmd = Command::new(&build.exe);
        cmd.arg("play")
            .args(&self.play)
            .arg("--out")
            .arg(&build.out);
        if let Some(root) = &self.fresh_data {
            let d = root.join("current");
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d)?;
            cmd.arg("--data").arg(d);
        } else if let Some(d) = &self.data {
            cmd.arg("--data").arg(d);
        }
        cmd.arg("--follow-status").arg(&self.status);
        // Its own process group: the terminal's Ctrl-C reaches only the
        // supervisor, which stops the child once.
        cmd.process_group(0);
        self.log.line(&format!(
            "starting {} ({}, launch {})",
            build.commit.short(),
            build.exe.display(),
            self.launches
        ));
        let mut child = cmd
            .spawn()
            .with_context(|| format!("running {}", build.exe.display()))?;
        self.pid.store(child.id(), Ordering::Relaxed);
        let (tx, pid) = (self.tx.clone(), Arc::clone(&self.pid));
        std::thread::Builder::new()
            .name("follow-child".into())
            .spawn(move || {
                let code = match child.wait() {
                    Ok(s) => s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)),
                    Err(_) => 1,
                };
                pid.store(0, Ordering::Relaxed);
                let _ = tx.send(Event::ChildExited(code));
            })?;
        Ok(())
    }

    fn terminate(&mut self) {
        let pid = self.pid.load(Ordering::Relaxed);
        if pid != 0 {
            // SAFETY: signalling the child we started.
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }
}

/// Writes the status file atomically (write, then rename).
struct StatusFile {
    path: PathBuf,
    log: Log,
}

impl StatusSink for StatusFile {
    fn set(&mut self, notice: Option<&Notice>) {
        let tmp = self.path.with_extension("tmp");
        let text = format_status(notice);
        if let Err(e) = fs::write(&tmp, &text).and_then(|_| fs::rename(&tmp, &self.path)) {
            self.log
                .line(&format!("writing {}: {e}", self.path.display()));
        }
        if let Some(n) = notice {
            self.log.line(&format!("banner: {}", n.text));
        }
    }
}

/// SIGINT and SIGTERM become [`Event::Interrupt`]. Blocked before any
/// thread starts, so only this thread sees them; children start with an
/// empty mask (std resets it).
fn watch_signals(tx: Sender<Event>) {
    // SAFETY: plain libc signal-mask calls on a local, initialised set.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    std::thread::Builder::new()
        .name("follow-signals".into())
        .spawn(move || {
            loop {
                let mut sig = 0;
                // SAFETY: `set` is a valid set; sigwait only writes `sig`.
                if unsafe { libc::sigwait(&set, &mut sig) } != 0
                    || tx.send(Event::Interrupt).is_err()
                {
                    return;
                }
            }
        })
        .expect("spawning the signal watcher");
}

/// Holds `<cache>/lock` for the supervisor's life: one follower per cache.
fn lock(cache: &Path) -> Result<fs::File> {
    use std::os::fd::AsRawFd;
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cache.join("lock"))?;
    // SAFETY: flock on a file descriptor we own.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("another `h2 play --follow` is using {}", cache.display());
    }
    Ok(f)
}

pub fn run(args: Vec<String>) -> Result<()> {
    let f = parse_args(args)?;
    let (disc, out, data) = play_paths(&f.play);
    let disc = disc.with_context(|| format!("no disc given and H2_DISC is not set\n\n{USAGE}"))?;
    let repo = match &f.repo {
        Some(r) => r.clone(),
        None => PathBuf::from(
            git(Path::new("."), &["rev-parse", "--show-toplevel"])
                .context("--repo is needed outside a checkout")?,
        ),
    };
    let cache = match f.cache {
        Some(c) => c,
        None => default_cache()?,
    };
    for sub in ["logs", "builds", "gen"] {
        fs::create_dir_all(cache.join(sub))
            .with_context(|| format!("creating {}", cache.join(sub).display()))?;
    }
    let _lock = lock(&cache)?;
    let log = Log::open(&cache.join("logs/follow.log"));
    // A persistent object cache, so a new emit only compiles changed C.
    if std::env::var_os("H2_OBJ_CACHE").is_none() {
        // SAFETY: no other thread exists yet.
        unsafe { std::env::set_var("H2_OBJ_CACHE", cache.join("objcache")) };
    }

    let (tx, rx) = channel();
    watch_signals(tx.clone());
    let watch = Watch {
        repo: repo.clone(),
        remote: f.remote,
        branch: f.branch,
        fetch: f.fetch,
    };
    let baseline = watch.head(&log)?;
    log.line(&format!(
        "following {} in {} (every {}s); running {} \"{}\"; cache {}",
        watch.reference(),
        repo.display(),
        f.poll.as_secs(),
        baseline.short(),
        clip(&baseline.subject, 60),
        cache.display()
    ));
    let running = Build {
        commit: baseline.clone(),
        exe: std::env::current_exe().context("finding this h2")?,
        out: out.clone(),
        subjects: Vec::new(),
    };
    watch.spawn(f.poll, log.clone(), tx.clone());

    let status = cache.join("status");
    let mut builder = GitCargoBuilder {
        repo,
        cache: cache.clone(),
        disc,
        log: log.clone(),
        tx: tx.clone(),
        // The first child's --out is taken to match the baseline commit.
        last_gen: Arc::new(Mutex::new((baseline.hash, out))),
        pgid: Arc::default(),
        cancelled: Arc::default(),
    };
    let mut launcher = ChildLauncher {
        play: f.play,
        data,
        fresh_data: f.fresh_data.then(|| cache.join("data")),
        status: status.clone(),
        log: log.clone(),
        tx,
        pid: Arc::default(),
        launches: 0,
    };
    let mut sink = StatusFile {
        path: status,
        log: log.clone(),
    };
    let code = drive(
        Supervisor::new(running),
        &rx,
        &mut builder,
        &mut launcher,
        &mut sink,
    );
    log.line(&format!("exiting ({code})"));
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(h: &str) -> Commit {
        Commit {
            hash: format!("{h}0000000000"),
            subject: format!("subject {h}"),
        }
    }

    fn build(h: &str, subjects: &[&str]) -> Build {
        Build {
            commit: commit(h),
            exe: PathBuf::from(format!("/c/builds/{h}/h2")),
            out: PathBuf::from(format!("/c/gen/{h}")),
            subjects: subjects.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn status_of(actions: &[Action]) -> Option<Option<Notice>> {
        actions.iter().rev().find_map(|a| match a {
            Action::Status(n) => Some(n.clone()),
            _ => None,
        })
    }

    #[test]
    fn idle_until_the_head_moves_then_building() {
        let mut s = Supervisor::new(build("aaa", &[]));
        assert_eq!(
            s.start(),
            vec![Action::Status(None), Action::Launch(build("aaa", &[]))]
        );
        // The baseline again: nothing to do.
        assert!(s.handle(Event::Head(commit("aaa"))).is_empty());
        let a = s.handle(Event::Head(commit("bbb")));
        match &a[0] {
            Action::StartBuild {
                commit: c,
                since,
                keep,
            } => {
                assert_eq!(c, &commit("bbb"));
                assert_eq!(since, &commit("aaa"));
                assert!(keep.contains(&PathBuf::from("/c/builds/aaa")));
                assert!(keep.contains(&PathBuf::from("/c/gen/aaa")));
            }
            other => panic!("{other:?}"),
        }
        let n = status_of(&a).unwrap().unwrap();
        assert_eq!(
            (n.kind, n.text.as_str()),
            (NoticeKind::Info, "Building bbb000000\u{2026}")
        );
        // Polls repeat the same head: no second build.
        assert!(s.handle(Event::Head(commit("bbb"))).is_empty());
    }

    #[test]
    fn ready_then_f5_restarts_into_the_new_build() {
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Head(commit("bbb")));
        let a = s.handle(Event::Built(Ok(build("bbb", &["d3d: fix the fog"]))));
        let n = status_of(&a).unwrap().unwrap();
        assert_eq!(n.kind, NoticeKind::Ready);
        assert_eq!(
            n.text,
            "Update ready: d3d: fix the fog \u{2014} press F5 to restart"
        );
        let a = s.handle(Event::ChildExited(RESTART_EXIT));
        assert_eq!(
            a,
            vec![
                Action::Status(None),
                Action::Launch(build("bbb", &["d3d: fix the fog"]))
            ]
        );
        // The new build is now the baseline for the next one.
        match &s.handle(Event::Head(commit("ccc")))[0] {
            Action::StartBuild { since, keep, .. } => {
                assert_eq!(since, &commit("bbb"));
                assert!(!keep.contains(&PathBuf::from("/c/gen/aaa")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn several_commits_are_counted() {
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Head(commit("bbb")));
        let a = s.handle(Event::Built(Ok(build("bbb", &["one", "two", "three"]))));
        assert!(
            status_of(&a)
                .unwrap()
                .unwrap()
                .text
                .starts_with("Update ready: 3 commits")
        );
    }

    #[test]
    fn failure_keeps_the_game_running_and_waits_for_a_new_head() {
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Head(commit("bbb")));
        let a = s.handle(Event::Built(Err((
            commit("bbb"),
            "error[E0425]: cannot find value `x`".into(),
        ))));
        let n = status_of(&a).unwrap().unwrap();
        assert_eq!(n.kind, NoticeKind::Error);
        assert_eq!(
            n.text,
            "Update failed to build: error[E0425]: cannot find value `x`"
        );
        assert!(
            !a.iter()
                .any(|a| matches!(a, Action::Launch(_) | Action::Exit(_)))
        );
        // The same head is not retried; F5 has nothing to restart into, so
        // it reboots the running build.
        assert!(s.handle(Event::Head(commit("bbb"))).is_empty());
        assert_eq!(
            s.handle(Event::ChildExited(RESTART_EXIT))[1],
            Action::Launch(build("aaa", &[]))
        );
        // A fix builds and clears the failure.
        assert!(matches!(
            s.handle(Event::Head(commit("ccc")))[0],
            Action::StartBuild { .. }
        ));
        assert_eq!(s.notice().unwrap().kind, NoticeKind::Info);
    }

    #[test]
    fn one_build_at_a_time_and_the_latest_queued_head_wins() {
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Head(commit("bbb")));
        assert!(s.handle(Event::Head(commit("ccc"))).is_empty());
        assert!(s.handle(Event::Head(commit("ddd"))).is_empty());
        let a = s.handle(Event::Built(Ok(build("bbb", &["b"]))));
        match &a[0] {
            Action::StartBuild {
                commit: c, keep, ..
            } => {
                assert_eq!(c, &commit("ddd"));
                // The ready build is kept while the next one builds.
                assert!(keep.contains(&PathBuf::from("/c/builds/bbb")));
            }
            other => panic!("{other:?}"),
        }
        // Ready stays offered while the newer one builds.
        let n = status_of(&a).unwrap().unwrap();
        assert_eq!(n.kind, NoticeKind::Ready);
        assert!(
            n.text.ends_with("(building ddd000000\u{2026})"),
            "{}",
            n.text
        );
        // The newer build failing leaves the older ready one on offer.
        let a = s.handle(Event::Built(Err((commit("ddd"), "boom".into()))));
        let n = status_of(&a).unwrap().unwrap();
        assert_eq!(n.kind, NoticeKind::Ready);
        assert!(
            n.text.ends_with("(ddd000000 failed to build)"),
            "{}",
            n.text
        );
    }

    #[test]
    fn closing_the_window_ends_it_and_cancels_the_build() {
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Head(commit("bbb")));
        assert_eq!(
            s.handle(Event::ChildExited(0)),
            vec![Action::CancelBuild, Action::Exit(0)]
        );
        // A late build result does nothing.
        assert!(s.handle(Event::Built(Ok(build("bbb", &[])))).is_empty());
        let mut s = Supervisor::new(build("aaa", &[]));
        assert_eq!(
            s.handle(Event::ChildExited(101)),
            vec![Action::CancelBuild, Action::Exit(101)]
        );
    }

    #[test]
    fn interrupt_stops_the_child_then_exits_with_its_code() {
        let mut s = Supervisor::new(build("aaa", &[]));
        assert_eq!(
            s.handle(Event::Interrupt),
            vec![Action::CancelBuild, Action::TerminateChild]
        );
        assert!(s.handle(Event::Head(commit("bbb"))).is_empty());
        assert_eq!(
            s.handle(Event::ChildExited(0)),
            vec![Action::CancelBuild, Action::Exit(0)]
        );
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Interrupt);
        assert_eq!(
            s.handle(Event::Interrupt),
            vec![Action::CancelBuild, Action::Exit(130)]
        );
    }

    /// Fakes for `drive`. The builder answers at once from a table; each
    /// child sends a scripted list of events when it starts (new heads, or
    /// its window closing); the "user" presses F5 as soon as the banner
    /// offers a restart with nothing else pending, and pushes a fix when a
    /// build fails.
    struct FakeBuilder {
        tx: Sender<Event>,
        results: Vec<(Commit, Result<Build, String>)>,
        started: Vec<String>,
    }
    impl Builder for FakeBuilder {
        fn start(&mut self, commit: &Commit, _: &Commit, _: &[PathBuf]) {
            self.started.push(commit.hash.clone());
            let (_, r) = self
                .results
                .iter()
                .find(|(c, _)| c == commit)
                .unwrap()
                .clone();
            self.tx
                .send(Event::Built(r.map_err(|e| (commit.clone(), e))))
                .unwrap();
        }
        fn cancel(&mut self) {}
    }
    struct FakeChild {
        tx: Sender<Event>,
        scripts: Vec<Vec<Event>>,
        launched: Vec<String>,
    }
    impl Launcher for FakeChild {
        fn launch(&mut self, b: &Build) -> Result<()> {
            self.launched.push(b.commit.hash.clone());
            for e in self.scripts.remove(0) {
                self.tx.send(e).unwrap();
            }
            Ok(())
        }
        fn terminate(&mut self) {}
    }
    struct FakeUser {
        tx: Sender<Event>,
        fix: Option<Commit>,
        seen: Vec<String>,
    }
    impl StatusSink for FakeUser {
        fn set(&mut self, n: Option<&Notice>) {
            let text = format_status(n);
            // Through the file format, as the child reads it.
            match parse_status(&text) {
                Some(n) if n.kind == NoticeKind::Ready && !n.text.contains('(') => {
                    self.tx.send(Event::ChildExited(RESTART_EXIT)).unwrap()
                }
                Some(n) if n.kind == NoticeKind::Error => {
                    if let Some(c) = self.fix.take() {
                        self.tx.send(Event::Head(c)).unwrap();
                    }
                }
                _ => {}
            }
            self.seen.push(text);
        }
    }

    #[test]
    fn drive_builds_offers_restarts_and_ends_with_the_window() {
        let (tx, rx) = channel();
        let mut builder = FakeBuilder {
            tx: tx.clone(),
            results: vec![
                (commit("bbb"), Ok(build("bbb", &["b"]))),
                (commit("ccc"), Err("error: nope".into())),
                (commit("ddd"), Ok(build("ddd", &["d"]))),
            ],
            started: vec![],
        };
        let mut child = FakeChild {
            tx: tx.clone(),
            scripts: vec![
                vec![Event::Head(commit("bbb")), Event::Head(commit("bbb"))],
                vec![Event::Head(commit("ccc"))],
                vec![Event::ChildExited(0)],
            ],
            launched: vec![],
        };
        let mut user = FakeUser {
            tx,
            fix: Some(commit("ddd")),
            seen: vec![],
        };
        let code = drive(
            Supervisor::new(build("aaa", &[])),
            &rx,
            &mut builder,
            &mut child,
            &mut user,
        );
        assert_eq!(code, 0);
        assert_eq!(
            builder.started,
            vec![commit("bbb").hash, commit("ccc").hash, commit("ddd").hash]
        );
        assert_eq!(
            child.launched,
            vec![commit("aaa").hash, commit("bbb").hash, commit("ddd").hash]
        );
        assert!(
            user.seen
                .iter()
                .any(|s| s.starts_with("info\tBuilding ccc"))
        );
        assert!(
            user.seen
                .iter()
                .any(|s| s == "error\tUpdate failed to build: error: nope\n")
        );
        // Each launch starts with the banner cleared.
        assert_eq!(user.seen.iter().filter(|s| s.is_empty()).count(), 3);
    }

    /// The real builder, watcher, launcher and status file against a
    /// scratch repo whose `cli` crate is a stand-in `h2` (its `build` writes
    /// an empty dylib, its `play` records its arguments). The first child
    /// is a shell script that waits for "ready" and exits for a restart.
    /// Runs cargo, so it is ignored by default:
    /// `cargo test -p cli --bin h2 -- --ignored real_builder`.
    #[test]
    #[ignore]
    fn real_builder_watcher_and_child_on_a_scratch_repo() {
        let root = std::env::temp_dir().join(format!("h2-follow-it-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (repo, cache) = (root.join("repo"), root.join("cache"));
        fs::create_dir_all(repo.join("crates/cli/src")).unwrap();
        for sub in ["logs", "builds", "gen"] {
            fs::create_dir_all(cache.join(sub)).unwrap();
        }
        let sh = |dir: &Path, args: &[&str]| git(dir, args).unwrap();
        let write = |rel: &str, text: &str| fs::write(repo.join(rel), text).unwrap();
        sh(
            &root,
            &["init", "-q", "-b", "master", &repo.to_string_lossy()],
        );
        sh(&repo, &["config", "user.email", "t@example.com"]);
        sh(&repo, &["config", "user.name", "t"]);
        write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/cli\"]\nresolver = \"2\"\n",
        );
        write(".gitignore", "/target\nCargo.lock\n");
        write(
            "crates/cli/Cargo.toml",
            "[package]\nname = \"cli\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[[bin]]\nname = \"h2\"\npath = \"src/main.rs\"\n",
        );
        write("crates/cli/src/main.rs", "fn main() {}\n");
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "a"]);
        write(
            "crates/cli/src/main.rs",
            r#"fn main() {
    let a: Vec<String> = std::env::args().collect();
    let i = a.iter().position(|x| x == "--out").unwrap();
    let out = std::path::PathBuf::from(&a[i + 1]);
    match a[1].as_str() {
        "build" => {
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("libh2game.dylib"), b"").unwrap();
        }
        "play" => std::fs::write(out.join("played"), a[1..].join(" ")).unwrap(),
        _ => std::process::exit(2),
    }
}
"#,
        );
        fs::create_dir_all(repo.join("crates/emit")).unwrap();
        write("crates/emit/lib.txt", "changed\n");
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "emit: new codegen"]);
        let b_hash = sh(&repo, &["rev-parse", "HEAD"]);
        let a_hash = sh(&repo, &["rev-parse", "HEAD~1"]);

        // The first child: waits for a ready banner, then asks to restart.
        let script = root.join("first-h2");
        fs::write(
            &script,
            "#!/bin/sh\n# The status file is the last argument.\nfor last; do :; done\ni=0\n\
             while [ $i -lt 1500 ]; do\n  grep -q '^ready' \"$last\" 2>/dev/null && exit 75\n  \
             i=$((i+1)); sleep 0.2\ndone\nexit 0\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let log = Log::open(&cache.join("logs/follow.log"));
        let (tx, rx) = channel();
        let watch = Watch {
            repo: repo.clone(),
            remote: "origin".into(),
            branch: "master".into(),
            fetch: false,
        };
        assert_eq!(watch.head(&log).unwrap().hash, b_hash);
        let a = Commit {
            hash: a_hash.clone(),
            subject: "a".into(),
        };
        watch.spawn(Duration::from_millis(200), log.clone(), tx.clone());
        let mut builder = GitCargoBuilder {
            repo: repo.clone(),
            cache: cache.clone(),
            disc: PathBuf::from("/nonexistent/disc.iso"),
            log: log.clone(),
            tx: tx.clone(),
            last_gen: Arc::new(Mutex::new((a_hash.clone(), root.join("out-a")))),
            pgid: Arc::default(),
            cancelled: Arc::default(),
        };
        let status = cache.join("status");
        let mut launcher = ChildLauncher {
            play: vec!["disc.iso".into(), "--status".into()],
            data: Some(root.join("data")),
            fresh_data: None,
            status: status.clone(),
            log: log.clone(),
            tx: tx.clone(),
            pid: Arc::default(),
            launches: 0,
        };
        let mut sink = StatusFile {
            path: status.clone(),
            log: log.clone(),
        };
        let first = Build {
            commit: a,
            exe: script,
            out: root.join("out-a"),
            subjects: vec![],
        };
        let code = drive(
            Supervisor::new(first),
            &rx,
            &mut builder,
            &mut launcher,
            &mut sink,
        );
        assert_eq!(code, 0);

        let b = Commit {
            hash: b_hash.clone(),
            subject: String::new(),
        };
        let gen_dir = cache.join("gen").join(b.short());
        let played = fs::read_to_string(gen_dir.join("played")).expect("the new build ran");
        assert!(
            played.starts_with("play disc.iso --status --out "),
            "{played}"
        );
        assert!(
            played.contains("--data") && played.ends_with(&status.to_string_lossy().to_string()),
            "{played}"
        );
        assert!(cache.join("builds").join(b.short()).join("h2").exists());
        assert!(cache.join(format!("logs/build-{}.log", b.short())).exists());
        // The user's checkout is untouched; the build ran in the cache's worktree.
        assert_eq!(sh(&repo, &["status", "--porcelain"]), "");
        assert_eq!(sh(&repo, &["rev-parse", "HEAD"]), b_hash);
        assert!(!repo.join("target").exists());
        assert_eq!(sh(&cache.join("checkout"), &["rev-parse", "HEAD"]), b_hash);

        // A broken commit fails with the compiler's first error, and an
        // h2-only change reuses the generated game.
        write(
            "crates/cli/src/main.rs",
            "fn main() { let x: u32 = \"no\"; }\n",
        );
        sh(&repo, &["commit", "-q", "-am", "broken"]);
        let c = Commit {
            hash: sh(&repo, &["rev-parse", "HEAD"]),
            subject: "broken".into(),
        };
        builder.start(&c, &b, &[]);
        loop {
            match rx.recv_timeout(Duration::from_secs(300)).unwrap() {
                Event::Built(Err((commit, e))) => {
                    assert_eq!(commit, c);
                    assert!(e.starts_with("error[E0308]"), "{e}");
                    break;
                }
                Event::Head(_) => {}
                other => panic!("{other:?}"),
            }
        }
        write("crates/cli/src/main.rs", "fn main() {}\n");
        sh(&repo, &["commit", "-q", "-am", "fixed"]);
        let d = Commit {
            hash: sh(&repo, &["rev-parse", "HEAD"]),
            subject: "fixed".into(),
        };
        builder.start(
            &d,
            &b,
            &[cache.join("builds").join(b.short()), gen_dir.clone()],
        );
        loop {
            match rx.recv_timeout(Duration::from_secs(300)).unwrap() {
                Event::Built(Ok(build)) => {
                    assert_eq!(build.out, gen_dir, "no emit for an h2-only change");
                    assert_eq!(
                        build.subjects,
                        vec!["fixed".to_string(), "broken".to_string()]
                    );
                    break;
                }
                Event::Head(_) => {}
                other => panic!("{other:?}"),
            }
        }
        // Fetching: a clone follows its origin's branch, and its own
        // branch and working tree stay where they were.
        let clone = root.join("clone");
        sh(
            &root,
            &[
                "clone",
                "-q",
                &repo.to_string_lossy(),
                &clone.to_string_lossy(),
            ],
        );
        let fetch = Watch {
            repo: clone.clone(),
            remote: "origin".into(),
            branch: "master".into(),
            fetch: true,
        };
        assert_eq!(fetch.head(&log).unwrap().hash, d.hash);
        write("README", "x\n");
        sh(&repo, &["add", "README"]);
        sh(&repo, &["commit", "-q", "-m", "docs: readme"]);
        let e = fetch.head(&log).unwrap();
        assert_eq!(
            (e.hash.as_str(), e.subject.as_str()),
            (sh(&repo, &["rev-parse", "HEAD"]).as_str(), "docs: readme")
        );
        assert_eq!(sh(&clone, &["rev-parse", "HEAD"]), d.hash);
        assert!(!clone.join("README").exists());
        sh(
            &repo,
            &[
                "worktree",
                "remove",
                "--force",
                &cache.join("checkout").to_string_lossy(),
            ],
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn status_file_round_trips() {
        for kind in [NoticeKind::Info, NoticeKind::Ready, NoticeKind::Error] {
            let n = Notice {
                kind,
                text: "Update ready: x \u{2014} press F5".into(),
            };
            assert_eq!(parse_status(&format_status(Some(&n))), Some(n));
        }
        assert_eq!(parse_status(&format_status(None)), None);
        assert_eq!(parse_status("garbage"), None);
        let n = Notice {
            kind: NoticeKind::Error,
            text: "a\nb\tc".into(),
        };
        assert_eq!(
            parse_status(&format_status(Some(&n))).unwrap().text,
            "a b c"
        );
    }

    #[test]
    fn first_error_prefers_the_compiler_error() {
        let log = "   Compiling foo\nwarning: unused\nerror[E0308]: mismatched types\n  --> a.rs:1:1\nerror: aborting\n";
        assert_eq!(first_error(log), "error[E0308]: mismatched types");
        assert_eq!(
            first_error("x.c:3:1: error: expected ';'\n"),
            "x.c:3:1: error: expected ';'"
        );
        assert_eq!(first_error("just\nnoise\n\n"), "noise");
        assert_eq!(first_error(""), "no output");
    }

    #[test]
    fn args_split_between_supervisor_and_child() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let f = parse_args(args("--follow disc.iso --branch boot --out /o --no-fetch --status --poll 60 --data /d --fresh-data")).unwrap();
        assert_eq!(f.branch, "boot");
        assert!(!f.fetch && f.fresh_data);
        assert_eq!(f.poll, Duration::from_secs(60));
        assert_eq!(f.play, args("disc.iso --out /o --status --data /d"));
        let (disc, out, data) = play_paths(&f.play);
        assert_eq!(disc, Some(PathBuf::from("disc.iso")));
        assert_eq!(out, PathBuf::from("/o"));
        assert_eq!(data, Some(PathBuf::from("/d")));
        // A value that looks like a path is not taken for the disc.
        let (disc, ..) = play_paths(&args("--bindings keys.toml disc.iso"));
        assert_eq!(disc, Some(PathBuf::from("disc.iso")));
        assert!(parse_args(args("--follow --follow-status x")).is_err());
    }
}
