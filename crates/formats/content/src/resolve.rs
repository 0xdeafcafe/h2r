//! The packages under `<data>/maps/`, and what each map name resolves to.
//!
//! ```text
//! <data>/maps/
//!   active.toml              optional: which package serves which map
//!   lockout-snipers/
//!     manifest.toml
//!     snipers.ppf
//!   cache/<MapId>.map        materialised maps (BPS), least recently used evicted
//! ```
//!
//! **Choice rule.** One package at most serves a map name:
//!
//! 1. `active.toml` decides when it names the map: `lockout = "dir"` picks
//!    the package in `<data>/maps/dir`, `lockout = false` the disc's map.
//!    If the named package is missing or broken, the disc's map is served
//!    (with a notice), never another package.
//! 2. Otherwise, of the valid packages whose `[base] map` is the name, the
//!    one whose directory name sorts last (byte order) wins, so dated or
//!    numbered directories pick the newest. A notice says when there was
//!    more than one. Forge packages require explicit selection.
//!
//! Map names match ASCII case-insensitively, as on the disc.
//!
//! **Resolution.** File layers are read and their SHA-256 checked, then:
//!
//! - If any layer is BPS, the whole result is built in memory once and
//!   written atomically to `cache/<MapId>.map`, then served from there. The
//!   cache is kept under a size cap, evicting least recently used (by
//!   modification time, refreshed on each use).
//! - Otherwise the result is an [`Overlay`] over the base: the disc's map,
//!   or the last whole `.map` layer. No copy is made.
//!
//! The base is read once to check its SHA-256 against the manifest (for a
//! whole `.map` layer, the layer's hash), and the header checksum at
//! +0x2d0 is recomputed over the patched bytes and written as the last
//! layer, as `map::writer` does. Content packs are checked and listed, not
//! applied: the feature engine applies them in memory after the load.
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use crate::{
    Hasher, Manifest, ManifestError, MapId, Sha256,
    manifest::{MANIFEST_FILE, relative},
    patch::{FileSource, Overlay, OverlayView, Patch, PatchError, Source},
};

/// The materialised-map cache, under the maps directory.
pub const CACHE_DIR: &str = "cache";
/// The file that picks a package per map name.
pub const ACTIVE_FILE: &str = "active.toml";
/// The cache's default size cap.
pub const DEFAULT_CACHE_CAP: u64 = 4 << 30;

/// Header checksum position and the start of the bytes it covers.
const CHECKSUM_AT: u64 = 0x2d0;
const HEADER_LEN: u64 = 0x800;
const CHUNK: usize = 1 << 20;

/// A valid package.
#[derive(Debug, Clone)]
pub struct Package {
    /// Its directory, canonical.
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub id: MapId,
    /// What the map list keys this package by: [`Manifest::variants_key`]
    /// for a package with variants (one list ID whichever is loaded), else
    /// [`Self::id`].
    pub list_key: MapId,
}

impl Package {
    /// The directory's own name.
    pub fn dir_name(&self) -> &str {
        self.dir.file_name().and_then(|n| n.to_str()).unwrap_or("")
    }
}

/// What a resolved map's bytes are laid over.
#[derive(Debug)]
pub enum BaseRef {
    /// The disc's map of the same name.
    Disc,
    /// A whole map file in the package (opened while resolving).
    File { path: PathBuf, file: File },
}

/// Where a resolved map's bytes come from.
#[derive(Debug)]
pub enum Data {
    /// The base with an overlay laid over it as it is read.
    Overlay { base: BaseRef, overlay: Overlay },
    /// A complete materialised map file.
    File { path: PathBuf, file: File, len: u64 },
}

/// A map name resolved through its package.
#[derive(Debug)]
pub struct Resolved {
    pub id: MapId,
    /// The package directory.
    pub package: PathBuf,
    pub data: Data,
    /// Content packs, in order, for the feature engine.
    pub packs: Vec<PathBuf>,
}

impl Resolved {
    pub fn len(&self) -> u64 {
        match &self.data {
            Data::Overlay { overlay, .. } => overlay.len(),
            Data::File { len, .. } => *len,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The whole resolved map in memory, reading the disc's map from `disc`
    /// for an overlay on it (the editor opens a package this way).
    pub fn read_all(&self, disc: &mut dyn Source) -> io::Result<Vec<u8>> {
        match &self.data {
            Data::Overlay { base, overlay } => match base {
                BaseRef::Disc => OverlayView {
                    overlay,
                    base: disc,
                }
                .read_all(),
                BaseRef::File { file, .. } => OverlayView {
                    overlay,
                    base: &mut FileSource::new(file.try_clone()?)?,
                }
                .read_all(),
            },
            Data::File { file, .. } => FileSource::new(file.try_clone()?)?.read_all(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("{0}")]
    Manifest(#[from] ManifestError),
    #[error("{0}")]
    Patch(#[from] PatchError),
    #[error("{file}: {why}")]
    File { file: String, why: String },
    #[error("{file}: SHA-256 differs from the manifest's")]
    Hash { file: String },
    #[error("the {what} does not match the package's base (SHA-256 differs)")]
    Base { what: String },
    #[error("Forge document {file}: {why}")]
    ForgeDocument { file: String, why: String },
    #[error("forge_expect_map_id: document expects {expected}, computed {actual}")]
    ForgeExpectedMapId { expected: MapId, actual: MapId },
    #[error("{0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Active {
    Disc,
    Package(String),
}

/// SHA-256 of the disc's own maps, by lowercase map name, shared between
/// whoever hashes them: the resolver records each disc base it reads in
/// full while checking a package, and live-hle's `Bases` (the boot's
/// `DiscBases`) asks before reading the map again. Clones share one table.
#[derive(Debug, Clone, Default)]
pub struct DiscHashes(Arc<Mutex<HashMap<String, Sha256>>>);

impl DiscHashes {
    /// The recorded hash of the disc's `map`, if any.
    pub fn get(&self, map: &str) -> Option<Sha256> {
        self.lock().get(&map.to_ascii_lowercase()).copied()
    }
    pub fn insert(&self, map: &str, hash: Sha256) {
        self.lock().insert(map.to_ascii_lowercase(), hash);
    }
    /// Every recorded hash, by lowercase name.
    pub fn snapshot(&self) -> HashMap<String, Sha256> {
        self.lock().clone()
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Sha256>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The packages under one maps directory.
#[derive(Debug)]
pub struct Library {
    root: PathBuf,
    packages: Vec<Package>,
    active: HashMap<String, Active>,
    cache_cap: u64,
    notices: Vec<String>,
    disc_hashes: Option<DiscHashes>,
    /// The sandbox whose variant each converted map loads, kept across
    /// [`Self::reopen`].
    sandbox: Option<String>,
}

impl Library {
    /// Scan `root` (normally `<data>/maps`). A missing directory is an
    /// empty library; packages that cannot be read are skipped with a
    /// notice.
    pub fn open(root: impl AsRef<Path>) -> Self {
        Self::open_with_sandbox(root, None)
    }

    /// [`Self::open`], loading `sandbox`'s variant (`h1` or `h2`) of each
    /// package that has variants ([`Manifest::select_variant`]; the
    /// default's when it has none of that sandbox). `boot::new_maps` passes
    /// the `multiplayer.halo1_sandbox` setting (docs/halo1/multiplayer.md).
    pub fn open_with_sandbox(root: impl AsRef<Path>, sandbox: Option<&str>) -> Self {
        let root = root.as_ref();
        let mut lib = Self {
            root: root.to_path_buf(),
            packages: Vec::new(),
            active: HashMap::new(),
            cache_cap: DEFAULT_CACHE_CAP,
            notices: Vec::new(),
            disc_hashes: None,
            sandbox: sandbox.map(str::to_owned),
        };
        let Ok(entries) = std::fs::read_dir(root) else {
            return lib;
        };
        if let Ok(root) = root.canonicalize() {
            lib.root = root;
        }
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                !name.starts_with('.') && name != CACHE_DIR
            })
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            match load(&dir, sandbox) {
                Ok(p) => lib.packages.push(p),
                Err(e) => lib
                    .notices
                    .push(format!("warning: skipping package {}: {e}", dir.display())),
            }
        }
        lib.read_active();
        lib
    }

    fn read_active(&mut self) {
        let path = self.root.join(ACTIVE_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => {
                self.notices
                    .push(format!("warning: cannot read {}: {e}", path.display()));
                return;
            }
        };
        let table: toml::Table = match toml::from_str(&text) {
            Ok(t) => t,
            Err(e) => {
                self.notices
                    .push(format!("warning: ignoring {}: {e}", path.display()));
                return;
            }
        };
        for (map, value) in table {
            let choice = match value {
                toml::Value::String(dir) => Active::Package(dir),
                toml::Value::Boolean(false) => Active::Disc,
                other => {
                    self.notices.push(format!(
                        "warning: {}: {map} = {other} (want a package directory name, or false for the disc)",
                        path.display()
                    ));
                    continue;
                }
            };
            self.active.insert(map.to_ascii_lowercase(), choice);
        }
    }

    /// Change the cache's size cap.
    pub fn with_cache_cap(mut self, cap: u64) -> Self {
        self.cache_cap = cap;
        self
    }

    /// Record the hash of each disc base read in full while resolving
    /// (the overlay's one pass, a materialisation) in `hashes`.
    pub fn with_disc_hashes(mut self, hashes: DiscHashes) -> Self {
        self.disc_hashes = Some(hashes);
        self
    }

    /// Scan the same directory again, keeping the cache cap and the shared
    /// disc hashes: what the folder holds now.
    pub fn reopen(&self) -> Self {
        let mut lib = Self::open_with_sandbox(&self.root, self.sandbox.as_deref())
            .with_cache_cap(self.cache_cap);
        lib.disc_hashes = self.disc_hashes.clone();
        lib
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }
    /// No packages, and nothing in `active.toml`: nothing to serve.
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
    /// Notices gathered so far (skipped packages, choices), drained.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// The package that serves `map`, by the choice rule (module docs).
    pub fn choose(&mut self, map: &str) -> Option<&Package> {
        let key = map.to_ascii_lowercase();
        match self.active.get(&key) {
            Some(Active::Disc) => return None,
            Some(Active::Package(dir)) => {
                let found = self.packages.iter().position(|p| p.dir_name() == dir);
                return match found {
                    Some(i) if self.packages[i].manifest.base.map.eq_ignore_ascii_case(map) => {
                        Some(&self.packages[i])
                    }
                    Some(_) => {
                        self.notices.push(format!(
                            "warning: {ACTIVE_FILE} picks {dir} for {map}, but it is a package for another map; using the disc's"
                        ));
                        None
                    }
                    None => {
                        self.notices.push(format!(
                            "warning: {ACTIVE_FILE} picks {dir} for {map}, which is not a valid package; using the disc's"
                        ));
                        None
                    }
                };
            }
            None => {}
        }
        let candidates: Vec<usize> = (0..self.packages.len())
            .filter(|&i| {
                let manifest = &self.packages[i].manifest;
                manifest.forge.is_empty() && manifest.base.map.eq_ignore_ascii_case(map)
            })
            .collect();
        // Sorted by directory when scanned: the last is the newest by name.
        let &chosen = candidates.last()?;
        if candidates.len() > 1 {
            let others: Vec<&str> = candidates[..candidates.len() - 1]
                .iter()
                .map(|&i| self.packages[i].dir_name())
                .collect();
            self.notices.push(format!(
                "{map}: {} packages; using {} (last by name; set one in {ACTIVE_FILE}), not {}",
                candidates.len(),
                self.packages[chosen].dir_name(),
                others.join(", ")
            ));
        }
        Some(&self.packages[chosen])
    }

    /// Resolve `map` (a name like `lockout`), reading the disc's map from
    /// `disc` when the package is laid over it. `Ok(None)` when no package
    /// serves the name.
    pub fn resolve(
        &mut self,
        map: &str,
        disc: &mut dyn Source,
    ) -> Result<Option<Resolved>, ResolveError> {
        let cap = self.cache_cap;
        let cache = self.root.join(CACHE_DIR);
        let Some(package) = self.choose(map).cloned() else {
            return Ok(None);
        };
        resolve_package(&package, disc, &cache, cap, self.disc_hashes.as_ref()).map(Some)
    }

    /// Resolve one package of this library, whatever the choice rule picks
    /// for its map (the editor opens packages this way).
    pub fn resolve_package(
        &self,
        package: &Package,
        disc: &mut dyn Source,
    ) -> Result<Resolved, ResolveError> {
        resolve_package(
            package,
            disc,
            &self.root.join(CACHE_DIR),
            self.cache_cap,
            self.disc_hashes.as_ref(),
        )
    }
}

fn load(dir: &Path, sandbox: Option<&str>) -> Result<Package, ResolveError> {
    let dir = dir.canonicalize()?;
    let text =
        std::fs::read_to_string(dir.join(MANIFEST_FILE)).map_err(|e| ResolveError::File {
            file: MANIFEST_FILE.into(),
            why: e.to_string(),
        })?;
    // A converted map with one whole map per sandbox: load the chosen
    // variant, else the default (docs/halo1/multiplayer.md).
    let full = Manifest::from_toml(&text)?;
    let manifest = full.select_variant(sandbox);
    manifest.validate_in(&dir)?;
    let id = manifest.map_id();
    let list_key = full.variants_key().unwrap_or(id);
    Ok(Package {
        dir,
        manifest,
        id,
        list_key,
    })
}

/// `file` in package `dir`, opened, after checking it is a regular file
/// that stays inside the directory once links are followed.
fn open_in(dir: &Path, file: &str) -> Result<(PathBuf, File), ResolveError> {
    let bad = |why: String| ResolveError::File {
        file: file.into(),
        why,
    };
    let path = dir.join(relative(file)?);
    let real = path.canonicalize().map_err(|e| bad(e.to_string()))?;
    if !real.starts_with(dir) {
        return Err(bad("leaves the package directory".into()));
    }
    let f = File::open(&real).map_err(|e| bad(e.to_string()))?;
    if !f.metadata()?.is_file() {
        return Err(bad("not a regular file".into()));
    }
    Ok((real, f))
}

enum Loaded {
    Patch(Patch),
    /// A whole map file, hashed when it is streamed.
    Whole(PathBuf, File, Sha256),
}

fn load_layer(dir: &Path, layer: &crate::manifest::Layer) -> Result<Loaded, ResolveError> {
    let (path, mut file) = open_in(dir, &layer.file)?;
    let mut head = [0; 5];
    let n = read_up_to(&mut file, &mut head)?;
    if Patch::recognises(&head[..n]) {
        let bytes = std::fs::read(&path)?;
        if Sha256::of(&bytes) != layer.sha256 {
            return Err(ResolveError::Hash {
                file: layer.file.clone(),
            });
        }
        return Ok(Loaded::Patch(Patch::parse(&bytes)?));
    }
    if layer.file.to_ascii_lowercase().ends_with(".map") {
        let file = File::open(&path)?;
        return Ok(Loaded::Whole(path, file, layer.sha256));
    }
    Err(PatchError::Unknown(format!(
        " in {} (PPF, IPS, BPS, or a whole .map file)",
        layer.file
    ))
    .into())
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    use std::io::Read;
    let mut at = 0;
    while at < buf.len() {
        match file.read(&mut buf[at..])? {
            0 => break,
            n => at += n,
        }
    }
    Ok(at)
}

/// Reads through to a source, hashing the bytes read in order from 0.
struct Tee<'a> {
    src: &'a mut dyn Source,
    hash: Hasher,
    next: u64,
}
impl Source for Tee<'_> {
    fn size(&self) -> u64 {
        self.src.size()
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.src.read_at(offset, buf)?;
        if offset == self.next {
            self.hash.update(&buf[..n]);
            self.next += n as u64;
        }
        Ok(n)
    }
}
impl Tee<'_> {
    /// Hash whatever was not read, and finish.
    fn finish(mut self) -> io::Result<Sha256> {
        let mut buf = vec![0; CHUNK];
        while self.next < self.src.size() {
            let n = (self.src.size() - self.next).min(CHUNK as u64) as usize;
            let at = self.next;
            self.read_exact_at(at, &mut buf[..n])?;
        }
        Ok(self.hash.finish())
    }
}

/// Recompute the header checksum of a map held in `bytes`, if it looks like
/// one (`daeh` at 0 and at least a header long). Returns whether it did.
pub fn repair_checksum(bytes: &mut [u8]) -> bool {
    if !is_map_header(bytes) {
        return false;
    }
    let sum = map::checksum::engine_xor(&bytes[HEADER_LEN as usize..]);
    bytes[CHECKSUM_AT as usize..CHECKSUM_AT as usize + 4].copy_from_slice(&sum.to_le_bytes());
    true
}

fn is_map_header(bytes: &[u8]) -> bool {
    bytes.len() as u64 >= HEADER_LEN && bytes.starts_with(b"daeh")
}

fn resolve_package(
    package: &Package,
    disc: &mut dyn Source,
    cache: &Path,
    cap: u64,
    hashes: Option<&DiscHashes>,
) -> Result<Resolved, ResolveError> {
    let m = &package.manifest;
    if !m.forge.is_empty() || m.package.format == crate::manifest::FORGE_FORMAT {
        m.validate()?;
        return resolve_forge(package, disc, hashes);
    }
    let mut layers = Vec::new();
    for l in &m.patches {
        layers.push(load_layer(&package.dir, l)?);
    }
    let mut packs = Vec::new();
    for l in &m.packs {
        let (path, _) = open_in(&package.dir, &l.file)?;
        if Sha256::of(&std::fs::read(&path)?) != l.sha256 {
            return Err(ResolveError::Hash {
                file: l.file.clone(),
            });
        }
        packs.push(path);
    }
    // A whole map replaces everything before it.
    let start = layers
        .iter()
        .rposition(|l| matches!(l, Loaded::Whole(..)))
        .map(|i| i + 1);
    if m.base.derived && !matches!(layers.first(), Some(Loaded::Whole(..))) {
        return Err(ResolveError::File {
            file: m.patches[0].file.clone(),
            why: "a derived map's first layer must be a whole .map file".into(),
        });
    }
    let (base, mut base_src, want, what) = match start {
        Some(i) => {
            let Loaded::Whole(path, file, sha) = &layers[i - 1] else {
                unreachable!()
            };
            let src = FileSource::new(file.try_clone()?)?;
            let base = BaseRef::File {
                path: path.clone(),
                file: file.try_clone()?,
            };
            (base, Some(src), *sha, m.patches[i - 1].file.clone())
        }
        None => {
            let want = m.base.sha256.expect("validated: a disc base has a hash");
            (
                BaseRef::Disc,
                None,
                want,
                format!("disc's {}.map", m.base.map),
            )
        }
    };
    let base_src: &mut dyn Source = match &mut base_src {
        Some(s) => s,
        None => disc,
    };
    let patches: Vec<&Patch> = layers[start.unwrap_or(0)..]
        .iter()
        .map(|l| match l {
            Loaded::Patch(p) => p,
            Loaded::Whole(..) => unreachable!("after the last whole layer"),
        })
        .collect();
    if patches.iter().any(|p| matches!(p, Patch::Bps(_))) {
        let path = cache.join(format!("{}.map", package.id));
        if let Ok(file) = File::open(&path) {
            // The cache key includes the expected base hash. Check the
            // actual base too, in case a different disc or Map Pack is mounted.
            let hash = Tee {
                src: base_src,
                hash: Hasher::default(),
                next: 0,
            }
            .finish()?;
            if hash != want {
                return Err(ResolveError::Base { what });
            }
            let len = file.metadata()?.len();
            touch(&path);
            return Ok(Resolved {
                id: package.id,
                package: package.dir.clone(),
                data: Data::File { path, file, len },
                packs,
            });
        }
        let bytes = base_src.read_all()?;
        let hash = Sha256::of(&bytes);
        if start.is_none()
            && let Some(h) = hashes
        {
            h.insert(&m.base.map, hash);
        }
        if hash != want {
            return Err(ResolveError::Base { what });
        }
        let mut bytes = materialise(bytes, &patches)?;
        repair_checksum(&mut bytes);
        let file = store(cache, &path, &bytes, cap)?;
        return Ok(Resolved {
            id: package.id,
            package: package.dir.clone(),
            data: Data::File {
                path,
                file,
                len: bytes.len() as u64,
            },
            packs,
        });
    }
    let mut overlay = Overlay::new(base_src.size());
    for p in &patches {
        match p {
            Patch::Ppf(ppf) => {
                ppf.check(&mut OverlayView {
                    overlay: &overlay,
                    base: &mut *base_src,
                })?;
                overlay.apply(&ppf.patch)?;
            }
            Patch::Ips(ips) => overlay.apply(ips)?,
            Patch::Bps(_) => unreachable!("materialised above"),
        }
    }
    // One pass over the patched stream: the checksum, and (through the
    // tee) the base's hash.
    let mut tee = Tee {
        src: base_src,
        hash: Hasher::default(),
        next: 0,
    };
    let mut sum = map::checksum::XorChecksum::default();
    let mut head = [0; 4];
    let mut buf = vec![0; CHUNK];
    let mut at = 0;
    while at < overlay.len() {
        let n = overlay.read_at(&mut tee, at, &mut buf)?;
        if at == 0 {
            let k = n.min(4);
            head[..k].copy_from_slice(&buf[..k]);
        }
        let skip = HEADER_LEN.saturating_sub(at).min(n as u64) as usize;
        sum.update(&buf[skip..n]);
        at += n as u64;
    }
    let hash = tee.finish()?;
    if start.is_none()
        && let Some(h) = hashes
    {
        h.insert(&m.base.map, hash);
    }
    if hash != want {
        return Err(ResolveError::Base { what });
    }
    if overlay.len() >= HEADER_LEN && &head == b"daeh" {
        overlay.write(CHECKSUM_AT, &sum.finish().to_le_bytes());
    }
    Ok(Resolved {
        id: package.id,
        package: package.dir.clone(),
        data: Data::Overlay { base, overlay },
        packs,
    })
}

fn resolve_forge(
    package: &Package,
    disc: &mut dyn Source,
    hashes: Option<&DiscHashes>,
) -> Result<Resolved, ResolveError> {
    use std::io::Read as _;

    let m = &package.manifest;
    let [layer] = m.forge.as_slice() else {
        return Err(ManifestError::Forge("requires one layer").into());
    };
    let want = m.base.sha256.ok_or(ManifestError::BaseHash)?;
    let (_, doc) = open_in(&package.dir, &layer.document_file)?;
    let mut bytes = Vec::new();
    doc.take(crate::forge::MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let header = crate::forge::read_header(&bytes).map_err(|why| ResolveError::ForgeDocument {
        file: layer.document_file.clone(),
        why,
    })?;
    if header.base.map != m.base.map || header.base.sha256 != want || header.id != layer.document {
        return Err(ResolveError::ForgeDocument {
            file: layer.document_file.clone(),
            why: "document hash or base differs from the manifest".into(),
        });
    }
    let id = MapId::forge(want, header.id);
    if let Some(expected) = header.expect
        && expected.map_id != id
    {
        return Err(ResolveError::ForgeExpectedMapId {
            expected: expected.map_id,
            actual: id,
        });
    }
    if id != package.id {
        return Err(ResolveError::ForgeDocument {
            file: layer.document_file.clone(),
            why: "map ID differs from the loaded manifest".into(),
        });
    }
    if let Some(expected) = header.expect
        && m.result.as_ref().and_then(|r| r.geometry) != Some(expected.geometry)
    {
        return Err(ResolveError::ForgeDocument {
            file: layer.document_file.clone(),
            why: "expected geometry differs from the build's result".into(),
        });
    }

    let hash = Tee {
        src: disc,
        hash: Hasher::default(),
        next: 0,
    }
    .finish()?;
    if let Some(h) = hashes {
        h.insert(&m.base.map, hash);
    }
    if hash != want {
        return Err(ResolveError::Base {
            what: format!("disc's {}.map", m.base.map),
        });
    }
    let (path, file) = open_in(&package.dir, &layer.file)?;
    let len = file.metadata()?.len();
    let mut source = FileSource::new(file.try_clone()?)?;
    let hash = Tee {
        src: &mut source,
        hash: Hasher::default(),
        next: 0,
    }
    .finish()?;
    if hash != layer.sha256 {
        return Err(ResolveError::Hash {
            file: layer.file.clone(),
        });
    }
    Ok(Resolved {
        id,
        package: package.dir.clone(),
        data: Data::File { path, file, len },
        packs: Vec::new(),
    })
}

/// Apply `patches` in order to the whole base.
fn materialise(mut bytes: Vec<u8>, patches: &[&Patch]) -> Result<Vec<u8>, PatchError> {
    for p in patches {
        bytes = match p {
            Patch::Bps(b) => b.apply(&bytes)?,
            Patch::Ppf(ppf) => {
                ppf.check(&mut &bytes[..])?;
                let mut o = Overlay::new(bytes.len() as u64);
                o.apply(&ppf.patch)?;
                o.materialise(bytes)
            }
            Patch::Ips(ips) => {
                let mut o = Overlay::new(bytes.len() as u64);
                o.apply(ips)?;
                o.materialise(bytes)
            }
        };
    }
    Ok(bytes)
}

fn touch(path: &Path) {
    if let Ok(f) = File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Write `bytes` to `path` atomically (a temporary file, then a rename),
/// then evict least recently used maps until the cache is within `cap`.
fn store(cache: &Path, path: &Path, bytes: &[u8], cap: u64) -> io::Result<File> {
    std::fs::create_dir_all(cache)?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = cache.join(format!(
        ".{}.{n}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("map"),
        std::process::id()
    ));
    let result = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    evict(cache, path, cap)?;
    File::open(path)
}

/// Remove the least recently used `*.map` files in `cache`, sparing `keep`,
/// until their total size is at most `cap`.
fn evict(cache: &Path, keep: &Path, cap: u64) -> io::Result<()> {
    let mut maps = Vec::new();
    for e in std::fs::read_dir(cache)?.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "map")
            && let Ok(m) = e.metadata()
            && m.is_file()
        {
            maps.push((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), p));
        }
    }
    let mut total: u64 = maps.iter().map(|m| m.1).sum();
    maps.sort();
    for (_, len, p) in maps {
        if total <= cap {
            break;
        }
        if p == keep {
            continue;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::manifest::Layer;
    use crate::patch::{bps, ips, ppf, write};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) struct Temp(pub PathBuf);
    impl Temp {
        pub(crate) fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "h2-content-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An invented map: a 0x800 header (`daeh`, the checksum at +0x2d0)
    /// then `payload`.
    pub(crate) fn synthetic_map(payload: &[u8]) -> Vec<u8> {
        let mut b = vec![0; 0x800];
        b[..4].copy_from_slice(b"daeh");
        b.extend_from_slice(payload);
        assert!(repair_checksum(&mut b));
        b
    }
    fn payload(n: usize, seed: u64) -> Vec<u8> {
        crate::testutil::Rng::new(seed).bytes(n)
    }

    /// Write a package: `files` beside a manifest for `map` over `base`.
    pub(crate) fn package(
        maps: &Path,
        dir: &str,
        map: &str,
        base: Option<&[u8]>,
        files: &[(&str, &[u8])],
    ) -> Manifest {
        let d = maps.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        let mut m = match base {
            Some(b) => Manifest::retail(map, Sha256::of(b)),
            None => {
                let mut m = Manifest::retail(map, Sha256([0; 32]));
                m.base.sha256 = None;
                m.base.derived = true;
                m
            }
        };
        m.package.name = format!("{dir} package");
        for (name, bytes) in files {
            std::fs::write(d.join(name), bytes).unwrap();
            let layer = Layer {
                file: (*name).into(),
                sha256: Sha256::of(bytes),
            };
            if name.ends_with(".toml") {
                m.packs.push(layer);
            } else {
                m.patches.push(layer);
            }
        }
        std::fs::write(d.join(MANIFEST_FILE), m.to_toml()).unwrap();
        m
    }

    fn read(r: &Resolved, disc: &[u8]) -> Vec<u8> {
        match &r.data {
            Data::Overlay { base, overlay } => {
                let mut v = match base {
                    BaseRef::Disc => OverlayView {
                        overlay,
                        base: &mut &disc[..],
                    }
                    .read_all(),
                    BaseRef::File { file, .. } => {
                        let mut src = FileSource::new(file.try_clone().unwrap()).unwrap();
                        OverlayView {
                            overlay,
                            base: &mut src,
                        }
                        .read_all()
                    }
                }
                .unwrap();
                v.shrink_to_fit();
                v
            }
            Data::File { path, .. } => std::fs::read(path).unwrap(),
        }
    }

    fn checksum_ok(bytes: &[u8]) -> bool {
        let mut b = bytes.to_vec();
        repair_checksum(&mut b);
        b == bytes
    }

    #[test]
    fn offset_patches_overlay_the_disc_and_the_checksum_is_repaired() {
        let t = Temp::new();
        let disc = synthetic_map(&payload(0xa000, 1));
        let ppf = ppf::tests::build(
            3,
            &disc,
            &[(0x900, b"PPF!"), (0x2d0, b"xx")],
            true,
            true,
            Some("d"),
        );
        let ips = ips::tests::build(
            &[
                ips::tests::Rec::Data(0x902, b"ips"),
                ips::tests::Rec::Rle(0x1000, 300, 5),
            ],
            None,
        );
        let m = package(
            &t.0,
            "lockout-snipers",
            "lockout",
            Some(&disc),
            &[("a.ppf", &ppf), ("b.ips", &ips), ("spawns.toml", b"x=1")],
        );
        let mut lib = Library::open(&t.0);
        assert_eq!(lib.packages().len(), 1);
        let r = lib.resolve("Lockout", &mut &disc[..]).unwrap().unwrap();
        assert_eq!(r.id, m.map_id());
        assert!(matches!(
            r.data,
            Data::Overlay {
                base: BaseRef::Disc,
                ..
            }
        ));
        assert_eq!(
            r.packs,
            vec![
                t.0.canonicalize()
                    .unwrap()
                    .join("lockout-snipers/spawns.toml")
            ]
        );
        let mut want = disc.clone();
        want[0x900..0x904].copy_from_slice(b"PPF!");
        want[0x902..0x905].copy_from_slice(b"ips");
        want[0x1000..0x1000 + 300].fill(5);
        assert!(!checksum_ok(&want));
        repair_checksum(&mut want);
        let got = read(&r, &disc);
        assert_eq!(got, want);
        assert!(checksum_ok(&got));
        assert_eq!(r.len(), disc.len() as u64);
        assert!(lib.resolve("midship", &mut &disc[..]).unwrap().is_none());
        assert!(!t.0.join(CACHE_DIR).exists(), "offset patches never copy");
    }

    #[test]
    fn resolving_over_the_disc_records_the_disc_maps_hash() {
        let t = Temp::new();
        let disc = synthetic_map(&payload(0x3000, 7));
        let ips = ips::tests::build(&[ips::tests::Rec::Data(0x900, b"z")], None);
        package(&t.0, "p", "Lockout", Some(&disc), &[("a.ips", &ips)]);
        let bps = write::bps(&disc, &[&disc[..], b"grown"].concat());
        package(&t.0, "q", "midship", Some(&disc), &[("a.bps", &bps)]);
        let hashes = DiscHashes::default();
        let mut lib = Library::open(&t.0).with_disc_hashes(hashes.clone());
        assert_eq!(hashes.get("lockout"), None);
        lib.resolve("lockout", &mut &disc[..]).unwrap().unwrap();
        assert_eq!(hashes.get("LOCKOUT"), Some(Sha256::of(&disc)));
        // A materialised map's base too; and a base that does not match
        // still records what the disc holds.
        let mut other = disc.clone();
        other[0x1000] ^= 1;
        assert!(lib.resolve("midship", &mut &other[..]).is_err());
        assert_eq!(hashes.get("midship"), Some(Sha256::of(&other)));
        // Reopening keeps the table and sees new packages.
        package(&t.0, "r", "ascension", Some(&disc), &[("a.ips", &ips)]);
        let mut again = lib.reopen();
        assert_eq!(again.packages().len(), 3);
        again.resolve("ascension", &mut &disc[..]).unwrap().unwrap();
        assert_eq!(hashes.get("ascension"), Some(Sha256::of(&disc)));
    }

    #[test]
    fn a_different_base_or_layer_is_refused() {
        let t = Temp::new();
        let disc = synthetic_map(&payload(0x1000, 2));
        let ips = ips::tests::build(&[ips::tests::Rec::Data(0x900, b"z")], None);
        package(&t.0, "p", "lockout", Some(&disc), &[("a.ips", &ips)]);
        let mut other = disc.clone();
        other[0xfff] ^= 1;
        let e = Library::open(&t.0)
            .resolve("lockout", &mut &other[..])
            .unwrap_err();
        assert!(matches!(e, ResolveError::Base { .. }), "{e}");
        assert!(e.to_string().contains("disc's lockout.map"), "{e}");
        std::fs::write(t.0.join("p/a.ips"), ips::tests::build(&[], None)).unwrap();
        let e = Library::open(&t.0)
            .resolve("lockout", &mut &disc[..])
            .unwrap_err();
        assert!(matches!(e, ResolveError::Hash { .. }), "{e}");
        // A PPF record past the end grows the map, streamed like any other
        // (a DotHalo PPF for a map that grew), checksum repaired.
        let t = Temp::new();
        let mut p = ppf::tests::build(1, &disc, &[], false, false, None);
        p.extend((disc.len() as u32 - 1).to_le_bytes());
        p.push(2);
        p.extend(b"zz");
        package(&t.0, "p", "lockout", Some(&disc), &[("a.ppf", &p)]);
        let r = Library::open(&t.0)
            .resolve("lockout", &mut &disc[..])
            .unwrap()
            .unwrap();
        assert!(
            matches!(r.data, Data::Overlay { .. }),
            "streamed, not copied"
        );
        assert_eq!(r.len(), disc.len() as u64 + 1);
        let got = read(&r, &disc);
        assert_eq!(&got[disc.len() - 1..], b"zz");
        assert_eq!(&got[0x800..disc.len() - 1], &disc[0x800..disc.len() - 1]);
        assert!(checksum_ok(&got));
        // Unknown formats.
        let t = Temp::new();
        package(
            &t.0,
            "p",
            "lockout",
            Some(&disc),
            &[("a.sppf", b"SPPF....")],
        );
        let e = Library::open(&t.0)
            .resolve("lockout", &mut &disc[..])
            .unwrap_err();
        assert!(e.to_string().contains("unrecognised"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_layer_cannot_leave_its_package() {
        let t = Temp::new();
        let disc = synthetic_map(&payload(0x100, 3));
        let ips = ips::tests::build(&[], None);
        package(&t.0, "p", "lockout", Some(&disc), &[("a.ips", &ips)]);
        std::fs::write(t.0.join("outside.ips"), &ips).unwrap();
        std::fs::remove_file(t.0.join("p/a.ips")).unwrap();
        std::os::unix::fs::symlink(t.0.join("outside.ips"), t.0.join("p/a.ips")).unwrap();
        let e = Library::open(&t.0)
            .resolve("lockout", &mut &disc[..])
            .unwrap_err();
        assert!(e.to_string().contains("leaves the package"), "{e}");
    }

    #[test]
    fn choice_rule() {
        let t = Temp::new();
        let disc = synthetic_map(&payload(0x100, 4));
        let none = ips::tests::build(&[], None);
        package(
            &t.0,
            "lockout-1",
            "lockout",
            Some(&disc),
            &[("a.ips", &none)],
        );
        package(
            &t.0,
            "lockout-2",
            "LOCKOUT",
            Some(&disc),
            &[("b.ips", &none)],
        );
        package(&t.0, "midship", "midship", Some(&disc), &[("c.ips", &none)]);
        std::fs::create_dir_all(t.0.join("broken")).unwrap();
        std::fs::write(
            t.0.join("broken").join(MANIFEST_FILE),
            "[package]\nformat = 9\n",
        )
        .unwrap();
        std::fs::create_dir_all(t.0.join(CACHE_DIR)).unwrap();
        let mut lib = Library::open(&t.0);
        let notices = lib.take_notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("broken"), "{notices:?}");
        assert_eq!(lib.packages().len(), 3);
        assert_eq!(lib.choose("lockout").unwrap().dir_name(), "lockout-2");
        assert!(lib.take_notices()[0].contains("using lockout-2"));
        assert_eq!(lib.choose("Midship").unwrap().dir_name(), "midship");
        assert!(lib.choose("zanzibar").is_none());

        std::fs::write(
            t.0.join(ACTIVE_FILE),
            "lockout = \"lockout-1\"\nMIDSHIP = false\nzanzibar = \"nope\"\n",
        )
        .unwrap();
        let mut lib = Library::open(&t.0);
        assert_eq!(lib.choose("lockout").unwrap().dir_name(), "lockout-1");
        assert!(lib.choose("midship").is_none(), "false means the disc");
        assert!(lib.choose("zanzibar").is_none());
        assert!(lib.take_notices().iter().any(|n| n.contains("nope")));
        std::fs::write(t.0.join(ACTIVE_FILE), "lockout = \"midship\"\n").unwrap();
        let mut lib = Library::open(&t.0);
        assert!(lib.choose("lockout").is_none(), "a package for another map");
        std::fs::write(t.0.join(ACTIVE_FILE), "lockout = 3\n").unwrap();
        let mut lib = Library::open(&t.0);
        assert_eq!(lib.choose("lockout").unwrap().dir_name(), "lockout-2");
        assert!(Library::open(t.0.join("absent")).is_empty());
    }

    #[test]
    fn bps_is_materialised_once_into_the_cache() {
        let t = Temp::new();
        let disc = synthetic_map(&payload(0x400, 5));
        let mut target = disc.clone();
        target.extend(b"grown");
        let patch = bps::tests::build(
            &disc,
            &target,
            &[
                bps::tests::Act::SourceRead(disc.len()),
                bps::tests::Act::TargetRead(b"grown"),
            ],
        );
        let ips = ips::tests::build(&[ips::tests::Rec::Data(0x800, b"after")], None);
        let m = package(
            &t.0,
            "p",
            "lockout",
            Some(&disc),
            &[("a.bps", &patch), ("b.ips", &ips)],
        );
        let mut lib = Library::open(&t.0);
        let r = lib.resolve("lockout", &mut &disc[..]).unwrap().unwrap();
        let Data::File { path, len, .. } = &r.data else {
            panic!("{:?}", r.data)
        };
        assert_eq!(
            *path,
            lib.root()
                .join(CACHE_DIR)
                .join(format!("{}.map", m.map_id()))
        );
        let mut want = target.clone();
        want[0x800..0x805].copy_from_slice(b"after");
        repair_checksum(&mut want);
        assert_eq!(std::fs::read(path).unwrap(), want);
        assert_eq!(*len, want.len() as u64);
        // The matching base can reuse the materialised bytes.
        let r2 = lib.resolve("lockout", &mut &disc[..]).unwrap().unwrap();
        // A different base cannot use the cache, even for the same package.
        assert!(matches!(
            lib.resolve("lockout", &mut &b"wrong base"[..]),
            Err(ResolveError::Base { .. })
        ));
        assert_eq!(read(&r2, b""), want);
        let leftovers: Vec<_> = std::fs::read_dir(lib.root().join(CACHE_DIR))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn the_cache_evicts_least_recently_used() {
        let t = Temp::new();
        let disc = synthetic_map(&payload(0x200, 6));
        let mut ids = Vec::new();
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            let mut target = disc.clone();
            target[0x900] = i as u8;
            let p = bps::tests::build(
                &disc,
                &target,
                &[
                    bps::tests::Act::SourceRead(0x900),
                    bps::tests::Act::TargetRead(&[i as u8]),
                    bps::tests::Act::SourceRead(0xff),
                ],
            );
            ids.push(package(&t.0, name, name, Some(&disc), &[("p.bps", &p)]).map_id());
        }
        let size = disc.len() as u64;
        let mut lib = Library::open(&t.0).with_cache_cap(2 * size);
        let cached = |id: &MapId| t.0.join(CACHE_DIR).join(format!("{id}.map")).exists();
        let age = |id: &MapId, secs: u64| {
            let f = File::options()
                .write(true)
                .open(t.0.join(CACHE_DIR).join(format!("{id}.map")))
                .unwrap();
            f.set_modified(SystemTime::now() - std::time::Duration::from_secs(secs))
                .unwrap();
        };
        lib.resolve("a", &mut &disc[..]).unwrap().unwrap();
        age(&ids[0], 100);
        lib.resolve("b", &mut &disc[..]).unwrap().unwrap();
        age(&ids[1], 50);
        // Using a refreshes it, so b is now the least recently used.
        lib.resolve("a", &mut &disc[..]).unwrap().unwrap();
        lib.resolve("c", &mut &disc[..]).unwrap().unwrap();
        assert!(cached(&ids[0]));
        assert!(!cached(&ids[1]));
        assert!(cached(&ids[2]));
        // A cap below one map still keeps the one just built.
        let mut lib = Library::open(&t.0).with_cache_cap(1);
        lib.resolve("b", &mut &disc[..]).unwrap().unwrap();
        assert!(cached(&ids[1]));
        assert!(!cached(&ids[0]) && !cached(&ids[2]));
    }

    #[test]
    fn a_derived_map_is_its_own_base() {
        let t = Temp::new();
        let mine = synthetic_map(&payload(0x300, 7));
        let ips = ips::tests::build(&[ips::tests::Rec::Data(0x810, b"tweak")], None);
        let m = package(
            &t.0,
            "mine",
            "mymap",
            None,
            &[("mymap.map", &mine), ("t.ips", &ips)],
        );
        assert!(m.base.derived);
        let mut lib = Library::open(&t.0);
        let r = lib.resolve("mymap", &mut &b""[..]).unwrap().unwrap();
        assert!(matches!(
            &r.data,
            Data::Overlay {
                base: BaseRef::File { .. },
                ..
            }
        ));
        let mut want = mine.clone();
        want[0x810..0x815].copy_from_slice(b"tweak");
        repair_checksum(&mut want);
        assert_eq!(read(&r, b""), want);
        // The whole map's hash is checked as it streams.
        let mut changed = mine.clone();
        changed[0x900] ^= 1;
        std::fs::write(t.0.join("mine/mymap.map"), &changed).unwrap();
        let e = Library::open(&t.0)
            .resolve("mymap", &mut &b""[..])
            .unwrap_err();
        assert!(e.to_string().contains("mymap.map does not match"), "{e}");
    }

    #[test]
    fn a_converted_package_resolves_its_default_variant() {
        let t = Temp::new();
        let d = t.0.join("h1_gulch");
        std::fs::create_dir_all(d.join("h1")).unwrap();
        std::fs::create_dir_all(d.join("h2")).unwrap();
        let (h1, h2) = (
            synthetic_map(&payload(0x200, 1)),
            synthetic_map(&payload(0x200, 2)),
        );
        std::fs::write(d.join("h1/h1_gulch.map"), &h1).unwrap();
        std::fs::write(d.join("h2/h1_gulch.map"), &h2).unwrap();
        let variant = |s: &str, bytes: &[u8]| {
            format!(
                "[[variant]]\nfile = \"{s}/h1_gulch.map\"\nsha256 = \"{}\"\n\
                 origin = {{ game = \"halo1\", map = \"gulch\", sandbox = \"{s}\", source_sha256 = \"{}\" }}\n",
                Sha256::of(bytes),
                Sha256([7; 32])
            )
        };
        let head =
            "[package]\nformat = 1\nname = \"Gulch\"\n[base]\nmap = \"h1_gulch\"\nderived = true\n";
        let write = |variants: &str| {
            std::fs::write(d.join(MANIFEST_FILE), format!("{head}{variants}")).unwrap();
        };
        write(&format!("{}{}", variant("h2", &h2), variant("h1", &h1)));
        let mut lib = Library::open(&t.0);
        assert_eq!(lib.packages().len(), 1, "{:?}", lib.take_notices());
        let r = lib.resolve("h1_gulch", &mut &b""[..]).unwrap().unwrap();
        assert_eq!(read(&r, b""), h1, "Halo 1's sandbox by default");
        let id_h1 = lib.packages()[0].id;
        // The setting asks for Halo 2's: that one, with its own MapId.
        let mut lib = Library::open_with_sandbox(&t.0, Some("h2"));
        let r = lib.resolve("h1_gulch", &mut &b""[..]).unwrap().unwrap();
        assert_eq!(read(&r, b""), h2);
        assert_ne!(lib.packages()[0].id, id_h1);
        let key = lib.packages()[0].list_key;
        // A rescan keeps the sandbox.
        let mut lib = lib.reopen();
        let r = lib.resolve("h1_gulch", &mut &b""[..]).unwrap().unwrap();
        assert_eq!(read(&r, b""), h2, "reopened in Halo 2's sandbox");
        let lib = Library::open_with_sandbox(&t.0, Some("h1"));
        assert_eq!(lib.packages()[0].id, id_h1);
        assert_eq!(lib.packages()[0].list_key, key, "one list key for both");
        assert_ne!(key, id_h1);
        // Only the Halo 2 variant: that one, with its own MapId.
        write(&variant("h2", &h2));
        let mut lib = Library::open(&t.0);
        let r = lib.resolve("h1_gulch", &mut &b""[..]).unwrap().unwrap();
        assert_eq!(read(&r, b""), h2);
        assert_ne!(lib.packages()[0].id, id_h1);
    }

    #[test]
    fn non_maps_are_served_without_a_checksum_repair() {
        let t = Temp::new();
        let disc = b"not a map at all".to_vec();
        let ips = ips::tests::build(&[ips::tests::Rec::Data(0, b"NOT")], None);
        package(&t.0, "p", "odd", Some(&disc), &[("a.ips", &ips)]);
        let r = Library::open(&t.0)
            .resolve("odd", &mut &disc[..])
            .unwrap()
            .unwrap();
        assert_eq!(read(&r, &disc), b"NOT a map at all");
    }
}
