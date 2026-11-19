//! Opens maps inside the user's own disc image (or a converted Halo 1 map
//! under the runtime's data root, an installed mod, or any map file by
//! path) and reads raw resources from the current map or the disc's shared
//! caches. No game data is copied anywhere: a mod's patched map is built in
//! memory, as the game's package backend serves it (a BPS package goes
//! through the resolver's own cache, `<data>/maps/cache/`, as in the game).
use anyhow::{Context, Result, bail};
use content::patch::Source;
use map::{Map, ResourceFile};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Disc {
    path: PathBuf,
    partition: u64,
    entries: Vec<xiso::Entry>,
    /// Mods resolved so far, by package directory: the patched map's
    /// bytes, so opening the map and its resources resolves once.
    resolved: Mutex<HashMap<String, Arc<Vec<u8>>>>,
}

/// Where a map's bytes are read from: a file (the disc image, a map file)
/// or a mod's patched map in memory.
pub enum MapBytes {
    File(BufReader<File>),
    Memory(Cursor<Shared>),
}

/// Bytes shared between a map and its resource reader.
#[derive(Clone)]
pub struct Shared(pub Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl MapBytes {
    fn file(path: &Path) -> io::Result<MapBytes> {
        Ok(MapBytes::File(BufReader::new(File::open(path)?)))
    }
}

impl Read for MapBytes {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            MapBytes::File(f) => f.read(buf),
            MapBytes::Memory(m) => m.read(buf),
        }
    }
}

impl Seek for MapBytes {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            MapBytes::File(f) => f.seek(pos),
            MapBytes::Memory(m) => m.seek(pos),
        }
    }
}

/// Where a located map is.
enum Located {
    /// A file, and the map's first byte and length in it.
    File(PathBuf, u64, u64),
    /// A mod's patched map.
    Memory(Arc<Vec<u8>>),
}

/// The prefix that names an installed mod by its package directory.
pub const MOD_PREFIX: &str = "mod:";

/// An installed mod, as `--list` shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledMod {
    /// The package directory's name: open it as `mod:<dir>`.
    pub dir: String,
    pub name: String,
    /// The map it is played as.
    pub base: String,
    pub map_id: String,
}

impl Disc {
    pub fn open(path: &Path) -> Result<Self> {
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(path)?))
            .with_context(|| format!("open {}", path.display()))?;
        let entries = iso.entries()?;
        Ok(Self {
            path: path.to_owned(),
            partition: iso.partition_offset(),
            entries,
            resolved: Mutex::new(HashMap::new()),
        })
    }
    /// Map names (without `.map`) in on-disc order.
    /// Only files directly under `/maps/`, the same set `open_map` accepts.
    pub fn map_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| !e.is_dir)
            .filter_map(|e| map_name(&e.path))
            .map(str::to_owned)
            .collect()
    }
    fn region(&self, name: &str) -> Result<(u64, u64)> {
        let want = name.strip_suffix(".map").unwrap_or(name);
        let e = self
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .find(|e| map_name(&e.path).is_some_and(|n| n.eq_ignore_ascii_case(want)))
            .with_context(|| format!("no /maps/{want}.map on disc"))?;
        Ok((
            self.partition + u64::from(e.sector) * xiso::SECTOR,
            u64::from(e.size),
        ))
    }
    /// Where a map's bytes are: `mod:<dir>` an installed mod's patched map
    /// ([`Disc::resolve_mod`]); else on the disc by name; else a `.map` file
    /// by path; else a Halo 1 map `h2 import halo1` converted under the data
    /// root ([`converted`]); else an installed mod by its package directory.
    /// Maps from outside the disc read the disc's shared caches.
    fn locate(&self, name: &str) -> Result<Located> {
        if let Some(dir) = name.strip_prefix(MOD_PREFIX) {
            return Ok(Located::Memory(self.resolve_mod(dir)?));
        }
        if let Ok((base, len)) = self.region(name) {
            return Ok(Located::File(self.path.clone(), base, len));
        }
        let file = Path::new(name);
        let found = if file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("map"))
            && file.is_file()
        {
            Some(file.to_path_buf())
        } else {
            converted(&data_root(), name)
        };
        if let Some(path) = found {
            let len = std::fs::metadata(&path)?.len();
            return Ok(Located::File(path, 0, len));
        }
        if installed_mods(&data_root()).iter().any(|m| m.dir == name) {
            return Ok(Located::Memory(self.resolve_mod(name)?));
        }
        bail!(
            "no /maps/{name}.map on disc, no such map file, no converted map and no mod {name} under {} (see --list)",
            data_root().display()
        )
    }
    pub fn open_map(&self, name: &str) -> Result<Map<MapBytes>> {
        Ok(match self.locate(name)? {
            Located::File(path, base, len) => Map::from_region(MapBytes::file(&path)?, base, len)?,
            Located::Memory(bytes) => {
                let len = bytes.len() as u64;
                Map::from_region(MapBytes::Memory(Cursor::new(Shared(bytes))), 0, len)?
            }
        })
    }
    /// A raw-resource reader for a map opened with [`Disc::open_map`]: the
    /// local map plus whichever shared caches the disc carries.
    pub fn resources(&self, local: &str) -> Result<DiscResources> {
        let mut files = Vec::new();
        match self.locate(local)? {
            Located::File(path, base, len) => {
                files.push((ResourceFile::Local, MapBytes::file(&path)?, (base, len)))
            }
            Located::Memory(bytes) => {
                let len = bytes.len() as u64;
                files.push((
                    ResourceFile::Local,
                    MapBytes::Memory(Cursor::new(Shared(bytes))),
                    (0, len),
                ))
            }
        }
        for (file, name) in [
            (ResourceFile::MainMenu, "mainmenu"),
            (ResourceFile::Shared, "shared"),
            (ResourceFile::SinglePlayerShared, "single_player_shared"),
        ] {
            if let Ok(r) = self.region(name) {
                files.push((file, MapBytes::file(&self.path)?, r));
            }
        }
        Ok(DiscResources { files })
    }

    /// The installed mod in package directory `dir` (under
    /// `<data>/maps`), resolved as the game resolves it: its layers laid
    /// over the disc's map of its base (or the installed map pack's), the
    /// base checked against the package's SHA-256. Kept for the next ask.
    pub fn resolve_mod(&self, dir: &str) -> Result<Arc<Vec<u8>>> {
        self.resolve_mod_in(&data_root(), dir)
    }

    fn resolve_mod_in(&self, data: &Path, dir: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(b) = self.resolved.lock().unwrap().get(dir) {
            return Ok(b.clone());
        }
        let lib = content::Library::open(content::store::maps_dir(data));
        let package = lib
            .packages()
            .iter()
            .find(|p| p.dir_name() == dir)
            .with_context(|| {
                format!(
                    "no mod package {dir} under {} (see --list)",
                    content::store::maps_dir(data).display()
                )
            })?;
        let base_name = package.manifest.base.map.clone();
        let mut base: Box<dyn Source> = match self.region(&base_name) {
            Ok((at, len)) => Box::new(Region::open(&self.path, at, len)?),
            Err(_) => match map_pack_base(data, &base_name) {
                Some(path) => {
                    let len = std::fs::metadata(&path)?.len();
                    Box::new(Region::open(&path, 0, len)?)
                }
                // A derived map brings its own whole map; anything else
                // fails the base check below with the resolver's reason.
                None => Box::new(&[][..]),
            },
        };
        let resolved = lib
            .resolve_package(package, base.as_mut())
            .with_context(|| format!("resolving mod {dir} over {base_name}"))?;
        let bytes = Arc::new(resolved.read_all(base.as_mut())?);
        self.resolved
            .lock()
            .unwrap()
            .insert(dir.to_string(), bytes.clone());
        Ok(bytes)
    }
}

/// The installed mods under the data root `data`: map packages with patch
/// layers (converted Halo 1 maps, which `converted` opens, are left out).
pub fn installed_mods(data: &Path) -> Vec<InstalledMod> {
    let lib = content::Library::open(content::store::maps_dir(data));
    lib.packages()
        .iter()
        .filter(|p| p.list_key == p.id && !p.manifest.patches.is_empty())
        .map(|p| InstalledMod {
            dir: p.dir_name().to_string(),
            name: p.manifest.package.name.clone(),
            base: p.manifest.base.map.clone(),
            map_id: p.id.to_string(),
        })
        .collect()
}

/// Halo 2's title ID, whose content holds the installed map pack.
const HALO2_TITLE: u32 = 0x4D53_0064;

/// The map pack map `name` as `h2 import-mappack` installed it under the
/// data root: `<data>/disk/Partition1/TDATA/4D530064/$c/<offer>/<map>.map`
/// for an offer of the import's (`content::map_pack::is_offer`).
pub fn map_pack_base(data: &Path, name: &str) -> Option<PathBuf> {
    let content = data.join("disk/Partition1/TDATA/4D530064/$c");
    let mut offers: Vec<PathBuf> = std::fs::read_dir(&content)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            u64::from_str_radix(&n.to_string_lossy(), 16)
                .is_ok_and(|o| content::map_pack::is_offer(HALO2_TITLE, o))
        })
        .map(|e| e.path())
        .collect();
    offers.sort();
    offers.into_iter().find_map(|dir| {
        std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.is_file()
                    && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("map"))
                    && p.file_stem()
                        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(name))
            })
    })
}

/// A byte range of a file as a patch source (the base a mod is laid over).
struct Region {
    file: File,
    base: u64,
    len: u64,
}

impl Region {
    fn open(path: &Path, base: u64, len: u64) -> io::Result<Region> {
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

/// The runtime's data root, where `h2 import halo1` writes: `$H2_DATA`,
/// else `h2boot-data` in the temporary directory (as `h2 play` defaults).
pub fn data_root() -> PathBuf {
    std::env::var_os("H2_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("h2boot-data"))
}

/// A converted Halo 1 map under `data` by the name the game knows it by
/// (docs/halo1/campaign.md "Importing"): a campaign level at
/// `campaigns/halo1/<name>.map`, a multiplayer map at
/// `maps/<name>/h1/<name>.map` (the Halo 1 sandbox), else its Halo 2
/// sandbox's `h2/`. The level geometry is the same in both sandboxes.
pub fn converted(data: &Path, name: &str) -> Option<PathBuf> {
    let name = name.strip_suffix(".map").unwrap_or(name);
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let file = format!("{name}.map");
    [
        data.join("campaigns").join("halo1").join(&file),
        data.join("maps").join(name).join("h1").join(&file),
        data.join("maps").join(name).join("h2").join(&file),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Reads raw-resource byte ranges, bounded by each map file's extent.
pub struct DiscResources {
    files: Vec<(ResourceFile, MapBytes, (u64, u64))>,
}
impl tags::geometry::RawSource for DiscResources {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let (file, offset) = (pointer.file, pointer.offset);
        let Some((_, handle, (base, len))) = self.files.iter_mut().find(|(f, _, _)| *f == file)
        else {
            return Err(tags::Error::MissingResource(file));
        };
        if u64::from(offset) + u64::from(size) > *len {
            return Err(tags::Error::ResourceBounds {
                file,
                offset,
                size,
                length: *len,
            });
        }
        let mut buf = vec![0; size as usize];
        handle
            .seek(SeekFrom::Start(*base + u64::from(offset)))
            .and_then(|_| handle.read_exact(&mut buf))
            .map_err(|e| tags::Error::Map(map::Error::from(e)))?;
        Ok(buf)
    }
}

/// `--disc`, else `H2_DISC`, else `disc/halo2.iso` in the repository.
pub fn disc_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Some(p) = std::env::var_os("H2_DISC") {
        return Ok(p.into());
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso");
    if p.exists() {
        return Ok(p);
    }
    bail!("no disc: pass --disc <iso> or set H2_DISC")
}

/// `/maps/<name>.map` (case-insensitive, no deeper directories) -> `<name>`.
fn map_name(path: &str) -> Option<&str> {
    let rest = path
        .get(..6)?
        .eq_ignore_ascii_case("/maps/")
        .then(|| &path[6..])?;
    let cut = rest.len().checked_sub(4)?;
    if !rest.get(cut..)?.eq_ignore_ascii_case(".map") || rest.contains('/') || cut == 0 {
        return None;
    }
    Some(&rest[..cut])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// An invented map's bytes (not a cache file: the resolver leaves the
    /// patched bytes as the patch makes them).
    fn invented(fill: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| fill ^ (i as u8)).collect()
    }

    /// A package in `data`'s maps directory: `edited` as a patch over `base`.
    fn package(data: &Path, dir: &str, name: &str, base_map: &str, base: &[u8], edited: &[u8]) {
        use content::{Manifest, Sha256, manifest::Layer};
        let d = content::store::maps_dir(data).join(dir);
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
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("viewer-mods-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// A disc whose `/maps/<name>.map` is the whole of `file`.
    fn disc_of(file: &Path, name: &str, len: usize) -> super::Disc {
        super::Disc {
            path: file.to_path_buf(),
            partition: 0,
            entries: vec![xiso::Entry {
                path: format!("/maps/{name}.map"),
                sector: 0,
                size: len as u32,
                is_dir: false,
            }],
            resolved: Default::default(),
        }
    }

    #[test]
    fn a_mod_opens_as_its_patched_map_over_the_disc_base() {
        let root = scratch("disc");
        let base = invented(0x5a, 6000);
        let mut edited = base.clone();
        edited[0x400..0x408].copy_from_slice(b"newvehi!");
        std::fs::write(root.join("disc.iso"), &base).unwrap();
        package(
            &root,
            "lockout-hogs",
            "Lockout Hogs",
            "lockout",
            &base,
            &edited,
        );
        let disc = disc_of(&root.join("disc.iso"), "lockout", base.len());
        let bytes = disc.resolve_mod_in(&root, "lockout-hogs").unwrap();
        assert_eq!(*bytes, edited);
        // Kept: a second ask is the same bytes without resolving again.
        std::fs::remove_dir_all(content::store::maps_dir(&root)).unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &bytes,
            &disc.resolve_mod_in(&root, "lockout-hogs").unwrap()
        ));
        assert!(disc.resolve_mod_in(&root, "other").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_mod_of_a_map_pack_map_uses_the_installed_copy() {
        let root = scratch("pack");
        let base = invented(0x21, 5000);
        let mut edited = base.clone();
        edited[100] ^= 0xff;
        let offer = root.join("disk/Partition1/TDATA/4D530064/$c/4d53006400000102");
        std::fs::create_dir_all(&offer).unwrap();
        std::fs::write(offer.join("Turf.map"), &base).unwrap();
        // Not one of the import's offers: ignored.
        let stray = root.join("disk/Partition1/TDATA/4D530064/$c/4d53006400000999");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("warlock.map"), b"x").unwrap();
        assert_eq!(
            super::map_pack_base(&root, "turf"),
            Some(offer.join("Turf.map"))
        );
        assert_eq!(super::map_pack_base(&root, "warlock"), None);
        assert_eq!(super::map_pack_base(&root.join("none"), "turf"), None);
        package(&root, "turf-mod", "Turf Mod", "turf", &base, &edited);
        std::fs::write(root.join("disc.iso"), b"").unwrap();
        let disc = disc_of(&root.join("disc.iso"), "lockout", 0);
        assert_eq!(*disc.resolve_mod_in(&root, "turf-mod").unwrap(), edited);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_mod_whose_base_differs_is_refused() {
        let root = scratch("wrong");
        let base = invented(0x11, 4000);
        let mut edited = base.clone();
        edited[9] ^= 1;
        package(&root, "m", "M", "lockout", &base, &edited);
        std::fs::write(root.join("disc.iso"), invented(0x12, 4000)).unwrap();
        let disc = disc_of(&root.join("disc.iso"), "lockout", 4000);
        assert!(disc.resolve_mod_in(&root, "m").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn installed_mods_are_listed_with_their_base_and_map_id() {
        let root = scratch("list");
        let base = invented(1, 3000);
        let mut edited = base.clone();
        edited[0] = 0;
        edited[1] = 0;
        package(&root, "b-mod", "B", "midship", &base, &edited);
        package(&root, "a-mod", "A", "lockout", &base, &edited);
        let mods = super::installed_mods(&root);
        let dirs: Vec<&str> = mods.iter().map(|m| m.dir.as_str()).collect();
        assert_eq!(dirs, ["a-mod", "b-mod"]);
        assert_eq!(
            (mods[0].name.as_str(), mods[0].base.as_str()),
            ("A", "lockout")
        );
        assert_eq!(mods[0].map_id.len(), 64);
        assert_ne!(mods[0].map_id, mods[1].map_id);
        assert!(super::installed_mods(&root.join("none")).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn memory_maps_read_and_seek_like_files() {
        use std::io::{Read, Seek, SeekFrom};
        let bytes = super::Shared(std::sync::Arc::new(b"0123456789".to_vec()));
        let mut m = super::MapBytes::Memory(std::io::Cursor::new(bytes));
        m.seek(SeekFrom::Start(4)).unwrap();
        let mut b = [0u8; 3];
        m.read_exact(&mut b).unwrap();
        assert_eq!(&b, b"456");
    }
    #[test]
    fn converted_halo1_maps_are_found_under_the_data_root() {
        use super::converted;
        let root = std::env::temp_dir().join(format!("viewer-h1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let camp = root.join("campaigns/halo1");
        std::fs::create_dir_all(&camp).unwrap();
        std::fs::write(camp.join("x10.map"), b"x").unwrap();
        for s in ["h1", "h2"] {
            std::fs::create_dir_all(root.join("maps/h1_alpha").join(s)).unwrap();
            std::fs::write(
                root.join("maps/h1_alpha").join(s).join("h1_alpha.map"),
                b"x",
            )
            .unwrap();
        }
        std::fs::create_dir_all(root.join("maps/h1_beta/h2")).unwrap();
        std::fs::write(root.join("maps/h1_beta/h2/h1_beta.map"), b"x").unwrap();
        assert_eq!(converted(&root, "x10"), Some(camp.join("x10.map")));
        assert_eq!(converted(&root, "x10.map"), Some(camp.join("x10.map")));
        assert_eq!(
            converted(&root, "h1_alpha"),
            Some(root.join("maps/h1_alpha/h1/h1_alpha.map")),
            "the Halo 1 sandbox's first"
        );
        assert_eq!(
            converted(&root, "h1_beta"),
            Some(root.join("maps/h1_beta/h2/h1_beta.map"))
        );
        for bad in ["x20", "../x10", "", "maps/x"] {
            assert_eq!(converted(&root, bad), None, "{bad}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn map_names_come_only_from_the_maps_directory() {
        use super::map_name;
        assert_eq!(map_name("/maps/mainmenu.map"), Some("mainmenu"));
        assert_eq!(map_name("/MAPS/Shared.MAP"), Some("Shared"));
        for bad in [
            "/other/x.map",
            "/maps/sub/x.map",
            "/maps/.map",
            "/maps/x.mapx",
            "/x.map",
            "/maps/",
        ] {
            assert_eq!(map_name(bad), None, "{bad}");
        }
    }
}
