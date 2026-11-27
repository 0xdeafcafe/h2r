//! `map` and `map-patch` items (docs/editor/studio-redesign.md, "Contents";
//! "How the game consumes packages", Prepare): whole maps, built to map
//! packages under `<data>/maps/` before the game boots.
//!
//! - A `map` item's source is a `map.toml` ([`MapSpec`]): a `mode`
//!   (`fresh` or `onto`), the disc map it is `base`d on, the `name` the new
//!   map is served as and, for `fresh`, a `scenario` and the `kits` that
//!   build it ([`Fresh`]). The items it takes are the package set's items
//!   whose `maps` list names the map, so the build that makes the map is
//!   the build the game runs on any other map ([`crate::build_map`]). The
//!   output is a derived map package: one whole map, listed as its base's
//!   entry gives it.
//! - A `map-patch` item's source is a PPF, IPS or BPS file and its one
//!   `inputs` file a TOML sidecar naming the base map and its SHA-256
//!   ([`PatchBase`]). The output is the package the importer writes for the
//!   same patch (`h2 live maps import`): the retail base and one layer, so
//!   its `MapId` is the importer's.
//!
//! Neither builds into another map: [`builders::Builder::apply`] does
//! nothing for them, and the rewrite leaves out the items a built map took
//! when it serves that map ([`built_names`], [`restrict`]) so they are not
//! applied twice.
//!
//! A map package a build wrote holds the key it was made for
//! ([`MapPackage::key`], in `built-from.toml`): [`prepare`] builds a `map`
//! item again only when the base map, the packages it takes, the builders
//! or the `fresh` kit ([`Fresh::version`]) changed.
//!
//! A `map` item with a `ground` file (a map with AI, [`GroundFile`]) builds
//! the ground over the map the items make, through [`Fresh::ground`]: the
//! generator is above this crate. The ground file and the builder's
//! version are in the key, so a map whose labels did not change is not
//! built again.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use content::manifest::{Layer, ListInfo, Manifest as MapManifest, is_map_name};
use content::package::{Item, Kind, Maps, Package};
use content::patch::Patch;
use content::{MapId, Sha256, store};
use serde::{Deserialize, Serialize};

use crate::build::{Failure, Loaded, Target};
use crate::ground::GroundFile;
use crate::builders::Job;
use crate::onto::{build_map, scope_of_bytes};

/// The marker a built map package carries, so a rebuild replaces only
/// what this build wrote.
pub const BUILT_FILE: &str = "built-from.toml";

/// The key a map package built from the tag tree carries in [`BUILT_FILE`]
/// (`project_build::write`); its build key is in its `receipt.json`.
pub const TREE_KEY: &str = "tag-tree";

/// What a `map` item builds from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// A new map made by a builder (the stage), over the base's globals
    /// and header.
    Fresh,
    /// The base map as it is, plus the items the map takes.
    Onto,
}

/// A `map` item's `map.toml`: also the tag workspace's project
/// (docs/editor/tag-workspace.md, "Projects"), one file per map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapSpec {
    pub mode: Mode,
    /// The disc map it is made over (`lockout`).
    pub base: String,
    /// The map's name and package directory (`stage`). Not the base's.
    pub name: String,
    /// `fresh`: the scenario's tag path.
    #[serde(default)]
    pub scenario: Option<String>,
    /// `fresh`: the kits that build it, in order.
    #[serde(default)]
    pub kits: Vec<String>,
    /// A file of the package with the map's AI ground ([`GroundFile`]): the
    /// ground is built over the map the item makes.
    #[serde(default)]
    pub ground: Option<String>,
    /// The package's display name; else the map's.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Edit files the tree build applies: `assets/tag-edits/<name>.yaml`
    /// in Git, or a local file's name ([`crate::project::EditRef`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<String>,
    /// Import layers the tree build reads, top first:
    /// `<data>/workspace/imports/<name>/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
}

impl MapSpec {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "map.toml is not UTF-8".to_owned())?;
        let spec: Self = toml::from_str(text).map_err(|e| format!("map.toml: {e}"))?;
        spec.validate()?;
        Ok(spec)
    }

    fn validate(&self) -> Result<(), String> {
        for (what, name) in [("base", &self.base), ("name", &self.name)] {
            if !is_map_name(name) {
                return Err(format!("map.toml: {what} {name:?} is not a map name"));
            }
        }
        if !store::is_dir_name(&self.name) {
            return Err(format!(
                "map.toml: name {:?} is not a directory name",
                self.name
            ));
        }
        if self.name.eq_ignore_ascii_case(&self.base) {
            return Err(format!(
                "map.toml: name {:?} is the base's: a map item makes a new map",
                self.name
            ));
        }
        match self.mode {
            Mode::Fresh => {
                if self.scenario.as_deref().is_none_or(str::is_empty) {
                    return Err("map.toml: a fresh map needs a scenario".into());
                }
                if self.kits.is_empty() {
                    return Err("map.toml: a fresh map needs kits".into());
                }
            }
            Mode::Onto => {
                if self.scenario.is_some() || !self.kits.is_empty() {
                    return Err("map.toml: scenario and kits are for a fresh map".into());
                }
            }
        }
        if let Some(s) = &self.scenario {
            let ok = !s.is_empty()
                && s.len() < 256
                && !s.contains('/')
                && s.rsplit('\\').next().is_some_and(|n| !n.is_empty());
            if !ok {
                return Err(format!("map.toml: scenario {s:?} is not a tag path"));
            }
        }
        let mut seen = BTreeSet::new();
        for k in &self.kits {
            if k.is_empty() || !seen.insert(("kit", k.as_str())) {
                return Err(format!("map.toml: kits: {k:?} is empty or named twice"));
            }
        }
        for e in &self.edits {
            crate::project::EditRef::parse(e).map_err(|e| format!("map.toml: {e}"))?;
            if !seen.insert(("edits", e.as_str())) {
                return Err(format!("map.toml: edits: {e:?} is named twice"));
            }
        }
        for i in &self.imports {
            if !crate::edits::is_plain_name(i) || !seen.insert(("import", i.as_str())) {
                return Err(format!(
                    "map.toml: imports: {i:?} is not an import layer's name, or is named twice"
                ));
            }
        }
        Ok(())
    }

    /// The name the map is served as, lowercase.
    pub fn served(&self) -> String {
        self.name.to_ascii_lowercase()
    }
}

/// A `map-patch` item's sidecar: the map the patch is for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBase {
    /// The disc map (`lockout`).
    pub map: String,
    /// SHA-256 of that disc map.
    pub sha256: Sha256,
    /// The package's display name; else the patch file's stem.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// The directory under `<data>/maps/`; else `<package id>-<map>`.
    #[serde(default)]
    pub dir: Option<String>,
}

impl PatchBase {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text =
            std::str::from_utf8(bytes).map_err(|_| "the base file is not UTF-8".to_owned())?;
        let b: Self = toml::from_str(text).map_err(|e| format!("the base file: {e}"))?;
        if !is_map_name(&b.map) {
            return Err(format!("the base file: map {:?} is not a map name", b.map));
        }
        if let Some(d) = &b.dir
            && !store::is_dir_name(d)
        {
            return Err(format!("the base file: dir {d:?} is not a directory name"));
        }
        Ok(b)
    }
}

/// The sidecar of a `map-patch` item: its one `inputs` file.
fn patch_base(job: &Job<'_>) -> Result<PatchBase, String> {
    let [path] = job.item.inputs.as_slice() else {
        return Err("a map-patch item names its base map and SHA-256 in one inputs file".into());
    };
    let bytes = job
        .files
        .get(path)
        .ok_or_else(|| format!("the package has no file {path:?}"))?;
    PatchBase::parse(bytes).map_err(|e| format!("{path}: {e}"))
}

/// `map` builder: nothing changes in the map it runs on.
pub(crate) fn apply(_: &mut crate::Cx<'_>, _: &Job<'_>) -> Result<(), Failure> {
    Ok(())
}

pub(crate) fn check(job: &Job<'_>) -> Result<(), Failure> {
    let spec = MapSpec::parse(job.source)?;
    if let Some(g) = &spec.ground {
        let bytes = job
            .files
            .get(g)
            .ok_or_else(|| format!("the package has no ground file {g:?}"))?;
        GroundFile::parse(bytes).map_err(|e| format!("{g}: {e}"))?;
    }
    Ok(())
}

pub(crate) fn patch_apply(_: &mut crate::Cx<'_>, _: &Job<'_>) -> Result<(), Failure> {
    Ok(())
}

pub(crate) fn patch_check(job: &Job<'_>) -> Result<(), Failure> {
    if !Patch::recognises(job.source) {
        return Err("not a PPF, IPS or BPS file".into());
    }
    patch_base(job)?;
    Ok(())
}

/// Builds a `fresh` map: the one thing `workspace` cannot do itself, since
/// the kits live above it. The bytes are a whole map file.
pub trait Fresh {
    fn build(&self, job: &FreshJob<'_>) -> Result<Vec<u8>, String>;

    /// A word that changes when the same job would give other bytes (the
    /// kit's version and the sources it reads): part of the key that lets
    /// [`prepare`] leave a map it built before alone.
    fn version(&self) -> String {
        String::new()
    }

    /// Build the AI ground a `map` item's ground file asks for over the
    /// map the item makes: the whole map with the ground in it. A build
    /// with no generator stops the item with the reason.
    fn ground(&self, _: &GroundJob<'_>) -> Result<Vec<u8>, String> {
        Err("this build has no builder for AI ground".into())
    }

    /// [`Fresh::version`] for the ground generator: the kit's version.
    fn ground_version(&self) -> String {
        String::new()
    }
}

/// What [`Fresh::ground`] is asked for.
pub struct GroundJob<'a> {
    /// The map's name.
    pub name: &'a str,
    /// The disc map it is over.
    pub base: &'a str,
    /// The map as the item makes it, before the ground.
    pub map: &'a [u8],
    pub ground: &'a GroundFile,
}

/// What a [`Fresh`] builder is asked for.
pub struct FreshJob<'a> {
    /// The new map's name.
    pub name: &'a str,
    /// The disc map it is over, and that map's bytes.
    pub base: &'a str,
    pub base_bytes: &'a [u8],
    pub scenario: &'a str,
    pub kits: &'a [String],
    /// The package's files, for a kit that reads one.
    pub files: &'a BTreeMap<String, Vec<u8>>,
}

/// A build with no `fresh` builder: every `fresh` map stops with the reason.
pub struct NoFresh;

impl Fresh for NoFresh {
    fn build(&self, _: &FreshJob<'_>) -> Result<Vec<u8>, String> {
        Err("this build has no builder for fresh maps".into())
    }
}

/// Where the disc maps come from (the VFS, in the game).
pub trait BaseMaps {
    /// The disc's map `name`, or why it is not there.
    fn read(&self, name: &str) -> Result<Vec<u8>, String>;
}

/// A map package a build made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPackage {
    /// The directory under `<data>/maps/`.
    pub dir: String,
    /// The package and item that made it.
    pub package: String,
    pub item: usize,
    pub manifest: MapManifest,
    /// The files the manifest names, and the image if any.
    pub files: Vec<(String, Vec<u8>)>,
    pub id: MapId,
    /// What the build was made from (empty for a `map-patch` item, whose
    /// output is a function of its files): [`prepare`] builds nothing
    /// again for a directory that holds this key.
    pub key: String,
}

/// A map package already in place for the key its item would build to:
/// not built again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub dir: String,
    pub package: String,
    pub item: usize,
    pub id: MapId,
}

impl From<&MapPackage> for Held {
    fn from(m: &MapPackage) -> Self {
        Self {
            dir: m.dir.clone(),
            package: m.package.clone(),
            item: m.item,
            id: m.id,
        }
    }
}

/// What an item came to.
enum Outcome {
    Built(MapPackage),
    Held(Held),
}

/// An item that did not build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub package: String,
    pub item: usize,
    pub why: String,
}

/// What [`build_maps`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapsBuilt {
    pub maps: Vec<MapPackage>,
    /// Items whose directory held their key already.
    pub held: Vec<Held>,
    pub failed: Vec<Failed>,
}

/// The names of the maps `packages`' `map` items build, lowercase; an item
/// whose `map.toml` does not read names none.
pub fn built_names(packages: &[Loaded]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for p in packages {
        for item in &p.package.manifest.items {
            if item.kind != Kind::Map {
                continue;
            }
            let spec = item
                .source
                .as_ref()
                .and_then(|s| p.files.get(s))
                .and_then(|b| MapSpec::parse(b).ok());
            if let Some(spec) = spec {
                out.insert(spec.served());
            }
        }
    }
    out
}

/// Whether `item` is one a map named `name` takes: an item that changes a
/// map and lists `name` in its `maps`.
pub(crate) fn taken_by(item: &Item, name: &str) -> bool {
    !matches!(item.kind, Kind::Map | Kind::MapPatch)
        && crate::builders::changes_map(item)
        && matches!(&item.maps, Maps::Names(n) if n.iter().any(|n| n.eq_ignore_ascii_case(name)))
}

/// `packages` cut to the items `keep` says; a package left with none is
/// dropped. The id of a cut package is its own, so a cache key over it is
/// for what is applied.
pub fn restrict(packages: &[Loaded], keep: &dyn Fn(&Item) -> bool) -> Vec<Loaded> {
    let mut out = Vec::new();
    for l in packages {
        let mut manifest = l.package.manifest.clone();
        let before = manifest.items.len();
        manifest.items.retain(|i| keep(i));
        if manifest.items.is_empty() {
            continue;
        }
        if manifest.items.len() == before {
            out.push(l.clone());
            continue;
        }
        if let Ok(package) = Package::new(manifest, l.package.lock.clone()) {
            out.push(Loaded {
                package,
                files: l.files.clone(),
            });
        }
    }
    out
}

/// What a built map `name` serves with: `packages` without the items it
/// took, which are in its bytes already.
pub fn without_taken(packages: &[Loaded], name: &str) -> Vec<Loaded> {
    restrict(packages, &|i| !taken_by(i, name))
}

fn short(sha: &Sha256) -> String {
    sha.to_string()[..8].to_owned()
}

fn clip(mut dir: String) -> String {
    dir.truncate(64);
    dir.trim_end_matches('-').to_owned()
}

/// Which directory holds which key: `(dir, package, item, key)` to the id
/// of the package there, when a build for that key is in place.
pub type HeldIn<'a> = dyn Fn(&str, &str, usize, &str) -> Option<MapId> + 'a;

/// Build the map packages of every `map` and `map-patch` item of
/// `packages`. An item that cannot build (no disc, a failing kit) is listed
/// in `failed` and the others still build.
pub fn build_maps(packages: &[Loaded], bases: &dyn BaseMaps, fresh: &dyn Fresh) -> MapsBuilt {
    build_maps_held(packages, bases, fresh, &|_, _, _, _| None)
}

/// [`build_maps`], leaving out the `map` items `held` says are in place.
pub fn build_maps_held(
    packages: &[Loaded],
    bases: &dyn BaseMaps,
    fresh: &dyn Fresh,
    held: &HeldIn<'_>,
) -> MapsBuilt {
    let mut out = MapsBuilt::default();
    for p in packages {
        let package = p.package.manifest.package.id.clone();
        for (index, item) in p.package.manifest.items.iter().enumerate() {
            let built = match item.kind {
                Kind::Map => build_map_item(packages, p, index, item, bases, fresh, held),
                Kind::MapPatch => build_patch_item(p, item).map(Outcome::Built),
                _ => continue,
            };
            match built {
                Ok(Outcome::Built(mut m)) => {
                    m.package = package.clone();
                    m.item = index;
                    out.maps.push(m);
                }
                Ok(Outcome::Held(h)) => out.held.push(h),
                Err(why) => out.failed.push(Failed {
                    package: package.clone(),
                    item: index,
                    why,
                }),
            }
        }
    }
    let mut seen: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for h in &out.held {
        seen.insert(h.dir.clone(), (h.package.clone(), h.item));
    }
    let mut keep = Vec::new();
    for m in std::mem::take(&mut out.maps) {
        if let Some((by, item)) = seen.get(&m.dir) {
            out.failed.push(Failed {
                package: m.package,
                item: m.item,
                why: format!(
                    "directory {:?} is already written by package {by:?} item {item}",
                    m.dir
                ),
            });
        } else {
            seen.insert(m.dir.clone(), (m.package.clone(), m.item));
            keep.push(m);
        }
    }
    out.maps = keep;
    out
}

fn source_of<'a>(p: &'a Loaded, item: &Item) -> Result<&'a [u8], String> {
    let path = item.source.as_ref().ok_or("the item has no source")?;
    p.files
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("the package has no file {path:?}"))
}

fn build_map_item(
    all: &[Loaded],
    p: &Loaded,
    index: usize,
    item: &Item,
    bases: &dyn BaseMaps,
    fresh: &dyn Fresh,
    held: &HeldIn<'_>,
) -> Result<Outcome, String> {
    let spec = MapSpec::parse(source_of(p, item)?)?;
    if !spec.edits.is_empty() || !spec.imports.is_empty() {
        // The tag workspace's build of this item is the map: kept as it is.
        let package = p.package.manifest.package.id.clone();
        if let Some(id) = held(&spec.served(), &package, index, TREE_KEY) {
            return Ok(Outcome::Held(Held {
                dir: spec.served(),
                package,
                item: index,
                id,
            }));
        }
        return Err(format!(
            "map {:?} names edit files or import layers, which apply to the tag tree: it builds with the tag workspace (Studio's project.build, h2 tags project build), not from the disc map",
            spec.name
        ));
    }
    let ground = match &spec.ground {
        Some(file) => {
            let bytes = p
                .files
                .get(file)
                .ok_or_else(|| format!("the package has no ground file {file:?}"))?;
            Some((GroundFile::parse(bytes).map_err(|e| format!("{file}: {e}"))?, bytes))
        }
        None => None,
    };
    let base = bases
        .read(&spec.base.to_ascii_lowercase())
        .map_err(|e| format!("the base map {}: {e}", spec.base))?;
    let name = spec.served();
    let key = match scope_of_bytes(&base) {
        Some(scope) => {
            let target = Target { name: &name, scope };
            let mut text = crate::cache_key(&Sha256::of(&base), all, &target).to_string();
            text.push('\n');
            text.push_str(&p.package.manifest.package.id.to_string());
            text.push('\n');
            text.push_str(&index.to_string());
            text.push('\n');
            if spec.mode == Mode::Fresh {
                text.push_str(&fresh.version());
            }
            if let Some((_, bytes)) = &ground {
                text.push('\n');
                text.push_str(&Sha256::of(bytes).to_string());
                text.push('\n');
                text.push_str(&fresh.ground_version());
            }
            Sha256::of(text.as_bytes()).to_string()
        }
        None => String::new(),
    };
    let package = p.package.manifest.package.id.clone();
    if !key.is_empty()
        && let Some(id) = held(&name, &package, index, &key)
    {
        return Ok(Outcome::Held(Held {
            dir: name,
            package,
            item: index,
            id,
        }));
    }
    let mut bytes = match spec.mode {
        Mode::Onto => base.clone(),
        Mode::Fresh => fresh.build(&FreshJob {
            name: &name,
            base: &spec.base.to_ascii_lowercase(),
            base_bytes: &base,
            scenario: spec.scenario.as_deref().unwrap_or_default(),
            kits: &spec.kits,
            files: &p.files,
        })?,
    };
    let taken = restrict(all, &|i| taken_by(i, &name));
    if taken.is_empty() && spec.mode == Mode::Onto && ground.is_none() {
        return Err(format!(
            "no item names the map {name:?} in its maps: an onto map is its base plus what it takes"
        ));
    }
    if !taken.is_empty() {
        let scope = scope_of_bytes(&bytes).ok_or("the map's header does not read")?;
        let target = Target { name: &name, scope };
        bytes = build_map(&bytes, &taken, &target)?.0;
    }
    if let Some((file, _)) = &ground {
        bytes = fresh.ground(&GroundJob {
            name: &name,
            base: &spec.base.to_ascii_lowercase(),
            map: &bytes,
            ground: file,
        })?;
    }
    let (manifest, file) = derived_manifest(&spec, &bytes);
    let id = manifest.map_id();
    Ok(Outcome::Built(MapPackage {
        dir: name,
        package: String::new(),
        item: 0,
        manifest,
        files: vec![(file, bytes)],
        id,
        key,
    }))
}

/// The manifest of the derived map package a `map` item makes: one whole
/// map, `<name>.map`, listed as its base's entry gives it. The disc build
/// and the tree build (`project_build::write`) both write it.
pub(crate) fn derived_manifest(spec: &MapSpec, bytes: &[u8]) -> (MapManifest, String) {
    let name = spec.served();
    let mut manifest = MapManifest::retail(&name, Sha256::of(bytes));
    manifest.base.sha256 = None;
    manifest.base.derived = true;
    manifest.package.name = spec.title.clone().unwrap_or_else(|| name.clone());
    manifest.package.author = spec.author.clone();
    manifest.package.description = spec.description.clone();
    let file = format!("{name}.map");
    manifest.patches = vec![Layer {
        file: file.clone(),
        sha256: Sha256::of(bytes),
    }];
    manifest.list = Some(ListInfo {
        base: Some(spec.base.to_ascii_lowercase()),
        ..ListInfo::default()
    });
    (manifest, file)
}

fn build_patch_item(p: &Loaded, item: &Item) -> Result<MapPackage, String> {
    let patch = source_of(p, item)?;
    let ext = if patch.starts_with(b"PPF") {
        "ppf"
    } else if patch.starts_with(b"PATCH") {
        "ips"
    } else if patch.starts_with(b"BPS1") {
        "bps"
    } else {
        return Err("not a PPF, IPS or BPS file".into());
    };
    let job = Job {
        item,
        source: patch,
        files: &p.files,
    };
    let base = patch_base(&job)?;
    let map = base.map.to_ascii_lowercase();
    let sha = Sha256::of(patch);
    let stem = item
        .source
        .as_deref()
        .and_then(|s| Path::new(s).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or(&map)
        .to_owned();
    // The layer is the patch as given, as the importer keeps one.
    let mut manifest = MapManifest::retail(&map, base.sha256);
    manifest.package.name = base.title.clone().unwrap_or(stem);
    manifest.package.author = base.author.clone();
    manifest.package.description = base.description.clone();
    let file = format!("{map}-{}.{ext}", short(&sha));
    manifest.patches = vec![Layer {
        file: file.clone(),
        sha256: sha,
    }];
    let dir = base
        .dir
        .clone()
        .unwrap_or_else(|| clip(format!("{}-{map}", p.package.manifest.package.id)));
    let id = manifest.map_id();
    Ok(MapPackage {
        dir,
        package: String::new(),
        item: 0,
        manifest,
        files: vec![(file, patch.to_vec())],
        id,
        key: String::new(),
    })
}

/// What [`write`] did to a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrote {
    Written,
    /// The directory held this package already.
    Unchanged,
}

fn marker(m: &MapPackage) -> String {
    format!(
        "package = {:?}\nitem = {}\nid = \"{}\"\nkey = \"{}\"\n",
        m.package, m.item, m.id, m.key
    )
}

/// The id of the package `<maps>/<dir>/` holds when it was built by item
/// `item` of `package` for `key`, and its map file is there.
pub fn held_in(maps: &Path, dir: &str, package: &str, item: usize, key: &str) -> Option<MapId> {
    let text = std::fs::read_to_string(maps.join(dir).join(BUILT_FILE)).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let [p, i, id, k] = lines.as_slice() else {
        return None;
    };
    let key_ok = *k == format!("key = \"{key}\"");
    if *p != format!("package = {package:?}") || *i != format!("item = {item}") || !key_ok {
        return None;
    }
    if !maps.join(dir).join(format!("{dir}.map")).is_file() {
        return None;
    }
    id.strip_prefix("id = \"")?.strip_suffix('"')?.parse().ok()
}

/// Write `m` to `<maps>/<dir>/`: nothing when the directory holds the
/// package already, a replacement when it holds an older build of the same
/// item, and a refusal when it holds anything else (a package the user
/// made or imported).
pub fn write(maps: &Path, m: &MapPackage) -> Result<Wrote, String> {
    let target = maps.join(&m.dir);
    let want = marker(m);
    if target.exists() {
        let have = std::fs::read_to_string(target.join(BUILT_FILE)).ok();
        let Some(have) = have else {
            return Err(format!(
                "{} exists and was not built from a package",
                target.display()
            ));
        };
        let same_item = have.lines().take(2).eq(want.lines().take(2));
        if !same_item {
            return Err(format!(
                "{} was built by another package item",
                target.display()
            ));
        }
        let files_ok = m.files.iter().all(|(n, b)| {
            std::fs::read(target.join(n)).is_ok_and(|have| Sha256::of(&have) == Sha256::of(b))
        });
        if have == want && files_ok {
            return Ok(Wrote::Unchanged);
        }
    }
    let mut files: Vec<(&str, &[u8])> = m
        .files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    files.push((BUILT_FILE, want.as_bytes()));
    store::write_package(maps, &m.dir, &m.manifest, &files, true).map_err(|e| e.to_string())?;
    Ok(Wrote::Written)
}

/// What [`prepare`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prepared {
    pub written: Vec<MapPackage>,
    /// Packages already in place: left as they are.
    pub unchanged: Vec<Held>,
    pub failed: Vec<Failed>,
}

/// [`build_maps_held`] and write each to `maps` (`<data>/maps/`): a map
/// already there for the same inputs is not built again. The map
/// packages are the form the VFS loads and Live names; nothing else about
/// them is new.
pub fn prepare(
    maps: &Path,
    packages: &[Loaded],
    bases: &dyn BaseMaps,
    fresh: &dyn Fresh,
) -> Prepared {
    let built = build_maps_held(packages, bases, fresh, &|dir, package, item, key| {
        held_in(maps, dir, package, item, key)
    });
    let mut out = Prepared {
        unchanged: built.held,
        failed: built.failed,
        ..Prepared::default()
    };
    for m in built.maps {
        match write(maps, &m) {
            Ok(Wrote::Written) => out.written.push(m),
            Ok(Wrote::Unchanged) => out.unchanged.push(Held::from(&m)),
            Err(why) => out.failed.push(Failed {
                package: m.package,
                item: m.item,
                why,
            }),
        }
    }
    out
}
