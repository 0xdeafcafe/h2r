//! Live object changes implied by the document. The game's placement indices
//! refer to the loaded file, including after an editor swap-removal. Only
//! salted handles from that map's stream or an acknowledged preview can move
//! or delete objects.

use std::collections::{BTreeMap, BTreeSet};

use agent_proto::{Kind, ObjectId};
use live_mirror::Sample;
use scenario_edit::geometry::{Vec3, rotation};
use scenario_edit::layout::{self, BLOCKS};
use scenario_edit::scenario::PlacementId;
use scenario_edit::{BlockKind, Role, Scenario};

pub(super) mod drive;
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
        frame: Frame,
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
    fn before_sample(&self, sample: &Sample) -> Option<ObjectId> {
        match *self {
            Self::Set { id, tick, .. } | Self::Spawn { id, tick, .. } if sample.tick <= tick => {
                Some(id)
            }
            _ => None,
        }
    }

    fn frame(&self, object: ObjectId) -> Option<Frame> {
        match *self {
            Self::Set { id, frame, .. } | Self::Spawn { id, frame, .. } if id == object => {
                Some(frame)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Op {
    Set { id: ObjectId, frame: Frame },
    Delete { id: ObjectId },
    Respawn,
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

fn occupants(sample: &Sample) -> BTreeMap<Slot, Occupant> {
    let mut slots = BTreeMap::new();
    for object in &sample.objects {
        let (Some(index), Some(kind)) = (object.placement, wire_kind(object.kind)) else {
            continue;
        };
        let Some(def) = BLOCKS
            .iter()
            .find(|b| b.object_type == Some(i16::from(object.kind)))
        else {
            continue;
        };
        let id = ObjectId {
            kind,
            key: object.id,
        };
        let occupant = if object.parent.is_some() {
            Err("the placed object is held or attached")
        } else {
            Ok(id)
        };
        slots
            .entry((def.kind, usize::from(index)))
            .and_modify(|slot| *slot = Err("more than one live object uses the placement"))
            .or_insert(occupant);
    }
    slots
}

/// Compare the desired rows with the file's slots and acknowledged object
/// edits. Physics can move an object between samples; that alone is never an
/// editor move. A release can flush the last frame of a dragged placement.
pub(super) fn diff(
    loaded: &Rows,
    doc: &Rows,
    sample: &Sample,
    ack: &BTreeMap<Key, Ack>,
    release: &BTreeSet<Key>,
) -> Vec<Change> {
    let occupants = occupants(sample);
    let attached: BTreeSet<_> = sample
        .objects
        .iter()
        .filter(|object| object.parent.is_some())
        .map(|object| object.id)
        .chain(sample.players.iter().filter_map(|player| player.held))
        .collect();
    let mut reserved = BTreeMap::new();
    for (&key, row) in loaded {
        reserved.insert((key.block, row.index), key);
    }
    for (&key, reply) in ack {
        if let Some((_, linked, index)) = reply.binding()
            && (!linked || !loaded.contains_key(&key))
        {
            reserved.insert((key.block, index), key);
        }
    }

    let keys: BTreeSet<_> = loaded
        .keys()
        .chain(doc.keys())
        .chain(ack.keys())
        .copied()
        .collect();
    let mut out = Vec::new();
    for key in keys {
        let original = loaded.get(&key);
        let desired = doc.get(&key);
        let reply = ack.get(&key);
        let binding = reply.and_then(Ack::binding);
        let spawned_slot = binding.filter(|(_, linked, _)| !*linked || original.is_none());
        let loaded_slot = original.map(|r| r.index).or(spawned_slot.map(|(_, _, i)| i));
        let occupant = loaded_slot
            .and_then(|i| occupants.get(&(key.block, i)))
            .copied();
        let Some(index) = desired.or(original).map(|r| r.index).or(loaded_slot) else {
            continue;
        };
        let mut change = |index, op| out.push(Change { key, index, op });
        let acknowledged = reply.and_then(|a| {
            a.preview()
                .map(|(id, _)| id)
                .or_else(|| a.before_sample(sample))
        });
        let id = match acknowledged.map(Ok).or(occupant) {
            Some(Err(why)) => {
                if original != desired || release.contains(&key) {
                    change(index, Op::Reload(why));
                }
                continue;
            }
            entry => entry
                .and_then(Result::ok)
                .or(spawned_slot.map(|(id, _, _)| id)),
        };
        if id.is_some_and(|id| attached.contains(&id.key)) {
            if original != desired || release.contains(&key) {
                change(index, Op::Reload("the placed object is held or attached"));
            }
            continue;
        }
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
            if original != desired {
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
            let op = if reserved
                .get(&(key.block, slot))
                .is_some_and(|owner| *owner != key)
            {
                Op::Reload("another placement occupies that slot in the game until reload")
            } else if original.is_none() && occupants.contains_key(&(key.block, slot)) {
                Op::Reload("the requested preview slot already has a live occupant")
            } else if layout::block(key.block)
                .object_type
                .and_then(|t| u8::try_from(t).ok())
                .and_then(wire_kind)
                .is_none()
            {
                Op::Reload("this placement type has no live object edit method")
            } else {
                Op::Respawn
            };
            change(slot, op);
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
    sample: &Sample,
    ack: &BTreeMap<Key, Ack>,
) -> Option<Change> {
    doc.get(&key)?.frame?;
    let binding = ack.get(&key).and_then(Ack::binding);
    let preview = binding.filter(|(_, linked, _)| !*linked || !loaded.contains_key(&key));
    let index = if let Some((_, _, index)) = preview {
        index
    } else {
        let index = loaded.get(&key)?.index;
        occupants(sample).get(&(key.block, index))?.as_ref().ok()?;
        index
    };
    Some(Change {
        key,
        index,
        op: Op::Respawn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::testing::session;
    use agent_proto::live::Object;

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
        assert!(!commands.iter().any(|c| c.op == Op::Respawn));
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
        let frame = loaded[&key].frame.unwrap();
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
                frame,
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
            [Change { key, index, op: Op::Respawn }]
        );
        let id = ObjectId { kind: Kind::Weapon, key: 0xcdef0001 };
        let frame = doc[&key].frame.unwrap();
        let ack = BTreeMap::from([(
            key,
            Ack::Spawn {
                id,
                frame,
                index,
                linked: false,
                tick: sample.tick,
            },
        )]);
        assert!(diff(&loaded, &doc, &sample, &ack, &release).is_empty());
        assert!(session.undo());
        let doc = document(session.scenario().unwrap());
        assert_eq!(
            diff(&loaded, &doc, &sample, &ack, &release),
            [Change { key, index, op: Op::Delete { id } }]
        );
    }
}
