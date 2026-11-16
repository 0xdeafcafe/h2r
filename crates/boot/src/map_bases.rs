//! The disc's own maps, hashed, for joining games on custom maps
//! (docs/game/maps.md §5): `live_hle::maps::Bases` over the retail ISO.
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
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

use content::{DiscHashes, Sha256};
use vfs::Backend;

/// The stamp file, under `<data>/maps/cache/`.
pub const STAMP_FILE: &str = "disc-hashes.toml";

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

/// The ISO's identity for the stamp: canonical path, size, mtime in ns.
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

/// The join-time map gate for a boot: packages in `<data>/maps`, bases
/// from the ISO at `disc`, sharing `hashes` with the package resolver.
pub fn gate(disc: &Path, data_root: &Path, hashes: DiscHashes) -> Arc<live_hle::maps::MapGate> {
    let maps = crate::map_override::packages_dir(data_root);
    let bases = DiscBases::new(disc, Some(&maps), hashes);
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
        let mut v = crate::map_override::mount_with_hashes::<crate::map_override::tests::Upper>(
            cfg,
            &iso,
            None,
            Some(&over),
            Some(&maps),
            hashes.clone(),
        )
        .unwrap();
        assert!(crate::map_override::map_id(&mut v, "level").is_some());
        let bases = DiscBases::new(&iso, Some(&maps), hashes);
        assert_eq!(bases.disc_hash("level"), Some(Sha256::of(LEVEL)));
    }
}
