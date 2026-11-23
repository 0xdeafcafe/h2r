//! Serves each multiplayer map with the kit in it: the VFS reads a disc
//! map once, hands it to [`PogoRewrite`], and serves what comes back.
//!
//! Which maps are multiplayer is read from each map's header, so a map
//! the disc does not have (a downloaded one, one of ours) gets the kit
//! too. The main menu and the shared maps are never asked for.
//!
//! The rebuilt map is kept on the host, one file per map under
//! `<data>/cache/pogo/`: `<map>.map` is the map, `<map>.json` says what it
//! was made from and which weapon tags it holds. The key is the SHA-256 of
//! the map's name, [`KIT_VERSION`], the built-in sources and atlases
//! ([`fingerprint`]) and the input map's bytes, so a boot after the first
//! reads the file back, and a changed source, atlas, kit or map rebuilds.
//!
//! Each map served with the kit is recorded in [`placed`]; one served
//! without it (no room, no shader template, too large to rewrite) is
//! cleared there and named once on stderr.

use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::Result;
use content::{Hasher, Sha256};
use map::CacheType;
use serde::{Deserialize, Serialize};

use crate::inject::{Added, Costs, Given, add_pogo_with};
use crate::placed::{self, Placed};
use crate::{KIT_VERSION, Style, source};

/// The folder under `<data>/cache` the rebuilt maps are kept in.
pub const CACHE_DIR: &str = "pogo";

/// The maps never asked for: they are not levels.
const NEVER: [&str; 3] = ["mainmenu", "shared", "single_player_shared"];

/// A rebuilt map is read back only up to this size.
const MAX_MAP: u64 = 256 << 20;

fn field(h: &mut Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

/// The digest of what the kit is built from: its version and each look's
/// source and atlas.
pub fn fingerprint() -> Sha256 {
    let mut h = Hasher::default();
    field(&mut h, format!("pogo_kit_v{KIT_VERSION}").as_bytes());
    for style in Style::ALL {
        field(&mut h, style.id().as_bytes());
        field(&mut h, source::text(style).as_bytes());
        field(&mut h, source::atlas(style));
    }
    h.finish()
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    key: Sha256,
    /// The cached map's digest.
    map: Sha256,
    /// The three looks' `weap` datums.
    weapons: [u32; 3],
    costs: Costs,
}

/// What [`PogoRewrite::rebuild`] gives back.
#[derive(Debug)]
pub struct Rebuilt {
    pub added: Added,
    pub from_cache: bool,
    /// The cache file the map is in, when there is a cache.
    pub file: Option<PathBuf>,
}

/// Serves every multiplayer `maps\<name>.map` with the kit added.
pub struct PogoRewrite {
    fingerprint: Sha256,
    cache: Option<PathBuf>,
    given: Given,
    /// The maps already named on stderr.
    named: HashSet<String>,
}

impl Default for PogoRewrite {
    fn default() -> Self {
        Self::new()
    }
}

impl PogoRewrite {
    pub fn new() -> Self {
        Self {
            fingerprint: fingerprint(),
            cache: None,
            given: Given::default(),
            named: HashSet::new(),
        }
    }

    /// Keep the rebuilt maps under `<data_root>/cache/pogo`.
    pub fn with_cache(mut self, data_root: &Path) -> Self {
        self.cache = Some(data_root.join("cache").join(CACHE_DIR));
        self
    }

    /// Build with these facts given, not read from each map (tests).
    pub fn with_given(mut self, given: Given) -> Self {
        let mut h = Hasher::default();
        h.update(&fingerprint().0);
        field(&mut h, format!("{given:?}").as_bytes());
        self.fingerprint = h.finish();
        self.given = given;
        self
    }

    /// The cache file of map `stem`.
    pub fn cache_file(&self, stem: &str) -> Option<PathBuf> {
        Some(self.cache.as_ref()?.join(format!("{stem}.map")))
    }

    fn receipt_file(&self, stem: &str) -> Option<PathBuf> {
        Some(self.cache.as_ref()?.join(format!("{stem}.json")))
    }

    /// The key of map `stem` whose bytes are `original`.
    pub fn key(&self, stem: &str, original: &[u8]) -> Sha256 {
        let mut h = Hasher::default();
        field(&mut h, stem.as_bytes());
        h.update(&self.fingerprint.0);
        field(&mut h, original);
        h.finish()
    }

    fn load(&self, stem: &str, key: Sha256) -> Option<Added> {
        let receipt: Receipt =
            serde_json::from_slice(&std::fs::read(self.receipt_file(stem)?).ok()?).ok()?;
        if receipt.key != key {
            return None;
        }
        let file = self.cache_file(stem)?;
        if std::fs::metadata(&file).ok()?.len() > MAX_MAP {
            return None;
        }
        let bytes = std::fs::read(file).ok()?;
        (Sha256::of(&bytes) == receipt.map).then_some(Added {
            bytes,
            placed: Placed {
                weapons: receipt.weapons,
            },
            costs: receipt.costs,
        })
    }

    fn store(&self, stem: &str, key: Sha256, added: &Added) -> std::io::Result<()> {
        let (Some(map), Some(receipt_file)) = (self.cache_file(stem), self.receipt_file(stem))
        else {
            return Ok(());
        };
        // The map first, the receipt that vouches for it last.
        content::store::write_atomic(&map, &added.bytes)?;
        let receipt = Receipt {
            key,
            map: Sha256::of(&added.bytes),
            weapons: added.placed.weapons,
            costs: added.costs,
        };
        content::store::write_atomic(&receipt_file, &serde_json::to_vec_pretty(&receipt)?)
    }

    /// Map `stem` with the kit: from the cache when it holds this key,
    /// else built (and cached). An error when the map cannot take the kit.
    /// Records the map's weapons in [`placed`], or clears them.
    pub fn rebuild(&mut self, stem: &str, original: &[u8]) -> Result<Rebuilt> {
        let key = self.key(stem, original);
        if let Some(added) = self.load(stem, key) {
            publish(stem, original, added.placed);
            return Ok(Rebuilt {
                added,
                from_cache: true,
                file: self.cache_file(stem),
            });
        }
        let added = match add_pogo_with(original, stem, &self.given) {
            Ok(a) => a,
            Err(e) => {
                placed::clear(stem);
                return Err(e);
            }
        };
        let mut file = None;
        match self.store(stem, key, &added) {
            Ok(()) => file = self.cache_file(stem),
            Err(e) => eprintln!("pogo: could not cache {stem}: {e}"),
        }
        publish(stem, original, added.placed);
        Ok(Rebuilt {
            added,
            from_cache: false,
            file,
        })
    }

    fn name_once(&mut self, stem: &str, why: &str) {
        if self.named.insert(stem.to_owned()) {
            eprintln!("pogo: {stem} is served without the pogo stick: {why}");
        }
    }
}

/// `maps\<stem>.map`'s stem, lower case, when it may be a level.
pub fn stem_of(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let stem = lower.strip_prefix("maps\\")?.strip_suffix(".map")?;
    (!stem.is_empty() && !stem.contains(['\\', '/']) && !NEVER.contains(&stem))
        .then(|| stem.to_owned())
}

/// Records `p` for the map: under its file's stem, and under the name its
/// header carries, which is what the game keeps of a loaded map (the
/// loadout looks it up by that; the two are the same on a retail disc).
fn publish(stem: &str, original: &[u8], p: placed::Placed) {
    placed::set(stem, p);
    if let Ok(h) = map::Header::read_region(&mut Cursor::new(original), 0, original.len() as u64)
        && !h.name.is_empty()
        && !h.name.eq_ignore_ascii_case(stem)
    {
        placed::set(&h.name, p);
    }
}

/// Whether `original` is a multiplayer map, by its header.
fn is_multiplayer(original: &[u8]) -> bool {
    map::Header::read_region(&mut Cursor::new(original), 0, original.len() as u64)
        .is_ok_and(|h| h.cache_type == CacheType::Multiplayer)
}

impl vfs::Rewrite for PogoRewrite {
    fn wants(&self, path: &str) -> bool {
        stem_of(path).is_some()
    }

    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        let stem = stem_of(path)?;
        if !is_multiplayer(original) {
            return None;
        }
        match self.rebuild(&stem, original) {
            Ok(r) => Some(r.added.bytes),
            Err(e) => {
                self.name_once(&stem, &format!("{e:#}"));
                None
            }
        }
    }

    fn skipped(&mut self, path: &str) {
        let Some(stem) = stem_of(path) else {
            return;
        };
        placed::clear(&stem);
        // Its header was not read, so whether it is a multiplayer map is
        // not known here.
        self.name_once(
            &stem,
            "it is larger than a rewrite reads (this matters only for a multiplayer map)",
        );
    }
}
