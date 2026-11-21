//! The lab's one detached checkout, game image and bounded build caches.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, ensure};

use super::{host::HostConfig, proto, transfer};

pub(super) const DEFAULT_OBJECT_LIMIT: u64 = 8 << 30;
pub(super) const DEFAULT_TARGET_LIMIT: u64 = 32 << 30;

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

/// Old images have 16-hex keys and old linked checkouts have full git IDs.
/// Select only direct, real children of the lab-owned cache directories.
/// Git removes both the worktree and its registration in one operation.
pub(super) fn migrate(cfg: &HostConfig) -> Result<()> {
    let src = cfg.dir("src");
    ensure_dir(&src)?;
    for entry in fs::read_dir(&src)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if proto::check_commit(name).is_err() || !entry.file_type()?.is_dir() {
            continue;
        }
        if !crate::follow::owned_worktree(&cfg.repo, &src, name).map_err(anyhow::Error::msg)? {
            continue;
        }
        let status = transfer::git(&cfg.repo)
            .args(["worktree", "remove", "--force"])
            .arg(entry.path())
            .status()?;
        ensure!(
            status.success(),
            "removing old lab checkout {}",
            entry.path().display()
        );
    }
    let images = cfg.dir("images");
    match fs::symlink_metadata(&images) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Ok(meta) if !meta.is_dir() => return Ok(()),
        other => {
            other?;
        }
    }
    for entry in fs::read_dir(&images)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.as_encoded_bytes();
        if name.len() == 16 && name.iter().all(u8::is_ascii_hexdigit) && entry.file_type()?.is_dir()
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    if fs::read_dir(&images)?.next().is_none() {
        fs::remove_dir(images)?;
    }
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
    let objects = cfg.dir("obj-cache");
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
    ensure!(
        pruned.kept <= cfg.obj_cache_limit,
        "failed to prune object cache to its size limit"
    );
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
