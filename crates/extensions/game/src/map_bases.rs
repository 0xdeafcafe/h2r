//! The disc's own maps, hashed, for joining games on custom maps
//! (docs/game/maps.md §5): `live_hle::maps::Bases` over the retail ISO and
//! the installed Multiplayer Map Pack.
//!
//! [`DiscBases`] hashes `maps\<name>.map` as the disc has it, read through
//! its own read-only [`vfs::DiscBackend`] on the ISO: below every package
//! and `H2_MAP_DIR` layer, so a package can never vouch for its own base.
//! Each map is hashed lazily, once, on first ask (a missing map is
//! remembered too). The hashes are shared with the package resolver
//! ([`content::DiscHashes`]), which records each disc base it reads in full
//! while checking a package, so a map hashed by either is not read again.
//!
//! The hashes also persist in `<data>/maps/cache/disc-hashes.toml`, stamped
//! with the ISO's path, size and modification time: a restart against the
//! same ISO does not read 300 MB maps again, and a different or touched ISO
//! starts afresh.
//!
//! [`MapPackBases`] hashes the Multiplayer Map Pack's maps as
//! `h2 import-mappack` installed them, byte for byte from the map pack disc
//! (`<data>/disk/Partition1/TDATA/4D530064/$c/<offer>/<map>.map`), so the
//! hash is the one `h2 live maps import` gives a mod on a map pack base
//! (docs/game/maps.md, "Importing a mod"). Each file is hashed once and
//! the hash kept in `<data>/maps/cache/mappack-hashes.toml`, stamped per
//! map with the file's path, size and modification time. [`UserBases`] asks
//! the disc first, then the map pack for a map pack name.
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

use content::{DiscHashes, Sha256};
use vfs::Backend;

/// The stamp file, under `<data>/maps/cache/`.
pub const STAMP_FILE: &str = "disc-hashes.toml";
/// The map pack's stamp file, under `<data>/maps/cache/`.
pub const MAP_PACK_STAMP_FILE: &str = "mappack-hashes.toml";

/// [`live_hle::maps::Bases`] over the retail ISO.
pub struct DiscBases {
    iso: PathBuf,
    stamp_file: Option<PathBuf>,
    hashes: DiscHashes,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The disc, opened on first need; `Err` once it failed to open.
    disc: Option<Result<vfs::DiscBackend<std::fs::File>, ()>>,
    /// The stamp file was read (or found stale).
    loaded: bool,
    /// Names the disc lacks.
    missing: std::collections::HashSet<String>,
    /// Full reads made (for tests: each map at most once).
    reads: usize,
}

/// A file's identity for a stamp: canonical path, size, mtime in ns.
fn identity(iso: &Path) -> Option<(String, u64, u128)> {
    let meta = std::fs::metadata(iso).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let path = iso.canonicalize().ok()?;
    Some((path.to_string_lossy().into_owned(), meta.len(), mtime))
}

/// A plain map name: letters, digits, `_`, `-` (a name from the network
/// never reaches the disc as a path).
fn plain(map: &str) -> bool {
    !map.is_empty()
        && map.len() <= 64
        && map
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl DiscBases {
    /// Over the ISO at `iso`, sharing `hashes` with the resolver, keeping
    /// the stamp under `maps_dir/cache` when given. No I/O until asked.
    pub fn new(iso: impl Into<PathBuf>, maps_dir: Option<&Path>, hashes: DiscHashes) -> Self {
        DiscBases {
            iso: iso.into(),
            stamp_file: maps_dir.map(|d| d.join(content::CACHE_DIR).join(STAMP_FILE)),
            hashes,
            state: Mutex::default(),
        }
    }

    /// How many maps were read in full so far.
    pub fn reads(&self) -> usize {
        self.lock().reads
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn load_stamp(&self) {
        let Some(file) = &self.stamp_file else { return };
        let Some((iso, size, mtime)) = identity(&self.iso) else {
            return;
        };
        // iso = "...", size = n, mtime = "ns", [maps] name = "hex".
        let Some(stamp) = std::fs::read_to_string(file)
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
        else {
            return;
        };
        let same = stamp.get("iso").and_then(|v| v.as_str()) == Some(iso.as_str())
            && stamp.get("size").and_then(|v| v.as_integer()) == i64::try_from(size).ok()
            && stamp.get("mtime").and_then(|v| v.as_str()) == Some(mtime.to_string().as_str());
        if !same {
            return;
        }
        let Some(maps) = stamp.get("maps").and_then(|v| v.as_table()) else {
            return;
        };
        for (map, hex) in maps {
            if let Some(h) = hex.as_str().and_then(|h| h.parse::<Sha256>().ok())
                && plain(map)
                && self.hashes.get(map).is_none()
            {
                self.hashes.insert(map, h);
            }
        }
    }

    fn save_stamp(&self) {
        let Some(file) = &self.stamp_file else { return };
        let Some((iso, size, mtime)) = identity(&self.iso) else {
            return;
        };
        let mut maps = toml::Table::new();
        for (k, v) in self.hashes.snapshot() {
            maps.insert(k, toml::Value::String(v.to_string()));
        }
        let mut stamp = toml::Table::new();
        stamp.insert("iso".into(), toml::Value::String(iso));
        stamp.insert(
            "size".into(),
            toml::Value::Integer(i64::try_from(size).unwrap_or(i64::MAX)),
        );
        stamp.insert("mtime".into(), toml::Value::String(mtime.to_string()));
        stamp.insert("maps".into(), toml::Value::Table(maps));
        let Ok(text) = toml::to_string(&stamp) else {
            return;
        };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
            let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, file)
        };
        if let Err(e) = write() {
            eprintln!("map bases: cannot write {}: {e}", file.display());
        }
    }

    /// Hash the disc's `maps\<map>.map`, None if the disc lacks it.
    fn hash_disc(state: &mut State, iso: &Path, map: &str) -> Option<Sha256> {
        let disc = state
            .disc
            .get_or_insert_with(|| {
                vfs::DiscBackend::open(iso).map_err(|e| {
                    eprintln!("map bases: cannot read {} as a disc: {e:?}", iso.display());
                })
            })
            .as_mut()
            .ok()?;
        let path = format!("maps\\{map}.map");
        let meta = disc.stat(&path).ok().filter(|m| !m.is_directory())?;
        let mut h = content::Hasher::default();
        let mut buf = vec![0; 1 << 20];
        let mut at = 0;
        while at < meta.end_of_file {
            let n = disc.read_at(&path, at, &mut buf).ok()?;
            if n == 0 {
                return None;
            }
            h.update(&buf[..n]);
            at += n as u64;
        }
        state.reads += 1;
        Some(h.finish())
    }
}

impl live_hle::maps::Bases for DiscBases {
    fn disc_hash(&self, map: &str) -> Option<Sha256> {
        if !plain(map) {
            return None;
        }
        let key = map.to_ascii_lowercase();
        let mut state = self.lock();
        if !state.loaded {
            state.loaded = true;
            self.load_stamp();
        }
        if let Some(h) = self.hashes.get(&key) {
            return Some(h);
        }
        if state.missing.contains(&key) {
            return None;
        }
        match Self::hash_disc(&mut state, &self.iso, &key) {
            Some(h) => {
                self.hashes.insert(&key, h);
                drop(state);
                self.save_stamp();
                Some(h)
            }
            None => {
                state.missing.insert(key);
                None
            }
        }
    }
}

fn map_pack_offers(data_root: &Path) -> Vec<(String, PathBuf)> {
    let title = crate::new_maps::HALO2_TITLE;
    let root = vfs::Config::new(data_root, title)
        .title_data_dir()
        .join(vfs::CONTENT_DIR);
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut offers: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let offer = u64::from_str_radix(&name, 16)
                .ok()
                .filter(|_| name.len() == 16)?;
            content::map_pack::is_offer(title, offer).then(|| (name, e.path()))
        })
        .collect();
    offers.sort();
    offers
}

/// The installed copy of map pack map `map` under `data_root`, as
/// `h2 import-mappack` writes it: `(offer directory, file name, host path)`
/// for the first `$c\<offer>` (by name) of a map pack offer holding
/// `<map>.map` in any case. None for a name that isn't a map pack map's.
pub fn map_pack_file(data_root: &Path, map: &str) -> Option<(String, String, PathBuf)> {
    if !plain(map) || !content::map_pack::is_map(map) {
        return None;
    }
    let want = format!("{map}.map");
    map_pack_offers(data_root).into_iter().find_map(|(offer, dir)| {
        std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            (name.eq_ignore_ascii_case(&want) && e.file_type().ok()?.is_file())
                .then(|| (offer.clone(), name, e.path()))
        })
    })
}

/// Matchmaking needs all nine map-pack offers installed. This checks the
/// packages in the console's data root without reading the maps.
fn map_pack_installed(data_root: &Path) -> bool {
    let mut installed = HashSet::new();
    let mut ids = HashSet::new();
    let mut record = vec![0; content::patch_lvl::V2_LEN];
    for (offer, dir) in map_pack_offers(data_root) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let files: HashMap<_, _> = entries
            .flatten()
            .filter_map(|entry| {
                if !entry.file_type().ok()?.is_file() {
                    return None;
                }
                Some((entry.file_name().to_str()?.to_ascii_lowercase(), entry))
            })
            .collect();
        let (Some(patch), Some(meta)) = (files.get("patch.lvl"), files.get("contentmeta.xbx")) else {
            continue;
        };
        if !signed_map_pack_offer(&offer, &meta.path()) {
            continue;
        }
        // The record identifies the one map the guest can load from this
        // directory. Merely copying all nine files into one offer can't list nine maps.
        let read = std::fs::File::open(patch.path())
            .and_then(|mut file| file.read_exact(&mut record));
        if read.is_err() || content::patch_lvl::validate_v2(&record).is_err() {
            continue;
        }
        let Ok(map) = content::patch_lvl::map_name(&record) else {
            continue;
        };
        if content::map_pack::is_map(&map)
            && files.get(&format!("{}.map", map.to_ascii_lowercase()))
                .and_then(|file| file.metadata().ok())
                .is_some_and(|meta| meta.len() != 0)
        {
            ids.insert(content::patch_lvl::v2_map_id(&record));
            installed.insert(map.to_ascii_lowercase());
        }
    }
    installed.len() == content::map_pack::MAPS.len() && ids.len() == installed.len()
}

fn signed_map_pack_offer(offer: &str, path: &Path) -> bool {
    use kernel::content::{FLAGS_OFFSET, FLAG_INSTALLED, SIGNATURE_SIZE, signature};
    const OFFER_ID: usize = 0x28;
    let mut header = [0; 0x88];
    if std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_err()
    {
        return false;
    }
    let flags = u32::from_le_bytes(header[FLAGS_OFFSET..FLAGS_OFFSET + 4].try_into().unwrap());
    let id = u64::from_le_bytes(header[OFFER_ID..OFFER_ID + 8].try_into().unwrap());
    flags & FLAG_INSTALLED != 0
        && u64::from_str_radix(offer, 16) == Ok(id)
        && signature(&header, crate::new_maps::HALO2_TITLE)
            .is_ok_and(|mac| header[..SIGNATURE_SIZE] == mac)
}

/// The map pack's installed maps, hashed (module docs).
pub struct MapPackBases {
    data_root: PathBuf,
    stamp_file: Option<PathBuf>,
    state: Mutex<PackState>,
}

#[derive(Default)]
struct PackState {
    /// The stamp file was read.
    loaded: bool,
    /// By lowercase map name: the file's identity when hashed, and its hash.
    known: HashMap<String, ((String, u64, u128), Sha256)>,
    /// Full reads made (for tests: each file at most once).
    reads: usize,
}

impl MapPackBases {
    /// The maps installed under `data_root`, keeping the stamp under
    /// `maps_dir/cache` when given. No I/O until asked.
    pub fn new(data_root: impl Into<PathBuf>, maps_dir: Option<&Path>) -> Self {
        MapPackBases {
            data_root: data_root.into(),
            stamp_file: maps_dir.map(|d| d.join(content::CACHE_DIR).join(MAP_PACK_STAMP_FILE)),
            state: Mutex::default(),
        }
    }

    /// How many maps were read in full so far.
    pub fn reads(&self) -> usize {
        self.lock().reads
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PackState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// SHA-256 of the installed `map`, None when the map pack isn't
    /// installed (or `map` isn't one of its maps).
    pub fn hash(&self, map: &str) -> Option<Sha256> {
        let key = map.to_ascii_lowercase();
        let (_, _, file) = map_pack_file(&self.data_root, &key)?;
        let id = identity(&file)?;
        let mut state = self.lock();
        if !state.loaded {
            state.loaded = true;
            self.load_stamp(&mut state);
        }
        if let Some((was, h)) = state.known.get(&key)
            && *was == id
        {
            return Some(*h);
        }
        let h = hash_file(&file)?;
        state.reads += 1;
        state.known.insert(key, (id, h));
        self.save_stamp(&state);
        Some(h)
    }

    // [<map>] path = "...", size = n, mtime = "ns", sha256 = "hex".
    fn load_stamp(&self, state: &mut PackState) {
        let Some(table) = self
            .stamp_file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| t.parse::<toml::Table>().ok())
        else {
            return;
        };
        for (map, row) in table {
            let get = |k: &str| row.get(k);
            let (Some(path), Some(size), Some(mtime), Some(h)) = (
                get("path").and_then(|v| v.as_str()),
                get("size")
                    .and_then(|v| v.as_integer())
                    .and_then(|n| u64::try_from(n).ok()),
                get("mtime")
                    .and_then(|v| v.as_str())
                    .and_then(|m| m.parse::<u128>().ok()),
                get("sha256")
                    .and_then(|v| v.as_str())
                    .and_then(|h| h.parse::<Sha256>().ok()),
            ) else {
                continue;
            };
            if plain(&map) {
                state.known.insert(
                    map.to_ascii_lowercase(),
                    ((path.to_owned(), size, mtime), h),
                );
            }
        }
    }

    fn save_stamp(&self, state: &PackState) {
        let Some(file) = &self.stamp_file else { return };
        let mut table = toml::Table::new();
        for (map, ((path, size, mtime), h)) in &state.known {
            let mut row = toml::Table::new();
            row.insert("path".into(), toml::Value::String(path.clone()));
            row.insert(
                "size".into(),
                toml::Value::Integer(i64::try_from(*size).unwrap_or(i64::MAX)),
            );
            row.insert("mtime".into(), toml::Value::String(mtime.to_string()));
            row.insert("sha256".into(), toml::Value::String(h.to_string()));
            table.insert(map.clone(), toml::Value::Table(row));
        }
        let Ok(text) = toml::to_string(&table) else {
            return;
        };
        if let Err(e) = write_stamp(file, &text) {
            eprintln!("map bases: cannot write {}: {e}", file.display());
        }
    }
}

fn write_stamp(file: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
    let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
}

fn hash_file(path: &Path) -> Option<Sha256> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = content::Hasher::default();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            return Some(h.finish());
        }
        h.update(&buf[..n]);
    }
}

/// The user's own bases: the disc's maps, then the installed map pack's.
pub struct UserBases {
    pub disc: DiscBases,
    pub map_pack: MapPackBases,
}

impl live_hle::maps::Bases for UserBases {
    fn disc_hash(&self, map: &str) -> Option<Sha256> {
        self.disc.disc_hash(map).or_else(|| self.map_pack.hash(map))
    }

    fn has_map_pack(&self) -> bool {
        map_pack_installed(&self.map_pack.data_root)
    }
}

/// The join-time map gate for a boot: packages in `<data>/maps`, bases
/// from the ISO at `disc` (sharing `hashes` with the package resolver)
/// and the map pack installed under `data_root`.
pub fn gate(disc: &Path, data_root: &Path, hashes: DiscHashes) -> Arc<live_hle::maps::MapGate> {
    let maps = crate::map_override::packages_dir(data_root);
    let bases = UserBases {
        disc: DiscBases::new(disc, Some(&maps), hashes),
        map_pack: MapPackBases::new(data_root, Some(&maps)),
    };
    Arc::new(live_hle::maps::MapGate::new(maps, Arc::new(bases)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_hle::maps::Bases;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let p = base.join(format!(
                "h2-boot-map-bases-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(p.join("data").join("maps")).unwrap();
            Self(p)
        }
        fn maps(&self) -> PathBuf {
            self.0.join("data").join("maps")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const LEVEL: &[u8] = b"the disc's own level map";

    fn iso(t: &Temp) -> PathBuf {
        crate::map_override::tests::iso_at(&t.0, LEVEL)
    }

    #[test]
    fn the_hash_is_the_disc_files_sha256_cached_and_unknown_names_are_none() {
        let t = Temp::new();
        let iso = iso(&t);
        let hashes = DiscHashes::default();
        let bases = DiscBases::new(&iso, Some(&t.maps()), hashes.clone());
        assert_eq!(bases.reads(), 0, "nothing read before the first ask");
        assert_eq!(bases.disc_hash("level"), Some(Sha256::of(LEVEL)));
        assert_eq!(bases.disc_hash("LEVEL"), Some(Sha256::of(LEVEL)));
        assert_eq!(bases.reads(), 1, "hashed once");
        assert_eq!(hashes.get("level"), Some(Sha256::of(LEVEL)), "shared");
        assert_eq!(bases.disc_hash("lockout"), None);
        assert_eq!(bases.disc_hash("lockout"), None);
        for odd in ["", "..\\default", "maps\\level", "level.map", "a b"] {
            assert_eq!(bases.disc_hash(odd), None, "{odd:?}");
        }
        assert_eq!(bases.reads(), 1);
        // Not a disc at all: None, no panic.
        let raw = t.0.join("raw.xbe");
        std::fs::write(&raw, b"XBEH").unwrap();
        assert_eq!(
            DiscBases::new(&raw, None, DiscHashes::default()).disc_hash("level"),
            None
        );
    }

    #[test]
    fn a_hash_the_resolver_recorded_is_not_read_again() {
        let t = Temp::new();
        let iso = iso(&t);
        let hashes = DiscHashes::default();
        hashes.insert("level", Sha256([7; 32]));
        let bases = DiscBases::new(&iso, None, hashes);
        assert_eq!(bases.disc_hash("level"), Some(Sha256([7; 32])));
        assert_eq!(bases.reads(), 0);
    }

    #[test]
    fn the_stamp_survives_a_restart_against_the_same_iso_only() {
        let t = Temp::new();
        let iso = iso(&t);
        let first = DiscBases::new(&iso, Some(&t.maps()), DiscHashes::default());
        assert_eq!(first.disc_hash("level"), Some(Sha256::of(LEVEL)));
        assert!(t.maps().join("cache").join(STAMP_FILE).is_file());
        let again = DiscBases::new(&iso, Some(&t.maps()), DiscHashes::default());
        assert_eq!(again.disc_hash("level"), Some(Sha256::of(LEVEL)));
        assert_eq!(again.reads(), 0, "from the stamp");
        // A touched ISO: hashed afresh.
        let f = std::fs::File::options().write(true).open(&iso).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        let touched = DiscBases::new(&iso, Some(&t.maps()), DiscHashes::default());
        assert_eq!(touched.disc_hash("level"), Some(Sha256::of(LEVEL)));
        assert_eq!(touched.reads(), 1);
        // The stamp is not a package.
        assert!(content::Library::open(t.maps()).packages().is_empty());
    }

    /// Below every layer: a package and an `H2_MAP_DIR` override serving
    /// `level` change nothing the bases say.
    #[test]
    fn packages_and_overrides_do_not_change_the_base() {
        let t = Temp::new();
        let iso = iso(&t);
        let maps = t.maps();
        let dir = maps.join("level-mod");
        std::fs::create_dir_all(&dir).unwrap();
        let patch = b"PATCH\0\0\0\0\x04DISCEOF";
        std::fs::write(dir.join("a.ips"), patch).unwrap();
        let mut m = content::Manifest::retail("level", Sha256::of(LEVEL));
        m.patches.push(content::manifest::Layer {
            file: "a.ips".into(),
            sha256: Sha256::of(patch),
        });
        std::fs::write(dir.join("manifest.toml"), m.to_toml()).unwrap();
        let over = t.0.join("override");
        std::fs::create_dir_all(&over).unwrap();
        std::fs::write(over.join("level.map"), b"overridden").unwrap();
        let hashes = DiscHashes::default();
        let cfg = vfs::Config::new(t.0.join("data"), 0x4D530064);
        let mut v = crate::map_override::mount::<crate::map_override::tests::Upper>(
            cfg,
            &iso,
            None,
            Some(&over),
            &[],
        )
        .unwrap();
        assert!(crate::map_override::map_id(&mut v, "level").is_some());
        let bases = DiscBases::new(&iso, Some(&maps), hashes);
        assert_eq!(bases.disc_hash("level"), Some(Sha256::of(LEVEL)));
    }

    const TURF: &[u8] = b"an invented map pack map";

    /// Turf installed as `h2 import-mappack` lays it out (the disc's own
    /// file name case), and a same-named file under a new map's offer.
    fn install_turf(t: &Temp) -> PathBuf {
        let cfg = vfs::Config::new(t.0.join("data"), crate::new_maps::HALO2_TITLE);
        let dir = cfg.content_dir(0x4D53_0064_0000_0102);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Turf.map"), TURF).unwrap();
        std::fs::write(dir.join("patch.lvl"), b"record").unwrap();
        let other = cfg.content_dir(0x4D53_0064_000a_0001);
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("gemini.map"), b"not the map pack's").unwrap();
        dir.join("Turf.map")
    }

    #[test]
    fn installed_map_pack_maps_hash_as_their_files_once_and_across_restarts() {
        let t = Temp::new();
        let data = t.0.join("data");
        let pack = MapPackBases::new(&data, Some(&t.maps()));
        assert_eq!(pack.hash("turf"), None, "not installed");
        let file = install_turf(&t);
        assert_eq!(
            map_pack_file(&data, "TURF").map(|f| (f.0, f.1)),
            Some(("4d53006400000102".to_string(), "Turf.map".to_string()))
        );
        assert_eq!(pack.hash("turf"), Some(Sha256::of(TURF)));
        assert_eq!(pack.hash("Turf"), Some(Sha256::of(TURF)));
        assert_eq!(pack.reads(), 1, "hashed once");
        // Only the map pack's names, under its offers.
        for not in ["gemini", "lockout", "patch", "..\\turf", ""] {
            assert_eq!(pack.hash(not), None, "{not:?}");
        }
        assert!(t.maps().join("cache").join(MAP_PACK_STAMP_FILE).is_file());
        let again = MapPackBases::new(&data, Some(&t.maps()));
        assert_eq!(again.hash("turf"), Some(Sha256::of(TURF)));
        assert_eq!(again.reads(), 0, "from the stamp");
        // A changed file: hashed afresh.
        std::fs::write(&file, b"another invented map, longer").unwrap();
        let changed = MapPackBases::new(&data, Some(&t.maps()));
        assert_eq!(
            changed.hash("turf"),
            Some(Sha256::of(b"another invented map, longer"))
        );
        assert_eq!(changed.reads(), 1);
        // Removed: none, whatever the stamp says.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(MapPackBases::new(&data, Some(&t.maps())).hash("turf"), None);
        // The stamp is not a package.
        assert!(content::Library::open(t.maps()).packages().is_empty());
    }

    #[test]
    fn the_user_bases_are_the_disc_then_the_map_pack() {
        let t = Temp::new();
        let iso = iso(&t);
        install_turf(&t);
        let bases = UserBases {
            disc: DiscBases::new(&iso, None, DiscHashes::default()),
            map_pack: MapPackBases::new(t.0.join("data"), None),
        };
        assert_eq!(bases.disc_hash("level"), Some(Sha256::of(LEVEL)));
        assert_eq!(bases.disc_hash("turf"), Some(Sha256::of(TURF)));
        assert_eq!(bases.disc_hash("dune"), None, "not installed");
        // The gate over them gives the map pack map its retail id.
        let gate = gate(&iso, &t.0.join("data"), DiscHashes::default());
        assert_eq!(
            gate.retail_id("turf"),
            Some(content::MapId::retail("turf", Sha256::of(TURF)))
        );
        assert!(!gate.has_map_pack(), "one installed map is not the full pack");
    }

    #[test]
    fn matchmaking_pack_requires_all_signed_one_map_offers() {
        let t = Temp::new();
        let data = t.0.join("data");
        let title = crate::new_maps::HALO2_TITLE;
        let cfg = vfs::Config::new(&data, title);
        // A stamp alone does not make an interrupted installation playable.
        std::fs::write(data.join("mappack.stamp"), b"stale").unwrap();
        assert!(!map_pack_installed(&data));
        for (i, (map, _)) in content::map_pack::MAPS.iter().enumerate() {
            let id = (u64::from(title) << 32) | (0x0101 + i as u64);
            let dir = cfg.content_dir(id);
            std::fs::create_dir_all(&dir).unwrap();
            let record = content::patch_lvl::testing::v3_record(map, i as u32 + 10, 0);
            let patch = content::patch_lvl::v2_from_v3(&record).unwrap();
            let mut meta = [0; 0x88];
            meta[0x14..0x18].copy_from_slice(b"XCSF");
            meta[0x18..0x1c].copy_from_slice(&0x88u32.to_le_bytes());
            meta[0x24..0x28].copy_from_slice(&title.to_le_bytes());
            meta[0x28..0x30].copy_from_slice(&id.to_le_bytes());
            kernel::content::sign(&mut meta, title).unwrap();
            std::fs::write(dir.join("ContentMeta.xbx"), meta).unwrap();
            std::fs::write(dir.join("Patch.lvl"), patch).unwrap();
            std::fs::write(dir.join(format!("{}.MAP", map.to_ascii_uppercase())), b"map").unwrap();
        }
        assert!(gate(&iso(&t), &data, DiscHashes::default()).has_map_pack());
        let first = cfg.content_dir((u64::from(title) << 32) | 0x0101);
        std::fs::write(first.join("ContentMeta.xbx"), [0; 0x88]).unwrap();
        assert!(!gate(&iso(&t), &data, DiscHashes::default()).has_map_pack());
    }
}
