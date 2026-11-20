//! Build, verify and cache the owner's private multiplayer DVD subset.
//!
//! See [Game data bundle](../../../../docs/live/dedicated-host.md#game-data-bundle)
//! and contract 7 in the dedicated-host build plan.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod bundle;
mod cli;
mod tar;

pub use bundle::{BundleOut, BundleSpec, build};
pub use cli::bundle_cmd;

use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Seek, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::TarError;

/// The archive file name contract 7 fixes.
pub const ARCHIVE_NAME: &str = "host-data.tar.zst";
/// The manifest file name, both beside the archive and inside it.
pub const MANIFEST_NAME: &str = "host-data.json";

const REQUIRED_FILES: [&str; 4] = [
    "default.xbe",
    "maps/mainmenu.map",
    "maps/shared.map",
    "maps/single_player_shared.map",
];
const CACHE_LOCK: &str = ".host-data.lock";
const LAST_USED: &str = ".last-used";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// The bundle's manifest: the archive's sha256 and its file list.
///
/// Written beside the archive (with [`MANIFEST_NAME`]) for the owner and
/// the upload. The embedded manifest lists the same files and leaves the
/// archive digest empty. The supervisor supplies that digest to [`provision`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// sha256 of `host-data.tar.zst`, lowercase hex; empty in the copy
    /// inside the archive.
    pub sha256: String,
    /// The payload files, sorted by path. The manifest itself is not
    /// listed.
    pub files: Vec<FileEntry>,
}

/// One payload file of the bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path in the archive, `/`-separated, relative (`maps/lockout.map`).
    pub path: String,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// sha256 of the file's bytes, lowercase hex.
    pub sha256: String,
}

/// Fetching, verifying or caching a bundle went wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The filesystem or network stream failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Reading the disc image failed.
    #[error("disc image: {0}")]
    Disc(#[from] xiso::Error),
    /// Building the bundle failed (unreadable disc, unusable output).
    #[error("bundle: {0}")]
    Bundle(String),
    /// The download itself failed.
    #[error("host data download: {0}")]
    Download(String),
    /// The downloaded bytes did not hash to the expected digest.
    #[error("host data hashes to {got}, expected {expected}")]
    Hash {
        /// Digest supplied by the supervisor.
        expected: String,
        /// Digest of the downloaded bytes.
        got: String,
    },
    /// The digest is not 64 hexadecimal characters.
    #[error("host data SHA-256 must be 64 hexadecimal characters")]
    InvalidHash,
    /// The URL's scheme is not one this crate fetches.
    #[error("host data URL must use HTTP(S) or file:// with an absolute path")]
    UnsupportedUrl,
    /// The archive is not a bundle this crate accepts.
    #[error("corrupt bundle: {0}")]
    Corrupt(String),
    /// A path or filesystem entry cannot be used safely.
    #[error("unsafe host-data path: {0}")]
    UnsafePath(String),
}

/// Knobs of [`provision_with`] that a supervisor or test may want to set;
/// [`provision`] takes them from the environment.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Evict least-recently-used cached bundles above this many bytes.
    /// The entry being provisioned is never evicted.
    pub max_cache_bytes: u64,
}

impl Options {
    /// Cache cap from `H2_HOST_CACHE_MAX_BYTES` (bytes), defaulting to
    /// 8 GiB — room for a handful of bundles on a host's disk.
    pub fn from_env() -> Self {
        const DEFAULT: u64 = 8 * 1024 * 1024 * 1024;
        let max_cache_bytes = std::env::var("H2_HOST_CACHE_MAX_BYTES")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT);
        Self { max_cache_bytes }
    }
}

/// Provision the host's game data: download `url` (streaming), verify it
/// hashes to `sha256`, extract atomically, and return the DVD directory
/// (`<cache_dir>/<sha256>`) the boot serves as the disc.
///
/// A complete cached copy can be reused after the signed URL expires.
pub fn provision(url: &str, sha256: &str, cache_dir: &Path) -> anyhow::Result<PathBuf> {
    Ok(provision_with(url, sha256, cache_dir, &Options::from_env())?)
}

/// [`provision`] with explicit [`Options`].
pub fn provision_with(
    url: &str,
    sha256: &str,
    cache_dir: &Path,
    opts: &Options,
) -> Result<PathBuf, Error> {
    let sha = normalize_sha(sha256)?;
    private_dir(cache_dir, true)?;
    if !fs::symlink_metadata(cache_dir)?.is_dir() {
        return Err(Error::UnsafePath(cache_dir.display().to_string()));
    }
    // A stable lock serializes downloads, publication and eviction across
    // processes. The OS releases the lock if a provisioner exits mid-write.
    let _lock = lock_cache(cache_dir)?;
    clean_stale_temps(cache_dir)?;

    let final_dir = cache_dir.join(&sha);
    match fs::symlink_metadata(&final_dir) {
        Ok(metadata) if metadata.is_dir() => {
            if cached_bundle_is_complete(&final_dir).is_err() {
                let stale = cache_dir.join(format!("{sha}.extracting"));
                fs::rename(&final_dir, &stale)?;
                fs::remove_dir_all(stale)?;
            }
        }
        Ok(_) => return Err(Error::UnsafePath(final_dir.display().to_string())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if !final_dir.is_dir() {
        download_and_publish(url, &sha, cache_dir, &final_dir)?;
    }
    set_used(&final_dir, std::time::SystemTime::now())?;
    evict(cache_dir, &sha, opts.max_cache_bytes)?;
    Ok(final_dir)
}

fn lock_cache(cache_dir: &Path) -> Result<File, Error> {
    let lock = open_state_file(&cache_dir.join(CACHE_LOCK))?;
    lock.lock()?;
    Ok(lock)
}

fn download_and_publish(url: &str, sha: &str, cache_dir: &Path, final_dir: &Path) -> Result<(), Error> {
    let partial = cache_dir.join(format!("{sha}.partial"));
    let staging = cache_dir.join(format!("{sha}.extracting"));
    let partial_file = new_private_file(&partial)?;
    let result = (|| {
        let mut out = HashWriter::new(partial_file);
        fetch(url, &mut out)?;
        let (mut file, got) = out.into_parts();
        if got != sha {
            return Err(Error::Hash { expected: sha.to_owned(), got });
        }
        file.rewind()?;
        private_dir(&staging, false)?;
        extract_bundle(file, &staging)?;
        // The digest directory becomes visible only after every file verifies.
        fs::rename(&staging, final_dir)?;
        Ok(())
    })();
    let _ = fs::remove_file(&partial);
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn normalize_sha(sha: &str) -> Result<String, Error> {
    if sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(sha.to_ascii_lowercase())
    } else {
        Err(Error::InvalidHash)
    }
}

/// Stream bytes; never include a signed URL or a transport error that may
/// contain it in the diagnostic. Local URLs use a literal absolute path.
fn fetch(url: &str, out: &mut impl Write) -> Result<(), Error> {
    if let Some(path) = url.strip_prefix("file://") {
        if !Path::new(path).is_absolute() {
            return Err(Error::UnsupportedUrl);
        }
        io::copy(&mut File::open(path)?, out)?;
    } else if url.starts_with("http://") || url.starts_with("https://") {
        let resp = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .timeout_write(Duration::from_secs(30))
            .build()
            .get(url)
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(code, _) => Error::Download(format!("HTTP {code}")),
                ureq::Error::Transport(error) => Error::Download(error.kind().to_string()),
            })?;
        if resp.status() != 200 {
            return Err(Error::Download(format!("HTTP {}", resp.status())));
        }
        io::copy(&mut resp.into_reader(), out)?;
    } else {
        return Err(Error::UnsupportedUrl);
    }
    Ok(())
}

/// Extract the verified archive at `archive` into the fresh directory
/// `dest`, checking every file against the manifest inside it.
fn extract_bundle(archive: File, dest: &Path) -> Result<(), Error> {
    let decoder = zstd::stream::read::Decoder::new(archive)?;
    let mut tar_reader = tar::TarReader::new(decoder);
    let mut extracted = std::collections::BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    while let Some((path, size)) = tar_reader.next().map_err(tar_error)? {
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(Error::Corrupt(format!("duplicate entry {path}")));
        }
        if path == MANIFEST_NAME {
            if size > MAX_MANIFEST_BYTES {
                return Err(Error::Corrupt(format!("{MANIFEST_NAME} is too large")));
            }
        } else {
            validate_payload_path(&path)?;
        }
        let target = dest.join(&path);
        fs::create_dir_all(target.parent().expect("validated relative path has a parent"))?;
        let mut out = HashWriter::new(new_private_file(&target)?);
        let written = io::copy(&mut tar_reader, &mut out)?;
        if written != size {
            return Err(Error::Corrupt(format!("{path} is truncated")));
        }
        let (file, digest) = out.into_parts();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o400))?;
        }
        drop(file);
        if path != MANIFEST_NAME {
            extracted.insert(path, (size, digest));
        }
    }

    let manifest = read_embedded_manifest(dest)?;
    for file in manifest.files {
        match extracted.remove(&file.path) {
            Some((size, digest)) if size == file.size && digest == file.sha256 => {}
            _ => return Err(Error::Corrupt(format!("{} differs from {MANIFEST_NAME}", file.path))),
        }
    }
    if let Some((path, _)) = extracted.pop_first() {
        return Err(Error::Corrupt(format!("{path} is missing from {MANIFEST_NAME}")));
    }
    Ok(())
}

fn validate_payload_path(path: &str) -> Result<(), Error> {
    tar::validate_path(path).map_err(Error::UnsafePath)?;
    let allowed = path == "default.xbe"
        || path.strip_prefix("maps/").is_some_and(|name| {
            name.strip_prefix("fonts/").map_or_else(
                || name.to_ascii_lowercase().ends_with(".map") && !name.contains('/'),
                |font| !font.contains('/'),
            )
        });
    if !allowed {
        return Err(Error::Corrupt(format!("unexpected payload path {path}")));
    }
    Ok(())
}

fn read_embedded_manifest(dir: &Path) -> Result<Manifest, Error> {
    let path = dir.join(MANIFEST_NAME);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(Error::Corrupt(format!("invalid {MANIFEST_NAME} file")));
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| Error::Corrupt(format!("reading {MANIFEST_NAME}: {error}")))?;
    if !manifest.sha256.is_empty() {
        return Err(Error::Corrupt("embedded archive digest must be empty".into()));
    }
    let mut paths = std::collections::BTreeSet::new();
    for file in &manifest.files {
        validate_payload_path(&file.path)?;
        if !paths.insert(file.path.to_ascii_lowercase()) {
            return Err(Error::Corrupt(format!("duplicate manifest path {}", file.path)));
        }
    }
    for required in REQUIRED_FILES {
        if !paths.contains(required) {
            return Err(Error::Corrupt(format!("{MANIFEST_NAME} is missing {required}")));
        }
    }
    Ok(manifest)
}

/// A completed directory is immutable to the host. Check its layout and
/// sizes on a hit without rereading and hashing gigabytes of cache data.
fn cached_bundle_is_complete(dir: &Path) -> Result<(), Error> {
    let manifest = read_embedded_manifest(dir)?;
    for file in manifest.files {
        let mut path = dir.to_path_buf();
        let mut components = file.path.split('/').peekable();
        while let Some(component) = components.next() {
            path.push(component);
            let metadata = fs::symlink_metadata(&path)?;
            let valid = if components.peek().is_some() {
                metadata.is_dir()
            } else {
                metadata.is_file() && metadata.len() == file.size
            };
            if !valid {
                return Err(Error::Corrupt(format!("invalid cached file {}", file.path)));
            }
        }
    }
    Ok(())
}

fn tar_error(e: TarError) -> Error {
    match e {
        TarError::Io(e) => e.into(),
        TarError::Corrupt(why) => Error::Corrupt(why.to_string()),
        TarError::UnsafePath(p) => Error::UnsafePath(p),
    }
}

fn private_dir(path: &Path, recursive: bool) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn new_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Clean only provisioning artifacts, with the cache lock held.
fn clean_stale_temps(cache_dir: &Path) -> Result<(), Error> {
    for entry in fs::read_dir(cache_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some((sha, suffix)) = name.to_str().and_then(|name| name.split_once('.')) else {
            continue;
        };
        if !is_cache_entry_name(sha) {
            continue;
        }
        if suffix == "partial" || suffix == "extracting" || suffix.starts_with("extracting-") {
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// Use a separate file so the mounted DVD files remain immutable.
fn set_used(dir: &Path, when: std::time::SystemTime) -> Result<(), Error> {
    open_state_file(&dir.join(LAST_USED))?
        .set_times(fs::FileTimes::new().set_modified(when))?;
    Ok(())
}

fn open_state_file(path: &Path) -> Result<File, Error> {
    match new_private_file(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(path)?.is_file() {
                return Err(Error::UnsafePath(path.display().to_string()));
            }
            Ok(OpenOptions::new().read(true).write(true).open(path)?)
        }
        Err(error) => Err(error.into()),
    }
}

fn is_cache_entry_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn dir_size(dir: &Path) -> io::Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        let size = if metadata.is_dir() {
            dir_size(&path)?
        } else if metadata.is_file() {
            metadata.len()
        } else {
            0
        };
        total = total.saturating_add(size);
    }
    Ok(total)
}

/// Drop least-recently-used entries until the cache fits `max_bytes`.
/// `keep` (the entry just provisioned or returned) is never dropped.
fn evict(cache_dir: &Path, keep: &str, max_bytes: u64) -> Result<(), Error> {
    let mut entries: Vec<(std::time::SystemTime, String, u64)> = Vec::new();
    for e in fs::read_dir(cache_dir)? {
        let e = e?;
        let Some(name) = e.file_name().into_string().ok() else { continue };
        if !is_cache_entry_name(&name) {
            continue;
        }
        let Ok(md) = fs::symlink_metadata(e.path()) else { continue };
        if !md.is_dir() {
            continue;
        }
        let used = fs::symlink_metadata(e.path().join(LAST_USED))
            .ok()
            .filter(|metadata| metadata.is_file())
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        entries.push((used, name, dir_size(&e.path())?));
    }
    let mut total = entries.iter().fold(0u64, |total, entry| total.saturating_add(entry.2));
    if total <= max_bytes {
        return Ok(());
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (_, name, size) in entries {
        if total <= max_bytes {
            break;
        }
        if name == keep {
            continue;
        }
        let stale = cache_dir.join(format!("{name}.extracting"));
        fs::rename(cache_dir.join(&name), &stale)?;
        fs::remove_dir_all(stale)?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

/// A writer that hashes everything it writes.
pub(crate) struct HashWriter<W: Write> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self { inner, hasher: Sha256::new() }
    }

    pub(crate) fn into_parts(self) -> (W, String) {
        let Self { inner, hasher } = self;
        (inner, tar::hex(&hasher.finalize()))
    }
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// A process-unique suffix for temporary names.
pub(crate) fn unique_suffix() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!("{}-{n}", std::process::id())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
use tests::testkit;
