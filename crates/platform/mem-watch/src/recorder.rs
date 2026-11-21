//! Bounded, newline-committed memory evidence. The lifetime lock identifies a
//! writer even across PID reuse; fetched copies have no live lock owner.
mod analysis;
mod archive;
pub use analysis::{Change, Diff, Postmortem, diff, diff_since, postmortem, postmortem_file};
pub use archive::recover;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Time is monotonic watcher uptime (t_ms) and wall time (unix_ms).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Record {
    pub pid: u32,
    pub t_ms: u64,
    pub unix_ms: u64,
    pub footprint_bytes: u64,
    #[serde(default)]
    pub soft: u64,
    #[serde(default)]
    pub probes: BTreeMap<String, u64>,
    #[serde(default)]
    pub tags: BTreeMap<String, u64>,
    #[serde(default)]
    pub gpu: BTreeMap<String, u64>,
    #[serde(default)]
    pub stacks: BTreeMap<String, u64>,
    pub level: Option<String>,
    pub tick: Option<u64>,
    #[serde(default)]
    pub agent_connections: u64,
    #[serde(default)]
    pub agent_calls: u64,
}

const HOUR: u64 = 3_600_000;
const SEGMENTS: usize = 7;
const SEGMENT_BYTES: u64 = 8 << 20;
const ROW_BYTES: usize = 128 << 10;
const CLOSED: &[u8] = b"{\"closed_cleanly\":true}\n";

pub struct Recorder {
    dir: PathBuf,
    pid: u32,
    _owner: File,
    active: File,
    size: u64,
    started_ms: Option<u64>,
    closed: bool,
}

fn io_err(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn private() -> OpenOptions {
    let mut opts = OpenOptions::new();
    opts.write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    opts
}

fn segment(dir: &Path, pid: u32, index: usize) -> PathBuf {
    if index == 0 {
        dir.join(format!("mem-{pid}.ndjson"))
    } else {
        dir.join(format!("mem-{pid}.ndjson.{index}"))
    }
}

fn current_pid(name: &str) -> Option<u32> {
    let name = name.strip_prefix("mem-")?;
    let (pid, suffix) = name.split_once('.')?;
    if suffix == "owner"
        || suffix == "ndjson"
        || suffix
            .strip_prefix("ndjson.")
            .and_then(|s| s.parse::<usize>().ok())
            .is_some_and(|n| (1..SEGMENTS).contains(&n))
    {
        pid.parse().ok()
    } else {
        None
    }
}

fn regular(path: &Path) -> io::Result<bool> {
    match path.symlink_metadata() {
        Ok(meta) => Ok(meta.is_file()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

impl Recorder {
    /// Recovers interrupted recordings, reserves the bounded disk budget,
    /// then opens the current process's stream. Call on the recorder thread.
    pub fn open(dir: &Path) -> io::Result<Self> {
        archive::open(dir, std::process::id(), archive::ARCHIVE_BYTES)
    }

    pub fn append(&mut self, record: &Record) -> io::Result<()> {
        if self.closed {
            return Err(io_err("memory recorder is closed"));
        }
        if self.pid != record.pid {
            return Err(io_err("memory recorder PID mismatch"));
        }
        let mut row = serde_json::to_vec(record)?;
        row.push(b'\n');
        if row.len() > ROW_BYTES {
            return Err(io_err("memory recorder row exceeds 128 KiB"));
        }
        // Reserve the footer and rotate before the allocation would exceed
        // the budget, even if a workload changes hundreds of probe values.
        if self
            .started_ms
            .is_some_and(|start| record.t_ms.saturating_sub(start) >= HOUR)
            || self.size + row.len() as u64 + CLOSED.len() as u64 > SEGMENT_BYTES
        {
            self.rotate()?;
        }
        // Write directly to the OS. A killed process loses at most a partial
        // final line. Power failure has ordinary filesystem durability.
        self.active.write_all(&row)?;
        self.size += row.len() as u64;
        self.started_ms.get_or_insert(record.t_ms);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Remove only our oldest *live* generation. Archived sessions are immutable.
        remove_if_exists(&segment(&self.dir, self.pid, SEGMENTS - 1))?;
        for n in (0..SEGMENTS - 1).rev() {
            let path = segment(&self.dir, self.pid, n);
            if regular(&path)? {
                fs::rename(path, segment(&self.dir, self.pid, n + 1))?;
            }
        }
        self.active = private()
            .create_new(true)
            .open(segment(&self.dir, self.pid, 0))?;
        self.size = 0;
        self.started_ms = None;
        Ok(())
    }

    /// Called only on an orderly exit. Dropping the object without close also
    /// releases its OS lock, leaving an unclean recorder for the next opener.
    pub fn close(&mut self) -> io::Result<()> {
        if !self.closed {
            self.active.write_all(CLOSED)?;
            self.closed = true;
        }
        Ok(())
    }

    pub fn history(&self, since_ms: Option<u64>) -> io::Result<Vec<Record>> {
        self.history_page(since_ms, usize::MAX, usize::MAX)
    }

    /// Return a chronological prefix; callers continue after the last t_ms.
    pub fn history_page(
        &self,
        since_ms: Option<u64>,
        limit: usize,
        byte_limit: usize,
    ) -> io::Result<Vec<Record>> {
        let mut rows = Vec::new();
        let mut used = 0;
        walk(&self.dir, self.pid, |row, size| {
            if since_ms.is_none_or(|t| row.t_ms >= t) {
                if rows.len() == limit || size > byte_limit - used {
                    return false;
                }
                used += size;
                rows.push(row);
            }
            true
        })?;
        Ok(rows)
    }

    pub fn diff_since(&self, from_ms: u64) -> io::Result<Diff> {
        let mut first = None;
        let mut last = None;
        walk(&self.dir, self.pid, |r, _| {
            if first.is_none() || r.t_ms <= from_ms {
                first = Some(r.clone());
            }
            last = Some(r);
            true
        })?;
        analysis::finish(first.as_ref(), last.as_ref(), Some(from_ms))
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// Returns true if the visitor read to the end. A clean footer is only
/// meaningful at EOF. An unterminated record has never been committed.
fn read_segment(
    path: &Path,
    mut visit: impl FnMut(Record, usize) -> bool,
) -> io::Result<(bool, bool)> {
    use std::io::Read;
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    let mut clean = false;
    loop {
        line.clear();
        let len = (&mut reader)
            .take(ROW_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if len == 0 {
            break;
        }
        clean = false;
        if len > ROW_BYTES {
            return Err(io_err("oversized memory recorder row"));
        }
        if !line.ends_with(b"\n") {
            break;
        }
        if line == CLOSED {
            clean = true;
            continue;
        }
        if !visit(serde_json::from_slice(&line)?, len) {
            return Ok((false, false));
        }
    }
    Ok((true, clean))
}

fn walk(dir: &Path, pid: u32, mut visit: impl FnMut(Record, usize) -> bool) -> io::Result<()> {
    for n in (0..SEGMENTS).rev() {
        let path = segment(dir, pid, n);
        if regular(&path)? && !read_segment(&path, &mut visit)?.0 {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
