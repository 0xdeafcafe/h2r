//! [`PackageRewrite`]: the per-map items of the game's set, served by the
//! VFS (docs/editor/studio-redesign.md, "How the game consumes packages",
//! 2).
//!
//! For each `maps\<name>.map` the game reads, the packages with an item
//! for it (by the map's name and its header's class) are applied in load
//! order through [`build_map`], the same path `h2 package build` takes. A
//! package whose added tags are all in the map already (a map that holds
//! the content) is left out, and a map no package is for is served as it
//! is.
//!
//! The result is kept under `<data>/packages/cache/`: `<map>-<key>.map`
//! (16 hex digits of the map's SHA-256 and of [`cache_key`]) and a receipt
//! beside it, `.json`, with the full key and the output's SHA-256, so a
//! second read is a cache hit and a changed map, package or builder builds
//! anew. A map that fails to build is served unchanged and named once on
//! stderr; [`PackageRewrite::served`] keeps what happened to each map for
//! `packages.list`.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use content::Sha256;
use content::package::Maps;
use ::game::bot_mark::GearRewrite;
use ::game::campaign_cards::{CardRewrite, MAINMENU};
use map::Map;
use serde::{Deserialize, Serialize};
use vfs::Rewrite as _;

use crate::build::{Loaded, Target, cache_key};
use crate::builders::changes_map;
use crate::game::{CACHE_DIR, PACKAGES_DIR};
use crate::kits::{Kit, kits_of};
use crate::load::added_tag;
use crate::onto::{build_map, scope_of};

/// What happened to one map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ServedMap {
    /// The map's class, by its header.
    pub scope: String,
    /// The packages applied, in order.
    pub packages: Vec<String>,
    /// Packages for the map that it already holds, left out.
    pub held: Vec<String>,
    /// [`cache_key`], hex; empty when nothing was applied.
    pub key: String,
    /// Reads served from the cache, and builds.
    pub hits: u32,
    pub builds: u32,
    /// Tags added and changed by the last build or the receipt.
    pub added: usize,
    pub changed: usize,
    /// Why the map was served unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Every map the rewrite was asked for, by name, shared with
/// `packages.list`.
pub type Served = Arc<Mutex<BTreeMap<String, ServedMap>>>;

fn lock(s: &Served) -> MutexGuard<'_, BTreeMap<String, ServedMap>> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    key: Sha256,
    /// The cached map's SHA-256.
    map: Sha256,
    added: usize,
    changed: usize,
}

/// A map with the set applied.
#[derive(Debug)]
pub struct Rebuilt {
    pub bytes: Vec<u8>,
    pub from_cache: bool,
    /// The cache file, when there is a cache.
    pub file: Option<PathBuf>,
}

/// What a boot does with each map as the game is handed it: the map's file
/// stem and its bytes after the set, or `None` when the map was too large
/// for a rewrite to read.
pub type Observer = Box<dyn Fn(&str, Option<&[u8]>) + Send>;

/// The font files' folder, and the table that names them.
const FONTS: &str = "maps\\fonts\\";
const FONT_TABLE: &str = "maps\\fonts\\font_table.txt";

/// A file of `maps\fonts`.
fn is_font(path: &str) -> bool {
    path.to_ascii_lowercase().starts_with(FONTS)
}

/// A font file the `glyphs` kit patches: the fonts, not the table.
fn is_font_file(path: &str) -> bool {
    is_font(path) && !path.to_ascii_lowercase().ends_with(".txt")
}

/// What the boot gives the kits of the set to run on: the game's inputs,
/// which no package holds ([`crate::kits`]).
#[derive(Default)]
pub struct KitInputs {
    /// `campaign-cards`: the level pictures, with the table the level lists
    /// read the cards' datums from.
    pub cards: Option<CardRewrite>,
    /// `glyphs`: the button icon sets, which go into every font before the
    /// gear and the other marks (`game::icons::Icons::font_rewrite`).
    pub icons: Option<Box<dyn vfs::Rewrite>>,
}

/// What the boot does with each font file and the font table as the game
/// is handed them: the path and the bytes after the set.
pub type FontObserver = Box<dyn Fn(&str, &[u8]) + Send>;

/// The one rewrite of the game's set.
pub struct PackageRewrite {
    packages: Vec<Loaded>,
    cache: Option<PathBuf>,
    /// Whether an item names a map by its class (every map may be for it).
    by_class: bool,
    /// The map names items name, lower case.
    names: HashSet<String>,
    /// The maps the set's `map` items build: they hold the items they
    /// took ([`crate::without_taken`]).
    built: HashSet<String>,
    served: Served,
    /// The maps already named on stderr.
    named: HashSet<String>,
    observer: Option<Observer>,
    /// The kits the set carries, and the inputs they run on.
    kits: BTreeSet<Kit>,
    inputs: KitInputs,
    fonts: Option<FontObserver>,
}

impl PackageRewrite {
    /// The rewrite of `packages`, on, in load order ([`crate::GameSet::loaded`]).
    pub fn new(packages: Vec<Loaded>) -> Self {
        let mut by_class = false;
        let mut names = HashSet::new();
        for i in packages
            .iter()
            .flat_map(|p| &p.package.manifest.items)
            .filter(|i| changes_map(i))
        {
            match &i.maps {
                Maps::Scope(_) => by_class = true,
                Maps::Names(n) => names.extend(n.iter().map(|n| n.to_ascii_lowercase())),
            }
        }
        let built = crate::maps::built_names(&packages).into_iter().collect();
        let kits = kits_of(&packages.iter().map(|p| &p.package).collect::<Vec<_>>());
        Self {
            packages,
            cache: None,
            by_class,
            names,
            built,
            served: Served::default(),
            named: HashSet::new(),
            observer: None,
            kits,
            inputs: KitInputs::default(),
            fonts: None,
        }
    }

    /// The inputs the set's kits run on. A kit with none does nothing.
    pub fn with_kits(mut self, inputs: KitInputs) -> Self {
        self.inputs = inputs;
        self
    }

    /// Call `f` with each font file and the font table as the game is
    /// handed them (the superbounce labels' HUD font is read from them:
    /// `game::superbounce_font`).
    pub fn observing_fonts(mut self, f: impl Fn(&str, &[u8]) + Send + 'static) -> Self {
        self.fonts = Some(Box::new(f));
        self
    }

    /// Whether the set carries `kit`.
    pub fn has_kit(&self, kit: Kit) -> bool {
        self.kits.contains(&kit)
    }

    /// Call `f` with each map the rewrite is asked for, as the game is
    /// handed it (the pogo stick's weapons are read from it:
    /// `pogo_kit::served`).
    pub fn observed(mut self, f: impl Fn(&str, Option<&[u8]>) + Send + 'static) -> Self {
        self.observer = Some(Box::new(f));
        self
    }

    /// Keep the built maps under `<data_root>/packages/cache`.
    pub fn with_cache(mut self, data_root: &Path) -> Self {
        self.cache = Some(data_root.join(PACKAGES_DIR).join(CACHE_DIR));
        self
    }

    /// What happened to each map, as it happens.
    pub fn served(&self) -> Served {
        Arc::clone(&self.served)
    }

    /// Whether the rewrite has anything to add to any map.
    pub fn is_empty(&self) -> bool {
        self.kits.is_empty()
            && self
                .packages
                .iter()
                .all(|p| p.package.manifest.items.iter().all(|i| !changes_map(i)))
    }

    /// Whether the set's `campaign-cards` kit is for `path`.
    fn cards_for(&self, path: &str) -> bool {
        self.has_kit(Kit::CampaignCards)
            && self.inputs.cards.is_some()
            && path.eq_ignore_ascii_case(MAINMENU)
    }

    /// A font file or the table, as the game reads it: the icon sets, then
    /// the gear and the marks (the `glyphs` kit), then the observer.
    fn font(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        let mut out: Option<Vec<u8>> = None;
        if self.has_kit(Kit::Glyphs) && is_font_file(path) {
            if let Some(icons) = self.inputs.icons.as_mut()
                && icons.wants(path)
            {
                out = icons.rewrite(path, original);
            }
            let bytes = out.as_deref().unwrap_or(original);
            if let Some(gear) = GearRewrite.rewrite(path, bytes) {
                out = Some(gear);
            }
        }
        if let Some(f) = &self.fonts {
            f(path, out.as_deref().unwrap_or(original));
        }
        out
    }

    fn files(&self, base: &Sha256, key: &Sha256) -> Option<(PathBuf, PathBuf)> {
        let dir = self.cache.as_ref()?;
        let stem = format!("{}-{}", &base.to_string()[..16], &key.to_string()[..16]);
        Some((
            dir.join(format!("{stem}.map")),
            dir.join(format!("{stem}.json")),
        ))
    }

    fn load(&self, base: &Sha256, key: &Sha256) -> Option<(Vec<u8>, Receipt, PathBuf)> {
        let (map, receipt) = self.files(base, key)?;
        let receipt: Receipt = serde_json::from_slice(&std::fs::read(receipt).ok()?).ok()?;
        if receipt.key != *key {
            return None;
        }
        let bytes = std::fs::read(&map).ok()?;
        (Sha256::of(&bytes) == receipt.map).then_some((bytes, receipt, map))
    }

    fn store(
        &self,
        base: &Sha256,
        key: &Sha256,
        bytes: &[u8],
        receipt: &Receipt,
    ) -> Option<PathBuf> {
        let (map, file) = self.files(base, key)?;
        // The map first, the receipt that vouches for it last.
        let done = content::store::write_atomic(&map, bytes).and_then(|()| {
            let text = serde_json::to_vec_pretty(receipt).map_err(std::io::Error::other)?;
            content::store::write_atomic(&file, &text)
        });
        match done {
            Ok(()) => Some(map),
            Err(e) => {
                eprintln!("packages: could not cache {}: {e}", map.display());
                None
            }
        }
    }

    /// Map `name` (its file's stem) whose bytes are `original`, with the
    /// set applied: from the cache when it holds the key, else built and
    /// cached. `Ok(None)` when no package adds anything to it.
    pub fn rebuild(&mut self, name: &str, original: &[u8]) -> Result<Option<Rebuilt>, String> {
        let name = name.to_ascii_lowercase();
        let map = Map::from_reader(Cursor::new(original)).map_err(|e| format!("not a map: {e}"))?;
        let scope = scope_of(map.header().cache_type);
        let target = Target { name: &name, scope };
        let mut applied = Vec::new();
        let mut held = Vec::new();
        // A map a `map` item built holds the items it took.
        let cut;
        let packages = if self.built.contains(&name) {
            cut = crate::maps::without_taken(&self.packages, &name);
            &cut
        } else {
            &self.packages
        };
        for p in packages {
            let items: Vec<_> = p
                .package
                .manifest
                .items
                .iter()
                .filter(|i| changes_map(i) && target.takes(&i.maps))
                .collect();
            if items.is_empty() {
                continue;
            }
            let adds: Vec<_> = items.iter().filter_map(|i| added_tag(i)).collect();
            let holds = !adds.is_empty()
                && adds.iter().all(|k| {
                    map.tags().iter().any(|t| {
                        t.group == k.group
                            && t.name
                                .as_deref()
                                .is_some_and(|n| n.eq_ignore_ascii_case(&k.path))
                    })
                });
            if holds {
                held.push(p.package.name().to_owned());
            } else {
                applied.push(p.clone());
            }
        }
        drop(map);
        let mut record = ServedMap {
            scope: scope.word().to_owned(),
            packages: applied
                .iter()
                .map(|p| p.package.name().to_owned())
                .collect(),
            held,
            ..ServedMap::default()
        };
        if applied.is_empty() {
            merge(&self.served, &name, record);
            return Ok(None);
        }
        let base = Sha256::of(original);
        let key = cache_key(&base, &applied, &target);
        record.key = key.to_string();
        if let Some((bytes, receipt, file)) = self.load(&base, &key) {
            record.hits = 1;
            record.added = receipt.added;
            record.changed = receipt.changed;
            merge(&self.served, &name, record);
            return Ok(Some(Rebuilt {
                bytes,
                from_cache: true,
                file: Some(file),
            }));
        }
        match build_map(original, &applied, &target) {
            Ok((bytes, report)) => {
                let receipt = Receipt {
                    key,
                    map: Sha256::of(&bytes),
                    added: report.added.len(),
                    changed: report.changed.len(),
                };
                let file = self.store(&base, &key, &bytes, &receipt);
                record.builds = 1;
                record.added = receipt.added;
                record.changed = receipt.changed;
                merge(&self.served, &name, record);
                Ok(Some(Rebuilt {
                    bytes,
                    from_cache: false,
                    file,
                }))
            }
            Err(e) => {
                record.error = Some(e.clone());
                merge(&self.served, &name, record);
                Err(e)
            }
        }
    }

    fn name_once(&mut self, name: &str, why: &str) {
        if self.named.insert(name.to_owned()) {
            eprintln!("packages: {name} is served without its packages: {why}");
        }
    }
}

/// Fold `now` into the map's record, keeping the counts.
fn merge(served: &Served, name: &str, mut now: ServedMap) {
    let mut all = lock(served);
    if let Some(was) = all.get(name) {
        now.hits += was.hits;
        now.builds += was.builds;
    }
    all.insert(name.to_owned(), now);
}

/// `maps\<stem>.map`'s stem, lower case.
pub fn stem_of(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase().replace('/', "\\");
    let stem = lower.strip_prefix("maps\\")?.strip_suffix(".map")?;
    (!stem.is_empty() && !stem.contains('\\')).then(|| stem.to_owned())
}

impl vfs::Rewrite for PackageRewrite {
    fn wants(&self, path: &str) -> bool {
        if is_font(path) {
            (self.fonts.is_some() && (is_font_file(path) || path.eq_ignore_ascii_case(FONT_TABLE)))
                || (self.has_kit(Kit::Glyphs) && is_font_file(path))
        } else {
            self.cards_for(path)
                || stem_of(path).is_some_and(|s| self.by_class || self.names.contains(&s))
        }
    }

    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        if is_font(path) {
            return self.font(path, original);
        }
        let stem = stem_of(path)?;
        // The level pictures first, then the set's items on that map.
        let carded = if self.cards_for(path) {
            self.inputs
                .cards
                .as_mut()
                .and_then(|c| c.rewrite(path, original))
        } else {
            None
        };
        let input = carded.as_deref().unwrap_or(original);
        let out = match self.rebuild(&stem, input) {
            Ok(r) => r.map(|r| r.bytes).or(carded),
            Err(e) => {
                self.name_once(&stem, &e);
                carded
            }
        };
        if let Some(f) = &self.observer {
            f(&stem, Some(out.as_deref().unwrap_or(original)));
        }
        out
    }

    fn skipped(&mut self, path: &str) {
        if is_font_file(path) && self.has_kit(Kit::Glyphs) {
            if let Some(icons) = self.inputs.icons.as_mut() {
                icons.skipped(path);
            }
            GearRewrite.skipped(path);
            return;
        }
        if self.cards_for(path)
            && let Some(cards) = self.inputs.cards.as_mut()
        {
            cards.skipped(path);
        }
        if let Some(stem) = stem_of(path) {
            let why = "it is larger than a rewrite reads";
            merge(
                &self.served,
                &stem,
                ServedMap {
                    error: Some(why.into()),
                    ..ServedMap::default()
                },
            );
            self.name_once(&stem, why);
            if let Some(f) = &self.observer {
                f(&stem, None);
            }
        }
    }
}
