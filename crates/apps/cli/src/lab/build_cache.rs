//! The lab's one detached checkout, its one Cargo target, the game images
//! (one work directory per build variant) and the bounded build caches.
//! See docs/runtime/lab.md, "Builds go by branch".

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{host::HostConfig, proto, transfer};

pub(super) const DEFAULT_OBJECT_LIMIT: u64 = 8 << 30;
pub(super) const DEFAULT_TARGET_LIMIT: u64 = 32 << 30;
/// Free space the lab disk keeps (`--disk-floor`). A build under it
/// prunes the lab's caches first and fails if that is not enough.
pub(super) const DEFAULT_DISK_FLOOR: u64 = 20 << 30;
/// The newest finished job runs a prune for the disk floor keeps.
pub(super) const RUNS_KEPT: usize = 30;

/// The image work directories kept, the one in use included. Each holds
/// the generated C, the object pins, the module and `h2` of its variant.
pub(super) const IMAGE_VARIANTS: usize = 4;

pub(super) fn workers() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(2).max(1))
}

// A lab-root child being cleaned or reused must be an actual directory.
pub(super) fn ensure_dir(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            ensure!(
                fs::symlink_metadata(path)?.is_dir(),
                "{} isn't a directory",
                path.display()
            );
            Ok(())
        }
        Err(err) => Err(err).with_context(|| format!("creating {}", path.display())),
    }
}

/// The one Cargo target every lab build uses. Image jobs and host updates
/// both build `src/current` into it, so one source tree has one target
/// and a commit both of them build is compiled once.
pub(super) fn target_dir(cfg: &HostConfig) -> PathBuf {
    cfg.dir("target")
}

/// `H2_OBJ_CACHE` for every image build: `obj` beside the lab root. With
/// the default root (`~/Library/Caches/h2recomp/lab`) that is
/// `~/Library/Caches/h2recomp/obj`, the object cache the Mac's own
/// `h2 build`s use (AGENTS.md, "Build fast"), so lab jobs, other
/// checkouts and worktrees share clang's shard objects.
pub(super) fn obj_cache_dir(cfg: &HostConfig) -> PathBuf {
    cfg.root
        .parent()
        .map_or_else(|| cfg.dir("obj-cache"), |parent| parent.join("obj"))
}

/// The `h2` a lab cargo build produces.
pub(super) fn built_h2(cfg: &HostConfig) -> PathBuf {
    target_dir(cfg).join("release/h2")
}

/// `cargo build --release -p cli --bin h2` of `src` into the lab's target,
/// for jobs and host updates alike. The host's `H2_*` variables are
/// dropped and a job's build variables are not passed: they pick the game
/// image, not the Rust build, and leaving them out keeps Cargo's
/// fingerprints (and a rustc wrapper's keys) the same for every variant.
/// Cargo's own configuration applies as in a Terminal, including a
/// `build.rustc-wrapper` such as kache; the lab never sets one.
pub(super) fn cargo_command(cfg: &HostConfig, src: &Path, features: &[String]) -> Command {
    let mut cargo = crate::follow::niced(&cfg.cargo);
    cargo.args(["build", "--release", "-p", "cli", "--bin", "h2"]);
    if !features.is_empty() {
        cargo.arg("--features").arg(features.join(","));
    }
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
            cargo.env_remove(k);
        }
    }
    cargo
        .current_dir(src)
        .env("CARGO_BUILD_JOBS", workers().to_string())
        .env("CARGO_TARGET_DIR", target_dir(cfg));
    cargo
}

/// The rustc wrapper Cargo will use for a build in `src`, and where it is
/// set: `RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WRAPPER`, then
/// `build.rustc-wrapper` in the nearest `.cargo/config.toml` (or
/// `.cargo/config`) above `src`, then in `$CARGO_HOME` (default
/// `~/.cargo`). An empty value turns the wrapper off. This is for the
/// build log only: Cargo applies the wrapper itself.
pub(super) fn rustc_wrapper(
    src: &Path,
    var: impl Fn(&str) -> Option<String>,
) -> Option<(String, String)> {
    for name in ["RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER"] {
        if let Some(v) = var(name) {
            return (!v.is_empty()).then(|| (v, name.to_string()));
        }
    }
    let home = var("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| Path::new(&h).join(".cargo")));
    let dirs = src.ancestors().map(|a| a.join(".cargo")).chain(home);
    for dir in dirs {
        for file in ["config.toml", "config"] {
            let path = dir.join(file);
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(table) = text.parse::<toml::Table>() else {
                continue;
            };
            if let Some(v) = table
                .get("build")
                .and_then(|b| b.get("rustc-wrapper"))
                .and_then(|w| w.as_str())
            {
                return (!v.is_empty()).then(|| (v.to_string(), path.display().to_string()));
            }
        }
    }
    None
}

/// One line for the build log: how many crates Cargo compiled and the
/// rustc wrapper in effect. Cargo prints a `Compiling` line for every
/// crate it builds; the others were fresh in the target.
pub(super) fn cargo_summary(output: &str, wrapper: Option<&(String, String)>) -> String {
    let compiled = output
        .lines()
        .filter(|l| l.trim_start().starts_with("Compiling "))
        .count();
    let wrapper = match wrapper {
        Some((w, from)) => format!("rustc wrapper {w} (from {from})"),
        None => "no rustc wrapper, so a new target compiles every crate".into(),
    };
    format!("lab: cargo compiled {compiled} crates, the rest were fresh in the lab target; {wrapper}")
}

/// One line for the build log from `h2 build`'s phase lines: whether the
/// analysis came from the boot cache, how many C files emission rewrote,
/// and how many objects clang compiled or took from the object cache.
pub(super) fn image_summary(output: &str) -> String {
    let mut parts = Vec::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("analyze: ") {
            parts.push(if rest.contains("(cached)") {
                "analysis cached".to_string()
            } else {
                "analysis recomputed".to_string()
            });
        } else if let Some(rest) = line.strip_prefix("emit: ") {
            let files = number_before(rest, " files (");
            let rewritten = number_before(rest, " rewritten)");
            if let (Some(files), Some(rewritten)) = (files, rewritten) {
                parts.push(format!("emit rewrote {rewritten}/{files} C files"));
            }
        } else if line.starts_with("clang (") {
            let compiled = number_before(line, " compiled");
            let reused = number_before(line, " reused");
            if let (Some(compiled), Some(reused)) = (compiled, reused) {
                parts.push(format!("clang reused {reused}/{} shards ({compiled} compiled)", compiled + reused));
            }
        }
    }
    if parts.is_empty() {
        "lab: h2 build printed no phase lines".into()
    } else {
        format!("lab: image reuse: {}", parts.join(", "))
    }
}

/// The decimal number just before the first `marker` in `s`.
fn number_before(s: &str, marker: &str) -> Option<u64> {
    let head = &s[..s.find(marker)?];
    let start = head
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1);
    head[start..].parse().ok()
}

/// What a step wrote to `log` after `offset`, at most its last 4 MiB.
pub(super) fn log_since(log: &Path, offset: u64) -> String {
    const MAX: u64 = 4 << 20;
    let Ok(mut file) = File::open(log) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let from = offset.max(len.saturating_sub(MAX));
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_ok() {
        let _ = file.take(MAX).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The build variant: everything that picks an image except the commit
/// (sorted features, build variables, the PGO profile's content hash).
/// Every commit of one variant builds in the same work directory, so
/// `h2 build` rewrites only the C that changed and clang compiles only
/// those shards; another `H2_OPT` or feature set gets its own directory.
pub(super) fn variant_key(
    features: &[String],
    env: &BTreeMap<String, String>,
    pgo_hash: &str,
) -> String {
    let mut s = String::from("variant\n");
    for f in features {
        s += &format!("feature {f}\n");
    }
    for (k, v) in env {
        s += &format!("env {k}={v}\n");
    }
    s += &format!("pgo {pgo_hash}\n");
    proto::to_hex(&Sha256::digest(s.as_bytes()))[..16].to_string()
}

/// `variants/<key>`: the variant's work directory. `h2 build` uses it as
/// its `--out`, and it is the published image its `image.json` names.
pub(super) fn variant_dir(cfg: &HostConfig, key: &str) -> PathBuf {
    cfg.dir("variants").join(key)
}

/// Holds a variant's work directory for one build. The activity lock
/// already allows one build at a time; this refuses a second builder of
/// the same directory outright rather than letting two interleave.
pub(super) fn lock_variant(dir: &Path) -> Result<File> {
    let path = dir.join("lab-build.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => bail!("another build is using {}", dir.display()),
        Err(TryLockError::Error(err)) => Err(err.into()),
    }
}

/// The commit a work directory's stamp names: what a rebuild starts from.
pub(super) fn stamped_commit(dir: &Path) -> Option<String> {
    let stamp = fs::read(dir.join("image.json")).ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&stamp).ok()?;
    value["commit"].as_str().map(str::to_owned)
}

/// Marks a variant used, so pruning keeps the ones used last.
pub(super) fn touch(dir: &Path) {
    let _ = File::options()
        .append(true)
        .open(dir.join("image.json"))
        .and_then(|f| f.set_modified(SystemTime::now()));
}

/// Keeps `keep` and the other work directories used last (by their stamp,
/// else the directory), [`IMAGE_VARIANTS`] in all. Only real directories
/// named like a variant key are removed; anything else is left alone.
pub(super) fn prune_variants(cfg: &HostConfig, keep: &Path, log: &mut impl Write) -> Result<()> {
    let dirs = variant_dirs(cfg, Some(keep))?;
    for path in variants_to_remove(dirs, IMAGE_VARIANTS.saturating_sub(1)) {
        writeln!(log, "lab: removing image work directory {}", path.display())?;
        fs::remove_dir_all(&path)?;
    }
    Ok(())
}

/// The variant work directories except `keep`, with when each was last
/// used (its stamp, else the directory).
fn variant_dirs(cfg: &HostConfig, keep: Option<&Path>) -> Result<Vec<(SystemTime, PathBuf)>> {
    let mut dirs = Vec::new();
    let entries = match fs::read_dir(cfg.dir("variants")) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(dirs),
        other => other?,
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let ours =
            name.len() == 16 && name.as_encoded_bytes().iter().all(u8::is_ascii_hexdigit);
        if !ours || !entry.file_type()?.is_dir() || Some(path.as_path()) == keep {
            continue;
        }
        let used = fs::metadata(path.join("image.json"))
            .or_else(|_| entry.metadata())
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        dirs.push((used, path));
    }
    Ok(dirs)
}

/// Those of `dirs` past the `keep` used last.
fn variants_to_remove(mut dirs: Vec<(SystemTime, PathBuf)>, keep: usize) -> Vec<PathBuf> {
    dirs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    dirs.into_iter().skip(keep).map(|(_, p)| p).collect()
}

pub(super) fn checkout_command(cfg: &HostConfig, commit: &str) -> Result<Command> {
    let parent = cfg.dir("src");
    ensure_dir(&parent)?;
    let checkout = parent.join("current");
    let exists =
        crate::follow::owned_worktree(&cfg.repo, &parent, "current").map_err(anyhow::Error::msg)?;
    let mut cmd = transfer::git(if exists { &checkout } else { &cfg.repo });
    if exists {
        // Restore tracked files while checking out the commit. Build outputs
        // live outside src/current. No untracked checkout-wide clean.
        cmd.args(["checkout", "--detach", "--force", commit]);
    } else {
        cmd.args(["worktree", "add", "--force", "--detach"])
            .arg(checkout)
            .arg(commit);
    }
    Ok(cmd)
}

pub(super) fn matches_stamp(dir: &Path, key: &str) -> Result<bool> {
    let stamp = match fs::read(dir.join("image.json")) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        other => other?,
    };
    let value = serde_json::from_slice::<serde_json::Value>(&stamp).unwrap_or_default();
    Ok(value["key"].as_str() == Some(key)
        && dir.join("h2").is_file()
        && dir
            .join(emit::compile::Arch::host().module_file_name())
            .is_file())
}

/// Old hosts hold the activity lock in the daemon. After an abnormal exit,
/// a child may still be alive with its stdout/stderr redirected to the
/// job logs. Check those files before migrating or admitting another job.
/// Do not pass an inheritable lock to build tools (their cache daemons can
/// outlive a build). Healthy restarts have no interrupted records to check.
pub(super) fn check_abandoned(run: &Path) -> Result<()> {
    let files: Vec<_> = ["run.log", "build.log"]
        .into_iter()
        .map(|log| run.join(log))
        .filter(|log| log.is_file())
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    let open = Command::new("lsof")
        .args(["-nP", "-w", "-t", "-a", "-d", "1,2", "--"])
        .args(files)
        .output()
        .context("checking interrupted lab job logs with lsof")?;
    ensure!(
        open.stdout.is_empty() && open.status.code() == Some(1),
        "cannot recover interrupted job {} while its logs may have a live writer (pids {}, {}); stop the old job before starting the lab host",
        run.display(),
        String::from_utf8_lossy(&open.stdout).trim(),
        String::from_utf8_lossy(&open.stderr).trim()
    );
    Ok(())
}

/// Cargo may not notice removal of individual build-script outputs. Keep
/// the whole target warm while it fits and clear it as a unit otherwise.
fn size(dir: &Path) -> Result<u64> {
    let mut bytes = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let len = if kind.is_dir() {
            size(&entry.path())?
        } else if kind.is_file() {
            entry.metadata()?.len()
        } else {
            0
        };
        bytes = bytes.saturating_add(len);
    }
    Ok(bytes)
}

pub(super) fn prune(cfg: &HostConfig, log: &mut impl Write) -> Result<()> {
    let objects = obj_cache_dir(cfg);
    ensure_dir(&objects)?;
    let pruned = emit::compile::prune_obj_cache(&objects, cfg.obj_cache_limit, &Default::default())
        .map_err(anyhow::Error::msg)?;
    if pruned.removed != 0 {
        writeln!(
            log,
            "lab: pruned {} cache objects, {} bytes remain",
            pruned.removed, pruned.kept
        )?;
    }
    // The cache is shared with the Mac's own builds, which may add objects
    // while this prunes; that is no reason to fail the job.
    if pruned.kept > cfg.obj_cache_limit {
        writeln!(
            log,
            "lab: object cache {} holds {} bytes, over its {} byte limit",
            objects.display(),
            pruned.kept,
            cfg.obj_cache_limit
        )?;
    }
    let target = cfg.dir("target");
    ensure_dir(&target)?;
    let bytes = size(&target)?;
    if bytes > cfg.target_limit {
        writeln!(
            log,
            "lab: clearing Cargo target ({bytes} bytes exceeds {})",
            cfg.target_limit
        )?;
        fs::remove_dir_all(&target)?;
        fs::create_dir(target)?;
    }
    Ok(())
}

/// The lab volume's free and total bytes, as `h2 lab status` reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) struct Disk {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// Free space on the volume holding `path` (what a non-root user can use).
#[allow(clippy::unnecessary_cast)]
pub(super) fn disk(path: &Path) -> Option<Disk> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `st` is a valid out-pointer.
    if unsafe { libc::statvfs(path.as_ptr(), st.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs succeeded, so it filled `st`.
    let st = unsafe { st.assume_init() };
    let unit = st.f_frsize as u64;
    Some(Disk {
        free_bytes: (st.f_bavail as u64).saturating_mul(unit),
        total_bytes: (st.f_blocks as u64).saturating_mul(unit),
    })
}

/// Bytes as GiB for messages: `20`, or `3.5`.
pub(super) fn gib(bytes: u64) -> String {
    let unit = 1u64 << 30;
    if bytes % unit == 0 {
        (bytes / unit).to_string()
    } else {
        format!("{:.1}", bytes as f64 / unit as f64)
    }
}

/// Why a build is refused when pruning could not clear the floor.
pub(super) fn floor_message(free: u64, floor: u64) -> String {
    format!(
        "lab: only {} GiB free on the lab disk (floor {} GiB) after pruning; free space on the laptop",
        gib(free),
        gib(floor)
    )
}

/// What a reclaim removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Goal {
    /// Stop as soon as this many bytes are free (the disk floor).
    Until(u64),
    /// Everything the floor's order prunes, to its end: runs past the
    /// newest [`RUNS_KEPT`], all variants but the one used last, the
    /// object cache down to half its budget, the Cargo target.
    Default,
    /// Finished runs past the newest [`RUNS_KEPT`] only (a job is active).
    Runs,
    /// Every finished run, every variant, the whole object cache and
    /// Cargo target, and the source checkout.
    All,
}

/// Bytes a reclaim freed, per category.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(super) struct Freed {
    pub runs: u64,
    pub variants: u64,
    pub object_cache: u64,
    pub cargo_target: u64,
    pub checkout: u64,
}

impl Freed {
    pub(super) fn total(&self) -> u64 {
        self.runs + self.variants + self.object_cache + self.cargo_target + self.checkout
    }
}

fn goal_met(goal: Goal, free: &mut impl FnMut() -> Option<u64>) -> bool {
    matches!(goal, Goal::Until(floor) if free().is_none_or(|bytes| bytes >= floor))
}

/// Finished job runs, newest first. A run whose metadata is missing,
/// unreadable or not final may belong to a live job and is left alone.
fn finished_runs(cfg: &HostConfig) -> Vec<(SystemTime, PathBuf)> {
    let mut runs = Vec::new();
    let Ok(entries) = fs::read_dir(cfg.dir("runs")) else {
        return runs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let finished = super::host::load_meta(&path)
            .ok()
            .flatten()
            .is_some_and(|m| super::host::is_final(&m.state));
        if finished {
            let used = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            runs.push((used, path));
        }
    }
    runs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    runs
}

/// Frees disk in the order that costs least: old job runs, the least
/// recently used variant work directories (never `keep`), the object cache
/// down to half its budget, then the Cargo target. `free` reports the
/// volume's free bytes; with [`Goal::Until`] the reclaim stops once it is
/// enough. The caller holds the activity lock, so nothing here is in use
/// except `keep` and runs that are not final.
pub(super) fn reclaim(
    cfg: &HostConfig,
    goal: Goal,
    keep: Option<&Path>,
    free: &mut impl FnMut() -> Option<u64>,
    log: &mut impl Write,
) -> Result<Freed> {
    let mut freed = Freed::default();
    let kept_runs = if goal == Goal::All { 0 } else { RUNS_KEPT };
    let old: Vec<_> = finished_runs(cfg).into_iter().skip(kept_runs).collect();
    for (_, dir) in old.into_iter().rev() {
        if goal_met(goal, free) {
            return Ok(freed);
        }
        let bytes = size(&dir).unwrap_or(0);
        writeln!(log, "lab: removing job run {}", dir.display())?;
        fs::remove_dir_all(&dir)?;
        freed.runs += bytes;
    }
    if goal == Goal::Runs {
        return Ok(freed);
    }

    let mut dirs = variant_dirs(cfg, keep)?;
    dirs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    if keep.is_none() && goal != Goal::All {
        // The variant used last is the one the next job most likely wants.
        dirs.pop();
    }
    for (_, dir) in dirs {
        if goal_met(goal, free) {
            return Ok(freed);
        }
        let bytes = size(&dir).unwrap_or(0);
        writeln!(log, "lab: removing image work directory {}", dir.display())?;
        fs::remove_dir_all(&dir)?;
        freed.variants += bytes;
    }

    let objects = obj_cache_dir(cfg);
    if goal_met(goal, free) {
        return Ok(freed);
    }
    if objects.is_dir() {
        let limit = if goal == Goal::All { 0 } else { cfg.obj_cache_limit / 2 };
        let pruned = emit::compile::prune_obj_cache(&objects, limit, &Default::default())
            .map_err(anyhow::Error::msg)?;
        writeln!(
            log,
            "lab: pruned {} cache objects ({} bytes), {} bytes remain",
            pruned.removed, pruned.freed, pruned.kept
        )?;
        freed.object_cache += pruned.freed;
    }

    let target = target_dir(cfg);
    if goal_met(goal, free) {
        return Ok(freed);
    }
    if target.is_dir() {
        let bytes = size(&target)?;
        writeln!(log, "lab: clearing Cargo target ({bytes} bytes)")?;
        fs::remove_dir_all(&target)?;
        fs::create_dir(&target)?;
        freed.cargo_target += bytes;
    }

    if goal == Goal::All {
        let checkout = cfg.dir("src").join("current");
        if fs::symlink_metadata(&checkout).is_ok_and(|m| m.is_dir()) {
            let bytes = size(&checkout).unwrap_or(0);
            writeln!(log, "lab: removing the source checkout ({bytes} bytes)")?;
            fs::remove_dir_all(&checkout)?;
            // Drop the registration; the next build adds the worktree again.
            let _ = transfer::git(&cfg.repo).args(["worktree", "prune"]).status();
            freed.checkout += bytes;
        }
    }
    Ok(freed)
}

/// Before a build: when free space is under the floor, prune until it is
/// above, else fail. `keep` is the variant being built. `free` is the
/// volume's free bytes. A floor of zero turns the check off.
pub(super) fn ensure_free(
    cfg: &HostConfig,
    keep: &Path,
    free: &mut impl FnMut() -> Option<u64>,
    log: &mut impl Write,
) -> Result<()> {
    let floor = cfg.disk_floor;
    if floor == 0 {
        return Ok(());
    }
    let Some(start) = free() else { return Ok(()) };
    if start >= floor {
        return Ok(());
    }
    writeln!(
        log,
        "lab: {} GiB free on the lab disk, under its {} GiB floor; pruning caches",
        gib(start),
        gib(floor)
    )?;
    let freed = reclaim(cfg, Goal::Until(floor), Some(keep), free, log)?;
    let after = free().unwrap_or(u64::MAX);
    writeln!(log, "lab: pruning freed {} bytes, {} GiB free", freed.total(), gib(after))?;
    if after < floor {
        let reason = floor_message(after, floor);
        writeln!(log, "{reason}")?;
        bail!("{reason}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::host::image_key;
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("h2lab-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(root: &Path) -> HostConfig {
        HostConfig {
            root: root.to_owned(),
            socket: root.join("lab.sock"),
            repo: root.join("repo"),
            disc: root.join("halo2.iso"),
            h2: "/bin/true".into(),
            out: None,
            data: root.join("data"),
            cargo: "cargo".into(),
            obj_cache_limit: 0,
            target_limit: 0,
            disk_floor: 0,
            max_jobs: 1,
            lan: None,
            managed: false,
            explicit_h2: true,
            grace: Duration::from_secs(1),
            takeover: Default::default(),
            agents: root.join("agents"),
        }
    }

    fn env(v: &[(&str, &str)]) -> BTreeMap<String, String> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn one_variant_builds_every_commit_in_one_work_directory() {
        let cfg = config(Path::new("/lab"));
        let opt2_env = env(&[("H2_OPT", "2")]);
        let opt2 = variant_key(&[], &opt2_env, "");
        assert_eq!(opt2, variant_key(&[], &env(&[("H2_OPT", "2")]), ""));
        assert_eq!(variant_dir(&cfg, &opt2), Path::new("/lab/variants").join(&opt2));
        assert_eq!(opt2.len(), 16);
        assert!(opt2.bytes().all(|b| b.is_ascii_hexdigit()));
        // The commit picks the image within the directory, not the directory.
        assert_ne!(image_key("aaa", &[], &opt2_env, ""), image_key("bbb", &[], &opt2_env, ""));
        // Anything else that picks an image gets its own directory.
        for other in [
            variant_key(&[], &env(&[("H2_OPT", "3")]), ""),
            variant_key(&[], &BTreeMap::new(), ""),
            variant_key(&["perf".into()], &opt2_env, ""),
            variant_key(&[], &opt2_env, "f00d"),
        ] {
            assert_ne!(opt2, other);
            assert_ne!(variant_dir(&cfg, &opt2), variant_dir(&cfg, &other));
        }
    }

    #[test]
    fn the_work_directory_is_the_published_image_of_its_last_commit() {
        let root = scratch("publish");
        let cfg = config(&root);
        let dir = variant_dir(&cfg, &variant_key(&[], &BTreeMap::new(), ""));
        fs::create_dir_all(&dir).unwrap();
        let first = image_key("aaa", &[], &BTreeMap::new(), "");
        let second = image_key("bbb", &[], &BTreeMap::new(), "");
        assert!(!matches_stamp(&dir, &first).unwrap());
        assert_eq!(stamped_commit(&dir), None);
        fs::write(dir.join("h2"), b"h2").unwrap();
        fs::write(dir.join(emit::compile::Arch::host().module_file_name()), b"m").unwrap();
        let stamp = serde_json::json!({ "key": first, "commit": "aaa" });
        fs::write(dir.join("image.json"), stamp.to_string()).unwrap();
        assert!(matches_stamp(&dir, &first).unwrap());
        assert!(!matches_stamp(&dir, &second).unwrap());
        assert_eq!(stamped_commit(&dir).as_deref(), Some("aaa"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_second_builder_of_one_work_directory_is_refused() {
        let root = scratch("lock");
        let held = lock_variant(&root).unwrap();
        assert!(format!("{:#}", lock_variant(&root).unwrap_err()).contains("another build"));
        drop(held);
        lock_variant(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pruning_keeps_the_variants_used_last_and_leaves_other_entries() {
        let root = scratch("prune");
        let cfg = config(&root);
        let variants = cfg.dir("variants");
        let now = SystemTime::now();
        let mut keys = Vec::new();
        for age in 0..6u64 {
            let key = format!("{age:016x}");
            let dir = variants.join(&key);
            fs::create_dir_all(&dir).unwrap();
            let stamp = File::create(dir.join("image.json")).unwrap();
            stamp.set_modified(now - Duration::from_secs(60 * (age + 1))).unwrap();
            keys.push(key);
        }
        fs::create_dir_all(variants.join("other")).unwrap();
        // The variant in use stays even when it is the oldest.
        let keep = variants.join(&keys[5]);
        let mut log = Vec::new();
        prune_variants(&cfg, &keep, &mut log).unwrap();
        let mut left: Vec<String> = fs::read_dir(&variants)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        let mut want = vec![
            keys[0].clone(),
            keys[1].clone(),
            keys[2].clone(),
            keys[5].clone(),
            "other".to_string(),
        ];
        want.sort();
        assert_eq!(left, want);
        assert_eq!(String::from_utf8(log).unwrap().lines().count(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn variants_past_the_limit_go_oldest_first() {
        let t = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let dirs: Vec<(SystemTime, PathBuf)> =
            vec![(t(1), "a".into()), (t(3), "c".into()), (t(2), "b".into())];
        assert_eq!(variants_to_remove(dirs.clone(), 1), [PathBuf::from("b"), PathBuf::from("a")]);
        assert!(variants_to_remove(dirs, 3).is_empty());
    }

    #[test]
    fn jobs_and_updates_share_one_target_and_keep_the_users_wrapper() {
        let cfg = config(Path::new("/lab"));
        let cmd = cargo_command(&cfg, Path::new("/lab/src/current"), &["perf".into()]);
        let args: Vec<String> =
            cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let want = ["build", "--release", "-p", "cli", "--bin", "h2", "--features", "perf"];
        assert!(args.ends_with(&want.map(String::from)), "{args:?}");
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/lab/src/current")));
        let set: BTreeMap<String, std::ffi::OsString> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_owned())))
            .collect();
        assert_eq!(set["CARGO_TARGET_DIR"].as_os_str(), Path::new("/lab/target").as_os_str());
        assert_eq!(built_h2(&cfg), Path::new("/lab/target/release/h2"));
        // Image builds share the Mac's object cache beside the lab root.
        let default = config(Path::new("/Users/me/Library/Caches/h2recomp/lab"));
        assert_eq!(obj_cache_dir(&default), Path::new("/Users/me/Library/Caches/h2recomp/obj"));
        assert_eq!(obj_cache_dir(&config(Path::new("/"))), Path::new("/obj-cache"));
        for key in set.keys() {
            assert!(!key.starts_with("H2_"), "{key}");
            assert!(key != "RUSTC_WRAPPER" && key != "SCCACHE_DIR", "{key}");
        }
    }

    #[test]
    fn the_wrapper_comes_from_the_environment_then_the_nearest_config() {
        let root = scratch("wrapper");
        // Cargo reads every ancestor's .cargo before the home one, so a real
        // config above the scratch dir (a $TMPDIR under the user's home)
        // would win; the lookup order is only checkable without one.
        if root.ancestors().skip(1).any(|a| {
            a.join(".cargo/config.toml").exists() || a.join(".cargo/config").exists()
        }) {
            let _ = fs::remove_dir_all(root);
            return;
        }
        let src = root.join("lab/src/current");
        fs::create_dir_all(&src).unwrap();
        let home = root.join("home");
        fs::create_dir_all(home.join(".cargo")).unwrap();
        fs::write(home.join(".cargo/config.toml"), "[build]\nrustc-wrapper = \"/bin/kache\"\n")
            .unwrap();
        let home = home.to_string_lossy().into_owned();
        fn vars(home: &str, set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
            let mut map: BTreeMap<String, String> = env(set);
            map.insert("HOME".into(), home.to_string());
            move |k: &str| map.get(k).cloned()
        }
        let from_home = rustc_wrapper(&src, vars(&home, &[])).unwrap();
        assert_eq!(from_home.0, "/bin/kache");
        assert!(from_home.1.ends_with("home/.cargo/config.toml"), "{}", from_home.1);
        assert_eq!(
            rustc_wrapper(&src, vars(&home, &[("RUSTC_WRAPPER", "/bin/other")])),
            Some(("/bin/other".to_string(), "RUSTC_WRAPPER".to_string()))
        );
        assert_eq!(rustc_wrapper(&src, vars(&home, &[("RUSTC_WRAPPER", "")])), None);
        // A config nearer the source wins, and an empty value turns it off.
        fs::create_dir_all(root.join("lab/.cargo")).unwrap();
        fs::write(root.join("lab/.cargo/config.toml"), "[build]\nrustc-wrapper = \"\"\n").unwrap();
        assert_eq!(rustc_wrapper(&src, vars(&home, &[])), None);
        // One without the key leaves it to the next.
        fs::write(root.join("lab/.cargo/config.toml"), "[alias]\nx = \"y\"\n").unwrap();
        assert_eq!(rustc_wrapper(&src, vars(&home, &[])).unwrap().0, "/bin/kache");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_build_log_says_what_was_reused() {
        let cargo = "   Compiling app v0.1.0 (/lab/src/current/crates/apps/app)\n\
                     \x20  Compiling cli v0.1.0 (/lab/src/current/crates/apps/cli)\n\
                     \x20   Finished `release` profile [optimized] target(s) in 41.20s\n";
        let wrapper = ("/bin/kache".to_string(), "/home/.cargo/config.toml".to_string());
        let line = cargo_summary(cargo, Some(&wrapper));
        assert!(line.contains("compiled 2 crates"), "{line}");
        assert!(line.contains("rustc wrapper /bin/kache"), "{line}");
        assert!(cargo_summary("", None).contains("no rustc wrapper"));

        let build = "analyze: 0.4s, 41234 functions (cached)\n\
            lift: 9.1s, 41000 lifted, 0 not lifted\n\
            emit: 6.0s, 39000 owners (120 cut into parts), 1043 files (12 rewritten), 812.3 MB of C, 41000 entries, 900 resumes\n\
            clang (arm64): 12 compiled, 1031 reused, 900 sources in the cold tier, 3.1s keying (dependency scans, hashing, pins), 20.0s compiling, 1.0s archive, 400.0 MB of objects\n";
        assert_eq!(
            image_summary(build),
            "lab: image reuse: analysis cached, emit rewrote 12/1043 C files, clang reused 1031/1043 shards (12 compiled)"
        );
        assert!(image_summary("analyze: 30.0s, 41234 functions\n").contains("analysis recomputed"));
        assert!(image_summary("fake build\n").contains("no phase lines"));
    }

    #[test]
    fn the_log_tail_starts_where_the_step_did() {
        let root = scratch("tail");
        let log = root.join("build.log");
        fs::write(&log, b"before\nafter\n").unwrap();
        assert_eq!(log_since(&log, 7), "after\n");
        assert_eq!(log_since(&root.join("missing"), 0), "");
        let _ = fs::remove_dir_all(root);
    }
    const GIB: u64 = 1 << 30;

    fn aged(path: &Path, age: u64) {
        File::open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(age))
            .unwrap();
    }

    fn run_dir(cfg: &HostConfig, id: &str, state: &str, age: u64) {
        let dir = cfg.dir("runs").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), format!(r#"{{"id":"{id}","state":"{state}"}}"#)).unwrap();
        fs::write(dir.join("run.log"), [0u8; 10]).unwrap();
        aged(&dir, age);
    }

    fn variant(cfg: &HostConfig, n: u64, age: u64) -> PathBuf {
        let dir = cfg.dir("variants").join(format!("{n:016x}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("big.bin"), [0u8; 100]).unwrap();
        let stamp = File::create(dir.join("image.json")).unwrap();
        stamp.set_modified(SystemTime::now() - Duration::from_secs(age)).unwrap();
        dir
    }

    /// 35 finished runs (`0000` newest), a running one, three variants
    /// (the oldest in use), an object and a Cargo target file.
    fn full_lab(name: &str) -> (PathBuf, HostConfig, PathBuf) {
        let root = scratch(name).join("lab");
        fs::create_dir_all(&root).unwrap();
        let mut cfg = config(&root);
        cfg.disk_floor = 20 * GIB;
        for i in 0..35u64 {
            run_dir(&cfg, &format!("{i:04}"), "done", 1000 + i * 10);
        }
        run_dir(&cfg, "9999", "running", 100_000);
        variant(&cfg, 1, 10);
        variant(&cfg, 2, 20);
        let keep = variant(&cfg, 3, 30);
        fs::create_dir_all(obj_cache_dir(&cfg)).unwrap();
        fs::write(obj_cache_dir(&cfg).join("a.o"), [0u8; 100]).unwrap();
        fs::create_dir_all(target_dir(&cfg)).unwrap();
        fs::write(target_dir(&cfg).join("deps.rlib"), [0u8; 100]).unwrap();
        (root, cfg, keep)
    }

    fn count(dir: &Path) -> usize {
        fs::read_dir(dir).map_or(0, Iterator::count)
    }

    #[test]
    fn under_the_floor_a_build_prunes_in_order_and_then_fails_with_the_reason() {
        let (root, cfg, keep) = full_lab("floor-order");
        let mut log = Vec::new();
        let err = ensure_free(&cfg, &keep, &mut || Some(3 * GIB + GIB / 2), &mut log).unwrap_err();
        let reason = "lab: only 3.5 GiB free on the lab disk (floor 20 GiB) after pruning; free space on the laptop";
        assert_eq!(format!("{err:#}"), reason);
        let log = String::from_utf8(log).unwrap();
        assert!(log.trim_end().ends_with(reason), "{log}");
        let at = |what: &str| log.find(what).unwrap_or_else(|| panic!("{what} not in {log}"));
        assert!(at("removing job run") < at("removing image work directory"));
        assert!(at("removing image work directory") < at("pruned "));
        assert!(at("pruned ") < at("clearing Cargo target"));
        // Runs: the oldest five finished ones go; the newest 30 and the
        // running job stay. Variants: all but the one in use.
        let runs = cfg.dir("runs");
        assert!(!runs.join("0034").exists() && !runs.join("0030").exists());
        assert!(runs.join("0029").exists() && runs.join("9999").exists());
        assert_eq!(count(&runs), 31);
        assert_eq!(count(&cfg.dir("variants")), 1);
        assert!(keep.exists());
        assert_eq!(count(&obj_cache_dir(&cfg)), 0);
        assert_eq!(count(&target_dir(&cfg)), 0);
        assert!(target_dir(&cfg).is_dir());
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn pruning_stops_at_the_first_step_that_clears_the_floor() {
        let (root, cfg, keep) = full_lab("floor-stop");
        let variants = cfg.dir("variants");
        // Free space is enough once only the variant in use is left.
        let mut free = || Some(if count(&variants) == 1 { 25 * GIB } else { GIB });
        let mut log = Vec::new();
        ensure_free(&cfg, &keep, &mut free, &mut log).unwrap();
        assert_eq!(count(&variants), 1);
        assert!(!cfg.dir("runs").join("0034").exists());
        assert_eq!(count(&obj_cache_dir(&cfg)), 1);
        assert_eq!(count(&target_dir(&cfg)), 1);
        // Plenty of space: nothing is touched.
        let (root2, cfg2, keep2) = full_lab("floor-ok");
        ensure_free(&cfg2, &keep2, &mut || Some(21 * GIB), &mut Vec::new()).unwrap();
        assert_eq!(count(&cfg2.dir("runs")), 36);
        assert_eq!(count(&cfg2.dir("variants")), 3);
        // A floor of zero is off, however little is free.
        let mut off = cfg2.clone();
        off.disk_floor = 0;
        ensure_free(&off, &keep2, &mut || Some(0), &mut Vec::new()).unwrap();
        assert_eq!(count(&cfg2.dir("runs")), 36);
        let _ = fs::remove_dir_all(root.parent().unwrap());
        let _ = fs::remove_dir_all(root2.parent().unwrap());
    }

    #[test]
    fn a_clean_reports_bytes_freed_per_category() {
        let (root, cfg, _) = full_lab("clean");
        let checkout = cfg.dir("src").join("current");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(checkout.join("lib.rs"), [0u8; 1000]).unwrap();
        // Default: the variant used last stays, runs past 30 go, caches go.
        let mut log = Vec::new();
        let freed = reclaim(&cfg, Goal::Default, None, &mut || None, &mut log).unwrap();
        assert_eq!(freed.runs, 5 * (10 + "{\"id\":\"0030\",\"state\":\"done\"}".len() as u64));
        assert_eq!(freed.variants, 2 * (100 + 0));
        assert_eq!(freed.object_cache, 100);
        assert_eq!(freed.cargo_target, 100);
        assert_eq!(freed.checkout, 0);
        assert_eq!(count(&cfg.dir("variants")), 1);
        assert!(checkout.exists());
        // All: every finished run, the last variant and the checkout too,
        // but not the running job.
        let freed = reclaim(&cfg, Goal::All, None, &mut || None, &mut log).unwrap();
        assert_eq!(count(&cfg.dir("runs")), 1);
        assert!(cfg.dir("runs").join("9999").exists());
        assert_eq!(count(&cfg.dir("variants")), 0);
        assert_eq!(freed.checkout, 1000);
        assert!(!checkout.exists());
        // With a job active only finished runs go.
        let (root2, cfg2, _) = full_lab("clean-runs");
        let freed = reclaim(&cfg2, Goal::Runs, None, &mut || None, &mut Vec::new()).unwrap();
        assert!(freed.runs > 0 && freed.total() == freed.runs);
        assert_eq!(count(&cfg2.dir("variants")), 3);
        assert_eq!(count(&target_dir(&cfg2)), 1);
        let _ = fs::remove_dir_all(root.parent().unwrap());
        let _ = fs::remove_dir_all(root2.parent().unwrap());
    }

    #[test]
    fn disk_serialises_as_free_and_total_bytes() {
        let json = serde_json::to_value(Disk { free_bytes: 5, total_bytes: 9 }).unwrap();
        assert_eq!(json, serde_json::json!({ "free_bytes": 5, "total_bytes": 9 }));
        let here = disk(&std::env::temp_dir()).unwrap();
        assert!(here.total_bytes > 0 && here.free_bytes <= here.total_bytes);
        assert!(disk(Path::new("/definitely/not/here")).is_none());
    }
}
