//! AI ground in a document (docs/game/game-types-without-carriers.md, "Maps
//! for scripted games"; docs/game/forge-map-variants.md, "AI ground").
//!
//! A document keeps the *places* the ground is built from and the recipe it
//! is built with, and nothing the build makes: the ground is derived from
//! the disc's collision, so each machine builds its own when the map is
//! picked.
//!
//! - **Marks.** A ground label is a netgame flag with a label op naming
//!   its place ([`GroundLabel`]). Its position and facing are the flag's;
//!   its team is the flag's team; a `siege_line`'s phase is the label's
//!   number. The materialiser reads them from the built scenario. The one
//!   exception is `siege_barrier`: a mark on a placed solid object (not a
//!   flag), numbered by the phase it opens in, which the ground build does
//!   not read: the script finds it by its label.
//! - **The recipe.** A `generate` recipe named [`GROUND_RECIPE`] with the
//!   kit's version, the home team and the mode ([`GroundRecipe`]). It is
//!   not a Forge-map recipe: a document that holds it stays a map
//!   variant ([`Tier::Variant`](crate::Tier)).
//! - **The stamp.** [`ForgeDoc::ground_stamp`] digests what the ground is
//!   built from: the recipe, the marks and every op that moves a mark or
//!   adds, moves or removes something solid. A session compares it with
//!   the stamp of its last build to know the ground is stale.
use std::collections::BTreeMap;

use crate::{
    BlockKind, Edit, Endpoint, EntryOp, FieldWrite, Forge, ForgeDoc, OpId, Placement, Recipe,
    Refusal, Result, Sha256, Value,
};

/// The generate recipe that asks for AI ground.
pub const GROUND_RECIPE: &str = "ai_ground";

/// The one block a ground label marks: marks are netgame flags, which have
/// a position, a facing and a team.
pub const GROUND_BLOCK: BlockKind = BlockKind::NetgameFlags;

/// The blocks whose entries are solid: placing, moving or removing one
/// changes the collision the ground is built over.
pub const SOLID_BLOCKS: [BlockKind; 4] = [
    BlockKind::Scenery,
    BlockKind::Crates,
    BlockKind::Machines,
    BlockKind::Vehicles,
];

/// A label the ground build reads (`firefight_kit::ground::Labels`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GroundLabel {
    /// Where a wave arrives; a facing.
    Drop,
    /// A base the actors push at; a team.
    Base,
    /// A firing position; a facing.
    Cover,
    /// A site the attackers take.
    Site,
    /// The final stand.
    Goal,
    /// Where a side spawns in a phase: a team, and the label's number is
    /// the phase.
    Line,
    /// An object that stands open from a phase on (`siege.rules`'
    /// `set_open`): the label's number is the phase it opens in, and 0 or
    /// no number is open from the start. It is the one ground label on a
    /// placed object (scenery, a crate, a machine or a vehicle), not on a
    /// netgame flag.
    Barrier,
}

impl GroundLabel {
    pub const ALL: [GroundLabel; 7] = [
        Self::Drop,
        Self::Base,
        Self::Cover,
        Self::Site,
        Self::Goal,
        Self::Line,
        Self::Barrier,
    ];

    /// The label's text, as a script reads it.
    pub const fn text(self) -> &'static str {
        match self {
            Self::Drop => "ff_drop",
            Self::Base => "ff_base",
            Self::Cover => "ff_cover",
            Self::Site => "siege_site",
            Self::Goal => "siege_goal",
            Self::Line => "siege_line",
            Self::Barrier => "siege_barrier",
        }
    }

    /// The name the palette page shows.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Drop => "Wave drop",
            Self::Base => "Base",
            Self::Cover => "Cover",
            Self::Site => "Siege site",
            Self::Goal => "Siege goal",
            Self::Line => "Siege line",
            Self::Barrier => "Siege barrier",
        }
    }

    /// The ground label a label's text names.
    pub fn of(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.text() == text)
    }

    /// Whether the mark's team matters to the build.
    pub const fn has_team(self) -> bool {
        matches!(self, Self::Base | Self::Line)
    }

    /// Whether the label carries a number (a `siege_line`'s or a
    /// `siege_barrier`'s phase).
    pub const fn has_phase(self) -> bool {
        matches!(self, Self::Line | Self::Barrier)
    }

    /// Whether the number is required: a line is in a phase, while a
    /// barrier with no number is open from the start.
    pub const fn needs_phase(self) -> bool {
        matches!(self, Self::Line)
    }

    /// Whether the label marks a netgame flag ([`GROUND_BLOCK`]), as every
    /// ground label but a barrier does; a barrier marks a placed solid
    /// object ([`SOLID_BLOCKS`]) and the ground build does not read it.
    pub const fn on_flag(self) -> bool {
        !matches!(self, Self::Barrier)
    }
}

/// A ground label on a netgame flag of the finished map, or a barrier's on
/// a solid object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroundMark {
    pub kind: GroundLabel,
    /// The block the marked entry is in: [`GROUND_BLOCK`] for every label
    /// but a barrier's.
    pub block: BlockKind,
    /// The entry's index after every placement edit.
    pub index: u32,
    /// The label's number: a `siege_line`'s or a `siege_barrier`'s phase.
    pub number: Option<i32>,
}

/// The generator's settings as a document holds them: the kit's version,
/// which team's base is home (0 red, 1 blue) and whether it builds the kit
/// or the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroundRecipe {
    pub kit_version: u32,
    pub home: u8,
    pub probe: bool,
}

const KIT_VERSION: &str = "kit_version";
const HOME: &str = "home";
const MODE: &str = "mode";

impl GroundRecipe {
    /// The recipe as a document holds it.
    pub fn to_recipe(self) -> Recipe {
        Recipe::Generate {
            generator: GROUND_RECIPE.into(),
            parameters: vec![
                FieldWrite {
                    field: KIT_VERSION.into(),
                    value: Value::Int(i64::from(self.kit_version)),
                },
                FieldWrite {
                    field: HOME.into(),
                    value: Value::Int(i64::from(self.home)),
                },
                FieldWrite {
                    field: MODE.into(),
                    value: Value::Text(if self.probe { "probe" } else { "kit" }.into()),
                },
            ],
        }
    }

    /// The recipe a `generate` recipe's parameters give: each of the
    /// three, once, in any order.
    pub fn from_parameters(parameters: &[FieldWrite]) -> Result<Self> {
        let bad = |reason: &str| Refusal::invalid("ground recipe", reason.to_string());
        let mut kit_version = None;
        let mut home = None;
        let mut probe = None;
        for write in parameters {
            let seen = match (write.field.as_str(), &write.value) {
                (KIT_VERSION, Value::Int(n)) => kit_version
                    .replace(u32::try_from(*n).map_err(|_| bad("kit_version is out of range"))?)
                    .is_some(),
                (HOME, Value::Int(n)) => home
                    .replace(
                        u8::try_from(*n)
                            .ok()
                            .filter(|t| *t < 2)
                            .ok_or_else(|| bad("home is 0 (red) or 1 (blue)"))?,
                    )
                    .is_some(),
                (MODE, Value::Text(mode)) => probe
                    .replace(match mode.as_str() {
                        "kit" => false,
                        "probe" => true,
                        _ => return Err(bad("mode is kit or probe")),
                    })
                    .is_some(),
                _ => return Err(bad("want kit_version, home and mode, as numbers and text")),
            };
            if seen {
                return Err(bad("a setting is given twice"));
            }
        }
        match (kit_version, home, probe) {
            (Some(kit_version), Some(home), Some(probe)) => Ok(Self {
                kit_version,
                home,
                probe,
            }),
            _ => Err(bad("want kit_version, home and mode")),
        }
    }

    /// The ground recipe a recipe is, `None` for any other.
    pub fn of(recipe: &Recipe) -> Result<Option<Self>> {
        match recipe {
            Recipe::Generate {
                generator,
                parameters,
            } if generator == GROUND_RECIPE => Self::from_parameters(parameters).map(Some),
            _ => Ok(None),
        }
    }
}

/// Whether a recipe is a well-formed ground recipe.
pub(crate) fn is_ground(recipe: &Recipe) -> bool {
    matches!(GroundRecipe::of(recipe), Ok(Some(_)))
}

fn names_ground(recipe: &Recipe) -> bool {
    matches!(recipe, Recipe::Generate { generator, .. } if generator == GROUND_RECIPE)
}

impl ForgeDoc {
    /// The document's ground recipe: `None` when it asks for no ground, a
    /// refusal when it has two or one that is malformed.
    pub fn ground_recipe(&self) -> Result<Option<GroundRecipe>> {
        let mut found = None;
        for recipe in &self.recipes {
            if let Some(r) = GroundRecipe::of(recipe)?
                && found.replace(r).is_some()
            {
                return Err(Refusal::invalid(
                    "ground recipe",
                    "a document asks for ground once",
                ));
            }
        }
        Ok(found)
    }

    /// The document with `recipe` as its ground recipe (`None` removes it).
    /// The recipe goes before the session's kit recipe, which stays last.
    /// A document that already has it is returned as it is; any other
    /// change drops `expect`, as the identity changes.
    pub fn with_ground(&self, recipe: Option<GroundRecipe>) -> ForgeDoc {
        let mut doc = self.clone();
        doc.recipes.retain(|r| !names_ground(r));
        if let Some(recipe) = recipe {
            let at = doc
                .recipes
                .iter()
                .position(|r| *r == crate::kit())
                .unwrap_or(doc.recipes.len());
            doc.recipes.insert(at, recipe.to_recipe());
        }
        if doc.recipes == self.recipes {
            return self.clone();
        }
        doc.expect = None;
        doc
    }

    /// The ground labels on the finished map's netgame flags, and the
    /// barriers on its solid objects, by label then block then index. A
    /// ground label on any other block, or a `siege_line` with no phase, is
    /// refused by name.
    pub fn ground_marks(&self, base_counts: &BTreeMap<BlockKind, u32>) -> Result<Vec<GroundMark>> {
        let mut marks = Vec::new();
        for active in self.labels(base_counts)?.active {
            let Some(kind) = GroundLabel::of(&active.label.text) else {
                continue;
            };
            if kind.on_flag() && active.block != GROUND_BLOCK {
                return Err(Refusal::invalid(
                    "label",
                    format!(
                        "{} marks a netgame flag, not a {} entry",
                        kind.text(),
                        active.block.name()
                    ),
                ));
            }
            if !kind.on_flag() && !SOLID_BLOCKS.contains(&active.block) {
                return Err(Refusal::invalid(
                    "label",
                    format!(
                        "{} marks a placed object (scenery, a crate, a machine or a vehicle), not a {} entry",
                        kind.text(),
                        active.block.name()
                    ),
                ));
            }
            let in_range = matches!(active.label.number, Some(0..=65535));
            if (kind.needs_phase() || (kind.has_phase() && active.label.number.is_some())) && !in_range {
                return Err(Refusal::invalid(
                    "label",
                    format!(
                        "{} wants a phase from 0 to 65535 as its number",
                        kind.text()
                    ),
                ));
            }
            marks.push(GroundMark {
                kind,
                block: active.block,
                index: active.index,
                number: active.label.number,
            });
        }
        marks.sort_by_key(|m| (m.kind, m.block as u32, m.index));
        Ok(marks)
    }

    /// The digest of what the ground is built from (see the module): the
    /// recipe, the marks, and the ops that move a mark or change what is
    /// solid. An edit that touches none of them leaves it as it was.
    pub fn ground_stamp(&self, base_counts: &BTreeMap<BlockKind, u32>) -> Result<Sha256> {
        // A barrier is no input of the ground: the script reads it.
        let marks: Vec<GroundMark> = self
            .ground_marks(base_counts)?
            .into_iter()
            .filter(|m| m.kind.on_flag())
            .collect();
        let mut added: BTreeMap<OpId, BlockKind> = BTreeMap::new();
        for op in &self.ops {
            if let Edit::Placement(Placement::Add { block, .. }) = &op.edit {
                added.insert(op.id, *block);
            }
        }
        let block_of = |end: Endpoint| match end {
            Endpoint::Base { block, .. } => Some(block),
            Endpoint::Added(id) => added.get(&id).copied(),
        };
        let relevant = |block: BlockKind| block == GROUND_BLOCK || SOLID_BLOCKS.contains(&block);

        let mut out = Vec::new();
        out.extend(b"ground-stamp-1");
        match self.ground_recipe()? {
            Some(r) => {
                out.push(1);
                out.extend(r.kit_version.to_le_bytes());
                out.push(r.home);
                out.push(u8::from(r.probe));
            }
            None => out.push(0),
        }
        for mark in &marks {
            out.push(mark.kind as u8);
            out.extend(mark.index.to_le_bytes());
            out.extend(mark.number.unwrap_or(i32::MIN).to_le_bytes());
        }
        for op in &self.ops {
            let touches = match &op.edit {
                Edit::Placement(p) => relevant(p.block()),
                Edit::Entry(e) => block_of(e.target()).is_some_and(relevant),
                Edit::Forge(Forge::Label { target, label }) => {
                    block_of(*target) == Some(GROUND_BLOCK)
                        && label
                            .as_ref()
                            .is_none_or(|l| GroundLabel::of(&l.text).is_some())
                }
                _ => false,
            };
            // Properties and areas are document data with no scenario bytes.
            if matches!(&op.edit, Edit::Entry(EntryOp::Property { .. } | EntryOp::Area { .. }))
                || !touches
            {
                continue;
            }
            let json = serde_json::to_vec(&op.edit)
                .map_err(|e| Refusal::invalid("ground stamp", e.to_string()))?;
            out.extend((json.len() as u32).to_le_bytes());
            out.extend(json);
        }
        Ok(Sha256::of(&out))
    }
}
