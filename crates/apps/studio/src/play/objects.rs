//! Live object changes implied by the document. The game's placement indices
//! refer to the loaded file, including after an editor swap-removal. Only
//! salted handles from that map's stream or an acknowledged preview can move
//! or delete objects.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use agent_proto::{Kind, ObjectId};
use live_mirror::Sample;
use scenario_edit::geometry::{Vec3, rotation};
use scenario_edit::layout::{self, BLOCKS};
use scenario_edit::scenario::PlacementId;
use scenario_edit::{BlockKind, Role, Scenario};

pub(super) mod drive;
pub(super) mod previews;
pub(super) use drive::State;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
    pub block: BlockKind,
    pub id: PlacementId,
}

pub(super) fn key(s: &Scenario, block: BlockKind, index: usize) -> Option<Key> {
    Some(Key {
        block,
        id: s.block(block).placement_id(index)?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Frame {
    pub pos: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
}

impl Frame {
    pub(super) fn from_entry(block: BlockKind, bytes: &[u8]) -> Option<Self> {
        let vector = |role| -> Option<Vec3> {
            let at = layout::block(block).role(role)?.offset as usize;
            let mut result = [0.; 3];
            for (component, value) in result.iter_mut().enumerate() {
                let start = at + component * 4;
                *value = f32::from_le_bytes(bytes.get(start..start + 4)?.try_into().ok()?);
            }
            result.iter().all(|v| v.is_finite()).then_some(result)
        };
        let pos = vector(Role::Position)?;
        let [forward, _, up] = rotation(vector(Role::Rotation)?);
        Some(Self { pos, forward, up })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Row {
    pub index: usize,
    pub frame: Option<Frame>,
}

pub(super) type Rows = BTreeMap<Key, Row>;

fn placements(s: &Scenario, source: bool) -> Rows {
    let mut rows = Rows::new();
    for def in BLOCKS.iter().filter(|b| b.object_type.is_some()) {
        let block = s.block(def.kind);
        let len = if source {
            block.capacity()
        } else {
            block.len()
        };
        for index in 0..len {
            let (id, bytes) = if source {
                (
                    PlacementId::Source(index),
                    block.source_entries()[index].as_slice(),
                )
            } else {
                let (Some(id), Some(bytes)) = (block.placement_id(index), block.entry(index)) else {
                    continue;
                };
                (id, bytes)
            };
            let row = Row {
                index,
                frame: Frame::from_entry(def.kind, bytes),
            };
            rows.insert(Key { block: def.kind, id }, row);
        }
    }
    rows
}

pub(super) fn document(s: &Scenario) -> Rows {
    placements(s, false)
}

pub(super) fn source(s: &Scenario) -> Rows {
    placements(s, true)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Ack {
    Set {
        id: ObjectId,
        frame: Frame,
        linked: bool,
        index: usize,
        tick: u64,
    },
    Spawn {
        id: ObjectId,
        linked: bool,
        index: usize,
        tick: u64,
    },
    Delete {
        id: ObjectId,
    },
}

impl Ack {
    fn binding(&self) -> Option<(ObjectId, bool, usize)> {
        match *self {
            Self::Set { id, linked, index, .. } | Self::Spawn { id, linked, index, .. } => {
                Some((id, linked, index))
            }
            Self::Delete { .. } => None,
        }
    }

    fn preview(&self) -> Option<(ObjectId, usize)> {
        let (id, linked, index) = self.binding()?;
        (!linked).then_some((id, index))
    }

    /// The displayed sample can be several game ticks behind a completed
    /// respawn. Its old occupant must not replace the newly returned handle.
    fn before_sample(&self, sample: &SampleIndex) -> Option<ObjectId> {
        match *self {
            Self::Set { id, tick, .. } | Self::Spawn { id, tick, .. } if sample.tick <= tick => {
                Some(id)
            }
            _ => None,
        }
    }

    fn frame(&self, object: ObjectId) -> Option<Frame> {
        match *self {
            Self::Set { id, frame, .. } if id == object => Some(frame),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Op {
    Set { id: ObjectId, frame: Frame },
    Delete { id: ObjectId },
    Respawn { id: Option<ObjectId> },
    Reload(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Change {
    pub key: Key,
    /// The slot in the loaded scenario for a respawn, otherwise the row to show.
    pub index: usize,
    pub op: Op,
}

type Slot = (BlockKind, usize);
type Occupant = Result<ObjectId, &'static str>;

fn wire_kind(kind: u8) -> Option<Kind> {
    match kind {
        1 => Some(Kind::Vehicle),
        7 => Some(Kind::Machine),
        other => Kind::of_object_type(other),
    }
}

/// The loaded document's stable slot owners, reused until its baseline changes.
#[derive(Default)]
pub(super) struct LayoutIndex(BTreeMap<Slot, Key>);

impl LayoutIndex {
    pub(super) fn new(loaded: &Rows) -> Self {
        Self(
            loaded
                .iter()
                .map(|(&key, row)| ((key.block, row.index), key))
                .collect(),
        )
    }

    pub(super) fn key_at(&self, block: BlockKind, index: usize) -> Option<Key> {
        self.0.get(&(block, index)).copied()
    }

    /// Swapping an untouched survivor into another document slot is not a frame edit.
    pub(super) fn changed(&self, loaded: &Rows, doc: &Rows) -> BTreeSet<Key> {
        let modified = loaded.iter().filter_map(|(&key, old)| {
            doc.get(&key)
                .is_none_or(|row| row.frame != old.frame)
                .then_some(key)
        });
        let added = doc.keys().filter(|key| !loaded.contains_key(*key)).copied();
        modified.chain(added).collect()
    }
}

/// Only topology is indexed. A moving body keeps the same placement and salted identity.
#[derive(Default)]
pub(super) struct SampleIndex {
    pub(super) tick: u64,
    observed: bool,
    slots: BTreeMap<Slot, Occupant>,
    seen: HashSet<ObjectId>,
    attached: HashSet<ObjectId>,
}

impl SampleIndex {
    pub(super) fn new(sample: &Sample) -> Self {
        let mut index = Self {
            tick: sample.tick,
            observed: true,
            ..Self::default()
        };
        for object in &sample.objects {
            let Some(kind) = wire_kind(object.kind) else {
                continue;
            };
            let id = ObjectId { kind, key: object.id };
            index.seen.insert(id);
            if object.parent.is_some() {
                index.attached.insert(id);
            }
            let Some(slot) = object.placement else { continue };
            let Some(def) = BLOCKS
                .iter()
                .find(|b| b.object_type == Some(i16::from(object.kind)))
            else {
                continue;
            };
            index.slots
                .entry((def.kind, usize::from(slot)))
                .and_modify(|slot| *slot = Err("more than one live object uses the placement"))
                .or_insert(Ok(id));
        }
        for key in sample.players.iter().filter_map(|player| player.held) {
            let id = ObjectId { kind: Kind::Weapon, key };
            index.seen.insert(id);
            index.attached.insert(id);
        }
        index
    }

    pub(super) fn contains(&self, id: ObjectId) -> bool {
        self.seen.contains(&id)
    }

    pub(super) fn attached(&self, id: ObjectId) -> bool {
        self.attached.contains(&id)
    }

    pub(super) fn occupant(&self, block: BlockKind, index: usize) -> Option<Occupant> {
        self.slots.get(&(block, index)).copied().map(|result| {
            result.and_then(|id| {
                if self.attached(id) {
                    Err("the placed object is held or attached")
                } else {
                    Ok(id)
                }
            })
        })
    }
}

fn occupant(
    key: Key,
    original: Option<&Row>,
    sample: &SampleIndex,
    ack: Option<&Ack>,
) -> Option<Occupant> {
    let acknowledged = ack.and_then(|a| {
        a.preview()
            .map(|(id, _)| id)
            .or_else(|| a.before_sample(sample))
    });
    if let Some(id) = acknowledged {
        return Some(if sample.attached(id) {
            Err("the placed object is held or attached")
        } else {
            Ok(id)
        });
    }
    let slot = original
        .map(|row| row.index)
        .or_else(|| ack.and_then(Ack::binding).map(|(_, _, index)| index))?;
    sample.occupant(key.block, slot)
}

/// Compare the desired rows with the file's slots and acknowledged object
/// edits. Physics can move an object between samples; that alone is never an
/// editor move. A release can flush the last frame of a dragged placement.
pub(super) fn diff(
    loaded: &Rows,
    doc: &Rows,
    layout: &LayoutIndex,
    changed: &BTreeSet<Key>,
    sample: &SampleIndex,
    ack: &BTreeMap<Key, Ack>,
    release: &BTreeSet<Key>,
) -> Vec<Change> {
    let mut reserved = BTreeMap::new();
    for (&key, reply) in ack {
        if let Some((_, linked, index)) = reply.binding()
            && (!linked || !loaded.contains_key(&key))
        {
            reserved.insert((key.block, index), key);
        }
    }
    // Untouched rows have no pending operations, regardless of physics movement.
    let keys: BTreeSet<_> = changed
        .iter()
        .chain(ack.keys())
        .chain(release)
        .copied()
        .collect();
    let mut out = Vec::new();
    for key in keys {
        let original = loaded.get(&key);
        let desired = doc.get(&key);
        let reply = ack.get(&key);
        let Some(index) = desired
            .or(original)
            .map(|r| r.index)
            .or_else(|| reply.and_then(Ack::binding).map(|(_, _, i)| i))
        else {
            continue;
        };
        let mut change = |index, op| out.push(Change { key, index, op });
        if layout::block(key.block)
            .object_type
            .and_then(|t| u8::try_from(t).ok())
            .and_then(wire_kind)
            .is_none()
        {
            if changed.contains(&key) {
                change(index, Op::Reload("this placement type has no live object edit method"));
            }
            continue;
        }
        if !sample.observed {
            if changed.contains(&key) {
                change(index, Op::Reload("live objects are unavailable; reload to apply the placement"));
            }
            continue;
        }
        let id = match occupant(key, original, sample, reply) {
            Some(Err(why)) => {
                if changed.contains(&key) || release.contains(&key) {
                    change(index, Op::Reload(why));
                }
                continue;
            }
            entry => entry.and_then(Result::ok),
        };
        // A by-slot respawn can delete the current occupant. A successful deletion
        // overrides an old sample for edit purposes, but wait for a visibly empty slot.
        let deleted = matches!(reply, Some(Ack::Delete { .. }));
        let current = id.filter(|id| !matches!(reply, Some(Ack::Delete { id: old }) if id == old));
        let Some(row) = desired else {
            if let Some(id) = current {
                change(index, Op::Delete { id });
            }
            continue;
        };
        // A missing frame is still a document row, never a delete.
        let Some(frame) = row.frame else {
            if changed.contains(&key) {
                change(index, Op::Reload("the placement has no finite frame"));
            }
            continue;
        };
        if let Some(id) = current {
            // A replacement may have spawned from edited scenario bytes. Its
            // first move must reach it even when undo restores the file frame.
            let last = match reply {
                Some(ack) => ack.frame(id),
                None => original.and_then(|r| r.frame),
            };
            if last != Some(frame) || release.contains(&key) {
                change(index, Op::Set { id, frame });
            }
        } else if original.is_none() || deleted {
            let slot = original.map_or(index, |r| r.index);
            if let Some(Ack::Delete { id }) = reply
                && sample.contains(*id)
            {
                continue;
            }
            let op = if layout.key_at(key.block, slot).is_some_and(|owner| owner != key)
                || reserved
                    .get(&(key.block, slot))
                    .is_some_and(|owner| *owner != key)
            {
                Op::Reload("another placement occupies that slot in the game until reload")
            } else if sample.occupant(key.block, slot).is_some() {
                Op::Reload("the requested preview slot already has a live occupant")
            } else {
                Op::Respawn { id: None }
            };
            change(slot, op);
        } else if changed.contains(&key) {
            change(
                index,
                Op::Reload("the placement has no live object; changes apply at its next creation"),
            );
        }
    }
    out
}

/// Explicit Respawn now follows an acknowledged creation-field write. A
/// swap-moved row still refers to the slot in the loaded file or its preview.
pub(super) fn respawn(
    loaded: &Rows,
    doc: &Rows,
    key: Key,
    sample: &SampleIndex,
    ack: &BTreeMap<Key, Ack>,
) -> Option<Change> {
    doc.get(&key)?.frame?;
    if !sample.observed {
        return None;
    }
    let original = loaded.get(&key);
    let reply = ack.get(&key);
    let index = reply
        .and_then(Ack::binding)
        .map(|(_, _, index)| index)
        .or_else(|| original.map(|row| row.index))?;
    let id = match occupant(key, original, sample, reply) {
        Some(Ok(id)) if !matches!(reply, Some(Ack::Delete { id: old }) if *old == id) => Some(id),
        None if matches!(reply, Some(Ack::Delete { id }) if !sample.contains(*id)) => None,
        _ => return None,
    };
    Some(Change {
        key,
        index,
        op: Op::Respawn { id },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::testing::session;
    use agent_proto::live::Object;

    fn diff(
        loaded: &Rows,
        doc: &Rows,
        sample: &Sample,
        ack: &BTreeMap<Key, Ack>,
        release: &BTreeSet<Key>,
    ) -> Vec<Change> {
        let layout = LayoutIndex::new(loaded);
        super::diff(
            loaded,
            doc,
            &layout,
            &layout.changed(loaded, doc),
            &SampleIndex::new(sample),
            ack,
            release,
        )
    }

    fn game() -> Sample {
        Sample {
            epoch: 1,
            tick: 1,
            stale: false,
            players: Vec::new(),
            objects: (0u16..2)
                .map(|slot| Object {
                    kind: 2,
                    id: 0x0010_0001 + u32::from(slot),
                    placement: Some(slot),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn swap_removal_does_not_retarget_the_surviving_object() {
        let (_t, mut session) = session("object-slot");
        let block = BlockKind::Weapons;
        let scnr = session.scenario().unwrap();
        let loaded = source(scnr);
        let first = key(scnr, block, 0).unwrap();
        let second = key(scnr, block, 1).unwrap();
        session.remove_entry(block, 0).unwrap();
        let scnr = session.scenario().unwrap();
        assert_eq!(key(scnr, block, 0), Some(second));
        let sample = game();
        let ack = BTreeMap::new();
        let release = BTreeSet::new();
        let removed = ObjectId { kind: Kind::Weapon, key: sample.objects[0].id };
        assert_eq!(
            diff(&loaded, &document(scnr), &sample, &ack, &release),
            [Change { key: first, index: 0, op: Op::Delete { id: removed } }]
        );

        session.add_entry(block, None).unwrap();
        let scnr = session.scenario().unwrap();
        let addition = key(scnr, block, 1).unwrap();
        let commands = diff(&loaded, &document(scnr), &sample, &ack, &release);
        assert!(commands.iter().any(|c| c.key == addition && matches!(c.op, Op::Reload(_))));
        assert!(!commands.iter().any(|c| matches!(c.op, Op::Respawn { .. })));
        assert!(session.undo());
        assert!(session.redo());
        assert_eq!(key(session.scenario().unwrap(), block, 1), Some(addition));
        assert!(session.undo());
        assert!(session.undo());
        assert_eq!(document(session.scenario().unwrap()), loaded);
    }

    #[test]
    fn moving_objects_in_the_stream_does_not_edit_or_reassign_a_placement() {
        let (_t, session) = session("object-occupant");
        let s = session.scenario().unwrap();
        let loaded = source(s);
        let mut doc = document(s);
        let key = key(s, BlockKind::Weapons, 0).unwrap();
        let ack = BTreeMap::new();
        let release = BTreeSet::new();
        let mut sample = game();
        sample.objects[0].position = [30.0, 60.0, 2.0];
        assert!(diff(&loaded, &doc, &sample, &ack, &release).is_empty());

        let frame = doc.get_mut(&key).unwrap().frame.as_mut().unwrap();
        frame.pos[0] += 2.0;
        let mut duplicate = sample.objects[0];
        duplicate.id += 0x0001_0000;
        sample.objects.push(duplicate);
        let changes = diff(&loaded, &doc, &sample, &ack, &release);
        assert!(matches!(changes.as_slice(), [Change { op: Op::Reload(_), .. }]));

        sample.objects.pop();
        sample.players.push(agent_proto::live::Player {
            held: Some(sample.objects[0].id),
            ..Default::default()
        });
        let changes = diff(&loaded, &doc, &sample, &ack, &release);
        assert!(matches!(changes.as_slice(), [Change { op: Op::Reload(_), .. }]));
    }

    #[test]
    fn a_respawn_reply_precedes_the_old_streamed_occupant() {
        let (_t, session) = session("object-respawn-stream");
        let s = session.scenario().unwrap();
        let loaded = source(s);
        let key = key(s, BlockKind::Weapons, 0).unwrap();
        let new_object = ObjectId { kind: Kind::Weapon, key: 0x4100_0001 };
        let mut doc = document(s);
        let wanted = doc.get_mut(&key).unwrap().frame.as_mut().unwrap();
        wanted.pos[0] += 1.;
        let mut sample = game();
        let ack = BTreeMap::from([(
            key,
            Ack::Spawn {
                id: new_object,
                linked: true,
                index: 0,
                tick: sample.tick + 1,
            },
        )]);
        let changed = |sample: &Sample| diff(&loaded, &doc, sample, &ack, &BTreeSet::new());
        assert!(matches!(changed(&sample).as_slice(),
            [Change { op: Op::Set { id, .. }, .. }] if *id == new_object));
        // A later sample can carry an ordinary game-side respawn.
        sample.tick += 3;
        sample.objects[0].id = 0x4500_0001;
        assert!(matches!(changed(&sample).as_slice(),
            [Change { op: Op::Set { id, .. }, .. }] if id.key == sample.objects[0].id));
    }

    #[test]
    fn undo_moves_a_replacement_object_back_to_the_loaded_frame_once() {
        let (_t, mut session) = session("object-undo-new-salt");
        let block = BlockKind::Weapons;
        let scnr = session.scenario().unwrap();
        let loaded = source(scnr);
        let key = key(scnr, block, 0).unwrap();
        let original = loaded[&key].frame.unwrap();
        let mut position = original.pos;
        position[0] += 5.;
        session
            .apply(scenario_edit::Command::set(
                block,
                0,
                "position",
                scenario_edit::Value::Vec3(position),
            ))
            .unwrap();
        let moved = document(session.scenario().unwrap())[&key].frame.unwrap();
        let mut sample = game();
        let old = ObjectId {
            kind: Kind::Weapon,
            key: sample.objects[0].id,
        };
        let receipt = |id, frame, tick| Ack::Set {
            id,
            frame,
            tick,
            linked: true,
            index: 0,
        };
        let mut ack = BTreeMap::from([(key, receipt(old, moved, sample.tick))]);
        sample.tick += 1;
        sample.objects[0].id += 0x0001_0000;
        sample.objects[0].position = moved.pos;
        let replacement = ObjectId {
            key: sample.objects[0].id,
            ..old
        };
        assert!(session.undo());
        let doc = document(session.scenario().unwrap());
        let release = BTreeSet::new();
        assert_eq!(
            diff(&loaded, &doc, &sample, &ack, &release),
            [Change {
                key,
                index: 0,
                op: Op::Set {
                    id: replacement,
                    frame: original,
                },
            }]
        );
        ack.insert(key, receipt(replacement, original, sample.tick));
        assert!(diff(&loaded, &doc, &sample, &ack, &release).is_empty());
        assert!(diff(&loaded, &doc, &sample, &BTreeMap::new(), &release).is_empty());
    }

    #[test]
    fn a_preview_keeps_its_salted_handle_after_its_row_is_removed() {
        let (_t, mut session) = session("object-preview-key");
        let block = BlockKind::Weapons;
        let loaded = document(session.scenario().unwrap());
        session.set_allow_growth(true);
        let index = session.add_entry(block, None).unwrap();
        let scnr = session.scenario().unwrap();
        let key = key(scnr, block, index).unwrap();
        let doc = document(scnr);
        let sample = game();
        let release = BTreeSet::new();
        assert_eq!(
            diff(&loaded, &doc, &sample, &BTreeMap::new(), &release),
            [Change { key, index, op: Op::Respawn { id: None } }]
        );
        let id = ObjectId { kind: Kind::Weapon, key: 0xcdef0001 };
        let frame = doc[&key].frame.unwrap();
        let ack = BTreeMap::from([(
            key,
            Ack::Spawn {
                id,
                index,
                linked: false,
                tick: sample.tick,
            },
        )]);
        assert_eq!(
            diff(&loaded, &doc, &sample, &ack, &release),
            [Change { key, index, op: Op::Set { id, frame } }]
        );
        assert!(session.undo());
        let doc = document(session.scenario().unwrap());
        assert_eq!(
            diff(&loaded, &doc, &sample, &ack, &release),
            [Change { key, index, op: Op::Delete { id } }]
        );
    }
    #[test]
    fn swap_removal_does_not_flag_an_untouched_held_survivor() {
        let (_t, mut session) = session("object-slot-held");
        let block = BlockKind::Weapons;
        let loaded = source(session.scenario().unwrap());
        let removed = key(session.scenario().unwrap(), block, 0).unwrap();
        session.remove_entry(block, 0).unwrap();
        let doc = document(session.scenario().unwrap());
        let mut sample = game();
        sample.objects[1].parent = Some(0x1200_0007);
        assert_eq!(LayoutIndex::new(&loaded).changed(&loaded, &doc), BTreeSet::from([removed]));
        let changes = diff(&loaded, &doc, &sample, &BTreeMap::new(), &BTreeSet::new());
        assert!(matches!(changes.as_slice(), [Change { op: Op::Delete { .. }, .. }]));
    }

    #[test]
    fn a_sound_scenery_move_stays_pending_for_reload() {
        let (_t, session) = session("sound-scenery-frame");
        let weapon = key(session.scenario().unwrap(), BlockKind::Weapons, 0).unwrap();
        let row = source(session.scenario().unwrap())[&weapon];
        let sound = Key { block: BlockKind::SoundScenery, id: PlacementId::Source(0) };
        let loaded = Rows::from([(sound, row)]);
        let mut doc = loaded.clone();
        doc.get_mut(&sound).unwrap().frame.as_mut().unwrap().pos[2] += 1.;
        let mut sample = game();
        sample.objects[0].kind = 10;
        let changes = diff(&loaded, &doc, &sample, &BTreeMap::new(), &BTreeSet::new());
        assert!(matches!(changes.as_slice(),
            [Change { key, op: Op::Reload(why), .. }] if *key == sound && why.contains("placement type")));
    }

    #[test]
    fn a_respawn_targets_the_salted_id_until_deletion_and_an_empty_sample() {
        let (_t, session) = session("object-respawn-by-id");
        let loaded = document(session.scenario().unwrap());
        let key = key(session.scenario().unwrap(), BlockKind::Weapons, 0).unwrap();
        let mut sample = game();
        let old = ObjectId { kind: Kind::Weapon, key: sample.objects[0].id };
        let choose = |sample: &Sample, ack| {
            respawn(&loaded, &loaded, key, &SampleIndex::new(sample), &ack).map(|c| c.op)
        };
        assert_eq!(choose(&sample, BTreeMap::new()), Some(Op::Respawn { id: Some(old) }));
        let ack = BTreeMap::from([(key, Ack::Delete { id: old })]);
        assert_eq!(choose(&sample, ack.clone()), None);
        sample.objects.remove(0);
        assert_eq!(choose(&sample, ack.clone()), Some(Op::Respawn { id: None }));
        sample.players.push(agent_proto::live::Player { held: Some(old.key), ..Default::default() });
        assert_eq!(choose(&sample, ack), None);
    }
    #[test]
    fn an_unobserved_move_explains_why_the_body_cannot_be_sent() {
        let (_t, session) = session("object-no-stream");
        let scenario = session.scenario().unwrap();
        let loaded = document(scenario);
        let key = key(scenario, BlockKind::Weapons, 0).unwrap();
        let mut doc = loaded.clone();
        doc.get_mut(&key).unwrap().frame.as_mut().unwrap().pos[0] += 1.;
        let layout = LayoutIndex::new(&loaded);
        let changes = super::diff(
            &loaded,
            &doc,
            &layout,
            &layout.changed(&loaded, &doc),
            &SampleIndex::default(),
            &BTreeMap::new(),
            &BTreeSet::new(),
        );
        assert!(matches!(
            changes.as_slice(),
            [Change { op: Op::Reload(why), .. }] if why.contains("unavailable")
        ));
    }
}
