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
//! [`StatusSink`], which the tests replace with fakes. See docs/runtime/app.md
//! "Following a branch".

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use app::{Notice, NoticeKind};

/// The child's exit code for "restart into the ready build" (EX_TEMPFAIL).
pub const RESTART_EXIT: i32 = 75;

const USAGE: &str = "usage:
  h2 play --follow [--repo PATH] [--branch master] [--remote origin] [--no-fetch]
          [--poll SECS] [--fresh-data] [--cache DIR] <h2 play arguments>

Runs `h2 play` as a child and follows a branch: every --poll seconds (45)
it fetches <remote>/<branch> (or reads the local <branch> with --no-fetch),
builds each new commit in its own worktree under the cache dir (default
~/Library/Caches/h2recomp/follow), and shows a banner in the game window.
F5 on \"Update ready\" restarts into the new build; F6 hides the banner.
The game's data dir stays the same unless --fresh-data. Closing the window
ends it. The child installs a mappack.iso beside the disc into its data dir
as `h2 play` does (--no-mappack and H2_MAPPACK=0 reach it).";

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
    /// The startup read of the branch's head (before [`drive`] runs).
    Baseline(Result<Commit, String>),
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
    /// The in-flight build's commit, once a rollback made it unwanted.
    dropped: Option<String>,
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
            dropped: None,
            stopping: false,
        }
    }

    pub fn start(&self) -> Vec<Action> {
        vec![Action::Status(None), Action::Launch(self.running.clone())]
    }

    /// The commit being built or about to be, unless the branch rolled back
    /// past it (that build is not shown, and dropped when it finishes).
    fn pending(&self) -> Option<&Commit> {
        self.building
            .as_ref()
            .filter(|b| self.dropped.as_ref() != Some(&b.hash))
            .or(self.queued.as_ref())
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
            if let Some(b) = self.pending() {
                text += &format!(" (building {}\u{2026})", b.short());
            } else if let Some((c, _)) = &self.failed {
                text += &format!(" ({} failed to build)", c.short());
            }
            return Some(Notice {
                kind: NoticeKind::Ready,
                text,
            });
        }
        if let Some(b) = self.pending() {
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
                // Only the latest head is wanted: whatever was queued is not.
                self.queued = None;
                let is_running = c.hash == self.running.commit.hash;
                if is_running || self.ready.as_ref().is_some_and(|r| r.commit.hash == c.hash) {
                    // Back to a build that exists (a rollback or force-push):
                    // nothing newer is wanted, and an in-flight build is
                    // dropped when it finishes.
                    if is_running {
                        self.ready = None;
                    }
                    self.failed = None;
                    self.dropped = self.building.as_ref().map(|b| b.hash.clone());
                    return vec![Action::Status(self.notice())];
                }
                if self.building.as_ref().is_some_and(|b| b.hash == c.hash) {
                    // Forward again to the build in flight: wanted after all.
                    self.dropped = None;
                    return vec![Action::Status(self.notice())];
                }
                if self.building.is_some() {
                    self.queued = Some(c);
                    return vec![Action::Status(self.notice())];
                }
                self.start_build(c)
            }
            Event::Built(result) => {
                self.building = None;
                if self.stopping {
                    return vec![];
                }
                let hash = match &result {
                    Ok(b) => &b.commit.hash,
                    Err((c, _)) => &c.hash,
                };
                // A build the branch rolled back past is dropped. (One the
                // branch moved forward from is still offered while the
                // newer one builds.)
                if self.dropped.take().as_ref() != Some(hash) {
                    match result {
                        Ok(b) => {
                            self.failed = None;
                            self.ready = Some(b);
                        }
                        Err((c, e)) => self.failed = Some((c, e)),
                    }
                }
                self.after_build()
            }
            Event::Baseline(_) => vec![],
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
    /// Cancel, and return only once the build and everything it started
    /// are gone (escalating to SIGKILL after a grace period).
    fn shutdown(&mut self) {}
}

/// Runs the game; its exit comes back as [`Event::ChildExited`].
pub trait Launcher {
    fn launch(&mut self, build: &Build) -> Result<()>;
    /// Ask the game to stop (it closes like Cmd+Q).
    fn terminate(&mut self);
    /// Return only once the game and its process group are gone: ask
    /// again, then SIGKILL after a grace period, and reap it.
    fn shutdown(&mut self) {}
}

/// Where the banner text goes (the child's status file).
pub trait StatusSink {
    fn set(&mut self, notice: Option<&Notice>);
}

/// Runs the supervisor until it exits; returns the exit code. Nothing it
/// started outlives it: the build and the game are shut down and reaped
/// before it returns.
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
                        builder.shutdown();
                        return 1;
                    }
                }
                Action::TerminateChild => launcher.terminate(),
                Action::CancelBuild => builder.cancel(),
                Action::Exit(code) => {
                    builder.shutdown();
                    launcher.shutdown();
                    return code;
                }
            }
        }
        let Ok(event) = events.recv() else {
            builder.shutdown();
            launcher.shutdown();
            return 1;
        };
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

/// How long one SIGTERM'd build step gets before SIGKILL.
const BUILD_GRACE: Duration = Duration::from_secs(3);
/// How long the game gets to close (its report included) before SIGKILL.
const GAME_GRACE: Duration = Duration::from_secs(15);
/// Local git commands; a fetch has its own, `--fetch-timeout`-like limit.
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// A process the supervisor starts and must be able to stop. Each runs in
/// its own process group with an empty signal mask: the supervisor blocks
/// SIGINT and SIGTERM for its signal thread, and a child would inherit
/// that, so SIGTERM to its group would stay pending. The group is published
/// under the same lock [`Owned::cancel`] takes, so a cancel either stops
/// the spawn or reaches the child.
struct Owned {
    state: Mutex<OwnedState>,
    /// Wakes [`Owned::sleep`] on a cancel.
    cancelled_cv: std::sync::Condvar,
    grace: Duration,
}

#[derive(Default)]
struct OwnedState {
    cancelled: bool,
    pgid: u32,
}

fn prepare(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    // SAFETY: sigemptyset and sigprocmask are async-signal-safe, as needed
    // between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            if libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

impl Owned {
    fn new(grace: Duration) -> Self {
        Self {
            state: Mutex::default(),
            cancelled_cv: std::sync::Condvar::new(),
            grace,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OwnedState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts `cmd` in its own group, unless cancelled.
    fn spawn(&self, cmd: &mut Command) -> io::Result<std::process::Child> {
        prepare(cmd);
        let mut s = self.lock();
        if s.cancelled {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let child = cmd.spawn()?;
        s.pgid = child.id();
        Ok(child)
    }

    fn signal(&self, sig: i32) {
        let s = self.lock();
        if s.pgid != 0 {
            // SAFETY: signalling a process group we started and have not
            // reaped (its leader is at worst a zombie, so the id is ours).
            unsafe { libc::kill(-(s.pgid as i32), sig) };
        }
    }

    /// Stop the running process (SIGTERM, then SIGKILL after the grace
    /// period, from [`Owned::wait`]) and refuse new ones until [`Owned::reset`].
    fn cancel(&self) {
        self.lock().cancelled = true;
        self.cancelled_cv.notify_all();
        self.signal(libc::SIGTERM);
    }

    /// Sleeps for `d` unless cancelled first; returns whether it was.
    fn sleep(&self, d: Duration) -> bool {
        let s = self.lock();
        let (s, _) = self
            .cancelled_cv
            .wait_timeout_while(s, d, |s| !s.cancelled)
            .unwrap_or_else(|e| e.into_inner());
        s.cancelled
    }

    fn reset(&self) {
        self.lock().cancelled = false;
    }

    fn cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// Waits for `child`, the group's leader. After a cancel or `timeout`
    /// the group gets SIGTERM, then SIGKILL once the grace period is up;
    /// once the leader has exited, however it exited, whatever it left in
    /// the group is killed before the leader is reaped (under the lock
    /// [`Owned::signal`] takes, so no signal can reach a reused id).
    /// Returns the status and whether it timed out.
    fn wait(
        &self,
        child: &mut std::process::Child,
        timeout: Option<Duration>,
    ) -> io::Result<(std::process::ExitStatus, bool)> {
        let start = Instant::now();
        let mut stopping: Option<Instant> = None;
        let mut timed_out = false;
        loop {
            // SAFETY: waitid with WNOWAIT only looks; the zombie stays ours.
            let exited = unsafe {
                let mut info: libc::siginfo_t = std::mem::zeroed();
                libc::waitid(
                    libc::P_PID,
                    child.id() as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                ) == 0
                    && info.si_pid() != 0
            };
            if exited {
                break;
            }
            if stopping.is_none() {
                timed_out = timeout.is_some_and(|t| start.elapsed() >= t);
                if timed_out || self.cancelled() {
                    self.signal(libc::SIGTERM);
                    stopping = Some(Instant::now());
                }
            } else if stopping.is_some_and(|t| t.elapsed() >= self.grace) {
                self.signal(libc::SIGKILL);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut s = self.lock();
        // SAFETY: the leader is a zombie we have not reaped, so the group id
        // is still ours; this ends anything it left behind.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let status = child.wait();
        s.pgid = 0;
        Ok((status?, timed_out))
    }
}

/// Runs git in `dir` under `owned`, with a time limit and no prompts.
fn git_owned(dir: &Path, args: &[&str], owned: &Owned, timeout: Duration) -> Result<String> {
    use std::io::Read;
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = owned.spawn(&mut cmd).context("running git")?;
    let reader = |p: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = p {
                let _ = p.read_to_string(&mut s);
            }
            s
        })
    };
    let out = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let (status, timed_out) = owned.wait(&mut child, Some(timeout))?;
    let (out, err) = (
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    );
    if timed_out {
        bail!("git {}: no answer in {timeout:?}", args.join(" "));
    }
    if !status.success() {
        bail!("git {}: {}", args.join(" "), err.trim());
    }
    Ok(out.trim().to_string())
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_owned(dir, args, &Owned::new(BUILD_GRACE), GIT_TIMEOUT)
}

/// What the watcher resolves each poll.
#[derive(Clone)]
struct Watch {
    repo: PathBuf,
    remote: String,
    branch: String,
    fetch: bool,
    fetch_timeout: Duration,
    /// The fetch in flight, so shutdown can stop it.
    owned: Arc<Owned>,
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
    /// A fetch that fails or does not answer in `fetch_timeout` falls back
    /// to the ref as last fetched. Every git step runs under `owned`, so a
    /// cancel stops the one running and refuses the rest.
    fn head(&self, log: &Log) -> Result<Commit> {
        if self.fetch
            && let Err(e) = git_owned(
                &self.repo,
                &["fetch", "--quiet", &self.remote, &self.branch],
                &self.owned,
                self.fetch_timeout,
            )
        {
            log.line(&format!(
                "fetch failed ({e:#}); using the last fetched {}",
                self.reference()
            ));
        }
        let git = |args: &[&str]| git_owned(&self.repo, args, &self.owned, GIT_TIMEOUT);
        let hash = git(&[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", self.reference()),
        ])?;
        let subject = git(&["log", "-1", "--format=%s", &hash])?;
        Ok(Commit { hash, subject })
    }

    /// Reads the head on a thread, so a hanging fetch cannot hold up the
    /// handling of an interrupt; the result arrives as [`Event::Baseline`].
    /// Join the handle before exiting: it reaps the fetch.
    fn spawn_baseline(&self, log: Log, tx: Sender<Event>) -> std::thread::JoinHandle<()> {
        let w = self.clone();
        std::thread::Builder::new()
            .name("follow-baseline".into())
            .spawn(move || {
                let r = w.head(&log).map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::Baseline(r));
            })
            .expect("spawning the baseline read")
    }

    /// Polls every `poll` until `owned` is cancelled. Cancel, then join the
    /// handle before exiting: the poll sleep wakes on the cancel, and a
    /// fetch in flight is stopped and reaped first.
    fn spawn(self, poll: Duration, log: Log, tx: Sender<Event>) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("follow-watch".into())
            .spawn(move || {
                loop {
                    if self.owned.sleep(poll) {
                        return;
                    }
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
            .expect("spawning the watcher")
    }
}

/// Waits for the startup head, handling interrupts meanwhile: `None` if
/// interrupted. Either way it returns only once the read's thread has
/// ended, so the fetch it ran (stopped, escalated to SIGKILL and reaped on
/// an interrupt) cannot outlive the supervisor.
fn wait_baseline(
    rx: &Receiver<Event>,
    watch: &Watch,
    read: std::thread::JoinHandle<()>,
) -> Result<Option<Commit>> {
    loop {
        match rx.recv() {
            Ok(Event::Baseline(r)) => {
                let _ = read.join();
                return r.map(Some).map_err(anyhow::Error::msg);
            }
            Ok(Event::Interrupt) | Err(_) => {
                watch.owned.cancel();
                let _ = read.join();
                return Ok(None);
            }
            Ok(_) => {}
        }
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
    /// The newest generated game's `--out` (the first child's to begin with).
    last_gen: Arc<Mutex<PathBuf>>,
    /// The build step running now.
    owned: Arc<Owned>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Step<'a> {
    log_path: &'a Path,
    owned: &'a Owned,
}

impl Step<'_> {
    /// Runs `cmd` in its own process group, output to the build log.
    fn run(&self, what: &str, cmd: &mut Command) -> Result<(), String> {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path)
            .map_err(|e| format!("opening {}: {e}", self.log_path.display()))?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        cmd.stdin(Stdio::null()).stdout(log).stderr(log2);
        let mut child = match self.owned.spawn(cmd) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err("cancelled".into()),
            Err(e) => return Err(format!("{what}: {e}")),
        };
        let status = self.owned.wait(&mut child, None);
        match status {
            Ok((s, _)) if s.success() => Ok(()),
            _ if self.owned.cancelled() => Err("cancelled".into()),
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

/// `p` is a directory and not a symlink.
fn real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// Removes entries of `dir` (a real directory of the cache) not in `keep`.
/// An entry that is a symlink is unlinked, never followed.
fn prune(dir: &Path, keep: &[PathBuf], log: &Log) {
    if !real_dir(dir) {
        log.line(&format!(
            "not pruning {}: not a plain directory",
            dir.display()
        ));
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if !keep.iter().any(|k| k == &p) {
            let _ = match e.file_type() {
                Ok(t) if t.is_dir() => fs::remove_dir_all(&p),
                _ => fs::remove_file(&p),
            };
        }
    }
}

/// Starts a new generated game from a clone of the last one's C, so the
/// emit rewrites only the files this commit changes (`write_if_changed`)
/// instead of all of them, and unchanged files share their blocks with the
/// last game. Only for an `out` that doesn't exist yet; the emit deletes
/// shards the new generator no longer makes. Best effort: if the clone
/// fails, the partial copy is removed and the emit writes everything.
fn seed_sources(prev: &Path, out: &Path, log: &Log) {
    let (from, to) = (prev.join("src"), out.join("src"));
    if prev == out || !real_dir(&from) || fs::symlink_metadata(out).is_ok() {
        return;
    }
    if let Err(e) = clone_dir(&from, &to) {
        log.line(&format!(
            "not reusing {}: {e}; generating every file",
            from.display()
        ));
        let _ = fs::remove_dir_all(out);
    }
}

/// `cp -cR`: an APFS clone, which takes no space until a file changes.
#[cfg(target_os = "macos")]
fn clone_dir(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let out = Command::new("/bin/cp")
        .arg("-cR")
        .arg(from)
        .arg(to)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cp: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "cp -cR: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn clone_dir(_: &Path, _: &Path) -> Result<(), String> {
    Err("cloning needs APFS".into())
}

fn canonical(p: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// `git rev-parse --git-common-dir` in `dir`, as an absolute path.
fn common_dir(dir: &Path) -> Result<PathBuf, String> {
    let d =
        PathBuf::from(git(dir, &["rev-parse", "--git-common-dir"]).map_err(|e| format!("{e:#}"))?);
    canonical(&if d.is_absolute() { d } else { dir.join(d) })
}

/// Whether `<cache>/checkout` exists and is the follower's own worktree:
/// a real directory (no symlink anywhere at its last step) at exactly that
/// path under the cache, the top of a work tree, registered in the repo's
/// `git worktree list` at that path as a linked (not the main) worktree,
/// sharing the repo's git dir, and with a `.git` file that selects that
/// worktree's own private git dir (`<common>/worktrees/<name>`, whose
/// `gitdir` names this `.git`), never the main git dir or another
/// worktree's. Anything else at that path is refused,
/// never reset, cleaned or removed: the user's work could be there.
fn own_checkout(repo: &Path, cache: &Path) -> Result<bool, String> {
    let path = cache.join("checkout");
    let meta = match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(m) => m,
    };
    let refuse = |why: &str| {
        Err(format!(
            "refusing to use {}: {why}; remove it yourself if it is safe to",
            path.display()
        ))
    };
    if meta.file_type().is_symlink() {
        return refuse("it is a symlink");
    }
    if !meta.is_dir() {
        return refuse("it is not a directory");
    }
    let here = canonical(&path)?;
    if here != canonical(cache)?.join("checkout") {
        return refuse("it resolves outside the cache");
    }
    let list = git(repo, &["worktree", "list", "--porcelain"]).map_err(|e| format!("{e:#}"))?;
    let trees: Vec<&str> = list
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .collect();
    let at = trees
        .iter()
        .position(|t| fs::canonicalize(t).is_ok_and(|t| t == here));
    match at {
        None => return refuse("it is not registered as a worktree of the followed repository"),
        Some(0) => return refuse("it is the repository's main checkout"),
        Some(_) => {}
    }
    let top = git(&path, &["rev-parse", "--show-toplevel"]).map_err(|e| format!("{e:#}"))?;
    if canonical(Path::new(&top))? != here {
        return refuse("it is inside another work tree");
    }
    let common = common_dir(repo)?;
    if common_dir(&path)? != common {
        return refuse("it belongs to another repository");
    }
    // Its .git must be a plain file selecting this worktree's own private
    // git dir, <common>/worktrees/<name>, whose backlink names this .git:
    // pointed at the main git dir (or another worktree's), a checkout here
    // would reset that index and HEAD.
    let dot_git = here.join(".git");
    if !fs::symlink_metadata(&dot_git).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
        return refuse("its .git is not a linked worktree's .git file");
    }
    let own = git(&path, &["rev-parse", "--absolute-git-dir"]).map_err(|e| format!("{e:#}"))?;
    let own = canonical(Path::new(&own))?;
    if own == common {
        return refuse("its .git selects the main repository's git dir");
    }
    if own.parent() != Some(common.join("worktrees").as_path()) {
        return refuse("its .git does not select a linked worktree's own git dir");
    }
    let back =
        fs::read_to_string(own.join("gitdir")).map_err(|e| format!("{}: {e}", own.display()))?;
    if fs::canonicalize(back.trim()).ok() != Some(dot_git) {
        return refuse("its git dir belongs to another worktree");
    }
    Ok(true)
}

impl GitCargoBuilder {
    fn build(
        repo: &Path,
        cache: &Path,
        disc: &Path,
        (commit, since): (&Commit, &Commit),
        last_gen: &Mutex<PathBuf>,
        step: &Step,
        log: &Log,
    ) -> Result<Build, String> {
        let short = commit.short();
        let checkout = cache.join("checkout");
        let e = |e: anyhow::Error| format!("{e:#}");
        if own_checkout(repo, cache)? {
            git(
                &checkout,
                &["checkout", "--quiet", "--detach", "--force", &commit.hash],
            )
            .map_err(e)?;
            git(&checkout, &["clean", "-ffdq"]).map_err(e)?;
        } else {
            // `--force` only re-adds our own registered-but-missing path; no
            // global `worktree prune`, which could drop others' entries.
            git(
                repo,
                &[
                    "worktree",
                    "add",
                    "--force",
                    "--quiet",
                    "--detach",
                    &checkout.to_string_lossy(),
                    &commit.hash,
                ],
            )
            .map_err(e)?;
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

        // The web app `h2 play --live` serves from this checkout. A failure
        // is logged and the game still runs, without it.
        let web = checkout.join("web");
        if web.join("package.json").is_file() {
            let tree = crate::web_build::clean_tree(&web);
            let bun = crate::web_build::find_bun();
            match crate::web_build::ensure(
                &web,
                bun.as_deref(),
                tree.as_deref(),
                &mut |what, cmd| {
                    log.line(&format!("building {short}: {what}"));
                    step.run(what, cmd)
                },
            ) {
                Ok(_) => {}
                Err(e) => log.line(&format!("building {short}: no web app: {e}")),
            }
        }

        // The new h2 says whether the last generated game is still its own
        // (`h2 build --check`: the image, generator sources and module ABI
        // it was stamped with). An h2 too old to know --check fails it.
        let prev = last_gen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut check = Command::new(&built);
        check
            .arg("build")
            .arg(disc)
            .arg("--out")
            .arg(&prev)
            .arg("--check");
        let out = if step.run("h2 build --check", &mut check).is_ok() {
            prev
        } else {
            let out = cache.join("gen").join(short);
            log.line(&format!(
                "building {short}: the generated game is out of date, h2 build --out {}",
                out.display()
            ));
            seed_sources(&prev, &out, log);
            let mut emit = niced(&built);
            emit.arg("build").arg(disc).arg("--out").arg(&out);
            step.run("h2 build", &mut emit)?;
            *last_gen.lock().unwrap_or_else(|e| e.into_inner()) = out.clone();
            out
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
        let (last_gen, owned) = (Arc::clone(&self.last_gen), Arc::clone(&self.owned));
        let (commit, since) = (commit.clone(), since.clone());
        self.thread = Some(
            std::thread::Builder::new()
                .name("follow-build".into())
                .spawn(move || {
                    prune(&cache.join("builds"), &keep, &log);
                    prune(&cache.join("gen"), &keep, &log);
                    let log_path = cache
                        .join("logs")
                        .join(format!("build-{}.log", commit.short()));
                    let _ = fs::remove_file(&log_path);
                    let step = Step {
                        log_path: &log_path,
                        owned: &owned,
                    };
                    let r = Self::build(
                        &repo,
                        &cache,
                        &disc,
                        (&commit, &since),
                        &last_gen,
                        &step,
                        &log,
                    );
                    match &r {
                        Ok(b) => {
                            log.line(&format!("{} ready: {}", commit.short(), b.exe.display()))
                        }
                        Err(e) => log.line(&format!(
                            "{} failed: {e} (log: {})",
                            commit.short(),
                            log_path.display()
                        )),
                    }
                    let _ = tx.send(Event::Built(r.map_err(|e| (commit, e))));
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

/// Starts `h2 play` children with the user's arguments, plus the build's
/// `--out`, the data dir and the status file.
struct ChildLauncher {
    play: Vec<String>,
    data: Option<PathBuf>,
    fresh_data: Option<PathBuf>,
    status: PathBuf,
    log: Log,
    tx: Sender<Event>,
    owned: Arc<Owned>,
    waiter: Option<std::thread::JoinHandle<()>>,
    launches: u32,
    /// The last child's pid (tests check it is gone).
    last_pid: u32,
}

impl Launcher for ChildLauncher {
    fn launch(&mut self, build: &Build) -> Result<()> {
        use std::os::unix::process::ExitStatusExt;
        if let Some(w) = self.waiter.take() {
            let _ = w.join();
        }
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
        // supervisor, which stops the child (and anything it started).
        self.log.line(&format!(
            "starting {} ({}, launch {})",
            build.commit.short(),
            build.exe.display(),
            self.launches
        ));
        let mut child = self
            .owned
            .spawn(&mut cmd)
            .with_context(|| format!("running {}", build.exe.display()))?;
        self.last_pid = child.id();
        let (tx, owned) = (self.tx.clone(), Arc::clone(&self.owned));
        self.waiter = Some(
            std::thread::Builder::new()
                .name("follow-child".into())
                .spawn(move || {
                    let code = match owned.wait(&mut child, None) {
                        Ok((s, _)) => s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)),
                        Err(_) => 1,
                    };
                    let _ = tx.send(Event::ChildExited(code));
                })?,
        );
        Ok(())
    }

    fn terminate(&mut self) {
        self.owned.signal(libc::SIGTERM);
    }

    fn shutdown(&mut self) {
        // A second SIGTERM makes `h2 play` exit at once; SIGKILL follows
        // after the grace period (Owned::wait), then the waiter reaps it.
        self.owned.cancel();
        if let Some(w) = self.waiter.take() {
            let _ = w.join();
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
/// thread starts, so only this thread sees them; every process the
/// supervisor starts gets an empty mask back ([`prepare`]).
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
        let d = cache.join(sub);
        fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
        if !real_dir(&d) {
            bail!(
                "{} is not a plain directory (a symlink?); refusing to use the cache",
                d.display()
            );
        }
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
        fetch_timeout: FETCH_TIMEOUT,
        owned: Arc::new(Owned::new(BUILD_GRACE)),
    };
    let read = watch.spawn_baseline(log.clone(), tx.clone());
    let Some(baseline) = wait_baseline(&rx, &watch, read)? else {
        log.line("interrupted while reading the branch");
        std::process::exit(130);
    };
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
    let fetches = Arc::clone(&watch.owned);
    let watcher = watch.spawn(f.poll, log.clone(), tx.clone());

    let status = cache.join("status");
    let mut builder = GitCargoBuilder {
        repo,
        cache: cache.clone(),
        disc,
        log: log.clone(),
        tx: tx.clone(),
        last_gen: Arc::new(Mutex::new(out)),
        owned: Arc::new(Owned::new(BUILD_GRACE)),
        thread: None,
    };
    let mut launcher = ChildLauncher {
        play: f.play,
        data,
        fresh_data: f.fresh_data.then(|| cache.join("data")),
        status: status.clone(),
        log: log.clone(),
        tx,
        owned: Arc::new(Owned::new(GAME_GRACE)),
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
        &mut launcher,
        &mut sink,
    );
    // A fetch in flight must not outlive the lock either.
    fetches.cancel();
    let _ = watcher.join();
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
        let starts = |a: Vec<Action>| a.iter().any(|a| matches!(a, Action::StartBuild { .. }));
        assert!(!starts(s.handle(Event::Head(commit("ccc")))));
        assert!(!starts(s.handle(Event::Head(commit("ddd")))));
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
    fn rolling_back_to_the_running_commit_drops_the_build_and_the_queue() {
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Head(commit("bbb")));
        s.handle(Event::Head(commit("ccc")));
        // Force-pushed back to what runs: nothing is wanted.
        let a = s.handle(Event::Head(commit("aaa")));
        assert_eq!(a, vec![Action::Status(None)]);
        // The in-flight build finishes: dropped, and ccc is not built.
        assert_eq!(
            s.handle(Event::Built(Ok(build("bbb", &["b"])))),
            vec![Action::Status(None)]
        );
        assert!(s.notice().is_none());
        assert_eq!(
            s.handle(Event::ChildExited(RESTART_EXIT))[1],
            Action::Launch(build("aaa", &[]))
        );
    }

    #[test]
    fn rolling_back_to_the_ready_commit_keeps_it_and_drops_newer_work() {
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Head(commit("bbb")));
        s.handle(Event::Built(Ok(build("bbb", &["b"]))));
        s.handle(Event::Head(commit("ccc")));
        s.handle(Event::Head(commit("ddd")));
        let a = s.handle(Event::Head(commit("bbb")));
        let n = status_of(&a).unwrap().unwrap();
        assert_eq!(n.text, "Update ready: b \u{2014} press F5 to restart");
        // ccc finishes: dropped, ddd is not built, bbb stays on offer.
        let a = s.handle(Event::Built(Ok(build("ccc", &["c"]))));
        assert!(!a.iter().any(|a| matches!(a, Action::StartBuild { .. })));
        assert_eq!(
            s.handle(Event::ChildExited(RESTART_EXIT))[1],
            Action::Launch(build("bbb", &["b"]))
        );
    }

    #[test]
    fn moving_forward_again_to_the_build_in_flight_keeps_it() {
        let mut s = Supervisor::new(build("aaa", &[]));
        s.handle(Event::Head(commit("bbb")));
        s.handle(Event::Head(commit("aaa")));
        assert!(s.notice().is_none());
        s.handle(Event::Head(commit("bbb")));
        assert_eq!(s.notice().unwrap().kind, NoticeKind::Info);
        let a = s.handle(Event::Built(Ok(build("bbb", &["b"]))));
        assert_eq!(status_of(&a).unwrap().unwrap().kind, NoticeKind::Ready);
        assert!(!a.iter().any(|a| matches!(a, Action::StartBuild { .. })));
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

    // ---- The real processes, against scratch repositories (no game). ----

    fn scratch(name: &str) -> PathBuf {
        let r = std::env::temp_dir().join(format!("h2-follow-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&r);
        fs::create_dir_all(&r).unwrap();
        r
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        git(dir, args).unwrap()
    }

    #[test]
    fn a_new_generated_game_starts_from_the_last_ones_sources() {
        let root = scratch("seed-sources");
        let log = Log::open(&root.join("follow.log"));
        let prev = root.join("gen/a");
        fs::create_dir_all(prev.join("src")).unwrap();
        fs::write(prev.join("src/shard_00001.c"), "int a;\n").unwrap();
        fs::write(prev.join("libh2game.dylib"), "old").unwrap();

        let out = root.join("gen/b");
        seed_sources(&prev, &out, &log);
        if cfg!(target_os = "macos") {
            assert_eq!(
                fs::read_to_string(out.join("src/shard_00001.c")).unwrap(),
                "int a;\n"
            );
            // Only the C: the new game's library and stamp are its own build's.
            assert!(!out.join("libh2game.dylib").exists());
            // A clone, not a link: editing the new copy leaves the old one.
            fs::write(out.join("src/shard_00001.c"), "int b;\n").unwrap();
            assert_eq!(
                fs::read_to_string(prev.join("src/shard_00001.c")).unwrap(),
                "int a;\n"
            );
        } else {
            assert!(!out.exists());
        }

        // An out that already exists is the emit's to update, as is.
        let existing = root.join("gen/c");
        fs::create_dir_all(&existing).unwrap();
        seed_sources(&prev, &existing, &log);
        assert!(!existing.join("src").exists());
        // Nothing to start from: no sources, or the same directory.
        let empty = root.join("gen/empty");
        fs::create_dir_all(&empty).unwrap();
        seed_sources(&empty, &root.join("gen/d"), &log);
        assert!(!root.join("gen/d").exists());
        seed_sources(&prev, &prev, &log);
        assert_eq!(fs::read_dir(prev.join("src")).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    /// A repository at `dir` with one commit of `a.txt`; returns its hash.
    fn repo_with_commit(dir: &Path) -> String {
        fs::create_dir_all(dir).unwrap();
        sh(dir, &["init", "-q", "-b", "master"]);
        sh(dir, &["config", "user.email", "t@example.com"]);
        sh(dir, &["config", "user.name", "t"]);
        fs::write(dir.join("a.txt"), "a\n").unwrap();
        sh(dir, &["add", "-A"]);
        sh(dir, &["commit", "-q", "-m", "a"]);
        sh(dir, &["rev-parse", "HEAD"])
    }

    /// Runs the builder's git part only: its build steps are cancelled.
    fn build_git_only(repo: &Path, cache: &Path, hash: &str) -> Result<Build, String> {
        fs::create_dir_all(cache.join("logs")).unwrap();
        let owned = Owned::new(BUILD_GRACE);
        owned.cancel();
        let log_path = cache.join("logs/build.log");
        let step = Step {
            log_path: &log_path,
            owned: &owned,
        };
        let c = Commit {
            hash: hash.into(),
            subject: String::new(),
        };
        let log = Log::open(&cache.join("logs/follow.log"));
        GitCargoBuilder::build(
            repo,
            cache,
            Path::new("/nonexistent"),
            (&c, &c),
            &Mutex::new(cache.join("o")),
            &step,
            &log,
        )
    }

    /// Whether a process whose command line contains `pattern` is alive
    /// (waiting up to 2 s for killed ones to go).
    fn running(pattern: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let found = Command::new("pgrep")
                .args(["-f", pattern])
                .output()
                .is_ok_and(|o| o.status.success());
            if !found || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_checkout_symlinked_to_the_user_checkout_is_refused() {
        let root = scratch("symlink");
        let user = root.join("user");
        let hash = repo_with_commit(&user);
        fs::write(user.join("a.txt"), "edited\n").unwrap();
        fs::write(user.join("untracked.txt"), "mine\n").unwrap();
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        std::os::unix::fs::symlink(&user, cache.join("checkout")).unwrap();
        let e = build_git_only(&user, &cache, &hash).unwrap_err();
        assert!(e.contains("symlink"), "{e}");
        assert_eq!(fs::read_to_string(user.join("a.txt")).unwrap(), "edited\n");
        assert!(user.join("untracked.txt").exists());
        assert_eq!(sh(&user, &["symbolic-ref", "HEAD"]), "refs/heads/master");
        // The user's main checkout sitting at the cache path itself.
        let main = root.join("cache2/checkout");
        let hash = repo_with_commit(&main);
        fs::write(main.join("untracked.txt"), "mine\n").unwrap();
        let e = build_git_only(&main, &root.join("cache2"), &hash).unwrap_err();
        assert!(e.contains("main checkout"), "{e}");
        assert!(main.join("untracked.txt").exists());
        assert_eq!(sh(&main, &["symbolic-ref", "HEAD"]), "refs/heads/master");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_foreign_repository_at_the_checkout_is_refused() {
        let root = scratch("foreign");
        let user = root.join("user");
        let hash = repo_with_commit(&user);
        let cache = root.join("cache");
        let foreign = cache.join("checkout");
        repo_with_commit(&foreign);
        fs::write(foreign.join("a.txt"), "edited\n").unwrap();
        fs::write(foreign.join("untracked.txt"), "x\n").unwrap();
        // Make the requested commit available there too.
        sh(
            &foreign,
            &["fetch", "-q", &user.to_string_lossy(), "master"],
        );
        let e = build_git_only(&user, &cache, &hash).unwrap_err();
        assert!(e.contains("not registered"), "{e}");
        assert_eq!(
            fs::read_to_string(foreign.join("a.txt")).unwrap(),
            "edited\n"
        );
        assert!(foreign.join("untracked.txt").exists());
        assert_eq!(sh(&foreign, &["symbolic-ref", "HEAD"]), "refs/heads/master");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_first_build_leaves_other_worktree_entries_alone() {
        let root = scratch("prune");
        let repo = root.join("repo");
        let hash = repo_with_commit(&repo);
        let other = root.join("other");
        sh(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &other.to_string_lossy(),
                "HEAD",
            ],
        );
        fs::remove_dir_all(&other).unwrap();
        let cache = root.join("cache");
        // Builds stop at cargo (cancelled), after the worktree is added.
        assert_eq!(
            build_git_only(&repo, &cache, &hash).unwrap_err(),
            "cancelled"
        );
        assert!(
            repo.join(".git/worktrees/other").exists(),
            "the unrelated entry survives"
        );
        assert_eq!(own_checkout(&repo, &cache), Ok(true));
        // Our own entry, registered but missing, is added again.
        fs::remove_dir_all(cache.join("checkout")).unwrap();
        assert_eq!(
            build_git_only(&repo, &cache, &hash).unwrap_err(),
            "cancelled"
        );
        assert_eq!(own_checkout(&repo, &cache), Ok(true));
        assert!(repo.join(".git/worktrees/other").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn build_steps_start_with_the_signals_unblocked() {
        let root = scratch("mask");
        let log_path = root.join("log");
        // Block them in this thread, as the supervisor does before any
        // thread starts.
        // SAFETY: plain signal-mask calls on local sets.
        let old = unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            let mut old: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
            old
        };
        let owned = Owned::new(BUILD_GRACE);
        let step = Step {
            log_path: &log_path,
            owned: &owned,
        };
        let r = step.run(
            "sh",
            Command::new("sh").args(["-c", "kill -TERM $$; echo survived"]),
        );
        // SAFETY: restoring this thread's mask.
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
        assert!(r.is_err());
        assert!(
            !fs::read_to_string(&log_path)
                .unwrap_or_default()
                .contains("survived")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_stops_a_step_and_its_whole_group() {
        let root = scratch("cancel");
        let log_path = root.join("log");
        let owned = Arc::new(Owned::new(Duration::from_millis(300)));
        let (o, lp) = (Arc::clone(&owned), log_path.clone());
        // Ignores SIGTERM (and so do its children), so only SIGKILL works.
        let t = std::thread::spawn(move || {
            Step {
                log_path: &lp,
                owned: &o,
            }
            .run(
                "sh",
                Command::new("sh").args(["-c", "trap '' TERM; sleep 4141 & sleep 4141; wait"]),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running("sleep 4141") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let start = Instant::now();
        owned.cancel();
        assert_eq!(t.join().unwrap(), Err("cancelled".to_string()));
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{:?}",
            start.elapsed()
        );
        assert!(
            !running("sleep 4141"),
            "the background sleep was left behind"
        );
        // Cancelled before the spawn: nothing starts.
        let marker = root.join("ran");
        let r = Step {
            log_path: &log_path,
            owned: &owned,
        }
        .run("touch", Command::new("touch").arg(&marker));
        assert_eq!(r, Err("cancelled".to_string()));
        assert!(!marker.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    struct NoBuilder;
    impl Builder for NoBuilder {
        fn start(&mut self, _: &Commit, _: &Commit, _: &[PathBuf]) {
            panic!("no builds here")
        }
        fn cancel(&mut self) {}
    }
    struct NoStatus;
    impl StatusSink for NoStatus {
        fn set(&mut self, _: Option<&Notice>) {}
    }

    #[test]
    fn a_second_interrupt_kills_and_reaps_a_child_that_ignores_sigterm() {
        let root = scratch("orphan");
        let ready = root.join("ready");
        let script = root.join("stubborn-h2");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\ntrap '' TERM\ntouch '{}'\nwhile :; do sleep 0.1; done\n",
                ready.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let (tx, rx) = channel();
        let mut launcher = ChildLauncher {
            play: vec![],
            data: None,
            fresh_data: None,
            status: root.join("status"),
            log: Log::open(&root.join("follow.log")),
            tx: tx.clone(),
            owned: Arc::new(Owned::new(Duration::from_millis(300))),
            waiter: None,
            launches: 0,
            last_pid: 0,
        };
        let r = ready.clone();
        std::thread::spawn(move || {
            while !r.exists() {
                std::thread::sleep(Duration::from_millis(20));
            }
            tx.send(Event::Interrupt).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            tx.send(Event::Interrupt).unwrap();
        });
        let first = Build {
            commit: commit("aaa"),
            exe: script,
            out: root.join("out"),
            subjects: vec![],
        };
        let code = drive(
            Supervisor::new(first),
            &rx,
            &mut NoBuilder,
            &mut launcher,
            &mut NoStatus,
        );
        assert_eq!(code, 130);
        // SAFETY: probing a pid with signal 0.
        let alive = unsafe { libc::kill(launcher.last_pid as i32, 0) } == 0;
        assert!(!alive, "the game child outlived the supervisor");
        fs::remove_dir_all(&root).unwrap();
    }

    /// A clone whose origin "answers" by sleeping: a fetch that hangs.
    /// `secs` is the hang, and also the test's own process marker: tests run
    /// in parallel, so each one looks for `sleep <secs>` with its own value.
    fn hanging_clone(root: &Path, secs: u32) -> (PathBuf, String) {
        let origin = root.join("origin");
        let hash = repo_with_commit(&origin);
        let clone = root.join("clone");
        sh(
            root,
            &[
                "clone",
                "-q",
                &origin.to_string_lossy(),
                &clone.to_string_lossy(),
            ],
        );
        sh(&clone, &["config", "protocol.ext.allow", "always"]);
        sh(
            &clone,
            &["remote", "set-url", "origin", &format!("ext::sleep {secs}")],
        );
        (clone, hash)
    }

    fn watch_of(repo: &Path, fetch_timeout: Duration) -> Watch {
        Watch {
            repo: repo.to_path_buf(),
            remote: "origin".into(),
            branch: "master".into(),
            fetch: true,
            fetch_timeout,
            owned: Arc::new(Owned::new(BUILD_GRACE)),
        }
    }

    #[test]
    fn a_hanging_fetch_times_out_to_the_last_fetched_ref() {
        let root = scratch("fetch-timeout");
        let (clone, hash) = hanging_clone(&root, 4242);
        let log = Log::open(&root.join("follow.log"));
        let start = Instant::now();
        let head = watch_of(&clone, Duration::from_secs(1)).head(&log).unwrap();
        assert_eq!(head.hash, hash);
        // The fetch would hang for 4242 s; a generous bound proves the timeout
        // fired without depending on how loaded the machine is.
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "{:?}",
            start.elapsed()
        );
        assert!(!running("sleep 4242"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_interrupt_during_the_startup_fetch_ends_it_at_once() {
        let root = scratch("fetch-interrupt");
        let (clone, _) = hanging_clone(&root, 4241);
        let watch = watch_of(&clone, Duration::from_secs(600));
        let (tx, rx) = channel();
        let read = watch.spawn_baseline(Log::open(&root.join("follow.log")), tx.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running("sleep 4241") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        tx.send(Event::Interrupt).unwrap();
        let start = Instant::now();
        assert!(wait_baseline(&rx, &watch, read).unwrap().is_none());
        assert!(start.elapsed() < Duration::from_secs(30));
        assert!(!running("sleep 4241"), "the fetch outlived the interrupt");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_checkout_whose_git_file_selects_another_git_dir_is_refused() {
        let root = scratch("gitfile");
        let repo = root.join("repo");
        let hash = repo_with_commit(&repo);
        let other = root.join("other");
        sh(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &other.to_string_lossy(),
                "HEAD",
            ],
        );
        // Work the user has staged in the main checkout.
        fs::write(repo.join("b.txt"), "staged\n").unwrap();
        sh(&repo, &["add", "b.txt"]);
        let cache = root.join("cache");
        assert_eq!(
            build_git_only(&repo, &cache, &hash).unwrap_err(),
            "cancelled"
        );
        assert_eq!(own_checkout(&repo, &cache), Ok(true));
        let dot_git = cache.join("checkout/.git");
        for (target, what) in [
            (repo.join(".git"), "the main git dir"),
            (repo.join(".git/worktrees/other"), "another worktree's"),
        ] {
            fs::write(&dot_git, format!("gitdir: {}\n", target.display())).unwrap();
            let e = build_git_only(&repo, &cache, &hash).unwrap_err();
            assert!(e.starts_with("refusing"), "{what}: {e}");
            assert_eq!(
                sh(&repo, &["diff", "--cached", "--name-only"]),
                "b.txt",
                "{what}"
            );
            assert_eq!(
                sh(&repo, &["symbolic-ref", "HEAD"]),
                "refs/heads/master",
                "{what}"
            );
            assert_eq!(sh(&other, &["rev-parse", "HEAD"]), hash, "{what}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_leader_that_exits_normally_leaves_nothing_in_its_group() {
        let owned = Owned::new(BUILD_GRACE);
        let mut child = owned
            .spawn(Command::new("sh").args(["-c", "sleep 4343 & exit 0"]))
            .unwrap();
        let (status, timed_out) = owned.wait(&mut child, None).unwrap();
        assert!(status.success() && !timed_out);
        assert!(
            !running("sleep 4343"),
            "the leader's background child survived it"
        );
    }

    /// A clone whose origin's transport helper ignores SIGTERM and hangs.
    fn stubborn_clone(root: &Path, secs: u32) -> PathBuf {
        let (clone, _) = hanging_clone(root, secs);
        let helper = root.join("stubborn-remote");
        fs::write(&helper, format!("#!/bin/sh\ntrap '' TERM\nsleep {secs}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        sh(
            &clone,
            &[
                "remote",
                "set-url",
                "origin",
                &format!("ext::{}", helper.display()),
            ],
        );
        clone
    }

    #[test]
    fn an_interrupt_during_the_startup_fetch_reaps_a_stubborn_helper() {
        let root = scratch("fetch-stubborn");
        let clone = stubborn_clone(&root, 4444);
        let watch = watch_of(&clone, Duration::from_secs(600));
        let (tx, rx) = channel();
        let read = watch.spawn_baseline(Log::open(&root.join("follow.log")), tx.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running("sleep 4444") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        tx.send(Event::Interrupt).unwrap();
        let start = Instant::now();
        assert!(wait_baseline(&rx, &watch, read).unwrap().is_none());
        // Back only after SIGKILL and the reap, within the grace period.
        assert!(
            start.elapsed() < BUILD_GRACE + Duration::from_secs(30),
            "{:?}",
            start.elapsed()
        );
        assert!(
            !running("sleep 4444"),
            "the fetch helper outlived wait_baseline"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_watcher_stops_at_once_on_cancel_even_mid_poll_or_mid_fetch() {
        let root = scratch("watch-stop");
        let repo = root.join("repo");
        repo_with_commit(&repo);
        let mut watch = watch_of(&repo, FETCH_TIMEOUT);
        watch.fetch = false;
        let (owned, (tx, _rx)) = (Arc::clone(&watch.owned), channel());
        let log = Log::open(&root.join("follow.log"));
        let watcher = watch.spawn(Duration::from_secs(600), log.clone(), tx.clone());
        std::thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        owned.cancel();
        watcher.join().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{:?}",
            start.elapsed()
        );
        // Mid-fetch: the stubborn helper is killed and reaped first.
        let clone = stubborn_clone(&root, 4545);
        let watch = watch_of(&clone, Duration::from_secs(600));
        let owned = Arc::clone(&watch.owned);
        let watcher = watch.spawn(Duration::from_millis(10), log, tx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !running("sleep 4545") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        owned.cancel();
        watcher.join().unwrap();
        assert!(
            !running("sleep 4545"),
            "the fetch helper outlived the watcher"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// The real builder, watcher, launcher and status file against a
    /// scratch repo whose `cli` crate is a stand-in `h2`: its `build`
    /// writes an empty dylib stamped with `crates/boot/c/boot_bridge.c`
    /// (standing in for boot's generator identity), `build --check`
    /// compares that stamp, and `play` records its arguments. The first
    /// child is a shell script that waits for "ready" and exits for a
    /// restart. Runs cargo, so it is ignored by default:
    /// `cargo test -p cli --bin h2 -- --ignored real_builder`.
    #[test]
    #[ignore]
    fn real_builder_watcher_and_child_on_a_scratch_repo() {
        let root = scratch("it");
        let (repo, cache) = (root.join("repo"), root.join("cache"));
        fs::create_dir_all(repo.join("crates/cli/src")).unwrap();
        fs::create_dir_all(repo.join("crates/boot/c")).unwrap();
        for sub in ["logs", "builds", "gen"] {
            fs::create_dir_all(cache.join(sub)).unwrap();
        }
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
        write("crates/boot/c/boot_bridge.c", "/* bridge v1 */\n");
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "a"]);
        let stand_in = r#"const ID: &str = include_str!("../../boot/c/boot_bridge.c");
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let i = a.iter().position(|x| x == "--out").unwrap();
    let out = std::path::PathBuf::from(&a[i + 1]);
    let stamp = out.join("libh2game.dylib.stamp");
    match a[1].as_str() {
        "build" if a.iter().any(|x| x == "--check") => {
            let ok = std::fs::read_to_string(&stamp).is_ok_and(|s| s == ID);
            std::process::exit(if ok { 0 } else { 3 });
        }
        "build" => {
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("libh2game.dylib"), b"").unwrap();
            std::fs::write(&stamp, ID).unwrap();
        }
        "play" => std::fs::write(out.join("played"), a[1..].join(" ")).unwrap(),
        _ => std::process::exit(2),
    }
}
"#;
        write("crates/cli/src/main.rs", stand_in);
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "cli: the stand-in h2"]);
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
        let mut watch = watch_of(&repo, FETCH_TIMEOUT);
        watch.fetch = false;
        assert_eq!(watch.head(&log).unwrap().hash, b_hash);
        let a = Commit {
            hash: a_hash,
            subject: "a".into(),
        };
        let polls = Arc::clone(&watch.owned);
        let watcher = watch.spawn(Duration::from_millis(200), log.clone(), tx.clone());
        let mut builder = GitCargoBuilder {
            repo: repo.clone(),
            cache: cache.clone(),
            disc: PathBuf::from("/nonexistent/disc.iso"),
            log: log.clone(),
            tx: tx.clone(),
            last_gen: Arc::new(Mutex::new(root.join("out-a"))),
            owned: Arc::new(Owned::new(BUILD_GRACE)),
            thread: None,
        };
        let status = cache.join("status");
        let mut launcher = ChildLauncher {
            play: vec!["disc.iso".into(), "--status".into()],
            data: Some(root.join("data")),
            fresh_data: None,
            status: status.clone(),
            log: log.clone(),
            tx: tx.clone(),
            owned: Arc::new(Owned::new(GAME_GRACE)),
            waiter: None,
            launches: 0,
            last_pid: 0,
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
        let gen_b = cache.join("gen").join(b.short());
        let played = fs::read_to_string(gen_b.join("played")).expect("the new build ran");
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

        let built = |rx: &Receiver<Event>| loop {
            match rx.recv_timeout(Duration::from_secs(300)).unwrap() {
                Event::Built(r) => return r,
                Event::Head(_) => {}
                other => panic!("{other:?}"),
            }
        };
        let commit_all = |msg: &str| {
            sh(&repo, &["commit", "-q", "-am", msg]);
            Commit {
                hash: sh(&repo, &["rev-parse", "HEAD"]),
                subject: msg.into(),
            }
        };
        // A broken commit fails with the compiler's first error.
        write(
            "crates/cli/src/main.rs",
            "fn main() { let x: u32 = \"no\"; }\n",
        );
        let c = commit_all("broken");
        builder.start(&c, &b, &[]);
        let (commit, e) = built(&rx).unwrap_err();
        assert_eq!(commit, c);
        assert!(e.starts_with("error[E0308]"), "{e}");
        // An h2-only change: the new h2 vouches for the generated game.
        write("crates/cli/src/main.rs", &format!("{stand_in}// fixed\n"));
        let d = commit_all("fixed");
        builder.start(
            &d,
            &b,
            &[cache.join("builds").join(b.short()), gen_b.clone()],
        );
        let build_d = built(&rx).unwrap();
        assert_eq!(build_d.out, gen_b, "no emit for an h2-only change");
        assert_eq!(
            build_d.subjects,
            vec!["fixed".to_string(), "broken".to_string()]
        );
        // A bridge-only change: h2's sources are the same, but the stamp is
        // not, so the game is generated again.
        write("crates/boot/c/boot_bridge.c", "/* bridge v2 */\n");
        let e = commit_all("boot: bridge v2");
        // The last game's C, which the new one starts from.
        fs::create_dir_all(gen_b.join("src")).unwrap();
        fs::write(gen_b.join("src/shard_00001.c"), "int a;\n").unwrap();
        builder.start(
            &e,
            &d,
            &[cache.join("builds").join(d.short()), gen_b.clone()],
        );
        let build_e = built(&rx).unwrap();
        let gen_e = cache.join("gen").join(e.short());
        assert_eq!(build_e.out, gen_e);
        assert_eq!(
            fs::read_to_string(gen_e.join("libh2game.dylib.stamp")).unwrap(),
            "/* bridge v2 */\n"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            fs::read_to_string(gen_e.join("src/shard_00001.c")).unwrap(),
            "int a;\n"
        );
        builder.shutdown();
        polls.cancel();
        watcher.join().unwrap();

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
        let fetch = watch_of(&clone, FETCH_TIMEOUT);
        assert_eq!(fetch.head(&log).unwrap().hash, e.hash);
        write("README", "x\n");
        sh(&repo, &["add", "README"]);
        sh(&repo, &["commit", "-q", "-m", "docs: readme"]);
        let f = fetch.head(&log).unwrap();
        assert_eq!(
            (f.hash.as_str(), f.subject.as_str()),
            (sh(&repo, &["rev-parse", "HEAD"]).as_str(), "docs: readme")
        );
        assert_eq!(sh(&clone, &["rev-parse", "HEAD"]), e.hash);
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

    #[test]
    fn no_mappack_reaches_the_child_with_the_same_disc() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let f = parse_args(args("--follow disc/halo2.iso --no-mappack")).unwrap();
        assert_eq!(f.play, args("disc/halo2.iso --no-mappack"));
        let (disc, ..) = play_paths(&f.play);
        assert_eq!(disc, Some(PathBuf::from("disc/halo2.iso")));
    }
}
