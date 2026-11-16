//! Map packages (docs/maps.md): saving the edited scenario as a package in
//! `<data>/maps/<dir>/`, listing a map's packages, and opening one as the
//! editor's current state.
//!
//! A package's patch is always the one layer from the disc's map (the base)
//! to the current state, written by `content::patch::diff`: PPF3 when the
//! map kept its length, BPS when a block grew. Saving an opened package
//! again replaces that layer. The source map and the disc are only read.
use crate::save::{self, MapSource};
use crate::scenario::Scenario;
use crate::{Error, Result};
use content::manifest::{Layer, Manifest, ResultInfo};
use content::patch::{Format, diff};
use content::{Library, MapId, Sha256, store};
use map::{Map, RawPointer, ResourceFile};
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tags::geometry::TagView;
use tags::structure::GeometryHeader;
use tags::structure::StructureRender;

/// What the "Save as package" form asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageForm {
    /// The package's display name (required).
    pub name: String,
    /// Optional; empty is none.
    pub author: String,
    /// Optional; empty is none.
    pub description: String,
    /// A PNG or JPEG of at most 512 KB, copied into the package.
    pub image: Option<PathBuf>,
    /// The directory under `<data>/maps/`.
    pub dir: String,
    /// Make it the package that serves its map (`active.toml`).
    pub activate: bool,
}

impl PackageForm {
    /// A directory name for package `name` of map `map`: `<map>-<name>`,
    /// lowercase, other characters as `-`, at most 64 bytes.
    pub fn default_dir(map: &str, name: &str) -> String {
        let mut slug = String::new();
        for c in name.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
        }
        let slug = slug.trim_matches('-');
        let mut dir = if slug.is_empty() {
            format!("{map}-edit")
        } else {
            format!("{map}-{slug}")
        };
        dir.truncate(64);
        dir.trim_end_matches('-').to_ascii_lowercase()
    }
}

/// A package written by [`save_package`].
#[derive(Clone, Debug)]
pub struct SavedPackage {
    pub dir: PathBuf,
    pub id: MapId,
    pub manifest: Manifest,
    pub format: Format,
    /// The patch file's size in bytes.
    pub patch_len: usize,
}

fn bad(why: impl Into<String>) -> Error {
    Error::Package(why.into())
}

/// The edited map's bytes: `source` with `scenario`'s changes, the header
/// checksum recomputed by the writer.
pub fn edited_bytes(source: &MapSource, scenario: &Scenario) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    save::write_patched(&mut source.open()?, scenario, &mut out)?;
    Ok(out.into_inner())
}

/// Save `scenario` (read from `source`) as package `form.dir` for map
/// `map` under `maps`. `replace` names the package directory being edited:
/// saving to it replaces its layer and manifest; any other existing
/// directory is refused.
pub fn save_package(
    maps: &Path,
    map: &str,
    source: &MapSource,
    scenario: &Scenario,
    form: &PackageForm,
    replace: Option<&str>,
) -> Result<SavedPackage> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err(bad("the package needs a name"));
    }
    if !store::is_dir_name(&form.dir) {
        return Err(bad(format!(
            "package directory {:?}: use letters, digits, '_' and '-'",
            form.dir
        )));
    }
    let image = match &form.image {
        Some(p) => {
            let bytes = std::fs::read(p).map_err(|e| bad(format!("{}: {e}", p.display())))?;
            let kind = store::image_kind(&bytes)?;
            Some((format!("preview.{kind}"), bytes))
        }
        None => None,
    };
    let base = source.read_base()?;
    let mut edited = edited_bytes(source, scenario)?;
    // The resolver repairs +0x2d0 after the layers. The writer already
    // left the right checksum, so the patch carries it and the repair
    // writes the same bytes; repairing here keeps that true.
    content::repair_checksum(&mut edited);
    let geometry = geometry_hash(&mut Map::from_reader(Cursor::new(&edited[..]))?).ok();
    let w = diff(&base, &edited, name);
    let sha = Sha256::of(&w.bytes);
    let file = format!("{map}-{}.{}", &sha.to_string()[..8], w.format.extension());
    let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
    let mut manifest = Manifest::retail(map, Sha256::of(&base));
    manifest.package.name = name.to_owned();
    manifest.package.author = opt(&form.author);
    manifest.package.description = opt(&form.description);
    manifest.package.image = image.as_ref().map(|i| i.0.clone());
    manifest.patches = vec![Layer {
        file: file.clone(),
        sha256: sha,
    }];
    manifest.result = geometry.map(|g| ResultInfo { geometry: Some(g) });
    let mut files: Vec<(&str, &[u8])> = vec![(&file, &w.bytes)];
    if let Some((n, b)) = &image {
        files.push((n, b));
    }
    let dir = store::write_package(
        maps,
        &form.dir,
        &manifest,
        &files,
        replace == Some(form.dir.as_str()),
    )?;
    if form.activate {
        store::set_active(maps, map, Some(&form.dir))?;
    }
    Ok(SavedPackage {
        dir,
        id: manifest.map_id(),
        manifest,
        format: w.format,
        patch_len: w.bytes.len(),
    })
}

/// The valid packages for `map` under `maps`, by directory name.
pub fn packages_for(maps: &Path, map: &str) -> Vec<content::Package> {
    Library::open(maps)
        .packages()
        .iter()
        .filter(|p| p.manifest.base.map.eq_ignore_ascii_case(map))
        .cloned()
        .collect()
}

/// Resolve package `dir` under `maps` over `base` (the disc's map) into a
/// source holding the resolved bytes in memory, with `base`'s location
/// kept (for the shared caches and the next save's base).
pub fn open_package(
    maps: &Path,
    dir: &str,
    base: &MapSource,
) -> Result<(MapSource, content::Package)> {
    let lib = Library::open(maps);
    let package = lib
        .packages()
        .iter()
        .find(|p| p.dir_name() == dir)
        .cloned()
        .ok_or_else(|| bad(format!("no valid package {dir} in {}", maps.display())))?;
    if package.manifest.base.derived {
        return Err(bad("a derived map (no disc base) cannot be edited here"));
    }
    let base_bytes = base.read_base()?;
    let resolved = lib.resolve_package(&package, &mut &base_bytes[..])?;
    let bytes = resolved.read_all(&mut &base_bytes[..])?;
    let source = MapSource {
        bytes: Some(Arc::from(bytes)),
        ..base.base()
    };
    Ok((source, package))
}

/// The `[result] geometry` hash of a map (docs/maps.md §1): SHA-256 over,
/// for each entry of the scenario's structure BSP block (`scnr+0x210`,
/// stride 0x44) in order,
///
/// - a `u32` 0 when the entry's file offset (`+0x00`) is 0; otherwise the
///   `u32` size (`+0x04`) then the BSP section's bytes
///   `[offset, offset + size)`;
/// - then, for that BSP's clusters and then its instance definitions in
///   order, each geometry block: `0x00` when it has none; `0x01`, the `u32`
///   block size and the block's bytes when it is in the map's own file;
///   `0x02`, the `u32` raw pointer word and the `u32` size when it is in a
///   shared cache.
///
/// Integers are little-endian. Placement edits and appended metadata leave
/// it unchanged, so an editor package reports its base's geometry.
pub fn geometry_hash<R: Read + Seek>(map: &mut Map<R>) -> Result<Sha256> {
    let scenario = map
        .scenario()
        .ok_or(Error::Invalid("map has no scenario"))?
        .clone();
    let location = scenario
        .location
        .ok_or(Error::Invalid("scenario has no payload"))?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    let entries = view.block(view.at(0, 0x218)?, 0x210, 0x44)?;
    let mut h = content::Hasher::default();
    for e in entries {
        let word = |o: usize| u32::from_le_bytes([e[o], e[o + 1], e[o + 2], e[o + 3]]);
        let (offset, size) = (word(0), word(4));
        if offset == 0 {
            h.update(&0u32.to_le_bytes());
            continue;
        }
        h.update(&size.to_le_bytes());
        h.update(&map.read_file_range(offset.into(), size.into())?);
        let bsp = StructureRender::read(map, map::TagId(word(0x14)))?;
        let headers = bsp
            .clusters
            .iter()
            .map(|c| &c.geometry)
            .chain(bsp.instance_definitions.iter().map(|d| &d.geometry));
        for g in headers {
            hash_block(&mut h, map, g)?;
        }
    }
    Ok(h.finish())
}

fn hash_block<R: Read + Seek>(
    h: &mut content::Hasher,
    map: &mut Map<R>,
    g: &GeometryHeader,
) -> Result<()> {
    let size = g.block.block_size;
    match g.block.block {
        None => h.update(&[0]),
        Some(RawPointer {
            file: ResourceFile::Local,
            offset,
        }) => {
            h.update(&[1]);
            h.update(&size.to_le_bytes());
            h.update(&map.read_file_range(offset.into(), size.into())?);
        }
        Some(RawPointer { file, offset }) => {
            let selector: u32 = match file {
                ResourceFile::Local => 0,
                ResourceFile::MainMenu => 1,
                ResourceFile::Shared => 2,
                ResourceFile::SinglePlayerShared => 3,
            };
            h.update(&[2]);
            h.update(&(selector << 30 | offset).to_le_bytes());
            h.update(&size.to_le_bytes());
        }
    }
    Ok(())
}
