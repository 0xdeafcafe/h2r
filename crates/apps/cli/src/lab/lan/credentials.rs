//! The host's pairing state and the driver's pinned peers.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use super::{Key, PairSecret, random};
use crate::lab::proto::to_hex;

const PAIRING_SECONDS: u64 = 10 * 60;

#[derive(Default, Deserialize, Serialize)]
struct Credentials {
    offer: Option<Offer>,
    key: Option<Key>,
}

#[derive(Deserialize, Serialize)]
struct Offer {
    secret: PairSecret,
    issued_at: u64,
    expires_at: u64,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Peer {
    pub(super) pin: Key,
    pub(super) key: Key,
}

pub(super) struct Store {
    dir: PathBuf,
}

impl Store {
    pub(super) fn new(root: &Path) -> Result<Self> {
        let dir = root.join("auth");
        private_dir(&dir)?;
        Ok(Self { dir })
    }

    pub(super) fn identity(&self) -> Result<Vec<u8>> {
        let _lock = self.lock()?;
        let path = self.dir.join("identity.pk8");
        if let Some(bytes) = read_private(&path)? {
            return Ok(bytes);
        }

        let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| anyhow::anyhow!("generating the lab TLS key"))?;
        write_private(&path, key.as_ref())?;
        Ok(key.as_ref().to_vec())
    }

    pub(super) fn offer(&self, now: u64) -> Result<PairSecret> {
        let _lock = self.lock()?;
        let mut credentials = self.credentials()?;
        let secret = random()?;
        credentials.offer = Some(Offer {
            secret,
            issued_at: now,
            expires_at: now + PAIRING_SECONDS,
        });
        self.save(&credentials)?;
        Ok(secret)
    }

    pub(super) fn pair(&self, secret: &PairSecret, key: Key, now: u64) -> Result<bool> {
        let _lock = self.lock()?;
        let mut credentials = self.credentials()?;
        let accepted = credentials.offer.as_ref().is_some_and(|offer| {
            bool::from(offer.secret.ct_eq(secret))
                && (offer.issued_at..offer.expires_at).contains(&now)
        });
        if accepted {
            credentials.offer = None;
            credentials.key = Some(key);
            self.save(&credentials)?;
        }
        Ok(accepted)
    }

    pub(super) fn key(&self) -> Result<Option<Key>> {
        Ok(self.credentials()?.key)
    }

    pub(super) fn revoke(&self) -> Result<()> {
        let _lock = self.lock()?;
        self.save(&Credentials::default())
    }

    fn credentials(&self) -> Result<Credentials> {
        read_json(&self.dir.join("pairing.json")).map(Option::unwrap_or_default)
    }

    fn save(&self, credentials: &Credentials) -> Result<()> {
        write_json(&self.dir.join("pairing.json"), credentials)
    }

    fn lock(&self) -> Result<File> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .write(true)
            .open(self.dir.join("lock"))?;
        lock.lock()?;
        Ok(lock)
    }
}

pub(super) fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

pub(super) fn read_peer(host: &str) -> Result<Peer> {
    read_json(&peer_path(host)?)?
        .with_context(|| format!("lab host {host:?} is unpaired (run `h2 lab pair {host} <code>`)"))
}

pub(super) fn stage_peer(host: &str, peer: &Peer) -> Result<PendingWrite> {
    let path = peer_path(host)?;
    private_dir(path.parent().unwrap())?;
    // Finish writing the driver key before asking the host to spend the code.
    PendingWrite::new(&path, &serde_json::to_vec(peer)?)
}

fn peer_path(host: &str) -> Result<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let config = if cfg!(target_os = "macos") {
        home().map(|home| home.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|home| home.join(".config")))
    }
    .context("finding the lab driver configuration directory")?;
    // Bonjour instance names are case-insensitive. Hashing also lets an IP
    // address with a port use the same layout without escaping file names.
    let name = to_hex(&super::fingerprint(host.to_ascii_lowercase().as_bytes()));
    Ok(config
        .join("h2recomp/lab/peers")
        .join(format!("{name}.json")))
}

fn private_dir(path: &Path) -> Result<()> {
    DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(path)
        .with_context(|| format!("creating {}", path.display()))
}

fn read_private(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut input = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(input) => input,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    let meta = input.metadata()?;
    ensure!(
        meta.is_file() && meta.permissions().mode() & 0o777 == 0o600,
        "{} must be a regular file with mode 0600",
        path.display()
    );
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    PendingWrite::new(path, bytes)?.publish()
}

pub(super) struct PendingWrite {
    dest: PathBuf,
    temp: PathBuf,
}

impl PendingWrite {
    fn new(dest: &Path, bytes: &[u8]) -> Result<Self> {
        let suffix = to_hex(&random::<16>()?);
        let temp = dest.with_extension(format!("tmp-{suffix}"));
        let mut output = OpenOptions::new()
            .create_new(true)
            .mode(0o600)
            .write(true)
            .open(&temp)
            .with_context(|| format!("creating {}", temp.display()))?;
        let pending = Self {
            dest: dest.into(),
            temp,
        };
        output.set_permissions(fs::Permissions::from_mode(0o600))?;
        output.write_all(bytes)?;
        output.sync_all()?;
        Ok(pending)
    }

    pub(super) fn publish(self) -> Result<()> {
        fs::rename(&self.temp, &self.dest)
            .with_context(|| format!("writing {}", self.dest.display()))
    }
}

impl Drop for PendingWrite {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temp);
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    read_private(path)?
        .map(|bytes| {
            serde_json::from_slice(&bytes).with_context(|| format!("reading {}", path.display()))
        })
        .transpose()
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    write_private(path, &serde_json::to_vec(value)?)
}
