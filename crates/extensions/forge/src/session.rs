use std::path::Path;
use std::sync::Arc;

use forge_doc::{
    ActiveLink, Before, BlockKind, Edit, Endpoint, Entry, EntryOp, F32, FieldWrite, Forge,
    ForgeDoc, Limits, Link, Links, LogHead, Op, Placement, Proposal, Sha256, Value,
};

use crate::things::{self, Kept, Own, OwnMap};
use crate::{
    BaseRows, Frame, Linkable, OpId, PaletteItem, PlacedObject, Teleporter, Thing, ThingKind,
    World, files, links, props, scene,
};

mod shared;

pub use shared::{Outgoing, SHARED_PROPOSE, SHARED_REDO, SessionAdmit};

#[derive(Clone, Debug)]
pub struct Session {
    doc: ForgeDoc,
    author: String,
    limits: Limits,
    palette: Vec<PaletteItem>,
    budget: usize,
    objects: Vec<PlacedObject>,
    /// The loaded map's teleporter flags with their own channels.
    teleporters: Vec<Teleporter>,
    /// The loaded map's rows as the session opened them. Shared by every
    /// candidate clone; never in the document, the log or a save.
    base: Arc<Kept>,
    /// The own entries as the document leaves them.
    own: OwnMap,
    /// The own entries as the world holds them: the document's, but for a
    /// held entry's frame.
    own_live: OwnMap,
    /// The entry held and the tick of its last move: a hold moves at most
    /// once a tick.
    held: Option<(Endpoint, u32)>,
    head: LogHead,
    log: Vec<Entry>,
    log_bytes: usize,
    last_id: u32,
    // One drop can write both position and rotation at the same tick.
    undo: Vec<usize>,
    redo: Vec<Vec<Op>>,
    // Only set if the engine refused both a load and its compensation.
    repair: Option<Vec<scene::Object>>,
    own_repair: Option<OwnMap>,
    /// Set in a synchronous network game ([`Session::share`]): edits queue
    /// for the host instead of applying here.
    shared: Option<shared::Shared>,
}

impl Session {
    pub fn new(
        base: ForgeDoc,
        limits: Limits,
        palette: Vec<PaletteItem>,
        budget: usize,
    ) -> Result<Self, String> {
        if !base.ops.is_empty() {
            return Err("Start with the loaded base map, then load the Forge document.".into());
        }
        if limits.world.is_none() {
            return Err("The map has no BSP world bounds.".into());
        }
        if palette
            .iter()
            .any(|p| !limits.base_counts.contains_key(&p.block))
        {
            return Err("The map is missing a palette block's placement count.".into());
        }
        forge_doc::validate(&base, &limits).map_err(reason)?;
        let head = LogHead::genesis(&base).map_err(reason)?;
        Ok(Self {
            author: base.author.clone(),
            doc: base,
            limits,
            palette,
            budget,
            objects: vec![],
            teleporters: vec![],
            base: Arc::default(),
            own: OwnMap::new(),
            own_live: OwnMap::new(),
            held: None,
            head,
            log: vec![],
            log_bytes: 0,
            last_id: 0,
            undo: vec![],
            redo: vec![],
            repair: None,
            own_repair: None,
            shared: None,
        })
    }

    /// The loaded map's rows, read from the running scenario as the session
    /// opens, before any edit. A block whose rows are not the map's (another
    /// count, a missing row, a print that is not its fields') is left out
    /// by name with the blocks that did not read; its entries are not
    /// things and an edit naming one is refused.
    pub fn with_base(mut self, base: BaseRows) -> Self {
        self.base = Arc::new(Kept::new(base, &self.limits.base_counts));
        self
    }

    /// The blocks of the loaded map left out of the things, each as
    /// `block: reason`.
    pub fn left_out(&self) -> &[String] {
        &self.base.left_out
    }

    /// Every thing of the loaded map: the own entries as the document leaves
    /// them, in the layout's block order and then index order, then the
    /// added ones in scenario order. A removed entry is not listed.
    pub fn things(&self) -> Vec<Thing> {
        let mut out = things::own_things(&self.base, &self.own);
        out.extend(self.added().iter().map(added_thing));
        out
    }

    /// The added objects as the document leaves them.
    fn added(&self) -> Vec<scene::Object> {
        scene::project(&self.doc, &self.limits, &self.palette, self.budget)
            .map(|scene| scene.objects)
            .unwrap_or_default()
    }

    /// The thing `id` names, by identity: an own entry with or without its
    /// print, or an added one.
    pub fn thing(&self, id: Endpoint) -> Option<Thing> {
        match id.identity() {
            Endpoint::Added(op) => self.added().iter().find(|o| o.id == op).map(added_thing),
            Endpoint::Base { block, index, .. } => {
                let row = self.base.row(block, index).ok()?;
                things::own_thing(row, self.own.get(&(block, index)))
            }
        }
    }

    /// The loaded map's teleporter flags, which the Links tool can join.
    pub fn with_teleporters(mut self, teleporters: Vec<Teleporter>) -> Result<Self, String> {
        let count = self
            .limits
            .base_counts
            .get(&BlockKind::NetgameFlags)
            .copied()
            .ok_or("The map is missing its netgame flag count.")?;
        if teleporters.iter().any(|t| t.index >= count) {
            return Err("A teleporter names a missing netgame flag.".into());
        }
        self.teleporters = teleporters;
        Ok(self)
    }

    pub fn document(&self) -> &ForgeDoc {
        &self.doc
    }
    pub fn log(&self) -> &[Entry] {
        &self.log
    }
    pub fn head(&self) -> LogHead {
        self.head
    }
    pub fn objects(&self) -> &[PlacedObject] {
        &self.objects
    }
    pub fn palette(&self) -> &[PaletteItem] {
        &self.palette
    }
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// The map's teleporter flags with the channels the document gives them.
    /// A flag the document removes is not listed.
    pub fn teleporters(&self) -> Vec<Teleporter> {
        let channels = self
            .resolved(&self.doc)
            .map(|l| links::effective(&self.teleporters, &l))
            .unwrap_or_default();
        self.teleporters
            .iter()
            .filter(|t| !self.removed(BlockKind::NetgameFlags, t.index))
            .map(|t| Teleporter {
                channel: channels.get(&t.index).copied().unwrap_or(t.channel),
                ..*t
            })
            .collect()
    }

    fn removed(&self, block: BlockKind, index: u32) -> bool {
        self.own.get(&(block, index)).is_some_and(|o| o.removed)
    }

    /// The active links naming `thing`, oldest first.
    pub fn links_of(&self, thing: Linkable) -> Vec<ActiveLink> {
        self.links_naming(thing.endpoint())
    }

    /// The active links naming the entry `end` names, by identity.
    fn links_naming(&self, end: Endpoint) -> Vec<ActiveLink> {
        let end = end.identity();
        self.resolved(&self.doc)
            .map(|l| l.active)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.link.endpoints().iter().any(|e| e.identity() == end))
            .collect()
    }

    /// Device links reach the game only when the map is materialised again:
    /// the engine entry that sets a live device group is unproven.
    pub fn pending_rebuild(&self) -> usize {
        self.resolved(&self.doc).map_or(0, |l| {
            l.active
                .iter()
                .filter(|a| matches!(a.link, Link::Device { .. }))
                .count()
        })
    }

    fn resolved(&self, doc: &ForgeDoc) -> Result<Links, String> {
        doc.links(&self.limits.base_counts).map_err(reason)
    }

    /// Join two things with one link op. Teleporters take a shared channel;
    /// two machines join one device group.
    pub fn link(
        &mut self,
        world: &mut impl World,
        tick: u32,
        a: Linkable,
        b: Linkable,
    ) -> Result<OpId, String> {
        let current = self.resolved(&self.doc)?;
        let link = links::link(a, b, &self.teleporters, &self.objects, &current)?;
        let ops = self.ops(vec![Edit::Forge(Forge::Link(link))])?;
        let id = ops[0].id;
        self.edit(world, tick, ops, None)?;
        Ok(id)
    }

    /// End the link op `link` made. A teleporter returns to the map's channel.
    pub fn unlink(&mut self, world: &mut impl World, tick: u32, link: OpId) -> Result<(), String> {
        let ops = self.ops(vec![Edit::Forge(Forge::Unlink { link })])?;
        self.edit(world, tick, ops, None)
    }
    pub fn bind_spawn(&mut self, staged: u32, actual: u32) {
        for object in &mut self.objects {
            if object.handle == staged {
                object.handle = actual;
            }
        }
    }

    pub fn place(
        &mut self,
        world: &mut impl World,
        tick: u32,
        item_index: usize,
        frame: Frame,
    ) -> Result<OpId, String> {
        let item = self
            .palette
            .get(item_index)
            .ok_or("Choose a tag from the map palette.")?;
        let fields = vec![
            FieldWrite {
                field: "palette index".into(),
                value: Value::Tag(Some(item.tag.clone())),
            },
            vector("position", frame.position)?,
            vector("rotation", frame.rotation)?,
        ];
        let ops = self.ops(vec![Edit::Placement(Placement::Add {
            block: item.block,
            like: None,
            fields,
        })])?;
        let id = ops[0].id;
        self.edit(world, tick, ops, None)?;
        Ok(id)
    }

    pub fn move_object(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: OpId,
        frame: Frame,
    ) -> Result<(), String> {
        self.restore(world)?;
        let (object, index) = self.target(id)?;
        let mut edits = vec![];
        for (name, old, new) in [
            ("position", object.frame.position, frame.position),
            ("rotation", object.frame.rotation, frame.rotation),
        ] {
            if old.map(f32::to_bits) != new.map(f32::to_bits) {
                edits.push(Edit::Placement(Placement::Set {
                    block: object.item.block,
                    index,
                    write: vector(name, new)?,
                }));
            }
        }
        if edits.is_empty() {
            return Ok(());
        }
        let ops = self.ops(edits)?;
        self.edit(world, tick, ops, None)
    }

    pub fn delete(&mut self, world: &mut impl World, tick: u32, id: OpId) -> Result<(), String> {
        self.restore(world)?;
        let (object, index) = self.target(id)?;
        // A linked entry cannot be removed, so its links end in the same
        // undo step.
        let mut edits: Vec<_> = self
            .links_of(Linkable::Object(id))
            .into_iter()
            .map(|a| Edit::Forge(Forge::Unlink { link: a.id }))
            .collect();
        edits.push(Edit::Placement(Placement::Remove {
            block: object.item.block,
            index,
        }));
        let ops = self.ops(edits)?;
        self.edit(world, tick, ops, None)
    }

    /// Move and turn a thing, own or added, to where a drop leaves it: one
    /// `EntryOp::Set` for its position and one for its rotation or facing,
    /// each only when it changes, in one undo step. An own entry is named by
    /// its print, which the session checks against the row it kept.
    pub fn move_thing(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: Endpoint,
        frame: Frame,
    ) -> Result<(), String> {
        self.restore(world)?;
        let thing = self.thing(id).ok_or(NO_THING)?;
        let mut edits = vec![];
        for write in things::frame_writes(thing.block, frame)? {
            if field(&thing, &write.field) != Some(&write.value) {
                edits.push(Edit::Entry(EntryOp::Set {
                    target: thing.id,
                    write,
                }));
            }
        }
        if edits.is_empty() {
            // A drop where the hold left it: the world may still be there.
            self.held = None;
            return self.settle(world);
        }
        let ops = self.ops(edits)?;
        self.edit(world, tick, ops, None)?;
        self.held = None;
        Ok(())
    }

    /// Carry a thing while it is held: the world moves it, and the document
    /// and the log do not change until the drop ([`Self::move_thing`]). A
    /// hold moves at most once a tick; a second move in the same tick is
    /// dropped.
    pub fn hold(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: Endpoint,
        frame: Frame,
    ) -> Result<(), String> {
        if self.held == Some((id.identity(), tick)) {
            return Ok(());
        }
        self.restore(world)?;
        let thing = self.thing(id).ok_or(NO_THING)?;
        match thing.id {
            Endpoint::Added(op) => {
                scene::in_bounds(&self.limits, frame)?;
                things::frame_writes(thing.block, frame)?;
                let object = self
                    .objects
                    .iter_mut()
                    .find(|o| o.id == op)
                    .ok_or(NO_THING)?;
                if object.frame != frame {
                    world.move_object(object.handle, frame)?;
                    object.frame = frame;
                }
            }
            Endpoint::Base { block, index, .. } => {
                scene::in_bounds(&self.limits, frame)?;
                let row = self.base.row(block, index)?;
                let have = self.own_live.get(&(block, index)).cloned().unwrap_or(Own {
                    fields: row.fields.clone(),
                    removed: false,
                });
                let fields = things::with_frame(block, &have.fields, frame)?;
                if fields != have.fields {
                    world.base_move(block, index, frame)?;
                    if fields == row.fields {
                        self.own_live.remove(&(block, index));
                    } else {
                        self.own_live.insert(
                            (block, index),
                            Own {
                                fields,
                                removed: false,
                            },
                        );
                    }
                }
            }
        }
        self.held = Some((thing.id.identity(), tick));
        Ok(())
    }

    /// Set a property of a thing ([`props`]): one `EntryOp::Set` of the
    /// field it writes. A name the table does not have, or one the thing's
    /// kind does not show, is refused.
    pub fn set_field(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: Endpoint,
        name: &str,
        value: Value,
    ) -> Result<(), String> {
        self.restore(world)?;
        let thing = self.thing(id).ok_or(NO_THING)?;
        let prop = props::prop(name)
            .filter(|p| p.read(&thing).is_some())
            .ok_or_else(|| format!("This {} has no {name}.", thing.kind.name()))?;
        if prop.read(&thing) == Some(&value) {
            return Ok(());
        }
        let op = prop
            .op(&thing, value)
            .ok_or_else(|| format!("This {} has no {name}.", thing.kind.name()))?;
        let ops = self.ops(vec![Edit::Entry(op)])?;
        self.edit(world, tick, ops, None)
    }

    /// Give an item spawner the game types `set` ([`props::games_ops`]):
    /// one `EntryOp::Set` of each slot that changes, in one undo step. A
    /// set it already has writes nothing.
    pub fn set_games(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: Endpoint,
        set: forge_doc::games::GameSet,
    ) -> Result<(), String> {
        self.restore(world)?;
        let thing = self.thing(id).ok_or(NO_THING)?;
        let edits: Vec<_> = props::games_ops(&thing, set)?
            .into_iter()
            .map(Edit::Entry)
            .collect();
        if edits.is_empty() {
            return Ok(());
        }
        let ops = self.ops(edits)?;
        self.edit(world, tick, ops, None)
    }

    /// An item spawner's team, by the document's `Property` ops
    /// ([`forge_doc::teams`]); none when it has none.
    pub fn team(&self, id: Endpoint) -> Option<u8> {
        forge_doc::teams(&self.doc.ops).get(&id.identity()).copied()
    }

    /// Give an item spawner a team, or none: one `EntryOp::Property`, which
    /// writes nothing to the world. A thing that is no spawner is refused
    /// by the sentence of the property's rule.
    pub fn set_team(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: Endpoint,
        team: Option<u8>,
    ) -> Result<(), String> {
        self.restore(world)?;
        let thing = self.thing(id).ok_or(NO_THING)?;
        if thing.kind != ThingKind::ItemSpawner {
            return Err(format!("This {} has no team.", thing.kind.name()));
        }
        if self.team(thing.id) == team {
            return Ok(());
        }
        let ops = self.ops(vec![Edit::Entry(EntryOp::Property {
            target: thing.id,
            property: forge_doc::Property::Team(team),
        })])?;
        self.edit(world, tick, ops, None)
    }

    /// Remove a thing, own or added. A linked entry cannot be removed, so
    /// its links end in the same undo step. In the world an own entry's row
    /// is made inert, and an undo writes the kept row back.
    pub fn remove(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: Endpoint,
    ) -> Result<(), String> {
        self.restore(world)?;
        let thing = self.thing(id).ok_or(NO_THING)?;
        let mut edits: Vec<_> = self
            .links_naming(thing.id)
            .into_iter()
            .map(|a| Edit::Forge(Forge::Unlink { link: a.id }))
            .collect();
        edits.push(Edit::Entry(EntryOp::Remove { target: thing.id }));
        let ops = self.ops(edits)?;
        self.edit(world, tick, ops, None)
    }

    /// Restore original: drop the effect of every entry op on an own entry,
    /// removed or changed, so it is as the map has it. Its links and label
    /// stay.
    pub fn reset(&mut self, world: &mut impl World, tick: u32, id: Endpoint) -> Result<(), String> {
        self.restore(world)?;
        let Endpoint::Base { block, index, .. } = id.identity() else {
            return Err("Only the map's own things can be restored.".into());
        };
        let row = self.base.row(block, index)?;
        if !self.own.contains_key(&(block, index)) {
            return Err("This is already as the map has it.".into());
        }
        let target = Endpoint::Base {
            block,
            index,
            print: Some(row.print),
        };
        let ops = self.ops(vec![Edit::Entry(EntryOp::Reset { target })])?;
        self.edit(world, tick, ops, None)
    }

    /// Put a held thing back where the document has it.
    pub fn release(&mut self, world: &mut impl World) -> Result<(), String> {
        self.held = None;
        self.settle(world)
    }

    /// Bring the world back to the document.
    fn settle(&mut self, world: &mut impl World) -> Result<(), String> {
        let doc = self.doc.clone();
        self.update_world(world, &doc, false)
    }

    /// One host-stamped portable op. Local input uses the same admission and
    /// projection as the game API. For a Set, was is the typed old value digest.
    pub fn propose(
        &mut self,
        world: &mut impl World,
        tick: u32,
        op: Op,
        was: Option<Sha256>,
    ) -> Result<Entry, String> {
        if self.shared.is_some() {
            return Err(SHARED_PROPOSE.into());
        }
        if op.edit == Edit::Undo {
            if self.doc.ops.last().map(|op| op.id) != Some(op.id) {
                return Err("Undo must name the last active edit.".into());
            }
            if was.is_some() {
                return Err("Undo does not accept a field precondition.".into());
            }
            self.retract(world, tick, 1)?;
            if let Some(group) = self.undo.last_mut() {
                *group -= 1;
                if *group == 0 {
                    self.undo.pop();
                }
            }
        } else {
            self.edit(world, tick, vec![op], was)?;
        }
        Ok(self
            .log
            .last()
            .expect("accepted op has a log entry")
            .clone())
    }

    pub fn undo(&mut self, world: &mut impl World, tick: u32) -> Result<(), String> {
        if self.shared.is_some() {
            return self.undo_shared();
        }
        let count = self.undo.last().copied().unwrap_or(1);
        self.retract(world, tick, count)?;
        self.undo.pop();
        Ok(())
    }

    fn retract(&mut self, world: &mut impl World, tick: u32, count: usize) -> Result<(), String> {
        let start = self
            .doc
            .ops
            .len()
            .checked_sub(count)
            .ok_or("There is nothing to undo.")?;
        let group = self.doc.ops[start..].to_vec();
        let inverse: Result<Vec<_>, _> = group
            .iter()
            .rev()
            .map(|op| forge_doc::inverse(op, &Before::capture(&self.doc)))
            .collect();
        self.apply(world, tick, &inverse.map_err(reason)?, None, true)?;
        self.redo.push(group);
        Ok(())
    }

    pub fn redo(&mut self, world: &mut impl World, tick: u32) -> Result<(), String> {
        if self.shared.is_some() {
            return Err(SHARED_REDO.into());
        }
        let ops = self.redo.last().ok_or("There is nothing to redo.")?.clone();
        self.apply(world, tick, &ops, None, true)?;
        self.undo.push(ops.len());
        self.redo.pop();
        Ok(())
    }

    fn edit(
        &mut self,
        world: &mut impl World,
        tick: u32,
        ops: Vec<Op>,
        was: Option<Sha256>,
    ) -> Result<(), String> {
        for op in &ops {
            if self.log.iter().any(|entry| entry.op.id == op.id)
                || self.doc.ops.iter().any(|seen| seen.id == op.id)
                || self.redo.iter().flatten().any(|seen| seen.id == op.id)
            {
                return Err(forge_doc::Refusal::DuplicateOp(op.id).to_string());
            }
        }
        self.apply(world, tick, &ops, was, false)?;
        self.last_id = ops
            .iter()
            .map(|op| op.id.0)
            .max()
            .unwrap_or(0)
            .max(self.last_id);
        if self.shared.is_none() {
            self.undo.push(ops.len());
            self.redo.clear();
        }
        Ok(())
    }

    fn apply(
        &mut self,
        world: &mut impl World,
        tick: u32,
        ops: &[Op],
        was: Option<Sha256>,
        respawn: bool,
    ) -> Result<(), String> {
        if self.shared.is_some() {
            return self.queue(ops, was);
        }
        let max_entries = self.limits.max_log_entries.min(forge_doc::MAX_LOG_ENTRIES);
        if self.log.len() + ops.len() > max_entries {
            return Err("Forge edit log is full.".into());
        }
        self.restore(world)?;
        let mut head = self.head;
        let mut bytes = self.log_bytes;
        let mut entries = Vec::with_capacity(ops.len());
        for op in ops {
            let holds = if was.is_some() { self.holds(op)? } else { None };
            let proposal = Proposal {
                author: self.author.clone(),
                base_seq: head.seq,
                op: op.clone(),
                was,
            };
            let entry = head
                .stamp(tick, proposal, holds, &self.limits)
                .map_err(reason)?;
            let canonical = entry.canonical_json().map_err(reason)?;
            bytes += canonical.len();
            if bytes > forge_doc::MAX_LOG_BYTES {
                return Err("Forge edit log byte limit reached.".into());
            }
            head = LogHead {
                seq: entry.seq,
                tick,
                hash: Sha256::of(canonical.as_bytes()),
            };
            entries.push(entry);
        }
        let (doc, head) =
            forge_doc::fold_from(&self.doc, self.head, &entries, &self.limits).map_err(reason)?;
        self.update_world(world, &doc, respawn)?;
        self.log.extend(entries);
        self.doc = doc;
        self.head = head;
        self.log_bytes = bytes;
        Ok(())
    }

    /// The digest of the value the target of a set holds now, for `was`.
    fn holds(&self, op: &Op) -> Result<Option<Sha256>, String> {
        let value = match &op.edit {
            Edit::Placement(Placement::Set {
                block,
                index,
                write,
            }) => {
                let scene = scene::project(&self.doc, &self.limits, &self.palette, self.budget)?;
                let Some(object) = scene
                    .objects
                    .iter()
                    .find(|o| o.item.block == *block && scene.index.get(&o.id) == Some(index))
                else {
                    return Ok(None);
                };
                let Some((_, value)) = added_fields(object)
                    .into_iter()
                    .find(|(name, _)| *name == write.field)
                else {
                    return Ok(None);
                };
                value
            }
            Edit::Entry(EntryOp::Set { target, write }) => {
                let Some(thing) = self.thing(*target) else {
                    return Ok(None);
                };
                let Some(value) = field(&thing, &write.field) else {
                    return Ok(None);
                };
                value.clone()
            }
            _ => return Ok(None),
        };
        value.digest().map(Some).map_err(reason)
    }

    /// An added object and the index its block has it at.
    fn target(&self, id: OpId) -> Result<(&PlacedObject, u32), String> {
        let object = self
            .objects
            .iter()
            .find(|o| o.id == id)
            .ok_or("Aim at a Forge object.")?;
        let scene = scene::project(&self.doc, &self.limits, &self.palette, self.budget)?;
        let index = *scene.index.get(&id).ok_or("Aim at a Forge object.")?;
        Ok((object, index))
    }

    fn ops(&self, edits: Vec<Edit>) -> Result<Vec<Op>, String> {
        if self.shared.is_some() {
            return self.lane_ops(edits);
        }
        edits
            .into_iter()
            .enumerate()
            .map(|(i, edit)| {
                let id = self
                    .last_id
                    .checked_add(i as u32 + 1)
                    .ok_or("Forge operation IDs exhausted.")?;
                Ok(Op { id: OpId(id), edit })
            })
            .collect()
    }

    fn update_world(
        &mut self,
        world: &mut impl World,
        doc: &ForgeDoc,
        respawn: bool,
    ) -> Result<(), String> {
        let desired = scene::project(doc, &self.limits, &self.palette, self.budget)?.objects;
        let own = things::desired(doc, &self.limits.base_counts, &self.base)?;
        // A removed flag's row is inert: no channel is written to it.
        let channels: Vec<_> = self
            .channel_changes(doc)?
            .into_iter()
            .filter(|&(flag, ..)| {
                !own.get(&(BlockKind::NetgameFlags, flag))
                    .is_some_and(|o| o.removed)
            })
            .collect();
        self.restore(world)?;
        let before: Vec<_> = self.objects.iter().map(scene::Object::from).collect();
        let own_before = self.own_live.clone();
        let mut result = scene::reconcile(world, &mut self.objects, &desired, respawn);
        if result.is_ok() {
            result = things::reconcile(world, &mut self.own_live, &own, &self.base);
        }
        if result.is_ok() {
            for (i, &(flag, new, _)) in channels.iter().enumerate() {
                if let Err(error) = world.channel(flag, new) {
                    // Put back the channels already written.
                    for &(flag, _, old) in channels[..i].iter().rev() {
                        let _ = world.channel(flag, old);
                    }
                    result = Err(error);
                    break;
                }
            }
        }
        if let Err(error) = result {
            let objects_repair = scene::reconcile(world, &mut self.objects, &before, true);
            let own_repair = things::reconcile(world, &mut self.own_live, &own_before, &self.base);
            if objects_repair.is_ok() && own_repair.is_ok() {
                return Err(error);
            }
            let mut message = error;
            if let Err(repair_error) = objects_repair {
                self.repair = Some(before);
                message += &format!(" Restoring the previous objects also failed: {repair_error}.");
            }
            if let Err(repair_error) = own_repair {
                self.own_repair = Some(own_before);
                message +=
                    &format!(" Restoring the map's own entries also failed: {repair_error}.");
            }
            return Err(message + " Retry to restore them.");
        }
        self.own = own;
        Ok(())
    }

    /// Teleporter channels that differ between the current document and
    /// `doc`, as (flag, new, old).
    fn channel_changes(&self, doc: &ForgeDoc) -> Result<Vec<(u32, i16, i16)>, String> {
        let next = self.resolved(doc)?;
        links::check_roles(&self.teleporters, &next)?;
        let old = links::effective(&self.teleporters, &self.resolved(&self.doc)?);
        let new = links::effective(&self.teleporters, &next);
        Ok(new
            .into_iter()
            .filter_map(|(flag, channel)| {
                let was = old.get(&flag).copied().unwrap_or(channel);
                (was != channel).then_some((flag, channel, was))
            })
            .collect())
    }

    fn restore(&mut self, world: &mut impl World) -> Result<(), String> {
        if let Some(repair) = &self.repair {
            scene::reconcile(world, &mut self.objects, repair, true)
                .map_err(|e| format!("Restoring objects after an interrupted load: {e}"))?;
            self.repair = None;
        }
        if let Some(repair) = &self.own_repair {
            things::reconcile(world, &mut self.own_live, repair, &self.base).map_err(|e| {
                format!("Restoring the map's own entries after an interrupted edit: {e}")
            })?;
            self.own_repair = None;
        }
        Ok(())
    }

    /// Allocate a new file without overwriting another session's save.
    pub fn save_new(&self, data_dir: &Path, base: &str) -> Result<String, String> {
        files::save_new(data_dir, base, &self.doc)
    }

    pub fn save(&self, data_dir: &Path, name: &str) -> Result<(), String> {
        files::save(data_dir, name, &self.doc)
    }

    /// Load the folded document as a fresh log checkpoint. Verify the whole
    /// file and project its final objects before asking the game to change.
    pub fn load(
        &mut self,
        world: &mut impl World,
        _tick: u32,
        data_dir: &Path,
        name: &str,
    ) -> Result<(), String> {
        let doc = files::load(data_dir, name, &self.limits)?;
        if doc.base != self.doc.base
            || doc.builder != self.doc.builder
            || doc.recipes != self.doc.recipes
            || doc.imports != self.doc.imports
            || doc.sources != self.doc.sources
        {
            return Err("The document requires another base map, builder or kit.".into());
        }
        let head = LogHead::genesis(&doc).map_err(reason)?;
        if let Some(expect) = doc.expect {
            let actual = forge_doc::MapId::forge(doc.base.sha256, head.hash);
            if actual != expect.map_id {
                return Err(format!(
                    "forge_expect_map_id: document expects {}, computed {actual}",
                    expect.map_id
                ));
            }
        }
        self.update_world(world, &doc, false)?;
        self.held = None;
        self.last_id = doc.ops.iter().map(|op| op.id.0).max().unwrap_or(0);
        self.doc = doc;
        self.head = head;
        self.log.clear();
        self.log_bytes = 0;
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }
}

const NO_THING: &str = "Aim at a thing of the map.";

/// An added object as a thing: its palette entry, position and rotation.
fn added_thing(object: &scene::Object) -> Thing {
    let block = object.item.block;
    Thing {
        id: Endpoint::Added(object.id),
        block,
        kind: ThingKind::of(block, &[]).unwrap_or(ThingKind::Object),
        own: false,
        frame: object.frame,
        fields: added_fields(object),
        changed: true,
    }
}

fn added_fields(object: &scene::Object) -> Vec<(&'static str, Value)> {
    let mut out = vec![("palette index", Value::Tag(Some(object.item.tag.clone())))];
    for (name, v) in [
        ("position", object.frame.position),
        ("rotation", object.frame.rotation),
    ] {
        // A document's frame is finite; the validator refuses any other.
        if let Ok(write) = vector(name, v) {
            out.push((name, write.value));
        }
    }
    out
}

/// The value of a thing's field `name`.
fn field<'a>(thing: &'a Thing, name: &str) -> Option<&'a Value> {
    thing
        .fields
        .iter()
        .find(|(field, _)| *field == name)
        .map(|(_, value)| value)
}

fn vector(name: &str, v: [f32; 3]) -> Result<FieldWrite, String> {
    let [x, y, z] = v.map(F32::new);
    Ok(FieldWrite {
        field: name.into(),
        value: Value::Vec3([x.map_err(reason)?, y.map_err(reason)?, z.map_err(reason)?]),
    })
}

fn reason(e: forge_doc::Refusal) -> String {
    e.to_string()
}
