use std::collections::{BTreeMap, BTreeSet};

use map::scenario_layout::{self as layout, BlockDef, FieldType, Role};

use crate::{
    BlockKind, BuilderVersions, Edit, FORMAT, FieldWrite, ForgeDoc, ImportId, MAX_DOC_BYTES, Op,
    OpId, Placement, Recipe, Refusal, Result, Sha256, TagKey, TagWrite, Value,
    document::{MAX_FIELDS, MAX_IMPORTS, MAX_OPS, MAX_RECIPES, MAX_SOURCES},
    wire,
};

pub(crate) const MAX_AUTHOR: usize = 128;
const MAX_TITLE: usize = 128;
const MAX_DESCRIPTION: usize = 4096;
const MAX_TAG_PATH: usize = 256;
const MAX_FIELD_PATH: usize = 256;
const MAX_FIELD_TEXT: usize = 512;
const MAX_BLOCK_INDEX: u32 = i16::MAX as u32;
const MAX_PATH_DEPTH: usize = 16;

/// Bounds on authored positions, in game units. The adapter supplies these
/// from the BSP. The two endpoints are inclusive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Admission limits. The wire caps apply even if a caller sets a higher cap.
/// An offline store can validate without map context. At replay, the adapter
/// verifies referenced tags, actual field types and resource costs.
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_ops: usize,
    pub max_log_entries: usize,
    /// Cap on Add ops in the active document, independent of per-block limits.
    pub max_adds: usize,
    /// The base's initial count for each block the caller knows.
    pub base_counts: BTreeMap<BlockKind, u32>,
    pub world: Option<WorldBounds>,
    pub builder: Option<BuilderVersions>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_ops: MAX_OPS,
            max_log_entries: crate::MAX_LOG_ENTRIES,
            max_adds: MAX_OPS,
            base_counts: BTreeMap::new(),
            world: None,
            builder: None,
        }
    }
}

impl Limits {
    pub(crate) fn ops(&self) -> usize {
        self.max_ops.min(MAX_OPS)
    }

    pub(crate) fn entries(&self) -> usize {
        self.max_log_entries.min(crate::MAX_LOG_ENTRIES)
    }

    fn check(&self) -> Result<()> {
        if let Some(bounds) = self.world
            && !bounds
                .min
                .into_iter()
                .zip(bounds.max)
                .all(|(a, b)| a.is_finite() && b.is_finite() && a <= b)
        {
            return Err(Refusal::invalid(
                "world bounds",
                "want finite, ordered endpoints",
            ));
        }
        for (&kind, &count) in &self.base_counts {
            wire::check_size(
                count as usize,
                kind.name(),
                block_max(layout::block(kind)) as usize,
            )?;
        }
        Ok(())
    }
}

/// Validate the whole document without opening a map.
pub fn validate(doc: &ForgeDoc, limits: &Limits) -> Result<()> {
    document(doc, limits)?;
    wire::measure(doc, "document bytes", MAX_DOC_BYTES)?;
    Ok(())
}

pub(crate) fn document(doc: &ForgeDoc, limits: &Limits) -> Result<()> {
    if doc.format != FORMAT {
        return Err(Refusal::Format(doc.format));
    }
    if let Some(expected) = limits.builder
        && doc.builder != expected
    {
        return Err(Refusal::invalid(
            "builder versions",
            format!(
                "document has {:?}, this machine has {expected:?}",
                doc.builder
            ),
        ));
    }
    limits.check()?;
    text("title", &doc.title, MAX_TITLE, Text::Name)?;
    author(&doc.author)?;
    text(
        "description",
        &doc.description,
        MAX_DESCRIPTION,
        Text::Description,
    )?;
    wire::check_size(doc.sources.len(), "sources", MAX_SOURCES)?;
    wire::check_size(doc.recipes.len(), "recipes", MAX_RECIPES)?;
    wire::check_size(doc.imports.len(), "import slots", MAX_IMPORTS)?;
    wire::check_size(doc.ops.len(), "operations", limits.ops())?;

    let mut maps = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    for source in std::iter::once(&doc.base).chain(&doc.sources) {
        if !content::manifest::is_map_name(&source.map) {
            return Err(Refusal::invalid("map name", "want a plain map name"));
        }
        if !maps.insert(source.map.to_ascii_lowercase()) || !hashes.insert(source.sha256) {
            return Err(Refusal::invalid("sources", "a map or hash is listed twice"));
        }
    }

    let mut imports = BTreeSet::new();
    for slot in &doc.imports {
        if slot.id.0 == 0 || !imports.insert(slot.id) {
            return Err(Refusal::invalid(
                "import slot id",
                "want a unique nonzero id",
            ));
        }
        text("import name", &slot.name, MAX_TITLE, Text::Name)?;
        if let Some(chassis) = &slot.chassis {
            tag_key(chassis)?;
            if chassis.group != "bipd" {
                return Err(Refusal::invalid("import chassis", "want a biped tag"));
            }
        }
    }
    for recipe in &doc.recipes {
        validate_recipe(recipe, &hashes, &imports)?;
    }

    let mut ids = BTreeSet::new();
    let mut counts = Counts::new();
    for item in &doc.ops {
        op(item, limits, false)?;
        if !ids.insert(item.id) {
            return Err(Refusal::DuplicateOp(item.id));
        }
        counts.push(item, limits)?;
    }
    Ok(())
}

fn validate_recipe(
    recipe: &Recipe,
    hashes: &BTreeSet<Sha256>,
    imports: &BTreeSet<ImportId>,
) -> Result<()> {
    match recipe {
        Recipe::Transplant { tag, source } => {
            tag_key(tag)?;
            if !hashes.contains(source) {
                return Err(Refusal::invalid(
                    "transplant source",
                    "hash is absent from base and sources",
                ));
            }
        }
        Recipe::Variant { tag, writes } => {
            tag_key(tag)?;
            wire::check_size(writes.len(), "variant writes", MAX_FIELDS)?;
            for write in writes {
                if &write.tag != tag {
                    return Err(Refusal::invalid("variant write", "write names another tag"));
                }
                tag_write(write)?;
            }
        }
        Recipe::Generate {
            generator,
            parameters,
        } => {
            token("generator", generator)?;
            fields("generator parameters", parameters, |write| {
                token("parameter name", &write.field)?;
                value(&write.value)
            })?;
        }
        Recipe::Import { slot } => {
            if !imports.contains(slot) {
                return Err(Refusal::invalid("import recipe", "no such import slot"));
            }
        }
    }
    Ok(())
}

pub(crate) fn op(op: &Op, limits: &Limits, in_log: bool) -> Result<()> {
    if op.id == OpId(0) {
        return Err(Refusal::invalid("operation id", "want a nonzero id"));
    }
    match &op.edit {
        Edit::Placement(p) => placement(p, limits),
        Edit::TagField(write) => tag_write(write),
        Edit::Undo if in_log => Ok(()),
        edit => Err(edit.unsupported()),
    }
}

fn placement(edit: &Placement, limits: &Limits) -> Result<()> {
    let def = layout::block(edit.block());
    let index = |index: u32| {
        if index >= block_max(def) {
            return Err(Refusal::invalid(
                def.kind.name(),
                "placement index exceeds block limit",
            ));
        }
        Ok(())
    };
    match edit {
        Placement::Set {
            index: i, write, ..
        } => {
            index(*i)?;
            placement_field(def, write, limits.world)
        }
        Placement::Add {
            like,
            fields: writes,
            ..
        } => {
            if let Some(why) = def.add_refusal {
                return Err(Refusal::invalid(def.kind.name(), why));
            }
            if let Some(i) = like {
                index(*i)?;
            }
            fields("placement fields", writes, |write| {
                placement_field(def, write, limits.world)
            })
        }
        Placement::Remove { index: i, .. } => {
            if let Some(why) = def.remove_refusal {
                return Err(Refusal::invalid(def.kind.name(), why));
            }
            index(*i)
        }
    }
}

fn fields(
    what: &str,
    writes: &[FieldWrite],
    check: impl Fn(&FieldWrite) -> Result<()>,
) -> Result<()> {
    wire::check_size(writes.len(), what, MAX_FIELDS)?;
    let mut names = BTreeSet::new();
    for write in writes {
        check(write)?;
        if !names.insert(&write.field) {
            return Err(Refusal::invalid(what, "a field is listed twice"));
        }
    }
    Ok(())
}

fn placement_field(def: &BlockDef, write: &FieldWrite, world: Option<WorldBounds>) -> Result<()> {
    let field = def
        .field(&write.field)
        .ok_or_else(|| Refusal::invalid(def.kind.name(), "unknown placement field"))?;
    if !field.editable {
        return Err(Refusal::invalid(field.name, "field is read-only"));
    }
    value(&write.value)?;

    let valid = match (field.ty, field.role, &write.value) {
        // The editor stores an index. Forge resolves a tag key to that index.
        (_, Role::PaletteIndex, Value::Tag(tag)) => {
            reference_group(
                field.name,
                tag.as_ref(),
                def.palette.as_ref().map_or(&[], |p| p.groups),
            )?;
            true
        }
        (_, Role::PaletteIndex, _) => false,
        (FieldType::U8, _, Value::Int(n)) => u8::try_from(*n).is_ok(),
        (FieldType::I16, _, Value::Int(n)) => i16::try_from(*n).is_ok(),
        (FieldType::U16, _, Value::Int(n)) => u16::try_from(*n).is_ok(),
        (FieldType::U32, _, Value::Int(n)) => u32::try_from(*n).is_ok(),
        (FieldType::F32, Role::Scale, Value::Float(n)) => n.get() >= 0.0,
        (FieldType::F32, _, Value::Float(_)) => true,
        (FieldType::Vec3, role, Value::Vec3(v)) => {
            if let (Role::Position, Some(bounds)) = (role, world)
                && !v
                    .iter()
                    .zip(bounds.min.into_iter().zip(bounds.max))
                    .all(|(x, (lo, hi))| (lo..=hi).contains(&x.get()))
            {
                return Err(Refusal::invalid(
                    field.name,
                    "position lies outside the BSP bounds",
                ));
            }
            true
        }
        (FieldType::TagRef, _, Value::Tag(tag)) => {
            reference_group(field.name, tag.as_ref(), layout::ITEM_GROUPS)?;
            true
        }
        _ => false,
    };
    if !valid {
        return Err(Refusal::invalid(
            field.name,
            "wrong type or value out of range",
        ));
    }
    Ok(())
}

fn reference_group(what: &str, tag: Option<&TagKey>, groups: &[[u8; 4]]) -> Result<()> {
    if let Some(tag) = tag
        && !groups
            .iter()
            .any(|group| group.as_slice() == tag.group.as_bytes())
    {
        return Err(Refusal::invalid(what, "tag group is not accepted here"));
    }
    Ok(())
}

fn block_max(def: &BlockDef) -> u32 {
    def.max_count
        .map_or(MAX_BLOCK_INDEX + 1, |count| count as u32)
}

/// Possible entry counts after a prefix of the document. Knowing the base
/// makes a range exact. With no map loaded we can still refuse edits that
/// cannot apply to any base.
#[derive(Clone, Copy)]
struct Count {
    low: u32,
    high: u32,
}

pub(crate) struct Counts {
    blocks: BTreeMap<BlockKind, Count>,
    adds: usize,
}

pub(crate) struct CountBefore {
    block: Option<(BlockKind, Option<Count>)>,
    adds: usize,
}

impl Counts {
    pub(crate) fn new() -> Self {
        Self {
            blocks: BTreeMap::new(),
            adds: 0,
        }
    }

    pub(crate) fn push(&mut self, op: &Op, limits: &Limits) -> Result<CountBefore> {
        let mut before = CountBefore {
            block: None,
            adds: self.adds,
        };
        let Edit::Placement(placement) = &op.edit else {
            return Ok(before);
        };
        let kind = placement.block();
        let cap = block_max(layout::block(kind));
        let previous = self.blocks.get(&kind).copied();
        let mut count = previous.unwrap_or_else(|| {
            limits
                .base_counts
                .get(&kind)
                .map_or(Count { low: 0, high: cap }, |&n| Count { low: n, high: n })
        });

        let i = match placement {
            Placement::Set { index, .. } | Placement::Remove { index, .. } => Some(*index),
            Placement::Add { like, .. } => *like,
        };
        if let Some(i) = i {
            // Each placement has already passed the block's index check.
            count.low = count.low.max(i + 1);
            if count.low > count.high {
                return Err(Refusal::invalid(
                    kind.name(),
                    "placement index does not exist",
                ));
            }
        }
        match placement {
            Placement::Add { .. } => {
                if count.low >= cap {
                    return Err(Refusal::limit(kind.name(), cap as usize));
                }
                wire::check_size(self.adds + 1, "placement adds", limits.max_adds)?;
                self.adds += 1;
                count.low += 1;
                count.high = (count.high + 1).min(cap);
            }
            Placement::Remove { .. } => {
                count.low -= 1;
                count.high -= 1;
            }
            Placement::Set { .. } => {}
        }

        self.blocks.insert(kind, count);
        before.block = Some((kind, previous));
        Ok(before)
    }

    pub(crate) fn pop(&mut self, before: CountBefore) {
        self.adds = before.adds;
        match before.block {
            Some((kind, Some(count))) => {
                self.blocks.insert(kind, count);
            }
            Some((kind, None)) => {
                self.blocks.remove(&kind);
            }
            None => {}
        }
    }
}

fn tag_write(write: &TagWrite) -> Result<()> {
    tag_key(&write.tag)?;
    field_path(&write.path)?;
    value(&write.value)
}

fn tag_key(tag: &TagKey) -> Result<()> {
    if tag.group.len() != 4
        || !tag.group.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        || tag.group.trim().is_empty()
    {
        return Err(Refusal::invalid(
            "tag group",
            "want four printable ASCII characters",
        ));
    }
    wire::check_size(tag.path.len(), "tag path", MAX_TAG_PATH)?;
    if !tag.path.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        || tag.path.contains(['/', ':'])
        || tag
            .path
            .split('\\')
            .any(|p| p.trim().is_empty() || p == "." || p == "..")
    {
        return Err(Refusal::invalid(
            "tag path",
            "want a relative tag key with backslashes",
        ));
    }
    Ok(())
}

fn field_path(path: &str) -> Result<()> {
    wire::check_size(path.len(), "field path", MAX_FIELD_PATH)?;
    for (i, segment) in path.split('.').enumerate() {
        wire::check_size(i + 1, "field path depth", MAX_PATH_DEPTH)?;
        let (name, index) = segment
            .split_once('[')
            .map_or((segment, None), |(n, i)| (n, Some(i)));
        if name.trim().is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b' '))
        {
            return Err(Refusal::invalid(
                "field path",
                "want dotted fields with optional array indices",
            ));
        }
        if let Some(index) = index
            && !index.strip_suffix(']').is_some_and(|i| {
                !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()) && i.parse::<u32>().is_ok()
            })
        {
            return Err(Refusal::invalid("field path", "invalid array index"));
        }
    }
    Ok(())
}

fn token(what: &str, s: &str) -> Result<()> {
    if !content::manifest::is_map_name(s) {
        return Err(Refusal::invalid(what, "want a plain name"));
    }
    Ok(())
}

pub(crate) fn value(value: &Value) -> Result<()> {
    match value {
        Value::Tag(Some(key)) => tag_key(key)?,
        Value::Text(s) => text("field text", s, MAX_FIELD_TEXT, Text::Description)?,
        Value::Int(n) if n.unsigned_abs() > 1 << 53 => {
            return Err(Refusal::invalid(
                "integer",
                "exceeds the canonical JSON integer range",
            ));
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn author(s: &str) -> Result<()> {
    text("author", s, MAX_AUTHOR, Text::Name)
}

enum Text {
    Name,
    Description,
}

fn text(what: &str, s: &str, max: usize, kind: Text) -> Result<()> {
    wire::check_size(s.len(), what, max)?;
    if matches!(kind, Text::Name) && s.trim().is_empty() {
        return Err(Refusal::invalid(what, "must have a name"));
    }
    if s.chars()
        .any(|c| c.is_control() && !(matches!(kind, Text::Description) && matches!(c, '\n' | '\t')))
    {
        return Err(Refusal::invalid(what, "contains a control character"));
    }
    Ok(())
}
