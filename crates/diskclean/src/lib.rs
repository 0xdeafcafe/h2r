//! `h2 disk clean`: frees disk taken by a development machine's leftovers,
//! never by anything a person or a running process might still want.
//!
//! It removes three kinds of thing (docs/dev/disk.md):
//! - linked git worktrees on a branch that is merged into the base branch,
//!   with no changes, idle for a day, and not in use;
//! - Cargo target directories no build has touched for two days, when no
//!   process has them open or names them;
//! - inside the other targets, `deps` artifacts and `incremental` sessions no
//!   build has written for two days (Cargo rebuilds a missing artifact).
//!
//! Everything else (run folders, data roots, caches of other tools) is left
//! alone: it only deletes what it can recognise as a worktree or a target.
//! "In use" is checked again right before each removal, not only when the
//! plan is made.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

pub const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// What to clean and how cautiously.
#[derive(Clone, Debug)]
pub struct Options {
    /// Any checkout of the repository; its worktrees are considered.
    pub repo: PathBuf,
    /// A worktree's branch must be merged into this one.
    pub base: String,
    /// Extra directories searched (three levels deep) for Cargo targets.
    pub scan: Vec<PathBuf>,
    /// A worktree must have had no commit, checkout or index write for this long.
    pub worktree_idle: Duration,
    /// A target no build has touched for this long is removed whole.
    pub target_idle: Duration,
    /// In other targets, artifacts older than this are removed.
    pub artifact_age: Duration,
    /// The clock, so tests can move it.
    pub now: SystemTime,
}

impl Options {
    pub fn new(repo: PathBuf) -> Self {
        Options {
            repo,
            base: "master".into(),
            scan: Vec::new(),
            worktree_idle: DAY,
            target_idle: 2 * DAY,
            artifact_age: 2 * DAY,
            now: SystemTime::now(),
        }
    }
}

/// One thing the clean will do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    RemoveWorktree { path: PathBuf, branch: String },
    RemoveTarget { path: PathBuf },
    PruneArtifacts { target: PathBuf, paths: Vec<PathBuf> },
}

/// A plan, with why each candidate was kept.
#[derive(Debug, Default)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub kept: Vec<(PathBuf, String)>,
}

// ---------------------------------------------------------------- worktrees

/// One entry of `git worktree list --porcelain`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub main: bool,
    pub locked: bool,
    pub prunable: bool,
}

pub fn parse_worktrees(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut out: Vec<WorktreeEntry> = Vec::new();
    for line in porcelain.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            out.push(WorktreeEntry { path: PathBuf::from(p), main: out.is_empty(), ..Default::default() });
            continue;
        }
        let Some(e) = out.last_mut() else { continue };
        if let Some(b) = line.strip_prefix("branch ") {
            e.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
        } else if line == "locked" || line.starts_with("locked ") {
            e.locked = true;
        } else if line == "prunable" || line.starts_with("prunable ") {
            e.prunable = true;
        }
    }
    out
}

/// What is known about a worktree when deciding.
#[derive(Clone, Debug)]
pub struct WorktreeFacts {
    pub entry: WorktreeEntry,
    pub merged: bool,
    pub clean: bool,
    pub last_activity: SystemTime,
    pub in_use: bool,
}

/// `Ok(branch)` to remove the worktree, or why it stays.
pub fn decide_worktree(f: &WorktreeFacts, now: SystemTime, idle: Duration) -> Result<String, String> {
    let e = &f.entry;
    if e.main {
        return Err("main checkout".into());
    }
    if e.locked {
        return Err("locked".into());
    }
    if e.prunable {
        return Err("prunable (git worktree prune handles it)".into());
    }
    let Some(branch) = &e.branch else { return Err("detached HEAD".into()) };
    if !f.merged {
        return Err(format!("{branch} not merged"));
    }
    if !f.clean {
        return Err("has changes or untracked files".into());
    }
    if age(now, f.last_activity) < idle {
        return Err("active recently".into());
    }
    if f.in_use {
        return Err("in use".into());
    }
    Ok(branch.clone())
}

fn age(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or(Duration::ZERO)
}

fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // Status must not refresh the index: that would make the worktree
        // look active, and write into someone else's checkout.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .with_context(|| format!("git {} in {}", args.join(" "), dir.display()))
}

fn git_ok(dir: &Path, args: &[&str]) -> Result<String> {
    let o = git(dir, args)?;
    if !o.status.success() {
        bail!("git {} in {}: {}", args.join(" "), dir.display(), String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(p).ok()?.modified().ok()
}

fn newest(paths: impl IntoIterator<Item = PathBuf>) -> SystemTime {
    paths.into_iter().filter_map(|p| mtime(&p)).max().unwrap_or(SystemTime::UNIX_EPOCH)
}

fn worktree_facts(e: WorktreeEntry, base: &str) -> Result<WorktreeFacts> {
    let p = &e.path;
    let merged = !e.main
        && git(p, &["merge-base", "--is-ancestor", "HEAD", base])?.status.success();
    let clean = git_ok(p, &["status", "--porcelain"]).map(|s| s.trim().is_empty()).unwrap_or(false);
    let gitdir = git_ok(p, &["rev-parse", "--absolute-git-dir"]).map(|s| PathBuf::from(s.trim()));
    let last_activity = match gitdir {
        Ok(g) => newest([g.join("HEAD"), g.join("index"), g.join("logs/HEAD"), p.clone()]),
        Err(_) => SystemTime::now(),
    };
    // Only ask the expensive question of worktrees that could go.
    let in_use = merged && clean && in_use(p);
    Ok(WorktreeFacts { entry: e, merged, clean, last_activity, in_use })
}

// ---------------------------------------------------------------- targets

/// Cargo writes both into every target directory it creates.
pub fn is_target(dir: &Path) -> bool {
    dir.join("CACHEDIR.TAG").is_file() && dir.join(".rustc_info.json").is_file()
}

/// Targets at or below `root`, `depth` levels down; not inside a target.
pub fn find_targets(root: &Path, depth: usize, out: &mut BTreeSet<PathBuf>) {
    if is_target(root) {
        out.insert(root.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let Ok(t) = e.file_type() else { continue };
        let name = e.file_name();
        if t.is_dir() && name != ".git" && name != "node_modules" {
            find_targets(&e.path(), depth - 1, out);
        }
    }
}

/// When a build last touched the target: the directories and lock files
/// Cargo writes on every build of a profile.
pub fn target_activity(target: &Path) -> SystemTime {
    let mut paths = vec![target.to_path_buf(), target.join(".rustc_info.json")];
    for p in profile_dirs(target) {
        for sub in ["", "deps", ".fingerprint", ".cargo-lock", "build"] {
            paths.push(if sub.is_empty() { p.clone() } else { p.join(sub) });
        }
    }
    newest(paths)
}

/// `<target>/<profile>` and `<target>/<triple>/<profile>`.
fn profile_dirs(target: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let visit = |dir: &Path, out: &mut Vec<PathBuf>| {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.join(".fingerprint").is_dir() {
                out.push(p);
            }
        }
    };
    visit(target, &mut out);
    for e in fs::read_dir(target).into_iter().flatten().flatten() {
        if e.path().is_dir() && !e.path().join(".fingerprint").is_dir() {
            visit(&e.path(), &mut out);
        }
    }
    out
}

/// Artifacts in `deps` and sessions in `incremental` last written before
/// `now - max_age`.
pub fn old_artifacts(target: &Path, now: SystemTime, max_age: Duration) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in profile_dirs(target) {
        for sub in ["deps", "incremental"] {
            for e in fs::read_dir(p.join(sub)).into_iter().flatten().flatten() {
                let path = e.path();
                if mtime(&path).is_some_and(|t| age(now, t) >= max_age) {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------- in use

/// Whether any process has a file under `path` open (or its working
/// directory there), or names it in its arguments or environment. If the
/// question can't be answered, the answer is yes.
pub fn in_use(path: &Path) -> bool {
    let me = std::process::id().to_string();
    let lsof = Command::new("lsof").arg("+D").arg(path).output();
    match lsof {
        Ok(o) if !o.stdout.is_empty() => return true,
        Ok(_) => {}
        Err(_) => return true,
    }
    let Ok(ps) = Command::new("ps").args(["-axeww", "-o", "pid=,command="]).output() else { return true };
    let needle = path.to_string_lossy();
    String::from_utf8_lossy(&ps.stdout).lines().any(|l| {
        let l = l.trim_start();
        let pid = l.split_whitespace().next().unwrap_or("");
        pid != me && l.contains(needle.as_ref())
    })
}

// ---------------------------------------------------------------- plan

pub fn plan(o: &Options) -> Result<Plan> {
    let mut plan = Plan::default();
    let list = git_ok(&o.repo, &["worktree", "list", "--porcelain"])?;
    let entries = parse_worktrees(&list);
    let mut kept_roots = Vec::new();
    let mut removed_roots = Vec::new();
    for e in entries {
        if !e.path.is_dir() {
            continue;
        }
        let path = e.path.clone();
        let facts = worktree_facts(e, &o.base)?;
        match decide_worktree(&facts, o.now, o.worktree_idle) {
            Ok(branch) => {
                removed_roots.push(path.clone());
                plan.actions.push(Action::RemoveWorktree { path, branch });
            }
            Err(why) => {
                plan.kept.push((path.clone(), why));
                kept_roots.push(path);
            }
        }
    }

    let mut targets = BTreeSet::new();
    for root in &kept_roots {
        find_targets(&root.join("target"), 0, &mut targets);
    }
    for root in &o.scan {
        find_targets(root, 3, &mut targets);
    }
    for t in targets {
        if removed_roots.iter().any(|r| t.starts_with(r)) {
            continue;
        }
        if age(o.now, target_activity(&t)) >= o.target_idle {
            if in_use(&t) {
                plan.kept.push((t, "idle target, but in use".into()));
            } else {
                plan.actions.push(Action::RemoveTarget { path: t });
            }
            continue;
        }
        let paths = old_artifacts(&t, o.now, o.artifact_age);
        if paths.is_empty() {
            continue;
        }
        if in_use(&t) {
            plan.kept.push((t, "has old artifacts, but a build is using it".into()));
        } else {
            plan.actions.push(Action::PruneArtifacts { target: t, paths });
        }
    }
    Ok(plan)
}

/// Bytes that removing `path` would free: files with no other hard link
/// (a kache-linked artifact frees nothing).
pub fn freeable(path: &Path) -> u64 {
    let Ok(m) = fs::symlink_metadata(path) else { return 0 };
    if m.is_dir() {
        fs::read_dir(path).into_iter().flatten().flatten().map(|e| freeable(&e.path())).sum()
    } else if m.nlink() <= 1 {
        m.blocks() * 512
    } else {
        0
    }
}

/// Carries out one action, checking again that nothing uses it. Returns
/// the bytes freed, or why it was skipped.
pub fn apply(o: &Options, a: &Action) -> Result<u64, String> {
    match a {
        Action::RemoveWorktree { path, branch } => {
            if in_use(path) {
                return Err("now in use".into());
            }
            let bytes = freeable(path);
            let out = git(&o.repo, &["worktree", "remove", &path.to_string_lossy()]).map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            // -d, not -D: git checks once more that it is merged.
            let _ = git(&o.repo, &["branch", "-d", branch]);
            Ok(bytes)
        }
        Action::RemoveTarget { path } => {
            if in_use(path) {
                return Err("now in use".into());
            }
            let bytes = freeable(path);
            fs::remove_dir_all(path).map_err(|e| e.to_string())?;
            Ok(bytes)
        }
        Action::PruneArtifacts { target, paths } => {
            if in_use(target) {
                return Err("now in use".into());
            }
            let mut bytes = 0;
            for p in paths {
                bytes += freeable(p);
                let r = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
                r.map_err(|e| format!("{}: {e}", p.display()))?;
            }
            Ok(bytes)
        }
    }
}

/// Bytes free to an unprivileged user on the volume holding `path`.
pub fn free_bytes(path: &Path) -> Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes())?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a valid C string and s a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        bail!("statvfs {}: {}", path.display(), std::io::Error::last_os_error());
    }
    Ok(s.f_bavail as u64 * s.f_frsize as u64)
}

pub fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

#[cfg(test)]
mod tests;
