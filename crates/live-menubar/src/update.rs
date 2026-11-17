//! Opt-in updates (`[update]` in menubar.toml): follow a branch of a
//! dedicated clone, rebuild what changed, install it and restart the
//! server.
//!
//! One check, [`Updater::check`]:
//!
//! 1. The checkout must be on the configured branch and clean; anything
//!    else is shown in the menu and nothing is touched.
//! 2. `git fetch <remote> <branch>`; when `<remote>/<branch>` moved and
//!    HEAD is its ancestor, `git merge --ff-only` to it. A history that
//!    does not fast-forward is shown, never merged or reset. Before any of
//!    it, a repository that uses Git LFS needs `git lfs` (else Blocked).
//!    A merge that fails is undone (`reset --hard` to the old HEAD and
//!    `clean -fd`), and so is dirt left by one the undo missed
//!    (`State::merge_failed`); the user's own changes are never reset.
//! 3. HEAD against the revision last installed (`update-state.json` in the
//!    install dir): the changed files pick the builds. The server's crates
//!    (live-server and its path dependencies, from `cargo metadata`) build
//!    `h2-live-server`; the CLI's build `h2`; `web/` runs the web build. The
//!    workspace's Cargo.toml, Cargo.lock, rust-toolchain.toml and .cargo/
//!    count for both binaries. Nothing installed yet builds everything.
//! 4. Each binary is copied beside its target as `.new` and renamed over
//!    it; the web build is synced into `web/`. When the art extractor's
//!    version changed, `h2 live assets check` decides whether the art is
//!    stale, and it is re-extracted from the configured disc (else the
//!    menu says so).
//! 5. A loaded server agent is restarted (`kickstart -k`) when its binary,
//!    the web app or the art changed; the menu bar re-executes itself when
//!    `h2` changed and it is the installed `h2`.
//!
//! A revision whose build failed is not retried on the timer, only by
//! Update Now. Builds run under `nice`, one at a time (the update thread
//! runs one check at a time), into a target dir of their own; their
//! output goes to `logs/update.log`.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::{Agent, UpdateConfig};
use crate::launchd::Services;

/// The git operations an update needs.
pub trait Git: Send + Sync {
    fn current_branch(&self) -> Result<String>;
    /// `status --porcelain` lines: changes to tracked or untracked files.
    fn dirty(&self) -> Result<Vec<String>>;
    fn fetch(&self, remote: &str, branch: &str) -> Result<()>;
    fn rev_parse(&self, rev: &str) -> Result<String>;
    /// Whether `a` is an ancestor of `b` (or the same commit).
    fn is_ancestor(&self, a: &str, b: &str) -> Result<bool>;
    fn merge_ff_only(&self, rev: &str) -> Result<()>;
    /// Why a merge cannot be attempted: the repository uses Git LFS and
    /// `git lfs` is not available. None when it is fine.
    fn lfs_problem(&self) -> Option<String>;
    /// Puts the (dedicated) clone back to `rev`: `reset --hard` and
    /// `clean -fd`, after a merge that failed halfway.
    fn restore(&self, rev: &str) -> Result<()>;
    /// Paths changed between two commits.
    fn changed(&self, from: &str, to: &str) -> Result<Vec<String>>;
    /// A file at a revision; None when it does not exist there.
    fn show(&self, rev: &str, path: &str) -> Result<Option<String>>;
}

/// The builds and installs an update runs.
pub trait Builder: Send + Sync {
    /// Repo-relative directories (`crates/x/`) of `package` and its path
    /// dependencies.
    fn crate_dirs(&self, package: &str) -> Result<Vec<String>>;
    /// `cargo build --release -p package --bin bin`: the built binary.
    fn cargo_build(&self, package: &str, bin: &str) -> Result<PathBuf>;
    /// `bun install --frozen-lockfile && bun run build` in web/: its dist.
    fn web_build(&self) -> Result<PathBuf>;
    /// Makes `to` a copy of `from` (rsync -a --delete).
    fn sync_dir(&self, from: &Path, to: &Path) -> Result<()>;
    /// Runs the installed `h2` with `args`: its exit code.
    fn run_h2(&self, h2: &Path, args: &[String]) -> Result<i32>;
}

/// What the last check came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    UpToDate,
    /// Installed a new revision.
    Updated {
        built: Vec<String>,
    },
    /// Nothing done, and why (dirty, not fast-forward, wrong branch).
    Blocked(String),
    Failed(String),
    Paused,
}

/// What `update-state.json` remembers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The revision whose builds are installed.
    pub installed: Option<String>,
    /// A revision whose build failed (not retried on the timer).
    pub failed: Option<String>,
    pub paused: bool,
    /// The HEAD a `git merge --ff-only` failed from. While the clone is
    /// dirty at exactly this revision, the dirt is the failed merge's and
    /// the updater restores it; it is cleared once restored.
    #[serde(default)]
    pub merge_failed: Option<String>,
    /// Unix seconds of the last finished check.
    pub last_check: Option<i64>,
    /// The art extractor changed and nothing re-extracted it.
    pub art_stale: bool,
}

impl State {
    pub fn load(path: &Path) -> State {
        fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// What the menu shows about updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateView {
    /// Building or checking right now ("building the server").
    pub working: Option<String>,
    /// HEAD of the checkout, short.
    pub head: Option<String>,
    pub last_check: Option<i64>,
    pub outcome: Option<Outcome>,
    pub paused: bool,
    pub art_stale: bool,
}

impl UpdateView {
    pub fn from_state(state: &State) -> Self {
        UpdateView {
            working: None,
            head: state.installed.as_deref().map(short),
            last_check: state.last_check,
            outcome: None,
            paused: state.paused,
            art_stale: state.art_stale,
        }
    }
}

pub fn short(rev: &str) -> String {
    rev.chars().take(7).collect()
}

/// Which builds a set of changed paths needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Plan {
    pub server: bool,
    pub h2: bool,
    pub web: bool,
}

impl Plan {
    pub const ALL: Plan = Plan {
        server: true,
        h2: true,
        web: true,
    };

    pub fn any(self) -> bool {
        self.server || self.h2 || self.web
    }
}

fn is_workspace_file(path: &str) -> bool {
    matches!(path, "Cargo.toml" | "Cargo.lock" | "rust-toolchain.toml")
        || path.starts_with(".cargo/")
}

pub fn plan(changed: &[String], server_dirs: &[String], cli_dirs: &[String]) -> Plan {
    let under = |dirs: &[String], p: &str| dirs.iter().any(|d| p.starts_with(d.as_str()));
    let mut plan = Plan::default();
    for p in changed {
        let workspace = is_workspace_file(p);
        plan.server |= workspace || under(server_dirs, p);
        plan.h2 |= workspace || under(cli_dirs, p);
        plan.web |= p.starts_with("web/");
    }
    plan
}

pub const ART_SOURCE: &str = "crates/cli/src/live_art.rs";

/// `pub const EXTRACTOR_VERSION: u32 = N;` in live_art.rs.
pub fn extractor_version(source: &str) -> Option<u32> {
    source.lines().find_map(|l| {
        let rest = l
            .trim()
            .strip_prefix("pub const EXTRACTOR_VERSION: u32 =")?;
        rest.trim().trim_end_matches(';').trim().parse().ok()
    })
}

/// The workspace's path dependencies, from `cargo metadata --no-deps`:
/// `root`'s crate dir and those of everything it depends on by path.
pub fn path_closure(metadata: &str, root: &str, repo: &Path) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Meta {
        packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
        manifest_path: PathBuf,
        dependencies: Vec<Dep>,
    }
    #[derive(Deserialize)]
    struct Dep {
        name: String,
        path: Option<PathBuf>,
        kind: Option<String>,
    }
    let meta: Meta = serde_json::from_str(metadata).context("parsing cargo metadata")?;
    let mut seen: Vec<&str> = Vec::new();
    let mut todo = vec![root];
    while let Some(name) = todo.pop() {
        if seen.contains(&name) {
            continue;
        }
        let Some(pkg) = meta.packages.iter().find(|p| p.name == name) else {
            bail!("cargo metadata has no package {name}");
        };
        seen.push(&pkg.name);
        for d in &pkg.dependencies {
            // Dev dependencies do not go into the binary.
            if d.path.is_some() && d.kind.as_deref() != Some("dev") {
                todo.push(&d.name);
            }
        }
    }
    let mut dirs = Vec::new();
    for name in seen {
        let pkg = meta.packages.iter().find(|p| p.name == name).unwrap();
        let dir = pkg.manifest_path.parent().unwrap_or(Path::new(""));
        let rel = dir.strip_prefix(repo).unwrap_or(dir);
        dirs.push(format!("{}/", rel.to_string_lossy().trim_end_matches('/')));
    }
    dirs.sort();
    Ok(dirs)
}

/// Copies `from` beside `to` as `.new` and renames it over `to`.
pub fn install_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let name = to.file_name().context("no file name")?.to_string_lossy();
    let new = to.with_file_name(format!("{name}.new"));
    fs::copy(from, &new)
        .with_context(|| format!("copying {} to {}", from.display(), new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&new, to).with_context(|| format!("replacing {}", to.display()))?;
    Ok(())
}

/// What [`Updater::check`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub outcome: Outcome,
    /// `h2` was replaced (the menu bar may re-execute).
    pub h2_changed: bool,
}

pub struct Updater<'a> {
    pub cfg: &'a UpdateConfig,
    pub git: &'a dyn Git,
    pub builder: &'a dyn Builder,
    pub services: &'a dyn Services,
    pub server: &'a Agent,
    /// The server's data dir (game art is `game-art/` in it).
    pub data_dir: &'a Path,
    /// Called with what is being done, for the menu.
    pub progress: &'a dyn Fn(&str),
    pub now: i64,
}

impl Updater<'_> {
    /// One check; `force` (Update Now) also runs while paused and retries a
    /// failed revision.
    pub fn check(&self, force: bool) -> Report {
        let path = self.cfg.state_file();
        let mut state = State::load(&path);
        let mut h2_changed = false;
        let outcome = if state.paused && !force {
            Outcome::Paused
        } else {
            match self.check_inner(&mut state, force, &mut h2_changed) {
                Ok(o) => o,
                Err(e) => Outcome::Failed(format!("{e:#}").lines().next().unwrap_or("").into()),
            }
        };
        if outcome != Outcome::Paused {
            state.last_check = Some(self.now);
        }
        let _ = state.save(&path);
        Report {
            outcome,
            h2_changed,
        }
    }

    fn check_inner(
        &self,
        state: &mut State,
        force: bool,
        h2_changed: &mut bool,
    ) -> Result<Outcome> {
        let git = self.git;
        let branch = git.current_branch()?;
        if branch != self.cfg.branch {
            return Ok(Outcome::Blocked(format!(
                "checkout is on {branch}, not {}",
                self.cfg.branch
            )));
        }
        if let Some(why) = git.lfs_problem() {
            return Ok(Outcome::Blocked(why));
        }
        let mut dirty = git.dirty()?;
        if !dirty.is_empty() {
            let head = git.rev_parse("HEAD")?;
            if state.merge_failed.as_deref() == Some(head.as_str()) {
                // Left by our own failed merge (the restore failed too, or
                // an older updater): not the user's. Put it back.
                (self.progress)("restoring the checkout");
                git.restore(&head).context("restoring the checkout")?;
                state.merge_failed = None;
                dirty = git.dirty()?;
            }
        }
        if !dirty.is_empty() {
            return Ok(Outcome::Blocked(format!(
                "checkout has uncommitted changes ({} files)",
                dirty.len()
            )));
        }
        (self.progress)("fetching");
        git.fetch(&self.cfg.remote, &self.cfg.branch)
            .context("git fetch")?;
        let upstream = format!("{}/{}", self.cfg.remote, self.cfg.branch);
        let remote = git.rev_parse(&upstream)?;
        let mut head = git.rev_parse("HEAD")?;
        if head != remote {
            if !git.is_ancestor(&head, &remote)? {
                return Ok(Outcome::Blocked(format!(
                    "{} is not a fast-forward of {} (local commits?)",
                    upstream,
                    short(&head)
                )));
            }
            if let Err(e) = git.merge_ff_only(&remote) {
                // A merge that dies halfway leaves files changed; put the
                // clone back so the next check can try again.
                state.merge_failed = Some(head.clone());
                return Err(match git.restore(&head) {
                    Ok(()) => {
                        state.merge_failed = None;
                        e.context("git merge --ff-only")
                    }
                    Err(r) => e.context(format!(
                        "git merge --ff-only (restoring the checkout failed too: {r:#})"
                    )),
                });
            }
            head = remote;
        }
        if state.installed.as_deref() == Some(head.as_str()) {
            return Ok(Outcome::UpToDate);
        }
        if state.failed.as_deref() == Some(head.as_str()) && !force {
            return Ok(Outcome::Failed(format!(
                "the build of {} failed; see update.log (Update Now retries)",
                short(&head)
            )));
        }

        let old = state.installed.clone();
        let (plan, art_changed) = match &old {
            None => (Plan::ALL, false),
            Some(old) => match git.changed(old, &head) {
                Ok(paths) => {
                    let server = self.builder.crate_dirs("live-server")?;
                    let cli = self.builder.crate_dirs("cli")?;
                    let art = paths.iter().any(|p| p == ART_SOURCE)
                        && git
                            .show(old, ART_SOURCE)?
                            .as_deref()
                            .and_then(extractor_version)
                            != git
                                .show(&head, ART_SOURCE)?
                                .as_deref()
                                .and_then(extractor_version);
                    (plan(&paths, &server, &cli), art)
                }
                // The old revision is gone (history rewritten): all of it.
                Err(_) => (Plan::ALL, false),
            },
        };

        match self.build(plan, art_changed, state, h2_changed) {
            Ok(built) => {
                state.installed = Some(head);
                state.failed = None;
                Ok(Outcome::Updated { built })
            }
            Err(e) => {
                state.failed = Some(head);
                Err(e)
            }
        }
    }

    fn build(
        &self,
        plan: Plan,
        art_changed: bool,
        state: &mut State,
        h2_changed: &mut bool,
    ) -> Result<Vec<String>> {
        let bin = self.cfg.bin_dir();
        let mut built = Vec::new();
        let mut restart = false;
        if plan.server {
            (self.progress)("building the server");
            let out = self.builder.cargo_build("live-server", "h2-live-server")?;
            install_file(&out, &bin.join("h2-live-server"))?;
            built.push("server".into());
            restart = true;
        }
        if plan.h2 {
            (self.progress)("building h2");
            let out = self.builder.cargo_build("cli", "h2")?;
            install_file(&out, &bin.join("h2"))?;
            built.push("h2".into());
            *h2_changed = true;
        }
        if plan.web {
            (self.progress)("building the web app");
            let dist = self.builder.web_build()?;
            self.builder.sync_dir(&dist, &self.cfg.web_dir())?;
            built.push("web".into());
            restart = true;
        }
        let h2 = bin.join("h2");
        if (art_changed || state.art_stale) && h2.exists() {
            (self.progress)("checking game art");
            let out = self.data_dir.join("game-art");
            let out_s = out.to_string_lossy().into_owned();
            let code = self.builder.run_h2(
                &h2,
                &[
                    "live".into(),
                    "assets".into(),
                    "check".into(),
                    "--out".into(),
                    out_s.clone(),
                ],
            )?;
            state.art_stale = code != 0;
            if state.art_stale
                && let Some(disc) = &self.cfg.disc
            {
                (self.progress)("re-extracting game art");
                let mut args: Vec<String> = vec![
                    "live".into(),
                    "assets".into(),
                    "extract".into(),
                    "--disc".into(),
                    disc.to_string_lossy().into_owned(),
                    "--out".into(),
                    out_s,
                ];
                if let Some(m) = &self.cfg.mappack {
                    args.extend(["--mappack".into(), m.to_string_lossy().into_owned()]);
                }
                if self.builder.run_h2(&h2, &args)? != 0 {
                    bail!("h2 live assets extract failed; see update.log");
                }
                state.art_stale = false;
                built.push("art".into());
                restart = true;
            }
        }
        if restart && self.services.state(self.server).is_loaded() {
            (self.progress)("restarting the server");
            self.services.restart(self.server)?;
        }
        Ok(built)
    }
}

// ---- the real git and builds ----

/// Appends a line to the update log.
pub fn log_line(log: &Path, line: &str) {
    if let Some(dir) = log.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log) {
        let _ = writeln!(f, "{line}");
    }
}

/// Runs a command with its output appended to `log`.
fn logged(mut cmd: Command, log: &Path) -> Result<i32> {
    log_line(log, &format!("$ {cmd:?}"));
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = fs::OpenOptions::new().create(true).append(true).open(log)?;
    let status = cmd
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file)
        .status()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    Ok(status.code().unwrap_or(-1))
}

fn captured(mut cmd: Command) -> Result<String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!(
            "{:?} failed: {}",
            cmd.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git -C <repo>`.
pub struct SystemGit {
    pub repo: PathBuf,
    pub log: PathBuf,
    /// git itself, when `[update] git` names it.
    pub git: Option<PathBuf>,
    /// The PATH children get (see [`tool_path`]): git runs its LFS filter
    /// from it.
    pub path: OsString,
}

impl SystemGit {
    fn git(&self) -> Command {
        let mut c = Command::new(self.git.as_deref().unwrap_or(Path::new("git")));
        c.env("PATH", &self.path).arg("-C").arg(&self.repo);
        c
    }

    /// Runs `c`, logging and returning stderr (one line) on failure.
    fn run_logged(&self, mut c: Command) -> Result<()> {
        log_line(&self.log, &format!("$ {c:?}"));
        let out = c
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("running {:?}", c.get_program()))?;
        let err = String::from_utf8_lossy(&out.stderr);
        for line in err.lines() {
            log_line(&self.log, line);
        }
        if out.status.success() {
            return Ok(());
        }
        let one_line = err.lines().map(str::trim).collect::<Vec<_>>().join("; ");
        bail!("exited {}: {one_line}", out.status.code().unwrap_or(-1))
    }
}

/// Directories children should find tools in, however the menu bar was
/// started: a LaunchAgent's PATH is just /usr/bin:/bin:/usr/sbin:/sbin.
/// `extra` (from `[update] path`) first, then the inherited PATH, then
/// Homebrew's two prefixes, `~/.cargo/bin` and `~/.bun/bin`; each once.
pub fn tool_path(extra: &[PathBuf], home: Option<&Path>, inherited: Option<&OsStr>) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut add = |d: PathBuf| {
        if !d.as_os_str().is_empty() && !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    extra.iter().cloned().for_each(&mut add);
    if let Some(p) = inherited {
        std::env::split_paths(p).for_each(&mut add);
    }
    add("/opt/homebrew/bin".into());
    add("/usr/local/bin".into());
    if let Some(h) = home {
        add(h.join(".cargo/bin"));
        add(h.join(".bun/bin"));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// [`tool_path`] from this process's HOME and PATH.
pub fn system_tool_path(extra: &[PathBuf]) -> OsString {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    tool_path(extra, home.as_deref(), std::env::var_os("PATH").as_deref())
}

/// The `PATH` environment entry for the agents the menu bar writes.
pub fn path_entry(extra: &[PathBuf]) -> (String, String) {
    (
        "PATH".into(),
        system_tool_path(extra).to_string_lossy().into_owned(),
    )
}

impl Git for SystemGit {
    fn current_branch(&self) -> Result<String> {
        let mut c = self.git();
        c.args(["rev-parse", "--abbrev-ref", "HEAD"]);
        Ok(captured(c)?.trim().into())
    }

    fn dirty(&self) -> Result<Vec<String>> {
        let mut c = self.git();
        c.args(["status", "--porcelain"]);
        Ok(captured(c)?.lines().map(str::to_string).collect())
    }

    fn fetch(&self, remote: &str, branch: &str) -> Result<()> {
        let mut c = self.git();
        c.args(["fetch", "--quiet", remote, branch]);
        match logged(c, &self.log)? {
            0 => Ok(()),
            code => bail!("git fetch {remote} {branch} exited {code}"),
        }
    }

    fn rev_parse(&self, rev: &str) -> Result<String> {
        let mut c = self.git();
        c.args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")]);
        Ok(captured(c)?.trim().into())
    }

    fn is_ancestor(&self, a: &str, b: &str) -> Result<bool> {
        let status = self
            .git()
            .args(["merge-base", "--is-ancestor", a, b])
            .status()?;
        match status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => bail!("git merge-base --is-ancestor failed"),
        }
    }

    fn merge_ff_only(&self, rev: &str) -> Result<()> {
        let mut c = self.git();
        c.args(["merge", "--ff-only", "--quiet", rev]);
        self.run_logged(c)
    }

    fn lfs_problem(&self) -> Option<String> {
        let attrs = fs::read_to_string(self.repo.join(".gitattributes")).ok()?;
        if !attrs.contains("filter=lfs") {
            return None;
        }
        let mut c = self.git();
        c.args(["lfs", "version"]);
        match self.run_logged(c) {
            Ok(()) => None,
            Err(e) => Some(format!(
                "git-lfs not found (install it: brew install git-lfs; or set [update] path): {e:#}"
            )),
        }
    }

    fn restore(&self, rev: &str) -> Result<()> {
        // Only ever the dedicated update clone itself, never a parent repo.
        if !self.repo.join(".git").exists() {
            bail!("{} is not a git checkout", self.repo.display());
        }
        let mut c = self.git();
        c.args(["reset", "--hard", "--quiet", rev]);
        self.run_logged(c)?;
        let mut c = self.git();
        c.args(["clean", "-fd", "--quiet"]);
        self.run_logged(c)
    }

    fn changed(&self, from: &str, to: &str) -> Result<Vec<String>> {
        let mut c = self.git();
        c.args(["diff", "--name-only", from, to]);
        Ok(captured(c)?.lines().map(str::to_string).collect())
    }

    fn show(&self, rev: &str, path: &str) -> Result<Option<String>> {
        let out = self
            .git()
            .args(["show", &format!("{rev}:{path}")])
            .stdin(Stdio::null())
            .output()?;
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
    }
}

/// Finds a tool on PATH or in a usual install dir (launchd's PATH is short).
pub fn find_tool(name: &str, extra: &[&str]) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(h) = &home {
        dirs.extend(extra.iter().map(|e| h.join(e)));
    }
    dirs.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// cargo and bun under `nice`, output to the update log.
pub struct SystemBuilder {
    pub repo: PathBuf,
    pub target_dir: PathBuf,
    pub log: PathBuf,
    pub cargo: Option<PathBuf>,
    pub bun: Option<PathBuf>,
    /// The PATH children get (see [`tool_path`]).
    pub path: OsString,
}

impl SystemBuilder {
    fn cargo(&self) -> Result<PathBuf> {
        self.cargo
            .clone()
            .or_else(|| find_tool("cargo", &[".cargo/bin"]))
            .context("cargo not found; set [update] cargo in menubar.toml")
    }

    fn nice(&self, program: &Path) -> Command {
        let mut c = Command::new("nice");
        c.args(["-n", "10"]).arg(program).current_dir(&self.repo);
        // cargo and rustup live beside each other; launchd's PATH lacks them.
        let mut dirs: Vec<PathBuf> = program
            .parent()
            .map(Path::to_path_buf)
            .into_iter()
            .collect();
        dirs.extend(std::env::split_paths(&self.path));
        c.env(
            "PATH",
            std::env::join_paths(dirs).unwrap_or_else(|_| self.path.clone()),
        );
        c
    }
}

impl Builder for SystemBuilder {
    fn crate_dirs(&self, package: &str) -> Result<Vec<String>> {
        let mut c = Command::new(self.cargo()?);
        c.args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .env("PATH", &self.path)
        .current_dir(&self.repo);
        path_closure(&captured(c)?, package, &self.repo)
    }

    fn cargo_build(&self, package: &str, bin: &str) -> Result<PathBuf> {
        let mut c = self.nice(&self.cargo()?);
        c.args(["build", "--release", "-p", package, "--bin", bin])
            .env("CARGO_TARGET_DIR", &self.target_dir);
        match logged(c, &self.log)? {
            0 => Ok(self.target_dir.join("release").join(bin)),
            code => bail!("cargo build -p {package} --bin {bin} exited {code}; see update.log"),
        }
    }

    fn web_build(&self) -> Result<PathBuf> {
        let bun = self
            .bun
            .clone()
            .or_else(|| find_tool("bun", &[".bun/bin"]))
            .context("bun not found; set [update] bun in menubar.toml")?;
        let web = self.repo.join("web");
        for args in [&["install", "--frozen-lockfile"][..], &["run", "build"][..]] {
            let mut c = self.nice(&bun);
            c.args(args).current_dir(&web);
            let code = logged(c, &self.log)?;
            if code != 0 {
                bail!("bun {} exited {code}; see update.log", args.join(" "));
            }
        }
        Ok(web.join("dist"))
    }

    fn sync_dir(&self, from: &Path, to: &Path) -> Result<()> {
        fs::create_dir_all(to)?;
        let mut c = Command::new("rsync");
        c.env("PATH", &self.path)
            .arg("-a")
            .arg("--delete")
            .arg(format!("{}/", from.display()))
            .arg(format!("{}/", to.display()));
        match logged(c, &self.log)? {
            0 => Ok(()),
            code => bail!("rsync exited {code}"),
        }
    }

    fn run_h2(&self, h2: &Path, args: &[String]) -> Result<i32> {
        let mut c = self.nice(h2);
        c.args(args);
        logged(c, &self.log)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::launchd::Launchd;
    use crate::launchd::fake::FakeLaunchd;

    /// One step's (parent, commit) and the paths it changed.
    pub type Diff = ((String, String), Vec<String>);

    /// A repository in memory: commits are names, each with its changed
    /// files and its live_art.rs.
    #[derive(Default)]
    pub struct FakeGit {
        pub branch: Mutex<String>,
        pub dirty: Mutex<Vec<String>>,
        pub head: Mutex<String>,
        /// What the remote branch is at after a fetch.
        pub remote: Mutex<String>,
        /// Commit -> parent, for ancestry.
        pub parents: Mutex<Vec<(String, String)>>,
        /// (from, to) -> changed paths; missing means the diff fails.
        pub diffs: Mutex<Vec<Diff>>,
        /// commit -> extractor version
        pub art: Mutex<Vec<(String, u32)>>,
        pub calls: Mutex<Vec<String>>,
        pub fetch_fails: Mutex<bool>,
        /// `git lfs` is unavailable (reason shown).
        pub lfs_missing: Mutex<bool>,
        /// The next merge dies halfway: the tree is left dirty, HEAD stays.
        pub merge_fails: Mutex<bool>,
        /// Restoring fails too.
        pub restore_fails: Mutex<bool>,
    }

    impl FakeGit {
        pub fn new(head: &str) -> Arc<Self> {
            let g = FakeGit::default();
            *g.branch.lock().unwrap() = "master".into();
            *g.head.lock().unwrap() = head.into();
            *g.remote.lock().unwrap() = head.into();
            Arc::new(g)
        }

        /// A new remote commit after `parent`, changing `paths`.
        pub fn push(&self, parent: &str, commit: &str, paths: &[&str]) {
            self.parents
                .lock()
                .unwrap()
                .push((commit.into(), parent.into()));
            self.diffs.lock().unwrap().push((
                (parent.into(), commit.into()),
                paths.iter().map(|p| p.to_string()).collect(),
            ));
            *self.remote.lock().unwrap() = commit.into();
        }

        fn ancestor(&self, a: &str, b: &str) -> bool {
            let parents = self.parents.lock().unwrap();
            let mut at = b.to_string();
            loop {
                if at == a {
                    return true;
                }
                match parents.iter().find(|(c, _)| *c == at) {
                    Some((_, p)) => at = p.clone(),
                    None => return false,
                }
            }
        }

        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut self.calls.lock().unwrap())
        }
    }

    impl Git for FakeGit {
        fn current_branch(&self) -> Result<String> {
            Ok(self.branch.lock().unwrap().clone())
        }
        fn dirty(&self) -> Result<Vec<String>> {
            Ok(self.dirty.lock().unwrap().clone())
        }
        fn fetch(&self, remote: &str, branch: &str) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("fetch {remote} {branch}"));
            if *self.fetch_fails.lock().unwrap() {
                bail!("could not resolve host");
            }
            Ok(())
        }
        fn rev_parse(&self, rev: &str) -> Result<String> {
            Ok(match rev {
                "HEAD" => self.head.lock().unwrap().clone(),
                "origin/master" => self.remote.lock().unwrap().clone(),
                other => bail!("unknown rev {other}"),
            })
        }
        fn is_ancestor(&self, a: &str, b: &str) -> Result<bool> {
            Ok(self.ancestor(a, b))
        }
        fn lfs_problem(&self) -> Option<String> {
            (*self.lfs_missing.lock().unwrap()).then(|| "git-lfs not found".to_string())
        }
        fn restore(&self, rev: &str) -> Result<()> {
            self.calls.lock().unwrap().push(format!("restore {rev}"));
            if *self.restore_fails.lock().unwrap() {
                bail!("index.lock exists");
            }
            *self.head.lock().unwrap() = rev.into();
            self.dirty.lock().unwrap().clear();
            Ok(())
        }
        fn merge_ff_only(&self, rev: &str) -> Result<()> {
            self.calls.lock().unwrap().push(format!("merge {rev}"));
            if *self.merge_fails.lock().unwrap() {
                *self.dirty.lock().unwrap() = vec![" M a".into(); 105];
                bail!("exited 128: git-lfs: command not found; fatal: the remote end hung up");
            }
            *self.head.lock().unwrap() = rev.into();
            Ok(())
        }
        fn changed(&self, from: &str, to: &str) -> Result<Vec<String>> {
            // Walk back from `to` to `from`, collecting each step's files.
            let mut out = Vec::new();
            let mut at = to.to_string();
            while at != from {
                let parent = self
                    .parents
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(c, _)| *c == at)
                    .map(|(_, p)| p.clone())
                    .ok_or_else(|| anyhow::anyhow!("bad revision {from}"))?;
                let diffs = self.diffs.lock().unwrap();
                let step = diffs
                    .iter()
                    .find(|((f, t), _)| *f == parent && *t == at)
                    .map(|(_, p)| p.clone())
                    .unwrap_or_default();
                out.extend(step);
                at = parent;
            }
            Ok(out)
        }
        fn show(&self, rev: &str, path: &str) -> Result<Option<String>> {
            assert_eq!(path, ART_SOURCE);
            let art = self.art.lock().unwrap();
            // The newest version at or before `rev`.
            let mut at = rev.to_string();
            loop {
                if let Some((_, v)) = art.iter().find(|(c, _)| *c == at) {
                    return Ok(Some(format!(
                        "//! art\npub const EXTRACTOR_VERSION: u32 = {v};\n"
                    )));
                }
                let parents = self.parents.lock().unwrap();
                match parents.iter().find(|(c, _)| *c == at) {
                    Some((_, p)) => at = p.clone(),
                    None => return Ok(None),
                }
            }
        }
    }

    /// Builds that write little files into a temp dir.
    pub struct FakeBuilder {
        pub out: PathBuf,
        pub calls: Mutex<Vec<String>>,
        pub fail: Mutex<Option<String>>,
        /// `assets check` exit code.
        pub art_check: Mutex<i32>,
        pub counter: Mutex<u32>,
    }

    impl FakeBuilder {
        pub fn new(out: PathBuf) -> Arc<Self> {
            fs::create_dir_all(&out).unwrap();
            Arc::new(FakeBuilder {
                out,
                calls: Mutex::new(Vec::new()),
                fail: Mutex::new(None),
                art_check: Mutex::new(0),
                counter: Mutex::new(0),
            })
        }

        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut self.calls.lock().unwrap())
        }

        fn step(&self, what: String) -> Result<u32> {
            self.calls.lock().unwrap().push(what.clone());
            if self.fail.lock().unwrap().as_deref() == Some(what.as_str()) {
                bail!("{what} failed");
            }
            let mut n = self.counter.lock().unwrap();
            *n += 1;
            Ok(*n)
        }
    }

    impl Builder for FakeBuilder {
        fn crate_dirs(&self, package: &str) -> Result<Vec<String>> {
            Ok(match package {
                "live-server" => vec!["crates/live-proto/".into(), "crates/live-server/".into()],
                "cli" => vec![
                    "crates/cli/".into(),
                    "crates/live-proto/".into(),
                    "crates/live-server/".into(),
                    "crates/runtime/".into(),
                ],
                _ => bail!("no package {package}"),
            })
        }
        fn cargo_build(&self, package: &str, bin: &str) -> Result<PathBuf> {
            let n = self.step(format!("cargo {package} {bin}"))?;
            let p = self.out.join(bin);
            fs::write(&p, format!("{bin} build {n}"))?;
            Ok(p)
        }
        fn web_build(&self) -> Result<PathBuf> {
            let n = self.step("web".into())?;
            let dist = self.out.join("dist");
            fs::create_dir_all(&dist)?;
            fs::write(dist.join("index.html"), format!("web {n}"))?;
            Ok(dist)
        }
        fn sync_dir(&self, from: &Path, to: &Path) -> Result<()> {
            self.step("sync".into())?;
            fs::create_dir_all(to)?;
            for e in fs::read_dir(from)? {
                let e = e?;
                fs::copy(e.path(), to.join(e.file_name()))?;
            }
            Ok(())
        }
        fn run_h2(&self, _: &Path, args: &[String]) -> Result<i32> {
            let what = format!("h2 {}", args[..3].join(" "));
            self.step(what)?;
            Ok(if args[2] == "check" {
                *self.art_check.lock().unwrap()
            } else {
                0
            })
        }
    }

    struct Rig {
        root: PathBuf,
        cfg: UpdateConfig,
        git: Arc<FakeGit>,
        builder: Arc<FakeBuilder>,
        fake: Arc<FakeLaunchd>,
        ld: Launchd,
        server: Agent,
    }

    fn rig(name: &str) -> Rig {
        let root =
            std::env::temp_dir().join(format!("h2-menubar-upd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let cfg = UpdateConfig {
            repo: root.join("src"),
            branch: "master".into(),
            remote: "origin".into(),
            interval_minutes: 10,
            install_dir: root.join("live"),
            disc: None,
            mappack: None,
            cargo: None,
            bun: None,
            git: None,
            path: Vec::new(),
        };
        let fake = FakeLaunchd::new();
        let ld = Launchd::new(fake.clone(), fake.domain.clone());
        let server = Agent::in_dir(&root, "org.h2recomp.live.server");
        fs::write(
            &server.plist,
            "<plist><key>Label</key><string>org.h2recomp.live.server</string></plist>",
        )
        .unwrap();
        Rig {
            builder: FakeBuilder::new(root.join("build")),
            git: FakeGit::new("a1"),
            root,
            cfg,
            fake,
            ld,
            server,
        }
    }

    impl Rig {
        fn check(&self, force: bool) -> Report {
            let progress = |_: &str| {};
            Updater {
                cfg: &self.cfg,
                git: self.git.as_ref(),
                builder: self.builder.as_ref(),
                services: &self.ld,
                server: &self.server,
                data_dir: &self.root.join("live/server"),
                progress: &progress,
                now: 1000,
            }
            .check(force)
        }

        fn state(&self) -> State {
            State::load(&self.cfg.state_file())
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.cfg.install_dir.join(rel)).unwrap()
        }

        fn load_server(&self) {
            self.ld.start(&self.server).unwrap();
            self.fake.calls();
        }
    }

    #[test]
    fn first_run_builds_everything_then_follows_the_branch() {
        let r = rig("flow");
        r.load_server();

        // Nothing installed: all of it, at HEAD.
        let rep = r.check(false);
        assert_eq!(
            rep.outcome,
            Outcome::Updated {
                built: vec!["server".into(), "h2".into(), "web".into()]
            }
        );
        assert!(rep.h2_changed);
        assert_eq!(
            r.builder.calls(),
            [
                "cargo live-server h2-live-server",
                "cargo cli h2",
                "web",
                "sync"
            ]
        );
        assert_eq!(r.read("bin/h2-live-server"), "h2-live-server build 1");
        assert_eq!(r.read("bin/h2"), "h2 build 2");
        assert_eq!(r.read("web/index.html"), "web 3");
        assert!(!r.cfg.bin_dir().join("h2.new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(r.cfg.bin_dir().join("h2"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        assert_eq!(
            r.fake.calls().last().unwrap(),
            "kickstart -k gui/501/org.h2recomp.live.server"
        );
        let st = r.state();
        assert_eq!(st.installed.as_deref(), Some("a1"));
        assert_eq!(st.last_check, Some(1000));
        r.git.calls();

        // Nothing new.
        assert_eq!(r.check(false).outcome, Outcome::UpToDate);
        assert!(r.builder.calls().is_empty());
        assert_eq!(r.git.calls(), ["fetch origin master"]);

        // The web app alone: just the web build, and a restart.
        r.git.push("a1", "b2", &["web/src/App.tsx"]);
        let rep = r.check(false);
        assert_eq!(
            rep.outcome,
            Outcome::Updated {
                built: vec!["web".into()]
            }
        );
        assert!(!rep.h2_changed);
        assert_eq!(r.git.calls(), ["fetch origin master", "merge b2"]);
        assert_eq!(r.builder.calls(), ["web", "sync"]);
        assert!(r.fake.calls().iter().any(|c| c.starts_with("kickstart -k")));

        // The CLI alone: h2, no server build, no server restart.
        r.git.push("b2", "c3", &["crates/cli/src/main.rs"]);
        let rep = r.check(false);
        assert_eq!(
            rep.outcome,
            Outcome::Updated {
                built: vec!["h2".into()]
            }
        );
        assert!(rep.h2_changed);
        assert_eq!(r.builder.calls(), ["cargo cli h2"]);
        assert!(!r.fake.calls().iter().any(|c| c.starts_with("kickstart")));

        // Two commits at once, a shared crate and the lockfile: both.
        r.git.push("c3", "d4", &["crates/live-proto/src/lib.rs"]);
        r.git.push("d4", "e5", &["docs/x.md"]);
        assert_eq!(
            r.check(false).outcome,
            Outcome::Updated {
                built: vec!["server".into(), "h2".into()]
            }
        );
        r.builder.calls();
        r.git.push("e5", "f6", &["Cargo.lock"]);
        r.check(false);
        assert_eq!(
            r.builder.calls(),
            ["cargo live-server h2-live-server", "cargo cli h2"]
        );

        // Docs only: nothing to build, but the revision is recorded.
        r.git.push("f6", "g7", &["docs/live/live.md"]);
        assert_eq!(r.check(false).outcome, Outcome::Updated { built: vec![] });
        assert!(r.builder.calls().is_empty());
        assert_eq!(r.state().installed.as_deref(), Some("g7"));
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn a_stopped_server_is_not_started() {
        let r = rig("stopped");
        r.check(false);
        assert!(
            !r.fake
                .calls()
                .iter()
                .any(|c| c.starts_with("kickstart") || c.starts_with("bootstrap"))
        );
        assert!(!r.ld.state(&r.server).is_loaded());
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn dirty_diverged_and_off_branch_checkouts_are_left_alone() {
        let r = rig("blocked");
        r.check(false);
        r.builder.calls();
        r.git.calls();

        r.git
            .dirty
            .lock()
            .unwrap()
            .push(" M crates/cli/src/main.rs".into());
        r.git.push("a1", "b2", &["web/x"]);
        assert_eq!(
            r.check(false).outcome,
            Outcome::Blocked("checkout has uncommitted changes (1 files)".into())
        );
        assert!(r.git.calls().is_empty(), "not even a fetch");
        r.git.dirty.lock().unwrap().clear();

        *r.git.branch.lock().unwrap() = "wip".into();
        assert_eq!(
            r.check(false).outcome,
            Outcome::Blocked("checkout is on wip, not master".into())
        );
        *r.git.branch.lock().unwrap() = "master".into();

        // A local commit the remote does not have.
        *r.git.head.lock().unwrap() = "local".into();
        r.git
            .parents
            .lock()
            .unwrap()
            .push(("local".into(), "a1".into()));
        let rep = r.check(false);
        assert!(
            matches!(rep.outcome, Outcome::Blocked(ref why) if why.contains("not a fast-forward")),
            "{rep:?}"
        );
        assert!(!r.git.calls().iter().any(|c| c.starts_with("merge")));
        assert!(r.builder.calls().is_empty());
        assert_eq!(r.state().installed.as_deref(), Some("a1"));

        // A failing fetch is a failure line, not a panic.
        *r.git.head.lock().unwrap() = "b2".into();
        *r.git.fetch_fails.lock().unwrap() = true;
        assert!(
            matches!(r.check(false).outcome, Outcome::Failed(ref e) if e.contains("git fetch"))
        );
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn tool_path_adds_the_usual_dirs_once() {
        let home = Path::new("/Users/me");
        let p = tool_path(
            &[PathBuf::from("/x/bin")],
            Some(home),
            Some(OsStr::new("/usr/bin:/bin:/opt/homebrew/bin:/usr/bin")),
        );
        let dirs: Vec<String> = std::env::split_paths(&p)
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            dirs,
            [
                "/x/bin",
                "/usr/bin",
                "/bin",
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/Users/me/.cargo/bin",
                "/Users/me/.bun/bin",
            ]
        );
        // A LaunchAgent's PATH gains everything; nothing inherited is fine.
        let p = tool_path(&[], None, None);
        let dirs: Vec<_> = std::env::split_paths(&p).collect();
        assert_eq!(
            dirs,
            [PathBuf::from("/opt/homebrew/bin"), "/usr/local/bin".into()]
        );
        let (k, v) = path_entry(&[]);
        assert_eq!(k, "PATH");
        assert!(v.contains("/opt/homebrew/bin"));
    }

    #[test]
    fn missing_git_lfs_blocks_before_any_merge() {
        let r = rig("lfs");
        r.check(false);
        r.git.calls();
        r.git.push("a1", "b2", &["web/x"]);
        *r.git.lfs_missing.lock().unwrap() = true;
        assert_eq!(
            r.check(false).outcome,
            Outcome::Blocked("git-lfs not found".into())
        );
        assert!(r.git.calls().is_empty(), "no fetch, no merge");
        assert_eq!(*r.git.head.lock().unwrap(), "a1");
        // Installed: the next check merges as usual.
        *r.git.lfs_missing.lock().unwrap() = false;
        assert!(matches!(r.check(false).outcome, Outcome::Updated { .. }));
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn a_failed_merge_is_restored_and_retried() {
        let r = rig("merge");
        r.check(false);
        r.builder.calls();
        r.git.calls();
        r.git.push("a1", "b2", &["web/x"]);
        *r.git.merge_fails.lock().unwrap() = true;
        let rep = r.check(false);
        assert!(
            matches!(rep.outcome, Outcome::Failed(ref e) if e.contains("git merge --ff-only") && e.contains("git-lfs: command not found")),
            "{rep:?}"
        );
        assert_eq!(
            r.git.calls(),
            ["fetch origin master", "merge b2", "restore a1"]
        );
        assert!(r.git.dirty.lock().unwrap().is_empty(), "restored");
        assert_eq!(r.state().merge_failed, None);
        assert_eq!(r.state().failed, None, "the timer retries");

        // Not Blocked forever: the next timer check merges and installs.
        *r.git.merge_fails.lock().unwrap() = false;
        assert!(matches!(r.check(false).outcome, Outcome::Updated { .. }));
        assert_eq!(r.state().installed.as_deref(), Some("b2"));
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn dirt_from_our_own_failed_merge_is_recovered_but_the_users_is_not() {
        let r = rig("recover");
        r.check(false);
        r.builder.calls();
        r.git.push("a1", "b2", &["web/x"]);
        *r.git.merge_fails.lock().unwrap() = true;
        *r.git.restore_fails.lock().unwrap() = true;
        let rep = r.check(false);
        assert!(
            matches!(rep.outcome, Outcome::Failed(ref e) if e.contains("restoring the checkout failed")),
            "{rep:?}"
        );
        assert_eq!(r.git.dirty.lock().unwrap().len(), 105);
        assert_eq!(r.state().merge_failed.as_deref(), Some("a1"));

        // Still failing to restore: a failure, not a block, and no merge.
        r.git.calls();
        assert!(matches!(r.check(false).outcome, Outcome::Failed(_)));
        assert_eq!(r.git.calls(), ["restore a1"]);

        // Restore works now: recovered, then the merge goes through.
        *r.git.restore_fails.lock().unwrap() = false;
        *r.git.merge_fails.lock().unwrap() = false;
        assert!(matches!(r.check(false).outcome, Outcome::Updated { .. }));
        assert_eq!(r.state().merge_failed, None);
        assert_eq!(r.state().installed.as_deref(), Some("b2"));

        // The user's dirt: no merge record, or a different HEAD: refused,
        // never reset.
        r.git.calls();
        r.git
            .dirty
            .lock()
            .unwrap()
            .push(" M crates/cli/src/main.rs".into());
        assert!(matches!(r.check(false).outcome, Outcome::Blocked(_)));
        let mut st = r.state();
        st.merge_failed = Some("a1".into());
        st.save(&r.cfg.state_file()).unwrap();
        assert!(matches!(r.check(false).outcome, Outcome::Blocked(_)));
        assert!(r.git.calls().is_empty(), "no restore, no fetch");
        assert_eq!(r.git.dirty.lock().unwrap().len(), 1);
        fs::remove_dir_all(&r.root).unwrap();
    }

    /// The real git: `restore` puts a dirty clone back; `lfs_problem`
    /// asks `git lfs version` only for repositories that use LFS.
    #[test]
    fn system_git_restores_and_checks_lfs() {
        let root = std::env::temp_dir().join(format!("h2-menubar-restore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        run(&["init", "-q"]);
        fs::write(root.join("a"), "one").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "one"]);
        let head = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&root)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let git = SystemGit {
            repo: root.clone(),
            log: root.join("update.log"),
            git: None,
            path: system_tool_path(&[]),
        };
        fs::write(root.join("a"), "two").unwrap();
        fs::create_dir_all(root.join("d")).unwrap();
        fs::write(root.join("d/new"), "x").unwrap();
        assert_eq!(git.dirty().unwrap().len(), 2);
        git.restore(head.trim()).unwrap();
        assert!(git.dirty().unwrap().is_empty());
        assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "one");
        assert!(!root.join("d/new").exists());

        // No LFS attributes: nothing to check. With them and a git that
        // has no lfs: the reason names git-lfs.
        assert_eq!(git.lfs_problem(), None);
        fs::write(root.join(".gitattributes"), "*.png filter=lfs diff=lfs\n").unwrap();
        let fake = root.join("fake-git");
        fs::write(
            &fake,
            "#!/bin/sh\necho \"git: 'lfs' is not a git command\" >&2\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let no_lfs = SystemGit {
            git: Some(fake),
            ..git
        };
        let why = no_lfs.lfs_problem().unwrap();
        assert!(why.starts_with("git-lfs not found"), "{why}");
        assert!(why.contains("not a git command"), "{why}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_failed_build_waits_for_update_now() {
        let r = rig("fail");
        r.check(false);
        r.builder.calls();
        r.git.push("a1", "b2", &["crates/live-server/src/app.rs"]);
        *r.builder.fail.lock().unwrap() = Some("cargo live-server h2-live-server".into());
        assert!(matches!(r.check(false).outcome, Outcome::Failed(ref e) if e.contains("failed")));
        assert_eq!(r.state().failed.as_deref(), Some("b2"));
        assert_eq!(r.state().installed.as_deref(), Some("a1"));
        r.builder.calls();

        // The timer does not try again.
        assert!(
            matches!(r.check(false).outcome, Outcome::Failed(ref e) if e.contains("Update Now"))
        );
        assert!(r.builder.calls().is_empty());

        // Update Now does; fixed, it installs.
        *r.builder.fail.lock().unwrap() = None;
        assert_eq!(
            r.check(true).outcome,
            Outcome::Updated {
                built: vec!["server".into(), "h2".into()]
            }
        );
        assert_eq!(r.state().failed, None);
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn paused_updates_wait_for_update_now() {
        let r = rig("pause");
        State {
            paused: true,
            ..State::default()
        }
        .save(&r.cfg.state_file())
        .unwrap();
        assert_eq!(r.check(false).outcome, Outcome::Paused);
        assert!(r.git.calls().is_empty());
        assert!(matches!(r.check(true).outcome, Outcome::Updated { .. }));
        assert!(r.state().paused, "Update Now does not unpause");
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn a_new_extractor_rechecks_the_art() {
        let r = rig("art");
        r.git.art.lock().unwrap().push(("a1".into(), 15));
        r.check(false);
        r.builder.calls();

        // The file changed, the version did not: no check.
        r.git.push("a1", "b2", &[ART_SOURCE]);
        r.check(false);
        assert_eq!(r.builder.calls(), ["cargo cli h2"]);

        // A new version, no disc configured: stale, said in the menu.
        r.git.push("b2", "c3", &[ART_SOURCE]);
        r.git.art.lock().unwrap().push(("c3".into(), 16));
        *r.builder.art_check.lock().unwrap() = 1;
        r.check(false);
        assert_eq!(r.builder.calls(), ["cargo cli h2", "h2 live assets check"]);
        assert!(r.state().art_stale);

        // With a disc it re-extracts, then restarts the server.
        let mut cfg = r.cfg.clone();
        cfg.disc = Some("/discs/h2.iso".into());
        cfg.mappack = Some("/discs/mp.iso".into());
        let r = Rig { cfg, ..r };
        r.load_server();
        r.git.push("c3", "d4", &["docs/x.md"]);
        let rep = r.check(false);
        assert_eq!(
            rep.outcome,
            Outcome::Updated {
                built: vec!["art".into()]
            }
        );
        assert_eq!(
            r.builder.calls(),
            ["h2 live assets check", "h2 live assets extract"]
        );
        assert!(!r.state().art_stale);
        assert!(r.fake.calls().iter().any(|c| c.starts_with("kickstart -k")));
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn a_rewritten_history_builds_everything() {
        let r = rig("rewrite");
        State {
            installed: Some("gone".into()),
            ..State::default()
        }
        .save(&r.cfg.state_file())
        .unwrap();
        assert!(
            matches!(r.check(false).outcome, Outcome::Updated { ref built } if built.len() == 3)
        );
        fs::remove_dir_all(&r.root).unwrap();
    }

    #[test]
    fn plans_and_versions() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let server = s(&["crates/live-server/", "crates/live-proto/"]);
        let cli = s(&["crates/cli/", "crates/live-server/", "crates/live-proto/"]);
        assert_eq!(
            plan(&s(&["web/a"]), &server, &cli),
            Plan {
                web: true,
                ..Plan::default()
            }
        );
        assert_eq!(
            plan(&s(&["crates/live-server-extra/x"]), &server, &cli),
            Plan::default(),
            "a prefix of a name is not the crate"
        );
        assert_eq!(
            plan(&s(&[".cargo/config.toml"]), &server, &cli),
            Plan {
                server: true,
                h2: true,
                web: false
            }
        );
        assert!(!plan(&s(&["README.md", "docs/a.md"]), &server, &cli).any());

        assert_eq!(
            extractor_version("x\npub const EXTRACTOR_VERSION: u32 = 15;\n"),
            Some(15)
        );
        assert_eq!(extractor_version("const OTHER: u32 = 1;"), None);
    }

    #[test]
    fn path_closure_from_cargo_metadata() {
        let meta = r#"{"packages":[
          {"name":"live-server","manifest_path":"/r/crates/live-server/Cargo.toml","dependencies":[
            {"name":"live-proto","path":"/r/crates/live-proto","kind":null},
            {"name":"serde","kind":null},
            {"name":"testkit","path":"/r/crates/testkit","kind":"dev"}]},
          {"name":"live-proto","manifest_path":"/r/crates/live-proto/Cargo.toml","dependencies":[
            {"name":"content","path":"/r/crates/content","kind":null}]},
          {"name":"content","manifest_path":"/r/crates/content/Cargo.toml","dependencies":[]},
          {"name":"testkit","manifest_path":"/r/crates/testkit/Cargo.toml","dependencies":[]}
        ]}"#;
        assert_eq!(
            path_closure(meta, "live-server", Path::new("/r")).unwrap(),
            [
                "crates/content/",
                "crates/live-proto/",
                "crates/live-server/"
            ]
        );
        assert!(path_closure(meta, "nope", Path::new("/r")).is_err());
    }

    /// The real git against a scratch repository: dirty, ancestry,
    /// fast-forward, diffs and show. No network: the remote is a local
    /// bare repository.
    #[test]
    fn system_git_on_a_scratch_repository() {
        let root = std::env::temp_dir().join(format!("h2-menubar-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let out = Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "init.defaultBranch=master",
                ])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let upstream = root.join("up");
        fs::create_dir_all(&upstream).unwrap();
        run(&upstream, &["init", "-q"]);
        fs::create_dir_all(upstream.join("crates/cli/src")).unwrap();
        fs::write(
            upstream.join(ART_SOURCE),
            "pub const EXTRACTOR_VERSION: u32 = 3;\n",
        )
        .unwrap();
        run(&upstream, &["add", "."]);
        run(&upstream, &["commit", "-q", "-m", "one"]);
        run(&root, &["clone", "-q", upstream.to_str().unwrap(), "clone"]);
        let clone = root.join("clone");
        let first = run(&clone, &["rev-parse", "HEAD"]);

        fs::create_dir_all(upstream.join("web")).unwrap();
        fs::write(upstream.join("web/a.ts"), "x").unwrap();
        run(&upstream, &["add", "."]);
        run(&upstream, &["commit", "-q", "-m", "two"]);
        let second = run(&upstream, &["rev-parse", "HEAD"]);

        let git = SystemGit {
            repo: clone.clone(),
            log: root.join("update.log"),
            git: None,
            path: system_tool_path(&[]),
        };
        assert_eq!(git.current_branch().unwrap(), "master");
        assert!(git.dirty().unwrap().is_empty());
        git.fetch("origin", "master").unwrap();
        assert_eq!(git.rev_parse("origin/master").unwrap(), second);
        assert!(git.is_ancestor(&first, &second).unwrap());
        assert!(!git.is_ancestor(&second, &first).unwrap());
        git.merge_ff_only(&second).unwrap();
        assert_eq!(git.rev_parse("HEAD").unwrap(), second);
        assert_eq!(git.changed(&first, &second).unwrap(), ["web/a.ts"]);
        assert_eq!(
            git.show(&first, ART_SOURCE)
                .unwrap()
                .as_deref()
                .and_then(extractor_version),
            Some(3)
        );
        assert_eq!(git.show(&first, "web/a.ts").unwrap(), None);
        fs::write(clone.join("web/a.ts"), "y").unwrap();
        assert_eq!(git.dirty().unwrap().len(), 1);
        assert!(git.rev_parse("nope").is_err());
        assert!(
            fs::read_to_string(root.join("update.log"))
                .unwrap()
                .contains("fetch")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn install_file_replaces_atomically() {
        let root = std::env::temp_dir().join(format!("h2-menubar-inst-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let from = root.join("built");
        fs::write(&from, "new").unwrap();
        let to = root.join("bin/h2");
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::write(&to, "old").unwrap();
        install_file(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(&to).unwrap(), "new");
        assert!(!root.join("bin/h2.new").exists());
        assert!(install_file(&root.join("missing"), &to).is_err());
        assert_eq!(fs::read_to_string(&to).unwrap(), "new");
        fs::remove_dir_all(&root).unwrap();
    }
}
