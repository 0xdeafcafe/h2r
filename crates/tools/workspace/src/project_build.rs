//! The build of a project into a map (docs/editor/tag-workspace.md,
//! "Projects" and "Kits are build steps").
//!
//! [`build`] in order:
//!
//! | Step | `fresh` | `onto` |
//! |---|---|---|
//! | 1 | the stack ([`open_stack`]), every edit applied once ([`Edited::check`]); a problem stops the build | the same |
//! | 2 | when the stack holds the scenario: closure from the globals, the scenario and the [`Keep`] roots; a missing or lossy tag stops it, named; [`to_tag_set`] | the disc map reassembled from the tree, every tag pinned, the tags the layers changed rebuilt ([`onto_retail`]); a changed tag the base keeps pinned, or one the base lacks, stops it, named |
//! | 3 | each kit, in order, on [`KitStep`]: the first may make the map when the stack has no scenario (the stage) | each kit, in order |
//! | 4 | lay out (`TagSet::build`), unless a kit laid it out; read it back; tag memory against the bound for its type | the same |
//!
//! [`write`] puts the map in a package under `<data>/maps/<name>/`: a
//! derived whole map for `fresh`, a patch of the disc map for `onto`, each
//! with `receipt.json` ([`Receipt`]).
//!
//! A kit is a [`Kit`]: the caller passes the kits it has ([`Tools`]), so
//! this crate needs none of them. A kit that lays out the whole map itself
//! sets [`KitStep::bytes`] and must be the project's last.
use std::collections::BTreeSet;
use std::io::Cursor;

use content::Sha256;
use content::manifest::Layer;
use map::FourCC;
use map::build::{TagPayload, TagSet};
use serde::Serialize;
use tags::repo::build::onto_retail;
use tags::repo::{BuildOptions, LooseTag, RawRes, TagKey, TagRepo, TagSource, closure, to_tag_set};

use crate::edits::{Override, key_text};
use crate::project::{
    BuildMode, LayerInfo, Project, ProjectError, ProjectFile, Roots, Stack, open_stack, open_tree,
};

const SCNR: FourCC = FourCC(*b"scnr");
const SBSP: FourCC = FourCC(*b"sbsp");
const LTMP: FourCC = FourCC(*b"ltmp");

fn invalid(why: impl Into<String>) -> ProjectError {
    ProjectError::Invalid(why.into())
}

/// The tags a fresh closure keeps as roots besides the globals and the
/// scenario: the ones the engine finds by name
/// (`h1convert::fresh::{KEEP_GROUPS, KEEP_PREFIXES}`, which the caller
/// passes: this crate is below the converter). A base tag of a kept group
/// or under a kept prefix is a root; the base's own `scnr`, `sbsp` and
/// `ltmp` never are.
#[derive(Clone, Copy, Debug)]
pub struct Keep {
    pub groups: &'static [&'static [u8; 4]],
    pub prefixes: &'static [&'static str],
}

impl Keep {
    pub const NONE: Keep = Keep {
        groups: &[],
        prefixes: &[],
    };
}

/// One build step: content added by code.
pub trait Kit {
    /// The name a project's `kits` gives.
    fn name(&self) -> &str;
    /// What the kit builds from (its version, its sources' hash): part of
    /// the build's key.
    fn fingerprint(&self) -> String;
    /// Add the kit's content: install into [`KitStep::set`], make it (a
    /// fresh project's first kit, when the stack has no scenario), or lay
    /// out the whole map into [`KitStep::bytes`].
    fn run(&self, step: &mut KitStep<'_>) -> Result<Installed, String>;
}

/// What a kit runs on.
pub struct KitStep<'a> {
    pub project: &'a Project,
    pub roots: &'a Roots,
    /// The stack: every tag with the project's edits.
    pub src: &'a mut dyn TagSource,
    /// The map so far. `None` in a fresh build whose stack holds no
    /// scenario, until a kit makes it.
    pub set: Option<TagSet>,
    /// The finished map, when a kit laid it out.
    pub bytes: Option<Vec<u8>>,
}

/// What a kit added.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Installed {
    /// Tags added or made, `<group>:<path>`.
    pub added: Vec<String>,
    pub notes: Vec<String>,
}

/// What a build has besides the project.
#[derive(Clone, Copy)]
pub struct Tools<'a> {
    pub kits: &'a [&'a dyn Kit],
    pub keep: Keep,
}

impl Tools<'_> {
    /// The kits `project` names, in its order; a name this build does not
    /// have is an error listing the ones it does.
    pub fn kits_of(&self, project: &Project) -> Result<Vec<&dyn Kit>, ProjectError> {
        project
            .kits
            .iter()
            .map(|n| {
                self.kits
                    .iter()
                    .copied()
                    .find(|k| k.name() == n)
                    .ok_or_else(|| {
                        let have: Vec<&str> = self.kits.iter().map(|k| k.name()).collect();
                        invalid(format!(
                            "kit {n:?} is not one this build has (it has: {})",
                            if have.is_empty() {
                                "none".to_owned()
                            } else {
                                have.join(", ")
                            }
                        ))
                    })
            })
            .collect()
    }
}

/// A kit as the receipt lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KitRan {
    pub name: String,
    pub fingerprint: String,
    pub added: Vec<String>,
    pub notes: Vec<String>,
}

/// What a build did, written beside the map as `receipt.json`.
#[derive(Clone, Debug, Serialize)]
pub struct Receipt {
    /// SHA-256 over the project, its edit files, its kits' fingerprints and
    /// the base's manifest and header in the tree.
    pub key: String,
    pub project: String,
    pub mode: BuildMode,
    pub base: String,
    pub layers: Vec<LayerInfo>,
    pub edit_files: Vec<String>,
    /// The tags an edit names, `<group>:<path>`.
    pub edited: Vec<String>,
    pub overrides: Vec<Override>,
    pub kits: Vec<KitRan>,
    /// `fresh`: the tags the closure reached (0 when a kit made the map).
    pub closure: usize,
    /// `onto`: the tags rebuilt from the layers.
    pub rebuilt: Vec<String>,
    pub file_size: u64,
    /// Tag memory used (header `+0x1c`) and the bound for the map's type.
    pub memory_size: u32,
    pub memory_max: u32,
}

/// A built map.
#[derive(Clone, Debug)]
pub struct Built {
    pub bytes: Vec<u8>,
    /// `onto`: the disc map it patches.
    pub base_bytes: Option<Vec<u8>>,
    pub receipt: Receipt,
}

fn listed(what: &str, items: Vec<String>) -> ProjectError {
    let n = items.len();
    let shown: Vec<String> = items.into_iter().take(8).collect();
    let more = if n > shown.len() {
        format!("; and {} more", n - shown.len())
    } else {
        String::new()
    };
    invalid(format!("{n} {what}: {}{more}", shown.join("; ")))
}

/// A fresh closure's roots: the globals, the scenario, then the base's
/// tags [`Keep`] names, each once.
pub fn fresh_roots(
    base: &tags::repo::store::Manifest,
    globals: &TagKey,
    scenario: &TagKey,
    keep: Keep,
) -> Vec<TagKey> {
    let mut roots = vec![globals.clone(), scenario.clone()];
    for (k, _) in &base.tags {
        if [SCNR, SBSP, LTMP].contains(&k.group) {
            continue;
        }
        let group = keep.groups.iter().any(|g| k.group == FourCC(**g));
        let prefix = keep.prefixes.iter().any(|p| k.path.starts_with(p));
        if (group || prefix) && !roots.contains(k) {
            roots.push(k.clone());
        }
    }
    roots
}

/// The tags of a fresh closure that are not exact in the tree, with what
/// each lost: what refuses a `fresh` build of the project.
pub fn lossy(tags: &[LooseTag]) -> Vec<String> {
    tags.iter()
        .filter_map(|t| {
            let tree = t.tree()?;
            (!tree.losses.is_empty())
                .then(|| format!("{} ({})", key_text(&t.key), tree.losses.join("; ")))
        })
        .collect()
}

/// The globals the tree's manifest of the base names.
fn globals_of(m: &tags::repo::store::Manifest) -> Result<TagKey, ProjectError> {
    m.globals
        .clone()
        .ok_or_else(|| invalid(format!("the tree's manifest of {} names no globals", m.map)))
}

/// A fresh project's closure over the stack, when the stack holds its
/// scenario; `None` when it does not (a kit makes it). A missing tag is an
/// error; lossy tags are left for the caller ([`lossy`]).
pub fn fresh_closure(
    roots: &Roots,
    project: &Project,
    stack: &mut Stack,
    keep: Keep,
) -> Result<Option<tags::repo::Closure>, ProjectError> {
    let top = open_tree(roots, &project.base)?;
    let m = top.manifest(&project.base)?;
    let scenario = project
        .scenario_key()
        .ok_or_else(|| invalid("an onto project has no closure of its own"))?;
    if stack.read(&scenario)?.is_none() {
        return Ok(None);
    }
    let globals = globals_of(&m)?;
    let c = closure(stack, &fresh_roots(&m, &globals, &scenario, keep))?;
    if !c.missing.is_empty() {
        let items = c
            .missing
            .iter()
            .map(|(to, from)| format!("{} (named by {})", key_text(to), key_text(from)))
            .collect();
        return Err(listed("tags no layer holds", items));
    }
    Ok(Some(c))
}

/// The tags of `onto`'s stack that differ from the tree: the edited ones,
/// and those of an import layer the tree lacks or has other content for.
fn changed(stack: &mut Stack, tree: &mut TagRepo) -> Result<BTreeSet<TagKey>, ProjectError> {
    let mut out: BTreeSet<TagKey> = stack.edited().into_iter().collect();
    let layers = stack.below().len();
    let mut upper = BTreeSet::new();
    for i in 0..layers.saturating_sub(1) {
        upper.extend(stack.below().keys_of(i));
    }
    for k in upper {
        if out.contains(&k) {
            continue;
        }
        let ours = stack.read(&k)?;
        let theirs = tree.read(&k)?;
        let same = match (&ours, &theirs) {
            (Some(a), Some(b)) => tags::repo::canonical(a) == tags::repo::canonical(b),
            _ => false,
        };
        if !same {
            out.insert(k);
        }
    }
    Ok(out)
}

/// `src` with only `keys` in it: what `onto` rebuilds.
struct Only<'a> {
    src: &'a mut dyn TagSource,
    keys: &'a BTreeSet<TagKey>,
}

impl TagSource for Only<'_> {
    fn keys(&self) -> Vec<TagKey> {
        self.keys.iter().cloned().collect()
    }

    fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
        if self.keys.contains(key) {
            self.src.read(key)
        } else {
            Ok(None)
        }
    }

    fn raw(&mut self, raw: &RawRes) -> tags::repo::Result<Vec<u8>> {
        self.src.raw(raw)
    }

    fn groups(&self) -> Vec<map::TagGroup> {
        self.src.groups()
    }
}

/// The disc map `base`, reassembled from the tree's skeleton.
fn base_map(top: &TagRepo, base: &str) -> Result<Vec<u8>, ProjectError> {
    if !top.has_skeleton(base) {
        return Err(invalid(format!(
            "an onto build starts from the disc map, and the tree holds no skeleton of {base}: extract it again with its raw resources"
        )));
    }
    let mut bytes = Vec::new();
    top.reassemble(base, &mut bytes).map_err(|e| {
        invalid(format!(
            "an onto build starts from the disc map, reassembled from the tree, and {base} does not reassemble: {e}"
        ))
    })?;
    Ok(bytes)
}

/// The build key ([`Receipt::key`]).
fn build_key(
    project: &Project,
    stack: &Stack,
    kits: &[&dyn Kit],
    manifest: &str,
    header: &[u8],
) -> String {
    let mut h = content::Hasher::default();
    let mut part = |bytes: &[u8]| {
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    part(project.to_text().as_bytes());
    for f in stack.files() {
        part(f.name.as_bytes());
        part(f.text.as_bytes());
    }
    for k in kits {
        part(k.name().as_bytes());
        part(k.fingerprint().as_bytes());
    }
    part(manifest.as_bytes());
    part(header);
    h.finish().to_string()
}

/// Build `file`'s project (module docs).
pub fn build(roots: &Roots, file: &ProjectFile, tools: &Tools<'_>) -> Result<Built, ProjectError> {
    roots.refuse_checkout()?;
    let project = &file.project;
    let kits = tools.kits_of(project)?;
    let (mut stack, layers) = open_stack(roots, project)?;
    let problems = stack.check();
    if !problems.is_empty() {
        return Err(ProjectError::Edits(problems));
    }
    let top = open_tree(roots, &project.base)?;
    let m = top.manifest(&project.base)?;
    let header = top.header(&project.base)?;
    let memory_max = map::writer::max_metadata_size(m.cache_type())
        .map_err(|e| invalid(format!("the tag memory bound: {e}")))?;
    let key = build_key(project, &stack, &kits, &m.to_text(), &header);

    let mut closure_len = 0;
    let mut rebuilt = Vec::new();
    let mut base_bytes = None;
    let set = match project.mode {
        BuildMode::Fresh => match fresh_closure(roots, project, &mut stack, tools.keep)? {
            None => None,
            Some(c) => {
                let bad = lossy(&c.tags);
                if !bad.is_empty() {
                    return Err(listed(
                        "tags the tree does not hold exactly, which a fresh build refuses",
                        bad,
                    ));
                }
                let low_detail = match &m.low_detail {
                    Some(h) => Some(top.raw_by_hash(h)?),
                    None => None,
                };
                let opts = BuildOptions {
                    cache_type: m.cache_type(),
                    name: project.name.clone(),
                    scenario: project.scenario_key().expect("a fresh project's"),
                    globals: globals_of(&m)?,
                    strings: top.strings()?,
                    header_template: Some(header.clone()),
                    low_detail,
                    index_word_14: m.index_word_14,
                };
                let (set, report) = to_tag_set(&mut stack, &c.tags, &opts)?;
                if !report.dangling.is_empty() {
                    let items = report
                        .dangling
                        .iter()
                        .map(|(from, to)| format!("{} -> {}", key_text(from), key_text(to)))
                        .collect();
                    return Err(listed("references to tags not in the map", items));
                }
                closure_len = c.tags.len();
                Some(set)
            }
        },
        BuildMode::Onto => {
            let bytes = base_map(&top, &project.base)?;
            let mut tree = TagRepo::for_map(&roots.tree, &project.base)?;
            let keys = changed(&mut stack, &mut tree)?;
            let retail = map::Map::from_reader(Cursor::new(bytes.clone()))?;
            let mut only = Only {
                src: &mut stack,
                keys: &keys,
            };
            let (set, report) = onto_retail(tags::schema::Schema::builtin(), retail, &mut only)?;
            let mut stuck = Vec::new();
            for k in &keys {
                match set.find(k.group, &k.path).and_then(|id| set.tag(id)) {
                    None => stuck.push(format!(
                        "{}: the base has no such tag, and an onto build adds none",
                        key_text(k)
                    )),
                    Some(t) if matches!(t.payload, TagPayload::Pinned { .. }) => {
                        let why = if report.lossy.contains(k) {
                            "the tree does not hold it exactly"
                        } else {
                            "the base keeps it where it is (it does not move)"
                        };
                        stuck.push(format!("{}: {why}", key_text(k)));
                    }
                    Some(_) => rebuilt.push(key_text(k)),
                }
            }
            if !stuck.is_empty() {
                return Err(listed("changed tags an onto build cannot rebuild", stuck));
            }
            base_bytes = Some(bytes);
            Some(set)
        }
    };

    let mut step = KitStep {
        project,
        roots,
        src: &mut stack,
        set,
        bytes: None,
    };
    let mut ran: Vec<KitRan> = Vec::new();
    for kit in &kits {
        if let Some(prev) = ran.last().filter(|_| step.bytes.is_some()) {
            return Err(invalid(format!(
                "kit {} lays out the whole map, so kit {} after it has nothing to install into: put {} last",
                prev.name,
                kit.name(),
                prev.name
            )));
        }
        let installed = kit
            .run(&mut step)
            .map_err(|why| invalid(format!("kit {}: {why}", kit.name())))?;
        if step.set.is_none() && step.bytes.is_none() {
            return Err(invalid(format!(
                "kit {} made no map, and no layer holds the scenario {}",
                kit.name(),
                project.scenario.as_deref().unwrap_or_default()
            )));
        }
        ran.push(KitRan {
            name: kit.name().to_owned(),
            fingerprint: kit.fingerprint(),
            added: installed.added,
            notes: installed.notes,
        });
    }
    let (set, made) = (step.set.take(), step.bytes.take());
    let bytes = match (made, set) {
        (Some(bytes), _) => bytes,
        (None, Some(set)) => {
            set.build()
                .map_err(|e| invalid(format!("laying out the map: {e}")))?
                .0
        }
        (None, None) => {
            return Err(invalid(format!(
                "nothing made the map: no layer holds the scenario {} and the project names no kit",
                project.scenario.as_deref().unwrap_or_default()
            )));
        }
    };
    map::Map::from_reader(Cursor::new(bytes.as_slice()))
        .map_err(|e| invalid(format!("the built map does not read back: {e}")))?;
    let memory_size = bytes
        .get(0x1c..0x20)
        .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
        .ok_or_else(|| invalid("the built map has no header"))?;
    if memory_size > memory_max {
        return Err(invalid(format!(
            "tag memory {memory_size:#x} is over the bound {memory_max:#x} for its type"
        )));
    }
    let receipt = Receipt {
        key,
        project: project.name.clone(),
        mode: project.mode,
        base: project.base.clone(),
        layers,
        edit_files: stack.files().iter().map(|f| f.name.clone()).collect(),
        edited: stack.edited().iter().map(key_text).collect(),
        overrides: stack.overrides().to_vec(),
        kits: ran,
        closure: closure_len,
        rebuilt,
        file_size: bytes.len() as u64,
        memory_size,
        memory_max,
    };
    Ok(Built {
        bytes,
        base_bytes,
        receipt,
    })
}

/// Where [`write`] put a build.
#[derive(Clone, Debug, Serialize)]
pub struct Wrote {
    /// The package directory.
    pub dir: String,
    /// The package's `MapId`, which the game loads it by.
    pub map_id: String,
    /// The map or patch file in it.
    pub file: String,
}

/// The receipt file beside a built map.
pub const RECEIPT_FILE: &str = "receipt.json";

/// Write `built` as the package `<data>/maps/<name>/` (module docs),
/// replacing an earlier build of the project.
pub fn write(roots: &Roots, built: &Built) -> Result<Wrote, ProjectError> {
    roots.refuse_checkout()?;
    let r = &built.receipt;
    let receipt = serde_json::to_vec_pretty(r).map_err(|e| invalid(format!("the receipt: {e}")))?;
    let (manifest, file, bytes) = match r.mode {
        BuildMode::Fresh => {
            let file = format!("{}.map", r.project);
            let text = format!(
                "[package]\nformat = 1\nname = \"{name}\"\n\
                 [base]\nmap = \"{name}\"\nderived = true\n\
                 [[patch]]\nfile = \"{file}\"\nsha256 = \"{sha}\"\n\
                 [list]\nbase = \"{base}\"\n",
                name = r.project,
                sha = Sha256::of(&built.bytes),
                base = r.base,
            );
            let m = content::Manifest::from_toml(&text)
                .map_err(|e| invalid(format!("the manifest: {e}")))?;
            (m, file, built.bytes.clone())
        }
        BuildMode::Onto => {
            let base = built
                .base_bytes
                .as_ref()
                .ok_or_else(|| invalid("an onto build without its base map"))?;
            let mut m = content::Manifest::retail(&r.base.to_ascii_lowercase(), Sha256::of(base));
            m.package.name = r.project.clone();
            let patch = content::patch::diff(base, &built.bytes, &format!("project {}", r.project));
            let sha256 = Sha256::of(&patch.bytes);
            let file = format!("project-{sha256}.{}", patch.format.extension());
            m.patches.push(Layer {
                file: file.clone(),
                sha256,
            });
            (m, file, patch.bytes)
        }
    };
    let dir = content::store::write_package(
        &roots.maps(),
        &r.project,
        &manifest,
        &[
            (file.as_str(), bytes.as_slice()),
            (RECEIPT_FILE, receipt.as_slice()),
        ],
        true,
    )
    .map_err(|e| invalid(format!("writing the package: {e}")))?;
    Ok(Wrote {
        dir: dir.display().to_string(),
        map_id: manifest.map_id().to_string(),
        file,
    })
}

/// What `project.read` gives: a project and what it resolves to.
#[derive(Clone, Debug, Serialize)]
pub struct Described {
    pub project: Project,
    pub file: String,
    pub origin: crate::edits::Origin,
    /// Top first; empty when the stack does not open.
    pub layers: Vec<LayerInfo>,
    pub edit_files: Vec<String>,
    pub edited: Vec<String>,
    /// Each kit the project names with its fingerprint, or why this build
    /// does not have it.
    pub kits: Vec<KitListed>,
    /// `fresh`: `layers` when a layer holds the scenario, `kit` when a kit
    /// must make it; `None` for `onto` or when the stack does not open.
    pub scenario_from: Option<&'static str>,
    /// `fresh` with the scenario in the layers, asked for: the tags of the
    /// closure the tree does not hold exactly, which refuse the build.
    pub lossy: Option<Vec<String>>,
    /// What stops a build before it runs: a missing tree, layer or edit
    /// file, an edit that does not apply, a kit this build lacks, a tag the
    /// closure misses.
    pub problems: Vec<String>,
}

/// A project's kit as [`Described`] lists it.
#[derive(Clone, Debug, Serialize)]
pub struct KitListed {
    pub name: String,
    pub fingerprint: Option<String>,
    pub missing: bool,
}

/// `file`'s project resolved without building it; with `closure`, a fresh
/// project's closure is walked for [`Described::lossy`].
pub fn describe(roots: &Roots, file: &ProjectFile, tools: &Tools<'_>, closure: bool) -> Described {
    let project = &file.project;
    let mut problems = Vec::new();
    let kits = project
        .kits
        .iter()
        .map(|n| {
            let k = tools.kits.iter().find(|k| k.name() == n);
            if k.is_none() {
                problems.push(format!("kit {n:?} is not one this build has"));
            }
            KitListed {
                name: n.clone(),
                fingerprint: k.map(|k| k.fingerprint()),
                missing: k.is_none(),
            }
        })
        .collect();
    let mut out = Described {
        project: project.clone(),
        file: file.path.display().to_string(),
        origin: file.origin,
        layers: Vec::new(),
        edit_files: Vec::new(),
        edited: Vec::new(),
        kits,
        scenario_from: None,
        lossy: None,
        problems,
    };
    if let Err(e) = roots.refuse_checkout() {
        out.problems.push(e.to_string());
        return out;
    }
    let (mut stack, layers) = match open_stack(roots, project) {
        Ok(s) => s,
        Err(e) => {
            out.problems.push(e.to_string());
            return out;
        }
    };
    out.layers = layers;
    out.edit_files = stack.files().iter().map(|f| f.name.clone()).collect();
    out.edited = stack.edited().iter().map(key_text).collect();
    out.problems
        .extend(stack.check().iter().map(ToString::to_string));
    if project.mode == BuildMode::Fresh {
        let scenario = project.scenario_key().expect("a fresh project's");
        match stack.read(&scenario) {
            Ok(Some(_)) => {
                out.scenario_from = Some("layers");
                if closure {
                    match fresh_closure(roots, project, &mut stack, tools.keep) {
                        Ok(Some(c)) => out.lossy = Some(lossy(&c.tags)),
                        Ok(None) => {}
                        Err(e) => out.problems.push(e.to_string()),
                    }
                }
            }
            Ok(None) => {
                out.scenario_from = Some("kit");
                if project.kits.is_empty() {
                    out.problems.push(format!(
                        "no layer holds the scenario {} and the project names no kit to make it",
                        key_text(&scenario)
                    ));
                }
            }
            Err(e) => out.problems.push(e.to_string()),
        }
    }
    out
}
