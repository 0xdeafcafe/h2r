//! The server's identity: its id (the `aud` of every token, the host in
//! every author id) and its Ed25519 key, kept in a data directory and never
//! in Postgres (docs/live/live-security.md §1).
//!
//! ```text
//! <data>/server.key    base64url Ed25519 seed, mode 0600
//! <data>/server.json   {"id": "..."}
//! ```

use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use live_proto::sign::KeyPair;

/// Used when the first start names no `--id`.
pub const DEFAULT_ID: &str = "localhost";

pub struct ServerIdentity {
    pub id: String,
    pub key: KeyPair,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Meta {
    id: String,
}

impl ServerIdentity {
    /// Loads the identity in `dir`, creating it on first start. `id` sets
    /// the id on first start and must match it afterwards: every account's
    /// author id contains it.
    pub fn open(dir: &Path, id: Option<&str>) -> Result<Self> {
        if !dir.join("server.key").exists() {
            create_private_dir(dir)?;
            let id = id.unwrap_or(DEFAULT_ID);
            valid_id(id)?;
            write_private(
                &dir.join("server.key"),
                format!("{}\n", KeyPair::generate().secret_b64()).as_bytes(),
            )?;
            write_private(
                &dir.join("server.json"),
                serde_json::to_string(&Meta { id: id.into() })?.as_bytes(),
            )?;
        }
        let me = Self::load(dir)?;
        if let Some(want) = id.filter(|want| *want != me.id) {
            bail!(
                "{} belongs to server {:?}, not {want:?}; ids cannot change",
                dir.display(),
                me.id
            );
        }
        Ok(me)
    }

    /// Loads an existing identity (the CLI signing as the operator).
    pub fn load(dir: &Path) -> Result<Self> {
        let key_path = dir.join("server.key");
        let secret = read_private(&key_path)?;
        let key = KeyPair::from_secret_b64(std::str::from_utf8(&secret).unwrap_or(""))
            .map_err(|_| anyhow::anyhow!("{} is not a server key", key_path.display()))?;
        let meta_path = dir.join("server.json");
        let meta: Meta = serde_json::from_slice(
            &fs::read(&meta_path).with_context(|| format!("reading {}", meta_path.display()))?,
        )
        .with_context(|| format!("parsing {}", meta_path.display()))?;
        Ok(ServerIdentity { id: meta.id, key })
    }

    pub fn author(&self) -> String {
        live_proto::Author::Server(self.id.clone()).to_string()
    }

    /// A 32-byte key for `label`, derived from the server key with
    /// HKDF-SHA256: the sealing key for recovery secrets, the invite-code
    /// MAC key. It lives and dies with `server.key`.
    pub fn derive(&self, label: &str) -> [u8; 32] {
        self.key.derive_key(label)
    }
}

/// The `--id` to start a server in `dir` with: as given; else, on a first
/// start (no key in `dir` yet), `lasting` (the host of a lasting public
/// address, docs/live/live-hosting.md), since an id cannot change later; else
/// none, which makes a new server [`DEFAULT_ID`]. An existing server keeps
/// its id.
pub fn first_start_id(id: Option<String>, lasting: Option<String>, dir: &Path) -> Option<String> {
    if id.is_some() || dir.join("server.key").exists() {
        return id;
    }
    lasting
}

/// A server id is a host name, optionally with a port.
fn valid_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 253
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'));
    if !ok {
        bail!("server id {id:?} must be a host name (letters, digits, '.', '-', ':port')");
    }
    Ok(())
}

fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting {}", dir.display()))?;
    Ok(())
}

/// Reads a secret file, refusing one that group or others can read. On
/// Windows there are no mode bits to check: the file is as private as the
/// ACL it inherits from its directory (a per-user profile directory is).
pub fn read_private(path: &Path) -> Result<Vec<u8>> {
    #[cfg_attr(not(unix), allow(unused_variables))]
    let meta = fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    #[cfg(unix)]
    if meta.permissions().mode() & 0o077 != 0 {
        bail!(
            "{} is readable by other users; chmod 600 it",
            path.display()
        );
    }
    fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// Writes `bytes` to `path` with mode 0600, atomically (a temp file in the
/// same directory, then a rename), creating the directory if needed.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp: PathBuf = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut f = options.open(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "h2live-{name}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn created_once_then_loaded() {
        let dir = temp_dir("identity");
        let a = ServerIdentity::open(&dir, Some("live.example")).unwrap();
        assert_eq!(a.id, "live.example");
        assert_eq!(a.author(), "server:live.example");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join("server.key")), 0o600);
        assert_eq!(mode(&dir), 0o700);

        let b = ServerIdentity::open(&dir, None).unwrap();
        assert_eq!(b.key.kid(), a.key.kid());
        assert!(ServerIdentity::open(&dir, Some("other")).is_err());

        // A key file others can read is refused.
        fs::set_permissions(dir.join("server.key"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(ServerIdentity::load(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lasting_host_names_a_new_server_only() {
        let dir = temp_dir("first-id");
        let host = || Some("live.example.net".to_string());
        assert_eq!(
            first_start_id(None, host(), &dir).as_deref(),
            Some("live.example.net")
        );
        assert_eq!(first_start_id(None, None, &dir), None);
        assert_eq!(
            first_start_id(Some("mine".into()), host(), &dir).as_deref(),
            Some("mine")
        );
        ServerIdentity::open(&dir, None).unwrap();
        assert_eq!(
            first_start_id(None, host(), &dir),
            None,
            "an existing server keeps its id"
        );
        assert_eq!(
            first_start_id(Some("x".into()), host(), &dir).as_deref(),
            Some("x")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ids_are_host_names() {
        let dir = temp_dir("badid");
        assert!(ServerIdentity::open(&dir, Some("a b")).is_err());
        assert!(ServerIdentity::open(&dir, Some("")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
