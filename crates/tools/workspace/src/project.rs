//! Projects over the tag tree (docs/editor/tag-workspace.md, "Projects"
//! and "Layers").
//!
//! A project is a package's `map` item: its `map.toml` ([`MapSpec`]) names
//! the map, one file per map. The fields the tree build reads besides the
//! map item's own:
//!
//! | Field | Means |
//! |---|---|
//! | `edits` | edit files: `assets/tag-edits/<name>.yaml` in Git, or a local file's `<name>` |
//! | `imports` | import layers, `<data>/workspace/imports/<name>/`, top first |
//!
//! The packages are the checkout's `content/*/` (Git) and
//! `<data>/packages/*/` (local, not `cache/`). A package that does not load
//! stops every tree build, named: its items may be ones a map takes. A map
//! name two items give is an error naming both.
//!
//! [`open_stack`] gives the tags a build reads, top first: the edit files
//! ([`Edited`]), the authored layer (the `tag` items the map takes,
//! [`crate::authored`]), the import layers, the tree, read with the base
//! map's variants. The edit files are the project's `edits` in order, then
//! its own local file `<data>/workspace/edits/<name>.yaml` when there is
//! one (what a save writes). A tree, a base map, an import layer or an edit
//! file that is not there is a named error.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use content::package::{Kind, MANIFEST_FILE};
use serde::Serialize;
use tags::repo::{TagKey, TagRepo, TagSource};

use crate::authored;
use crate::build::Loaded;
use crate::edits::{self, EditFile, Edited, Origin, Problem};
use crate::game::{CACHE_DIR, PACKAGES_DIR};
use crate::layer::Layered;
use crate::maps::{MapSpec, Mode};

/// Where the packages in Git live: `content/<dir>/package.toml`.
pub const CONTENT_DIR: &str = "content";
/// Under `<data>/workspace/`: the user's import layers.
pub const IMPORTS_DIR: &str = "imports";

/// A project: a `map` item's spec.
pub type Project = MapSpec;

/// What can stop a project.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("{0}")]
    Invalid(String),
    #[error(
        "no tag tree at {}: extract the disc first (workspace.extract, h2 tags extract)",
        .0.display()
    )]
    NoTree(PathBuf),
    #[error("{}", edits::problems_text(.0))]
    Edits(Vec<Problem>),
    #[error(transparent)]
    Repo(#[from] tags::repo::Error),
    #[error(transparent)]
    Map(#[from] map::Error),
}

fn invalid(why: impl Into<String>) -> ProjectError {
    ProjectError::Invalid(why.into())
}

/// An entry of a project's `edits`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditRef {
    /// `assets/tag-edits/<name>.yaml`, from the checkout's root.
    Git(String),
    /// `<data>/workspace/edits/<name>.yaml`, by name.
    Local(String),
}

impl EditRef {
    pub fn parse(s: &str) -> Result<Self, String> {
        if s.contains('/') {
            edits::git_path(Path::new(""), s).map_err(|e| format!("edits entry {e}"))?;
            Ok(Self::Git(s.to_owned()))
        } else if edits::is_plain_name(s) {
            Ok(Self::Local(s.to_owned()))
        } else {
            Err(format!(
                "edits entry {s:?}: {}/<name>.{} in Git, or a local edit file's name",
                edits::GIT_DIR,
                edits::EXT
            ))
        }
    }
}

/// `fresh`'s scenario key; `None` for `onto`.
pub fn scenario_key(project: &Project) -> Option<TagKey> {
    project
        .scenario
        .as_ref()
        .map(|s| TagKey::new(map::FourCC(*b"scnr"), s.clone()))
}

/// Where a build reads and writes.
#[derive(Clone, Debug)]
pub struct Roots {
    /// The tag tree (Studio's `tags_root`, `<data>/tags/` unless set).
    pub tree: PathBuf,
    /// The data root: `workspace/`, `packages/` and `maps/` are under it.
    pub data: PathBuf,
    /// The Git checkout our sources are read from (`content/`,
    /// `assets/tag-edits/`); none for a player's machine.
    pub checkout: Option<PathBuf>,
}

/// The Git checkout `path` is in, if any.
pub fn git_checkout(path: &Path) -> Option<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    abs.ancestors()
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}

impl Roots {
    pub fn workspace(&self) -> PathBuf {
        self.data.join("workspace")
    }

    pub fn maps(&self) -> PathBuf {
        content::store::maps_dir(&self.data)
    }

    /// The tree and the data root are game data: neither is in a Git
    /// checkout.
    pub fn refuse_checkout(&self) -> Result<(), ProjectError> {
        for (what, dir) in [("tag tree", &self.tree), ("data root", &self.data)] {
            if let Some(repo) = git_checkout(dir) {
                return Err(invalid(format!(
                    "the {what} {} is inside the Git checkout {}: it is game data and never sits in a repository",
                    dir.display(),
                    repo.display()
                )));
            }
        }
        Ok(())
    }
}

/// A package directory [`packages`] finds: loaded, or why not.
#[derive(Clone, Debug)]
pub struct PackageDir {
    pub origin: Origin,
    pub path: PathBuf,
    pub loaded: Result<Loaded, String>,
}

fn package_dirs(root: &Path, skip: Option<&str>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(MANIFEST_FILE).is_file())
        .filter(|p| skip.is_none_or(|s| p.file_name().and_then(|n| n.to_str()) != Some(s)))
        .collect();
    dirs.sort();
    dirs
}

fn read_package(origin: Origin, path: PathBuf) -> PackageDir {
    let loaded = Loaded::from_dir(&path).map_err(|e| e.to_string());
    PackageDir {
        origin,
        path,
        loaded,
    }
}

/// Every package: the checkout's `content/*/`, then `<data>/packages/*/`
/// (not `cache/`), each sorted by path.
pub fn packages(roots: &Roots) -> Vec<PackageDir> {
    let mut out = Vec::new();
    if let Some(checkout) = &roots.checkout {
        for d in package_dirs(&checkout.join(CONTENT_DIR), None) {
            out.push(read_package(Origin::Git, d));
        }
    }
    for d in package_dirs(&roots.data.join(PACKAGES_DIR), Some(CACHE_DIR)) {
        out.push(read_package(Origin::Local, d));
    }
    out
}

/// The packages a tree build reads: every one [`packages`] finds, in its
/// order. One that does not load is the error.
pub fn package_set(found: &[PackageDir]) -> Result<Vec<Loaded>, ProjectError> {
    found
        .iter()
        .map(|d| {
            d.loaded.clone().map_err(|why| {
                invalid(format!(
                    "the package at {} does not load, and a tree build reads every package for the items a map takes: {why}",
                    d.path.display()
                ))
            })
        })
        .collect()
}

/// A project as [`discover`] finds it: read, or why not.
#[derive(Clone, Debug)]
pub struct Found {
    pub origin: Origin,
    /// The package directory.
    pub path: PathBuf,
    /// The package's id and the `map` item's index; `None` when the
    /// package does not load.
    pub package: Option<String>,
    pub item: Option<usize>,
    pub project: Result<Project, String>,
}

fn map_items(d: &PackageDir) -> Vec<Found> {
    let l = match &d.loaded {
        Ok(l) => l,
        Err(why) => {
            return vec![Found {
                origin: d.origin,
                path: d.path.clone(),
                package: None,
                item: None,
                project: Err(why.clone()),
            }];
        }
    };
    let id = l.package.manifest.package.id.clone();
    let mut out = Vec::new();
    for (i, item) in l.package.manifest.items.iter().enumerate() {
        if item.kind != Kind::Map {
            continue;
        }
        let project = item
            .source
            .as_ref()
            .ok_or_else(|| "the map item has no source".to_owned())
            .and_then(|s| {
                l.files
                    .get(s)
                    .ok_or_else(|| format!("the package has no file {s:?}"))
                    .and_then(|b| MapSpec::parse(b).map_err(|e| format!("{s}: {e}")))
            })
            .map_err(|e| format!("{} item {i}: {e}", d.path.display()));
        out.push(Found {
            origin: d.origin,
            path: d.path.clone(),
            package: Some(id.clone()),
            item: Some(i),
            project,
        });
    }
    out
}

/// Every project: each `map` item of [`packages`], and each package that
/// does not load.
pub fn discover(roots: &Roots) -> Vec<Found> {
    packages(roots).iter().flat_map(map_items).collect()
}

/// A project to build: the map item and the packages around it.
#[derive(Clone, Debug)]
pub struct ProjectFile {
    pub project: Project,
    pub origin: Origin,
    /// The package directory.
    pub path: PathBuf,
    /// The package's id and the `map` item's index.
    pub package: String,
    pub item: usize,
    /// Every package, for the items the map takes.
    pub packages: Vec<Loaded>,
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn file_of(f: &Found, project: &Project, packages: Vec<Loaded>) -> ProjectFile {
    ProjectFile {
        project: project.clone(),
        origin: f.origin,
        path: f.path.clone(),
        package: f.package.clone().unwrap_or_default(),
        item: f.item.unwrap_or_default(),
        packages,
    }
}

/// The project `which` names: a package directory (or its
/// `package.toml`) with one `map` item, or a map's name among
/// [`discover`]'s. A name no item gives, a name two items give, a package
/// with no or several map items and a package that does not load are
/// errors; so is any package of the set that does not load
/// ([`package_set`]).
pub fn load(roots: &Roots, which: &str) -> Result<ProjectFile, ProjectError> {
    let mut found = packages(roots);
    let as_path = PathBuf::from(which);
    let dir = if as_path.file_name().is_some_and(|n| n == MANIFEST_FILE) {
        as_path.parent().map(Path::to_path_buf)
    } else if as_path.join(MANIFEST_FILE).is_file() {
        Some(as_path)
    } else {
        None
    };
    if let Some(dir) = dir {
        let at = match found.iter().position(|d| same_dir(&d.path, &dir)) {
            Some(i) => i,
            None => {
                let in_checkout = roots
                    .checkout
                    .as_ref()
                    .is_some_and(|c| git_checkout(&dir).is_some_and(|g| same_dir(&g, c)));
                let origin = if in_checkout {
                    Origin::Git
                } else {
                    Origin::Local
                };
                found.push(read_package(origin, dir.clone()));
                found.len() - 1
            }
        };
        let items = map_items(&found[at]);
        let f = match items.as_slice() {
            [one] => one.clone(),
            [] => {
                return Err(invalid(format!(
                    "the package at {} has no map item",
                    dir.display()
                )));
            }
            many => {
                let names: Vec<String> = many
                    .iter()
                    .map(|f| match &f.project {
                        Ok(p) => p.name.clone(),
                        Err(_) => format!("item {}", f.item.unwrap_or_default()),
                    })
                    .collect();
                return Err(invalid(format!(
                    "the package at {} has {} map items ({}): name the map",
                    dir.display(),
                    many.len(),
                    names.join(", ")
                )));
            }
        };
        let project = f.project.clone().map_err(invalid)?;
        let set = package_set(&found)?;
        return Ok(file_of(&f, &project, set));
    }
    let all: Vec<Found> = found.iter().flat_map(map_items).collect();
    let hits: Vec<(&Found, &Project)> = all
        .iter()
        .filter_map(|f| f.project.as_ref().ok().map(|p| (f, p)))
        .filter(|(_, p)| p.name.eq_ignore_ascii_case(which))
        .collect();
    match hits.as_slice() {
        [(f, p)] => {
            let set = package_set(&found)?;
            Ok(file_of(f, p, set))
        }
        [] => {
            if let Some(bad) = all.iter().find_map(|f| f.project.as_ref().err()) {
                return Err(invalid(format!(
                    "no project is named {which:?}, and a package does not read: {bad}"
                )));
            }
            let names: Vec<String> = all
                .iter()
                .filter_map(|f| f.project.as_ref().ok().map(|p| p.name.clone()))
                .collect();
            Err(invalid(format!(
                "no project is named {which:?} (the projects are: {})",
                if names.is_empty() {
                    "none".to_owned()
                } else {
                    names.join(", ")
                }
            )))
        }
        many => Err(invalid(format!(
            "two map items are named {which:?}: {}",
            many.iter()
                .map(|(f, _)| format!("{} item {}", f.path.display(), f.item.unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(" and ")
        ))),
    }
}

/// A layer of a project's stack, top first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LayerInfo {
    /// `edits`, `authored`, `import` or `tree`.
    pub kind: &'static str,
    /// The authored layer's is its fingerprint.
    pub name: String,
    /// The authored layer's is empty when it is not kept.
    pub path: String,
}

/// The tags a build reads.
pub type Stack = Edited<Layered>;

/// A project's stack, opened.
pub struct Opened {
    pub stack: Stack,
    /// Top first.
    pub layers: Vec<LayerInfo>,
    /// The authored layer's fingerprint and tags; `None` when the map
    /// takes no `tag` item.
    pub authored: Option<(String, Vec<TagKey>)>,
}

/// The project's edit files, in the order they apply (module docs).
pub fn edit_files(roots: &Roots, project: &Project) -> Result<Vec<EditFile>, ProjectError> {
    let workspace = roots.workspace();
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let mut own_listed = false;
    for e in &project.edits {
        let (origin, path) = match EditRef::parse(e).map_err(invalid)? {
            EditRef::Git(name) => {
                let checkout = roots.checkout.as_ref().ok_or_else(|| {
                    invalid(format!("edit file {name}: no checkout to read it from"))
                })?;
                (
                    Origin::Git,
                    edits::git_path(checkout, &name).map_err(invalid)?,
                )
            }
            EditRef::Local(name) => {
                own_listed |= name == project.name;
                (
                    Origin::Local,
                    edits::local_path(&workspace, &name).map_err(invalid)?,
                )
            }
        };
        if !path.is_file() {
            return Err(invalid(format!(
                "edit file {e} is not at {}",
                path.display()
            )));
        }
        match EditFile::read(e, origin, &path) {
            Ok(f) => out.push(f),
            Err(p) => problems.extend(p),
        }
    }
    if !own_listed {
        let own = edits::local_path(&workspace, &project.name).map_err(invalid)?;
        if own.is_file() {
            match EditFile::read(&project.name, Origin::Local, &own) {
                Ok(f) => out.push(f),
                Err(p) => problems.extend(p),
            }
        }
    }
    if !problems.is_empty() {
        return Err(ProjectError::Edits(problems));
    }
    Ok(out)
}

/// The tree at `roots.tree`, which must hold `base`.
pub fn open_tree(roots: &Roots, base: &str) -> Result<TagRepo, ProjectError> {
    if !roots.tree.join("repo.txt").is_file() {
        return Err(ProjectError::NoTree(roots.tree.clone()));
    }
    let top = TagRepo::open(&roots.tree)?;
    if !top.has_manifest(base) {
        return Err(invalid(format!(
            "the tree at {} has no map {base}: extract it (workspace.extract with maps [{base}])",
            roots.tree.display()
        )));
    }
    Ok(top)
}

/// The project's stack (module docs). With `keep`, the authored layer is
/// written to `<data>/workspace/generated/<fingerprint>/` (once) and read
/// from there; without, it is held in memory and nothing is written.
pub fn open_stack(roots: &Roots, file: &ProjectFile, keep: bool) -> Result<Opened, ProjectError> {
    let project = &file.project;
    let top = open_tree(roots, &project.base)?;
    let candidates: Vec<TagKey> = top
        .manifest(&project.base)?
        .tags
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let files = edit_files(roots, project)?;
    let mut infos: Vec<LayerInfo> = files
        .iter()
        .map(|f| LayerInfo {
            kind: "edits",
            name: f.name.clone(),
            path: f.path.display().to_string(),
        })
        .collect();
    let mut layered = Layered::new();
    let imports = roots.workspace().join(IMPORTS_DIR);
    let mut lower = Vec::new();
    for name in &project.imports {
        let dir = imports.join(name);
        if !dir.join("repo.txt").is_file() {
            return Err(invalid(format!(
                "import layer {name} is not at {}: workspace.import stores it",
                dir.display()
            )));
        }
        let repo = TagRepo::open(&dir)?;
        let repo = if repo.has_manifest(name) {
            TagRepo::for_map(&dir, name)?
        } else {
            repo
        };
        layered.push(Box::new(repo));
        lower.push(LayerInfo {
            kind: "import",
            name: name.clone(),
            path: dir.display().to_string(),
        });
    }
    layered.push(Box::new(TagRepo::for_map(&roots.tree, &project.base)?));
    lower.push(LayerInfo {
        kind: "tree",
        name: project.base.clone(),
        path: roots.tree.display().to_string(),
    });
    let made = authored::make(&file.packages, &project.served(), &candidates, &mut layered)
        .map_err(|why| invalid(format!("the authored layer of {}: {why}", project.name)))?;
    let authored = match made {
        None => None,
        Some(made) => {
            let path = if keep {
                let (repo, dir) = authored::keep(&roots.workspace(), &made)?;
                layered.put_top(Box::new(repo));
                dir.display().to_string()
            } else {
                layered.put_top(Box::new(authored::InMemory::of(&made)));
                String::new()
            };
            infos.push(LayerInfo {
                kind: "authored",
                name: made.fingerprint.clone(),
                path,
            });
            Some((
                made.fingerprint,
                made.tags.into_iter().map(|t| t.key).collect(),
            ))
        }
    };
    infos.extend(lower);
    let stack = Edited::new(layered, files).map_err(ProjectError::Edits)?;
    Ok(Opened {
        stack,
        layers: infos,
        authored,
    })
}

/// The authored layers some project's stack asks for, by fingerprint:
/// what `workspace.prune` keeps under `<data>/workspace/generated/`. A
/// project whose stack does not open is the error, so nothing is pruned
/// on a guess.
pub fn wanted_layers(roots: &Roots) -> Result<BTreeSet<String>, String> {
    let found = packages(roots);
    let set = package_set(&found).map_err(|e| e.to_string())?;
    let mut out = BTreeSet::new();
    for d in &found {
        for f in map_items(d) {
            let project = f.project.clone()?;
            let has_tags = !authored::tag_items(&set, &project.served()).is_empty();
            if !has_tags {
                continue;
            }
            let file = file_of(&f, &project, set.clone());
            let opened = open_stack(roots, &file, false)
                .map_err(|e| format!("project {}: {e}", project.name))?;
            if let Some((fp, _)) = opened.authored {
                out.insert(fp);
            }
        }
    }
    Ok(out)
}

/// A project as `project.list` gives it.
#[derive(Clone, Debug, Serialize)]
pub struct Listed {
    pub name: Option<String>,
    /// The package directory.
    pub path: String,
    pub origin: Origin,
    pub package: Option<String>,
    pub item: Option<usize>,
    pub mode: Option<Mode>,
    pub base: Option<String>,
    /// Why the package or its `map.toml` does not read.
    pub error: Option<String>,
}

/// Every project ([`discover`]), each with what it names or why it does
/// not read.
pub fn list(roots: &Roots) -> Vec<Listed> {
    discover(roots)
        .into_iter()
        .map(|f| {
            let (name, mode, base, error) = match f.project {
                Ok(p) => (Some(p.name), Some(p.mode), Some(p.base), None),
                Err(e) => (None, None, None, Some(e)),
            };
            Listed {
                name,
                path: f.path.display().to_string(),
                origin: f.origin,
                package: f.package,
                item: f.item,
                mode,
                base,
                error,
            }
        })
        .collect()
}

/// What `edits.check` gives: every edit of a project applied once over
/// its layers.
#[derive(Clone, Debug, Serialize)]
pub struct Checked {
    pub project: String,
    pub files: Vec<String>,
    /// The tags an edit names, `<group>:<path>`.
    pub edited: Vec<String>,
    pub problems: Vec<Problem>,
    pub overrides: Vec<edits::Override>,
}

/// Apply every edit of `file`'s project over its layers. An edit file that
/// does not parse is listed in `problems`; a missing tree, base, layer or
/// edit file is the error.
pub fn check_edits(roots: &Roots, file: &ProjectFile) -> Result<Checked, ProjectError> {
    let project = &file.project;
    let mut stack = match open_stack(roots, file, false) {
        Ok(o) => o.stack,
        Err(ProjectError::Edits(problems)) => {
            return Ok(Checked {
                project: project.name.clone(),
                files: Vec::new(),
                edited: Vec::new(),
                problems,
                overrides: Vec::new(),
            });
        }
        Err(e) => return Err(e),
    };
    let problems = stack.check();
    Ok(Checked {
        project: project.name.clone(),
        files: stack.files().iter().map(|f| f.name.clone()).collect(),
        edited: stack.edited().iter().map(edits::key_text).collect(),
        problems,
        overrides: stack.overrides().to_vec(),
    })
}

/// The hash an edit made now on `tag` records as its `was`: the tag as the
/// project's layers give it, below every edit file.
pub fn was_of(
    roots: &Roots,
    file: &ProjectFile,
    tag: &TagKey,
) -> Result<content::Sha256, ProjectError> {
    let mut stack = open_stack(roots, file, false)?.stack;
    let t = stack.below_mut().read(tag)?.ok_or_else(|| {
        invalid(format!(
            "no layer of project {} holds {}",
            file.project.name,
            edits::key_text(tag)
        ))
    })?;
    Ok(edits::loose_hash(&t))
}
