//! Level images (docs/engine/asset-store.md, "Runtime assembly"): a level
//! of the tag store with packages laid over it, built once and kept by its
//! [`LevelId`].
//!
//! [`assemble`] takes a level of the store (its skeleton and extent table,
//! `tags::repo::skeleton`) and the game's set in load order. The packages
//! with an item for the level are chosen as the game's rewrite chooses
//! them ([`crate::rewrite::applied_to`]). With none, the level is the
//! disc's bytes: no image, its retail [`MapId`]. Otherwise the level's
//! [`LevelId`] names its image under `<data>/store/images/`: a hit is used
//! as it is; a miss runs [`build_map_laid_out`] on the map the store gives
//! back and [`split`]s the result:
//!
//! ```text
//! <images>/<LevelId>.img       the image's skeleton (tags::repo::skeleton::Skeleton)
//! <images>/<LevelId>.extents   its extent table (tags::repo::skeleton::Extents)
//! ```
//!
//! A range of the built map that equals one of the base's extents at the
//! same offset stays that extent, so it names the raw file the store
//! already has; a raw resource the build placed is stored in the store's
//! `raw/` by hash (once) and named the same way; everything else is the
//! image's skeleton. `vfs::store_map` serves the image through
//! [`tags::repo::StoreFile`] exactly as it serves a retail level.
//!
//! The image is written skeleton first and extent table last, so a
//! stopped write leaves no table and the next call builds again. An image
//! whose metadata is over the tag memory bound for its type (the retail
//! bound, or the base's own total when that is larger) is refused naming
//! the tags the packages added and changed ([`check_tag_memory`]).
//! [`trim`] keeps the folder under a cap, least recently used first,
//! sparing the ids a caller names (the images saved films use).
//!
//! **Identity.** A level with nothing over it has for its [`LevelId`] its
//! skeleton's hash and, from the store's catalogue
//! (`tags::repo::catalogue`), every asset of its manifest by hash
//! ([`level_assets`]): a join names the assets that differ without
//! reading a tag. With packages over it the skeleton's hash stands for
//! the base and the packages' ids for what they change, and no asset is
//! named.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use content::package::MapScope;
use content::{LevelId, LevelInputs, MapId, Sha256};
use map::{CacheType, Map};
use tags::repo::StoreFile;
use tags::repo::catalogue::Catalogue;
use tags::repo::skeleton::{Extent, Extents, Skeleton};

use crate::build::{Loaded, Report, Target};
use crate::builders::BUILDERS_VERSION;
use crate::onto::{BuiltMap, build_map_laid_out, scope_of};
use crate::rewrite::{Served, ServedMap, applied_to, merge};

/// The images' folder under the data root.
pub const IMAGES_DIR: &str = "store/images";
/// Extension of an image's skeleton.
pub const IMAGE_EXT: &str = "img";
/// Extension of an image's extent table.
pub const IMAGE_EXTENTS_EXT: &str = "extents";
/// The images' cap when the caller names none: 4 GiB.
pub const DEFAULT_IMAGE_CAP: u64 = 4 << 30;

/// `<data_root>/store/images`.
pub fn images_dir(data_root: &Path) -> PathBuf {
    data_root.join(IMAGES_DIR)
}

/// A level's image on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// `<LevelId>.img`.
    pub skeleton: PathBuf,
    /// `<LevelId>.extents`.
    pub extents: PathBuf,
    /// The map file's length.
    pub len: u64,
    /// The map file's SHA-256: what the guest is handed.
    pub file: Sha256,
}

/// A level of the store under a set.
#[derive(Debug, Clone)]
pub struct Level {
    /// The store's name for the level (lower case).
    pub name: String,
    pub id: LevelId,
    pub scope: MapScope,
    /// The SHA-256 of the map file the store's skeleton gives back.
    pub base: Sha256,
    /// The retail identity, when nothing is laid over the level.
    pub map_id: Option<MapId>,
    /// The image, when something is.
    pub image: Option<Image>,
    /// Packages applied, in load order, and those the level already holds.
    pub packages: Vec<String>,
    pub held: Vec<String>,
    /// The assets its id names by hash ([`level_assets`]): none for a
    /// level with packages over it.
    pub assets: Vec<(String, Sha256)>,
    /// What the build did, when this call built the image.
    pub report: Option<Report>,
}

impl Level {
    /// Whether this call built the image (else it was a hit, or there is
    /// none).
    pub fn built(&self) -> bool {
        self.report.is_some()
    }
}

/// The paths of the image of `id` under `images`.
pub fn image_paths(images: &Path, id: &LevelId) -> (PathBuf, PathBuf) {
    (
        images.join(format!("{id}.{IMAGE_EXT}")),
        images.join(format!("{id}.{IMAGE_EXTENTS_EXT}")),
    )
}

fn err(level: &str, e: impl std::fmt::Display) -> String {
    format!("level {level}: {e}")
}

/// Open the image of `id`, checking that its parts tile the file; `None`
/// when it is missing or does not read.
pub fn open_image(images: &Path, raw_root: &Path, id: &LevelId) -> Option<(Image, StoreFile)> {
    let (sp, xp) = image_paths(images, id);
    let ext = Extents::parse(&std::fs::read_to_string(&xp).ok()?).ok()?;
    let skel = Skeleton::decode(&std::fs::read(&sp).ok()?).ok()?;
    let file: Sha256 = ext.file.parse().ok()?;
    let len = ext.len;
    let f = StoreFile::from_parts(&id.to_string(), skel, ext, raw_root.to_path_buf()).ok()?;
    Some((
        Image {
            skeleton: sp,
            extents: xp,
            len,
            file,
        },
        f,
    ))
}

/// The [`LevelId`] of `level` over the base whose file is `base`, with
/// `packages` laid over it in load order and no asset named by hash: the
/// skeleton's SHA-256 stands for every asset the store gives the level,
/// and the packages' ids for what they add and change.
pub fn level_id(level: &str, base: Sha256, packages: &[Loaded]) -> LevelId {
    level_id_of(level, base, packages, Vec::new())
}

/// [`level_id`] with `assets` in the identity: (key as
/// `<group>:<path>`, asset hash).
pub fn level_id_of(
    level: &str,
    base: Sha256,
    packages: &[Loaded],
    assets: Vec<(String, Sha256)>,
) -> LevelId {
    LevelId::of(&LevelInputs {
        builders: BUILDERS_VERSION,
        level: level.to_ascii_lowercase(),
        skeleton: Some(base),
        assets,
        packages: packages.iter().map(|p| p.package.id).collect(),
    })
}

/// The assets a level with nothing over it reads, from the store's
/// catalogue ([`assemble`]): every tag of its manifest as
/// (`<group>:<path>`, asset hash), sorted by key. A store with no manifest
/// for the level (a level of ours, built whole) names none. A manifest
/// with no current catalogue is an error: the id would not be the one
/// another machine makes from the same store.
pub fn level_assets(
    store: &Path,
    stem: &str,
    catalogue: Option<&Catalogue>,
) -> Result<Vec<(String, Sha256)>, String> {
    if !store.join("maps").join(format!("{stem}.txt")).is_file() {
        return Ok(Vec::new());
    }
    let Some(c) = catalogue else {
        return Err(err(
            stem,
            "the store has no catalogue (assets.idx); h2 build or workspace.extract writes it",
        ));
    };
    if !c.is_current(store).unwrap_or(false) {
        return Err(err(
            stem,
            "the store's catalogue (assets.idx) is stale; h2 build or workspace.extract rewrites it",
        ));
    }
    c.of_map(stem)
        .into_iter()
        .map(|(k, h)| {
            let hash = h
                .parse()
                .map_err(|e| err(stem, format!("the asset hash of {k}: {e}")))?;
            Ok((
                format!("{}:{}", tags::defs::fourcc_text(k.group), k.path),
                hash,
            ))
        })
        .collect()
}

/// The image of the store's level `level` (its map stem) under `packages`
/// (the game's set, in load order), from the store at `store` with images
/// kept in `images`. See the module docs.
pub fn assemble(
    store: &Path,
    images: &Path,
    packages: &[Loaded],
    level: &str,
) -> Result<Level, String> {
    let catalogue = match tags::repo::catalogue::read(store) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("store: {}: assets.idx does not read: {e}", store.display());
            None
        }
    };
    assemble_with(store, images, packages, level, catalogue.as_ref())
}

/// [`assemble`] with the store's catalogue read once for its levels.
pub fn assemble_with(
    store: &Path,
    images: &Path,
    packages: &[Loaded],
    level: &str,
    catalogue: Option<&Catalogue>,
) -> Result<Level, String> {
    let name = level.to_ascii_lowercase();
    let stem = find_stem(store, &name).ok_or_else(|| {
        err(
            &name,
            format!("the store at {} has no skeleton for it", store.display()),
        )
    })?;
    let head = StoreFile::open(store, &stem).map_err(|e| err(&name, e))?;
    let base: Sha256 = head
        .file_hash()
        .parse()
        .map_err(|e| err(&name, format!("the extent table's file hash: {e}")))?;
    // The header, the index and the names are the skeleton's: reading them
    // opens no raw file.
    let map = Map::from_reader(head).map_err(|e| err(&name, format!("not a map: {e}")))?;
    let cache_type = map.header().cache_type;
    let base_memory = map.header().meta_size;
    let scope = scope_of(cache_type);
    let target = Target { name: &name, scope };
    let built_by_items: HashSet<String> = crate::maps::built_names(packages).into_iter().collect();
    let (applied, held) = applied_to(packages, &built_by_items, &target, &map);
    drop(map);
    let mut level = Level {
        name: name.clone(),
        id: level_id(&name, base, &applied),
        scope,
        base,
        map_id: None,
        image: None,
        packages: applied
            .iter()
            .map(|p| p.package.name().to_owned())
            .collect(),
        held,
        assets: Vec::new(),
        report: None,
    };
    if applied.is_empty() {
        level.assets = level_assets(store, &stem, catalogue)?;
        level.id = level_id_of(&name, base, &[], level.assets.clone());
        level.map_id = Some(MapId::retail(&name, base));
        return Ok(level);
    }
    let raw_root = store.join("raw");
    if let Some((image, _)) = open_image(images, &raw_root, &level.id) {
        touch(&image.extents);
        level.image = Some(image);
        return Ok(level);
    }
    let mut whole = Vec::new();
    StoreFile::open(store, &stem)
        .and_then(|mut f| f.read_to_end(&mut whole).map_err(Into::into))
        .map_err(|e| err(&name, e))?;
    let built = build_map_laid_out(&whole, &applied, &target).map_err(|e| err(&name, e))?;
    drop(whole);
    check_tag_memory(&built, cache_type, base_memory).map_err(|e| err(&name, e))?;
    let base_extents = StoreFile::extents(store, &stem).map_err(|e| err(&name, e))?;
    let mut put = |bytes: &[u8]| put_raw(&raw_root, bytes);
    let (skel, ext) =
        split(&built.bytes, &base_extents, &built.raw, &mut put).map_err(|e| err(&name, e))?;
    let image = write_image(images, &level.id, &skel, &ext).map_err(|e| err(&name, e))?;
    level.image = Some(image);
    level.report = Some(built.report);
    Ok(level)
}

/// The store's stem for `key` (lower case), matched without regard to case.
fn find_stem(store: &Path, key: &str) -> Option<String> {
    if StoreFile::exists(store, key) {
        return Some(key.to_owned());
    }
    std::fs::read_dir(store.join("maps"))
        .ok()?
        .flatten()
        .find_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let stem = name.strip_suffix(".extents")?;
            (stem.eq_ignore_ascii_case(key) && StoreFile::exists(store, stem))
                .then(|| stem.to_owned())
        })
}

/// Store `bytes` in `raw_root` by hash unless it is there; the hash (hex).
fn put_raw(raw_root: &Path, bytes: &[u8]) -> Result<String, String> {
    let hash = Sha256::of(bytes).to_string();
    let path = raw_root.join(&hash[..2]).join(&hash);
    if !path.is_file() {
        content::store::write_atomic(&path, bytes)
            .map_err(|e| format!("storing raw {hash}: {e}"))?;
    }
    Ok(hash)
}

/// Mark an image used now, for [`trim`].
fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// Refuse a built map whose tag memory is over the bound for `cache_type`,
/// or over `base_memory` (the base's own total, header `+0x1c`) when that
/// is larger, naming the tags the packages added and changed.
pub fn check_tag_memory(
    built: &BuiltMap,
    cache_type: CacheType,
    base_memory: u32,
) -> Result<(), String> {
    let retail = map::writer::max_metadata_size(cache_type).map_err(|e| e.to_string())?;
    let bound = retail.max(base_memory);
    let used = built.layout.memory_size;
    if used <= bound {
        return Ok(());
    }
    let mut named: Vec<String> = built.report.added.iter().map(|k| k.to_string()).collect();
    named.extend(
        built
            .report
            .changed
            .iter()
            .map(|k| format!("{k} (changed)")),
    );
    Err(format!(
        "tag memory {used:#x} is over the bound {bound:#x} by {:#x}; the packages added or changed: {}",
        used - bound,
        if named.is_empty() {
            "nothing named".to_owned()
        } else {
            named.join(", ")
        }
    ))
}

/// Split the built map `bytes` into an image: a skeleton and an extent
/// table that give it back byte for byte. A range that equals an extent of
/// `base` at the same offset (by its hash) is that extent; each range of
/// `placed` (the raw resources the build placed, as offset and length) is
/// stored through `put` and named by the hash it returns; where two
/// overlap, the one starting first (the longer at one offset) wins. Every
/// other byte is the skeleton's.
pub fn split(
    bytes: &[u8],
    base: &Extents,
    placed: &[(u64, u64)],
    put: &mut dyn FnMut(&[u8]) -> Result<String, String>,
) -> Result<(Skeleton, Extents), String> {
    let len = bytes.len() as u64;
    let slice = |o: u64, l: u64| &bytes[o as usize..(o + l) as usize];
    let fits = |o: u64, l: u64| l > 0 && o.checked_add(l).is_some_and(|e| e <= len);
    // (offset, end, hash or None for a placed range not stored yet)
    let mut cand: Vec<(u64, u64, Option<&str>)> = Vec::new();
    for e in &base.extents {
        if fits(e.offset, e.len) && Sha256::of(slice(e.offset, e.len)).to_string() == e.hash {
            cand.push((e.offset, e.offset + e.len, Some(e.hash.as_str())));
        }
    }
    for &(o, l) in placed {
        if fits(o, l) {
            cand.push((o, o + l, None));
        }
    }
    cand.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut skel = Skeleton {
        len,
        ..Default::default()
    };
    let mut ext = Extents {
        file: Sha256::of(bytes).to_string(),
        len,
        extents: Vec::new(),
    };
    let mut at = 0u64;
    let gap = |skel: &mut Skeleton, from: u64, to: u64| {
        if to > from {
            skel.ranges.push((from, to - from));
            skel.bytes.extend_from_slice(slice(from, to - from));
        }
    };
    for (o, e, hash) in cand {
        if o < at {
            continue;
        }
        let hash = match hash {
            Some(h) => h.to_owned(),
            None => put(slice(o, e - o))?,
        };
        gap(&mut skel, at, o);
        ext.extents.push(Extent {
            offset: o,
            len: e - o,
            hash,
        });
        at = e;
    }
    gap(&mut skel, at, len);
    Ok((skel, ext))
}

/// Write the image of `id`: the skeleton, then the extent table that
/// vouches for it.
pub fn write_image(
    images: &Path,
    id: &LevelId,
    skel: &Skeleton,
    ext: &Extents,
) -> Result<Image, String> {
    let (sp, xp) = image_paths(images, id);
    let encoded = skel.encode().map_err(|e| e.to_string())?;
    content::store::write_atomic(&sp, &encoded)
        .map_err(|e| format!("writing {}: {e}", sp.display()))?;
    content::store::write_atomic(&xp, ext.to_text().as_bytes())
        .map_err(|e| format!("writing {}: {e}", xp.display()))?;
    Ok(Image {
        skeleton: sp,
        extents: xp,
        len: ext.len,
        file: ext.file.parse().map_err(|e| format!("{e}"))?,
    })
}

/// One image in the folder, as [`images`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageStatus {
    pub id: LevelId,
    /// The map file's length and SHA-256.
    pub len: u64,
    pub file: String,
    /// Bytes on disk: the skeleton file and the extent table.
    pub disk: u64,
    /// Extents, and the bytes they cover.
    pub extents: (usize, u64),
}

/// Every image under `images` with an extent table that reads, by id.
pub fn images(images: &Path) -> Vec<ImageStatus> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(images).into_iter().flatten().flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(id) = name
            .strip_suffix(&format!(".{IMAGE_EXTENTS_EXT}"))
            .and_then(|s| s.parse::<LevelId>().ok())
        else {
            continue;
        };
        let (sp, xp) = image_paths(images, &id);
        let Some(x) = std::fs::read_to_string(&xp)
            .ok()
            .and_then(|t| Extents::parse(&t).ok())
        else {
            continue;
        };
        let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
        out.push(ImageStatus {
            id,
            len: x.len,
            file: x.file.clone(),
            disk: size(&sp) + size(&xp),
            extents: (x.extents.len(), x.raw_bytes()),
        });
    }
    out.sort_by_key(|s| s.id);
    out
}

/// Delete images under `images`, least recently used first (by the extent
/// table's modification time, which a hit renews), until their files take
/// at most `cap` bytes. An image in `keep` is never deleted. Returns the
/// ids deleted. Raw files are the store's and stay.
pub fn trim(images: &Path, cap: u64, keep: &BTreeSet<LevelId>) -> Vec<LevelId> {
    let mut all: Vec<(std::time::SystemTime, LevelId, u64)> = Vec::new();
    let mut total = 0u64;
    for s in self::images(images) {
        let (_, xp) = image_paths(images, &s.id);
        let when = std::fs::metadata(&xp)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        total += s.disk;
        all.push((when, s.id, s.disk));
    }
    all.sort();
    let mut gone = Vec::new();
    for (_, id, disk) in all {
        if total <= cap {
            break;
        }
        if keep.contains(&id) {
            continue;
        }
        let (sp, xp) = image_paths(images, &id);
        // The table first: without it the image is not used.
        if std::fs::remove_file(&xp).is_ok() {
            let _ = std::fs::remove_file(&sp);
            total = total.saturating_sub(disk);
            gone.push(id);
        }
    }
    gone
}

/// The game's levels under its set, for `vfs::store_map`: each map the
/// game opens is [`assemble`]d once per boot, recorded in the
/// [`Served`] map `packages.list` reads. A level that does not assemble is
/// served as the store has it, named once on stderr and in its record.
/// The store's catalogue is read once, at the first level. After a build
/// the folder is [`trim`]med to [`DEFAULT_IMAGE_CAP`], sparing the images
/// this boot serves.
pub struct StoreLevels {
    store: PathBuf,
    images: PathBuf,
    packages: Vec<Loaded>,
    served: Served,
    done: BTreeMap<String, Option<vfs::store_map::LevelImage>>,
    /// The store's catalogue: not read yet, read and absent, or read.
    catalogue: Option<Option<Catalogue>>,
}

impl StoreLevels {
    /// The levels of the store at `store` under `packages` (load order),
    /// images under `images`, recorded in `served`.
    pub fn new(store: PathBuf, images: PathBuf, packages: Vec<Loaded>, served: Served) -> Self {
        Self {
            store,
            images,
            packages,
            served,
            done: BTreeMap::new(),
            catalogue: None,
        }
    }

    /// Shared, as `vfs::Config::store_levels` takes it.
    pub fn shared(self) -> vfs::store_map::SharedLevels {
        vfs::store_map::SharedLevels(Arc::new(Mutex::new(self)))
    }
}

impl vfs::store_map::Levels for StoreLevels {
    fn image(&mut self, stem: &str) -> Result<Option<vfs::store_map::LevelImage>, String> {
        let key = stem.to_ascii_lowercase();
        if let Some(done) = self.done.get(&key) {
            return Ok(done.clone());
        }
        if self.catalogue.is_none() {
            self.catalogue = Some(match tags::repo::catalogue::read(&self.store) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "store: {}: assets.idx does not read: {e}",
                        self.store.display()
                    );
                    None
                }
            });
        }
        let result = assemble_with(
            &self.store,
            &self.images,
            &self.packages,
            &key,
            self.catalogue.as_ref().and_then(|c| c.as_ref()),
        );
        let mut record = ServedMap::default();
        let out = match result {
            Ok(level) => {
                record.scope = level.scope.word().to_owned();
                record.packages = level.packages.clone();
                record.held = level.held.clone();
                if level.image.is_some() {
                    record.key = level.id.to_string();
                }
                let built = level.report.is_some();
                if let Some(r) = &level.report {
                    record.builds = 1;
                    record.added = r.added.len();
                    record.changed = r.changed.len();
                } else if level.image.is_some() {
                    record.hits = 1;
                }
                let image = level.image.map(|i| vfs::store_map::LevelImage {
                    id: level.id.to_string(),
                    skeleton: i.skeleton,
                    extents: i.extents,
                });
                merge(&self.served, &key, record);
                if built {
                    let mut keep: BTreeSet<LevelId> = self
                        .done
                        .values()
                        .flatten()
                        .filter_map(|i| i.id.parse().ok())
                        .collect();
                    keep.insert(level.id);
                    trim(&self.images, DEFAULT_IMAGE_CAP, &keep);
                }
                Ok(image)
            }
            Err(why) => {
                record.error = Some(why.clone());
                merge(&self.served, &key, record);
                Err(why)
            }
        };
        let kept = out.clone().unwrap_or(None);
        self.done.insert(key, kept);
        out
    }
}
