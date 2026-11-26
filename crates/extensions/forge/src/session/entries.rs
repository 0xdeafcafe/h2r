//! Kit sessions: entries added to the starting location, netgame flag and
//! netgame equipment blocks (docs/game/forge-editing.md, "The object
//! menu"; docs/game/forge-design.md, "The kit").
//!
//! A session started on a map materialised with the kit ([`Session::with_kit`])
//! has room for a few more entries in each of those blocks. An add is one
//! `Placement::Add` in the document and one `World::kit_add` in the game,
//! which writes the block's next spare row and raises its count; the entry's
//! later sets are `base_move` and `base_field` on that row, and its removal,
//! or the undo of its add, is `kit_drop`. On a map without the kit the same
//! adds are refused by name ([`kit::no_kit`]).
use std::collections::{BTreeMap, BTreeSet};

use forge_doc::{BlockKind, Edit, F32, FieldWrite, Placement, TagKey, Value};
use map::scenario_layout::{self as layout, FieldDef, Role};

use super::{Session, reason, vector};
use crate::{Frame, OpId, World, kit, props, scene, things};

/// One row the kit holds in the running game: an entry the document added.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KitRow {
    pub id: OpId,
    pub block: BlockKind,
    /// The fields the row has written, by layout name. Any other field is
    /// as a zeroed row has it.
    pub set: Vec<(&'static str, Value)>,
}

/// The rows of each block, in the order the document adds them.
pub(crate) type KitRows = BTreeMap<BlockKind, Vec<KitRow>>;

/// What a session knows of the kit of the map it runs on.
#[derive(Clone, Debug)]
pub(crate) struct Kit {
    /// The spare rows each block had when the session opened.
    pub room: BTreeMap<BlockKind, u32>,
    /// The rows the world holds.
    pub live: KitRows,
    /// Set only if the engine refused a call and the call that put the rows
    /// back: the rows the world should hold.
    pub repair: Option<KitRows>,
}

/// The rows a scene's added entries and flags are, in each block's order.
pub(crate) fn rows(scene: &scene::Scene) -> Result<KitRows, String> {
    let mut out = KitRows::new();
    for marker in &scene.markers {
        let block = BlockKind::NetgameFlags;
        let float = |v: f32| F32::new(v).map_err(reason);
        out.entry(block).or_default().push(KitRow {
            id: marker.id,
            block,
            set: vec![
                ("position", vector("position", marker.frame.position)?.value),
                ("facing", Value::Float(float(marker.frame.rotation[0])?)),
                ("type", Value::Int(marker.kind.into())),
                ("team", Value::Int(marker.team.into())),
                ("identifier", Value::Int(marker.identifier.into())),
            ],
        });
    }
    for entry in &scene.entries {
        out.entry(entry.block).or_default().push(KitRow {
            id: entry.id,
            block: entry.block,
            set: entry.set.clone(),
        });
    }
    Ok(out)
}

/// The value of `field` in a row that has written `set`.
fn value_of(set: &[(&'static str, Value)], field: &FieldDef) -> Value {
    set.iter()
        .find(|(name, _)| *name == field.name)
        .map_or_else(|| kit::zero(field), |(_, value)| value.clone())
}

/// Every field of `block` in the layout's order, with the value a row that
/// has written `set` holds.
pub(crate) fn fields_of(
    block: BlockKind,
    set: &[(&'static str, Value)],
) -> Vec<(&'static str, Value)> {
    layout::block(block)
        .fields
        .iter()
        .map(|field| (field.name, value_of(set, field)))
        .collect()
}

fn is_frame(field: &FieldDef) -> bool {
    matches!(field.role, Role::Position | Role::Rotation | Role::Facing)
}

fn record(have: &mut KitRow, field: &FieldDef, value: Value) {
    match have.set.iter_mut().find(|(name, _)| *name == field.name) {
        Some((_, held)) => *held = value,
        None => have.set.push((field.name, value)),
    }
}

/// Make the base calls that take row `have` of `block`, at `index`, to the
/// fields of `want`: one `base_move` for a changed position or turn, then
/// one `base_field` for each other changed field, in the layout's order.
fn edit(
    world: &mut impl World,
    block: BlockKind,
    index: u32,
    have: &mut KitRow,
    want: &KitRow,
) -> Result<(), String> {
    let def = layout::block(block);
    let changed: Vec<&FieldDef> = def
        .fields
        .iter()
        .filter(|f| f.editable && value_of(&have.set, f) != value_of(&want.set, f))
        .collect();
    if changed.iter().any(|f| is_frame(f)) {
        let values: Vec<Value> = def.fields.iter().map(|f| value_of(&want.set, f)).collect();
        world.base_move(block, index, things::frame_of(block, &values))?;
        for field in def.fields.iter().filter(|f| is_frame(f)) {
            record(have, field, value_of(&want.set, field));
        }
    }
    for field in changed.into_iter().filter(|f| !is_frame(f)) {
        let value = value_of(&want.set, field);
        world.base_field(block, index, field.name, &value)?;
        record(have, field, value);
    }
    Ok(())
}

/// Make the kit calls that take the rows the world holds from `live` to
/// `want`, recording each in `live` as it succeeds so a refused call can be
/// compensated from where it stopped. The row of block `b`'s `n`th entry is
/// `base[b] + n`: the rows the map has come first. Per block: the rows past
/// the common prefix of ids are dropped, last first; the rows that stay are
/// edited; the rest of `want` is added in order.
pub(crate) fn reconcile(
    world: &mut impl World,
    live: &mut KitRows,
    want: &KitRows,
    base: &BTreeMap<BlockKind, u32>,
) -> Result<(), String> {
    let blocks: BTreeSet<BlockKind> = live.keys().chain(want.keys()).copied().collect();
    let first = |block: &BlockKind| base.get(block).copied().unwrap_or(0);
    let wanted = |block: &BlockKind| want.get(block).map_or(&[][..], Vec::as_slice);
    for block in &blocks {
        let wanted = wanted(block);
        let have = live.entry(*block).or_default();
        let keep = have.iter().zip(wanted).take_while(|(a, b)| a.id == b.id).count();
        while have.len() > keep {
            world.kit_drop(*block, first(block) + have.len() as u32 - 1)?;
            have.pop();
        }
    }
    for block in &blocks {
        let wanted = wanted(block);
        let have = live.entry(*block).or_default();
        for (i, row) in have.iter_mut().enumerate() {
            edit(world, *block, first(block) + i as u32, row, &wanted[i])?;
        }
    }
    for block in &blocks {
        let wanted = wanted(block);
        let have = live.entry(*block).or_default();
        for row in &wanted[have.len()..] {
            world.kit_add(*block, first(block) + have.len() as u32, &row.set)?;
            have.push(row.clone());
        }
    }
    Ok(())
}

impl Session {
    /// The session runs on a map materialised with the kit
    /// ([`forge_doc::kit`]): `room` is the spare rows each block of the
    /// loaded scenario has past its count. The document takes the kit
    /// recipe, as a session's document has it.
    pub fn with_kit(mut self, room: BTreeMap<BlockKind, u32>) -> Result<Self, String> {
        let doc = forge_doc::session_form(&self.doc);
        forge_doc::validate(&doc, &self.limits).map_err(reason)?;
        self.head = forge_doc::LogHead::genesis(&doc).map_err(reason)?;
        self.doc = doc;
        self.kit = Some(Kit {
            room,
            live: KitRows::new(),
            repair: None,
        });
        // The map with the kit holds the empty document in its session form.
        self.baked = super::reload::Baked::of(&self.rebuild_form()?);
        Ok(self)
    }

    /// Whether the session runs on a map with the kit.
    pub fn has_kit(&self) -> bool {
        self.kit.is_some()
    }

    /// The spare rows each kit block has, as the session opened them; none
    /// on a map without the kit.
    pub fn kit_room(&self) -> Option<BTreeMap<BlockKind, u32>> {
        self.kit.as_ref().map(|kit| kit.room.clone())
    }

    /// The rows of `block` the kit still has room for, or none for a map
    /// without the kit.
    pub fn kit_left(&self, block: BlockKind) -> Option<u32> {
        let kit = self.kit.as_ref()?;
        let room = kit.room.get(&block).copied().unwrap_or(0);
        Some(room.saturating_sub(self.kit_added(block)))
    }

    /// The entries of `block` the document adds.
    fn kit_added(&self, block: BlockKind) -> u32 {
        let Ok(scene) = scene::project(&self.doc, &self.limits, &self.palette, self.budget) else {
            return 0;
        };
        let count = if block == BlockKind::NetgameFlags {
            scene.markers.len()
        } else {
            scene.entries.iter().filter(|e| e.block == block).count()
        };
        count as u32
    }

    /// Whether another entry of `block` can be added: the map has the kit,
    /// the engine's count for the block and the kit's room are not used.
    pub(super) fn kit_check(&self, block: BlockKind) -> Result<(), String> {
        let Some(left) = self.kit_left(block) else {
            return Err(kit::no_kit(block));
        };
        if let Some(budget) = self.budget_for(block)
            && budget.left == 0
        {
            return Err(budget.full());
        }
        if left == 0 {
            return Err(kit::full(block));
        }
        Ok(())
    }

    /// The items a kit session adds as item spawners: the collections the
    /// loaded map's own spawners name, each once, in the order the map
    /// has them. The group each is offered in is [`kit::item_group`].
    pub fn kit_items(&self) -> Vec<TagKey> {
        let mut out: Vec<TagKey> = Vec::new();
        let block = BlockKind::NetgameEquipment;
        let Some(at) = layout::block(block).fields.iter().position(|f| f.name == "item") else {
            return out;
        };
        for row in self.base.rows_of(block) {
            if let Some(Value::Tag(Some(tag))) = row.fields.get(at)
                && !out.contains(tag)
            {
                out.push(tag.clone());
            }
        }
        out
    }

    /// The goals the session's game type reads, which the object menu
    /// offers: a Hill and a Territory are read by their own game type only.
    pub fn goals(&self, game: Option<i32>) -> Vec<crate::Goal> {
        crate::Goal::ALL
            .into_iter()
            .filter(|goal| goal.in_game(game))
            .collect()
    }

    /// Add an entry of `block` with `fields`, in one undo step.
    fn add_entry(
        &mut self,
        world: &mut impl World,
        tick: u32,
        block: BlockKind,
        fields: Vec<FieldWrite>,
    ) -> Result<OpId, String> {
        let ops = self.ops(vec![Edit::Placement(Placement::Add {
            block,
            like: None,
            fields,
        })])?;
        let id = ops[0].id;
        self.edit(world, tick, ops, None)?;
        Ok(id)
    }

    /// Add a starting point for `team` at `frame`, facing its yaw.
    pub fn place_spawn(
        &mut self,
        world: &mut impl World,
        tick: u32,
        team: u8,
        frame: Frame,
    ) -> Result<OpId, String> {
        let block = BlockKind::StartingLocations;
        self.kit_check(block)?;
        let fields = vec![
            vector("position", frame.position)?,
            facing(frame)?,
            int("team", team.into()),
        ];
        self.add_entry(world, tick, block, fields)
    }

    /// Add an item spawner of `item`, one of [`Session::kit_items`], at
    /// `frame`, for every game type.
    pub fn place_item(
        &mut self,
        world: &mut impl World,
        tick: u32,
        item: &TagKey,
        frame: Frame,
    ) -> Result<OpId, String> {
        let block = BlockKind::NetgameEquipment;
        self.kit_check(block)?;
        if !self.kit_items().contains(item) {
            return Err(format!("{} is not an item this map's spawners use.", item.path));
        }
        let slots = forge_doc::games::encode(forge_doc::games::GameSet::ALL).map_err(|e| e.to_string())?;
        let mut fields = vec![
            vector("position", frame.position)?,
            vector("orientation", frame.rotation)?,
            FieldWrite {
                field: "item".into(),
                value: Value::Tag(Some(item.clone())),
            },
        ];
        fields.extend(
            props::GAME_SLOTS
                .iter()
                .zip(slots)
                .map(|(name, slot)| int(name, slot.into())),
        );
        self.add_entry(world, tick, block, fields)
    }

    /// Add a teleporter flag at `frame`: a sender or a receiver. A sender
    /// takes the lowest channel no teleporter uses; a receiver takes the
    /// channel of the latest sender this session added that has none, else
    /// a free one.
    pub fn place_teleporter(
        &mut self,
        world: &mut impl World,
        tick: u32,
        sender: bool,
        frame: Frame,
    ) -> Result<OpId, String> {
        let block = BlockKind::NetgameFlags;
        self.kit_check(block)?;
        let channel = self.teleporter_channel(sender)?;
        let kind = if sender {
            forge_doc::TELEPORTER_SENDER
        } else {
            forge_doc::TELEPORTER_RECEIVER
        };
        let fields = vec![
            vector("position", frame.position)?,
            facing(frame)?,
            int("type", kind.into()),
            int("team", 8),
            int("identifier", channel.into()),
        ];
        self.add_entry(world, tick, block, fields)
    }

    /// The channel the next teleporter flag takes.
    fn teleporter_channel(&self, sender: bool) -> Result<i16, String> {
        let flags: Vec<(u16, i16, bool)> = self
            .things()
            .iter()
            .filter(|t| t.block == BlockKind::NetgameFlags && t.kind == crate::ThingKind::Teleporter)
            .filter_map(|t| {
                let int = |name| {
                    t.fields.iter().find(|(n, _)| *n == name).and_then(|(_, v)| match v {
                        Value::Int(n) => Some(*n),
                        _ => None,
                    })
                };
                Some((
                    u16::try_from(int("type")?).ok()?,
                    i16::try_from(int("identifier")?).ok()?,
                    !t.own,
                ))
            })
            .collect();
        if !sender {
            let waiting = flags.iter().rev().find(|(kind, channel, added)| {
                *added
                    && *kind == forge_doc::TELEPORTER_SENDER
                    && !flags
                        .iter()
                        .any(|(k, c, _)| *k == forge_doc::TELEPORTER_RECEIVER && c == channel)
            });
            if let Some((_, channel, _)) = waiting {
                return Ok(*channel);
            }
        }
        (0..=forge_doc::MAX_CHANNEL as i16)
            .find(|c| !flags.iter().any(|(_, used, _)| used == c))
            .ok_or_else(|| "Every teleporter channel is used.".to_string())
    }

    /// What the kit wants of the world for the entries `scene` has, or none
    /// when the session has no kit: every block's added rows fit its room.
    /// A scene with added starting points or spawners and no kit is refused
    /// by name.
    pub(super) fn kit_want(&self, scene: &scene::Scene) -> Result<Option<KitRows>, String> {
        let Some(kit) = &self.kit else {
            return match scene.entries.first() {
                Some(entry) => Err(kit::no_kit(entry.block)),
                None => Ok(None),
            };
        };
        let want = rows(scene)?;
        for (block, rows) in &want {
            if rows.len() as u32 > kit.room.get(block).copied().unwrap_or(0) {
                return Err(kit::full(*block));
            }
        }
        Ok(Some(want))
    }
}

fn int(name: &str, value: i64) -> FieldWrite {
    FieldWrite {
        field: name.into(),
        value: Value::Int(value),
    }
}

fn facing(frame: Frame) -> Result<FieldWrite, String> {
    Ok(FieldWrite {
        field: "facing".into(),
        value: Value::Float(F32::new(frame.rotation[0]).map_err(reason)?),
    })
}
