//! The local store of game variants and their scripts
//! (docs/game/rules-engine.md, "Live game variants in the game"):
//!
//! | Path under the store root | What |
//! |---|---|
//! | `*.h2variant` | bundles the player put there (imported files) |
//! | `live/<id>.h2variant` | the signed-in player's Live variants, with their scripts |
//! | `live/<id>.pending.json` | a Live variant whose script's IR is not here yet: its metadata and full hash |
//! | `ported/<key>.h2variant` | saved variants ported at boot ([`super::port`]) |
//! | `scripts/<hash>.ir` | rules IR blobs fetched from Live, by full hash |
//!
//! The root is `<data>/game-variants` ([`default_root`]). Every
//! `.h2variant` is a whole format 2 file (`variants::portable`). Scripts are
//! kept by full SHA-256 with an index by the 8-byte prefix a record
//! carries; a pending Live variant puts its script's full hash in the index
//! too, so a host can fetch the blob by it. Once the IR is here the pending
//! variant is written as its bundle and its pending file removed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use content::Sha256;

use live_hle::game_variants::LiveVariant;

use super::bundle::{self, Bundle, EXTENSION};

/// A pending Live variant's file suffix.
pub const PENDING: &str = ".pending.json";

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
    /// The file it was read from or written to, when the store has a
    /// directory: where [`Store::set_maps`] writes it again.
    pub path: Option<PathBuf>,
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
    /// Live variants whose script's IR is not here yet.
    pending: Vec<LiveVariant>,
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
                    .and_then(|t| variants::portable::read(&t).map_err(|e| e.to_string()))
                    .and_then(|b| s.add_at(origin, b, Some(p.clone())));
                if let Err(why) = added {
                    lines.push(format!("game variants: {} left out: {why}", p.display()));
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(root.join(LIVE_DIR)) {
            let mut paths: Vec<PathBuf> = rd
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(PENDING))
                .collect();
            paths.sort();
            for p in paths {
                let read = std::fs::read_to_string(&p)
                    .map_err(|e| e.to_string())
                    .and_then(|t| {
                        serde_json::from_str::<LiveVariant>(&t).map_err(|e| e.to_string())
                    })
                    .and_then(|v| s.add_pending(v));
                if let Err(why) = read {
                    lines.push(format!("game variants: {} left out: {why}", p.display()));
                }
            }
        }
        // A pending variant whose IR has arrived since becomes its bundle.
        for v in s.pending.clone() {
            let here = bundle::live_hash(&v).is_some_and(|h| s.scripts.contains_key(&h));
            if here && let Err(why) = s.put_live(&v) {
                lines.push(format!("game variants: Live variant {} left out: {why}", v.name));
            }
        }
        (s, lines)
    }

    /// Live variants whose script's IR is not here yet.
    pub fn pending(&self) -> &[LiveVariant] {
        &self.pending
    }

    /// Keeps a Live variant whose script is named by full hash only, and
    /// indexes the hash by its prefix.
    pub fn add_pending(&mut self, v: LiveVariant) -> Result<(), String> {
        let h = bundle::live_hash(&v).ok_or("it names no script by a 64-digit hash")?;
        self.prefixes.entry(prefix(&h)).or_insert(h);
        self.pending.retain(|p| p.id != v.id);
        self.pending.push(v);
        Ok(())
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Adds a bundle that holds to the format (`Bundle::integrity`): its
    /// script's IR joins the scripts. One of the same bundle id already
    /// kept stays. Returns whether it was new.
    pub fn add(&mut self, origin: Origin, bundle: Bundle) -> Result<bool, String> {
        self.add_at(origin, bundle, None)
    }

    fn add_at(&mut self, origin: Origin, bundle: Bundle, path: Option<PathBuf>) -> Result<bool, String> {
        let checked = bundle.integrity().map_err(|e| e.to_string())?;
        let hash = match checked.script {
            Some((ir, _)) => Some(self.add_script(ir)?),
            None => None,
        };
        let id = bundle.id();
        if self.entries.iter().any(|e| e.bundle.id() == id) {
            return Ok(false);
        }
        self.entries.push(Entry {
            origin,
            bundle,
            record: *checked.variant.bytes(),
            hash,
            path,
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
        bundle.integrity().map_err(|e| e.to_string())?;
        let mut at = None;
        if let Some(root) = &self.root {
            let dir = root.join(dir);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{}.{EXTENSION}", safe(name)));
            let text = bundle.canonical();
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                std::fs::write(&path, text).map_err(|e| e.to_string())?;
            }
            at = Some(path);
        }
        self.add_at(origin, bundle, at)
    }

    /// Sets the map variants the kept game variant `id` (its bundle id)
    /// suggests, `maps` in the order given and at most
    /// [`variants::portable::MAX_MAPS`] (`Bundle::with_maps`), none to clear
    /// them. The record is untouched; the bundle, so its id, changes, and
    /// the file it came from is written again with the new bytes. Returns
    /// the new id. A Live variant is refused: Live sends its own copy again
    /// at every refresh and does not carry suggestions; so is an id the
    /// store does not hold, and an id of suggestions another kept bundle
    /// already has.
    pub fn set_maps(
        &mut self,
        id: &str,
        maps: Vec<variants::portable::MapSuggestion>,
    ) -> Result<String, String> {
        let at = self
            .entries
            .iter()
            .position(|e| e.bundle.id() == id)
            .ok_or_else(|| format!("no game variant {id} is kept here"))?;
        let entry = &self.entries[at];
        if entry.origin == Origin::Live {
            return Err(format!(
                "{} is a Live variant: Live keeps its copy, which carries no map variants",
                entry.bundle.name
            ));
        }
        let bundle = entry.bundle.clone().with_maps(maps).map_err(|e| e.to_string())?;
        let new = bundle.id();
        if new == id {
            return Ok(new);
        }
        if self.entries.iter().any(|e| e.bundle.id() == new) {
            return Err(format!("{} with those map variants is already kept", bundle.name));
        }
        if let Some(path) = &entry.path {
            std::fs::write(path, bundle.canonical())
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        self.entries[at].bundle = bundle;
        Ok(new)
    }

    /// A Live variant: saved as its bundle under `live/<id>.h2variant`
    /// when it names no script or its script's IR is here, else kept
    /// pending (`live/<id>.pending.json`) until the IR arrives. Returns
    /// whether a new bundle was added.
    pub fn put_live(&mut self, v: &LiveVariant) -> Result<bool, String> {
        let name = if v.id.is_empty() {
            bundle::hex(&Sha256::of(v.record_hex.as_bytes()).0[..8])
        } else {
            safe(&v.id)
        };
        let ir = match &v.script_hash {
            None => None,
            Some(h) => {
                let hash: Sha256 = h
                    .parse()
                    .map_err(|_| format!("its script hash {h:?} is not 64 hex digits"))?;
                match self.scripts.get(&hash).cloned() {
                    Some(ir) => Some(ir),
                    None => {
                        self.add_pending(v.clone())?;
                        if let Some(root) = &self.root {
                            let dir = root.join(LIVE_DIR);
                            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                            let text = serde_json::to_string(v).map_err(|e| e.to_string())?;
                            std::fs::write(dir.join(format!("{name}{PENDING}")), text)
                                .map_err(|e| e.to_string())?;
                        }
                        return Ok(false);
                    }
                }
            }
        };
        let b = bundle::from_live(v, ir.as_deref())?;
        let added = self.put(Origin::Live, LIVE_DIR, &name, b)?;
        self.pending.retain(|p| p.id != v.id);
        if let Some(root) = &self.root {
            let _ = std::fs::remove_file(root.join(LIVE_DIR).join(format!("{name}{PENDING}")));
        }
        Ok(added)
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
