//! Installed mods for the stats site's art (docs/live/live-art.md, "Mods";
//! docs/game/maps.md, "Pictures and the 3D viewer"): every map package
//! under `<data>/maps/` resolved to the bytes the game would load, so the
//! 3D viewer draws the mod's own level, objects, scripted spawns and
//! pictures, not the retail map's.
//!
//! Each package is resolved through `content::Library` exactly as the
//! game's package backend does it: over the disc's map (or the installed
//! map pack map) of its `[base] map`, checked against the base's SHA-256,
//! or from its own whole map for a derived map. An offset patch's result is
//! written once into a scratch directory (the extractor reads maps from
//! files); a materialised (BPS) result is read from the package cache
//! where the resolver keeps it.
//!
//! A mod's art is keyed by what it is, never by a map name: its stem is
//! `mod-<first 16 hex digits of its MapId>` (the `MapId` the game reports
//! with each game), its manifest entry carries the full `map_id`, the base
//! map's name and the SHA-256 of the patched map, and that hash joins the
//! art's cache key, so changing a mod's bytes extracts it again.
//!
//! Converted Halo 1 maps (packages with variants) are left to
//! `crate::live_halo1`. Nothing of any map is in the repository; the tests
//! build invented packages over invented bases.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use content::patch::Source;
use content::{Data, Hasher, Library, MapId, Sha256};

/// The prefix of a mod's art stem.
pub const STEM_PREFIX: &str = "mod-";
const HEAD: usize = 2048;

/// A mod's art stem: [`STEM_PREFIX`] and the first 16 hex digits of its
/// `MapId` (what `boot::new_maps` and the joining code name packages by).
pub fn mod_stem(id: &MapId) -> String {
    format!("{STEM_PREFIX}{}", &id.to_string()[..16])
}

/// What a mod's manifest entry says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModInfo {
    /// The package's display name.
    pub name: String,
    pub map_id: MapId,
    /// The map it is played as (`[base] map`).
    pub base: String,
    /// SHA-256 of the patched map.
    pub sha256: Sha256,
    /// The package's picture, if it has one.
    pub image: Option<PathBuf>,
}

/// A resolved mod: where its patched map is and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundMod {
    pub stem: String,
    pub path: PathBuf,
    pub size: u32,
    /// The first 2 KiB (the cache header).
    pub head: Vec<u8>,
    pub info: ModInfo,
}

/// What [`installed`] found, and the packages it could not use, with why.
#[derive(Debug, Default)]
pub struct Found {
    pub mods: Vec<FoundMod>,
    pub notices: Vec<String>,
}

/// A byte range of a file as a patch [`Source`]: a map inside the disc
/// image or a map pack file.
pub struct Region {
    file: File,
    base: u64,
    len: u64,
}

impl Region {
    pub fn open(path: &Path, base: u64, len: u64) -> io::Result<Region> {
        Ok(Region {
            file: File::open(path)?,
            base,
            len,
        })
    }
}

impl Source for Region {
    fn size(&self) -> u64 {
        self.len
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let n = (self.len - offset).min(buf.len() as u64) as usize;
        self.file.seek(SeekFrom::Start(self.base + offset))?;
        self.file.read_exact(&mut buf[..n])?;
        Ok(n)
    }
}

fn hash_file(path: &Path) -> io::Result<(Sha256, u64, Vec<u8>)> {
    let mut f = File::open(path)?;
    let mut h = Hasher::default();
    let mut buf = vec![0; 1 << 20];
    let mut head = Vec::new();
    let mut len = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if head.len() < HEAD {
            head.extend_from_slice(&buf[..n.min(HEAD - head.len())]);
        }
        h.update(&buf[..n]);
        len += n as u64;
    }
    Ok((h.finish(), len, head))
}

/// The packages that are mods: patch layers over a base, not a converted
/// map with a variant per sandbox (`crate::live_halo1`).
fn is_mod(p: &content::Package) -> bool {
    p.list_key == p.id && !p.manifest.patches.is_empty()
}

/// The `MapId`s of the mods installed under `data`, without resolving
/// any: what the art's cache key names them by. A `MapId` hashes the base's
/// SHA-256 and every layer's, so it names the patched map's bytes; a
/// changed mod has another.
pub fn package_ids(data: &Path) -> Vec<MapId> {
    let lib = Library::open(content::store::maps_dir(data));
    let mut ids: Vec<MapId> = lib
        .packages()
        .iter()
        .filter(|p| is_mod(p))
        .map(|p| p.id)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Every installed mod under `data` (`<data>/maps`), resolved. `base`
/// gives the bytes of a map the disc (or the installed map pack) has, by
/// name; patched maps that are not already files are written into
/// `scratch`. A package that cannot be resolved is left out with a notice.
pub fn installed(
    data: &Path,
    scratch: &Path,
    base: &mut dyn FnMut(&str) -> Option<Box<dyn Source>>,
) -> Found {
    let mut lib = Library::open(content::store::maps_dir(data));
    let mut found = Found {
        notices: lib.take_notices(),
        ..Found::default()
    };
    for p in lib.packages() {
        let dir = p.dir_name().to_string();
        if !is_mod(p) {
            continue;
        }
        let mut src: Box<dyn Source> =
            base(&p.manifest.base.map).unwrap_or_else(|| Box::new(&[][..]) as Box<dyn Source>);
        let one = (|| -> anyhow::Result<FoundMod> {
            let resolved = lib.resolve_package(p, src.as_mut())?;
            let stem = mod_stem(&p.id);
            let path = match &resolved.data {
                Data::File { path, .. } => path.clone(),
                Data::Overlay { .. } => {
                    let bytes = resolved.read_all(src.as_mut())?;
                    std::fs::create_dir_all(scratch)?;
                    let path = scratch.join(format!("{stem}.map"));
                    std::fs::write(&path, &bytes)?;
                    path
                }
            };
            let (sha256, len, head) = hash_file(&path)?;
            let size = u32::try_from(len)
                .map_err(|_| anyhow::anyhow!("{len} bytes is too large for a map"))?;
            Ok(FoundMod {
                stem,
                path,
                size,
                head,
                info: ModInfo {
                    name: p.manifest.package.name.clone(),
                    map_id: p.id,
                    base: p.manifest.base.map.to_ascii_lowercase(),
                    sha256,
                    image: p
                        .manifest
                        .package
                        .image
                        .as_deref()
                        .and_then(|i| content::manifest::relative(i).ok())
                        .map(|i| p.dir.join(i))
                        .filter(|i| i.is_file()),
                },
            })
        })();
        match one {
            Ok(m) => found.mods.push(m),
            Err(e) => found
                .notices
                .push(format!("mod {dir} ({}): {e:#}", p.manifest.base.map)),
        }
    }
    found.mods.sort_by(|a, b| a.stem.cmp(&b.stem));
    found.mods.dedup_by(|a, b| a.stem == b.stem);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use content::Manifest;
    use content::manifest::Layer;

    /// An invented map: a cache type and some bytes. No `daeh` header, so
    /// the resolver leaves the bytes as the patch makes them.
    fn invented(cache_type: u32, fill: u8, len: usize) -> Vec<u8> {
        let mut b = vec![fill; len];
        b[0x140..0x144].copy_from_slice(&cache_type.to_le_bytes());
        b
    }

    fn package(
        maps: &Path,
        dir: &str,
        name: &str,
        base_map: &str,
        base: &[u8],
        edited: &[u8],
    ) -> MapId {
        let d = maps.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        let patch = content::patch::diff(base, edited, "test");
        std::fs::write(d.join("mod.patch"), &patch.bytes).unwrap();
        let mut m = Manifest::retail(base_map, Sha256::of(base));
        m.package.name = name.into();
        m.patches = vec![Layer {
            file: "mod.patch".into(),
            sha256: Sha256::of(&patch.bytes),
        }];
        std::fs::write(d.join("manifest.toml"), m.to_toml()).unwrap();
        m.map_id()
    }

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("live-mods-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_mod_resolves_to_its_patched_map_keyed_by_map_id_and_hash() {
        let root = scratch("ok");
        let maps = content::store::maps_dir(&root);
        let base = invented(1, 0x11, 8192);
        let mut edited = base.clone();
        // An extra vehicle, a moved spawn: any bytes the scenario holds.
        edited[0x1000..0x1010].copy_from_slice(&[0xab; 16]);
        let id = package(
            &maps,
            "lockout-hogs",
            "Lockout Hogs",
            "lockout",
            &base,
            &edited,
        );
        let mut asked = Vec::new();
        let found = installed(&root, &root.join("scratch"), &mut |name| {
            asked.push(name.to_string());
            (name == "lockout").then(|| Box::new(Cursor(base.clone())) as Box<dyn Source>)
        });
        assert!(found.notices.is_empty(), "{:?}", found.notices);
        assert_eq!(asked, ["lockout"]);
        let [m] = found.mods.as_slice() else {
            panic!("{:?}", found.mods)
        };
        assert_eq!(m.stem, format!("mod-{}", &id.to_string()[..16]));
        assert_eq!(m.info.map_id, id);
        assert_eq!(m.info.base, "lockout");
        assert_eq!(m.info.name, "Lockout Hogs");
        assert_eq!(
            m.info.sha256,
            Sha256::of(&edited),
            "the patched map's own hash"
        );
        assert_ne!(m.info.sha256, Sha256::of(&base));
        assert_eq!(std::fs::read(&m.path).unwrap(), edited);
        assert_eq!(m.size as usize, edited.len());
        assert_eq!(m.head, edited[..HEAD]);
        assert_eq!(
            crate::live_art::cache_type(&m.head),
            Some(map::CacheType::Multiplayer)
        );
        assert_eq!(package_ids(&root), [id], "named without resolving");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn two_mods_of_one_base_are_two_entries() {
        let root = scratch("two");
        let maps = content::store::maps_dir(&root);
        let base = invented(1, 0x22, 4096);
        let (mut a, mut b) = (base.clone(), base.clone());
        a[0x900] = 1;
        b[0x900] = 2;
        package(&maps, "a", "A", "midship", &base, &a);
        package(&maps, "b", "B", "midship", &base, &b);
        let found = installed(&root, &root.join("scratch"), &mut |_| {
            Some(Box::new(Cursor(base.clone())) as Box<dyn Source>)
        });
        assert_eq!(found.mods.len(), 2, "not one per map name");
        assert_ne!(found.mods[0].stem, found.mods[1].stem);
        assert_ne!(found.mods[0].info.sha256, found.mods[1].info.sha256);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_mod_whose_base_is_missing_or_different_is_left_out_with_a_notice() {
        let root = scratch("nobase");
        let maps = content::store::maps_dir(&root);
        let base = invented(1, 0x33, 4096);
        let mut edited = base.clone();
        edited[0x800] = 9;
        package(&maps, "gone", "Gone", "ascension", &base, &edited);
        let found = installed(&root, &root.join("scratch"), &mut |_| None);
        assert!(found.mods.is_empty());
        assert_eq!(found.notices.len(), 1, "{:?}", found.notices);
        assert!(found.notices[0].starts_with("mod gone (ascension): "));
        let other = invented(1, 0x44, 4096);
        let found = installed(&root, &root.join("scratch"), &mut |_| {
            Some(Box::new(Cursor(other.clone())) as Box<dyn Source>)
        });
        assert!(found.mods.is_empty(), "a base that is not the package's");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_maps_directory_is_no_mods() {
        let root = scratch("empty");
        let found = installed(&root, &root.join("scratch"), &mut |_| None);
        assert!(found.mods.is_empty() && found.notices.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_region_reads_only_its_range() {
        let root = scratch("region");
        let path = root.join("host");
        std::fs::write(&path, b"xxxxhello worldyyyy").unwrap();
        let mut r = Region::open(&path, 4, 11).unwrap();
        assert_eq!(r.size(), 11);
        assert_eq!(r.read_all().unwrap(), b"hello world");
        let mut buf = [0u8; 8];
        assert_eq!(r.read_at(6, &mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], b"world");
        assert_eq!(r.read_at(11, &mut buf).unwrap(), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Owned bytes as a [`Source`].
    struct Cursor(Vec<u8>);
    impl Source for Cursor {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            let mut s = &self.0[..];
            s.read_at(offset, buf)
        }
    }
}
