//! The set: the packages of one game, in load order.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    id::{IdError, Lock, PackageId},
    manifest::{Manifest, ManifestError},
};
use crate::Sha256;

/// A validated manifest with its lock and id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub manifest: Manifest,
    pub lock: Lock,
    pub id: PackageId,
}

/// Why a package cannot be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Id(#[from] IdError),
}

impl Package {
    /// Validate `manifest` and take its id over `lock`.
    pub fn new(manifest: Manifest, lock: Lock) -> Result<Self, PackageError> {
        manifest.validate()?;
        let id = PackageId::of(&manifest, &lock)?;
        Ok(Self { manifest, lock, id })
    }

    /// The slug the manifest names itself by.
    pub fn name(&self) -> &str {
        &self.manifest.package.id
    }
}

/// `<data>/packages/set.toml`: the user's order and what is switched off.
/// Every package is on unless it is listed in `off`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetFile {
    /// Package ids, first to load first. Breaks ties the dependencies leave.
    #[serde(default)]
    pub order: Vec<String>,
    /// Package ids that are switched off.
    #[serde(default)]
    pub off: Vec<String>,
}

impl SetFile {
    pub fn from_toml(text: &str) -> Result<Self, SetError> {
        toml::from_str(text).map_err(|e| SetError::Toml(e.to_string()))
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a set file serialises")
    }
}

/// A package left out of the set: it is never loaded half.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    #[error("package {package:?} needs {requires:?}, which is not installed")]
    Missing { package: String, requires: String },
    #[error("package {package:?} needs {requires:?}, which is switched off")]
    Off { package: String, requires: String },
    #[error("package {package:?} needs {requires:?}, which is itself disabled")]
    Disabled { package: String, requires: String },
}

impl Problem {
    /// The package that was left out.
    pub fn package(&self) -> &str {
        match self {
            Self::Missing { package, .. }
            | Self::Off { package, .. }
            | Self::Disabled { package, .. } => package,
        }
    }
}

/// Why a set cannot be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetError {
    #[error("set.toml: {0}")]
    Toml(String),
    #[error("two packages are named {0:?}")]
    DuplicateId(String),
    #[error("packages require or follow each other in a cycle: {}", .0.join(", "))]
    Cycle(Vec<String>),
}

/// The enabled packages in load order, and the lines for those left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Set {
    /// Load order: every `requires` and present `after` first, then the
    /// user's order, then the package id.
    pub loaded: Vec<Package>,
    /// Packages left out for a missing or disabled `requires`, by package
    /// then by requirement.
    pub problems: Vec<Problem>,
}

impl Set {
    /// The set of `packages` under the user's `file`. The result does not
    /// depend on the order `packages` arrive in.
    pub fn new(packages: Vec<Package>, file: &SetFile) -> Result<Self, SetError> {
        let mut all: BTreeMap<String, Package> = BTreeMap::new();
        for p in packages {
            let name = p.name().to_owned();
            if all.insert(name.clone(), p).is_some() {
                return Err(SetError::DuplicateId(name));
            }
        }
        let off: BTreeSet<&str> = file.off.iter().map(String::as_str).collect();

        let mut enabled: BTreeSet<String> = all
            .keys()
            .filter(|k| !off.contains(k.as_str()))
            .cloned()
            .collect();
        let mut problems = Vec::new();
        loop {
            let mut dropped = Vec::new();
            for name in &enabled {
                for req in &all[name].manifest.package.requires {
                    let package = name.clone();
                    let requires = req.clone();
                    let problem = if !all.contains_key(req) {
                        Problem::Missing { package, requires }
                    } else if off.contains(req.as_str()) {
                        Problem::Off { package, requires }
                    } else if !enabled.contains(req) {
                        Problem::Disabled { package, requires }
                    } else {
                        continue;
                    };
                    dropped.push(problem);
                }
            }
            if dropped.is_empty() {
                break;
            }
            for p in &dropped {
                enabled.remove(p.package());
            }
            problems.extend(dropped);
        }
        problems.sort_by(|a, b| {
            a.package()
                .cmp(b.package())
                .then_with(|| rank(a).cmp(&rank(b)))
        });

        // Edges run from the package that loads first.
        let mut before: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut waiting: BTreeMap<&str, usize> = BTreeMap::new();
        for name in &enabled {
            let h = &all[name].manifest.package;
            let mut deps = BTreeSet::new();
            deps.extend(h.requires.iter().map(String::as_str));
            deps.extend(
                h.after
                    .iter()
                    .map(String::as_str)
                    .filter(|a| enabled.contains(*a)),
            );
            waiting.insert(name, deps.len());
            for d in deps {
                before.entry(d).or_default().push(name);
            }
        }
        let user: BTreeMap<&str, usize> = file
            .order
            .iter()
            .enumerate()
            .rev()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let key = |n: &str| (user.get(n).copied().unwrap_or(usize::MAX), n.to_owned());

        let mut ready: BTreeSet<(usize, String)> = waiting
            .iter()
            .filter(|(_, w)| **w == 0)
            .map(|(n, _)| key(n))
            .collect();
        let mut order: Vec<String> = Vec::new();
        while let Some(next) = ready.pop_first() {
            let name = next.1;
            for after in before.get(name.as_str()).into_iter().flatten() {
                let w = waiting.get_mut(after).expect("every package waits");
                *w -= 1;
                if *w == 0 {
                    ready.insert(key(after));
                }
            }
            order.push(name);
        }
        if order.len() < enabled.len() {
            return Err(SetError::Cycle(cycle_members(&enabled, &order, &before)));
        }

        let loaded = order
            .into_iter()
            .map(|n| all.remove(&n).expect("ordered from the set"))
            .collect();
        Ok(Self { loaded, problems })
    }

    /// The ids of the loaded packages that have a `sim` item, in load order:
    /// what every peer of a networked game must share.
    pub fn content_set(&self) -> Vec<PackageId> {
        self.loaded
            .iter()
            .filter(|p| p.manifest.has_sim_item())
            .map(|p| p.id)
            .collect()
    }

    /// The SHA-256 of the content set as a canonical JSON array of ids.
    pub fn content_hash(&self) -> Sha256 {
        let ids = self
            .content_set()
            .iter()
            .map(|id| Value::String(id.to_string()))
            .collect();
        let json = crate::canonical::canonicalize(&Value::Array(ids)).expect("strings only");
        Sha256::of(json.as_bytes())
    }
}

fn rank(p: &Problem) -> &str {
    match p {
        Problem::Missing { requires, .. }
        | Problem::Off { requires, .. }
        | Problem::Disabled { requires, .. } => requires,
    }
}

/// The packages the sort could not place that lie on a cycle, sorted.
fn cycle_members(
    enabled: &BTreeSet<String>,
    placed: &[String],
    before: &BTreeMap<&str, Vec<&str>>,
) -> Vec<String> {
    let stuck: BTreeSet<&str> = enabled
        .iter()
        .map(String::as_str)
        .filter(|n| !placed.iter().any(|p| p.as_str() == *n))
        .collect();
    let reaches_itself = |start: &str| {
        let mut seen = BTreeSet::new();
        let mut todo = vec![start];
        while let Some(n) = todo.pop() {
            for next in before.get(n).into_iter().flatten() {
                if !stuck.contains(next) {
                    continue;
                }
                if *next == start {
                    return true;
                }
                if seen.insert(*next) {
                    todo.push(*next);
                }
            }
        }
        false
    };
    stuck
        .iter()
        .filter(|n| reaches_itself(n))
        .map(|n| (*n).to_owned())
        .collect()
}
