//! The game's set (docs/editor/studio-redesign.md, "How the game consumes
//! packages"): the baked packages, then the user's under `<data>/packages/`,
//! ordered and switched by `<data>/packages/set.toml`, with the packages
//! whose game feature is off left out whole.
//!
//! [`GameSet::assemble`] is pure; [`GameSet::read`] reads the folder and
//! the set file first. A package that cannot be read, or that takes a baked
//! package's id, is a line in [`GameSet::problems`] and stays out; so does
//! the whole set when it cannot be ordered.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use content::package::{ALWAYS_ON, ItemScope, MANIFEST_FILE, SET_FILE, Set, SetError, SetFile};
use serde::Serialize;

use crate::build::Loaded;
use crate::load::LoadError;

/// The folder under the data root the user's packages live in.
pub const PACKAGES_DIR: &str = "packages";
/// The folder under [`PACKAGES_DIR`] built maps are kept in; not a package.
pub const CACHE_DIR: &str = "cache";

/// Where a package came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Ours, in the binary: trusted as part of it.
    Baked,
    /// A folder under `<data>/packages/`. Unsigned: it loads in local
    /// games only.
    Local(PathBuf),
}

impl Origin {
    pub fn word(&self) -> &'static str {
        match self {
            Self::Baked => "baked",
            Self::Local(_) => "local",
        }
    }
}

/// A loaded package and where it came from.
#[derive(Debug, Clone)]
pub struct Entry {
    pub loaded: Loaded,
    pub origin: Origin,
}

/// Why a package is not loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Off {
    /// `set.toml` lists it in `off`.
    SetFile,
    /// It names a game feature that is switched off.
    Feature(String),
    /// A package it requires is missing or off ([`content::package::Problem`]).
    Requires(String),
    /// The set could not be ordered.
    Set(String),
}

impl std::fmt::Display for Off {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SetFile => write!(f, "switched off in {SET_FILE}"),
            Self::Feature(name) => write!(f, "feature {name:?} is off"),
            Self::Requires(why) | Self::Set(why) => f.write_str(why),
        }
    }
}

/// One row of `packages.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listed {
    /// The slug (`pogo`).
    pub id: String,
    pub name: String,
    /// The `PackageId`, hex.
    pub package_id: String,
    /// `baked` or `local`.
    pub origin: &'static str,
    /// The local package's folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    pub on: bool,
    /// Why it is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// `core` and `ui`: no switch turns them off.
    pub always_on: bool,
    /// Baked packages are; a local one is not, and loads in local games
    /// only.
    pub signed: bool,
    pub features: Vec<String>,
    /// Items by kind.
    pub items: BTreeMap<String, usize>,
    /// The scopes its items have: `sim`, `client`, `live`, `per-feature`.
    pub scopes: Vec<&'static str>,
}

/// The game's packages: those on in load order, and a row for every one.
#[derive(Debug, Clone, Default)]
pub struct GameSet {
    /// On, in load order.
    pub on: Vec<Entry>,
    /// Every package read: on ones in load order, then the off ones by id.
    pub listed: Vec<Listed>,
    /// Lines for the boot log and `packages.list`: unreadable packages,
    /// refused ids, a set file that is not used, a set that cannot be
    /// ordered.
    pub problems: Vec<String>,
    /// The content set's hash ([`Set::content_hash`]), when the set
    /// ordered.
    pub content_hash: Option<content::Sha256>,
}

fn scope_word(s: ItemScope) -> &'static str {
    match s {
        ItemScope::Sim => "sim",
        ItemScope::Client => "client",
        ItemScope::Live => "live",
        ItemScope::PerFeature => "per-feature",
    }
}

fn row(e: &Entry, why: Option<&Off>) -> Listed {
    let m = &e.loaded.package.manifest;
    let mut items: BTreeMap<String, usize> = BTreeMap::new();
    let mut scopes = Vec::new();
    for i in &m.items {
        let kind = serde_json::to_value(i.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        *items.entry(kind).or_default() += 1;
        let s = scope_word(i.kind.scope());
        if !scopes.contains(&s) {
            scopes.push(s);
        }
    }
    scopes.sort();
    Listed {
        id: m.package.id.clone(),
        name: m.package.name.clone(),
        package_id: e.loaded.package.id.to_string(),
        origin: e.origin.word(),
        dir: match &e.origin {
            Origin::Local(d) => Some(d.display().to_string()),
            Origin::Baked => None,
        },
        on: why.is_none(),
        why: why.map(ToString::to_string),
        always_on: ALWAYS_ON.contains(&m.package.id.as_str()),
        signed: e.origin == Origin::Baked,
        features: m.package.features.clone(),
        items,
        scopes,
    }
}

impl GameSet {
    /// The set of `baked` and `local` packages under the user's `file`
    /// (`Err`: the set file did not read, so none of it is used), with
    /// `feature_on` saying which game features are on. An always-on package
    /// is never left out for a feature.
    pub fn assemble(
        baked: Vec<Loaded>,
        local: Vec<Result<(PathBuf, Loaded), LoadError>>,
        file: Result<SetFile, SetError>,
        feature_on: &dyn Fn(&str) -> bool,
    ) -> Self {
        let mut problems = Vec::new();
        let file = file.unwrap_or_else(|e| {
            problems.push(format!("{e}; every package is on, in the default order"));
            SetFile::default()
        });
        let mut entries: Vec<Entry> = baked
            .into_iter()
            .map(|loaded| Entry {
                loaded,
                origin: Origin::Baked,
            })
            .collect();
        for l in local {
            match l {
                Ok((dir, loaded)) => {
                    let id = loaded.package.name();
                    if let Some(e) = entries.iter().find(|e| e.loaded.package.name() == id) {
                        problems.push(format!(
                            "{}: package {id:?} is already {} ({}), so this one is not loaded",
                            dir.display(),
                            match &e.origin {
                                Origin::Baked => "built in".to_owned(),
                                Origin::Local(d) => format!("in {}", d.display()),
                            },
                            e.loaded.package.id,
                        ));
                        continue;
                    }
                    entries.push(Entry {
                        loaded,
                        origin: Origin::Local(dir),
                    });
                }
                Err(e) => problems.push(e.to_string()),
            }
        }

        // Why each package is off before the set's own reasons.
        let mut off: BTreeMap<String, Off> = BTreeMap::new();
        for e in &entries {
            let h = &e.loaded.package.manifest.package;
            if ALWAYS_ON.contains(&h.id.as_str()) {
                continue;
            }
            if file.off.contains(&h.id) {
                off.insert(h.id.clone(), Off::SetFile);
            } else if let Some(f) = h.features.iter().find(|f| !feature_on(f)) {
                off.insert(h.id.clone(), Off::Feature(f.clone()));
            }
        }
        let mut run = file.clone();
        for (id, why) in &off {
            if *why != Off::SetFile {
                run.off.push(id.clone());
            }
        }
        let packages = entries.iter().map(|e| e.loaded.package.clone()).collect();
        let mut content_hash = None;
        let order: Vec<String> = match Set::new(packages, &run) {
            Ok(set) => {
                content_hash = Some(set.content_hash());
                for p in &set.problems {
                    off.entry(p.package().to_owned())
                        .or_insert_with(|| Off::Requires(p.to_string()));
                }
                set.loaded.iter().map(|p| p.name().to_owned()).collect()
            }
            Err(e) => {
                problems.push(format!("packages: {e}; no package is loaded"));
                for en in &entries {
                    off.entry(en.loaded.package.name().to_owned())
                        .or_insert_with(|| Off::Set(e.to_string()));
                }
                Vec::new()
            }
        };

        let mut by_id: BTreeMap<String, Entry> = entries
            .into_iter()
            .map(|e| (e.loaded.package.name().to_owned(), e))
            .collect();
        let mut on = Vec::new();
        let mut listed = Vec::new();
        for id in &order {
            let e = by_id.remove(id).expect("the set holds what was read");
            listed.push(row(&e, None));
            on.push(e);
        }
        for e in by_id.values() {
            let why = off
                .get(e.loaded.package.name())
                .cloned()
                .unwrap_or_else(|| Off::Set("not ordered".into()));
            listed.push(row(e, Some(&why)));
        }
        Self {
            on,
            listed,
            problems,
            content_hash,
        }
    }

    /// [`assemble`](Self::assemble) over `baked` and the folders under
    /// `<data_root>/packages/` that hold a `package.toml`, by name, and
    /// that folder's `set.toml` when there is one.
    pub fn read(baked: Vec<Loaded>, data_root: &Path, feature_on: &dyn Fn(&str) -> bool) -> Self {
        let root = data_root.join(PACKAGES_DIR);
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join(MANIFEST_FILE).is_file())
            .collect();
        dirs.sort();
        let local = dirs
            .into_iter()
            .map(|d| Loaded::from_dir(&d).map(|l| (d, l)))
            .collect();
        let file = match std::fs::read_to_string(root.join(SET_FILE)) {
            Ok(text) => SetFile::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SetFile::default()),
            Err(e) => Err(SetError::Toml(e.to_string())),
        };
        Self::assemble(baked, local, file, feature_on)
    }

    /// The packages on, in load order, for [`crate::build`].
    pub fn loaded(&self) -> Vec<Loaded> {
        self.on.iter().map(|e| e.loaded.clone()).collect()
    }

    /// The `packages.list` result: every package in order with on or off
    /// and why, the problems, the content hash, the refusal a networked
    /// session would meet, and what `served` says each map read got.
    pub fn list(&self, served: &crate::rewrite::Served) -> serde_json::Value {
        let maps = served.lock().unwrap_or_else(|e| e.into_inner()).clone();
        serde_json::json!({
            "packages": self.listed,
            "problems": self.problems,
            "content_hash": self.content_hash.map(|h| h.to_string()),
            "local_only": self.network_refusal(),
            "maps": maps,
        })
    }

    /// Why a Live or System Link session cannot start with this set: the
    /// first unsigned package on that has a `sim` item, named. `None` when
    /// every peer-visible package is signed or baked.
    pub fn network_refusal(&self) -> Option<String> {
        self.on.iter().find_map(|e| match &e.origin {
            Origin::Local(dir) if e.loaded.package.manifest.has_sim_item() => Some(format!(
                "package {:?} ({}) is not signed and changes the game: it loads in local games only",
                e.loaded.package.name(),
                dir.display()
            )),
            _ => None,
        })
    }
}

/// Every baked package from `files`: `(folder, [(path in the package,
/// bytes)])`, the table `boot`'s build script writes from `content/<id>/`.
/// One that does not read is a line in the second list.
pub fn baked(files: &[(&str, &[(&str, &[u8])])]) -> (Vec<Loaded>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for (folder, list) in files {
        let map: BTreeMap<String, Vec<u8>> = list
            .iter()
            .map(|(n, b)| ((*n).to_owned(), b.to_vec()))
            .collect();
        match Loaded::from_files(&format!("content/{folder}"), &map) {
            Ok(l) => out.push(l),
            Err(e) => problems.push(e.to_string()),
        }
    }
    (out, problems)
}
