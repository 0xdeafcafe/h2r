//! `build`: an ordered set of packages applied to a map's [`TagSet`].
//!
//! The set is the base, `onto` style: nothing is rebuilt that no item
//! touches, so the output differs from the base only in the tags the items
//! add or change. A tag in the base map that a decomposed map keeps pinned is
//! laid out anew (through [`Unpin`]) only when an item changes it.
//!
//! Conflicts (docs/editor/studio-redesign.md, "Identity, dependencies and
//! load order"):
//!
//! - two items that add the same tag path stop the build, the error naming
//!   both; so does an item that adds a tag the base already has;
//! - two packages that set the same field: the later wins and the build
//!   report lists the override;
//! - an edit whose tag or path does not resolve, or whose `was` is not the
//!   tag it was made on, stops the build with the file's line.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use content::package::{Item, Kind, MapScope, Maps, Package};
use content::{Sha256, canonicalize};
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, TagId};
use serde_json::json;
use tags::repo::TagKey;

use crate::builders::{self, BUILDERS_VERSION};
use crate::hash::tag_hash;

/// A package with the bytes of the files its manifest names, by path in the
/// package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub package: Package,
    pub files: BTreeMap<String, Vec<u8>>,
}

/// The map a build is for.
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    /// The map's name (`lockout`), matched against an item's `maps` list
    /// without regard to case.
    pub name: &'a str,
    /// Its class: `multiplayer`, `campaign`, `mainmenu` or `shared`.
    pub scope: MapScope,
}

impl Target<'_> {
    /// Whether an item with `maps` applies to this map.
    pub fn takes(&self, maps: &Maps) -> bool {
        match maps {
            Maps::Scope(MapScope::All) => true,
            Maps::Scope(s) => *s == self.scope,
            Maps::Names(n) => n.iter().any(|n| n.eq_ignore_ascii_case(self.name)),
        }
    }
}

/// Lays a pinned tag out anew, so an item can change it. `h2` has the map's
/// reader and gives this ([`crate::MapUnpin`]); a caller with only built
/// tags gives none.
pub trait Unpin {
    /// The tag as a block tree, the base map untouched.
    fn tree(&self, id: TagId) -> Result<Block, String>;
    /// Move the tag out of its old bytes: afterwards `set` holds a
    /// [`TagPayload::Built`] tag in its place.
    fn relocate(&self, set: &mut TagSet, id: TagId) -> Result<(), String>;
}

/// Why an item failed, before it is put in its package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub why: String,
    /// A line of the item's source, 1-based.
    pub line: Option<usize>,
}

impl Failure {
    pub fn at(line: usize, why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            line: Some(line),
        }
    }
}

impl From<String> for Failure {
    fn from(why: String) -> Self {
        Self { why, line: None }
    }
}
impl From<&str> for Failure {
    fn from(why: &str) -> Self {
        Self::from(why.to_owned())
    }
}

/// An item stopped the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError {
    pub package: String,
    /// The item's place in the manifest.
    pub index: usize,
    pub kind: Kind,
    pub source: Option<String>,
    pub line: Option<usize>,
    pub why: String,
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "package {:?}, item {} ({:?}",
            self.package, self.index, self.kind
        )?;
        if let Some(s) = &self.source {
            write!(f, " {s}")?;
            if let Some(l) = self.line {
                write!(f, ", line {l}")?;
            }
        }
        write!(f, "): {}", self.why)
    }
}
impl std::error::Error for BuildError {}

/// A field two packages set: the later one's value is in the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub tag: TagKey,
    pub path: String,
    pub earlier: String,
    pub later: String,
}

impl fmt::Display for Override {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}: {:?} overrides {:?}",
            self.tag, self.path, self.later, self.earlier
        )
    }
}

/// What a build did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Packages that had an item for the map, with how many items ran.
    pub packages: Vec<(String, usize)>,
    /// Tags the items added, in order.
    pub added: Vec<TagKey>,
    /// Tags of the base the items changed.
    pub changed: BTreeSet<TagKey>,
    pub overrides: Vec<Override>,
}

/// Everything an item sees.
pub struct Cx<'a> {
    pub set: &'a mut TagSet,
    /// The package being built.
    pub package: &'a str,
    /// The item's place in its manifest.
    pub item: usize,
    pub(crate) state: &'a mut State,
    unpin: Option<&'a dyn Unpin>,
}

#[derive(Default)]
pub(crate) struct State {
    /// Tags added by an item, with who added them.
    added: BTreeMap<TagKey, String>,
    /// The hash of each tag the first time an item touched it.
    touched: BTreeMap<TagId, Sha256>,
    /// The package that last set a field.
    fields: BTreeMap<(TagKey, String), String>,
    report: Report,
}

impl Cx<'_> {
    /// `group` and `path` as the map names the tag (backslashes).
    pub fn key(group: FourCC, path: &str) -> TagKey {
        TagKey::new(group, path.replace('/', "\\"))
    }

    pub fn find(&self, key: &TagKey) -> Option<TagId> {
        self.set.find(key.group, &key.path)
    }

    fn who(&self) -> String {
        format!("package {:?} item {}", self.package, self.item)
    }

    /// Refuse a tag path that is taken: by an earlier item, or by the base.
    pub fn check_free(&self, key: &TagKey) -> Result<(), String> {
        if let Some(by) = self.state.added.get(key) {
            return Err(format!("tag {key} is already added by {by}"));
        }
        if self.find(key).is_some() {
            return Err(format!("tag {key} is already in the map"));
        }
        Ok(())
    }

    /// Add a built tag, and note it as this item's.
    pub fn add_tag(&mut self, key: &TagKey, block: Block) -> Result<TagId, String> {
        self.check_free(key)?;
        self.set.add_group(key.group, None, None);
        let id = self
            .set
            .add_tag(key.group, &key.path, TagPayload::Built(block))
            .map_err(|e| format!("adding tag {key}: {e}"))?;
        self.note_added(key, id);
        Ok(id)
    }

    /// Note a tag this item added by other means (a bitmap's writer).
    pub fn note_added(&mut self, key: &TagKey, id: TagId) {
        let who = self.who();
        self.state.added.insert(key.clone(), who);
        self.state.report.added.push(key.clone());
        if let Some(h) = tag_hash(self.set, id) {
            self.state.touched.insert(id, h);
        }
    }

    /// The tag's hash as the build first found it, taken now if this is the
    /// first touch. Call before changing the tag.
    pub fn touch(&mut self, id: TagId) -> Result<Sha256, String> {
        if let Some(h) = self.state.touched.get(&id) {
            return Ok(*h);
        }
        let h = tag_hash(self.set, id).ok_or("the tag is not in the map")?;
        self.state.touched.insert(id, h);
        Ok(h)
    }

    /// The tag's block tree: its own, or laid out anew from the base's
    /// bytes. The map is not changed.
    pub fn read_block(&self, id: TagId) -> Result<Block, String> {
        match self.set.tag(id).map(|t| &t.payload) {
            Some(TagPayload::Built(b)) => Ok(b.clone()),
            Some(TagPayload::Pinned { .. }) => match &self.unpin {
                Some(u) => u.tree(id),
                None => Err(PINNED.into()),
            },
            Some(TagPayload::Empty) => Err("the tag has no data".into()),
            Some(TagPayload::Alias { .. }) => Err("the tag shares another tag's bytes".into()),
            None => Err("the tag is not in the map".into()),
        }
    }

    /// Replace the tag's data with `block`, moving it out of its old bytes
    /// first if the base pinned it.
    pub fn write_block(&mut self, id: TagId, key: &TagKey, block: Block) -> Result<(), String> {
        if matches!(
            self.set.tag(id).map(|t| &t.payload),
            Some(TagPayload::Pinned { .. })
        ) {
            match &self.unpin {
                Some(u) => u.relocate(self.set, id)?,
                None => return Err(PINNED.into()),
            }
        }
        let entry = self.set.tag_mut(id).ok_or("the tag left the map")?;
        entry.payload = TagPayload::Built(block);
        if !self.state.added.contains_key(key) {
            self.state.report.changed.insert(key.clone());
        }
        Ok(())
    }

    /// Note that this package set a field; an earlier package's value is
    /// overridden and listed.
    pub fn note_set(&mut self, key: &TagKey, path: &str) {
        let slot = (key.clone(), path.to_owned());
        if let Some(earlier) = self.state.fields.get(&slot)
            && earlier != self.package
        {
            let o = Override {
                tag: key.clone(),
                path: path.to_owned(),
                earlier: earlier.clone(),
                later: self.package.to_owned(),
            };
            self.state.report.overrides.push(o);
        }
        self.state.fields.insert(slot, self.package.to_owned());
    }
}

const PINNED: &str =
    "the tag is pinned in the base map and no map reader was given to lay it out anew";

/// Apply `packages`, in order, to `set` for `target`. After an error `set`
/// is partly changed: the caller drops it.
pub fn build(
    set: &mut TagSet,
    packages: &[Loaded],
    target: &Target<'_>,
    unpin: Option<&dyn Unpin>,
) -> Result<Report, BuildError> {
    let mut state = State::default();
    for loaded in packages {
        let manifest = &loaded.package.manifest;
        let name = manifest.package.id.as_str();
        let mut ran = 0;
        for (index, item) in manifest.items.iter().enumerate() {
            if !target.takes(&item.maps) {
                continue;
            }
            let fail = |line: Option<usize>, why: String| BuildError {
                package: name.to_owned(),
                index,
                kind: item.kind,
                source: item.source.clone(),
                line,
                why,
            };
            let Some(builder) = builders::builder(item.kind) else {
                return Err(fail(None, "no builder for this kind in this build".into()));
            };
            let source = source_of(loaded, item).map_err(|why| fail(None, why))?;
            let mut cx = Cx {
                set: &mut *set,
                package: name,
                item: index,
                state: &mut state,
                unpin,
            };
            (builder.apply)(&mut cx, &builders::Job { item, source })
                .map_err(|f| fail(f.line, f.why))?;
            ran += 1;
        }
        if ran > 0 {
            state.report.packages.push((name.to_owned(), ran));
        }
    }
    Ok(state.report)
}

fn source_of<'a>(loaded: &'a Loaded, item: &Item) -> Result<&'a [u8], String> {
    match &item.source {
        Some(path) => loaded
            .files
            .get(path)
            .map(Vec::as_slice)
            .ok_or_else(|| format!("the package has no file {path:?}")),
        None => Ok(&[]),
    }
}

/// The key of a build's output: equal keys mean equal bytes. It is over the
/// base file's hash, the target, the ids of the packages with an item for
/// it (in order; an id covers the manifest and every source's hash) and the
/// builders' version, so a package for other maps does not move it.
pub fn cache_key(base: &Sha256, packages: &[Loaded], target: &Target<'_>) -> Sha256 {
    let ids: Vec<String> = packages
        .iter()
        .filter(|p| {
            p.package
                .manifest
                .items
                .iter()
                .any(|i| target.takes(&i.maps))
        })
        .map(|p| p.package.id.to_string())
        .collect();
    let value = json!({
        "builders": BUILDERS_VERSION,
        "base": base.to_string(),
        "map": target.name.to_ascii_lowercase(),
        "scope": target.scope.word(),
        "packages": ids,
    });
    let text = canonicalize(&value).expect("the key holds no floats");
    Sha256::of(text.as_bytes())
}
