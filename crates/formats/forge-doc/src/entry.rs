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
    BlockKind, Edit, Endpoint, FieldWrite, Limits, MAX_DOC_BYTES, Op, OpId, Refusal, Result,
    Sha256, Value, WorldBounds, validate, wire,
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
}

impl EntryOp {
    pub fn target(&self) -> Endpoint {
        match self {
            Self::Set { target, .. } | Self::Remove { target } | Self::Reset { target } => *target,
        }
    }

    pub(crate) fn what(&self) -> &'static str {
        match self {
            Self::Set { .. } => "entry set",
            Self::Remove { .. } => "entry removal",
            Self::Reset { .. } => "entry reset",
        }
    }
}

/// Checks that need only the op: the target is well formed and a base
/// target has its print, `Reset` names a base entry, and a base target's
/// write is to a typed, editable field of its block with a value of the
/// field's type. An added target's field is checked with the document.
pub(crate) fn check_op(edit: &EntryOp, limits: &Limits) -> Result<()> {
    let what = edit.what();
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
    }
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
