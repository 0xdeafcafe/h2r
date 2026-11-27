//! Projects over the tag tree (docs/editor/tag-workspace.md, "Projects"
//! and "Layers").
//!
//! A project file names one map to build:
//!
//! | Field | Means |
//! |---|---|
//! | `name` | the map and its package directory, `<data>/maps/<name>/` |
//! | `mode` | `fresh` (closure over the layers, nothing pinned) or `onto` (the disc map with the changed tags rebuilt) |
//! | `base` | the disc map: its variants, globals and header |
//! | `scenario` | `fresh`: the scenario's tag path; `onto` takes the base's |
//! | `kits` | build steps, in order ([`crate::project_build::Kit`]) |
//! | `edits` | edit files: `assets/tag-edits/<name>.yaml` in Git, or a local file's `<name>` |
//! | `imports` | import layers, `<data>/workspace/imports/<name>/`, top first |
//! | `output` | `package` |
//!
//! Files: `art/<dir>/project.toml` in the checkout, and
//! `<data>/workspace/projects/<name>.toml`. A name two files give is an
//! error naming both.
//!
//! [`open_stack`] gives the tags a build reads: the edit files ([`Edited`])
//! over [`Layered`] import layers over the tree, read with the base map's
//! variants. The edit files are the project's `edits` in order, then the
//! project's own local file `<data>/workspace/edits/<name>.yaml` when
//! there is one (what a save writes). A tree, a base map, an import layer
//! or an edit file that is not there is a named error.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tags::repo::{TagRepo, TagSource};

use crate::edits::{self, EditFile, Edited, Origin, Problem};
use crate::layer::Layered;

/// Where a project in Git lives: `art/<dir>/project.toml`.
pub const ART_DIR: &str = "art";
pub const GIT_FILE: &str = "project.toml";
/// Under `<data>/workspace/`: the user's projects, import layers and
/// build caches.
pub const PROJECTS_DIR: &str = "projects";
pub const IMPORTS_DIR: &str = "imports";
pub const BUILD_DIR: &str = "build";

/// How a project builds its map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildMode {
    Fresh,
    Onto,
}

/// What a build writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    /// A map package under `<data>/maps/<name>/`.
    Package,
}

/// A project file (module docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    pub mode: BuildMode,
    pub base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    pub output: Output,
}

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

fn is_tag_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 256
        && !s.contains('/')
        && s.rsplit('\\').next().is_some_and(|n| !n.is_empty())
}

impl Project {
    /// A project file's text, checked (module docs). `what` names the file
    /// in errors.
    pub fn parse(text: &str, what: &str) -> Result<Self, String> {
        let p: Project = toml::from_str(text).map_err(|e| format!("{what}: {e}"))?;
        p.validate().map_err(|e| format!("{what}: {e}"))?;
        Ok(p)
    }

    pub fn validate(&self) -> Result<(), String> {
        use content::manifest::is_map_name;
        if !is_map_name(&self.name) || !content::store::is_dir_name(&self.name) {
            return Err(format!("name {:?} is not a map name", self.name));
        }
        if !is_map_name(&self.base) {
            return Err(format!("base {:?} is not a map name", self.base));
        }
        if self.name.eq_ignore_ascii_case(&self.base) {
            return Err(format!(
                "name {:?} is the base's: a project makes a map of its own",
                self.name
            ));
        }
        match (self.mode, &self.scenario) {
            (BuildMode::Fresh, None) => return Err("a fresh project names its scenario".into()),
            (BuildMode::Fresh, Some(s)) if !is_tag_path(s) => {
                return Err(format!("scenario {s:?} is not a tag path"));
            }
            (BuildMode::Onto, Some(_)) => {
                return Err("an onto project builds its base's scenario: drop `scenario`".into());
            }
            _ => {}
        }
        let mut seen = std::collections::HashSet::new();
        for k in &self.kits {
            if k.is_empty() || !seen.insert(("kit", k.as_str())) {
                return Err(format!("kits: {k:?} is empty or named twice"));
            }
        }
        for e in &self.edits {
            EditRef::parse(e)?;
            if !seen.insert(("edits", e.as_str())) {
                return Err(format!("edits: {e:?} is named twice"));
            }
        }
        for i in &self.imports {
            if !edits::is_plain_name(i) || !seen.insert(("import", i.as_str())) {
                return Err(format!(
                    "imports: {i:?} is not an import layer's name, or is named twice"
                ));
            }
        }
        Ok(())
    }

    /// The project file's text.
    pub fn to_text(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    /// `fresh`'s scenario key; `None` for `onto`.
    pub fn scenario_key(&self) -> Option<tags::repo::TagKey> {
        self.scenario
            .as_ref()
            .map(|s| tags::repo::TagKey::new(map::FourCC(*b"scnr"), s.clone()))
    }
}

/// Where a build reads and writes.
#[derive(Clone, Debug)]
pub struct Roots {
    /// The tag tree (Studio's `tags_root`, `<data>/tags/` unless set).
    pub tree: PathBuf,
    /// The data root: `workspace/` and `maps/` are under it.
    pub data: PathBuf,
    /// The Git checkout our sources are read from (`art/`,
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

/// A project file found on disk.
#[derive(Clone, Debug)]
pub struct ProjectFile {
    pub project: Project,
    pub origin: Origin,
    pub path: PathBuf,
}

/// A project file as [`discover`] finds it: read, or why not.
#[derive(Clone, Debug)]
pub struct Found {
    pub origin: Origin,
    pub path: PathBuf,
    pub project: Result<Project, String>,
}

fn read_found(origin: Origin, path: PathBuf) -> Found {
    let project = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}", path.display()))
        .and_then(|t| Project::parse(&t, &path.display().to_string()));
    Found {
        origin,
        path,
        project,
    }
}

/// Every project file: `art/*/project.toml` in the checkout, then
/// `<data>/workspace/projects/*.toml`, each sorted by path.
pub fn discover(roots: &Roots) -> Vec<Found> {
    let mut out = Vec::new();
    if let Some(checkout) = &roots.checkout {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(checkout.join(ART_DIR))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join(GIT_FILE))
            .filter(|p| p.is_file())
            .collect();
        dirs.sort();
        out.extend(dirs.into_iter().map(|p| read_found(Origin::Git, p)));
    }
    let mut local: Vec<PathBuf> = std::fs::read_dir(roots.workspace().join(PROJECTS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "toml"))
        .collect();
    local.sort();
    out.extend(local.into_iter().map(|p| read_found(Origin::Local, p)));
    out
}

/// The project `which` names: a path to a project file (it ends in
/// `.toml`), or a project's name among [`discover`]'s. A name no file
/// gives, a name two files give and a file that does not read are errors;
/// a file that does not read and would have given the name (its
/// directory's or its own stem) is that file's error.
pub fn load(roots: &Roots, which: &str) -> Result<ProjectFile, ProjectError> {
    if which.ends_with(".toml") {
        let path = PathBuf::from(which);
        let in_checkout = roots
            .checkout
            .as_ref()
            .is_some_and(|c| git_checkout(&path).is_some_and(|g| same_dir(&g, c)));
        let origin = if in_checkout {
            Origin::Git
        } else {
            Origin::Local
        };
        let f = read_found(origin, path);
        return Ok(ProjectFile {
            project: f.project.map_err(ProjectError::Invalid)?,
            origin: f.origin,
            path: f.path,
        });
    }
    let found = discover(roots);
    let mut hits: Vec<ProjectFile> = Vec::new();
    for f in &found {
        match &f.project {
            Ok(p) if p.name == which => hits.push(ProjectFile {
                project: p.clone(),
                origin: f.origin,
                path: f.path.clone(),
            }),
            Ok(_) => {}
            Err(why) if names(&f.path, which) => return Err(invalid(why.clone())),
            Err(_) => {}
        }
    }
    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => {
            let all: Vec<String> = found
                .iter()
                .filter_map(|f| f.project.as_ref().ok().map(|p| p.name.clone()))
                .collect();
            Err(invalid(format!(
                "no project is named {which:?} (the projects are: {})",
                if all.is_empty() {
                    "none".to_owned()
                } else {
                    all.join(", ")
                }
            )))
        }
        _ => Err(invalid(format!(
            "two project files are named {which:?}: {}",
            hits.iter()
                .map(|h| h.path.display().to_string())
                .collect::<Vec<_>>()
                .join(" and ")
        ))),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Whether a project file's place names `name`: `art/<name>/project.toml`
/// or `projects/<name>.toml`.
fn names(path: &Path, name: &str) -> bool {
    let stem = path.file_stem().and_then(|s| s.to_str());
    let dir = path
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|s| s.to_str());
    if stem == Some("project") {
        dir == Some(name)
    } else {
        stem == Some(name)
    }
}

/// A layer of a project's stack, top first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LayerInfo {
    /// `edits`, `import` or `tree`.
    pub kind: &'static str,
    pub name: String,
    pub path: String,
}

/// The tags a build reads.
pub type Stack = Edited<Layered>;

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

/// The project's stack (module docs) and its layers, top first.
pub fn open_stack(
    roots: &Roots,
    project: &Project,
) -> Result<(Stack, Vec<LayerInfo>), ProjectError> {
    open_tree(roots, &project.base)?;
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
        infos.push(LayerInfo {
            kind: "import",
            name: name.clone(),
            path: dir.display().to_string(),
        });
    }
    layered.push(Box::new(TagRepo::for_map(&roots.tree, &project.base)?));
    infos.push(LayerInfo {
        kind: "tree",
        name: project.base.clone(),
        path: roots.tree.display().to_string(),
    });
    let stack = Edited::new(layered, files).map_err(ProjectError::Edits)?;
    Ok((stack, infos))
}

/// Whether the stack holds `key` (for a fresh project's scenario).
pub fn holds(stack: &mut Stack, key: &tags::repo::TagKey) -> Result<bool, ProjectError> {
    Ok(stack.read(key)?.is_some())
}

/// A project file as `project.list` gives it.
#[derive(Clone, Debug, Serialize)]
pub struct Listed {
    pub name: Option<String>,
    pub path: String,
    pub origin: Origin,
    pub mode: Option<BuildMode>,
    pub base: Option<String>,
    /// Why the file does not read.
    pub error: Option<String>,
}

/// Every project file ([`discover`]), each with what it names or why it
/// does not read.
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

/// Apply every edit of `project` over its layers. An edit file that does
/// not parse is listed in `problems`; a missing tree, base, layer or edit
/// file is the error.
pub fn check_edits(roots: &Roots, project: &Project) -> Result<Checked, ProjectError> {
    let (mut stack, _) = match open_stack(roots, project) {
        Ok(s) => s,
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
    project: &Project,
    tag: &tags::repo::TagKey,
) -> Result<content::Sha256, ProjectError> {
    let (mut stack, _) = open_stack(roots, project)?;
    let t = stack.below_mut().read(tag)?.ok_or_else(|| {
        invalid(format!(
            "no layer of project {} holds {}",
            project.name,
            edits::key_text(tag)
        ))
    })?;
    Ok(edits::loose_hash(&t))
}
