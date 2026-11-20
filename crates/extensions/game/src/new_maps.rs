//! Registers installed maps as multiplayer content offers. The game applies
//! each offer's `patch.lvl` to a free slot (docs/game/map-pack.md §8).
//! The VFS resolves the map at that offer's path on demand.
use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use anyhow::{Context, Result, anyhow, bail};
use content::{
    Library, MapId, Package,
    manifest::LANGUAGES,
    map_list::{self, BaseEntry, Entry},
};
use vfs::Backend;

pub const HALO2_TITLE: u32 = 0x4D53_0064;
/// Which package a directory holds: `source`, `map_id`, `map`.
pub const MARKER: &str = "h2recomp-map.toml";
const META: &str = "ContentMeta.xbx";
const LVL: &str = "patch.lvl";
/// FATX allows 42 bytes a name, so `<map>.map` leaves 38 for the map.
const MAX_MAP_NAME: usize = 42 - 4;
/// The Map Pack import's offers (docs/game/map-pack.md §5): `…0101` to `…03ff`.
const MAP_PACK_OFFERS: std::ops::RangeInclusive<u32> = 0x0101..=0x03ff;

/// What the install did.
#[derive(Debug, Default)]
pub struct Report {
    /// `(map name, offer ID, list map id)` of each package in place.
    pub installed: Vec<(String, u64, u32)>,
    /// Installed packages left out because no multiplayer slot was free.
    pub overflow: usize,
    /// The partition-relative paths to resolve at load time.
    pub mounts: Vec<(String, vfs::ContentMap)>,
    /// Package directories removed: their source is gone or moved.
    pub removed: Vec<PathBuf>,
    /// Packages left out, with why.
    pub skipped: Vec<String>,
    pub notices: Vec<String>,
    /// Converted maps with a variant per sandbox: the path 1.0 opens on
    /// the data partition and the two variant files, served by the choice
    /// made for the next game (`crate::h1_variants`).
    pub variants: Vec<(String, vfs::VariantPair)>,
}

impl Report {
    /// One line for the console, or none when there was nothing to do.
    pub fn line(&self) -> Option<String> {
        if self.installed.is_empty()
            && self.removed.is_empty()
            && self.skipped.is_empty()
            && self.overflow == 0
        {
            return None;
        }
        let names: Vec<&str> = self.installed.iter().map(|(n, ..)| n.as_str()).collect();
        let mut s = format!("new maps: {} in the map list", names.len());
        if !names.is_empty() {
            s += &format!(" ({})", names.join(", "));
        }
        if self.overflow != 0 {
            s += &format!(", {} mods not shown, list full", self.overflow);
        }
        if !self.removed.is_empty() {
            s += &format!(", {} removed", self.removed.len());
        }
        for k in &self.skipped {
            s += &format!("\nnew maps: skipped {k}");
        }
        Some(s)
    }
}

/// What the install needs from the game disc.
pub trait Disc {
    /// Whether the disc has `maps\<name>.map`.
    fn has_map(&mut self, name: &str) -> Result<bool>;
    /// The ISO region of the disc's own map, below any VFS overrides.
    fn map_file(&mut self, name: &str) -> Result<Option<vfs::HostFile>>;
    /// `mainmenu.map`'s multiplayer level list, one `0xc64`-byte element
    /// each.
    fn slots(&mut self) -> Result<Vec<Vec<u8>>>;
    /// The preview pixels of a level list element's bitmap, as
    /// [`map_list::PREVIEW_LEN`] bytes.
    fn preview(&mut self, slot: &[u8]) -> Result<Vec<u8>>;
}

/// Shared by the installer and the data partition, including after boot.
pub fn mounted_packages() -> vfs::ContentMaps {
    static MAPS: OnceLock<vfs::ContentMaps> = OnceLock::new();
    MAPS.get_or_init(vfs::ContentMaps::default).clone()
}

/// Install the offers and publish their load inputs to the VFS.
pub fn install_at_boot(data_root: &Path, disc: &Path) -> usize {
    static INSTALL: Mutex<()> = Mutex::new(());
    let _install = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let mut d = IsoDisc::new(disc);
    match install(data_root, &mut d) {
        Ok(r) => {
            for n in &r.notices {
                eprintln!("new maps: {n}");
            }
            crate::h1_variants::mp_variants().replace(r.variants.iter().cloned());
            if let Some(l) = r.line() {
                eprintln!("{l}");
            }
            mounted_packages().replace(r.mounts);
            r.overflow
        }
        Err(e) => {
            eprintln!("new maps: not installed: {e:#}");
            0
        }
    }
}

/// A package can update its own ID, or use a free slot. Reserve IDs of
/// both the disc and existing content so generated records do only the latter.
fn slot_plan(slots: &[Vec<u8>], content: &[Entry], keys: &[MapId]) -> (Vec<u32>, usize) {
    let mut taken: BTreeSet<_> = slots
        .iter()
        .filter_map(|slot| BaseEntry::from_slot(slot).map(|entry| entry.map_id))
        .collect();
    let free = slots
        .iter()
        .filter(|slot| slot.get(..4) == Some(&[0xff; 4]))
        .count();
    let used_by_content = content
        .iter()
        .filter(|entry| taken.insert(entry.map_id))
        .count();
    (
        map_list::list_ids(keys, &taken.into_iter().collect::<Vec<_>>()),
        free.saturating_sub(used_by_content),
    )
}

fn existing_content(root: &Path) -> Vec<Entry> {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !generated_offer(&e.file_name().to_string_lossy()))
        .filter_map(|e| {
            let bytes = std::fs::read(e.path().join(LVL)).ok()?;
            Entry::from_v2(bytes.get(..content::patch_lvl::V2_LEN)?).ok()
        })
        .collect()
}

fn generated_offer(name: &str) -> bool {
    parse_offer(name).is_some_and(|offer| {
        offer >> 32 == u64::from(HALO2_TITLE) && map_list::is_new_map_offer(offer as u32)
    })
}

fn base_file(data_root: &Path, disc: &mut dyn Disc, map: &str) -> Result<vfs::HostFile> {
    if let Some(base) = disc.map_file(map)? {
        return Ok(base);
    }
    let (_, _, path) = crate::map_bases::map_pack_file(data_root, map)
        .with_context(|| format!("the disc and Map Pack have no {map}.map"))?;
    Ok(vfs::HostFile {
        len: path.metadata()?.len(),
        path,
        offset: 0,
    })
}

/// The alias fits the cache header's 32-byte name and FATX. The hash
/// distinguishes packages even if the engine's 31-bit IDs collide.
fn alias(id: u32, package: MapId) -> String {
    format!("m-{id:08x}-{}", &package.to_string()[..20])
}

/// Installs one content record per package using the remaining slots.
/// Existing `active.toml` and source files are left in place.
pub fn install(data_root: &Path, disc: &mut dyn Disc) -> Result<Report> {
    let cfg = vfs::Config::new(data_root, HALO2_TITLE);
    let content_root = cfg.title_data_dir().join(vfs::CONTENT_DIR);
    let maps = crate::map_override::packages_dir(data_root);
    let mut report = Report::default();
    let mut keep = BTreeSet::new();
    if maps.is_dir() {
        let (store, _) = crate::setting_defs::load(data_root, &[]);
        let sandbox = crate::setting_defs::halo1_sandbox(&store);
        let mut library = Library::open_with_sandbox(&maps, Some(sandbox));
        report.notices.extend(library.take_notices());
        let packages = library.packages();
        if !packages.is_empty() {
            let slots = disc.slots().context("reading mainmenu.map's map list")?;
            let bases: Vec<_> = slots
                .iter()
                .filter_map(|s| BaseEntry::from_slot(s).map(|b| (b, s.as_slice())))
                .collect();
            let existing = existing_content(&content_root);
            let keys: Vec<_> = packages.iter().map(|p| p.list_key).collect();
            let (ids, free) = slot_plan(&slots, &existing, &keys);
            let existing_bases: Vec<_> = existing
                .iter()
                .map(|e| BaseEntry {
                    map_id: e.map_id,
                    map: e
                        .scenario
                        .rsplit('\\')
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    sort: e.sort,
                    flags: e.flags,
                    teams: e.teams,
                })
                .zip(&existing)
                .collect();
            let template = map_pack_meta(&content_root);
            let first_slot = bases.iter().min_by_key(|(b, _)| (b.sort, b.map_id));
            let mut frame: Option<Option<Vec<u8>>> = None;
            let mut name_counts = HashMap::new();
            for p in packages {
                *name_counts
                    .entry(p.manifest.base.map.to_ascii_lowercase())
                    .or_insert(0) += 1;
            }
            for (p, id) in packages.iter().zip(ids) {
                let base_name = &p.manifest.base.map;
                let base = if p.manifest.base.derived {
                    None
                } else {
                    match base_file(data_root, disc, base_name) {
                        Ok(base) => Some(base),
                        Err(e) => {
                            report.skipped.push(format!("{}: {e:#}", p.dir_name()));
                            continue;
                        }
                    }
                };
                let needs_alias = base.is_some()
                    || disc.has_map(base_name)?
                    || existing_bases
                        .iter()
                        .any(|(b, _)| b.map.eq_ignore_ascii_case(base_name))
                    || name_counts[&base_name.to_ascii_lowercase()] > 1;
                let alias = needs_alias.then(|| alias(id, p.id));
                let map = alias.as_deref().unwrap_or(base_name);
                if map.len() > MAX_MAP_NAME {
                    report
                        .skipped
                        .push(format!("{}: map name doesn't fit FATX", p.dir_name()));
                    continue;
                }
                if report.installed.len() == free {
                    report.overflow += 1;
                    continue;
                }
                let list_base = p.manifest.list.as_ref().and_then(|l| l.base.as_deref());
                let visual_base = list_base.unwrap_or(base_name);
                let disc_base = bases
                    .iter()
                    .find(|(b, _)| b.map.eq_ignore_ascii_case(visual_base));
                let pack_base = existing_bases
                    .iter()
                    .find(|(b, _)| b.map.eq_ignore_ascii_case(visual_base));
                if let Some(name) = list_base
                    && disc_base.is_none()
                    && pack_base.is_none()
                {
                    report.notices.push(format!(
                        "{}: [list] base {name} is absent; using defaults",
                        p.dir_name()
                    ));
                }
                let own_preview = p.manifest.package.image.is_some();
                if (own_preview || (disc_base.is_none() && pack_base.is_none())) && frame.is_none()
                {
                    frame = Some(first_slot.and_then(|(_, s)| disc.preview(s).ok()));
                }
                let mut fallback = || {
                    disc_base
                        .and_then(|(_, slot)| disc.preview(slot).ok())
                        .or_else(|| pack_base.map(|(_, record)| record.preview.clone()))
                };
                let preview = if own_preview {
                    match map_list::preview_for(&p.manifest, &p.dir, None) {
                        Ok(mut pixels) => {
                            if let Some(Some(frame)) = &frame {
                                map_list::with_frame(&mut pixels, frame);
                            }
                            pixels
                        }
                        Err(e) => {
                            report
                                .notices
                                .push(format!("{}: preview: {e}", p.dir_name()));
                            fallback().unwrap_or_else(|| map_list::generated_card(&p.id))
                        }
                    }
                } else {
                    fallback().unwrap_or_else(|| {
                        let mut pixels = map_list::generated_card(&p.id);
                        if let Some(Some(frame)) = &frame {
                            map_list::with_frame(&mut pixels, frame);
                        }
                        pixels
                    })
                };
                let base_entry = disc_base
                    .map(|(b, _)| b)
                    .or_else(|| pack_base.map(|(b, _)| b))
                    .or_else(|| first_slot.map(|(b, _)| b));
                let mut entry = Entry::for_manifest(&p.manifest, id, base_entry, preview);
                entry.scenario = format!("scenarios\\multi\\{map}\\{map}");
                match install_one(&cfg, p, map, &entry, template.as_deref()) {
                    Ok(offer) => {
                        keep.insert(offer);
                        let path = content_path(offer, map);
                        if alias.is_none()
                            && let Some(pair) = variant_pair(p)
                        {
                            report.variants.push((path.clone(), pair));
                        }
                        report.installed.push((map.to_owned(), offer, id));
                        report.mounts.push((
                            path,
                            vfs::ContentMap {
                                package: p.clone(),
                                base,
                                alias,
                            },
                        ));
                    }
                    Err(e) => report.skipped.push(format!("{}: {e:#}", p.dir_name())),
                }
            }
        }
    }
    if let Ok(dir) = std::fs::read_dir(&content_root) {
        for e in dir.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if !generated_offer(&name)
                || parse_offer(&name).is_some_and(|offer| keep.contains(&offer))
            {
                continue;
            }
            let path = e.path();
            remove(&path).with_context(|| format!("removing {}", path.display()))?;
            report.removed.push(path);
        }
    }
    Ok(report)
}

/// The data partition's path of a package's map, as 1.0 opens it
/// (`T:\$c\<offer>\<map>.map`, T: being `TDATA\<title>`).
pub fn content_path(offer: u64, map: &str) -> String {
    format!(
        "TDATA\\{HALO2_TITLE:08X}\\{}\\{}\\{map}.map",
        vfs::CONTENT_DIR,
        vfs::content_dir_name(offer)
    )
}

/// A converted map's two sandbox variants, when its package has both
/// files (docs/halo1/multiplayer.md): read from its manifest, each with
/// the `MapId` it has when loaded.
pub fn variant_pair(p: &Package) -> Option<vfs::VariantPair> {
    let text = std::fs::read_to_string(p.dir.join(content::manifest::MANIFEST_FILE)).ok()?;
    let full = content::Manifest::from_toml(&text).ok()?;
    let file = |sandbox: &str| {
        let v = full.variant(sandbox)?;
        let file = p.dir.join(content::manifest::relative(&v.file).ok()?);
        file.is_file().then(|| vfs::VariantFile {
            file,
            id: Some(full.select_variant(Some(sandbox)).map_id()),
        })
    };
    Some(vfs::VariantPair {
        h1: file("h1")?,
        h2: file("h2")?,
    })
}

fn parse_offer(name: &str) -> Option<u64> {
    (name.len() == 16)
        .then(|| u64::from_str_radix(name, 16).ok())
        .flatten()
}

fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// An installed Map Pack package's ContentMeta.xbx, if there is one.
fn map_pack_meta(content_root: &Path) -> Option<Vec<u8>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(content_root)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .and_then(parse_offer)
                .is_some_and(|o| {
                    o >> 32 == u64::from(HALO2_TITLE) && MAP_PACK_OFFERS.contains(&(o as u32))
                })
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.iter().find_map(|d| {
        std::fs::read(d.join(META))
            .ok()
            .filter(|m| m.len() >= 0x88 && m[0x14..0x18] == *b"XCSF")
    })
}

/// Writes `bytes` to `path` unless it already holds them: a temporary file
/// beside it, then a rename.
fn write_file(path: &Path, bytes: &[u8]) -> Result<bool> {
    if std::fs::read(path).is_ok_and(|have| have == bytes) {
        return Ok(false);
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(true)
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct Marker {
    source: String,
    map: String,
    map_id: MapId,
}

/// The full package identity of the offer whose record names `file`.
pub(super) fn package_id(dir: &Path, file: &str) -> Option<MapId> {
    let marker = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let marker: Marker = toml::from_str(&marker).ok()?;
    marker.map.eq_ignore_ascii_case(file).then_some(marker.map_id)
}

/// Write the data the retail content scan consumes. The .map entry is
/// a directory placeholder, with the actual bytes resolved by the VFS.
fn install_one(
    cfg: &vfs::Config,
    p: &Package,
    map: &str,
    entry: &Entry,
    template: Option<&[u8]>,
) -> Result<u64> {
    let offer = vfs::offer_id(HALO2_TITLE, map_list::offer_content(entry.map_id));
    let lvl = entry.to_v2()?;
    let dir = cfg.content_dir(offer);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let map_name = format!("{map}.map");
    let map_file = dir.join(&map_name);
    // Retire whole maps materialised by previous versions. Only files
    // in this generated offer are affected.
    for e in std::fs::read_dir(&dir)?.flatten() {
        if e.file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".map")
            && (e.file_name() != std::ffi::OsStr::new(&map_name) || e.metadata()?.len() != 0)
        {
            remove(&e.path())?;
        }
    }
    write_file(&map_file, &[])?;
    let mut meta = content_meta(&entry.names, offer, template);
    kernel::content::sign(&mut meta, HALO2_TITLE).map_err(|e| anyhow!("signing: {e}"))?;
    write_file(&dir.join(META), &meta)?;
    let marker = Marker {
        source: p.dir_name().to_owned(),
        map: map.to_owned(),
        map_id: p.id,
    };
    write_file(&dir.join(MARKER), toml::to_string(&marker)?.as_bytes())?;
    write_file(&dir.join(LVL), &lvl)?;
    Ok(offer)
}

// ---------------------------------------------------------------------------
// ContentMeta.xbx (docs/game/map-pack.md §7).

const META_HEADER: usize = 0x88;
/// The INI sections the Map Pack's names use, for our languages 0..8
/// (Portuguese has none there).
const META_SECTIONS: [(&str, usize); 8] = [
    ("EN", 0),
    ("JA", 1),
    ("DE", 2),
    ("FR", 3),
    ("ES", 4),
    ("IT", 5),
    ("KO", 6),
    ("TW", 7),
];

/// A ContentMeta.xbx for the package with `offer`, unsigned:
///
/// | offset | field |
/// |---|---|
/// | `0x14` | `XCSF`, header size `0x88` |
/// | `0x1c` | 1 (as in the Map Pack's; the template's word when given) |
/// | `0x20` | flags, 0 until signed |
/// | `0x24` | title ID, then the offer ID at `0x28` |
/// | `0x30` | 1 (likewise) |
/// | `0x34` | names: offset, size, SHA-1 |
/// | `0x50` | file table: offset, size, SHA-1 |
///
/// The names are UTF-16 INI text (`[Default]`, then a section a language,
/// `Name=`). The file table lists no files: 1.0 hashes none of a package's
/// files (docs/game/map-pack.md 8.4); the table only has to match its digest.
/// With `template` (an installed Map Pack package's) its words at `0x1c`,
/// `0x30` and `0x6c..0x88` are kept.
pub fn content_meta(names: &[String; 9], offer: u64, template: Option<&[u8]>) -> Vec<u8> {
    let mut text = format!("\u{feff}[Default]\r\nName={}\r\n", clean(&names[0]));
    for (section, lang) in META_SECTIONS {
        text += &format!("[{section}]\r\nName={}\r\n", clean(&names[lang]));
    }
    debug_assert_eq!(LANGUAGES.len(), 9);
    let ini: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let table: Vec<u8> = [0u32, 0, 12].iter().flat_map(|w| w.to_le_bytes()).collect();
    let names_at = META_HEADER;
    let table_at = (names_at + ini.len()).next_multiple_of(32);
    let mut m = vec![0u8; table_at + table.len()];
    let put = |m: &mut Vec<u8>, at: usize, v: u32| m[at..at + 4].copy_from_slice(&v.to_le_bytes());
    m[0x14..0x18].copy_from_slice(b"XCSF");
    put(&mut m, 0x18, META_HEADER as u32);
    put(&mut m, 0x1c, 1);
    put(&mut m, 0x24, HALO2_TITLE);
    m[0x28..0x30].copy_from_slice(&offer.to_le_bytes());
    put(&mut m, 0x30, 1);
    if let Some(t) = template.filter(|t| t.len() >= META_HEADER) {
        m[0x1c..0x20].copy_from_slice(&t[0x1c..0x20]);
        m[0x30..0x34].copy_from_slice(&t[0x30..0x34]);
        m[0x6c..META_HEADER].copy_from_slice(&t[0x6c..META_HEADER]);
    }
    put(&mut m, 0x34, names_at as u32);
    put(&mut m, 0x38, ini.len() as u32);
    m[0x3c..0x50].copy_from_slice(&sha1(&ini));
    put(&mut m, 0x50, table_at as u32);
    put(&mut m, 0x54, table.len() as u32);
    m[0x58..0x6c].copy_from_slice(&sha1(&table));
    m[names_at..names_at + ini.len()].copy_from_slice(&ini);
    m[table_at..].copy_from_slice(&table);
    m
}

fn clean(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).collect()
}

fn sha1(b: &[u8]) -> [u8; 20] {
    let mut h = kernel::crypto::Sha1::new();
    h.update(b);
    h.finish()
}

// ---------------------------------------------------------------------------
// The game disc.

const MATG_LEVELS: usize = 0x178;
const LEVEL_DATA_MP: usize = 0x10;
const SLOT_BITMAP: usize = 0x8;
const MAX_LEVELS: u32 = 256;

/// The game disc (image or extracted folder), opened on first use.
pub struct IsoDisc {
    path: PathBuf,
    disc: Option<vfs::AnyDisc>,
    previews: HashMap<u32, Vec<u8>>,
}

impl IsoDisc {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            disc: None,
            previews: HashMap::new(),
        }
    }

    fn disc(&mut self) -> Result<&mut vfs::AnyDisc> {
        if self.disc.is_none() {
            self.disc = Some(vfs::open_disc(&self.path).context("opening the disc")?);
        }
        Ok(self.disc.as_mut().unwrap())
    }

    fn region(&mut self, name: &str) -> Result<assets::Region> {
        let file = self
            .map_file(name)?
            .with_context(|| format!("the disc has no {name}.map"))?;
        Ok(assets::Region {
            path: file.path,
            base: file.offset,
            length: file.len,
        })
    }

    fn mainmenu(&mut self) -> Result<map::Map<BufReader<File>>> {
        let r = self.region("mainmenu")?;
        map::Map::from_region(BufReader::new(File::open(&r.path)?), r.base, r.length)
            .context("parsing mainmenu.map")
    }
}

impl Disc for IsoDisc {
    fn has_map(&mut self, name: &str) -> Result<bool> {
        Ok(self.map_file(name)?.is_some())
    }

    fn map_file(&mut self, name: &str) -> Result<Option<vfs::HostFile>> {
        Ok(self.disc()?.host_file(&format!("maps\\{name}.map")))
    }

    fn slots(&mut self) -> Result<Vec<Vec<u8>>> {
        let mut m = self.mainmenu()?;
        let globals = m
            .globals()
            .map(|t| t.id)
            .context("mainmenu.map has no globals tag")?;
        let matg = m.read_tag_bytes(globals)?;
        let mut read = |addr: u32, size: u32| -> Result<Vec<u8>> {
            let off = m.file_offset(addr, size)?;
            Ok(m.read_file_range(off, size.into())?)
        };
        let le = |b: &[u8], at: usize| -> Result<u32> {
            Ok(u32::from_le_bytes(
                b.get(at..at + 4)
                    .context("short block")?
                    .try_into()
                    .unwrap(),
            ))
        };
        let (n, addr) = (le(&matg, MATG_LEVELS)?, le(&matg, MATG_LEVELS + 4)?);
        if n == 0 {
            bail!("no UI level data");
        }
        let head = read(addr, (LEVEL_DATA_MP + 8) as u32)?;
        let (count, at) = (le(&head, LEVEL_DATA_MP)?, le(&head, LEVEL_DATA_MP + 4)?);
        if count == 0 || count > MAX_LEVELS {
            bail!("a multiplayer level list of {count}");
        }
        let all = read(at, count * map_list::SLOT_LEN as u32)?;
        Ok(all
            .as_chunks::<{ map_list::SLOT_LEN }>()
            .0
            .iter()
            .map(|c| c.to_vec())
            .collect())
    }

    fn preview(&mut self, slot: &[u8]) -> Result<Vec<u8>> {
        let id = u32::from_le_bytes(
            slot.get(SLOT_BITMAP..SLOT_BITMAP + 4)
                .context("short slot")?
                .try_into()
                .unwrap(),
        );
        if let Some(px) = self.previews.get(&id) {
            return Ok(px.clone());
        }
        let mut files = assets::ResourceFiles::new();
        let menu = self.region("mainmenu")?;
        files.set(map::ResourceFile::Local, menu.clone())?;
        files.set(map::ResourceFile::MainMenu, menu)?;
        files.set(map::ResourceFile::Shared, self.region("shared")?)?;
        let mut m = self.mainmenu()?;
        let tags::TagData::Bitmap(b) = tags::DecodedTag::read(&mut m, map::TagId(id))?.data else {
            bail!("the level's picture {id:#x} is not a bitmap");
        };
        let image = b.images.first().context("a bitmap with no images")?;
        let d = assets::decode_image(image, &mut files)?;
        let l = d
            .levels
            .into_iter()
            .find(|l| l.level == 0 && l.face == 0)
            .context("no base level")?;
        let px = map_list::preview_from_rgba(
            l.width as usize,
            l.height as usize,
            &l.rgba[..(l.width * l.height * 4) as usize],
        );
        self.previews.insert(id, px.clone());
        Ok(px)
    }
}

#[cfg(test)]
mod tests;
