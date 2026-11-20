//! Finding running games (docs/runtime/agent-api.md, "Connecting").
//!
//! A game that isn't told where to listen (`h2 play` with `H2_AGENT`
//! unset) opens one socket per process in [`dir`]: `<pid>.sock`. A client
//! lists that directory ([`scan`]), asks each socket for `session.status`,
//! and drops the files nothing answers on (a game that was killed leaves
//! its file behind).
//!
//! `H2_AGENT=off` (or `0`, `none`, `false`, `no`) switches the default
//! socket off ([`is_off`]); `H2_AGENT=<path>` names the socket as before
//! and is not published here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{Client, Endpoint, Status, method};

/// The directory of per-process sockets, under [`crate::cache_dir`].
pub const DIR: &str = "agents";

/// How long [`scan`] gives one socket to answer `session.status`.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Where games publish their default sockets.
pub fn dir() -> PathBuf {
    crate::cache_dir().join(DIR)
}

/// The socket a game with process id `pid` opens in `dir`.
pub fn socket_in(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.sock"))
}

/// The socket this process opens by default.
pub fn process_socket() -> PathBuf {
    socket_in(&dir(), std::process::id())
}

/// The process id a socket's file name carries (`<pid>.sock`).
pub fn pid_of(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_suffix(".sock")?
        .parse()
        .ok()
}

/// Whether an `H2_AGENT` value switches the default socket off.
pub fn is_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "none" | "false" | "no"
    )
}

/// Whether `$H2_AGENT` switches the default socket off.
pub fn off_from_env() -> bool {
    std::env::var(crate::ENV).is_ok_and(|v| is_off(&v))
}

/// Makes `dir` (and its parents) if need be, readable by this user only.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// What asking one socket found.
#[derive(Debug, Clone, PartialEq)]
pub enum Probe {
    /// A game answered `session.status`.
    Alive(Box<Status>),
    /// Something accepted the connection but did not answer in time or
    /// sensibly (a game busy loading, or not a game).
    Busy(String),
    /// Nothing listens there: the file is a leftover.
    Stale,
}

/// Connects to the Unix socket at `path` and asks `session.status`.
pub fn probe(path: &Path, timeout: Duration) -> Probe {
    let mut c = match Client::connect_to(&Endpoint::Unix(path.to_owned())) {
        Ok(c) => c,
        Err(e) => {
            return match e.kind() {
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound => Probe::Stale,
                _ => Probe::Busy(e.to_string()),
            };
        }
    };
    let _ = c.set_timeout(Some(timeout));
    match c.call(method::STATUS, serde_json::Value::Null) {
        Ok(v) => match serde_json::from_value::<Status>(v) {
            Ok(s) => Probe::Alive(Box::new(s)),
            Err(e) => Probe::Busy(format!("session.status: {e}")),
        },
        Err(e) => Probe::Busy(e.to_string()),
    }
}

/// A game found in a directory of sockets.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    /// From the file name, when it is `<pid>.sock`.
    pub pid: Option<u32>,
    /// Its last `session.status`; `None` when it did not answer.
    pub status: Option<Status>,
    /// Why it did not answer.
    pub note: Option<String>,
}

impl Found {
    /// The game's map, when it says.
    pub fn map(&self) -> Option<&str> {
        self.status.as_ref()?.at.map.as_deref()
    }

    /// The endpoint to pair with.
    pub fn endpoint(&self) -> Endpoint {
        Endpoint::Unix(self.path.clone())
    }
}

/// Every `*.sock` socket file directly in `dir`, by name.
fn socket_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sock") && is_socket(p))
        .collect();
    v.sort();
    v
}

/// Whether `p` is a Unix socket file (not a link to one, not a plain file).
pub fn is_socket(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_socket())
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        false
    }
}

/// The games listening in `dir`. A socket nothing answers on is removed
/// (a game that was killed leaves its file); one that accepts but does not
/// answer within `timeout` is listed with a note and kept.
pub fn scan(dir: &Path, timeout: Duration) -> Vec<Found> {
    scan_paths(&socket_files(dir), timeout, true)
}

/// Asks each of `paths` (Unix sockets) for its status. With `clean`, a
/// socket file nothing answers on is removed; without, it is skipped.
pub fn scan_paths(paths: &[PathBuf], timeout: Duration, clean: bool) -> Vec<Found> {
    let mut out = Vec::new();
    for path in paths {
        let pid = pid_of(path);
        let path = path.clone();
        match probe(&path, timeout) {
            Probe::Alive(s) => out.push(Found {
                path,
                pid,
                status: Some(*s),
                note: None,
            }),
            Probe::Busy(why) => out.push(Found {
                path,
                pid,
                status: None,
                note: Some(why),
            }),
            Probe::Stale => {
                if clean {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    out
}

/// Whether the editor's open map and a game's map are the same level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapMatch {
    /// Both known and equal.
    Same,
    /// Both known and different.
    Different,
    /// One of them is not known (no map open, or the game is loading or
    /// at its menu).
    Unknown,
}

impl MapMatch {
    /// Map names compare without case (the game reports `ascension`, a
    /// package may say `Ascension`), and a path or extension is dropped.
    pub fn of(open: Option<&str>, game: Option<&str>) -> MapMatch {
        let norm = |s: &str| {
            let s = s.rsplit(['/', '\\']).next().unwrap_or(s);
            s.strip_suffix(".map").unwrap_or(s).to_ascii_lowercase()
        };
        match (open.filter(|s| !s.is_empty()), game.filter(|s| !s.is_empty())) {
            (Some(a), Some(b)) if norm(a) == norm(b) => MapMatch::Same,
            (Some(_), Some(_)) => MapMatch::Different,
            _ => MapMatch::Unknown,
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
