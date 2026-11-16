//! `h2 import-mappack`: install the Halo 2 Multiplayer Map Pack disc's
//! maps into the emulated hard disk so 1.0 lists them, and keep its title
//! update aside.
//!
//! 1.0 reads one `patch.lvl` record per content package, so each map gets a
//! package of its own: the map, a version 2 `patch.lvl` made from its
//! `patch_v3.lvl` record (`patch_lvl`), and the disc package's
//! `ContentMeta.xbx` with the new offer ID, signed for this console as the
//! disc's installer does on a real one. Nothing else is read from the game
//! beyond the disc's directory tree and, for the update, the XBE
//! certificate. The layout and what the game does with it are in
//! docs/game/map-pack.md, sections 5 and 8.
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use xiso::{Entry, Xiso};

use content::patch_lvl;
use crate::safe_out::OutDir;

const HALO2_TITLE: u32 = 0x4D53_0064;
/// Where the disc keeps its content packages, one directory per package.
const DLC_DIR: &str = "/demos/dlc/";
/// Where the disc keeps one signed copy of the update per base disc version.
const UPDATE_DIR: &str = "/demos/update/Updates/";
const UPDATE_STEM: &str = "default.xbe.";
/// FATX allows 42 bytes per name (docs/runtime/storage.md).
const FATX_NAME_MAX: usize = 42;
const CONTENT_META: &str = "contentmeta.xbx";
/// The name an installed package's metadata is written under.
const CONTENT_META_NAME: &str = "ContentMeta.xbx";
/// The map list entry 1.0 reads: written by us, never copied from the disc.
const PATCH_V2: &str = "patch.lvl";
/// The update's map list, one record per map in the disc package.
const PATCH_V3: &str = "patch_v3.lvl";
/// 0x2d400 bytes a record; a disc package holds a handful.
const PATCH_V3_MAX: u32 = 64 << 20;
/// ContentMeta.xbx is a header, names and a file table: about 1 KB on the
/// map pack. Anything far bigger is not one.
const CONTENT_META_MAX: u32 = 1 << 20;

const USAGE: &str = "usage: h2 import-mappack <mappack.iso> [--data DIR] [--disc <halo2.iso|default.xbe>]
                       [--no-update] [--force] [--dry-run]

Installs the Multiplayer Map Pack's nine maps under the emulated hard disk, one
content package per map (<data>/disk/Partition1/TDATA/4D530064/$c/<offer id>/)
so Halo 2 1.0 lists them, and copies the title update that matches your Halo 2
disc to <data>/updates/4D530064/<version>/.
The update is never placed where the game would launch it. --data defaults to
$TMPDIR/h2boot-data, as for `h2 play`. --disc defaults to $H2_DISC, then the
first .iso in disc/. See docs/game/map-pack.md.";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub iso: PathBuf,
    pub data: Option<PathBuf>,
    pub disc: Option<PathBuf>,
    pub update: bool,
    pub force: bool,
    pub dry_run: bool,
}

pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut iso, mut data, mut disc) = (None, None, None);
    let (mut update, mut force, mut dry_run) = (true, false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => data = Some(PathBuf::from(it.next().ok_or("--data needs a directory")?)),
            "--disc" => disc = Some(PathBuf::from(it.next().ok_or("--disc needs a path")?)),
            "--no-update" => update = false,
            "--force" => force = true,
            "--dry-run" => dry_run = true,
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if iso.is_none() => iso = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    let iso = iso.ok_or("missing the map pack .iso")?;
    Ok(Args { iso, data, disc, update, force, dry_run })
}

/// One content package on the disc.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// The disc directory's name: 8 hex digits.
    pub dir: String,
    /// That name's value: the package number.
    pub number: u32,
    pub files: Vec<Entry>,
}

/// What the disc holds, found from its directory tree alone.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub packages: Vec<Package>,
    /// `default.xbe.NN` copies, by NN (the base disc's certificate version).
    pub updates: Vec<(u32, Entry)>,
}

impl Plan {
    pub fn from_entries(entries: &[Entry]) -> Result<Self> {
        let mut packages: Vec<Package> = Vec::new();
        let mut updates = Vec::new();
        for e in entries {
            let lower = e.path.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix(DLC_DIR) {
                let parts: Vec<&str> = rest.split('/').collect();
                let dir = &e.path[DLC_DIR.len()..DLC_DIR.len() + parts[0].len()];
                let number = match u32::from_str_radix(dir, 16) {
                    Ok(n) if dir.len() == 8 => n,
                    // maps.png and the like sit beside the packages.
                    _ => continue,
                };
                match parts.as_slice() {
                    [_] if e.is_dir => packages.push(Package { dir: dir.to_string(), number, files: Vec::new() }),
                    [_, name] if !e.is_dir => {
                        check_fatx_name(name, &e.path)?;
                        let pkg = packages
                            .iter_mut()
                            .find(|p| p.dir == dir)
                            .with_context(|| format!("{} is outside a listed package", e.path))?;
                        pkg.files.push(e.clone());
                    }
                    _ => bail!("unexpected entry {} in a content package", e.path),
                }
            } else if let Some(rest) = lower.strip_prefix(&UPDATE_DIR.to_ascii_lowercase())
                && let Some(n) = rest.strip_prefix(UPDATE_STEM)
                && let (Ok(n), false) = (n.parse::<u32>(), e.is_dir)
            {
                updates.push((n, e.clone()));
            }
        }
        packages.retain(|p| !p.files.is_empty());
        if packages.is_empty() {
            bail!("no content packages under {DLC_DIR}: is this the Multiplayer Map Pack disc?");
        }
        for p in &packages {
            let has_meta = p.files.iter().any(is_content_meta);
            if !has_meta {
                bail!("package {} has no ContentMeta.xbx", p.dir);
            }
        }
        updates.sort_by_key(|(n, _)| *n);
        Ok(Self { packages, updates })
    }

    /// The update copy signed for a base disc with this certificate version.
    pub fn update_for(&self, base_version: u32) -> Option<&Entry> {
        self.updates.iter().find(|(n, _)| *n == base_version & 0xff).map(|(_, e)| e)
    }
}

fn file_name(file: &Entry) -> &str {
    file.path.rsplit('/').next().unwrap_or_default()
}

fn is_content_meta(file: &Entry) -> bool {
    file_name(file).eq_ignore_ascii_case(CONTENT_META)
}

fn is_named(file: &Entry, name: &str) -> bool {
    file_name(file).eq_ignore_ascii_case(name)
}

fn is_map(file: &Entry) -> bool {
    file_name(file).to_ascii_lowercase().ends_with(".map")
}

fn check_fatx_name(name: &str, path: &str) -> Result<()> {
    if name.is_empty() || name.len() > FATX_NAME_MAX || !name.is_ascii() {
        bail!("{path}: name is not a valid FATX name");
    }
    Ok(())
}

/// The offer ID of the package that carries record `index` (from 0) of disc
/// package `number`'s `patch_v3.lvl`: Halo 2's title ID in the high half,
/// `number << 8 | (index + 1)` in the low half. So disc package 1's two maps
/// are `4d53006400000101` and `...0102`, package 3's five are `...0301` to
/// `...0305`. Never the bare package number an older import used.
pub fn map_offer(number: u32, index: usize) -> Result<u64> {
    if number == 0 || number > 0x00ff_ffff || index >= 0xff {
        bail!("package {number:08x} record {index} has no offer ID in this scheme");
    }
    Ok(vfs::offer_id(HALO2_TITLE, number << 8 | (index as u32 + 1)))
}

/// One installed package: one map, served to 1.0 with its own `patch.lvl`.
#[derive(Debug, Clone, PartialEq)]
pub struct MapPackage {
    /// The disc package it comes from.
    pub source: String,
    pub offer: u64,
    /// The map file on the disc, installed under its own name.
    pub map: Entry,
    /// The record's map id: the map list slot it fills.
    pub map_id: u32,
    /// The disc's ContentMeta.xbx with this package's offer ID, signed.
    pub meta: Vec<u8>,
    /// Version 2, one record, made from the map's `patch_v3.lvl` record.
    pub patch_lvl: Vec<u8>,
    /// Other files carried over byte for byte (ContentImage.xbx).
    pub extras: Vec<Entry>,
}

/// Where the offer ID sits in a ContentMeta.xbx header (docs/game/map-pack.md 7).
const META_OFFER: usize = 0x28;

/// A package's ContentMeta.xbx as this console's installer writes it, for
/// the package with `offer`: the disc's copy carries an all-zero signature
/// and XAPI refuses it until it is signed with the console's hard disk key
/// (`kernel::content`). The offer ID is inside the signed header, so it is
/// changed first. The file table and its digest stay as on the disc: 1.0
/// doesn't hash the package's files (docs/game/map-pack.md 8.4).
fn content_meta_for(disc: &[u8], path: &str, offer: u64) -> Result<Vec<u8>> {
    let mut meta = disc.to_vec();
    let header = meta.get(0x18..0x1c).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    if header.is_some_and(|h| h >= META_OFFER + 8 && h <= meta.len()) {
        meta[META_OFFER..META_OFFER + 8].copy_from_slice(&offer.to_le_bytes());
    }
    kernel::content::sign(&mut meta, HALO2_TITLE).with_context(|| path.to_string())?;
    if header.is_none_or(|h| h < META_OFFER + 8) {
        bail!("{path}: the header is too short to hold an offer ID");
    }
    Ok(meta)
}

/// The packages to install: one per map, from each disc package's
/// `patch_v3.lvl`. Reads each package's ContentMeta.xbx and patch_v3.lvl and
/// checks everything before anything is written. The disc's own `patch.lvl`
/// (a Lockout record, docs/game/map-pack.md 8.5) and `patch_v3.lvl` are never
/// installed.
pub fn layout<R: Read + Seek>(iso: &mut Xiso<R>, plan: &Plan) -> Result<Vec<MapPackage>> {
    let mut out: Vec<MapPackage> = Vec::new();
    for p in &plan.packages {
        let meta_entry = p.files.iter().find(|f| is_content_meta(f)).expect("checked by the plan");
        if meta_entry.size > CONTENT_META_MAX {
            bail!("{} is {} bytes, too big for a content header", meta_entry.path, meta_entry.size);
        }
        let disc_meta = iso.read(meta_entry)?;
        let Some(v3_entry) = p.files.iter().find(|f| is_named(f, PATCH_V3)) else {
            bail!("package {} has no {PATCH_V3}", p.dir);
        };
        if v3_entry.size > PATCH_V3_MAX {
            bail!("{} is {} bytes, too big for a map list", v3_entry.path, v3_entry.size);
        }
        let v3 = iso.read(v3_entry)?;
        let records = patch_lvl::v3_records(&v3).with_context(|| v3_entry.path.clone())?;
        let extras: Vec<Entry> = p
            .files
            .iter()
            .filter(|f| !is_content_meta(f) && !is_map(f) && !is_named(f, PATCH_V2) && !is_named(f, PATCH_V3))
            .cloned()
            .collect();
        let mut used = Vec::new();
        for (i, record) in records.iter().enumerate() {
            let name = patch_lvl::map_name(record).with_context(|| format!("{} record {i}", v3_entry.path))?;
            let file = format!("{name}.map");
            let Some(map) = p.files.iter().find(|f| is_named(f, &file)) else {
                bail!("{} record {i} names {file}, which package {} doesn't have", v3_entry.path, p.dir);
            };
            if used.contains(&map.path) {
                bail!("{} names {file} twice", v3_entry.path);
            }
            used.push(map.path.clone());
            // Made, then checked as 1.0 checks it.
            let lvl = patch_lvl::v2_from_v3(record)?;
            patch_lvl::validate_v2(&lvl)?;
            let map_id = patch_lvl::v2_map_id(&lvl);
            if let Some(other) = out.iter().find(|m| m.map_id == map_id) {
                bail!("{file} and {} share map id {map_id:#x}", other.map.path);
            }
            let offer = map_offer(p.number, i)?;
            out.push(MapPackage {
                source: p.dir.clone(),
                offer,
                map: map.clone(),
                map_id,
                meta: content_meta_for(&disc_meta, &meta_entry.path, offer)?,
                patch_lvl: lvl,
                extras: extras.clone(),
            });
        }
        if let Some(stray) = p.files.iter().find(|f| is_map(f) && !used.contains(&f.path)) {
            bail!("{} has no record in {}", stray.path, v3_entry.path);
        }
    }
    Ok(out)
}

/// Checks a candidate update against the base image: same title, and the
/// low byte of its certificate version names the base disc's version.
/// Returns the update's certificate version.
pub fn check_update(update: &[u8], base: &xbe::Certificate) -> Result<u32> {
    let x = xbe::Xbe::parse(update).context("parsing the update XBE")?;
    let c = &x.certificate;
    if c.title_id != base.title_id {
        bail!("update title {:08X} is not the base's {:08X}", c.title_id, base.title_id);
    }
    if c.version & 0xff != base.version & 0xff || c.version >> 8 == 0 {
        bail!("update version {:#x} is not an update for base version {:#x}", c.version, base.version);
    }
    Ok(c.version)
}

/// Output path components, relative to the data root, for one file of the
/// package with `offer`. Matches `vfs::Config::content_dir`.
fn package_components(offer: u64, name: &str) -> Vec<String> {
    [
        "disk",
        "Partition1",
        "TDATA",
        &format!("{HALO2_TITLE:08X}"),
        vfs::CONTENT_DIR,
        &vfs::content_dir_name(offer),
        name,
    ]
    .map(str::to_string)
    .to_vec()
}

fn update_components(version: u32) -> Vec<String> {
    ["updates", &format!("{HALO2_TITLE:08X}"), &format!("{version:08x}"), "default.xbe"].map(str::to_string).to_vec()
}

/// What an install wrote (or, with `dry_run`, would write) and removed.
#[derive(Debug, Default)]
pub struct Installed {
    pub packages: usize,
    pub written: Vec<PathBuf>,
    /// Package directories of the older one-package-per-disc-directory
    /// layout, whose `patch.lvl` overwrote 1.0's Lockout slot.
    pub removed: Vec<PathBuf>,
}

/// Removes `dir`, an old package directory, if it is one: a real directory
/// (a symlink there is unlinked, never followed).
fn remove_old_package(dir: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(dir)?,
        Ok(_) => std::fs::remove_file(dir)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    }
    Ok(true)
}

/// Writes one package per map, and the update when `base` is given, then
/// removes the older layout's package directories.
pub fn install<R: Read + Seek>(
    iso: &mut Xiso<R>,
    plan: &Plan,
    out: &OutDir,
    data_root: &Path,
    base: Option<&xbe::Certificate>,
    dry_run: bool,
) -> Result<Installed> {
    // Everything is read and checked before anything is written, so a bad
    // header or map list leaves the data dir as it was.
    let packages = layout(iso, plan)?;
    let update = match base {
        Some(base) => {
            let Some(entry) = plan.update_for(base.version) else {
                let have: Vec<String> = plan.updates.iter().map(|(n, _)| format!("{n:02}")).collect();
                bail!(
                    "no update on this disc for base version {:#x} (it has {}); rerun with --no-update",
                    base.version,
                    have.join(", ")
                );
            };
            let bytes = iso.read(entry)?;
            let version = check_update(&bytes, base).with_context(|| entry.path.clone())?;
            Some((update_components(version), bytes))
        }
        None => None,
    };
    let at = |comps: &[String]| comps.iter().fold(data_root.to_path_buf(), |a, c| a.join(c));
    let mut done = Installed { packages: packages.len(), ..Default::default() };
    for m in &packages {
        let generated = [(CONTENT_META_NAME, &m.meta), (PATCH_V2, &m.patch_lvl)];
        for (name, bytes) in generated {
            let comps = package_components(m.offer, name);
            done.written.push(if dry_run { at(&comps) } else { out.write(&comps, bytes)? });
        }
        for f in std::iter::once(&m.map).chain(&m.extras) {
            let comps = package_components(m.offer, file_name(f));
            done.written.push(if dry_run {
                at(&comps)
            } else {
                out.write_with(&comps, |w| iso.copy_to(f, w).map(drop).map_err(|e| std::io::Error::other(e.to_string())))
                    .with_context(|| format!("writing {}", f.path))?
            });
        }
    }
    if let Some((comps, bytes)) = update {
        done.written.push(if dry_run { at(&comps) } else { out.write(&comps, &bytes)? });
    }
    let cfg = vfs::Config::new(data_root, HALO2_TITLE);
    for p in &plan.packages {
        let offer = vfs::offer_id(HALO2_TITLE, p.number);
        if packages.iter().any(|m| m.offer == offer) {
            continue;
        }
        let old = cfg.content_dir(offer);
        if dry_run {
            if std::fs::symlink_metadata(&old).is_ok() {
                done.removed.push(old);
            }
        } else if remove_old_package(&old).with_context(|| format!("removing {}", old.display()))? {
            done.removed.push(old);
        }
    }
    Ok(done)
}

pub fn run(args: Vec<String>) -> Result<()> {
    let a = match parse_args(&args) {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            return Ok(());
        }
        Err(e) => bail!("{e}\n\n{USAGE}"),
    };
    let data = a.data.clone().unwrap_or_else(|| std::env::temp_dir().join("h2boot-data"));
    let base = if a.update {
        let disc = match a.disc.clone() {
            Some(d) => d,
            None => crate::default_disc().context("finding your Halo 2 disc for the update (or pass --no-update)")?,
        };
        let bytes = crate::load_xbe(&disc)?;
        let x = xbe::Xbe::parse(&bytes)?;
        if x.certificate.title_id != HALO2_TITLE {
            bail!("{} is not Halo 2 (title {:08X})", disc.display(), x.certificate.title_id);
        }
        Some(x.certificate.clone())
    } else {
        None
    };
    let mut iso = crate::open_iso(&a.iso)?;
    let plan = Plan::from_entries(&iso.entries()?)?;
    let out = OutDir::new(&data, a.force)?.protecting(&a.iso);
    let done = install(&mut iso, &plan, &out, &data, base.as_ref(), a.dry_run)?;
    let (verb, gone) = if a.dry_run { ("would write", "would remove") } else { ("wrote", "removed") };
    for p in &done.written {
        println!("{verb} {}", p.display());
    }
    for p in &done.removed {
        println!("{gone} {} (the older layout)", p.display());
    }
    println!(
        "{} map package(s), {} file(s) into {}",
        done.packages,
        done.written.len() - usize::from(base.is_some()),
        data.display()
    );
    if base.is_some() {
        println!("the update is kept aside: nothing runs it yet (docs/runtime/title-updates.md)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Automatic install for `h2 play` and `h2 run`.

/// The name the docs tell people to give the map pack disc.
const AUTO_NAME: &str = "mappack.iso";
/// Beside the data root's own directories: what was installed, from which
/// file, for which base disc.
const STAMP: &str = "mappack.stamp";
/// Bumped when an older import's files are wrong and must be written again.
/// 2: ContentMeta.xbx is signed for this console (1, unversioned, copied it
/// unsigned, and the game called the content damaged).
/// 3: one package per map with a version 2 `patch.lvl` made from its
/// `patch_v3.lvl` record, so 1.0 lists the maps; the three disc-shaped
/// packages, whose `patch.lvl` took over the Lockout slot, are removed.
const STAMP_VERSION: u32 = 3;

/// Whether the automatic install runs: not with `--no-mappack`, and not with
/// `H2_MAPPACK` set to `0`, `false`, `no` or `off`.
pub fn auto_enabled(no_flag: bool) -> bool {
    auto_enabled_with(no_flag, std::env::var("H2_MAPPACK").ok().as_deref())
}

fn auto_enabled_with(no_flag: bool, env: Option<&str>) -> bool {
    let off = env.is_some_and(|v| ["0", "false", "no", "off"].contains(&v.trim().to_ascii_lowercase().as_str()));
    !no_flag && !off
}

/// Whether `path` is a disc with the map pack's content layout. Reads the
/// disc's header and directory tables only.
fn looks_like_map_pack(path: &Path) -> bool {
    let Ok(f) = std::fs::File::open(path) else { return false };
    let Ok(mut iso) = Xiso::open(std::io::BufReader::new(f)) else { return false };
    iso.entries().is_ok_and(|e| Plan::from_entries(&e).is_ok())
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// The map pack disc beside the game disc: `mappack.iso` (any case), else
/// the first other `.iso` (by name) that has the map pack's layout.
pub fn discover(game_disc: &Path) -> Option<PathBuf> {
    let dir = match game_disc.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut isos: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso")))
        .filter(|p| p.is_file() && !same_file(p, game_disc))
        .collect();
    isos.sort();
    let named = |p: &PathBuf| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(AUTO_NAME));
    if let Some(i) = isos.iter().position(named) {
        return Some(isos.swap_remove(i));
    }
    isos.into_iter().find(|p| looks_like_map_pack(p))
}

fn stamp_text(iso: &Path, base_version: u32) -> Result<String> {
    let m = std::fs::metadata(iso)?;
    let mtime = m.modified()?.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Ok(format!("v={STAMP_VERSION} size={} mtime={mtime} base={base_version:#x}\n", m.len()))
}

/// Whether this disc, in this state, was already installed into `data`.
fn already_installed(data: &Path, iso: &Path, base_version: u32) -> bool {
    let content = vfs::Config::new(data, HALO2_TITLE).title_data_dir().join(vfs::CONTENT_DIR);
    content.is_dir()
        && stamp_text(iso, base_version).is_ok_and(|want| std::fs::read_to_string(data.join(STAMP)).is_ok_and(|have| have == want))
}

/// Looks for the map pack beside `game_disc` and installs it into `data`
/// when it is not there yet. Prints one line; never fails: a problem is a
/// warning and the game starts anyway.
pub fn auto_install(game_disc: &Path, data: &Path) {
    match try_auto(game_disc, data) {
        Ok(Some(line)) => println!("map pack: {line}"),
        Ok(None) => {}
        Err(e) => eprintln!("map pack: not installed: {e:#}"),
    }
}

fn try_auto(game_disc: &Path, data: &Path) -> Result<Option<String>> {
    let Some(iso_path) = discover(game_disc) else { return Ok(None) };
    let name = iso_path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let bytes = crate::load_xbe(game_disc)?;
    let x = xbe::Xbe::parse(&bytes)?;
    if x.certificate.title_id != HALO2_TITLE {
        return Ok(None);
    }
    let base = x.certificate;
    if already_installed(data, &iso_path, base.version) {
        return Ok(Some("already installed".into()));
    }
    let mut iso = crate::open_iso(&iso_path)?;
    let plan = Plan::from_entries(&iso.entries()?).with_context(|| name.clone())?;
    let has_update = plan.update_for(base.version).is_some();
    // Overwrite: whatever is there came from this import or an older one.
    let out = OutDir::new(data, true)?.protecting(&iso_path);
    let maps = install(&mut iso, &plan, &out, data, has_update.then_some(&base), false)?.packages;
    std::fs::write(data.join(STAMP), stamp_text(&iso_path, base.version)?)?;
    Ok(Some(if has_update {
        format!("installed {maps} maps and title update from {name}")
    } else {
        format!("installed {maps} maps from {name} (no title update for this disc's version)")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use content::patch_lvl::testing::{v3_file, v3_record};
    use std::fs;
    use std::io::Cursor;

    const SECTOR: usize = 2048;

    /// One directory table: entries chained down the right-hand side.
    fn table(items: &[(&str, u32, u32, bool)]) -> Vec<u8> {
        let mut recs: Vec<Vec<u8>> = Vec::new();
        for &(name, sector, size, dir) in items {
            let mut v = vec![0u8; 4];
            v.extend(sector.to_le_bytes());
            v.extend(size.to_le_bytes());
            v.push(if dir { 0x10 } else { 0 });
            v.push(name.len() as u8);
            v.extend(name.as_bytes());
            while !v.len().is_multiple_of(4) {
                v.push(0xFF);
            }
            recs.push(v);
        }
        let mut out = Vec::new();
        let mut offs = Vec::new();
        for r in &recs {
            offs.push(out.len());
            out.extend(r);
        }
        for i in 0..recs.len().saturating_sub(1) {
            let right = (offs[i + 1] / 4) as u16;
            out[offs[i] + 2..offs[i] + 4].copy_from_slice(&right.to_le_bytes());
        }
        out.resize(SECTOR, 0xFF);
        out
    }

    /// A synthetic XBE: just enough header for `xbe::Xbe::parse`.
    fn xbe(title: u32, version: u32) -> Vec<u8> {
        let base = 0x10000u32;
        let mut d = vec![0u8; 0x1000];
        d[0..4].copy_from_slice(b"XBEH");
        let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0x104, base);
        put(&mut d, 0x108, 0x1000);
        put(&mut d, 0x10c, 0x1000);
        put(&mut d, 0x110, 0x178);
        put(&mut d, 0x118, base + 0x200); // certificate
        put(&mut d, 0x11c, 1); // one executable section over the headers
        put(&mut d, 0x120, base + 0x300);
        put(&mut d, 0x300, 0x04);
        put(&mut d, 0x304, base);
        put(&mut d, 0x308, 0x1000);
        put(&mut d, 0x314, base + 0x380); // its name, "x"
        d[0x380] = b'x';
        put(&mut d, 0x128, 0xA8FC_57AB ^ (base + 0x400)); // entry (retail key)
        put(&mut d, 0x158, 0x5B6D_40B6 ^ (base + 0x400)); // thunk (retail key)
        put(&mut d, 0x200, 0x1d0); // certificate size
        put(&mut d, 0x208, title);
        put(&mut d, 0x200 + 0xac, version);
        d
    }

    const META_LEN: u32 = 0xa0;

    /// A synthetic ContentMeta.xbx as a disc carries it: XCSF, header size
    /// 0x88, Halo 2's title and the package's offer, a body past the header,
    /// and an all-zero signature with the installed flag clear.
    fn meta(package: u32) -> Vec<u8> {
        let mut m = vec![0u8; META_LEN as usize];
        m[0x14..0x18].copy_from_slice(b"XCSF");
        m[0x18..0x1c].copy_from_slice(&0x88u32.to_le_bytes());
        m[0x1c..0x20].copy_from_slice(&1u32.to_le_bytes());
        m[0x24..0x28].copy_from_slice(&HALO2_TITLE.to_le_bytes());
        m[0x28..0x30].copy_from_slice(&vfs::offer_id(HALO2_TITLE, package).to_le_bytes());
        for (i, b) in m[0x30..].iter_mut().enumerate() {
            *b = (i as u32 * 5 + package) as u8;
        }
        m
    }

    /// `written` is `meta(package)` for the package with `offer`, signed
    /// for this console: the offer ID, the signature the kernel's key gives
    /// and the installed flag changed, nothing else.
    fn assert_signed(written: &[u8], package: u32, offer: u64) {
        let disc = meta(package);
        assert_eq!(written.len(), disc.len());
        assert_eq!(written[..20], kernel::content::signature(written, HALO2_TITLE).unwrap());
        assert_ne!(written[..20], [0; 20]);
        assert_eq!(written[0x20] & 1, 1);
        assert_eq!(written[0x28..0x30], offer.to_le_bytes());
        let changed: Vec<usize> = (0..disc.len()).filter(|&i| written[i] != disc[i]).collect();
        assert!(changed.iter().all(|&i| i < 20 || i == 0x20 || (0x28..0x30).contains(&i)), "{changed:x?}");
    }

    /// Map ids of the synthetic records.
    const ALPHA: u32 = 0x51;
    const GAMMA: u32 = 0x52;
    const BETA: u32 = 0x53;
    /// The Lockout-like record the disc's own patch.lvl carries.
    const LOCKOUT: u32 = 0x07;

    fn lockout_patch_lvl() -> Vec<u8> {
        patch_lvl::v2_from_v3(&v3_record("scenarios\\multi\\lockout\\lockout", LOCKOUT, 9)).unwrap()
    }

    /// The sectors of the synthetic files.
    const PKG1_V3: usize = 100;
    const PKG3_V3: usize = 300;
    const DISC_PATCH: usize = 400;

    /// A map pack shaped disc: package 1 with two maps (one named in another
    /// case than its record), an image, the disc's own Lockout patch.lvl and
    /// a patch_v3.lvl; package 3 with one map; a stray file beside them, one
    /// empty directory written as a zeroed sector, and two updates.
    fn disc_with(pkg1_v3: &[u8], pkg3_v3: &[u8]) -> Vec<u8> {
        let mut img = vec![0u8; 500 * SECTOR];
        let vd = 32 * SECTOR;
        img[vd..vd + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
        img[vd + 20..vd + 24].copy_from_slice(&33u32.to_le_bytes());
        img[vd + 24..vd + 28].copy_from_slice(&(SECTOR as u32).to_le_bytes());
        let s = SECTOR as u32;
        let len = |b: &[u8]| b.len() as u32;
        let dirs: [(usize, Vec<u8>); 9] = [
            (33, table(&[("default.xbe", 60, 4, false), ("demos", 34, s, true), ("videos", 45, s, true)])),
            (34, table(&[("dlc", 35, s, true), ("update", 38, s, true)])),
            (35, table(&[("00000001", 36, s, true), ("00000003", 37, s, true), ("maps.png", 61, 3, false)])),
            (
                36,
                table(&[
                    ("ContentImage.xbx", 72, 3, false),
                    ("ContentMeta.xbx", 62, META_LEN, false),
                    ("Gamma.map", 73, 8, false),
                    ("alpha.map", 63, 2000, false),
                    ("patch.lvl", DISC_PATCH as u32, patch_lvl::V2_LEN as u32, false),
                    ("patch_v3.lvl", PKG1_V3 as u32, len(pkg1_v3), false),
                ]),
            ),
            (
                37,
                table(&[
                    ("beta.map", 65, 7, false),
                    ("ContentMeta.xbx", 66, META_LEN, false),
                    ("patch_v3.lvl", PKG3_V3 as u32, len(pkg3_v3), false),
                ]),
            ),
            (38, table(&[("Updates", 39, s, true), ("updater.xbe", 61, 3, false)])),
            (
                39,
                table(&[
                    ("contentmeta.xbx.03", 61, 3, false),
                    ("default.xbe.03", 67, 0x1000, false),
                    ("default.xbe.04", 70, 0x1000, false),
                ]),
            ),
            (45, vec![0u8; SECTOR]),
            (46, vec![0u8; SECTOR]),
        ];
        for (sector, t) in dirs {
            img[sector * SECTOR..(sector + 1) * SECTOR].copy_from_slice(&t);
        }
        let mut fill =
            |sector: usize, bytes: &[u8]| img[sector * SECTOR..sector * SECTOR + bytes.len()].copy_from_slice(bytes);
        fill(62, &meta(1));
        fill(63, &[0xA5; 2000]);
        fill(65, b"betamap");
        fill(66, &meta(3));
        fill(67, &xbe(HALO2_TITLE, 0x303));
        fill(70, &xbe(HALO2_TITLE, 0x304));
        fill(72, b"img");
        fill(73, b"gammamap");
        fill(PKG1_V3, pkg1_v3);
        fill(PKG3_V3, pkg3_v3);
        fill(DISC_PATCH, &lockout_patch_lvl());
        img
    }

    fn pkg1_v3() -> Vec<u8> {
        v3_file(&[
            v3_record("scenarios\\multi\\alpha\\alpha", ALPHA, 0x1111),
            v3_record("scenarios\\multi\\gamma\\gamma", GAMMA, 0x2222),
        ])
    }

    fn pkg3_v3() -> Vec<u8> {
        v3_file(&[v3_record("scenarios\\multi\\beta\\beta", BETA, 0x3333)])
    }

    fn disc() -> Vec<u8> {
        disc_with(&pkg1_v3(), &pkg3_v3())
    }

    /// The offer IDs of the synthetic disc's three maps.
    const OFFER_ALPHA: u64 = 0x4D53_0064_0000_0101;
    const OFFER_GAMMA: u64 = 0x4D53_0064_0000_0102;
    const OFFER_BETA: u64 = 0x4D53_0064_0000_0301;

    /// Every `patch.lvl` under the data dir's content directory, by package.
    fn installed_patch_lvls(data: &Path) -> Vec<(String, Vec<u8>)> {
        let content = vfs::Config::new(data, HALO2_TITLE).title_data_dir().join(vfs::CONTENT_DIR);
        let mut out = Vec::new();
        for d in fs::read_dir(content).unwrap().flatten() {
            for f in fs::read_dir(d.path()).unwrap().flatten() {
                if f.file_name().to_string_lossy().to_ascii_lowercase().starts_with("patch") {
                    out.push((d.file_name().to_string_lossy().into_owned(), fs::read(f.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// The installed layout: three packages, each a signed ContentMeta.xbx,
    /// a valid version 2 patch.lvl naming the map beside it, and nothing of
    /// the disc's Lockout record.
    fn assert_layout(data: &Path) {
        let cfg = vfs::Config::new(data, HALO2_TITLE);
        let content = cfg.title_data_dir().join(vfs::CONTENT_DIR);
        let mut dirs: Vec<String> =
            fs::read_dir(&content).unwrap().flatten().map(|d| d.file_name().to_string_lossy().into_owned()).collect();
        dirs.sort();
        assert_eq!(dirs, ["4d53006400000101", "4d53006400000102", "4d53006400000301"]);
        let names = |offer: u64| {
            let mut v: Vec<String> = fs::read_dir(cfg.content_dir(offer))
                .unwrap()
                .flatten()
                .map(|d| d.file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(names(OFFER_ALPHA), ["ContentImage.xbx", "ContentMeta.xbx", "alpha.map", "patch.lvl"]);
        assert_eq!(names(OFFER_GAMMA), ["ContentImage.xbx", "ContentMeta.xbx", "Gamma.map", "patch.lvl"]);
        assert_eq!(names(OFFER_BETA), ["ContentMeta.xbx", "beta.map", "patch.lvl"]);

        for (offer, package, map, id, bytes) in [
            (OFFER_ALPHA, 1, "alpha.map", ALPHA, vec![0xA5; 2000]),
            (OFFER_GAMMA, 1, "Gamma.map", GAMMA, b"gammamap".to_vec()),
            (OFFER_BETA, 3, "beta.map", BETA, b"betamap".to_vec()),
        ] {
            let dir = cfg.content_dir(offer);
            assert_signed(&fs::read(dir.join("ContentMeta.xbx")).unwrap(), package, offer);
            assert_eq!(fs::read(dir.join(map)).unwrap(), bytes);
            let lvl = fs::read(dir.join("patch.lvl")).unwrap();
            patch_lvl::validate_v2(&lvl).unwrap();
            assert_eq!(patch_lvl::v2_map_id(&lvl), id);
            assert!(format!("{}.map", patch_lvl::map_name(&lvl).unwrap()).eq_ignore_ascii_case(map));
        }
        assert_eq!(fs::read(cfg.content_dir(OFFER_ALPHA).join("ContentImage.xbx")).unwrap(), b"img");

        // Only the three generated patch.lvl files, none of them Lockout's.
        let lvls = installed_patch_lvls(data);
        assert_eq!(lvls.len(), 3, "{:?}", lvls.iter().map(|l| &l.0).collect::<Vec<_>>());
        for (dir, lvl) in &lvls {
            assert_ne!(lvl, &lockout_patch_lvl(), "{dir}");
            assert_ne!(patch_lvl::v2_map_id(lvl), LOCKOUT, "{dir}");
            assert_ne!(patch_lvl::map_name(lvl).unwrap(), "lockout", "{dir}");
        }
    }

    fn base(version: u32) -> xbe::Certificate {
        xbe::Xbe::parse(&xbe(HALO2_TITLE, version)).unwrap().certificate
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-mappack-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn plan_finds_packages_and_updates() {
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let dirs: Vec<_> = plan.packages.iter().map(|p| (p.dir.as_str(), p.number, p.files.len())).collect();
        assert_eq!(dirs, [("00000001", 1, 6), ("00000003", 3, 3)]);
        let ups: Vec<u32> = plan.updates.iter().map(|(n, _)| *n).collect();
        assert_eq!(ups, [3, 4]);
        assert_eq!(plan.update_for(3).unwrap().path, "/demos/update/Updates/default.xbe.03");
        assert_eq!(plan.update_for(0x10a), None);
    }

    #[test]
    fn offer_ids_keep_the_title_and_never_reuse_a_package_number() {
        assert_eq!(map_offer(1, 0).unwrap(), 0x4D53_0064_0000_0101);
        assert_eq!(map_offer(3, 4).unwrap(), 0x4D53_0064_0000_0305);
        assert_eq!(map_offer(2, 1).unwrap() >> 32, u64::from(HALO2_TITLE));
        assert!(map_offer(0, 0).is_err());
        assert!(map_offer(1, 0xff).is_err());
        assert!(map_offer(0x0100_0000, 0).is_err());
        for n in 1..=3 {
            assert_ne!(map_offer(n, 0).unwrap(), vfs::offer_id(HALO2_TITLE, n));
        }
    }

    #[test]
    fn layout_is_one_package_per_map() {
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let l = layout(&mut iso, &plan).unwrap();
        let got: Vec<_> = l.iter().map(|m| (m.source.as_str(), m.offer, m.map.path.as_str(), m.map_id)).collect();
        assert_eq!(
            got,
            [
                ("00000001", OFFER_ALPHA, "/demos/dlc/00000001/alpha.map", ALPHA),
                ("00000001", OFFER_GAMMA, "/demos/dlc/00000001/Gamma.map", GAMMA),
                ("00000003", OFFER_BETA, "/demos/dlc/00000003/beta.map", BETA),
            ]
        );
        let extras: Vec<Vec<&str>> = l.iter().map(|m| m.extras.iter().map(file_name).collect()).collect();
        assert_eq!(extras, [vec!["ContentImage.xbx"], vec!["ContentImage.xbx"], vec![]]);
        // Each patch.lvl is its own record, mapped from the v3 one.
        let v3 = pkg1_v3();
        let recs = patch_lvl::v3_records(&v3).unwrap();
        assert_eq!(l[1].patch_lvl, patch_lvl::v2_from_v3(recs[1]).unwrap());
    }

    fn layout_err(pkg1: &[u8], pkg3: &[u8]) -> String {
        let mut iso = Xiso::open(Cursor::new(disc_with(pkg1, pkg3))).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        format!("{:#}", layout(&mut iso, &plan).unwrap_err())
    }

    #[test]
    fn layout_refuses_map_lists_that_dont_match_the_package() {
        let missing = v3_file(&[v3_record("x\\alpha", ALPHA, 1), v3_record("x\\delta", GAMMA, 2)]);
        assert!(layout_err(&missing, &pkg3_v3()).contains("names delta.map, which package 00000001 doesn't have"));
        let unlisted = v3_file(&[v3_record("x\\alpha", ALPHA, 1)]);
        let e = layout_err(&unlisted, &pkg3_v3());
        assert!(e.contains("Gamma.map has no record"), "{e}");
        let twice = v3_file(&[v3_record("x\\alpha", ALPHA, 1), v3_record("x\\ALPHA", GAMMA, 2)]);
        assert!(layout_err(&twice, &pkg3_v3()).contains("names ALPHA.map twice"));
        let shared = v3_file(&[v3_record("x\\beta", ALPHA, 3)]);
        assert!(layout_err(&pkg1_v3(), &shared).contains("share map id 0x51"));
        let mut bad_crc = pkg3_v3();
        bad_crc[0x1000] ^= 1;
        let e = layout_err(&pkg1_v3(), &bad_crc);
        assert!(e.contains("00000003/patch_v3.lvl") && e.contains("CRC"), "{e}");
    }

    #[test]
    fn plan_refuses_a_disc_without_packages() {
        let mut img = disc();
        // Rename "dlc" so nothing matches.
        let at = 34 * SECTOR + 14;
        img[at..at + 3].copy_from_slice(b"xyz");
        let mut iso = Xiso::open(Cursor::new(img)).unwrap();
        let err = Plan::from_entries(&iso.entries().unwrap()).unwrap_err();
        assert!(err.to_string().contains("no content packages"), "{err}");
    }

    #[test]
    fn plan_refuses_a_package_without_metadata() {
        let mut img = disc();
        let t = &img[37 * SECTOR..38 * SECTOR];
        let at = 37 * SECTOR + t.windows(15).position(|w| w == b"ContentMeta.xbx").unwrap();
        img[at..at + 15].copy_from_slice(b"ContentMeta.xyz");
        let mut iso = Xiso::open(Cursor::new(img)).unwrap();
        let err = Plan::from_entries(&iso.entries().unwrap()).unwrap_err();
        assert!(err.to_string().contains("00000003 has no ContentMeta"), "{err}");
    }

    #[test]
    fn install_lays_packages_out_as_vfs_expects_and_keeps_the_update_aside() {
        let d = scratch("install");
        let data = d.join("data");
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let out = OutDir::new(&data, false).unwrap();
        let done = install(&mut iso, &plan, &out, &data, Some(&base(3)), false).unwrap();
        assert_eq!((done.packages, done.written.len(), done.removed.len()), (3, 12, 0));
        assert_layout(&data);

        // The update matching base version 3, under its own version, not in TDATA.
        let cfg = vfs::Config::new(&data, HALO2_TITLE);
        let up = data.join("updates/4D530064/00000303/default.xbe");
        assert_eq!(fs::read(&up).unwrap(), xbe(HALO2_TITLE, 0x303));
        assert!(!cfg.title_data_dir().join("$u").exists());

        // A second run refuses to overwrite without --force, and succeeds with it.
        assert!(install(&mut iso, &plan, &out, &data, None, false).is_err());
        let forced = OutDir::new(&data, true).unwrap();
        install(&mut iso, &plan, &forced, &data, Some(&base(3)), false).unwrap();
        assert_layout(&data);
        fs::remove_dir_all(d).unwrap();
    }

    /// The older layout: one package per disc directory, holding the disc's
    /// Lockout patch.lvl.
    fn old_layout(data: &Path) -> Vec<PathBuf> {
        let cfg = vfs::Config::new(data, HALO2_TITLE);
        [1, 3]
            .map(|n| {
                let dir = cfg.content_dir(vfs::offer_id(HALO2_TITLE, n));
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("patch.lvl"), lockout_patch_lvl()).unwrap();
                fs::write(dir.join("ContentMeta.xbx"), meta(n)).unwrap();
                dir
            })
            .to_vec()
    }

    #[test]
    fn install_removes_the_older_layout() {
        let d = scratch("old-layout");
        let data = d.join("data");
        let old = old_layout(&data);
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let out = OutDir::new(&data, false).unwrap();
        let dry = install(&mut iso, &plan, &out, &data, None, true).unwrap();
        assert_eq!(dry.removed, old);
        assert!(old.iter().all(|o| o.is_dir()), "a dry run removes nothing");
        let done = install(&mut iso, &plan, &out, &data, None, false).unwrap();
        assert_eq!(done.removed, old);
        assert!(old.iter().all(|o| !o.exists()));
        assert_layout(&data);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn install_refuses_a_malformed_content_meta_before_writing() {
        let d = scratch("badmeta");
        let data = d.join("data");
        let mut img = disc();
        img[66 * SECTOR + 0x14] = b'Y'; // package 3's magic
        let mut iso = Xiso::open(Cursor::new(img)).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let out = OutDir::new(&data, false).unwrap();
        let err = install(&mut iso, &plan, &out, &data, None, false).unwrap_err();
        assert!(format!("{err:#}").contains("00000003/ContentMeta.xbx: ContentMeta.xbx has no XCSF magic"), "{err:#}");
        assert!(!data.join("disk").exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn dry_run_writes_nothing() {
        let d = scratch("dry");
        let data = d.join("data");
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let out = OutDir::new(&data, false).unwrap();
        let listed = install(&mut iso, &plan, &out, &data, Some(&base(4)), true).unwrap().written;
        assert_eq!(listed.len(), 12);
        assert_eq!(listed[11], data.join("updates/4D530064/00000304/default.xbe"));
        assert_eq!(listed[0], vfs::Config::new(&data, HALO2_TITLE).content_dir(OFFER_ALPHA).join("ContentMeta.xbx"));
        assert_eq!(fs::read_dir(&data).unwrap().count(), 0);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn install_refuses_a_base_version_with_no_update() {
        let d = scratch("noupdate");
        let data = d.join("data");
        let mut iso = Xiso::open(Cursor::new(disc())).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let out = OutDir::new(&data, false).unwrap();
        let err = install(&mut iso, &plan, &out, &data, Some(&base(9)), true).unwrap_err();
        assert!(err.to_string().contains("it has 03, 04"), "{err}");
        fs::remove_dir_all(d).unwrap();
    }

    /// The user's map pack disc, read only: nine packages, each `patch.lvl`
    /// passing 1.0's check and naming the map file in its own package, none
    /// of them the disc's Lockout record. Nothing is written.
    /// `H2_MAPPACK_ISO=disc/mappack.iso cargo test -p cli --bin h2 real_map_pack -- --ignored`
    #[test]
    #[ignore = "needs the map pack ISO ($H2_MAPPACK_ISO, else disc/mappack.iso)"]
    fn real_map_pack_gives_nine_packages_1_0_can_read() {
        let path = std::env::var_os("H2_MAPPACK_ISO")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../disc/mappack.iso"));
        let mut iso = crate::open_iso(&path).unwrap();
        let plan = Plan::from_entries(&iso.entries().unwrap()).unwrap();
        let l = layout(&mut iso, &plan).unwrap();
        assert_eq!(l.len(), 9);
        let mut offers: Vec<u64> = l.iter().map(|m| m.offer).collect();
        offers.sort();
        offers.dedup();
        assert_eq!(offers.len(), 9);
        let mut ids: Vec<u32> = l.iter().map(|m| m.map_id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 9);
        for m in &l {
            let p = plan.packages.iter().find(|p| p.dir == m.source).unwrap();
            patch_lvl::validate_v2(&m.patch_lvl).unwrap();
            let name = format!("{}.map", patch_lvl::map_name(&m.patch_lvl).unwrap());
            assert!(file_name(&m.map).eq_ignore_ascii_case(&name), "{name}");
            assert!(p.files.contains(&m.map));
            assert_eq!(m.offer >> 32, u64::from(HALO2_TITLE));
            assert_eq!(m.meta[..20], kernel::content::signature(&m.meta, HALO2_TITLE).unwrap());
            assert_eq!(m.meta[0x20] & 1, 1);
            assert_eq!(m.meta[0x28..0x30], m.offer.to_le_bytes());
            assert!(m.extras.iter().all(|e| !file_name(e).to_ascii_lowercase().starts_with("patch")));
            // The disc's own patch.lvl is the Lockout record (docs/game/map-pack.md 8.5).
            let disc_lvl = iso.read(p.files.iter().find(|f| is_named(f, PATCH_V2)).unwrap()).unwrap();
            patch_lvl::validate_v2(&disc_lvl).unwrap();
            assert_ne!(patch_lvl::v2_map_id(&disc_lvl), m.map_id);
            assert_ne!(disc_lvl, m.patch_lvl);
            println!("{} {:016x} {} map id {:#x}", m.source, m.offer, file_name(&m.map), m.map_id);
        }
    }

    #[test]
    fn check_update_rejects_other_titles_and_versions() {
        assert_eq!(check_update(&xbe(HALO2_TITLE, 0x303), &base(3)).unwrap(), 0x303);
        assert!(check_update(&xbe(HALO2_TITLE, 0x304), &base(3)).is_err());
        assert!(check_update(&xbe(HALO2_TITLE, 0x3), &base(3)).is_err(), "the base itself is not an update");
        assert!(check_update(&xbe(0x4D53_80CD, 0x303), &base(3)).is_err());
        assert!(check_update(b"not an xbe", &base(3)).is_err());
    }

    /// A temp dir with links resolved (`/var` is a symlink on macOS).
    fn real_scratch(name: &str) -> PathBuf {
        scratch(name).canonicalize().unwrap()
    }

    /// A game "disc": a raw XBE is enough for `load_xbe`.
    fn game(dir: &Path, title: u32, version: u32) -> PathBuf {
        let p = dir.join("halo2.xbe");
        fs::write(&p, xbe(title, version)).unwrap();
        p
    }

    #[test]
    fn auto_install_can_be_switched_off() {
        assert!(auto_enabled_with(false, None));
        assert!(auto_enabled_with(false, Some("1")));
        assert!(auto_enabled_with(false, Some("")));
        assert!(!auto_enabled_with(true, None));
        for v in ["0", "false", "No", " off "] {
            assert!(!auto_enabled_with(false, Some(v)), "{v}");
        }
    }

    #[test]
    fn discover_takes_mappack_iso_in_any_case() {
        for name in ["mappack.iso", "MapPack.ISO", "MAPPACK.iso"] {
            let d = real_scratch("disc-case");
            let g = d.join("halo2.iso");
            fs::write(&g, b"game").unwrap();
            fs::write(d.join(name), b"not even an xiso").unwrap();
            let found = discover(&g).unwrap();
            assert_eq!(found.file_name().unwrap(), name);
            fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn discover_recognises_another_iso_by_layout_and_skips_the_rest() {
        let d = real_scratch("disc-layout");
        let g = d.join("halo2.iso");
        fs::write(&g, disc()).unwrap(); // the game disc itself is never a candidate
        fs::write(d.join("a-junk.iso"), vec![7u8; 100]).unwrap();
        fs::write(d.join("b-empty.iso"), b"").unwrap();
        fs::write(d.join("mappack.txt"), disc()).unwrap();
        fs::write(d.join("mappack.iso.part"), disc()).unwrap();
        fs::create_dir(d.join("dir.iso")).unwrap();
        assert_eq!(discover(&g), None);

        // A valid disc with another layout: no content packages.
        let mut other = disc();
        other[34 * SECTOR + 14..34 * SECTOR + 17].copy_from_slice(b"xyz");
        fs::write(d.join("c-other.iso"), other).unwrap();
        assert_eq!(discover(&g), None);

        fs::write(d.join("d-pack.ISO"), disc()).unwrap();
        assert_eq!(discover(&g).unwrap(), d.join("d-pack.ISO"));

        // The name wins over a layout match.
        fs::write(d.join("mappack.iso"), b"named").unwrap();
        assert_eq!(discover(&g).unwrap(), d.join("mappack.iso"));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn discover_in_an_empty_or_missing_directory() {
        let d = real_scratch("disc-none");
        assert_eq!(discover(&d.join("halo2.iso")), None);
        assert_eq!(discover(&d.join("nope/halo2.iso")), None);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn auto_installs_once_then_says_already_installed() {
        let d = real_scratch("auto");
        let data = d.join("data");
        let g = game(&d, HALO2_TITLE, 3);
        assert_eq!(try_auto(&g, &data).unwrap(), None, "no map pack beside the disc");
        assert!(!data.exists());

        fs::write(d.join("MAPPACK.ISO"), disc()).unwrap();
        assert_eq!(
            try_auto(&g, &data).unwrap().unwrap(),
            "installed 3 maps and title update from MAPPACK.ISO"
        );
        assert_layout(&data);
        let cfg = vfs::Config::new(&data, HALO2_TITLE);
        let one = cfg.content_dir(OFFER_ALPHA);
        assert_eq!(fs::read(one.join("alpha.map")).unwrap(), vec![0xA5; 2000]);
        assert!(data.join("updates/4D530064/00000303/default.xbe").is_file());
        assert_eq!(try_auto(&g, &data).unwrap().unwrap(), "already installed");

        // The maps are gone (a fresh data dir): installs again, over the stamp.
        fs::remove_dir_all(data.join("disk")).unwrap();
        assert!(try_auto(&g, &data).unwrap().unwrap().starts_with("installed 3 maps"));
        assert!(one.join("alpha.map").is_file());

        // A different file at the same name (size changes): installs again.
        let mut bigger = disc();
        bigger.extend([0u8; SECTOR]);
        fs::write(d.join("MAPPACK.ISO"), bigger).unwrap();
        assert!(try_auto(&g, &data).unwrap().unwrap().starts_with("installed"));
        assert_eq!(try_auto(&g, &data).unwrap().unwrap(), "already installed");

        // An import from before signing (unversioned stamp, unsigned header)
        // is written again, signed.
        let stamp = fs::read_to_string(data.join(STAMP)).unwrap();
        let rest = stamp.strip_prefix(&format!("v={STAMP_VERSION} ")).unwrap();
        fs::write(data.join(STAMP), rest).unwrap();
        fs::write(one.join("ContentMeta.xbx"), meta(1)).unwrap();
        assert!(try_auto(&g, &data).unwrap().unwrap().starts_with("installed 3 maps"));
        assert_signed(&fs::read(one.join("ContentMeta.xbx")).unwrap(), 1, OFFER_ALPHA);
        assert_eq!(try_auto(&g, &data).unwrap().unwrap(), "already installed");

        // A version 2 import (three disc-shaped packages with the disc's
        // Lockout patch.lvl) is redone once in the new layout, and its
        // packages removed.
        fs::remove_dir_all(cfg.title_data_dir().join(vfs::CONTENT_DIR)).unwrap();
        let old = old_layout(&data);
        fs::write(data.join(STAMP), format!("v=2 {rest}")).unwrap();
        assert!(try_auto(&g, &data).unwrap().unwrap().starts_with("installed 3 maps"));
        assert!(old.iter().all(|o| !o.exists()));
        assert_layout(&data);
        assert_eq!(try_auto(&g, &data).unwrap().unwrap(), "already installed");

        // Another base disc version needs its own update.
        let g4 = game(&d, HALO2_TITLE, 4);
        assert!(try_auto(&g4, &data).unwrap().unwrap().starts_with("installed"));
        assert!(data.join("updates/4D530064/00000304/default.xbe").is_file());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn auto_installs_maps_without_an_update_for_an_unknown_base_version() {
        let d = real_scratch("auto-noupdate");
        let data = d.join("data");
        let g = game(&d, HALO2_TITLE, 9);
        fs::write(d.join("mappack.iso"), disc()).unwrap();
        assert_eq!(
            try_auto(&g, &data).unwrap().unwrap(),
            "installed 3 maps from mappack.iso (no title update for this disc's version)"
        );
        assert!(!data.join("updates").exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn auto_ignores_other_games_and_survives_a_bad_map_pack() {
        let d = real_scratch("auto-bad");
        let data = d.join("data");
        fs::write(d.join("mappack.iso"), disc()).unwrap();
        let other = game(&d, 0x4D53_80CD, 3);
        assert_eq!(try_auto(&other, &data).unwrap(), None);
        assert!(!data.exists());

        let g = game(&d, HALO2_TITLE, 3);
        fs::write(d.join("mappack.iso"), b"garbage").unwrap();
        assert!(try_auto(&g, &data).is_err());
        auto_install(&g, &data); // warns, does not panic
        assert!(!data.join(STAMP).exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn parses_args() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let a = parse_args(&s(&["mp.iso", "--data", "/d", "--no-update", "--dry-run"])).unwrap();
        assert_eq!(
            a,
            Args {
                iso: "mp.iso".into(),
                data: Some("/d".into()),
                disc: None,
                update: false,
                force: false,
                dry_run: true
            }
        );
        assert!(parse_args(&s(&[])).is_err());
        assert!(parse_args(&s(&["a.iso", "b.iso"])).is_err());
        assert!(parse_args(&s(&["a.iso", "--bogus"])).is_err());
    }
}
