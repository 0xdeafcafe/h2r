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
//!
//! A `from`, `was` or `replaces` that does not match what the base holds is
//! a **refusal**: the item is for other content, and so is a patch of an
//! asset an earlier package overrides whole without naming its hash, or a
//! second override of one asset (docs/engine/asset-store.md, "Conflicts").
//! Here it stops the build like any failure. [`crate::build_map_settled`] is
//! the same build for the game, where a refused package is off for the map,
//! whole, with its reason in [`Report::refused`], and the rest still apply.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use content::package::{Item, Kind, MapScope, Maps, Package, Refusal};
use content::patch::Overlay;
use content::{Sha256, canonicalize};
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, TagId};
use serde_json::json;
use tags::repo::TagKey;

use crate::assets::Assets;
use crate::base::Base;
use crate::builders::{self, BUILDERS_VERSION};
use crate::hash::tag_hash;
use crate::unpin::MapUnpin;
pub use content_kit::kit::Shading;

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
    /// A tree that includes copies of arrays packed into other tags.
    fn owned_tree(&self, id: TagId) -> Result<Block, String> {
        self.tree(id)
    }
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

/// The `path` of an [`Override`] that takes a whole asset: a package's
/// `replaces` over fields an earlier package set.
pub const WHOLE: &str = "*";

/// A field two packages set: the later one's value is in the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub tag: TagKey,
    /// The field, or [`WHOLE`] when a later package took the asset whole.
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
    /// Packages off for this map, in the order they were refused and then
    /// by what requires them. Empty for [`build`], which stops instead.
    pub refused: Vec<Refusal>,
    /// Assets a package overrides whole, in order, and the asset's hash as
    /// that package leaves it: what a later patch's `was` names.
    pub replaced: Vec<Replaced>,
}

/// An asset a package overrides whole, and its hash as that package leaves
/// it: the asset hash when the build has the base's assets ([`Assets`]),
/// else the tag's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub key: TagKey,
    pub by: String,
    pub hash: Sha256,
}

/// Everything an item sees.
pub struct Cx<'a> {
    pub set: &'a mut TagSet,
    /// The package being built.
    pub package: &'a str,
    /// The item's place in its manifest.
    pub item: usize,
    /// What the package `requires`.
    pub(crate) requires: &'a [String],
    /// The base's asset hashes, what a `was` or `replaces` names
    /// ([`crate::assets`]); `None`, and a tag's own hash stands for them.
    pub(crate) assets: Option<&'a mut dyn Assets>,
    pub(crate) state: &'a mut State,
    unpin: Option<&'a dyn Unpin>,
    shading: Option<&'a Result<Shading, String>>,
    /// A streamed set's base map, read by offset (`TagSet::from_map_streamed`);
    /// `None` when the set holds its file prefix.
    pub(crate) base: Option<Base<'a>>,
}

#[derive(Default)]
pub(crate) struct State {
    /// Tags added by an item, with who added them.
    added: BTreeMap<TagKey, String>,
    /// The hash of each tag the first time an item touched it.
    touched: BTreeMap<TagId, Sha256>,
    /// The bytes of each tag an item kept as the build first found it, for
    /// a later `from` line ([`Cx::keep_base`]).
    first: BTreeMap<TagId, Result<Block, String>>,
    /// The package that last set a field.
    fields: BTreeMap<(TagKey, String), String>,
    /// Assets a package overrides whole, with who and the asset's hash after
    /// it: a later patch of one must name that hash and require that package.
    overridden: BTreeMap<TagKey, (String, Sha256)>,
    /// Each asset hash asked of [`Cx::assets`], by key.
    asset_hashes: BTreeMap<TagKey, Sha256>,
    /// Set just before an item fails in a way that is a refusal.
    refusal: Option<()>,
    report: Report,
    // After a builder pins a snapshot of the current map, later items
    // relocate against that snapshot instead of the original map.
    snapshot_unpin: Option<Result<MapUnpin, String>>,
    /// A streamed set's snapshot: the file it was decomposed from again, as
    /// an overlay over the base ([`Cx::base`]). The final build reads that
    /// file, not the base.
    pub(crate) snapshot_over: Option<Overlay>,
}

impl State {
    fn reader<'a>(&'a self, original: Option<&'a dyn Unpin>) -> Result<&'a dyn Unpin, String> {
        match &self.snapshot_unpin {
            Some(Ok(reader)) => Ok(reader),
            Some(Err(why)) => Err(why.clone()),
            None => original.ok_or_else(|| PINNED.into()),
        }
    }
}

impl Cx<'_> {
    /// `group` and `path` as the map names the tag (backslashes).
    pub fn key(group: FourCC, path: &str) -> TagKey {
        TagKey::new(group, path.replace('/', "\\"))
    }

    pub(crate) fn pin_snapshot(&mut self, set: TagSet, unpin: Result<MapUnpin, String>) {
        *self.set = set;
        self.state.snapshot_unpin = Some(unpin);
    }

    pub fn find(&self, key: &TagKey) -> Option<TagId> {
        self.set.find(key.group, &key.path)
    }

    fn who(&self) -> String {
        format!("package {:?} item {}", self.package, self.item)
    }

    /// Refuse a tag path that is taken: by an earlier item, or by the base.
    /// The first is a refusal (the later package is off, naming both); the
    /// second stops the build.
    pub fn check_free(&mut self, key: &TagKey) -> Result<(), String> {
        if let Some(by) = self.state.added.get(key).cloned() {
            return Err(self.refuse(format!("tag {key} is already added by {by}")));
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

    /// Note a tag of the base this item changed by other means (a sound
    /// gestalt rewritten as sounds join it). A tag an item added is not a
    /// change of the base.
    pub fn note_changed(&mut self, key: &TagKey) {
        if !self.state.added.contains_key(key) {
            self.state.report.changed.insert(key.clone());
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
        self.read_block_with(id, false)
    }

    /// Read a tag for a builder that edits arrays which may be shared.
    pub(crate) fn read_owned_block(&self, id: TagId) -> Result<Block, String> {
        self.read_block_with(id, true)
    }

    fn read_block_with(&self, id: TagId, own_arrays: bool) -> Result<Block, String> {
        match self.set.tag(id).map(|t| &t.payload) {
            Some(TagPayload::Built(b)) => Ok(b.clone()),
            Some(TagPayload::Pinned { .. }) => {
                let unpin = self.state.reader(self.unpin)?;
                if own_arrays {
                    unpin.owned_tree(id)
                } else {
                    unpin.tree(id)
                }
            }
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
            self.state.reader(self.unpin)?.relocate(self.set, id)?;
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

    /// Mark the error that follows as a **refusal** (module docs): the item
    /// is for other content, or clashes with an earlier package. The build
    /// leaves the package off for the map whole
    /// ([`crate::build_map_settled`]) or stops as it does for any failure.
    /// Returns `why` for the caller's error.
    pub fn refuse(&mut self, why: impl Into<String>) -> String {
        self.state.refusal = Some(());
        why.into()
    }

    /// Keep `block` as the tag's first bytes, for a later item's `from`
    /// lines: when none are kept and the tag still hashes as the build
    /// first found it.
    pub(crate) fn keep_base(&mut self, id: TagId, block: &Block) {
        if self.state.first.contains_key(&id) {
            return;
        }
        let Some(first) = self.state.touched.get(&id) else {
            return;
        };
        if tag_hash(self.set, id) == Some(*first) {
            self.state.first.insert(id, Ok(block.clone()));
        }
    }

    /// The tag as a `from` line checks it: the bytes the build first found,
    /// when an item kept them ([`Self::keep_base`]); else the tag as it is
    /// now. Call before changing the tag.
    pub fn base_block(&mut self, id: TagId) -> Result<Block, String> {
        match self.state.first.get(&id) {
            Some(kept) => kept.clone(),
            None => self.read_block(id),
        }
    }

    /// The asset hash of `key` (the tag `id`) as the base holds it: what a
    /// `was` or `replaces` is checked against ([`crate::assets`]). Asked of
    /// the base's assets once; a tag the base does not hold (one an item
    /// added) has its own hash, as the build first found it. With no assets
    /// given, every tag has its own hash. `Err` is a refusal when the base
    /// cannot give the asset its form.
    pub fn asset_hash(&mut self, key: &TagKey, id: TagId) -> Result<Sha256, String> {
        if let Some(h) = self.state.asset_hashes.get(key) {
            return Ok(*h);
        }
        let asked = self.assets.as_deref_mut().map(|assets| assets.hash(key));
        let hash = match asked {
            Some(Ok(Some(hash))) => hash,
            Some(Err(why)) => {
                return Err(self.refuse(format!("{key} cannot be given its normal form: {why}")));
            }
            Some(Ok(None)) | None => self
                .state
                .touched
                .get(&id)
                .copied()
                .or_else(|| tag_hash(self.set, id))
                .ok_or_else(|| self.refuse(format!("{key} is not in the map")))?,
        };
        self.state.asset_hashes.insert(key.clone(), hash);
        Ok(hash)
    }

    /// The asset hash of `tag`, a loose form of what an item wrote, read
    /// for the base ([`Assets::hash_of`]).
    pub fn asset_hash_of(&mut self, tag: &tags::repo::LooseTag) -> Result<Sha256, String> {
        self.assets
            .as_deref_mut()
            .ok_or("the build was given no asset hashes")?
            .hash_of(tag)
    }

    /// A patch of the asset `key` (an `edits` entry or a `tag-delta` item)
    /// whose base is the asset `was` names, when it names one
    /// (docs/engine/asset-store.md, "Conflicts"). `Ok(true)` when the patch
    /// is over an asset an earlier package overrides whole.
    ///
    /// - The tag is in the map, else the patch is off with the key.
    /// - `was` is the hash the build first found the tag at, else the patch
    ///   is for other content (a stale edit).
    /// - Over an earlier package's whole override, `was` names the asset's
    ///   hash after it and this package `requires` that one (or is it);
    ///   else the patch is off, naming both.
    pub fn check_base(
        &mut self,
        key: &TagKey,
        id: TagId,
        was: Option<&str>,
    ) -> Result<bool, String> {
        if let Some((by, hash)) = self.state.overridden.get(key).cloned() {
            let ours = by == self.package || self.requires.contains(&by);
            if ours && was.is_some_and(|w| w.parse::<Sha256>().is_ok_and(|h| h == hash)) {
                return Ok(true);
            }
            let need = if ours {
                String::new()
            } else {
                format!(" and the package must require {by:?}")
            };
            return Err(self.refuse(format!(
                "it patches {key}, which package {by:?} overrides whole (the asset is {hash} now): its `was` must name that hash{need}"
            )));
        }
        let Some(was) = was else {
            return Ok(false);
        };
        let want: Sha256 = was.parse().map_err(|_| {
            self.refuse(format!(
                "was {was:?} is not a SHA-256 hex digest of the tag it was made on"
            ))
        })?;
        let first = self.asset_hash(key, id)?;
        if want != first {
            return Err(self.refuse(format!(
                "stale edit: it was made on tag {key} with hash {want}, but the asset is {first}"
            )));
        }
        Ok(false)
    }

    /// The tag an item with `replaces = want` overrides whole at `key`. The
    /// tag is in the map (else off with the key), hashes to `want` as the
    /// build first found it (else the item is for other content), and no
    /// earlier package overrides it whole (else off, naming both). Call
    /// [`Self::note_replaced`] once the tag is written.
    pub fn replace_target(&mut self, key: &TagKey, want: &str) -> Result<TagId, String> {
        if let Some((by, hash)) = self.state.overridden.get(key).cloned() {
            return Err(self.refuse(format!(
                "it overrides {key} whole, which package {by:?} already overrides (the asset is {hash} now)"
            )));
        }
        let id = self
            .find(key)
            .ok_or_else(|| self.refuse(format!("the map has no tag {key}")))?;
        let want: Sha256 = want.parse().map_err(|_| {
            self.refuse(format!(
                "replaces {want:?} is not a SHA-256 hex digest of the asset it replaces"
            ))
        })?;
        let first = self.asset_hash(key, id)?;
        if want != first {
            return Err(self.refuse(format!(
                "it replaces the asset with hash {want}, but the map's {key} is {first}"
            )));
        }
        Ok(id)
    }

    /// Note that this package overrode `key` whole and left the asset
    /// hashing `hash` (its asset hash, [`Self::asset_hash_of`]; else its
    /// own): a later patch of the asset names that hash, and the fields an
    /// earlier package set on it are listed as overridden ([`WHOLE`]).
    pub fn note_replaced(&mut self, key: &TagKey, hash: Sha256) {
        let earlier: Vec<(String, String)> = self
            .state
            .fields
            .iter()
            .filter(|((k, _), _)| k == key)
            .map(|((_, path), by)| (path.clone(), by.clone()))
            .collect();
        for (path, by) in earlier {
            if by != self.package {
                self.state.report.overrides.push(Override {
                    tag: key.clone(),
                    path: path.clone(),
                    earlier: by,
                    later: self.package.to_owned(),
                });
            }
            self.state.fields.remove(&(key.clone(), path));
        }
        self.state
            .fields
            .insert((key.clone(), WHOLE.to_owned()), self.package.to_owned());
        self.state
            .overridden
            .insert(key.clone(), (self.package.to_owned(), hash));
        self.state.report.replaced.push(Replaced {
            key: key.clone(),
            by: self.package.to_owned(),
            hash,
        });
    }
}

impl<'a> Cx<'a> {
    /// What the base map lends a model: its shader templates and material.
    /// The caller of [`build_with`] reads them from the map; a build with
    /// none given refuses a `model` item by name.
    pub fn shading(&self) -> Result<&'a Shading, String> {
        match self.shading {
            Some(Ok(s)) => Ok(s),
            Some(Err(why)) => Err(format!("what the map lends a model: {why}")),
            None => {
                Err("the build was given no map to read a model's shader and material from".into())
            }
        }
    }
}

const PINNED: &str =
    "the tag is pinned in the base map and no map reader was given to lay it out anew";

/// Whether an item of `packages` for `target` reads the base map's shader
/// templates and material ([`Cx::shading`]).
pub fn wants_shading(packages: &[Loaded], target: &Target<'_>) -> bool {
    packages.iter().any(|p| {
        p.package
            .manifest
            .items
            .iter()
            .any(|i| i.kind == Kind::Model && target.takes(&i.maps))
    })
}

/// Apply `packages`, in order, to `set` for `target`. After an error `set`
/// is partly changed: the caller drops it.
pub fn build(
    set: &mut TagSet,
    packages: &[Loaded],
    target: &Target<'_>,
    unpin: Option<&dyn Unpin>,
) -> Result<Report, BuildError> {
    build_with(set, packages, target, unpin, None)
}

/// [`build`], with what the base map lends a `model` item given.
pub fn build_with(
    set: &mut TagSet,
    packages: &[Loaded],
    target: &Target<'_>,
    unpin: Option<&dyn Unpin>,
    shading: Option<&Result<Shading, String>>,
) -> Result<Report, BuildError> {
    build_over(set, packages, target, unpin, shading, None).map(|(report, _)| report)
}

/// [`build_with`] on a set streamed from `base` (`None`: a set that holds
/// its file prefix), with the snapshot overlay a screen item left
/// ([`State::snapshot_over`]).
pub(crate) fn build_over(
    set: &mut TagSet,
    packages: &[Loaded],
    target: &Target<'_>,
    unpin: Option<&dyn Unpin>,
    shading: Option<&Result<Shading, String>>,
    base: Option<Base<'_>>,
) -> Result<(Report, Option<Overlay>), BuildError> {
    run(set, packages, target, unpin, shading, None, None, base).map_err(|stopped| match stopped {
        Stopped::Failed(e) => e,
        // Only the refusing build starts again.
        Stopped::Again { package, why } => unreachable!("{package}: {why}"),
    })
}

/// Why a build of a map stopped: an item failed, or a package was refused
/// after one of its items had run ([`build_refusing`]).
pub(crate) enum Stopped {
    Failed(BuildError),
    Again { package: String, why: String },
}

/// [`build_with`] for the game ([`crate::build_map_settled`]): a refused
/// package is off for the map unless it is in `strict`, and `off` grows by
/// it and by what requires it. A package refused after one of its items ran
/// cannot be taken back out of `set`, so the build stops with
/// [`Stopped::Again`] and the caller starts it again without the package.
pub(crate) fn build_refusing(
    set: &mut TagSet,
    packages: &[Loaded],
    target: &Target<'_>,
    unpin: Option<&dyn Unpin>,
    shading: Option<&Result<Shading, String>>,
    strict: &BTreeSet<String>,
    off: &mut Vec<Refusal>,
    assets: Option<&mut dyn Assets>,
    base: Option<Base<'_>>,
) -> Result<(Report, Option<Overlay>), Stopped> {
    run(
        set,
        packages,
        target,
        unpin,
        shading,
        Some((strict, off)),
        assets,
        base,
    )
}

/// Put `package`, which is in `packages`, off for a map with `why`, then
/// every package there that requires one already off (and so on), each
/// naming what it requires.
pub(crate) fn turn_off(off: &mut Vec<Refusal>, packages: &[Loaded], package: &str, why: String) {
    fn is_off<'a>(off: &'a [Refusal], id: &str) -> Option<&'a Refusal> {
        off.iter().find(|r| r.package == id)
    }
    off.push(Refusal {
        package: package.to_owned(),
        why,
    });
    loop {
        let done: &[Refusal] = off;
        let next = packages.iter().find_map(|p| {
            let id = p.package.name();
            if is_off(done, id).is_some() {
                return None;
            }
            let needs = p
                .package
                .manifest
                .package
                .requires
                .iter()
                .find_map(|r| is_off(done, r))?;
            Some((id.to_owned(), needs.package.clone(), needs.why.clone()))
        });
        let Some((id, needs, because)) = next else {
            break;
        };
        off.push(Refusal {
            package: id,
            why: format!("it requires {needs:?}, which is off for this map: {because}"),
        });
    }
}

fn run(
    set: &mut TagSet,
    packages: &[Loaded],
    target: &Target<'_>,
    unpin: Option<&dyn Unpin>,
    shading: Option<&Result<Shading, String>>,
    mut off: Option<(&BTreeSet<String>, &mut Vec<Refusal>)>,
    mut assets: Option<&mut dyn Assets>,
    base: Option<Base<'_>>,
) -> Result<(Report, Option<Overlay>), Stopped> {
    let mut state = State::default();
    for loaded in packages {
        let manifest = &loaded.package.manifest;
        let name = manifest.package.id.as_str();
        if off
            .as_ref()
            .is_some_and(|(_, off)| off.iter().any(|r| r.package == name))
        {
            continue;
        }
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
                return Err(Stopped::Failed(fail(
                    None,
                    "no builder for this kind in this build".into(),
                )));
            };
            if let Some(want) = &item.replaces
                && !builders::overrides_whole(item.kind)
            {
                return Err(Stopped::Failed(fail(
                    None,
                    format!(
                        "`replaces` ({want}) is for a `tag` item: this builder writes its tags through the map and overrides nothing whole"
                    ),
                )));
            }
            let source = source_of(loaded, item).map_err(|why| Stopped::Failed(fail(None, why)))?;
            let mut cx = Cx {
                set: &mut *set,
                package: name,
                requires: &manifest.package.requires,
                item: index,
                assets: crate::assets::lend(&mut assets),
                state: &mut state,
                unpin,
                shading,
                base,
            };
            let job = builders::Job {
                item,
                source,
                files: &loaded.files,
            };
            cx.state.refusal = None;
            let failed = (builder.apply)(&mut cx, &job).err();
            // A streamed map leaves its file prefix in the source: an item
            // that wrote it in memory cannot be served, so its package is
            // refused for the map, by name. The item changed the set, so a
            // refusing build starts again without the package.
            let wrote_prefix = failed.is_none() && prefix_written(set);
            let failed = failed.or_else(|| {
                wrote_prefix.then(|| {
                    state.refusal = Some(());
                    Failure::from(STREAMED_PREFIX)
                })
            });
            let Some(f) = failed else {
                ran += 1;
                continue;
            };
            // A refusal under the refusing build: the package is off for the
            // map, whole. Anything else stops the build with the item's line.
            let refused = state.refusal.take().is_some();
            let off = match off.as_mut() {
                Some((strict, off)) if refused && !strict.contains(name) => off,
                _ => return Err(Stopped::Failed(fail(f.line, f.why))),
            };
            if ran > 0 || wrote_prefix {
                return Err(Stopped::Again {
                    package: name.to_owned(),
                    why: f.why,
                });
            }
            // None of its items ran: the rest are skipped, as if it were not here.
            turn_off(off, packages, name, f.why);
            break;
        }
        if ran > 0 {
            state.report.packages.push((name.to_owned(), ran));
        }
    }
    Ok((state.report, state.snapshot_over))
}

/// Why a package is refused for a streamed map ([`prefix_written`]).
pub(crate) const STREAMED_PREFIX: &str = "it writes the map's raw data or BSP regions in memory, which a streamed map leaves in its file";

/// Whether a builder wrote the file prefix of a set that leaves it in its
/// source (`map::build::TagSet::from_map_streamed`).
fn prefix_written(set: &TagSet) -> bool {
    set.pinned
        .as_ref()
        .is_some_and(|p| p.file_in_source > 0 && !p.file.is_empty())
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
