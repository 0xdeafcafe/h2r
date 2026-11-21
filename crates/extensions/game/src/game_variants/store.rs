//! The local store of game variants and their scripts
//! (docs/game/rules-engine.md, "Live game variants in the game"):
//!
//! | Path under the store root | What |
//! |---|---|
//! | `*.h2variant` | bundles the player put there (imported files) |
//! | `live/<id>.h2variant` | the signed-in player's Live variants, as fetched |
//! | `ported/<key>.h2variant` | saved variants ported at boot ([`super::port`]) |
//! | `scripts/<hash>.ir` | rules IR blobs fetched from Live, by full hash |
//!
//! The root is `<data>/game-variants` ([`default_root`]). Scripts are kept by
//! full SHA-256 with an index by the 8-byte prefix a record carries; a
//! bundle naming a script by hash only still puts that hash in the index,
//! so a host can fetch the blob by its full hash.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use content::Sha256;

use super::bundle::{Bundle, EXTENSION};

/// The store's directory name under the data root.
pub const DIR: &str = "game-variants";
pub const LIVE_DIR: &str = "live";
pub const PORTED_DIR: &str = "ported";
pub const SCRIPTS_DIR: &str = "scripts";

/// `<data>/game-variants`.
pub fn default_root() -> PathBuf {
    content::store::default_data_root().join(DIR)
}

/// Where a bundle came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Imported,
    Live,
    Ported,
}

/// One variant in the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub origin: Origin,
    pub bundle: Bundle,
    pub record: [u8; variants::record::RECORD_SIZE],
    /// The script's full hash, when the record names one.
    pub hash: Option<Sha256>,
}

/// The store: its entries (one per bundle id) and scripts.
#[derive(Default)]
pub struct Store {
    root: Option<PathBuf>,
    entries: Vec<Entry>,
    scripts: HashMap<Sha256, Vec<u8>>,
    prefixes: HashMap<[u8; 8], Sha256>,
    /// The ported sources' keys seen in this run.
    ported: std::collections::HashSet<String>,
}

fn prefix(h: &Sha256) -> [u8; 8] {
    let mut p = [0; 8];
    p.copy_from_slice(&h.0[..8]);
    p
}

/// A file name of `s`'s safe characters.
fn safe(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect()
}

fn bundles_in(dir: &Path) -> Vec<(PathBuf, std::io::Result<String>)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == EXTENSION))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p);
            (p, text)
        })
        .collect()
}

impl Store {
    /// A store with no directory (tests, and a machine with no data root).
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Reads the store under `root`. Returns it and a line for each file
    /// left out, by name.
    pub fn open(root: &Path) -> (Self, Vec<String>) {
        let mut s = Store {
            root: Some(root.to_path_buf()),
            ..Default::default()
        };
        let mut lines = Vec::new();
        if let Ok(rd) = std::fs::read_dir(root.join(SCRIPTS_DIR)) {
            for e in rd.filter_map(Result::ok) {
                let p = e.path();
                if p.extension().is_none_or(|x| x != "ir") {
                    continue;
                }
                match std::fs::read(&p) {
                    Ok(blob) => {
                        if let Err(why) = s.add_script_named(&p, blob) {
                            lines.push(format!("game variants: {} left out: {why}", p.display()));
                        }
                    }
                    Err(e) => lines.push(format!("game variants: {}: {e}", p.display())),
                }
            }
        }
        for (dir, origin) in [
            (root.to_path_buf(), Origin::Imported),
            (root.join(LIVE_DIR), Origin::Live),
            (root.join(PORTED_DIR), Origin::Ported),
        ] {
            for (p, text) in bundles_in(&dir) {
                let added = text
                    .map_err(|e| e.to_string())
                    .and_then(|t| Bundle::parse(&t))
                    .and_then(|b| s.add(origin, b));
                if let Err(why) = added {
                    lines.push(format!("game variants: {} left out: {why}", p.display()));
                }
            }
        }
        (s, lines)
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Adds a checked bundle. One of the same id already kept stays (a
    /// Live variant and a file of the same record are one), taking the
    /// newcomer's IR if it had none. Returns whether it was new.
    pub fn add(&mut self, origin: Origin, bundle: Bundle) -> Result<bool, String> {
        let checked = bundle.check()?;
        if let Some(ir) = checked.ir.clone() {
            self.add_script(ir)?;
        }
        if let Some(h) = checked.hash {
            self.prefixes.entry(prefix(&h)).or_insert(h);
        }
        let id = bundle.id();
        if self.entries.iter().any(|e| e.bundle.id() == id) {
            return Ok(false);
        }
        self.entries.push(Entry {
            origin,
            bundle,
            record: checked.record,
            hash: checked.hash,
        });
        Ok(true)
    }

    fn add_script_named(&mut self, path: &Path, blob: Vec<u8>) -> Result<(), String> {
        let want = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let got = Sha256::of(&blob);
        if got.to_string() != want {
            return Err(format!("the blob hashes to {got}, not its name"));
        }
        self.add_script(blob).map(|_| ())
    }

    /// Keeps a blob under its SHA-256 (in memory). Returns its hash.
    pub fn add_script(&mut self, blob: Vec<u8>) -> Result<Sha256, String> {
        if blob.len() > super::bundle::MAX_IR {
            return Err(format!(
                "the blob is {} bytes, over {}",
                blob.len(),
                super::bundle::MAX_IR
            ));
        }
        let h = Sha256::of(&blob);
        self.prefixes.insert(prefix(&h), h);
        self.scripts.insert(h, blob);
        Ok(h)
    }

    /// Keeps a fetched blob and writes it to `scripts/<hash>.ir`.
    pub fn save_script(&mut self, blob: Vec<u8>) -> Result<Sha256, String> {
        let bytes = blob.clone();
        let h = self.add_script(blob)?;
        if let Some(root) = &self.root {
            let dir = root.join(SCRIPTS_DIR);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(format!("{h}.ir")), bytes).map_err(|e| e.to_string())?;
        }
        Ok(h)
    }

    /// The full hash the store knows for `p`, with or without its blob.
    pub fn full_hash(&self, p: [u8; 8]) -> Option<Sha256> {
        self.prefixes.get(&p).copied()
    }

    /// The blob whose SHA-256 starts with `p`, when the store has it.
    pub fn script(&self, p: [u8; 8]) -> Option<&[u8]> {
        let h = self.prefixes.get(&p)?;
        self.scripts.get(h).map(Vec::as_slice)
    }

    /// Writes `bundle` to `<dir>/<name>.h2variant` and adds it.
    fn put(
        &mut self,
        origin: Origin,
        dir: &str,
        name: &str,
        bundle: Bundle,
    ) -> Result<bool, String> {
        bundle.check()?;
        if let Some(root) = &self.root {
            let dir = root.join(dir);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{}.{EXTENSION}", safe(name)));
            let text = bundle.to_json();
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                std::fs::write(&path, text).map_err(|e| e.to_string())?;
            }
        }
        self.add(origin, bundle)
    }

    /// A Live variant, saved as its bundle (with its IR when the store has
    /// it) under `live/<id>.h2variant`.
    pub fn put_live(&mut self, v: &live_hle::game_variants::LiveVariant) -> Result<bool, String> {
        let mut b = Bundle::from_live(v);
        let ir = b
            .script
            .as_ref()
            .and_then(|s| s.hash.parse::<Sha256>().ok())
            .and_then(|h| self.scripts.get(&h).cloned());
        if let Some(ir) = ir {
            b = b.with_ir(&ir);
        }
        let name = if v.id.is_empty() {
            b.id()
        } else {
            v.id.clone()
        };
        self.put(Origin::Live, LIVE_DIR, &name, b)
    }

    /// Whether `ported/<key>.h2variant` exists (in memory: whether a
    /// ported entry of that key was added).
    pub fn has_ported(&self, key: &str) -> bool {
        self.ported.contains(key)
            || self.root.as_ref().is_some_and(|root| {
                root.join(PORTED_DIR)
                    .join(format!("{}.{EXTENSION}", safe(key)))
                    .is_file()
            })
    }

    /// A ported save, saved under `ported/<key>.h2variant`.
    pub fn put_ported(&mut self, key: &str, bundle: Bundle) -> Result<bool, String> {
        let added = self.put(Origin::Ported, PORTED_DIR, key, bundle)?;
        self.ported.insert(key.to_owned());
        Ok(added)
    }
}
