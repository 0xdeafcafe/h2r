//! Forge labels: a name and an optional number a script finds a placement by.
//!
//! A label is a field of a placement in the document, set through the same
//! edit log as any other field. It has no scenario bytes, so a session that
//! runs no script pays nothing for it. A label op names a placement the way
//! a link does ([`Endpoint`]), so the label survives later appends and
//! swap-removes, and [`ForgeDoc::labels`](crate::ForgeDoc::labels) resolves
//! the active labels to final indices.
//!
//! A placement has at most one label; setting another replaces it. Labels
//! need not be unique: many placements share `"ctf_flag_spawn"`, and the
//! number tells them apart. A script asks for all of them.
//!
//! A document with a label op is format 2 ([`FORMAT_LABELS`](crate::FORMAT_LABELS)).
//! One with none stays format 1, so every older document keeps its bytes and
//! its identity.
//!
//! The census (below) is the start gate's view of a map: each labelled
//! thing with its class, number and team, and the object type of each tag
//! (`rules::labels::Census`; docs/game/rules-start-gate.md, "The check").
//! It has two builders with one result, [`CensusOf::of_file`] and
//! [`CensusOf::of_game`], over one function, [`labelled`].
use std::collections::BTreeMap;
use std::io::{Read, Seek};

use rules::labels::{Census, Class, Member, ObjectType};
use serde::{Deserialize, Serialize};

use crate::{
    BlockKind, Edit, Endpoint, FieldWrite, ForgeDoc, MarkerArea, OpId, Placement, Refusal, Result,
    Value,
};

/// The most bytes a label's text takes.
pub const MAX_LABEL_BYTES: usize = 32;

/// The characters a label's text may hold: lowercase ASCII letters, digits,
/// `_`, `-` and `.`. A label is a name for a script to match, not display
/// text, and the fixed set keeps its comparison byte-exact.
pub fn label_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')
}

/// A label: text of 1 to 32 bytes from [`label_char`] and an optional number
/// (`"checkpoint" 3`, `"hill" 0`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<i32>,
}

impl Label {
    /// A validated label.
    pub fn new(text: impl Into<String>, number: Option<i32>) -> Result<Self> {
        let label = Self {
            text: text.into(),
            number,
        };
        label.check()?;
        Ok(label)
    }

    /// Refuse text that is empty, over 32 bytes or outside [`label_char`].
    pub fn check(&self) -> Result<()> {
        if self.text.is_empty() || self.text.len() > MAX_LABEL_BYTES {
            return Err(Refusal::invalid(
                "label",
                format!("want 1 to {MAX_LABEL_BYTES} bytes of text"),
            ));
        }
        if !self.text.chars().all(label_char) {
            return Err(Refusal::invalid(
                "label",
                "want lowercase letters, digits, '_', '-' and '.'",
            ));
        }
        Ok(())
    }
}

/// A label an active op gives a placement, with the placement's final index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveLabel {
    /// The op that set it.
    pub id: OpId,
    pub block: BlockKind,
    /// The entry's index after every placement edit.
    pub index: u32,
    pub label: Label,
}

/// The document's active labels, resolved against the base map's counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels {
    /// By block, then final index.
    pub active: Vec<ActiveLabel>,
}

/// The label of a netgame flag of the given type on a map with no Forge
/// document (or no label on that placement), so `objects labelled
/// "ctf_flag_spawn"` works on every map. The number is the hill, 0 to 7,
/// for King of the Hill corners and none otherwise.
///
/// Types 0, 1, 4, 7, 8, 10 and 11 to 18 are traced (docs/game/bots.md 3.7;
/// `engine::objectives::kind`). Types 2 and 3 are the Assault bomb spawn and
/// goal, which bots.md 3.7 gives as "bomb and plate" without naming the
/// plate's role precisely, so their text is the less certain one. Types 5, 6
/// and 9 are not traced and have no built-in label.
pub fn builtin_label(flag_type: u16) -> Option<Label> {
    let (text, number) = match flag_type {
        0 => ("ctf_flag_spawn", None),
        1 => ("ctf_flag_return", None),
        2 => ("assault_bomb_spawn", None),
        3 => ("assault_bomb_return", None),
        4 => ("oddball_spawn", None),
        7 => ("teleporter_sender", None),
        8 => ("teleporter_receiver", None),
        10 => ("territory", None),
        11..=18 => ("hill_corner", Some(i32::from(flag_type) - 11)),
        _ => return None,
    };
    Some(Label {
        text: text.into(),
        number,
    })
}

/// The check that needs only the op.
pub(crate) fn check_op(target: &Endpoint, label: Option<&Label>) -> Result<()> {
    match *target {
        Endpoint::Base { index, .. } if index > i16::MAX as u32 => {
            return Err(Refusal::invalid("label", "entry index exceeds block limit"));
        }
        Endpoint::Added(OpId(0)) => {
            return Err(Refusal::invalid("label", "want the ID of an add op"));
        }
        _ => {}
    }
    label.map_or(Ok(()), Label::check)
}

// ---- the census ----

/// The netgame flags a census looks at, at most (the engine's own walk,
/// `engine::objectives::NETGAME_FLAG_LIMIT`).
pub const CENSUS_FLAG_LIMIT: u32 = 512;
/// The entries of any other block a census counts, at most: a label names
/// an entry by an index of at most `i16::MAX`.
pub const CENSUS_ENTRY_LIMIT: u32 = i16::MAX as u32 + 1;

/// The type and team fields of one netgame flag (`scnr+0x118`, `+0x10` and
/// `+0x12`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlagRow {
    pub kind: u16,
    pub team: u16,
}

/// What a census reads of a map before its Forge document: each block's
/// count, the netgame flags' type and team, each starting location's team
/// (`scnr+0x100`, `+0x10`), and the object type of each tag the map names
/// by path. A map file gives all of it ([`rows_of_file`]); the running
/// game gives every part but the tags, whose paths are only in the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapRows {
    pub counts: BTreeMap<BlockKind, u32>,
    pub flags: Vec<FlagRow>,
    pub starts: Vec<i16>,
    pub tags: BTreeMap<String, ObjectType>,
    /// The path of each sound (`snd!`) the map names; none when the rows do
    /// not read tag paths (the running game's).
    pub sounds: Option<std::collections::BTreeSet<String>>,
}

/// One labelled thing of a map, after its Forge document: the block and
/// final index of its placement, its label, what it is and its team.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelledThing {
    pub block: BlockKind,
    pub index: u32,
    pub label: Label,
    pub class: Class,
    /// The placement's team designator, 0 to 7: a flag's or a starting
    /// location's team field. Neutral (8) and none (-1) are `None`.
    pub team: Option<u8>,
    /// The area on the flag, in offsets from its placement.
    pub area: Option<MarkerArea>,
    /// Whether the flag's play form shows in a match: a flag the document
    /// added unless its `Show` op turns it off, a flag of the base map only
    /// when one turns it on ([`ForgeDoc::shown_flags`]). Never set on a
    /// thing that is no flag.
    pub show: bool,
}

/// A team field as a census member's team: designators 0 to 7; neutral (8),
/// none (-1) and anything else are no team.
pub fn designator(v: i64) -> Option<u8> {
    u8::try_from(v).ok().filter(|&t| t < 8)
}

/// Every labelled thing of the map `rows` describes, after `doc`, in block
/// then final index order. A netgame flag takes its own label from the
/// document, else its type's built-in one ([`builtin_label`]); a starting
/// location or an object placement is labelled only by the document.
/// Starting equipment is not a placement a script can find and is left
/// out. `rows` are the map before the document: its placement ops are
/// replayed over the flag and starting-location rows, so an added or
/// swap-removed entry keeps its fields.
pub fn labelled(rows: &MapRows, doc: Option<&ForgeDoc>) -> Result<Vec<LabelledThing>> {
    let mut flags = rows.flags.clone();
    let mut starts = rows.starts.clone();
    let mut labels = Labels::default();
    // An item spawner's team is a property of the document
    // (`EntryOp::Property`), by the spawner's final index.
    let mut spawner_teams: BTreeMap<u32, u8> = BTreeMap::new();
    let mut areas = BTreeMap::new();
    let mut shown = std::collections::BTreeSet::new();
    if let Some(doc) = doc {
        labels = doc.labels(&rows.counts)?;
        areas = doc.areas(&rows.counts)?;
        shown = doc.shown_flags(&rows.counts)?;
        let teams = crate::teams(&doc.ops);
        let resolved = (!areas.is_empty() || !teams.is_empty() || !shown.is_empty())
            .then(|| crate::resolve(&doc.ops, &rows.counts))
            .transpose()?;
        for op in &doc.ops {
            let placement = match &op.edit {
                Edit::Placement(p) => Some(p),
                Edit::Entry(_) if !areas.is_empty() || !shown.is_empty() => {
                    resolved.as_ref().and_then(|r| r.lowered.get(&op.id))
                }
                _ => None,
            };
            if let Some(p) = placement {
                replay_row(&mut flags, &mut starts, p);
            }
        }
        if let Some(resolved) = resolved {
            for (end, team) in teams {
                if let Some(index) = resolved.index(end) {
                    spawner_teams.insert(index, team);
                }
            }
        }
    }
    let own = |block: BlockKind, index: u32| {
        labels
            .active
            .iter()
            .find(|l| l.block == block && l.index == index)
            .map(|l| l.label.clone())
    };
    let mut out = Vec::new();
    for (i, f) in flags.iter().enumerate() {
        let index = i as u32;
        if let Some(label) = own(BlockKind::NetgameFlags, index).or_else(|| builtin_label(f.kind)) {
            out.push(LabelledThing {
                block: BlockKind::NetgameFlags,
                index,
                label,
                class: Class::Flag,
                team: designator(i64::from(f.team)),
                area: areas.remove(&index),
                show: shown.contains(&index),
            });
        }
    }
    for l in &labels.active {
        let (class, team) = match l.block {
            BlockKind::NetgameFlags | BlockKind::StartingEquipment => continue,
            BlockKind::StartingLocations => (
                Class::Spawn,
                starts
                    .get(l.index as usize)
                    .and_then(|&t| designator(i64::from(t))),
            ),
            BlockKind::NetgameEquipment => (Class::Object, spawner_teams.get(&l.index).copied()),
            _ => (Class::Object, None),
        };
        out.push(LabelledThing {
            block: l.block,
            index: l.index,
            label: l.label.clone(),
            class,
            team,
            area: None,
            show: false,
        });
    }
    out.sort_by_key(|t| (t.block, t.index));
    Ok(out)
}

fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

fn write_flag(f: &mut FlagRow, w: &FieldWrite) {
    match (w.field.as_str(), int(&w.value)) {
        ("type", Some(n)) => f.kind = n as u16,
        ("team", Some(n)) => f.team = n as u16,
        _ => {}
    }
}

fn write_start(t: &mut i16, w: &FieldWrite) {
    if let ("team", Some(n)) = (w.field.as_str(), int(&w.value)) {
        *t = n as i16;
    }
}

/// One placement op over the rows a census reads: an add copies its `like`
/// entry (or starts from zeros) and takes its field writes, a remove swaps
/// the last entry in, a set writes the field. Only `type` and `team` are
/// read; other fields and blocks are not a census's.
fn replay_row(flags: &mut Vec<FlagRow>, starts: &mut Vec<i16>, p: &Placement) {
    match p {
        Placement::Add {
            block: BlockKind::NetgameFlags,
            like,
            fields,
        } => {
            let mut f = like
                .and_then(|i| flags.get(i as usize).copied())
                .unwrap_or_default();
            fields.iter().for_each(|w| write_flag(&mut f, w));
            flags.push(f);
        }
        Placement::Add {
            block: BlockKind::StartingLocations,
            like,
            fields,
        } => {
            let mut t = like
                .and_then(|i| starts.get(i as usize).copied())
                .unwrap_or_default();
            fields.iter().for_each(|w| write_start(&mut t, w));
            starts.push(t);
        }
        Placement::Remove {
            block: BlockKind::NetgameFlags,
            index,
        } if (*index as usize) < flags.len() => {
            flags.swap_remove(*index as usize);
        }
        Placement::Remove {
            block: BlockKind::StartingLocations,
            index,
        } if (*index as usize) < starts.len() => {
            starts.swap_remove(*index as usize);
        }
        Placement::Set {
            block: BlockKind::NetgameFlags,
            index,
            write,
        } => {
            if let Some(f) = flags.get_mut(*index as usize) {
                write_flag(f, write);
            }
        }
        Placement::Set {
            block: BlockKind::StartingLocations,
            index,
            write,
        } => {
            if let Some(t) = starts.get_mut(*index as usize) {
                write_start(t, write);
            }
        }
        _ => {}
    }
}

/// The census of labelled things and tags: the start gate's view of a map
/// (`rules::labels::check`).
pub fn census(things: &[LabelledThing], tags: &BTreeMap<String, ObjectType>) -> Census {
    let mut census = Census {
        labels: BTreeMap::new(),
        tags: tags.clone(),
        sounds: None,
    };
    for t in things {
        census
            .labels
            .entry(t.label.text.clone())
            .or_default()
            .push(Member {
                class: t.class,
                number: t.label.number,
                team: t.team,
            });
    }
    census
}

/// The two builders of a census, with one result: from a map file and the
/// lobby's Forge document, and from the running game's scenario and the
/// session's document (docs/game/rules-start-gate.md, "The check"). A
/// trait, since [`Census`] is the `rules` crate's: with it in scope they
/// read `Census::of_file` and `Census::of_game`.
pub trait CensusOf: Sized {
    /// The census of the map file `map` with `doc` applied.
    fn of_file<R: Read + Seek>(map: &mut map::Map<R>, doc: Option<&ForgeDoc>) -> Result<Self>;
    /// The census of the loaded scenario's rows (the game reads them from
    /// guest memory) with the session's document applied.
    fn of_game(rows: &MapRows, doc: Option<&ForgeDoc>) -> Result<Self>;
}

impl CensusOf for Census {
    fn of_file<R: Read + Seek>(map: &mut map::Map<R>, doc: Option<&ForgeDoc>) -> Result<Self> {
        let rows = rows_of_file(map)?;
        let mut census = census(&labelled(&rows, doc)?, &rows.tags);
        census.sounds = rows.sounds.clone();
        Ok(census)
    }

    fn of_game(rows: &MapRows, doc: Option<&ForgeDoc>) -> Result<Self> {
        Ok(census(&labelled(rows, doc)?, &rows.tags))
    }
}

/// The object type of a tag group, for a census's tags.
pub fn object_type(group: &[u8; 4]) -> Option<ObjectType> {
    Some(match group {
        b"bipd" => ObjectType::Biped,
        b"vehi" => ObjectType::Vehicle,
        b"weap" => ObjectType::Weapon,
        b"eqip" => ObjectType::Equipment,
        b"garb" => ObjectType::Garbage,
        b"proj" => ObjectType::Projectile,
        b"scen" => ObjectType::Scenery,
        b"mach" => ObjectType::Machine,
        b"ctrl" => ObjectType::Control,
        b"lifi" => ObjectType::LightFixture,
        b"ssce" => ObjectType::SoundScenery,
        b"bloc" => ObjectType::Crate,
        b"crea" => ObjectType::Creature,
        _ => return None,
    })
}

pub(crate) fn map_error(e: impl std::fmt::Display) -> Refusal {
    Refusal::invalid("map", e.to_string())
}

fn le_u32(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| map_error("the scenario is too short for its blocks"))
}

/// A block of the scenario root `root` (at `space`), its entries' bytes;
/// at most `limit` of them.
pub(crate) fn read_block<R: Read + Seek>(
    map: &mut map::Map<R>,
    root: &[u8],
    space: map::AddressSpace,
    kind: BlockKind,
    limit: u32,
) -> Result<Vec<Vec<u8>>> {
    let def = map::scenario_layout::block(kind);
    let count = le_u32(root, def.offset as usize)?.min(limit);
    if count == 0 {
        return Ok(Vec::new());
    }
    let address = le_u32(root, def.offset as usize + 4)?;
    let len = count * def.stride;
    let at = space.translate(address, len).map_err(map_error)?;
    let bytes = map.read_file_range(at, u64::from(len)).map_err(map_error)?;
    Ok(bytes
        .chunks_exact(def.stride as usize)
        .map(<[u8]>::to_vec)
        .collect())
}

fn u16_at(e: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([e[at], e[at + 1]])
}

/// The rows of a map file: each block's count from the scenario root, the
/// netgame flags' and starting locations' fields, and the path of every
/// object definition in the script's spelling (`/` between parts).
pub fn rows_of_file<R: Read + Seek>(map: &mut map::Map<R>) -> Result<MapRows> {
    let scnr = map
        .scenario()
        .ok_or_else(|| map_error("the map has no scenario"))?;
    let id = scnr.id;
    let loc = scnr
        .location
        .ok_or_else(|| map_error("the scenario has no payload"))?;
    let root = map.read_tag_bytes(id).map_err(map_error)?;
    let mut rows = MapRows::default();
    for def in map::scenario_layout::BLOCKS {
        let count = le_u32(&root, def.offset as usize)?;
        rows.counts.insert(def.kind, count.min(CENSUS_ENTRY_LIMIT));
    }
    rows.flags = read_block(
        map,
        &root,
        loc.space,
        BlockKind::NetgameFlags,
        CENSUS_FLAG_LIMIT,
    )?
    .iter()
    .map(|e| FlagRow {
        kind: u16_at(e, 0x10),
        team: u16_at(e, 0x12),
    })
    .collect();
    rows.starts = read_block(
        map,
        &root,
        loc.space,
        BlockKind::StartingLocations,
        CENSUS_ENTRY_LIMIT,
    )?
    .iter()
    .map(|e| u16_at(e, 0x10) as i16)
    .collect();
    rows.counts
        .insert(BlockKind::NetgameFlags, rows.flags.len() as u32);
    rows.counts
        .insert(BlockKind::StartingLocations, rows.starts.len() as u32);
    let mut sounds = std::collections::BTreeSet::new();
    for tag in map.tags() {
        if &tag.group.0 == b"snd!"
            && let Some(name) = tag.name.as_deref()
            && !name.is_empty()
        {
            sounds.insert(name.replace('\\', "/"));
        }
        if let (Some(ty), Some(name)) = (object_type(&tag.group.0), tag.name.as_deref())
            && !name.is_empty()
        {
            rows.tags.insert(name.replace('\\', "/"), ty);
        }
    }
    rows.sounds = Some(sounds);
    Ok(rows)
}
