//! Reserve one full rotation per live recorder. When preserved evidence fills
//! the budget, refuse the next reservation until the operator moves an archive.
use super::*;
use std::collections::BTreeSet;
use std::fs::TryLockError;
use std::time::{SystemTime, UNIX_EPOCH};

const RESERVATION: u64 = SEGMENTS as u64 * SEGMENT_BYTES;
pub(super) const ARCHIVE_BYTES: u64 = 512 << 20;

fn registry(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let file = private()
        .create(true)
        .truncate(false)
        .open(dir.join(".mem-recorders.lock"))?;
    file.lock()?;
    Ok(file)
}

pub(super) fn pids(dir: &Path) -> io::Result<BTreeSet<u32>> {
    let mut found = BTreeSet::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && let Some(pid) = entry.file_name().to_str().and_then(current_pid)
        {
            found.insert(pid);
        }
    }
    Ok(found)
}

fn owner(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("mem-{pid}.owner"))
}

fn try_owner(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

pub(super) fn is_dead(dir: &Path, pid: u32) -> io::Result<bool> {
    match File::open(owner(dir, pid)) {
        Ok(file) => try_owner(&file),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
}

fn recovered_dir(dir: &Path, pid: u32) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.join("reports").join(format!("mem-{pid}-{nanos}"))
}

fn recover_locked(dir: &Path) -> io::Result<u64> {
    let mut live: u64 = 0;
    for pid in pids(dir)? {
        if !is_dead(dir, pid)? {
            live += 1;
            continue;
        }
        let mut last = None;
        let mut clean = false;
        let mut files = Vec::new();
        for n in (0..SEGMENTS).rev() {
            let path = segment(dir, pid, n);
            if !regular(&path)? {
                continue;
            }
            files.push(path);
        }
        // Usually the active segment holds the final footprint. Immediately
        // after rotation it can contain only a footer; use its predecessor.
        for (i, path) in files.iter().rev().enumerate() {
            let state = read_segment(path, |r, _| {
                last = Some(r);
                true
            });
            if i == 0 {
                clean = state.as_ref().is_ok_and(|r| r.1);
            }
            if last.is_some() || state.is_err() {
                break;
            }
        }
        let over_soft = last
            .as_ref()
            .is_some_and(|r| r.soft != 0 && r.footprint_bytes > r.soft);
        if !files.is_empty() && (over_soft || !clean) {
            let dest = recovered_dir(dir, pid);
            fs::create_dir_all(&dest)?;
            for file in files {
                fs::rename(&file, dest.join(file.file_name().unwrap()))?;
            }
            let reason = if clean {
                "last sample exceeded soft limit"
            } else {
                "unclean exit"
            };
            eprintln!(
                "memory recorder: recovered pid {pid} ({reason}): {}",
                dest.display()
            );
        } else {
            for file in files {
                remove_if_exists(&file)?;
            }
        }
        remove_if_exists(&owner(dir, pid))?;
    }
    Ok(live.saturating_mul(RESERVATION))
}

/// Recover abandoned writers in this report directory; safe to call while
/// other processes (or another lab instance) are actively recording.
pub fn recover(dir: &Path) -> io::Result<()> {
    if !dir.try_exists()? {
        return Ok(());
    }
    let _guard = registry(dir)?;
    recover_locked(dir).map(drop)
}

pub(super) fn archived_dirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    let reports = dir.join("reports");
    if !reports.try_exists()? {
        return Ok(dirs);
    }
    for entry in fs::read_dir(reports)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.file_name().to_string_lossy().starts_with("mem-") {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

fn archive_size(dir: &Path) -> io::Result<u64> {
    let mut bytes: u64 = 0;
    for archive in archived_dirs(dir)? {
        for entry in fs::read_dir(archive)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                bytes = bytes.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(bytes)
}

pub(super) fn open(dir: &Path, pid: u32, budget: u64) -> io::Result<Recorder> {
    let _guard = registry(dir)?;
    let reserved = recover_locked(dir)?;
    if reserved
        .saturating_add(archive_size(dir)?)
        .saturating_add(RESERVATION)
        > budget
    {
        return Err(io::Error::other(format!(
            "recorder disk budget full; move preserved evidence from {}",
            dir.join("reports").display()
        )));
    }
    let owner_file = private().create_new(true).open(owner(dir, pid))?;
    if !try_owner(&owner_file)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "memory recorder owner is locked",
        ));
    }
    let active = private().create_new(true).open(segment(dir, pid, 0))?;
    Ok(Recorder {
        dir: dir.into(),
        pid,
        _owner: owner_file,
        active,
        size: 0,
        started_ms: None,
        closed: false,
    })
}
