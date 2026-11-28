//! Resolve salted world ids and read a fresh, ownerless placement from the
//! running scenario. All reads finish before the first destructive call.

use std::collections::{BTreeMap, BTreeSet};

use agent_proto::sim::REFUSED;
use agent_proto::{Kind, ObjectId, RpcError, code};
use guest_layout::GuestMemory;
use guest_layout::handle::DatumHandle;
use guest_layout::view::{Tags, objects};
use guest_layout::{Object, ObjectHeader, ObjectType};
use live_telemetry::layout;

use super::RespawnTarget;

/// `world.placement.edit`'s block transaction.
pub(super) mod edit;

/// `world.object.respawn {definition}`.
pub(super) mod definition;

fn fault(e: guest_layout::MemFault) -> RpcError {
    RpcError::new(
        code::UNAVAILABLE,
        format!("guest memory fault at {:#x}", e.addr),
    )
}

fn refused(reason: impl Into<String>) -> RpcError {
    RpcError::new(REFUSED, reason)
}

/// Match the reader's biped and held-item rules as well as the slot salt.
pub(super) fn resolve(m: &dyn GuestMemory, id: ObjectId) -> Result<Object, RpcError> {
    let gone = || RpcError::new(code::GONE, format!("{id} is gone"));
    let array = objects::array(m).map_err(fault)?.ok_or_else(gone)?;
    let header = array
        .get(m, DatumHandle(id.key))
        .map_err(fault)?
        .ok_or_else(gone)?;
    let header = ObjectHeader::at(header.addr);
    let body = header.object(m).map_err(fault)?.ok_or_else(gone)?;
    if header.flags(m).map_err(fault)? & guest_layout::object::header::DELETE != 0 {
        return Err(gone());
    }
    let kind = match header.object_type(m).map_err(fault)? {
        Some(ObjectType::Biped) => {
            if body.depleted(m).map_err(fault)? {
                Some(Kind::Body)
            } else {
                let unit = body.as_unit();
                let ai = unit.player(m).map_err(fault)?.is_none()
                    && !unit.actor(m).map_err(fault)?.is_none();
                ai.then_some(Kind::Actor)
            }
        }
        Some(ObjectType::Vehicle) => Some(Kind::Vehicle),
        Some(ObjectType::Machine) => Some(Kind::Machine),
        Some(t) if body.parent(m).map_err(fault)?.is_none() => Kind::of_object_type(t as u8),
        _ => None,
    };
    if kind != Some(id.kind) {
        return Err(gone());
    }
    Ok(body)
}

/// Wire names shared with the editor. The descriptor in tags::object
/// supplies the group's scenario offsets and stride.
const BLOCKS: &[(&str, Kind, ObjectType)] = &[
    ("scenery", Kind::Scenery, ObjectType::Scenery),
    ("vehicles", Kind::Vehicle, ObjectType::Vehicle),
    ("equipment", Kind::Equipment, ObjectType::Equipment),
    ("weapons", Kind::Weapon, ObjectType::Weapon),
    ("machines", Kind::Machine, ObjectType::Machine),
    ("controls", Kind::Control, ObjectType::Control),
    ("crates", Kind::Crate, ObjectType::Crate),
    ("creatures", Kind::Creature, ObjectType::Creature),
];

/// One (count, absolute guest pointer) block. The record and its last byte
/// must be addressable before the caller reads individual fields.
fn record(m: &dyn GuestMemory, header: u32, stride: u32, index: u16) -> Result<u32, RpcError> {
    let count = m.i32(header).map_err(fault)?;
    if i32::from(index) >= count || index > i16::MAX as u16 {
        return Err(refused(format!(
            "index {index} is outside a block of {count} records"
        )));
    }
    let base = m.u32(header + 4).map_err(fault)?;
    base.checked_add(u32::from(index) * stride)
        .filter(|at| base != 0 && at.checked_add(stride - 1).is_some())
        .ok_or_else(|| refused("invalid scenario block pointer"))
}

/// Ownerless previews have no engine placement index. Their salted handles
/// can only identify replacements on the timeline on which we made them.
#[derive(Clone, Debug)]
pub(super) struct Preview {
    /// A salted handle from this timeline, or none after a guest checkpoint.
    current: Option<u32>,
    // A restored checkpoint can contain an older version of this placement.
    tags: BTreeSet<u32>,
}

#[derive(Debug, Default)]
pub(super) struct Previews {
    objects: BTreeMap<(Kind, u16), Preview>,
}

impl Previews {
    fn index(&self, id: ObjectId) -> Option<u16> {
        self.objects.iter().find_map(|(&(kind, index), p)| {
            (kind == id.kind && p.current == Some(id.key)).then_some(index)
        })
    }

    /// Mark a potentially live ownerless object before yielding to OBJECT_NEW.
    /// A dropped continuation cannot tell whether the game created it.
    pub(super) fn creating(&mut self, entry: &Entry) -> Option<Preview> {
        let key = (entry.kind, entry.index);
        let previous = self.objects.get(&key).cloned();
        let p = self.objects.entry(key).or_insert_with(|| Preview {
            current: None,
            tags: BTreeSet::new(),
        });
        p.current = None;
        p.tags.insert(entry.tag);
        previous
    }

    pub(super) fn refused(&mut self, entry: &Entry, previous: Option<Preview>) {
        let key = (entry.kind, entry.index);
        match previous {
            Some(p) => {
                self.objects.insert(key, p);
            }
            None => {
                self.objects.remove(&key);
            }
        }
    }

    pub(super) fn remember(&mut self, entry: &Entry, object: ObjectId) {
        if let Some(p) = self.objects.get_mut(&(entry.kind, entry.index)) {
            p.current = Some(object.key);
        }
    }

    pub(super) fn checkpoint_reverted(&mut self) {
        for preview in self.objects.values_mut() {
            preview.current = None;
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// A placement edit renumbered the block: the previews of deleted
    /// placements are gone, and those of relinked ones (`(from, to)`, old and
    /// new numbering) follow their entry.
    pub(super) fn reindex(&mut self, kind: Kind, deleted: &[u16], relinked: &[(u16, u16)]) {
        for index in deleted {
            self.objects.remove(&(kind, *index));
        }
        let moved: Vec<(u16, Option<Preview>)> = relinked
            .iter()
            .map(|&(from, to)| (to, self.objects.remove(&(kind, from))))
            .collect();
        for (to, preview) in moved {
            if let Some(preview) = preview {
                self.objects.insert((kind, to), preview);
            }
        }
    }

    pub(super) fn deleted(&mut self, id: ObjectId, campaign: bool) {
        if let Some(index) = self.index(id) {
            if campaign {
                // A checkpoint may still hold this preview.
                self.objects.get_mut(&(id.kind, index)).unwrap().current = None;
            } else {
                self.objects.remove(&(id.kind, index));
            }
        }
    }
}

pub(super) struct Entry {
    pub old: Option<ObjectId>,
    pub kind: Kind,
    pub index: u16,
    pub tag: u32,
    pub pos: [f32; 3],
    pub axes: ([f32; 3], [f32; 3]),
}

fn occupant(
    m: &dyn GuestMemory,
    kind: Kind,
    ty: ObjectType,
    index: u16,
    previews: &Previews,
) -> Result<Option<ObjectId>, RpcError> {
    let previous = previews.objects.get(&(kind, index));
    let preview = previous.and_then(|p| p.current);
    let mut old = None;
    for obj in sim_state::objects(m).map_err(fault)? {
        let obj = obj.map_err(fault)?;
        if obj.kind().map_err(fault)? != Some(ty) || obj.flags().map_err(fault)?.deleting {
            continue;
        }
        let id = ObjectId {
            kind,
            key: obj.id().0,
        };
        let place = obj.placement().map_err(fault)?;
        if place == Some(index) || Some(id.key) == preview {
            if obj.parent().map_err(fault)?.is_some() {
                return Err(refused("the placement's object is held or attached"));
            }
            if old.replace(id).is_some() {
                return Err(refused("more than one live object occupies this placement"));
            }
        } else if place.is_none()
            && let Some(stale) = previous.filter(|p| p.current.is_none())
            && stale.tags.contains(&obj.definition().map_err(fault)?.raw())
            && previews.index(id).is_none()
        {
            // A checkpoint can restore an earlier preview. An unlinked object
            // of its tag is ambiguous even when the guest reuses the same salt.
            return Err(refused(
                "an unlinked object could be this placement's preview from before the checkpoint",
            ));
        }
    }
    Ok(old)
}

pub(super) fn read(
    m: &dyn GuestMemory,
    target: &RespawnTarget,
    previews: &Previews,
) -> Result<Entry, RpcError> {
    let (kind, ty, index) = match target {
        RespawnTarget::Object(id) => {
            let body = resolve(m, *id)?;
            let index = u16::try_from(body.placement_index(m).map_err(fault)?)
                .ok()
                .or_else(|| previews.index(*id))
                .ok_or_else(|| {
                    refused(format!("{id} has no scenario placement or current preview"))
                })?;
            let &(_, kind, ty) = BLOCKS
                .iter()
                .find(|b| b.1 == id.kind)
                .ok_or_else(|| refused(format!("{id} has no supported palette block")))?;
            (kind, ty, index)
        }
        RespawnTarget::Placement(place) => {
            let &(_, kind, ty) = BLOCKS.iter().find(|b| b.0 == place.block).ok_or_else(|| {
                refused(format!("{} has no supported object palette and world id", place.block))
            })?;
            (kind, ty, place.index)
        }
    };

    let scnr = m.u32(guest_layout::points::SCENARIO.base).map_err(fault)?;
    let desc = tags::object::OBJECT_TYPES[ty as usize];
    let (placements, palette, stride) = desc
        .placement
        .ok_or_else(|| refused("object type has no scenario placement block"))?;
    let header = |offset: u16| {
        scnr.checked_add(u32::from(offset))
            .filter(|at| scnr != 0 && at.checked_add(4).is_some())
            .ok_or_else(|| refused("no loaded scenario block"))
    };
    let entry = record(m, header(placements)?, u32::from(stride), index)?;
    let palette = record(m, header(palette)?, 0x28, m.u16(entry).map_err(fault)?)?;
    let tag = m.u32(palette + 4).map_err(fault)?;
    let loaded = Tags
        .entry(m, DatumHandle(tag))
        .map_err(fault)?
        .ok_or_else(|| refused("the palette tag is absent from the loaded map"))?;
    let group = loaded.group(m).map_err(fault)?.0;
    let definition = loaded.address(m).map_err(fault)?;
    if group.to_be_bytes().as_slice() != desc.group.as_bytes()
        || m.u32(palette).map_err(fault)? != group
        || definition == 0
        || m.u16(definition).map_err(fault)? != desc.index
    {
        return Err(refused(
            "the palette entry does not name a loaded object of this block's type",
        ));
    }

    // scenario_edit::Scenario::new_entry starts an unnamed placement at
    // name -1 with all flags and type-specific fields zero. The documented
    // ownerless call has no ABI for copying these fields into a live object.
    if m.u16(entry + 2).map_err(fault)? != u16::MAX {
        return Err(refused("the spawn path cannot recreate a named placement"));
    }
    if m.u32(entry + 4).map_err(fault)? != 0 {
        return Err(refused("the spawn path cannot apply scenario placement flags"));
    }
    let tail = m.bytes(entry + 0x24, usize::from(stride) - 0x24).map_err(fault)?;
    if tail.iter().any(|&byte| byte != 0) {
        return Err(refused("the spawn path cannot apply this placement's extra creation fields"));
    }

    let pos = m.vec3(entry + layout::PLACEMENT_POSITION).map_err(fault)?;
    let rotation = m.vec3(entry + layout::PLACEMENT_ROTATION).map_err(fault)?;
    if !pos.iter().chain(&rotation).all(|v| v.is_finite()) {
        return Err(refused("placement position or rotation is not finite"));
    }
    let scale = m.f32(entry + 0x20).map_err(fault)?;
    if scale != 0.0 && scale != 1.0 {
        return Err(refused("the spawn path has no verified field for non-unit placement scale"));
    }
    let (f, u) = live_telemetry::rotation_axes(rotation.map(f64::from));
    let old = occupant(m, kind, ty, index, previews)?;
    if let RespawnTarget::Object(id) = target
        && old != Some(*id)
    {
        return Err(RpcError::new(code::GONE, format!("{id} is gone")));
    }
    Ok(Entry {
        old,
        kind,
        index,
        tag,
        pos,
        axes: (f.map(|v| v as f32), u.map(|v| v as f32)),
    })
}
