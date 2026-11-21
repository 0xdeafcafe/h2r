//! Test-map recipes (docs/game/test-maps.md): a declarative description of
//! a purpose-built test map, turned into placement edits on a base map
//! through a [`Session`], so every edit is checked as the editor checks it,
//! and saved as a map package (docs/game/maps.md).
//!
//! A recipe names a base map, an anchor (a placement of the base, or a point
//! given here), the blocks to clear and what to place. Positions are given
//! in the anchor's frame (forward, left, up, in game units) and dropped onto
//! the floor of the base's structure BSP, so a recipe carries no geometry
//! and no coordinates read from the disc. Types are named by tag path, or by
//! a part of one, and resolved in the base map's own palettes and tag
//! index when the map is built.
//!
//! Built-in recipes are TOML files in `crates/tools/scenario-edit/recipes/`,
//! compiled in ([`BUILT_IN`]).
//!
//! The order of a build: read the anchor from the base; apply `[[set]]`
//! patches to the base's entries; then, block by block, `clear` and
//! `[[place]]`. A cleared block's entries are reused for its placements
//! (fields set over the base's entry, the rest of its bytes kept) before
//! any is added, and the ones left over are removed, so a rig needs no
//! growth while it places no more than the base had.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::geometry::Tri;
use crate::layout::{self, BlockKind, FieldType, Role};
use crate::scenario::TagRef;
use crate::{Command, Error, MapSource, PackageForm, Result, SavedPackage, Scenario, Session, Value};

/// The recipes that ship with the repository, by name. Data only: names,
/// tag path fragments and offsets.
pub const BUILT_IN: &[(&str, &str)] = &[
    ("firefight-probe", include_str!("../recipes/firefight-probe.toml")),
    ("empty-room", include_str!("../recipes/empty-room.toml")),
    ("spawn-grid", include_str!("../recipes/spawn-grid.toml")),
    ("weapon-range", include_str!("../recipes/weapon-range.toml")),
    ("vehicle-course", include_str!("../recipes/vehicle-course.toml")),
    ("hud-check", include_str!("../recipes/hud-check.toml")),
    ("superbounce", include_str!("../recipes/superbounce.toml")),
    ("lighting-box", include_str!("../recipes/lighting-box.toml")),
    ("shadow-rig", include_str!("../recipes/shadow-rig.toml")),
    ("shader-row", include_str!("../recipes/shader-row.toml")),
];

/// The game variants a test map loads with. The load builds the variant
/// with the game's own defaults for Slayer (docs/runtime/level-boot.md).
pub const VARIANTS: &[&str] = &["slayer"];

/// Floors this far above a placement's point still count as under it.
pub const FLOOR_REACH_UP: f32 = 2.0;
/// How far above the floor a dropped placement is put.
pub const FLOOR_LIFT: f32 = 0.05;
/// The most placements one recipe may expand to.
pub const MAX_PLACEMENTS: usize = 256;
/// The most types an `each` takes when it gives no `max`.
pub const EACH_MAX: usize = 64;

fn bad(why: impl Into<String>) -> Error {
    Error::Recipe(why.into())
}

/// How close a rig comes to what its name promises.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fidelity {
    /// The builder supplies every part of the rig.
    Full,
    /// Part of it needs what placement edits can't make (new geometry,
    /// light fixtures); `note` says which part.
    Approximate,
}

/// Which offline builder provides the contents of a test map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Builder {
    #[default]
    Placements,
    FirefightProbe,
}

/// A recipe, as its TOML file or a JSON object reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// `[a-z0-9-]`, at most 40 bytes: the test map's name and package
    /// directory (`test-<name>`).
    pub name: String,
    pub title: String,
    /// What the rig is for, one or two sentences.
    pub purpose: String,
    /// The base map: a disc map's file name without `.map`.
    pub base: String,
    pub fidelity: Fidelity,
    #[serde(default)]
    pub builder: Builder,
    /// What an approximate rig leaves out (required when approximate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The game variant the map loads with ([`VARIANTS`]).
    #[serde(default = "slayer")]
    pub variant: String,
    #[serde(default)]
    pub anchor: Anchor,
    /// Blocks emptied before placing: `"all"` or a list of block names.
    #[serde(default)]
    pub clear: Clear,
    /// Let a block grow past the base's size (the editor's switch).
    #[serde(default)]
    pub allow_growth: bool,
    /// Field patches to the base's own entries, before clearing.
    #[serde(default)]
    pub set: Vec<SetOp>,
    #[serde(default)]
    pub place: Vec<Place>,
    /// Extra checks for `sim.expect` once the map is loaded, in
    /// `agent_proto::expect`'s string form (`"<pointer> <op> [JSON]"`).
    #[serde(default)]
    pub expect: Vec<String>,
}

fn slayer() -> String {
    "slayer".into()
}

fn yes() -> bool {
    true
}

/// Where offsets are measured from: a placement of the base (by default
/// its first starting location), or a point.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// A block of the base; `starting_locations` when neither this nor
    /// `at` is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The entry of `from`, in the base before any edit.
    #[serde(default)]
    pub index: usize,
    /// A world point (game units), in place of a placement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f32; 3]>,
    /// The frame's yaw in degrees: with `at`, else in place of the
    /// placement's own facing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing: Option<f32>,
}

/// Blocks named in a `clear`: `"all"`, or a list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Clear {
    All(String),
    Some(Vec<String>),
}

impl Default for Clear {
    fn default() -> Self {
        Clear::Some(Vec::new())
    }
}

/// A patch to one of the base's entries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetOp {
    pub block: String,
    pub index: usize,
    /// Fields by the editor's names (`position`, `facing`, `team`, ...).
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// What to place: one entry, a grid of them, or one per matching type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Place {
    pub block: String,
    /// Forward, left and up from the anchor, in game units.
    #[serde(default)]
    pub at: [f32; 3],
    /// Degrees, from the anchor's facing.
    #[serde(default)]
    pub facing: f32,
    /// A grid centred on `at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<Grid>,
    /// The type placed: a tag path or a part of one (case-insensitive),
    /// looked up in the block's palette, or the map's tag index for
    /// `netgame_equipment`'s item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// One entry per matching type, `step` apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<Each>,
    /// Between the entries of an `each` (forward, left, up).
    #[serde(default)]
    pub step: [f32; 3],
    /// Drop onto the floor of the base's structure BSP below the point.
    #[serde(default = "yes")]
    pub floor: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i64>,
    /// Teams 0..teams in turn over a grid or an `each`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams: Option<i64>,
    /// Other fields by the editor's names.
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub rows: usize,
    pub cols: usize,
    /// Game units between neighbours.
    pub spacing: f32,
}

/// Which types an `each` takes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Each {
    /// A tag group (`weap`, `vehi`, `itmc`, ...). Required for
    /// `netgame_equipment`; a palette block takes its palette's entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Keep a type whose path contains any of these (all when empty).
    #[serde(default)]
    pub contains: Vec<String>,
    /// Drop a type whose path contains any of these.
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

/// A block by its script name (`scenario_edit::script::BLOCKS`).
pub fn block_kind(name: &str) -> Result<BlockKind> {
    crate::script::BLOCKS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, k)| *k)
        .ok_or_else(|| {
            bad(format!(
                "no block {name:?} (one of {})",
                crate::script::BLOCKS.map(|(n, _)| n).join(", ")
            ))
        })
}

fn block_name(k: BlockKind) -> &'static str {
    crate::script::BLOCKS
        .iter()
        .find(|(_, b)| *b == k)
        .map_or("?", |(n, _)| n)
}

/// Whether a block's type comes from a tag reference field (`item`)
/// rather than a palette.
fn item_field(k: BlockKind) -> Option<&'static layout::FieldDef> {
    layout::block(k)
        .fields
        .iter()
        .find(|f| f.ty == FieldType::TagRef && f.editable)
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite() && x.abs() < 1.0e5)
}

fn plain_name(s: &str, extra: char) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == extra || c == '_')
}

impl Clear {
    fn kinds(&self) -> Result<Vec<BlockKind>> {
        match self {
            Clear::All(a) if a == "all" => Ok(crate::script::BLOCKS.iter().map(|b| b.1).collect()),
            Clear::All(other) => Ok(vec![block_kind(other)?]),
            Clear::Some(list) => list.iter().map(|n| block_kind(n)).collect(),
        }
    }
}

impl Recipe {
    /// A recipe from TOML, checked.
    pub fn from_toml(text: &str) -> Result<Recipe> {
        let r: Recipe = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
        r.validate()?;
        Ok(r)
    }

    /// A recipe from JSON (the agent API's form), checked.
    pub fn from_json(v: serde_json::Value) -> Result<Recipe> {
        let r: Recipe = serde_json::from_value(v).map_err(|e| bad(e.to_string()))?;
        r.validate()?;
        Ok(r)
    }

    /// The package directory a test map is saved in.
    pub fn package_dir(&self) -> String {
        package_dir(&self.name)
    }

    /// This recipe on another base map: anchors and offsets are relative,
    /// so they port; a point anchor (`anchor.at`) does not, and is refused.
    pub fn on_base(&self, base: &str) -> Result<Recipe> {
        if self.base.eq_ignore_ascii_case(base) {
            return Ok(self.clone());
        }
        if self.anchor.at.is_some() {
            return Err(bad(format!(
                "{} is anchored at a point of {}; it doesn't port to another map",
                self.name, self.base
            )));
        }
        let mut r = self.clone();
        r.base = base.to_ascii_lowercase();
        r.name = format!("{}-{}", self.name, r.base.replace('_', "-"));
        r.validate()?;
        Ok(r)
    }

    /// Every rule a recipe must keep, checked before anything is read.
    pub fn validate(&self) -> Result<()> {
        let name_ok = plain_name(&self.name, '-')
            && !self.name.contains('_')
            && self.name.len() <= 40
            && !self.name.starts_with('-')
            && !self.name.ends_with('-');
        if !name_ok {
            return Err(bad(format!(
                "name {:?}: 1 to 40 of a-z, 0-9 and '-'",
                self.name
            )));
        }
        if self.title.trim().is_empty() {
            return Err(bad("title is empty"));
        }
        if !plain_name(&self.base, '-') {
            return Err(bad(format!(
                "base {:?}: a disc map's file name without .map",
                self.base
            )));
        }
        if !VARIANTS.iter().any(|v| v.eq_ignore_ascii_case(&self.variant)) {
            return Err(bad(format!(
                "variant {:?}: one of {}",
                self.variant,
                VARIANTS.join(", ")
            )));
        }
        if self.fidelity == Fidelity::Approximate
            && self.note.as_deref().is_none_or(|n| n.trim().is_empty())
        {
            return Err(bad("an approximate rig says what it leaves out (note)"));
        }
        if let Some(from) = &self.anchor.from {
            if self.anchor.at.is_some() {
                return Err(bad("anchor: from or at, not both"));
            }
            block_kind(from)?;
        }
        if let Some(at) = self.anchor.at
            && !finite(&at)
        {
            return Err(bad("anchor.at: three finite coordinates"));
        }
        if self.anchor.facing.is_some_and(|f| !f.is_finite()) {
            return Err(bad("anchor.facing: degrees"));
        }
        if self.builder == Builder::FirefightProbe
            && (self.anchor != Anchor::default()
                || self.clear != Clear::default()
                || self.allow_growth
                || !self.set.is_empty()
                || !self.place.is_empty())
        {
            return Err(bad("firefight-probe chooses its zones and squad with the map kit; remove placement edits and anchors"));
        }
        self.clear.kinds()?;
        for (i, s) in self.set.iter().enumerate() {
            let k = block_kind(&s.block)?;
            check_fields(k, &s.fields, false).map_err(|e| bad(format!("set {i}: {e}")))?;
        }
        let mut total = 0;
        for (i, p) in self.place.iter().enumerate() {
            total += p.validate().map_err(|e| bad(format!("place {i}: {e}")))?;
        }
        if total > MAX_PLACEMENTS {
            return Err(bad(format!(
                "{total} placements; at most {MAX_PLACEMENTS}"
            )));
        }
        if self.expect.iter().any(|e| e.trim().is_empty()) {
            return Err(bad("an empty expect"));
        }
        Ok(())
    }
}

/// `test-<name>`.
pub fn package_dir(name: &str) -> String {
    format!("test-{name}")
}

/// Fields by name: known to the block, editable, and not the ones a
/// placement sets itself (`position` and the facing come from `at` and
/// `facing`; the type from `tag` or `each`).
fn check_fields(
    k: BlockKind,
    fields: &BTreeMap<String, serde_json::Value>,
    placing: bool,
) -> std::result::Result<(), String> {
    let def = layout::block(k);
    for name in fields.keys() {
        let f = def
            .field(name)
            .ok_or_else(|| format!("{} has no field {name:?}", block_name(k)))?;
        if !f.editable {
            return Err(format!("{name} is read-only"));
        }
        if f.ty == FieldType::TagRef {
            return Err(format!("{name}: name the type with tag or each"));
        }
        if placing && matches!(f.role, Role::Position | Role::Rotation | Role::Facing) {
            return Err(format!("{name}: use at and facing"));
        }
    }
    Ok(())
}

impl Place {
    /// Checks the placement; how many entries it may make (an `each` counts
    /// its cap).
    fn validate(&self) -> std::result::Result<usize, String> {
        let k = block_kind(&self.block).map_err(|e| e.to_string())?;
        let def = layout::block(k);
        if !finite(&self.at) || !finite(&self.step) || !self.facing.is_finite() {
            return Err("at, step and facing must be finite".into());
        }
        if self.grid.is_some() && self.each.is_some() {
            return Err("grid or each, not both".into());
        }
        if self.tag.is_some() && self.each.is_some() {
            return Err("tag or each, not both".into());
        }
        let typed = def.palette.is_some() || item_field(k).is_some();
        if (self.tag.is_some() || self.each.is_some()) && !typed {
            return Err(format!("{} has no type to choose", self.block));
        }
        if let Some(t) = &self.tag
            && t.trim().is_empty()
        {
            return Err("tag is empty".into());
        }
        if (self.team.is_some() || self.teams.is_some()) && def.field("team").is_none() {
            return Err(format!("{} has no team", self.block));
        }
        if self.team.is_some() && self.teams.is_some() {
            return Err("team or teams, not both".into());
        }
        if self.teams.is_some_and(|t| !(1..=16).contains(&t)) {
            return Err("teams: 1 to 16".into());
        }
        check_fields(k, &self.fields, true)?;
        if let Some(g) = &self.grid {
            if !(1..=16).contains(&g.rows) || !(1..=16).contains(&g.cols) {
                return Err("grid: 1 to 16 rows and columns".into());
            }
            if !(g.spacing.is_finite() && g.spacing > 0.0 && g.spacing < 1000.0) {
                return Err("grid.spacing: a positive distance".into());
            }
            return Ok(g.rows * g.cols);
        }
        if let Some(e) = &self.each {
            match (&e.group, item_field(k)) {
                (Some(g), _) if g.len() != 4 || !g.is_ascii() => {
                    return Err(format!("each.group {g:?}: four characters"));
                }
                (Some(g), Some(_)) if !layout::ITEM_GROUPS.iter().any(|x| x == g.as_bytes()) => {
                    return Err(format!("each.group {g:?} can't be placed by {}", self.block));
                }
                (None, Some(_)) => {
                    return Err(format!("each.group is needed for {}", self.block));
                }
                _ => {}
            }
            if e.max == Some(0) {
                return Err("each.max: at least 1".into());
            }
            return Ok(e.max.unwrap_or(EACH_MAX));
        }
        Ok(1)
    }
}

/// The frame placements are given in: a point and a yaw.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub pos: [f32; 3],
    /// Radians, counter-clockwise from +x.
    pub yaw: f32,
}

impl Frame {
    /// `(forward, left, up)` in this frame, as a world point.
    pub fn world(&self, off: [f32; 3]) -> [f32; 3] {
        let (s, c) = self.yaw.sin_cos();
        [
            self.pos[0] + off[0] * c - off[1] * s,
            self.pos[1] + off[0] * s + off[1] * c,
            self.pos[2] + off[2],
        ]
    }
}

/// The highest floor under `(x, y)` no higher than `from_z +`
/// [`FLOOR_REACH_UP`]: triangles facing up or down by more than 60
/// degrees from vertical (either winding), hit by a vertical line.
pub fn floor_z(tris: &[Tri], x: f32, y: f32, from_z: f32) -> Option<f32> {
    let top = from_z + FLOOR_REACH_UP;
    let mut best: Option<f32> = None;
    for t in tris {
        if t.n[2].abs() < 0.5 {
            continue;
        }
        let [a, b, c] = t.p;
        let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if d.abs() < 1e-9 {
            continue;
        }
        let l1 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
        let l2 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
        let l3 = 1.0 - l1 - l2;
        const E: f32 = -1e-4;
        if l1 < E || l2 < E || l3 < E {
            continue;
        }
        let z = l1 * a[2] + l2 * b[2] + l3 * c[2];
        if z <= top && best.is_none_or(|high| z > high) {
            best = Some(z);
        }
    }
    best
}

/// One entry a recipe placed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub block: String,
    pub index: usize,
    pub pos: [f32; 3],
    pub facing_deg: f32,
    /// The type's tag path, from the map's own names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i64>,
    /// Whether a floor was found under it (else it stays at its point).
    pub on_floor: bool,
}

/// What a build did, for the agent that asked for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Built {
    pub recipe: String,
    pub base: String,
    pub fidelity: Option<Fidelity>,
    /// The anchor's point and facing.
    pub anchor: Option<Frame>,
    /// One line per step.
    pub done: Vec<String>,
    /// Entries per block after the build, non-empty blocks only.
    pub counts: BTreeMap<String, usize>,
    pub placed: Vec<Placed>,
    /// What was left as it was, and why.
    pub notes: Vec<String>,
    /// Checks for `sim.expect` once loaded: the spawn's point when the rig
    /// has one starting location, then the recipe's own.
    pub expect: Vec<serde_json::Value>,
}

/// The type a placement takes.
#[derive(Clone, Debug, PartialEq)]
enum Type {
    /// The entry's own (a reused entry), or the palette's first (a new one).
    Own,
    Palette(i64, String),
    Item(TagRef, String),
}

/// One entry to write.
#[derive(Clone, Debug)]
struct Spec {
    pos: [f32; 3],
    yaw: f32,
    on_floor: bool,
    ty: Type,
    team: Option<i64>,
    fields: BTreeMap<String, serde_json::Value>,
}

/// The types a placement may choose from, sorted by path: the block's
/// palette (index, tag, path), or the map's tags of the item's groups.
fn candidates(sc: &Scenario, k: BlockKind, group: Option<&str>) -> Vec<(Type, String)> {
    let name_of = |r: &TagRef| {
        sc.tags
            .get(&r.id)
            .and_then(|t| t.name.clone())
            .unwrap_or_default()
    };
    let mut out: Vec<(Type, String)> = if layout::block(k).palette.is_some() {
        sc.block(k)
            .palette
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_none())
            .filter(|(_, r)| group.is_none_or(|g| r.group.0 == g.as_bytes()))
            .map(|(i, r)| {
                let n = name_of(r);
                (Type::Palette(i as i64, n.clone()), n)
            })
            .collect()
    } else {
        sc.tags
            .iter()
            .filter(|(_, t)| layout::ITEM_GROUPS.contains(&t.group.0))
            .filter(|(_, t)| group.is_none_or(|g| t.group.0 == g.as_bytes()))
            .map(|(id, t)| {
                let n = t.name.clone().unwrap_or_default();
                (
                    Type::Item(
                        TagRef {
                            group: t.group,
                            id: *id,
                        },
                        n.clone(),
                    ),
                    n,
                )
            })
            .collect()
    };
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by(|a, b| a.1 == b.1 && !a.1.is_empty());
    out
}

fn matches(path: &str, needle: &str) -> bool {
    path.to_ascii_lowercase()
        .contains(&needle.trim().to_ascii_lowercase())
}

/// The anchor's frame in the base, before any edit.
fn anchor_frame(sc: &Scenario, a: &Anchor) -> Result<Frame> {
    let yaw_override = a.facing.map(f32::to_radians);
    if let Some(at) = a.at {
        return Ok(Frame {
            pos: at,
            yaw: yaw_override.unwrap_or(0.0),
        });
    }
    let k = block_kind(a.from.as_deref().unwrap_or("starting_locations"))?;
    let def = layout::block(k);
    let pos_field = def
        .role(Role::Position)
        .ok_or_else(|| bad(format!("{} has no position", block_name(k))))?;
    let pos = sc
        .get(k, a.index, pos_field.name)
        .map_err(|_| {
            bad(format!(
                "the anchor: {} has no entry {} in the base",
                block_name(k),
                a.index
            ))
        })?
        .as_vec3()
        .ok_or_else(|| bad("the anchor's position is not a point"))?;
    let own = if let Some(f) = def.role(Role::Facing) {
        sc.get(k, a.index, f.name)?.as_f32()
    } else if let Some(f) = def.role(Role::Rotation) {
        sc.get(k, a.index, f.name)?.as_vec3().map(|r| r[0])
    } else {
        None
    };
    Ok(Frame {
        pos,
        yaw: yaw_override.or(own).unwrap_or(0.0),
    })
}

/// The placements of `p`, resolved against the base.
fn expand(sc: &Scenario, tris: &[Tri], frame: Frame, p: &Place) -> Result<Vec<Spec>> {
    let k = block_kind(&p.block)?;
    let mut offsets: Vec<([f32; 3], Type)> = Vec::new();
    if let Some(g) = &p.grid {
        let ty = match &p.tag {
            Some(t) => pick(sc, k, None, t)?,
            None => Type::Own,
        };
        for r in 0..g.rows {
            for c in 0..g.cols {
                let f = (r as f32 - (g.rows as f32 - 1.0) / 2.0) * g.spacing;
                let l = (c as f32 - (g.cols as f32 - 1.0) / 2.0) * g.spacing;
                offsets.push(([p.at[0] + f, p.at[1] + l, p.at[2]], ty.clone()));
            }
        }
    } else if let Some(e) = &p.each {
        let all = candidates(sc, k, e.group.as_deref());
        let chosen: Vec<Type> = all
            .into_iter()
            .filter(|(_, n)| e.contains.is_empty() || e.contains.iter().any(|c| matches(n, c)))
            .filter(|(_, n)| !e.exclude.iter().any(|x| matches(n, x)))
            .map(|(t, _)| t)
            .take(e.max.unwrap_or(EACH_MAX))
            .collect();
        if chosen.is_empty() {
            return Err(bad(format!(
                "{}: no type in the base matches each {e:?}",
                p.block
            )));
        }
        for (i, ty) in chosen.into_iter().enumerate() {
            let s = i as f32;
            offsets.push((
                [
                    p.at[0] + s * p.step[0],
                    p.at[1] + s * p.step[1],
                    p.at[2] + s * p.step[2],
                ],
                ty,
            ));
        }
    } else {
        let ty = match &p.tag {
            Some(t) => pick(sc, k, None, t)?,
            None => Type::Own,
        };
        offsets.push((p.at, ty));
    }
    let yaw = frame.yaw + p.facing.to_radians();
    Ok(offsets
        .into_iter()
        .enumerate()
        .map(|(i, (off, ty))| {
            let mut pos = frame.world(off);
            let mut on_floor = false;
            if p.floor
                && let Some(z) = floor_z(tris, pos[0], pos[1], pos[2])
            {
                pos[2] = z + FLOOR_LIFT;
                on_floor = true;
            }
            Spec {
                pos,
                yaw,
                on_floor,
                ty,
                team: p.team.or(p.teams.map(|n| i as i64 % n)),
                fields: p.fields.clone(),
            }
        })
        .collect())
}

/// The one type `needle` names: an exact path first, else the first path
/// (sorted) that contains it.
fn pick(sc: &Scenario, k: BlockKind, group: Option<&str>, needle: &str) -> Result<Type> {
    let all = candidates(sc, k, group);
    let n = needle.trim();
    all.iter()
        .find(|(_, p)| p.eq_ignore_ascii_case(n))
        .or_else(|| all.iter().find(|(_, p)| matches(p, n)))
        .map(|(t, _)| t.clone())
        .ok_or_else(|| {
            bad(format!(
                "{}: no type in the base matches tag {needle:?}",
                block_name(k)
            ))
        })
}

fn set(s: &mut Session, k: BlockKind, i: usize, field: &'static str, v: Value) -> Result<()> {
    s.apply(Command::set(k, i, field, v))
}

fn set_fields(
    s: &mut Session,
    k: BlockKind,
    i: usize,
    fields: &BTreeMap<String, serde_json::Value>,
) -> Result<()> {
    let def = layout::block(k);
    for (name, v) in fields {
        let f = def.field(name).ok_or_else(|| Error::NoField(name.clone()))?;
        set(s, k, i, f.name, crate::script::value(f.ty, name, v)?)?;
    }
    Ok(())
}

/// Writes `spec` over entry `i` of block `k`.
fn write(s: &mut Session, k: BlockKind, i: usize, spec: &Spec) -> Result<Option<String>> {
    let def = layout::block(k);
    if let Some(f) = def.role(Role::Position) {
        set(s, k, i, f.name, Value::Vec3(spec.pos))?;
    }
    if let Some(f) = def.role(Role::Facing) {
        set(s, k, i, f.name, Value::Float(spec.yaw))?;
    } else if let Some(f) = def.role(Role::Rotation) {
        set(s, k, i, f.name, Value::Vec3([spec.yaw, 0.0, 0.0]))?;
    }
    let tag = match &spec.ty {
        Type::Own => None,
        Type::Palette(index, name) => {
            if let Some(f) = def.role(Role::PaletteIndex) {
                set(s, k, i, f.name, Value::Int(*index))?;
            }
            Some(name.clone())
        }
        Type::Item(r, name) => {
            if let Some(f) = item_field(k) {
                set(s, k, i, f.name, Value::Tag(*r))?;
            }
            Some(name.clone())
        }
    };
    if let (Some(t), Some(f)) = (spec.team, def.field("team")) {
        set(s, k, i, f.name, Value::Int(t))?;
    }
    set_fields(s, k, i, &spec.fields)?;
    Ok(tag.filter(|n| !n.is_empty()))
}

fn len(s: &Session, k: BlockKind) -> usize {
    s.scenario().map_or(0, |sc| sc.block(k).len())
}

/// Runs `recipe` on the map open in `s` (its base). Stops at the first
/// refused edit, saying which step.
pub fn build(s: &mut Session, recipe: &Recipe) -> Result<Built> {
    recipe.validate()?;
    if recipe.builder != Builder::Placements {
        return Err(bad("the firefight-probe recipe requires the Firefight kit builder"));
    }
    let sc = s.scenario().ok_or(Error::Save("no map is open"))?;
    let frame = anchor_frame(sc, &recipe.anchor)?;
    let tris: Vec<Tri> = s.world().0.to_vec();
    let sc = s.scenario().ok_or(Error::Save("no map is open"))?;
    let mut plan: BTreeMap<BlockKind, Vec<Spec>> = BTreeMap::new();
    for (i, p) in recipe.place.iter().enumerate() {
        let specs = expand(sc, &tris, frame, p).map_err(|e| bad(format!("place {i}: {e}")))?;
        plan.entry(block_kind(&p.block)?).or_default().extend(specs);
    }
    let cleared = recipe.clear.kinds()?;
    s.set_allow_growth(recipe.allow_growth);
    let mut b = Built {
        recipe: recipe.name.clone(),
        base: recipe.base.clone(),
        fidelity: Some(recipe.fidelity),
        anchor: Some(frame),
        ..Built::default()
    };
    for (i, op) in recipe.set.iter().enumerate() {
        let k = block_kind(&op.block)?;
        set_fields(s, k, op.index, &op.fields).map_err(|e| bad(format!("set {i}: {e}")))?;
        b.done.push(format!("set {} {}", op.block, op.index));
    }
    for (name, k) in crate::script::BLOCKS {
        let specs = plan.remove(&k).unwrap_or_default();
        let clear = cleared.contains(&k);
        if specs.is_empty() && !clear {
            continue;
        }
        let n = len(s, k);
        if specs.is_empty() && n == 0 {
            continue;
        }
        let reuse = if clear { specs.len().min(n) } else { 0 };
        let mut indices = Vec::with_capacity(specs.len());
        for (i, spec) in specs[..reuse].iter().enumerate() {
            indices.push((i, write(s, k, i, spec).map_err(|e| bad(format!("{name} {i}: {e}")))?));
        }
        let mut removed = 0;
        if clear && n > specs.len() {
            if let Some(why) = s.remove_refusal(k) {
                b.notes.push(format!("{name}: {} entries kept ({why})", n - specs.len()));
            } else {
                for i in (specs.len()..n).rev() {
                    s.remove_entry(k, i)?;
                }
                removed = n - specs.len();
            }
        }
        // A copy of a starting location, flag or netgame item keeps the
        // base's other fields; an object starts zeroed, so it carries no
        // scenario object name.
        let like = (layout::block(k).object_type.is_none() && len(s, k) > 0).then_some(0);
        for spec in &specs[reuse..] {
            let i = s
                .add_entry(k, like)
                .map_err(|e| bad(format!("{name}: adding an entry: {e}")))?;
            indices.push((i, write(s, k, i, spec).map_err(|e| bad(format!("{name} {i}: {e}")))?));
        }
        b.done.push(format!(
            "{name}: {} placed ({reuse} reused, {} added), {removed} removed",
            specs.len(),
            specs.len() - reuse
        ));
        for ((index, tag), spec) in indices.into_iter().zip(&specs) {
            b.placed.push(Placed {
                block: name.into(),
                index,
                pos: spec.pos,
                facing_deg: spec.yaw.to_degrees(),
                tag,
                team: spec.team,
                on_floor: spec.on_floor,
            });
        }
    }
    for (name, k) in crate::script::BLOCKS {
        let c = len(s, k);
        if c > 0 {
            b.counts.insert(name.into(), c);
        }
    }
    let spawns: Vec<&Placed> = b
        .placed
        .iter()
        .filter(|p| p.block == "starting_locations")
        .collect();
    if let [one] = spawns.as_slice()
        && b.counts.get("starting_locations") == Some(&1)
    {
        b.expect.push(serde_json::json!({
            "pointer": "/player/0/pos",
            "op": "near",
            "want": {"to": one.pos, "within": 1.5},
        }));
    }
    b.expect
        .extend(recipe.expect.iter().map(|e| serde_json::Value::String(e.clone())));
    Ok(b)
}

/// Opens `source` (the base map, `maps\<base>.map`), builds `recipe` and
/// saves the result as package `recipe.package_dir()` under `data_root`'s
/// `maps/`. An existing directory of that name is refused, as the editor
/// refuses it.
pub fn build_package(
    source: MapSource,
    recipe: &Recipe,
    data_root: &Path,
) -> Result<(Built, SavedPackage)> {
    build_package_with_replace(source, recipe, data_root, false)
}

/// Save the rebuilt package over an earlier build when requested. The
/// package store publishes the new manifest after its patch is complete.
pub fn build_package_with_replace(
    source: MapSource,
    recipe: &Recipe,
    data_root: &Path,
    replace: bool,
) -> Result<(Built, SavedPackage)> {
    let mut s = Session::new();
    s.build_scenes = false;
    s.set_data_root(data_root);
    s.open_named(source, &recipe.base)?;
    let built = build(&mut s, recipe)?;
    let form = PackageForm {
        name: format!("Test: {}", recipe.title),
        description: recipe.purpose.clone(),
        dir: recipe.package_dir(),
        ..PackageForm::default()
    };
    let saved = crate::package::save_package(
        &s.maps_dir(),
        &recipe.base,
        s.source().ok_or(Error::Save("no map is open"))?,
        s.scenario().ok_or(Error::Save("no map is open"))?,
        &form,
        replace.then_some(form.dir.as_str()),
        false,
    )?;
    Ok((built, saved))
}

/// The disc map called `base` (any case), from the maps under the
/// image's `/maps/`.
pub fn disc_source(iso: &Path, base: &str) -> Result<MapSource> {
    MapSource::disc_maps(iso)?
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(base))
        .map(|(_, s)| s)
        .ok_or_else(|| bad(format!("the disc has no maps\\{base}.map")))
}

/// Every built-in recipe, parsed and checked.
pub fn built_in() -> Result<Vec<Recipe>> {
    BUILT_IN
        .iter()
        .map(|(n, text)| Recipe::from_toml(text).map_err(|e| bad(format!("{n}: {e}"))))
        .collect()
}

/// The built-in recipe called `name`.
pub fn built_in_named(name: &str) -> Result<Recipe> {
    let (_, text) = BUILT_IN
        .iter()
        .find(|(n, _)| *n == name)
        .ok_or_else(|| {
            bad(format!(
                "no built-in recipe {name:?} (one of {})",
                BUILT_IN.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
            ))
        })?;
    Recipe::from_toml(text)
}

#[cfg(test)]
mod tests;
