//! The properties a thing shows, by name (docs/game/forge-map-variants.md,
//! "Fields by kind"). Each is a typed, editable field of the layout whose
//! write is proven, on the kinds of thing that have it: how it reads a
//! thing and the one `EntryOp::Set` it writes. A field the design gives a
//! row only once something else is done (the game types, the respawn
//! trigger, spare clips, a weight) is absent until then.
use forge_doc::games::{self, GameSet};
use forge_doc::{BlockKind, EntryOp, FieldWrite, Value};
use map::scenario_layout::{self as layout, FieldDef, Role};

use crate::{Thing, ThingKind};

/// Which field of an entry a property writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// The block's position.
    Position,
    /// The block's rotation (yaw, pitch, roll), or its facing (the yaw).
    Turn,
    /// The layout field of this name.
    Field(&'static str),
}

/// One property row.
#[derive(Debug)]
pub struct Prop {
    /// The name a player and `Session::set_field` use.
    pub name: &'static str,
    /// The kinds of thing that have it.
    pub kinds: &'static [ThingKind],
    source: Source,
}

use ThingKind::{Goal, ItemSpawner, Object, StartingPoint, Teleporter};

const ALL: &[ThingKind] = &[Object, ItemSpawner, StartingPoint, Goal, Teleporter];

/// Every property, in the order a thing shows them.
pub static PROPS: &[Prop] = &[
    Prop {
        name: "position",
        kinds: ALL,
        source: Source::Position,
    },
    Prop {
        name: "rotation",
        kinds: ALL,
        source: Source::Turn,
    },
    // The palette entry the placement makes.
    Prop {
        name: "object",
        kinds: &[Object],
        source: Source::Field("palette index"),
    },
    // A collection or an object tag.
    Prop {
        name: "item",
        kinds: &[ItemSpawner],
        source: Source::Field("item"),
    },
    // 0 is the collection's own.
    Prop {
        name: "respawn time",
        kinds: &[ItemSpawner],
        source: Source::Field("respawn time"),
    },
    // How long an object may be away from its spot.
    Prop {
        name: "return time",
        kinds: &[ItemSpawner],
        source: Source::Field("abandon limit"),
    },
    Prop {
        name: "team",
        kinds: &[StartingPoint, Goal],
        source: Source::Field("team"),
    },
    // The netgame flag's type.
    Prop {
        name: "role",
        kinds: &[Goal],
        source: Source::Field("type"),
    },
    // The territory or flag number. A hill corner's number is its role.
    Prop {
        name: "order",
        kinds: &[Goal],
        source: Source::Field("identifier"),
    },
];

/// The property named `name`, or none for a name the table does not have.
pub fn prop(name: &str) -> Option<&'static Prop> {
    PROPS.iter().find(|p| p.name == name)
}

/// The properties a kind of thing has, in the table's order.
pub fn of_kind(kind: ThingKind) -> impl Iterator<Item = &'static Prop> {
    PROPS.iter().filter(move |p| p.kinds.contains(&kind))
}

/// The properties `thing` shows: its kind's, where its block has the field
/// and the thing holds a value for it.
pub fn of(thing: &Thing) -> impl Iterator<Item = &'static Prop> + '_ {
    of_kind(thing.kind).filter(|p| p.read(thing).is_some())
}

impl Prop {
    /// The layout field this property writes on an entry of `block`, when
    /// the block has it and the layout lets it be edited.
    pub fn field(&self, block: BlockKind) -> Option<&'static FieldDef> {
        let def = layout::block(block);
        let field = match self.source {
            Source::Position => def.role(Role::Position),
            Source::Turn => def.role(Role::Rotation).or_else(|| def.role(Role::Facing)),
            Source::Field(name) => def.field(name),
        }?;
        field.editable.then_some(field)
    }

    /// The value `thing` holds for this property, or none when the thing
    /// does not have it.
    pub fn read<'a>(&self, thing: &'a Thing) -> Option<&'a Value> {
        if !self.kinds.contains(&thing.kind) {
            return None;
        }
        let field = self.field(thing.block)?;
        thing
            .fields
            .iter()
            .find(|(name, _)| *name == field.name)
            .map(|(_, value)| value)
    }

    /// The one op that sets this property of `thing` to `value`, named as
    /// the thing is: an own entry by index and print, an added one by its
    /// add.
    pub fn op(&self, thing: &Thing, value: Value) -> Option<EntryOp> {
        self.read(thing)?;
        Some(EntryOp::Set {
            target: thing.id,
            write: FieldWrite {
                field: self.field(thing.block)?.name.into(),
                value,
            },
        })
    }
}

/// The fields of an item spawner's four game-type slots.
pub const GAME_SLOTS: [&str; 4] = ["game type 1", "game type 2", "game type 3", "game type 4"];

/// The game types of an item spawner, from its four slots: none for another
/// kind of thing, a thing whose block lacks the slots, or a slot the table
/// does not know, which is shown as it is and not edited
/// ([`games::decode`]).
pub fn games(thing: &Thing) -> Option<GameSet> {
    if thing.kind != ThingKind::ItemSpawner {
        return None;
    }
    let slots: Option<Vec<i16>> = GAME_SLOTS
        .iter()
        .map(|name| match thing.fields.iter().find(|(f, _)| f == name) {
            Some((_, Value::Int(v))) => i16::try_from(*v).ok(),
            _ => None,
        })
        .collect();
    games::decode(&slots?)
}

/// The ops that give an item spawner the game types `set`: one
/// `EntryOp::Set` of each slot that changes, none when the set is the
/// one it has. A thing that has no editable game types, and a set the slots
/// cannot name ([`games::encode`]), are refused by their sentence.
pub fn games_ops(thing: &Thing, set: GameSet) -> Result<Vec<EntryOp>, String> {
    let have = games(thing)
        .ok_or_else(|| format!("This {} has no game types to edit.", thing.kind.name()))?;
    let slots = games::encode(set).map_err(|e| e.to_string())?;
    if have == set {
        return Ok(Vec::new());
    }
    let def = layout::block(thing.block);
    let mut ops = Vec::new();
    for (name, slot) in GAME_SLOTS.iter().zip(slots) {
        if !def.field(name).is_some_and(|f| f.editable) {
            return Err(format!(
                "This {} has no game types to edit.",
                thing.kind.name()
            ));
        }
        let value = Value::Int(i64::from(slot));
        let held = thing.fields.iter().find(|(f, _)| f == name).map(|(_, v)| v);
        if held != Some(&value) {
            ops.push(EntryOp::Set {
                target: thing.id,
                write: FieldWrite {
                    field: (*name).into(),
                    value,
                },
            });
        }
    }
    Ok(ops)
}

/// The name of the Properties row that shows a flag's play form in a match,
/// On or Off (docs/game/gametype-marker-forms.md, "Interfaces"). It is no
/// field of the layout: its value is a document fact.
pub const SHOW_IN_PLAY: &str = "Show in play";

/// Whether `thing` has the **Show in play** row: a netgame flag, which has
/// no body of its own, so its marker is what a player sees.
pub fn has_show(thing: &Thing) -> bool {
    thing.block == BlockKind::NetgameFlags
}

/// The op that shows `thing`'s play form in a match (`on`) or hides it,
/// named as the thing is; none for a thing with no such row.
pub fn show_op(thing: &Thing, on: bool) -> Option<EntryOp> {
    has_show(thing).then_some(EntryOp::Show {
        target: thing.id,
        on,
    })
}
