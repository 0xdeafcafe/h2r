//! [`PackageRewrite`]: the per-map items of the game's set, served by the
//! VFS (docs/editor/studio-redesign.md, "How the game consumes packages",
//! 2).
//!
//! For each `maps\<name>.map` the game reads, the packages with an item
//! for it (by the map's name and its header's class) are applied in load
//! order through [`build_map_streamed`], the build `h2 package build` takes
//! ([`crate::build_map`]) on a map read by offset, with the same bytes. A
//! package whose added tags are all in the map already (a map that holds
//! the content) is left out, and a map no package is for is served as it
//! is.
//!
//! Only the map's header, index, names and metadata are held, and the
//! result is an overlay over the original ([`vfs::Rewritten::Over`]) whose
//! raw data and BSP regions the VFS serves from the disc. A map of any
//! size takes its packages; memory is what the build changes. A package
//! whose item would write the raw data or BSP regions in memory is refused
//! for the map by name.
//!
//! The result is kept under `<data>/packages/cache/`: `<map>-<key>.splice`
//! (16 hex digits of the map's SHA-256 and of [`cache_key`]), the
//! overlay's bytes, and a receipt beside it, `.json`, with the full key,
//! the splice's SHA-256 and where its runs go, so a second read is a cache
//! hit and a changed map, package or builder builds anew. A map that fails
//! to build is served unchanged and named once on stderr;
//! [`PackageRewrite::served`] keeps what happened to each map for
//! `packages.list`.
//!
//! A package refused for a map (docs/engine/asset-store.md, "Conflicts") is
//! off for it, whole, with its reason in the record's `refused`; the first
//! read of a map under a set asks the settling build
//! ([`crate::build_map_settled`]) and remembers, so a
//! later read builds or serves what the map takes straight. The cached map
//! is keyed by what the map takes, so a set that never had the refused
//! package shares it.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use content::Sha256;
use content::package::{Maps, Refusal};
use content::patch::{Overlay, OverlayView, Source};
use ::game::bot_mark::GearRewrite;
use ::game::campaign_cards::{CardRewrite, MAINMENU};
use map::Map;
use serde::{Deserialize, Serialize};
use vfs::{Rewrite as _, Rewritten};

use crate::base::{Base, Shared};
use crate::build::{Loaded, Target, cache_key};
use crate::builders::changes_map;
use crate::game::{CACHE_DIR, PACKAGES_DIR};
use crate::kits::{Kit, kits_of};
use crate::load::added_tag;
use crate::onto::{build_map_streamed, scope_of};

/// What happened to one map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ServedMap {
    /// The map's class, by its header.
    pub scope: String,
    /// The packages applied, in order.
    pub packages: Vec<String>,
    /// Packages for the map that it already holds, left out.
    pub held: Vec<String>,
    /// Packages off for this map, whole, and why (module docs).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<Refusal>,
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
    /// The splice file's SHA-256.
    map: Sha256,
    added: usize,
    changed: usize,
    /// Packages off for the map when it was built.
    refused: Vec<Refusal>,
    /// The built map's length, and how much of the base shows through.
    len: u64,
    base: u64,
    /// Each run of the splice file, in order: (offset in the map, length).
    runs: Vec<(u64, u64)>,
}

/// A map with the set applied.
#[derive(Debug)]
pub struct Rebuilt {
    pub bytes: Vec<u8>,
    pub from_cache: bool,
    /// The cache file, when there is a cache.
    pub file: Option<PathBuf>,
}

/// A map with the set applied, as an overlay over the base
/// ([`PackageRewrite::rebuild_streamed`]).
#[derive(Debug)]
pub struct RebuiltOver {
    pub overlay: Overlay,
    pub from_cache: bool,
    /// The cache file, when there is a cache.
    pub file: Option<PathBuf>,
}

/// What a boot does with each map as the game is handed it: the map's file
/// stem and its bytes after the set, read by offset.
pub type Observer = Box<dyn Fn(&str, &mut dyn Source) + Send>;

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
    packages: Arc<[Loaded]>,
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
    /// The maps come from the tag store with the set already laid over
    /// them ([`crate::level::StoreLevels`]): no map is rebuilt here.
    levels: bool,
    /// The packages that are ours: one of them refused stops a map's build
    /// instead of being left out.
    strict: BTreeSet<String>,
    /// For each map and set asked for (the [`cache_key`] over every package
    /// with an item for the map, hex), the packages a build found off for
    /// the map: the first read asks, later ones skip them.
    settled: BTreeMap<String, Vec<Refusal>>,
}

impl PackageRewrite {
    /// The rewrite of `packages`, on, in load order ([`crate::GameSet::loaded`]).
    pub fn new(packages: impl Into<Arc<[Loaded]>>) -> Self {
        let packages = packages.into();
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
            levels: false,
            strict: BTreeSet::new(),
            settled: BTreeMap::new(),
        }
    }

    /// The packages that are ours, by id (`strict`): a refusal of one stops
    /// a map's build, as a failure does; any other package refused for a map
    /// is off for it, with its reason.
    pub fn strict(mut self, strict: BTreeSet<String>) -> Self {
        self.strict = strict;
        self
    }

    /// The maps are the tag store's level images, which hold the set's
    /// items already ([`crate::level`]): the rewrite rebuilds none, and
    /// still runs the kits and the observers on what the game is handed.
    pub fn levels_from_store(mut self) -> Self {
        self.levels = true;
        self
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
    /// `content_kit::served`).
    pub fn observed(mut self, f: impl Fn(&str, &mut dyn Source) + Send + 'static) -> Self {
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
            dir.join(format!("{stem}.splice")),
            dir.join(format!("{stem}.json")),
        ))
    }

    /// The cached overlay over a base of `base_len` bytes.
    fn load(
        &self,
        base: &Sha256,
        key: &Sha256,
        base_len: u64,
    ) -> Option<(Overlay, Receipt, PathBuf)> {
        let (splice, receipt) = self.files(base, key)?;
        let receipt: Receipt = serde_json::from_slice(&std::fs::read(receipt).ok()?).ok()?;
        if receipt.key != *key || receipt.base > base_len {
            return None;
        }
        let bytes = std::fs::read(&splice).ok()?;
        if Sha256::of(&bytes) != receipt.map
            || receipt.runs.iter().map(|r| r.1).sum::<u64>() != bytes.len() as u64
        {
            return None;
        }
        let mut overlay = Overlay::new(base_len);
        overlay.set_len(receipt.base);
        let mut at = 0;
        for &(offset, len) in &receipt.runs {
            overlay.write(offset, &bytes[at..at + len as usize]);
            at += len as usize;
        }
        (overlay.len() == receipt.len).then_some((overlay, receipt, splice))
    }

    fn store(
        &self,
        base: &Sha256,
        key: &Sha256,
        overlay: &Overlay,
        mut receipt: Receipt,
    ) -> Option<PathBuf> {
        let (splice, file) = self.files(base, key)?;
        let mut bytes = Vec::with_capacity(overlay.held() as usize);
        for r in overlay.runs() {
            bytes.extend_from_slice(&r.bytes);
        }
        receipt.map = Sha256::of(&bytes);
        receipt.len = overlay.len();
        receipt.base = overlay.base_visible();
        receipt.runs = overlay
            .runs()
            .iter()
            .map(|r| (r.offset, r.bytes.len() as u64))
            .collect();
        // The splice first, the receipt that vouches for it last.
        let done = content::store::write_atomic(&splice, &bytes).and_then(|()| {
            let text = serde_json::to_vec_pretty(&receipt).map_err(std::io::Error::other)?;
            content::store::write_atomic(&file, &text)
        });
        match done {
            Ok(()) => Some(splice),
            Err(e) => {
                eprintln!("packages: could not cache {}: {e}", splice.display());
                None
            }
        }
    }

    /// Map `name` (its file's stem) whose bytes are `original`, with the
    /// set applied ([`Self::rebuild_streamed`], laid over `original`).
    pub fn rebuild(&mut self, name: &str, original: &[u8]) -> Result<Option<Rebuilt>, String> {
        let mut source = original;
        Ok(self.rebuild_streamed(name, &mut source)?.map(|r| Rebuilt {
            bytes: r.overlay.materialise(original.to_vec()),
            from_cache: r.from_cache,
            file: r.file,
        }))
    }

    /// Map `name` (its file's stem), read from `source`, with the set
    /// applied as an overlay over it: from the cache when it holds the key,
    /// else built ([`build_map_streamed`]) and cached. `Ok(None)` when no
    /// package adds anything to it, as when every package is refused for it
    /// (module docs).
    pub fn rebuild_streamed(
        &mut self,
        name: &str,
        source: &mut dyn Source,
    ) -> Result<Option<RebuiltOver>, String> {
        let name = name.to_ascii_lowercase();
        let shared = Shared::new(source);
        let original = Base::from(&shared);
        let base_len = original.len();
        let map = Map::from_reader(original.reader()).map_err(|e| format!("not a map: {e}"))?;
        let scope = scope_of(map.header().cache_type);
        let target = Target { name: &name, scope };
        let (mut applied, held) = applied_to(&self.packages, &self.built, &target, &map);
        drop(map);
        let mut record = ServedMap {
            scope: scope.word().to_owned(),
            held,
            ..ServedMap::default()
        };
        if applied.is_empty() {
            merge(&self.served, &name, record);
            return Ok(None);
        }
        let base = original
            .sha256()
            .map_err(|e| format!("reading the map: {e}"))?;
        // What the map takes. A read that found packages off for it remembers
        // them by the key over every package asked for. Otherwise the cache
        // under that key is tried first: a map built with a package refused
        // is keyed by what the map takes, so a hit there means the map takes
        // them all. Else the build says.
        let asked = cache_key(&base, &applied, &target);
        let mut built = None;
        let refused = match self.settled.get(&asked.to_string()).cloned() {
            Some(refused) => refused,
            None => match self.load(&base, &asked, base_len) {
                Some((overlay, receipt, file)) => {
                    record.key = asked.to_string();
                    record.packages = applied
                        .iter()
                        .map(|p| p.package.name().to_owned())
                        .collect();
                    record.hits = 1;
                    record.added = receipt.added;
                    record.changed = receipt.changed;
                    merge(&self.served, &name, record);
                    return Ok(Some(RebuiltOver {
                        overlay,
                        from_cache: true,
                        file: Some(file),
                    }));
                }
                None => match build_map_streamed(
                    original,
                    &applied,
                    &target,
                    Some(&self.strict),
                    Some(&mut crate::assets::OfMap::of_map(original)),
                ) {
                    Ok(map) => {
                        let refused = map.report.refused.clone();
                        self.settled.insert(asked.to_string(), refused.clone());
                        built = Some(map);
                        refused
                    }
                    Err(why) => {
                        record.error = Some(why.clone());
                        merge(&self.served, &name, record);
                        return Err(why);
                    }
                },
            },
        };
        applied.retain(|p| !refused.iter().any(|r| r.package == p.package.name()));
        record.packages = applied
            .iter()
            .map(|p| p.package.name().to_owned())
            .collect();
        record.refused = refused.clone();
        if applied.is_empty() {
            merge(&self.served, &name, record);
            return Ok(None);
        }
        let key = cache_key(&base, &applied, &target);
        record.key = key.to_string();
        if built.is_none() {
            if let Some((overlay, receipt, file)) = self.load(&base, &key, base_len) {
                record.hits = 1;
                record.added = receipt.added;
                record.changed = receipt.changed;
                merge(&self.served, &name, record);
                return Ok(Some(RebuiltOver {
                    overlay,
                    from_cache: true,
                    file: Some(file),
                }));
            }
            match build_map_streamed(original, &applied, &target, None, None) {
                Ok(map) => built = Some(map),
                Err(e) => {
                    record.error = Some(e.clone());
                    merge(&self.served, &name, record);
                    return Err(e);
                }
            }
        }
        let built = built.expect("built above");
        let overlay = built.overlay;
        let receipt = Receipt {
            key,
            map: Sha256::of(&[]),
            added: built.report.added.len(),
            changed: built.report.changed.len(),
            refused,
            len: 0,
            base: 0,
            runs: Vec::new(),
        };
        record.builds = 1;
        record.added = receipt.added;
        record.changed = receipt.changed;
        let file = self.store(&base, &key, &overlay, receipt);
        merge(&self.served, &name, record);
        Ok(Some(RebuiltOver {
            overlay,
            from_cache: false,
            file,
        }))
    }

    fn name_once(&mut self, name: &str, why: &str) {
        if self.named.insert(name.to_owned()) {
            eprintln!("packages: {name} is served without its packages: {why}");
        }
    }
}

/// The packages of `packages` (load order) with an item for `target`,
/// whose base is `map`: (the ones to apply, the names of those the map
/// already holds). A package whose added tags are all in the map already
/// is held, not applied; a map a `map` item built holds the items it took
/// (`built`, [`crate::without_taken`]). The game's rewrite and the level
/// images ([`crate::level::assemble`]) choose the same way.
pub(crate) fn applied_to<R: std::io::Read + std::io::Seek>(
    packages: &[Loaded],
    built: &HashSet<String>,
    target: &Target<'_>,
    map: &Map<R>,
) -> (Vec<Loaded>, Vec<String>) {
    let name = target.name.to_ascii_lowercase();
    let cut;
    let packages = if built.contains(&name) {
        cut = crate::maps::without_taken(packages, &name);
        &cut
    } else {
        packages
    };
    let mut applied = Vec::new();
    let mut held = Vec::new();
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
    (applied, held)
}

/// Fold `now` into the map's record, keeping the counts.
pub(crate) fn merge(served: &Served, name: &str, mut now: ServedMap) {
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

    /// [`Self::rewrite_streamed`] over `original`, laid over it.
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        let mut source = original;
        self.rewrite_streamed(path, &mut source)
            .ok()
            .flatten()
            .map(|r| r.materialise(original))
    }

    fn rewrite_streamed(
        &mut self,
        path: &str,
        source: &mut dyn Source,
    ) -> std::io::Result<Option<Rewritten>> {
        if is_font(path) {
            let original = source.read_all()?;
            return Ok(self.font(path, &original).map(Rewritten::Whole));
        }
        let Some(stem) = stem_of(path) else {
            return Ok(None);
        };
        // The level pictures first, on the whole map (mainmenu), then the
        // set's items on that map.
        let carded = if self.cards_for(path) {
            let original = source.read_all()?;
            self.inputs
                .cards
                .as_mut()
                .and_then(|c| c.rewrite(path, &original))
        } else {
            None
        };
        let out = if self.levels {
            carded.map(Rewritten::Whole)
        } else {
            let built = match &carded {
                Some(bytes) => self.rebuild_streamed(&stem, &mut bytes.as_slice()),
                None => self.rebuild_streamed(&stem, source),
            };
            match built {
                Ok(Some(r)) => Some(match carded {
                    Some(bytes) => Rewritten::Whole(r.overlay.materialise(bytes)),
                    None => Rewritten::Over(r.overlay),
                }),
                Ok(None) => carded.map(Rewritten::Whole),
                Err(e) => {
                    self.name_once(&stem, &e);
                    carded.map(Rewritten::Whole)
                }
            }
        };
        if let Some(f) = &self.observer {
            match &out {
                None => f(&stem, source),
                Some(Rewritten::Whole(bytes)) => f(&stem, &mut bytes.as_slice()),
                Some(Rewritten::Over(overlay)) => f(
                    &stem,
                    &mut OverlayView {
                        overlay,
                        base: source,
                    },
                ),
            }
        }
        Ok(out)
    }
}
