//! New maps in 1.0's multiplayer map list (docs/game/maps.md §4).
//!
//! A map package whose map name the disc doesn't have (a derived map,
//! `[base] derived = true`) can't replace a disc map, so it is installed the
//! way the Map Pack is on 1.0 (docs/game/map-pack.md §5, §8): as a content
//! package of its own under `T:\$c\<offer id>\`, holding
//!
//! - `<map>.map`, the map materialised through the content resolver (1.0
//!   opens it from the package directory, not `\maps`), written once and
//!   again only when the package's `MapId` changes;
//! - `patch.lvl`, a version 2 record built by [`content::map_list`];
//! - `ContentMeta.xbx`, synthesised (in the Map Pack's shape when it is
//!   installed) and signed with [`kernel::content::sign`];
//! - the package's image, if it has one, under its own name;
//! - [`MARKER`], which source package and `MapId` the directory holds.
//!
//! Offer IDs are Halo 2's title ID over [`content::map_list::offer_content`]
//! of the map's list id, so they never meet the Map Pack's. A directory in
//! that range whose source package is gone is removed. With no such
//! packages nothing is read or written.
use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use content::{
    manifest::LANGUAGES,
    map_list::{self, BaseEntry, Entry},
    patch::Source,
    Library, MapId, Package,
};

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
    /// Maps written (materialised) this time.
    pub materialised: Vec<String>,
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
        if self.installed.is_empty() && self.removed.is_empty() && self.skipped.is_empty() {
            return None;
        }
        let names: Vec<&str> = self.installed.iter().map(|(n, ..)| n.as_str()).collect();
        let mut s = format!("new maps: {} in the map list", names.len());
        if !names.is_empty() {
            s += &format!(" ({})", names.join(", "));
        }
        if !self.materialised.is_empty() {
            s += &format!(", {} written", self.materialised.len());
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
    /// `mainmenu.map`'s multiplayer level list, one `0xc64`-byte element
    /// each.
    fn slots(&mut self) -> Result<Vec<Vec<u8>>>;
    /// The preview pixels of a level list element's bitmap, as
    /// [`map_list::PREVIEW_LEN`] bytes.
    fn preview(&mut self, slot: &[u8]) -> Result<Vec<u8>>;
}

/// Installs at boot: never fails, prints what it did.
pub fn install_at_boot(data_root: &Path, disc: &Path) {
    let mut d = IsoDisc::new(disc);
    match install(data_root, &mut d) {
        Ok(r) => {
            for n in &r.notices {
                eprintln!("new maps: {n}");
            }
            let table = crate::h1_variants::mp_variants();
            table.clear();
            for (path, pair) in &r.variants {
                table.register(path, pair.clone());
            }
            if !r.variants.is_empty() {
                eprintln!(
                    "new maps: {} converted map(s) follow the sandbox chosen per game",
                    r.variants.len()
                );
            }
            if let Some(l) = r.line() {
                eprintln!("{l}");
            }
        }
        Err(e) => eprintln!("new maps: not installed: {e:#}"),
    }
}

/// Installs every new map package under `<data_root>/maps` into
/// `<data_root>/disk/Partition1/TDATA/4D530064/$c`, and removes the ones
/// whose source is gone.
pub fn install(data_root: &Path, disc: &mut dyn Disc) -> Result<Report> {
    let cfg = vfs::Config::new(data_root, HALO2_TITLE);
    let content_root = cfg.title_data_dir().join(vfs::CONTENT_DIR);
    let mut report = Report::default();
    let maps = crate::map_override::packages_dir(data_root);
    let mut chosen: Vec<Package> = Vec::new();
    let mut library = None;
    if maps.is_dir() {
        // Converted Halo 1 maps: the sandbox the setting names.
        let (store, _) = crate::setting_defs::load(data_root, &[]);
        let sandbox = crate::setting_defs::halo1_sandbox(&store);
        let mut lib = Library::open_with_sandbox(&maps, Some(sandbox));
        let names: BTreeSet<String> = lib
            .packages()
            .iter()
            .filter(|p| p.manifest.base.derived)
            .map(|p| p.manifest.base.map.to_ascii_lowercase())
            .collect();
        for name in names {
            if disc.has_map(&name)? {
                continue;
            }
            if name.len() > MAX_MAP_NAME {
                report.skipped.push(format!(
                    "{name}: a map name over {MAX_MAP_NAME} bytes doesn't fit FATX"
                ));
                continue;
            }
            if let Some(p) = lib
                .choose(&name)
                .cloned()
                .filter(|p| p.manifest.base.derived)
            {
                chosen.push(p);
            }
        }
        report.notices.extend(lib.take_notices());
        library = Some(lib);
    }
    let mut keep = BTreeSet::new();
    if let (false, Some(lib)) = (chosen.is_empty(), library.as_ref()) {
        let slots = disc.slots().context("reading mainmenu.map's map list")?;
        let bases: Vec<(BaseEntry, &Vec<u8>)> = slots
            .iter()
            .filter_map(|s| BaseEntry::from_slot(s).map(|b| (b, s)))
            .collect();
        let taken: Vec<u32> = bases.iter().map(|(b, _)| b.map_id).collect();
        // A converted map's list ID is the same in both sandboxes
        // (`Package::list_key`).
        let ids = map_list::list_ids(&chosen.iter().map(|p| p.list_key).collect::<Vec<_>>(), &taken);
        let template = map_pack_meta(&content_root);
        // Without a base of its own, a map takes its flags and team counts
        // from the list's first retail map (lowest sort order): what the
        // user's own disc has, never a table of ours.
        let first = bases
            .iter()
            .min_by_key(|(b, _)| (b.sort, b.map_id))
            .map(|(b, _)| b);
        for (p, id) in chosen.iter().zip(ids) {
            let base = p
                .manifest
                .list
                .as_ref()
                .and_then(|l| l.base.as_deref())
                .map(|want| {
                    let found = bases.iter().find(|(b, _)| b.map.eq_ignore_ascii_case(want));
                    if found.is_none() {
                        report.notices.push(format!(
                        "{}: [list] base {want} is not in mainmenu.map's map list; using defaults",
                        p.dir_name()
                    ));
                    }
                    found
                });
            let base = base.flatten().map(|(b, s)| (b, s.as_slice()));
            match install_one(lib, &cfg, p, id, base, first, disc, template.as_deref()) {
                Ok((offer, wrote)) => {
                    keep.insert(offer);
                    if let Some(pair) = variant_pair(p) {
                        report.variants.push((content_path(offer, &p.manifest.base.map), pair));
                    }
                    if wrote {
                        report.materialised.push(p.manifest.base.map.clone());
                    }
                    report
                        .installed
                        .push((p.manifest.base.map.clone(), offer, id));
                }
                Err(e) => report.skipped.push(format!("{}: {e:#}", p.dir_name())),
            }
        }
    }
    if let Ok(dir) = std::fs::read_dir(&content_root) {
        for e in dir.flatten() {
            let Some(offer) = e.file_name().to_str().and_then(parse_offer) else {
                continue;
            };
            if offer >> 32 != u64::from(HALO2_TITLE)
                || !map_list::is_new_map_offer(offer as u32)
                || keep.contains(&offer)
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

/// A disc source for derived maps, which never read the disc.
struct NoDisc;
impl Source for NoDisc {
    fn size(&self) -> u64 {
        0
    }
    fn read_at(&mut self, _: u64, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("a derived map reads no disc map"))
    }
}

/// One package. Returns its offer ID and whether the map was written.
#[allow(clippy::too_many_arguments)]
fn install_one(
    lib: &Library,
    cfg: &vfs::Config,
    p: &Package,
    id: u32,
    base: Option<(&BaseEntry, &[u8])>,
    first: Option<&BaseEntry>,
    disc: &mut dyn Disc,
    template: Option<&[u8]>,
) -> Result<(u64, bool)> {
    let map = p.manifest.base.map.clone();
    let offer = vfs::offer_id(HALO2_TITLE, map_list::offer_content(id));
    let dir = cfg.content_dir(offer);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let marker = Marker {
        source: p.dir_name().to_owned(),
        map: map.clone(),
        map_id: p.id,
    };
    let had: Option<Marker> = std::fs::read_to_string(dir.join(MARKER))
        .ok()
        .and_then(|t| toml::from_str(&t).ok());
    let map_file = dir.join(format!("{map}.map"));
    let fresh = had
        .as_ref()
        .is_some_and(|m| m.map_id == p.id && m.map == map)
        && map_file.is_file();
    if !fresh {
        // The marker goes first, so a half-written directory is never taken
        // as up to date.
        let _ = std::fs::remove_file(dir.join(MARKER));
        for e in std::fs::read_dir(&dir)?.flatten() {
            if e.file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".map")
            {
                remove(&e.path())?;
            }
        }
        let resolved = lib
            .resolve_package(p, &mut NoDisc)
            .map_err(|e| anyhow!("resolving: {e}"))?;
        let bytes = resolved
            .read_all(&mut NoDisc)
            .context("reading the resolved map")?;
        write_file(&map_file, &bytes)?;
        // Stored once: a map the resolver materialised in its cache lives
        // here instead.
        let cached = match &resolved.data {
            content::Data::File { path, .. }
                if path.starts_with(lib.root().join(content::CACHE_DIR)) =>
            {
                Some(path.clone())
            }
            _ => None,
        };
        drop(resolved);
        if let Some(path) = cached {
            let _ = std::fs::remove_file(path);
        }
    }
    let base_preview = match (&p.manifest.package.image, base) {
        (None, Some((_, slot))) => match disc.preview(slot) {
            Ok(px) => Some(px),
            Err(e) => {
                eprintln!("new maps: {map}: base preview: {e:#}; using a plain card");
                None
            }
        },
        _ => None,
    };
    let preview = map_list::preview_for(&p.manifest, &p.dir, base_preview)
        .map_err(|e| anyhow!("preview: {e}"))?;
    let entry = Entry::for_manifest(&p.manifest, id, base.map(|(b, _)| b).or(first), preview);
    write_file(&dir.join(LVL), &entry.to_v2()?)?;
    let mut meta = content_meta(&entry.names, offer, template);
    kernel::content::sign(&mut meta, HALO2_TITLE).map_err(|e| anyhow!("signing: {e}"))?;
    write_file(&dir.join(META), &meta)?;
    if let Some(image) = &p.manifest.package.image {
        let name = Path::new(image).file_name().context("image name")?;
        let bytes = std::fs::read(p.dir.join(content::manifest::relative(image)?))?;
        write_file(&dir.join(name), &bytes)?;
    }
    write_file(&dir.join(MARKER), toml::to_string(&marker)?.as_bytes())?;
    Ok((offer, !fresh))
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

/// The game disc as an XISO, opened on first use.
pub struct IsoDisc {
    path: PathBuf,
    entries: Option<(u64, Vec<xiso::Entry>)>,
    previews: HashMap<u32, Vec<u8>>,
}

impl IsoDisc {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            entries: None,
            previews: HashMap::new(),
        }
    }

    fn entries(&mut self) -> Result<&(u64, Vec<xiso::Entry>)> {
        if self.entries.is_none() {
            let f = File::open(&self.path)
                .with_context(|| format!("opening {}", self.path.display()))?;
            let mut iso = xiso::Xiso::open(BufReader::new(f)).context("reading the disc")?;
            self.entries = Some((iso.partition_offset(), iso.entries()?));
        }
        Ok(self.entries.as_ref().unwrap())
    }

    fn map_entry(&mut self, name: &str) -> Result<Option<(u64, xiso::Entry)>> {
        let want = format!("/maps/{name}.map");
        let (base, entries) = self.entries()?;
        Ok(entries
            .iter()
            .find(|e| !e.is_dir && e.path.eq_ignore_ascii_case(&want))
            .map(|e| (*base, e.clone())))
    }

    fn region(&mut self, name: &str) -> Result<assets::Region> {
        let (base, e) = self
            .map_entry(name)?
            .with_context(|| format!("the disc has no {name}.map"))?;
        Ok(assets::Region {
            path: self.path.clone(),
            base: base + u64::from(e.sector) * xiso::SECTOR,
            length: e.size.into(),
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
        Ok(self.map_entry(name)?.is_some())
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
