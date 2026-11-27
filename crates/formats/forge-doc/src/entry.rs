//! Edits of the map's own entries: field writes, removals and resets named
//! by identity, and the print that guards each base entry they touch
//! (docs/game/forge-map-variants.md, "Naming an entry" and "The ops").
//!
//! An entry op names its target as a link does ([`Endpoint`]): a base entry
//! by its index in the base map's block, or the entry a `Placement::Add` op
//! made. A base target carries the entry's [`Print`], a digest of its typed
//! fields as the base map has them, so an adapter can tell that the entry in
//! front of it is the one the edit was made on ([`check_print`]). A document
//! with an entry op or a print is format 3
//! ([`FORMAT_ENTRIES`](crate::FORMAT_ENTRIES)).
use std::collections::BTreeSet;
use std::fmt;

use map::scenario_layout as layout;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    Area, BlockKind, Edit, Endpoint, FieldWrite, ForgeDoc, Limits, MAX_DOC_BYTES, Op, OpId,
    Refusal, Result, Sha256, Value, WorldBounds, games::GameSet, validate, wire,
};

/// The first eight bytes of the SHA-256 of an entry's typed fields
/// ([`print`]), written as sixteen lowercase hexadecimal digits. It is a
/// digest: no value of the map can be read back from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Print(pub [u8; 8]);

impl fmt::Display for Print {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for Print {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Print {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let hex = std::borrow::Cow::<str>::deserialize(d)?;
        if hex.len() != 16
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(de::Error::custom(
                "expected sixteen lowercase hex digits for a print",
            ));
        }
        let mut bytes = [0; 8];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(de::Error::custom)?;
        }
        Ok(Self(bytes))
    }
}

/// The print of one entry of `block`: `fields` are its typed fields as
/// [`Value`]s, one for each field of the layout's
/// (`scenario_layout::BlockDef::fields`) in the layout's order. A tag
/// reference is its tag key and a palette index the key of the palette
/// entry it selects (`Value::Tag(None)` for none), so the print holds no
/// salted ID. The digest is over the canonical JSON of
/// `{"block": <name>, "fields": [...]}`.
pub fn print(block: BlockKind, fields: &[Value]) -> Result<Print> {
    #[derive(Serialize)]
    struct Printed<'a> {
        block: &'static str,
        fields: &'a [Value],
    }

    let def = layout::block(block);
    if fields.len() != def.fields.len() {
        return Err(Refusal::invalid(
            block.name(),
            format!("a print covers the layout's {} fields", def.fields.len()),
        ));
    }
    for (field, value) in def.fields.iter().zip(fields) {
        validate::value(value)?;
        if !validate::typed(field, value) {
            return Err(Refusal::invalid(
                field.name,
                "a print wants the field's own type",
            ));
        }
    }
    let json = wire::canonical(
        &Printed {
            block: block.name(),
            fields,
        },
        "print bytes",
        MAX_DOC_BYTES,
    )?;
    let Sha256(digest) = Sha256::of(json.as_bytes());
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&digest[..8]);
    Ok(Print(bytes))
}

/// The adapter's check before its first write to the base entry at
/// `(block, index)`: the entry's fields as it holds them must give the
/// print the edit carries, or the edit is [`Refusal::Print`].
pub fn check_print(block: BlockKind, index: u32, expected: Print, fields: &[Value]) -> Result<()> {
    if print(block, fields)? == expected {
        Ok(())
    } else {
        Err(Refusal::Print { block, index })
    }
}

/// An edit of one entry, named by identity (`Edit::Entry`). Every base
/// target carries its print.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EntryOp {
    /// Set one typed, editable field of the entry.
    Set { target: Endpoint, write: FieldWrite },
    /// Remove the entry.
    Remove { target: Endpoint },
    /// Drop the effect of every earlier entry op on a base entry: it is in
    /// the map again as the disc has it. Links and labels are their own ops
    /// and are not reset.
    Reset { target: Endpoint },
    /// Set or clear a property of the entry that has no retail field
    /// ([`Property`]): document data with no scenario bytes, as a label
    /// is. It lowers to nothing.
    Property {
        target: Endpoint,
        property: Property,
    },
    /// Set or clear the area of a netgame flag, with no scenario bytes.
    Area {
        target: Endpoint,
        #[serde(deserialize_with = "Option::deserialize")]
        area: Option<Area>,
    },
    /// Show a netgame flag's play form in a match, or hide it, with no
    /// scenario bytes (docs/game/gametype-marker-forms.md, "When a form
    /// shows"). With no op a flag the document added is shown and a flag
    /// of the base map is not ([`shown_flags`](ForgeDoc::shown_flags)).
    Show { target: Endpoint, on: bool },
}

/// A property of an entry the retail layout has no field for
/// (docs/game/forge-map-variants.md, "Properties without a retail
/// field"). A document that sets one is format 3 as any entry op is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Property {
    /// An item spawner's team, `0..8`, or none. A rules script reads it as
    /// it reads a label; no retail game type does.
    Team(Option<u8>),
    /// A weapon spawner's spare clips, `0..=SPARE_CLIPS_MAX`, or none for
    /// the weapon's own reserve. The game writes it to the reserve of each
    /// weapon the spawner makes
    /// (docs/engine/notes/2026-11-27-forge-spawner-rows.md).
    SpareClips(Option<u8>),
    /// A starting point's weight, or none for Normal. The game adds its
    /// bias to the weight the spawn chooser gives the location
    /// (docs/engine/notes/2026-11-27-forge-spawner-rows.md).
    Weight(Option<SpawnWeight>),
    /// The game types an object is in ([`OBJECT_BLOCKS`]), or none for
    /// every game type. A retail object has no game-type field: the
    /// materialiser leaves the placement out of a map built for an engine
    /// the set lacks ([`for_game`]). A set is never empty and never every
    /// engine, which is a clear.
    Games(Option<GameSet>),
}

/// A starting point's weight other than Normal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnWeight {
    Low,
    High,
}

/// How many teams a team property names.
pub const PROPERTY_TEAMS: u8 = 8;

/// The most spare clips a spawner gives its weapon. The engine caps the
/// reserve at the weapon's own maximum all the same.
pub const SPARE_CLIPS_MAX: u8 = 10;

/// The blocks whose entries carry a `games` property: scenery, crates and
/// machines, the placements with no game-type field.
pub const OBJECT_BLOCKS: [BlockKind; 3] =
    [BlockKind::Scenery, BlockKind::Crates, BlockKind::Machines];

impl EntryOp {
    pub fn target(&self) -> Endpoint {
        match self {
            Self::Set { target, .. }
            | Self::Remove { target }
            | Self::Reset { target }
            | Self::Property { target, .. }
            | Self::Area { target, .. }
            | Self::Show { target, .. } => *target,
        }
    }

    pub(crate) fn what(&self) -> &'static str {
        match self {
            Self::Set { .. } => "entry set",
            Self::Remove { .. } => "entry removal",
            Self::Reset { .. } => "entry reset",
            Self::Property { .. } => "entry property",
            Self::Area { .. } => "entry area",
            Self::Show { .. } => "entry show",
        }
    }
}

/// Checks that need only the op: the target is well formed and a base
/// target has its print, `Reset` names a base entry, and a base target's
/// write is to a typed, editable field of its block with a value of the
/// field's type. An added target's field is checked with the document.
pub(crate) fn check_op(edit: &EntryOp, limits: &Limits) -> Result<()> {
    let what = edit.what();
    if let EntryOp::Area { area: Some(area), .. } = edit {
        area.check()?;
    }
    match edit.target() {
        Endpoint::Base {
            block,
            index,
            print,
        } => {
            let def = layout::block(block);
            if index >= validate::block_max(def) {
                return Err(Refusal::invalid(what, "entry index exceeds block limit"));
            }
            if print.is_none() {
                return Err(Refusal::invalid(
                    what,
                    "a base entry is named with its print",
                ));
            }
            match edit {
                EntryOp::Set { write, .. } => validate::placement_field(def, write, limits.world),
                EntryOp::Remove { .. } => removable(block),
                EntryOp::Reset { .. } => Ok(()),
                EntryOp::Property { property, .. } => check_property(block, *property, what),
                EntryOp::Area { .. } => crate::area::check_block(block),
                EntryOp::Show { .. } => check_show_block(block),
            }
        }
        Endpoint::Added(OpId(0)) => Err(Refusal::invalid(what, "want the ID of an add op")),
        Endpoint::Added(_) if matches!(edit, EntryOp::Reset { .. }) => {
            Err(Refusal::invalid(what, "a reset names a base entry"))
        }
        Endpoint::Added(_) => Ok(()),
    }
}

/// The checks of an added target, once the document says its block.
pub(crate) fn check_added(
    edit: &EntryOp,
    block: BlockKind,
    world: Option<WorldBounds>,
) -> Result<()> {
    match edit {
        EntryOp::Set { write, .. } => validate::placement_field(layout::block(block), write, world),
        EntryOp::Remove { .. } => removable(block),
        EntryOp::Reset { .. } => Err(Refusal::invalid(edit.what(), "a reset names a base entry")),
        EntryOp::Property { property, .. } => check_property(block, *property, edit.what()),
        EntryOp::Area { .. } => crate::area::check_block(block),
        EntryOp::Show { .. } => check_show_block(block),
    }
}

/// Shown in play is a netgame flag's: a flag has no body, so its marker is
/// what a player sees.
fn check_show_block(block: BlockKind) -> Result<()> {
    if block == BlockKind::NetgameFlags {
        Ok(())
    } else {
        Err(Refusal::invalid("entry show", "shown in play belongs to a netgame flag"))
    }
}

/// A property is on the kinds of entry that have it, with a value in its
/// range.
fn check_property(block: BlockKind, property: Property, what: &'static str) -> Result<()> {
    match property {
        Property::Team(team) => {
            if block != BlockKind::NetgameEquipment {
                return Err(Refusal::invalid(what, "a team is an item spawner's"));
            }
            if team.is_some_and(|t| t >= PROPERTY_TEAMS) {
                return Err(Refusal::invalid(what, "a team is 0 to 7"));
            }
            Ok(())
        }
        Property::Weight(_) => {
            if block != BlockKind::StartingLocations {
                return Err(Refusal::invalid(what, "a weight is a starting point's"));
            }
            Ok(())
        }
        Property::SpareClips(clips) => {
            if block != BlockKind::NetgameEquipment {
                return Err(Refusal::invalid(what, "spare clips are an item spawner's"));
            }
            if clips.is_some_and(|c| c > SPARE_CLIPS_MAX) {
                return Err(Refusal::invalid(what, "spare clips are 0 to 10"));
            }
            Ok(())
        }
        Property::Games(set) => {
            if !OBJECT_BLOCKS.contains(&block) {
                return Err(Refusal::invalid(
                    what,
                    "game types are a scenery, crate or machine placement's",
                ));
            }
            match set {
                Some(set) if set.is_empty() => {
                    Err(Refusal::invalid(what, "an object needs a game type"))
                }
                Some(set) if set == GameSet::ALL => Err(Refusal::invalid(
                    what,
                    "every game type is no property: clear it",
                )),
                _ => Ok(()),
            }
        }
    }
}

/// The game types each object has by the document's ops, by identity: the
/// last `Games` op of an entry not covered by a later reset, a clear
/// leaving none. An entry absent here is in every game type. A document
/// with any is built for one engine ([`for_game`]) and has the engine in
/// its identity (`MapId::forge_game`).
pub fn object_games(ops: &[Op]) -> std::collections::BTreeMap<Endpoint, GameSet> {
    let covered = covered(ops);
    let mut out = std::collections::BTreeMap::new();
    for op in ops {
        let Edit::Entry(EntryOp::Property {
            target,
            property: Property::Games(set),
        }) = &op.edit
        else {
            continue;
        };
        if covered.contains(&op.id) {
            continue;
        }
        match set {
            Some(set) => out.insert(target.identity(), *set),
            None => out.remove(&target.identity()),
        };
    }
    out
}

/// The document as it is built for the engine at variant index `engine`
/// (docs/game/forge-map-variants.md, "Game types"): each object whose
/// `games` set lacks the engine and is still in the map after every op is
/// removed by an entry op after the last op, and the `Games` ops that set
/// its set, which have no scenario bytes, are left out, so the document
/// never grows. Every other op is as it is. `base_counts` are the map's,
/// as [`crate::resolve`] takes them. The result is for building only: its
/// hash is not the document's.
pub fn for_game(
    doc: &ForgeDoc,
    engine: i32,
    base_counts: &std::collections::BTreeMap<BlockKind, u32>,
) -> Result<ForgeDoc> {
    let games = object_games(&doc.ops);
    let out_of_game: BTreeSet<Endpoint> = games
        .iter()
        .filter(|(_, set)| !set.has(engine))
        .map(|(end, _)| *end)
        .collect();
    if out_of_game.is_empty() {
        return Ok(doc.clone());
    }
    let resolved = crate::resolve(&doc.ops, base_counts)?;
    let covered = covered(&doc.ops);
    // The target as the last uncovered games op names it, print and all.
    let mut named = std::collections::BTreeMap::new();
    let mut ops = Vec::with_capacity(doc.ops.len());
    for op in &doc.ops {
        if let Edit::Entry(EntryOp::Property {
            target,
            property: Property::Games(_),
        }) = &op.edit
            && !covered.contains(&op.id)
            && out_of_game.contains(&target.identity())
        {
            named.insert(target.identity(), *target);
            continue;
        }
        ops.push(op.clone());
    }
    let mut next = doc.ops.iter().map(|op| op.id.0).max().unwrap_or(0);
    for (identity, target) in named {
        if resolved.index(identity).is_none() {
            continue;
        }
        next = next
            .checked_add(1)
            .ok_or_else(|| Refusal::invalid("entry removal", "no operation id is left"))?;
        ops.push(Op {
            id: OpId(next),
            edit: Edit::Entry(EntryOp::Remove { target }),
        });
    }
    let mut built = doc.clone();
    built.ops = ops;
    built.format = built.required_format();
    Ok(built)
}

/// The team each entry has by the document's ops, by identity
/// ([`Endpoint::identity`]): the last `Team` op of an entry not covered by
/// a later reset, a clear leaving none. An entry that is removed is not
/// told apart here: resolve its identity to a final index to find out
/// ([`crate::resolve`]).
pub fn teams(ops: &[Op]) -> std::collections::BTreeMap<Endpoint, u8> {
    last_property(ops, |p| match p {
        Property::Team(team) => Some(*team),
        _ => None,
    })
}

/// The spare clips each spawner has by the document's ops, by identity, as
/// [`teams`] reads teams.
pub fn spare_clips(ops: &[Op]) -> std::collections::BTreeMap<Endpoint, u8> {
    last_property(ops, |p| match p {
        Property::SpareClips(clips) => Some(*clips),
        _ => None,
    })
}

/// The weight each starting point has by the document's ops, by identity,
/// as [`teams`] reads teams; Normal is none.
pub fn spawn_weights(ops: &[Op]) -> std::collections::BTreeMap<Endpoint, SpawnWeight> {
    last_property(ops, |p| match p {
        Property::Weight(weight) => Some(*weight),
        _ => None,
    })
}

/// The value of one property each entry has: the last op `pick` reads (a
/// set, or a clear as `Some(None)`) of an entry not covered by a later
/// reset.
fn last_property<T: Copy>(
    ops: &[Op],
    pick: impl Fn(&Property) -> Option<Option<T>>,
) -> std::collections::BTreeMap<Endpoint, T> {
    let covered = covered(ops);
    let mut out = std::collections::BTreeMap::new();
    for op in ops {
        let Edit::Entry(EntryOp::Property { target, property }) = &op.edit else {
            continue;
        };
        let Some(value) = pick(property) else {
            continue;
        };
        if covered.contains(&op.id) {
            continue;
        }
        match value {
            Some(v) => out.insert(target.identity(), v),
            None => out.remove(&target.identity()),
        };
    }
    out
}

fn removable(block: BlockKind) -> Result<()> {
    match layout::block(block).remove_refusal {
        Some(why) => Err(Refusal::invalid(block.name(), why)),
        None => Ok(()),
    }
}

/// The entry ops a later `Reset` of the same entry covers, and the resets
/// themselves: the ops the fold leaves out. Placement ops by running index
/// are never covered.
pub(crate) fn covered(ops: &[Op]) -> BTreeSet<OpId> {
    let mut covered = BTreeSet::new();
    let mut open: Vec<(Endpoint, OpId)> = Vec::new();
    for op in ops {
        let Edit::Entry(edit) = &op.edit else {
            continue;
        };
        let target = edit.target().identity();
        if let EntryOp::Reset { .. } = edit {
            covered.insert(op.id);
            open.retain(|&(end, id)| {
                if end == target {
                    covered.insert(id);
                    false
                } else {
                    true
                }
            });
        } else {
            open.push((target, op.id));
        }
    }
    covered
}
