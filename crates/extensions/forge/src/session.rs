use std::path::Path;

use forge_doc::{
    ActiveLink, Before, BlockKind, Edit, Entry, F32, FieldWrite, Forge, ForgeDoc, Limits, Link,
    Links, LogHead, Op, Placement, Proposal, Sha256, Value,
};

use crate::{
    Frame, Linkable, OpId, PaletteItem, PlacedObject, Teleporter, World, files, links, scene,
};

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
    head: LogHead,
    log: Vec<Entry>,
    log_bytes: usize,
    last_id: u32,
    // One drop can write both position and rotation at the same tick.
    undo: Vec<usize>,
    redo: Vec<Vec<Op>>,
    // Only set if the engine refused both a load and its compensation.
    repair: Option<Vec<scene::Object>>,
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
            head,
            log: vec![],
            log_bytes: 0,
            last_id: 0,
            undo: vec![],
            redo: vec![],
            repair: None,
        })
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
    pub fn teleporters(&self) -> Vec<Teleporter> {
        let channels = self
            .resolved(&self.doc)
            .map(|l| links::effective(&self.teleporters, &l))
            .unwrap_or_default();
        self.teleporters
            .iter()
            .map(|t| Teleporter {
                channel: channels.get(&t.index).copied().unwrap_or(t.channel),
                ..*t
            })
            .collect()
    }

    /// The active links naming `thing`, oldest first.
    pub fn links_of(&self, thing: Linkable) -> Vec<ActiveLink> {
        let end = thing.endpoint();
        self.resolved(&self.doc)
            .map(|l| l.active)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.link.endpoints().contains(&end))
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

    /// One host-stamped portable op. Local input uses the same admission and
    /// projection as the game API. For a Set, was is the typed old value digest.
    pub fn propose(
        &mut self,
        world: &mut impl World,
        tick: u32,
        op: Op,
        was: Option<Sha256>,
    ) -> Result<Entry, String> {
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
        self.undo.push(ops.len());
        self.redo.clear();
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

    fn holds(&self, op: &Op) -> Result<Option<Sha256>, String> {
        let Edit::Placement(Placement::Set {
            block,
            index,
            write,
        }) = &op.edit
        else {
            return Ok(None);
        };
        let Some(offset) = index.checked_sub(
            *self
                .limits
                .base_counts
                .get(block)
                .ok_or("Unknown placement block.")?,
        ) else {
            return Ok(None);
        };
        let Some(object) = self
            .objects
            .iter()
            .filter(|row| row.item.block == *block)
            .nth(offset as usize)
        else {
            return Ok(None);
        };
        let value = match write.field.as_str() {
            "position" => object.frame.position,
            "rotation" => object.frame.rotation,
            _ => return Ok(None),
        };
        vector(&write.field, value)?
            .value
            .digest()
            .map(Some)
            .map_err(reason)
    }

    fn target(&self, id: OpId) -> Result<(&PlacedObject, u32), String> {
        let offset = self
            .objects
            .iter()
            .position(|o| o.id == id)
            .ok_or("Aim at a Forge object.")?;
        let object = &self.objects[offset];
        let block = object.item.block;
        let index = self.limits.base_counts[&block]
            + self.objects[..offset]
                .iter()
                .filter(|o| o.item.block == block)
                .count() as u32;
        Ok((object, index))
    }

    fn ops(&self, edits: Vec<Edit>) -> Result<Vec<Op>, String> {
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
        let desired = scene::project(doc, &self.limits, &self.palette, self.budget)?;
        let channels = self.channel_changes(doc)?;
        self.restore(world)?;
        let before: Vec<_> = self.objects.iter().map(scene::Object::from).collect();
        let mut result = scene::reconcile(world, &mut self.objects, &desired, respawn);
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
            if let Err(repair_error) = scene::reconcile(world, &mut self.objects, &before, true) {
                self.repair = Some(before);
                return Err(format!(
                    "{error} Restoring the previous objects also failed: {repair_error}. Retry to restore them."
                ));
            }
            return Err(error);
        }
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
