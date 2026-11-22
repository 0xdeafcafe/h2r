//! Serves the maps with our screens in them (docs/game/ui-in-maps.md,
//! "The rewrite and its cache"): the VFS reads a disc map once, hands it
//! to [`ScreensRewrite`], and serves what comes back, as
//! `campaign_cards::CardRewrite` serves mainmenu with the level pictures.
//!
//! The rebuilt map is kept on the host, one file per map under
//! `<data>/cache/ui-screens/`: `<map>.map` is the map itself, a plain
//! cache file the editor opens as it opens any loose map, and
//! `<map>.json` says which input it was made from. The key is the SHA-256
//! of the input map's bytes, the docs ([`Docs`]: `assets/ui/design.yaml`
//! and every screen's file as baked into the build), the plan the docs and
//! the screens' samples give ([`super::plan`]) and [`VERSION`], so a boot
//! after the first reads the file back, and a changed doc, screen or
//! generator rebuilds.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use content::{Hasher, Sha256};
use serde::{Deserialize, Serialize};

use super::tag::{Added, add_screens};
use super::{Entry, plan};
use crate::ui_kit::data;

/// Changes with the generator, so a cache made by another one is not read.
pub const VERSION: &str = "ui-inject-1";

/// The maps whose interface holds screens the game opens by id: the main
/// menu's, and the two shared maps' (`shared`: multiplayer, and
/// `single_player_shared`: campaign). The levels are not rewritten until
/// a game run shows the game reads its screens from them
/// (docs/game/ui-in-maps.md, "Which maps").
pub const MAPS: [&str; 3] = ["mainmenu", "shared", "single_player_shared"];

/// A rebuilt map is read back only up to this size.
const MAX_MAP: u64 = 256 << 20;

/// The docs a build is made from: the design system's file and each
/// screen's, by name, as their text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Docs {
    pub design: String,
    pub screens: Vec<(String, String)>,
}

impl Docs {
    /// The docs baked into this build.
    pub fn baked() -> Docs {
        Docs {
            design: data::baked::DESIGN.into(),
            screens: data::baked::SCREENS
                .iter()
                .map(|(n, t)| ((*n).into(), (*t).into()))
                .collect(),
        }
    }

    /// The docs' digest: every file's name and text, in name order.
    pub fn hash(&self) -> Sha256 {
        let mut h = Hasher::default();
        field(&mut h, b"design");
        field(&mut h, self.design.as_bytes());
        let mut screens: Vec<&(String, String)> = self.screens.iter().collect();
        screens.sort();
        for (name, text) in screens {
            field(&mut h, name.as_bytes());
            field(&mut h, text.as_bytes());
        }
        h.finish()
    }
}

fn field(h: &mut Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

/// What [`ScreensRewrite::rebuild`] gives back.
#[derive(Debug)]
pub struct Rebuilt {
    pub map: Vec<u8>,
    pub from_cache: bool,
    /// The cache file the map is in, when there is a cache.
    pub file: Option<PathBuf>,
    pub injected: usize,
    pub skipped: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    key: Sha256,
    map: Sha256,
    #[serde(flatten)]
    added: Added,
}

/// Serves `maps\mainmenu.map` and the shared maps with our screens added.
pub struct ScreensRewrite {
    plan: Vec<Entry>,
    /// The docs, the plan and the version: what a map's key adds to the
    /// map's own bytes.
    fingerprint: Sha256,
    cache: Option<PathBuf>,
}

impl ScreensRewrite {
    /// A rewrite of `docs` and the plan they give. The error names the
    /// screen whose doc or sample is wrong.
    pub fn new(docs: &Docs) -> Result<Self, String> {
        Ok(Self::with_plan(docs, plan()?))
    }

    /// A rewrite of this build's baked docs.
    pub fn baked() -> Result<Self, String> {
        Self::new(&Docs::baked())
    }

    /// A rewrite of an explicit plan (the tests'; [`ScreensRewrite::new`]
    /// makes the real one).
    pub fn with_plan(docs: &Docs, plan: Vec<Entry>) -> Self {
        let mut h = Hasher::default();
        field(&mut h, VERSION.as_bytes());
        h.update(&docs.hash().0);
        field(&mut h, &serde_json::to_vec(&plan).unwrap_or_default());
        Self {
            plan,
            fingerprint: h.finish(),
            cache: None,
        }
    }

    /// Keep the rebuilt maps under `<data_root>/cache/ui-screens`.
    pub fn with_cache(mut self, data_root: &Path) -> Self {
        self.cache = Some(data_root.join("cache").join("ui-screens"));
        self
    }

    /// The screens the plan lists, in order.
    pub fn plan(&self) -> &[Entry] {
        &self.plan
    }

    /// The cache file of map `stem` (`mainmenu`): the rebuilt map as the
    /// editor opens it.
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
        let map = std::fs::read(file).ok()?;
        (Sha256::of(&map) == receipt.map).then_some(Added {
            map,
            ..receipt.added
        })
    }

    fn store(&self, stem: &str, key: Sha256, added: &Added) -> std::io::Result<()> {
        let (Some(map), Some(receipt_file)) = (self.cache_file(stem), self.receipt_file(stem)) else {
            return Ok(());
        };
        // The map first, the receipt that vouches for it last.
        content::store::write_atomic(&map, &added.map)?;
        let receipt = Receipt {
            key,
            map: Sha256::of(&added.map),
            added: Added {
                map: Vec::new(),
                ..added.clone()
            },
        };
        content::store::write_atomic(&receipt_file, &serde_json::to_vec_pretty(&receipt)?)
    }

    /// Map `stem`'s bytes with our screens in them: from the cache when
    /// it holds this key, else built (and cached). An error when the map
    /// is not one the screens can be added to.
    pub fn rebuild(&mut self, stem: &str, original: &[u8]) -> Result<Rebuilt> {
        let key = self.key(stem, original);
        if let Some(added) = self.load(stem, key) {
            return Ok(Rebuilt {
                from_cache: true,
                file: self.cache_file(stem),
                injected: added.injected.len(),
                skipped: added.skipped,
                map: added.map,
            });
        }
        let added = add_screens(original, &self.plan).with_context(|| format!("{stem}.map"))?;
        let mut file = None;
        match self.store(stem, key, &added) {
            Ok(()) => file = self.cache_file(stem),
            Err(e) => eprintln!("ui screens: could not cache {stem}: {e}"),
        }
        Ok(Rebuilt {
            from_cache: false,
            file,
            injected: added.injected.len(),
            skipped: added.skipped,
            map: added.map,
        })
    }
}

/// `maps\<stem>.map`'s stem, lower case, when it is one of [`MAPS`].
fn stem_of(path: &str) -> Option<&'static str> {
    MAPS.iter().copied().find(|stem| {
        path.eq_ignore_ascii_case(&format!("maps\\{stem}.map"))
    })
}

impl vfs::Rewrite for ScreensRewrite {
    fn wants(&self, path: &str) -> bool {
        stem_of(path).is_some()
    }

    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        let stem = stem_of(path)?;
        let started = Instant::now();
        match self.rebuild(stem, original) {
            Ok(r) => {
                eprintln!(
                    "ui screens: {} screens in {stem}{} ({} ms)",
                    r.injected,
                    if r.from_cache { " from cache" } else { "" },
                    started.elapsed().as_millis()
                );
                for (name, why) in &r.skipped {
                    eprintln!("ui screens: {stem}: {name} not added: {why}");
                }
                Some(r.map)
            }
            Err(e) => {
                eprintln!("ui screens: {stem} left as it is: {e:#}");
                None
            }
        }
    }

    fn skipped(&mut self, path: &str) {
        eprintln!("ui screens: {path} is too large to rewrite; no screens of ours in it");
    }
}
