//! Views and immediate operations over Halo 2's native actor and squad data.

use std::collections::BTreeSet;

use engine::{
    DatumHandle,
    datum::DatumArray,
    entry::{Cx, EntryError, GuestCall},
    memory::{GuestMemory, MemFault},
    view::{Record, objects},
};
use feature::points;
use live_telemetry::layout as unit;

use super::{AI_KIND, NONE, SQUAD_COUNT, SQUAD_SIZE, SQUADS, squads};

pub(super) const ACTORS: u32 = 0x004f_55f0;
pub(super) const AI_GLOBALS: u32 = 0x004f_55d0;

const LISTS: u32 = 0x004f_55d8;
const LIST_NODES: u32 = 0x004f_55d4;

const ACTOR_UNIT: u32 = 0x18;
const ACTOR_SQUAD: u32 = 0x30;
const ACTOR_STARTING_LOCATION: u32 = 0x38;
const ACTOR_MIGRATE: u32 = 0x001e_3400;
const UNIT_DETACH_FROM_PARENT: u32 = 0x000c_c590;
const OBJECT_SET_TRANSFORM: u32 = 0x000b_7430;
const OBJECT_WAKE_PHYSICS: u32 = 0x000b_7360;

type R<T> = Result<T, MemFault>;

#[derive(Clone, Copy, Debug)]
pub(super) struct Actor {
    pub handle: u32,
    pub at: u32,
    pub unit: u32,
    pub squad: u32,
}

impl Actor {
    fn read(m: &dyn GuestMemory, handle: DatumHandle, at: u32) -> R<Self> {
        Ok(Self {
            handle: handle.raw(),
            at,
            unit: m.u32(at + ACTOR_UNIT)?,
            squad: m.u32(at + ACTOR_SQUAD)?,
        })
    }
}

pub(super) fn active(m: &dyn GuestMemory) -> R<bool> {
    let ai = m.u32(AI_GLOBALS)?;
    Ok(ai != 0 && m.u8(ai + 1)? != 0)
}

pub(super) fn all(m: &dyn GuestMemory) -> R<Vec<Actor>> {
    let base = m.u32(ACTORS)?;
    let mut out = Vec::new();
    if base == 0 {
        return Ok(out);
    }
    let data = DatumArray::at(base);
    let mut next = 0;
    while let Some(index) = data.next_index(m, next)? {
        if let Some(at) = data.get_absolute(m, index)?
            && let Some(handle) = data.handle_of(m, index)?
        {
            out.push(Actor::read(m, handle, at)?);
        }
        next = index + 1;
    }
    Ok(out)
}

pub(super) fn selected(m: &dyn GuestMemory, reference: u32) -> R<Vec<Actor>> {
    if reference == NONE {
        return Ok(Vec::new());
    }
    let kind = reference & AI_KIND;
    if kind == 0x8000_0000 {
        return Ok(all(m)?
            .into_iter()
            .filter(|actor| actor.handle as u16 == reference as u16)
            .collect());
    }
    let squad_ref = if kind == AI_KIND {
        (reference >> 16) & 0x3fff
    } else {
        reference
    };
    let selected: BTreeSet<_> = squads(m, squad_ref)?.into_iter().collect();
    let wanted = if kind == AI_KIND {
        start_for_reference(m, squad_ref, u32::from(reference as u16))?
    } else {
        None
    };
    if kind == AI_KIND && wanted.is_none() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for actor in all(m)? {
        if actor.squad != NONE
            && selected.contains(&u32::from(actor.squad as u16))
            && match wanted {
                Some(start) => {
                    let name = m.u32(start)?;
                    name != 0 && m.u32(actor.at + ACTOR_STARTING_LOCATION)? == name
                }
                None => true,
            }
        {
            out.push(actor);
        }
    }
    Ok(out)
}

pub(super) fn object_list(m: &dyn GuestMemory, list: u32) -> R<BTreeSet<u32>> {
    let mut out = BTreeSet::new();
    if list == NONE {
        return Ok(out);
    }
    let lists = DatumArray::at(m.u32(LISTS)?);
    let Some(header) = lists.try_get(m, DatumHandle(list))? else {
        return Ok(out);
    };
    let list_nodes = DatumArray::at(m.u32(LIST_NODES)?);
    let mut seen = BTreeSet::new();
    let mut next = m.u32(header + 8)?;
    while seen.insert(next)
        && let Some(node) = list_nodes.try_get(m, DatumHandle(next))?
    {
        out.insert(m.u32(node + 4)?);
        next = m.u32(node + 8)?;
    }
    Ok(out)
}

pub(super) fn actors_of_units(m: &dyn GuestMemory, list: u32) -> R<Vec<Actor>> {
    let units = object_list(m, list)?;
    let mut out = Vec::new();
    for actor in all(m)? {
        if units.contains(&actor.unit) {
            out.push(actor);
            continue;
        }
        if let Some((_, body)) = objects::get(m, DatumHandle(actor.unit))?
            && body
                .parent(m)?
                .is_some_and(|object| units.contains(&object.raw()))
        {
            out.push(actor);
        }
    }
    Ok(out)
}

pub(super) fn squad_record(m: &dyn GuestMemory, index: u32) -> R<Option<u32>> {
    let scnr = m.u32(points::SCENARIO.base)?;
    if scnr == 0 || index >= m.u32(scnr + SQUAD_COUNT)? {
        return Ok(None);
    }
    Ok(Some(m.u32(scnr + SQUADS)? + SQUAD_SIZE * index))
}

pub(super) fn migrate_to(m: &dyn GuestMemory, actor: Actor, to: u32) -> R<Option<u32>> {
    let candidates = squads(m, to)?;
    if candidates.len() <= 1 {
        return Ok(candidates.first().copied());
    }
    let character = u32::from(m.u16(actor.at + 4)?);
    let mut first = None;
    for squad in candidates {
        let Some(at) = squad_record(m, squad)? else {
            continue;
        };
        first.get_or_insert(squad);
        // Squad +36 indexes the scenario character palette. Actor +4 is
        // the actor type, so compare through the native character tag below.
        if let Some(kind) = character_type(m, m.u16(at + 0x36)?)?
            && kind == character
        {
            return Ok(Some(squad));
        }
    }
    Ok(first)
}

fn character_type(m: &dyn GuestMemory, index: u16) -> R<Option<u32>> {
    use engine::tags::{GroupTag, tag_get_checked};
    let scnr = m.u32(points::SCENARIO.base)?;
    let index = u32::from(index);
    if index >= m.u32(scnr + 0x178)? {
        return Ok(None);
    }
    let pointer = m.u32(scnr + 0x17c)?;
    let char_id = m.u32(pointer + index * 8 + 4)?;
    let Some(at) = tag_get_checked(m, DatumHandle(char_id), GroupTag::from_chars(*b"char"))? else {
        return Ok(None);
    };
    if m.u32(at + 0x34)? == 0 {
        return Ok(None);
    }
    let general = m.u32(at + 0x38)?;
    Ok(Some(u32::from(m.u16(general + 4)?)))
}

pub(super) async fn move_actor(cx: &Cx, actor: Actor, destination: u32) -> Result<(), EntryError> {
    if actor.squad != destination {
        cx.call(
            GuestCall::new(ACTOR_MIGRATE)
                .eax(actor.handle)
                .args([destination]),
        )
        .await?;
    }
    Ok(())
}

fn start_for_reference(m: &dyn GuestMemory, squad: u32, index: u32) -> R<Option<u32>> {
    let Some(record) = squad_record(m, squad)? else {
        return Ok(None);
    };
    if index >= m.u32(record + 0x48)? {
        return Ok(None);
    }
    Ok(Some(m.u32(record + 0x4c)? + index * 0x64))
}

#[derive(Debug)]
struct Teleport {
    unit: u32,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    unseat: bool,
}

fn teleports(m: &dyn GuestMemory, reference: u32, unsupported_only: bool) -> R<Vec<Teleport>> {
    let mut out = Vec::new();
    let mut claimed = BTreeSet::new();
    for actor in selected(m, reference)? {
        if actor.squad == NONE {
            continue;
        }
        let Some((kind, object)) = objects::get(m, DatumHandle(actor.unit))? else {
            continue;
        };
        if !matches!(kind, objects::BIPED | objects::VEHICLE) || object.damage_flags(m)? & 4 != 0 {
            continue;
        }
        let parent = object.parent(m)?;
        if unsupported_only
            && (kind != objects::BIPED
                || parent.is_some()
                || m.u8(object.addr() + unit::BIPED_SUPPORT_FLAGS)? & unit::BIPED_SUPPORT_COPIED
                    != 0)
        {
            continue;
        }
        let Some(record) = squad_record(m, u32::from(actor.squad as u16))? else {
            continue;
        };
        let position = object.position(m)?;
        let own = m.u32(actor.at + ACTOR_STARTING_LOCATION)?;
        let count = m.u32(record + 0x48)?;
        if count == 0 {
            continue;
        }
        let starts = m.u32(record + 0x4c)?;
        let mut best = None::<(u32, u32, f32, [f32; 3])>;
        for index in 0..count {
            let at = starts + index * 0x64;
            let candidate = m.vec3(at + 4)?;
            let distance: f32 = candidate
                .iter()
                .zip(position)
                .map(|(a, b)| (a - b).powi(2))
                .sum();
            let name = m.u32(at)?;
            let rank = 2 * u32::from(claimed.contains(&at)) + u32::from(name != own || name == 0);
            if distance.is_finite()
                && best
                    .as_ref()
                    .is_none_or(|(_, old, d, _)| rank < *old || (rank == *old && distance < *d))
            {
                best = Some((at, rank, distance, candidate));
            }
        }
        if let Some((at, _, _, position)) = best {
            let yaw = m.f32(at + 0x14)?;
            let pitch = m.f32(at + 0x18)?;
            if yaw.is_finite() && pitch.is_finite() {
                claimed.insert(at);
                out.push(Teleport {
                    unit: actor.unit,
                    position,
                    yaw,
                    pitch,
                    unseat: parent.is_some(),
                });
            }
        }
    }
    Ok(out)
}

pub(super) async fn teleport(
    cx: &Cx,
    reference: u32,
    unsupported_only: bool,
) -> Result<(), EntryError> {
    for motion in cx.read(|m| teleports(m, reference, unsupported_only))? {
        if motion.unseat {
            cx.call(GuestCall::new(UNIT_DETACH_FROM_PARENT).eax(motion.unit))
                .await?;
        }
        let (sy, cy) = motion.yaw.sin_cos();
        let (sp, cp) = motion.pitch.sin_cos();
        let pose = [
            motion.position,
            [cy * cp, sy * cp, sp],
            [-cy * sp, -sy * sp, cp],
        ];
        let base = cx.entry().esp.wrapping_sub(36);
        let args = [motion.unit, base, base + 12, base + 24, 0, 1, 1, 0, 0]
            .into_iter()
            .chain(pose.into_iter().flatten().map(f32::to_bits));
        let result = cx
            .call(GuestCall::new(OBJECT_SET_TRANSFORM).args(args))
            .await?;
        if result.eax as u8 != 0 {
            cx.call(GuestCall::new(OBJECT_WAKE_PHYSICS).eax(motion.unit))
                .await?;
        }
    }
    Ok(())
}
