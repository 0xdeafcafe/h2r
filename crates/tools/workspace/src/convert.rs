//! `convert` items (docs/editor/studio-redesign.md, "Contents"; "How the
//! game consumes packages", Prepare; "Conversions"): a recipe that names a
//! converter and its settings. The input is the user's own disc, found at
//! prepare time; the outputs stay under the data root. The converters are
//! Rust above this crate (the Halo 1 import, the Map Pack install), handed
//! to [`convert`] as [`Converter`]s; this module decides what runs.
//!
//! - A converter splits its input into [`Unit`]s (a map, a level). Each
//!   unit's key covers the converter's version, the input's identity, the
//!   recipe's settings and the unit's own key ([`unit_key`]).
//! - The stamp, `<data>/packages/stamps/<package>.toml` ([`Stamp`]),
//!   records the key each unit was converted with, or why it failed. It is
//!   written after every unit, so an interrupted run resumes with the units
//!   it had not reached, and a unit whose key changed (a stale stamp) or
//!   whose output is gone is converted again on its own. A unit that failed
//!   with the same key is not tried again until the key changes or the run
//!   is forced.
//! - With no input (no disc) and nothing converted before, the package is
//!   off with the converter's reason: the stamp's `off` line, which
//!   [`crate::GameSet::read`] reads ([`inputs_off`]). Outputs converted
//!   before (by a run, or by the converter's own command:
//!   [`Converter::kept`]) keep the package on when the disc is taken away.
//!
//! A `convert` item changes no map: [`crate::builders`] checks its recipe
//! and its `apply` does nothing.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use content::Sha256;
use content::package::Kind;
use serde::{Deserialize, Serialize};

use crate::build::{Failure, Loaded};
use crate::builders::Job;
use crate::game::PACKAGES_DIR;

/// The folder under `<data>/packages/` the stamps are kept in; not a
/// package.
pub const STAMPS_DIR: &str = "stamps";

/// The converters a recipe may name: the closed list, as the builders are.
pub const CONVERTERS: [&str; 2] = ["halo1", "map-pack"];

/// A `convert` item's source, `<name>.convert.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvertSpec {
    /// One of [`CONVERTERS`].
    pub converter: String,
    /// The converter's own settings (which maps, which sandbox); it refuses
    /// a key it does not know.
    #[serde(default)]
    pub settings: toml::Table,
}

impl ConvertSpec {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "the recipe is not UTF-8".to_owned())?;
        let spec: Self = toml::from_str(text).map_err(|e| format!("the recipe: {e}"))?;
        if spec.converter.is_empty() {
            return Err("the recipe names no converter".into());
        }
        Ok(spec)
    }

    /// The settings as text, for the unit keys.
    fn settings_text(&self) -> String {
        toml::to_string(&self.settings).unwrap_or_default()
    }
}

/// `convert` builder: nothing changes in the map it runs on.
pub(crate) fn apply(_: &mut crate::Cx<'_>, _: &Job<'_>) -> Result<(), Failure> {
    Ok(())
}

pub(crate) fn check(job: &Job<'_>) -> Result<(), Failure> {
    let spec = ConvertSpec::parse(job.source)?;
    if !CONVERTERS.contains(&spec.converter.as_str()) {
        return Err(format!(
            "no converter named {:?} (this build has {})",
            spec.converter,
            CONVERTERS.join(", ")
        )
        .into());
    }
    Ok(())
}

/// What a converter reads: the user's disc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub path: PathBuf,
    /// What the lines call it.
    pub label: String,
    /// Changes when the input does (its size and modification time, or
    /// its hash): part of every unit's key.
    pub identity: String,
}

/// One piece of a conversion, done or not as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// Its name in the stamp (`a10`, `bloodgulch/h2`).
    pub name: String,
    /// What, beyond the input and the converter, its output depends on.
    pub key: String,
    /// The file or folder it writes: a unit whose output is gone is
    /// converted again.
    pub output: Option<PathBuf>,
}

/// A converter: the code a recipe names. It finds its input, lists the
/// units, and converts those it is given in any order and on any threads,
/// saying each as it finishes.
pub trait Converter {
    /// The recipe's `converter` (one of [`CONVERTERS`]).
    fn name(&self) -> &str;
    /// Changes whenever the same input would convert to other output.
    fn version(&self) -> String;
    /// The input for `spec`, or why there is none (no disc): the package is
    /// then off with that reason.
    fn input(&self, spec: &ConvertSpec) -> Result<Input, String>;
    /// Whether output this converter made before the stamps did (by its
    /// own command) is in place: with no input it keeps the package on.
    fn kept(&self, _spec: &ConvertSpec) -> bool {
        false
    }
    /// The units `input` gives under `spec`'s settings, in order. A setting
    /// it does not know is an error.
    fn units(&self, input: &Input, spec: &ConvertSpec) -> Result<Vec<Unit>, String>;
    /// Convert `todo`, calling `done` with each unit's name and outcome as
    /// it finishes. A unit it never calls `done` for is left for the next
    /// run.
    fn convert(
        &self,
        input: &Input,
        spec: &ConvertSpec,
        todo: &[Unit],
        done: &mut dyn FnMut(&str, Result<(), String>),
    );
}

/// A unit that failed with a key: not tried again for that key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitFailed {
    pub key: String,
    pub why: String,
}

/// One `convert` item's record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemStamp {
    /// The item's index in the manifest.
    pub index: usize,
    pub converter: String,
    /// The input last converted from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Why the package is off: no input, and nothing converted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<String>,
    /// Unit name to the key it was converted with.
    #[serde(default)]
    pub done: BTreeMap<String, String>,
    #[serde(default)]
    pub failed: BTreeMap<String, UnitFailed>,
}

/// `<data>/packages/stamps/<package>.toml`: what a package's `convert`
/// items made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    #[serde(default, rename = "item")]
    pub items: Vec<ItemStamp>,
}

const STAMP_HEAD: &str = "# What this package's convert items made (docs/editor/studio-redesign.md,\n\
     # \"Conversions\"). Delete it, or run `h2 package prepare --force`, to convert again.\n";

impl Stamp {
    /// The stamp at `path`; none when it is missing or does not read (then
    /// everything converts again).
    pub fn read(path: &Path) -> Option<Self> {
        toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    /// Write it whole, through a temporary file, so a stopped run leaves
    /// the last complete stamp.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        let dir = path.parent().ok_or("the stamp has no folder")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("stamp");
        let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
        std::fs::write(&tmp, format!("{STAMP_HEAD}{text}"))
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The slot of item `index`; a fresh one when it is new or was another
    /// converter's.
    fn slot(&mut self, index: usize, converter: &str) -> usize {
        match self.items.iter().position(|i| i.index == index) {
            Some(at) => {
                if self.items[at].converter != converter {
                    self.items[at] = ItemStamp {
                        index,
                        converter: converter.to_owned(),
                        ..ItemStamp::default()
                    };
                }
                at
            }
            None => {
                self.items.push(ItemStamp {
                    index,
                    converter: converter.to_owned(),
                    ..ItemStamp::default()
                });
                self.items.len() - 1
            }
        }
    }
}

/// Where package `package`'s stamp is kept.
pub fn stamp_path(data_root: &Path, package: &str) -> PathBuf {
    data_root
        .join(PACKAGES_DIR)
        .join(STAMPS_DIR)
        .join(format!("{package}.toml"))
}

/// The packages the stamps under `data_root` leave off for want of input,
/// with the reason.
pub fn inputs_off(data_root: &Path) -> BTreeMap<String, String> {
    let dir = data_root.join(PACKAGES_DIR).join(STAMPS_DIR);
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(stamp) = Stamp::read(&path) else {
            continue;
        };
        if let Some(why) = stamp.items.iter().find_map(|i| i.off.clone()) {
            out.insert(id.to_owned(), why);
        }
    }
    out
}

/// The key a unit is converted with: the converter, its version, the
/// input's identity, the recipe's settings and the unit's own key.
pub fn unit_key(
    converter: &str,
    version: &str,
    input: &Input,
    spec: &ConvertSpec,
    unit: &Unit,
) -> String {
    let text = format!(
        "{converter}\n{version}\n{}\n{}\n{}\n{}",
        input.identity,
        spec.settings_text(),
        unit.name,
        unit.key
    );
    Sha256::of(text.as_bytes()).to_string()
}

/// A unit converted in this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub package: String,
    pub item: usize,
    pub unit: String,
}

/// A package left off: its converter found no input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputOff {
    pub package: String,
    pub item: usize,
    pub why: String,
}

/// An item or a unit that did not convert in this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertFailed {
    pub package: String,
    pub item: usize,
    /// `None`: the whole item (its recipe, its input's units).
    pub unit: Option<String>,
    pub why: String,
}

/// An item whose converter this run does not have (switched off for the
/// run): its stamp is left as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub package: String,
    pub item: usize,
    pub converter: String,
}

/// What [`convert`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Converted {
    pub ran: Vec<Ran>,
    /// Units whose stamp held their key: not converted again.
    pub held: usize,
    /// Units that failed before with the same key: not tried again.
    pub not_retried: usize,
    pub off: Vec<InputOff>,
    pub failed: Vec<ConvertFailed>,
    pub skipped: Vec<Skipped>,
}

/// Run the `convert` items of `packages` with `converters`, under
/// `data_root`'s stamps. `force` forgets what the stamps of the items that
/// run say. Every unit is a line through `say`.
pub fn convert(
    data_root: &Path,
    packages: &[Loaded],
    converters: &[&dyn Converter],
    force: bool,
    say: &mut dyn FnMut(String),
) -> Converted {
    let mut out = Converted::default();
    for p in packages {
        let id = p.package.manifest.package.id.clone();
        let items: Vec<usize> = p
            .package
            .manifest
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.kind == Kind::Convert)
            .map(|(n, _)| n)
            .collect();
        if items.is_empty() {
            continue;
        }
        let path = stamp_path(data_root, &id);
        let mut stamp = Stamp::read(&path).unwrap_or_default();
        stamp.items.retain(|s| items.contains(&s.index));
        for index in items {
            convert_item(
                p, &id, index, converters, force, &path, &mut stamp, &mut out, say,
            );
        }
        if stamp.items.is_empty() && !path.exists() {
            continue;
        }
        if let Err(e) = stamp.write(&path) {
            say(format!("{id}: could not write the stamp: {e}"));
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn convert_item(
    p: &Loaded,
    id: &str,
    index: usize,
    converters: &[&dyn Converter],
    force: bool,
    path: &Path,
    stamp: &mut Stamp,
    out: &mut Converted,
    say: &mut dyn FnMut(String),
) {
    let item = &p.package.manifest.items[index];
    let fail = |out: &mut Converted, unit: Option<String>, why: String| {
        out.failed.push(ConvertFailed {
            package: id.to_owned(),
            item: index,
            unit,
            why,
        });
    };
    let spec = item
        .source
        .as_ref()
        .and_then(|s| p.files.get(s))
        .ok_or_else(|| "the item has no recipe".to_owned())
        .and_then(|b| ConvertSpec::parse(b));
    let spec = match spec {
        Ok(s) => s,
        Err(why) => return fail(out, None, why),
    };
    let Some(conv) = converters.iter().find(|c| c.name() == spec.converter) else {
        out.skipped.push(Skipped {
            package: id.to_owned(),
            item: index,
            converter: spec.converter.clone(),
        });
        return;
    };
    let at = stamp.slot(index, &spec.converter);
    if force {
        stamp.items[at].done.clear();
        stamp.items[at].failed.clear();
    }
    let input = match conv.input(&spec) {
        Ok(i) => i,
        Err(why) => {
            let kept = conv.kept(&spec);
            let st = &mut stamp.items[at];
            if st.done.is_empty() && !kept {
                st.off = Some(why.clone());
                out.off.push(InputOff {
                    package: id.to_owned(),
                    item: index,
                    why,
                });
            } else {
                // What was converted before stays, and so does the package.
                st.off = None;
                out.held += st.done.len();
            }
            return;
        }
    };
    {
        let st = &mut stamp.items[at];
        st.off = None;
        st.input = Some(input.label.clone());
    }
    let units = match conv.units(&input, &spec) {
        Ok(u) => u,
        Err(why) => return fail(out, None, why),
    };
    let version = conv.version();
    let keys: BTreeMap<String, String> = units
        .iter()
        .map(|u| {
            (
                u.name.clone(),
                unit_key(conv.name(), &version, &input, &spec, u),
            )
        })
        .collect();
    let mut todo = Vec::new();
    {
        let st = &mut stamp.items[at];
        st.done.retain(|n, _| keys.contains_key(n));
        st.failed.retain(|n, _| keys.contains_key(n));
        for u in &units {
            let key = &keys[&u.name];
            let made = u.output.as_ref().is_none_or(|o| o.exists());
            if st.done.get(&u.name) == Some(key) && made {
                out.held += 1;
            } else if st.failed.get(&u.name).is_some_and(|f| f.key == *key) {
                out.not_retried += 1;
            } else {
                st.done.remove(&u.name);
                st.failed.remove(&u.name);
                todo.push(u.clone());
            }
        }
    }
    if todo.is_empty() {
        return;
    }
    say(format!(
        "{id}: converting {} of {} from {}",
        todo.len(),
        units.len(),
        input.label
    ));
    let wanted: BTreeSet<String> = todo.iter().map(|u| u.name.clone()).collect();
    let mut told: BTreeSet<String> = BTreeSet::new();
    conv.convert(&input, &spec, &todo, &mut |name, r| {
        if !wanted.contains(name) || !told.insert(name.to_owned()) {
            return;
        }
        let key = keys[name].clone();
        let st = &mut stamp.items[at];
        match r {
            Ok(()) => {
                st.done.insert(name.to_owned(), key);
                out.ran.push(Ran {
                    package: id.to_owned(),
                    item: index,
                    unit: name.to_owned(),
                });
                say(format!("{id}: {name}: done"));
            }
            Err(why) => {
                st.failed.insert(
                    name.to_owned(),
                    UnitFailed {
                        key,
                        why: why.clone(),
                    },
                );
                say(format!("{id}: {name}: failed: {why}"));
                out.failed.push(ConvertFailed {
                    package: id.to_owned(),
                    item: index,
                    unit: Some(name.to_owned()),
                    why,
                });
            }
        }
        if let Err(e) = stamp.write(path) {
            say(format!("{id}: could not write the stamp: {e}"));
        }
    });
    for u in todo.iter().filter(|u| !told.contains(&u.name)) {
        fail(
            out,
            Some(u.name.clone()),
            "the converter stopped before it; the next run converts it".into(),
        );
    }
}
