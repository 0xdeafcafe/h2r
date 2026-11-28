//! Views and immediate operations over Halo 2's native actor and squad data.

use std::collections::{BTreeMap, BTreeSet};

use guest_layout::{
    DatumHandle,
    datum::DatumArray,
    entry::{Cx, EntryError, GuestCall},
    memory::{GuestMemory, MemFault},
    view::objects,
};
use hooks::points;
use hs::h1_ai_slots::ManeuverEdge;

use super::{AI_KIND, NONE, SQUAD_COUNT, SQUAD_SIZE, SQUADS, squads};

pub(super) const ACTORS: u32 = 0x004f_55f0;
pub(super) const AI_GLOBALS: u32 = 0x004f_55d0;

const LISTS: u32 = 0x004f_55d8;
const LIST_NODES: u32 = 0x004f_55d4;

const ACTOR_UNIT: u32 = 0x18;
const ACTOR_SQUAD: u32 = 0x30;
const ACTOR_STARTING_LOCATION: u32 = 0x38;
const ACTOR_ATTACH: u32 = 0x001e_0160;
const ACTOR_MIGRATE: u32 = 0x001e_3400;
const UNIT_DETACH_FROM_PARENT: u32 = 0x000c_c590;
const OBJECT_SET_TRANSFORM: u32 = 0x000b_7430;
const OBJECT_WAKE_PHYSICS: u32 = 0x000b_7360;
const WORLD_VECTOR_TEST: u32 = 0x0016_97c0;
const FRONT_FACING_STRUCTURE: u32 = 0x0080_0001;

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

pub(super) fn by_handle(m: &dyn GuestMemory, handle: DatumHandle) -> R<Option<Actor>> {
    let pool = m.u32(ACTORS)?;
    if pool == 0 {
        return Ok(None);
    }
    DatumArray::at(pool)
        .try_get(m, handle)?
        .map(|at| Actor::read(m, handle, at))
        .transpose()
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

pub(super) fn matches_reference(m: &dyn GuestMemory, actor: Actor, reference: u32) -> R<bool> {
    if reference == NONE {
        return Ok(false);
    }
    match reference & AI_KIND {
        0 => Ok(actor.squad == u32::from(reference as u16)),
        super::AI_GROUP => super::squad_control::contains_squad(m, reference as u16, actor.squad),
        0x8000_0000 => Ok(actor.handle as u16 == reference as u16),
        _ => {
            let squad = (reference >> 16) & 0x3fff;
            if actor.squad != squad {
                return Ok(false);
            }
            let Some(start) = start_for_reference(m, squad, u32::from(reference as u16))? else {
                return Ok(false);
            };
            let name = m.u32(start)?;
            Ok(name != 0 && m.u32(actor.at + ACTOR_STARTING_LOCATION)? == name)
        }
    }
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
    use guest_layout::tags::{GroupTag, tag_get_checked};
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

pub(super) async fn attach_free(cx: &Cx, unit: u32, character: u32) -> Result<(), EntryError> {
    use guest_layout::tags::{GroupTag, tag_get_checked};

    let valid = cx.read(|m| {
        let Some((kind, body)) = objects::get(m, DatumHandle(unit))? else {
            return Ok(false);
        };
        Ok::<_, MemFault>(
            matches!(kind, objects::BIPED | objects::VEHICLE)
                && body.damage_flags(m)? & 4 == 0
                && tag_get_checked(m, DatumHandle(character), GroupTag::from_chars(*b"char"))?
                    .is_some(),
        )
    })?;
    if valid {
        cx.call(
            GuestCall::new(ACTOR_ATTACH)
                .eax(NONE)
                .edx(character)
                .args([unit, 0]),
        )
        .await?;
    }
    Ok(())
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

pub(super) async fn maneuver(cx: &Cx, encoded: u32) -> Result<(), EntryError> {
    if matches!(encoded, 0 | NONE) {
        return Ok(());
    }
    let moves = cx.read(|m| {
        let scnr = m.u32(points::SCENARIO.base)?;
        let count = m.u32(scnr + SQUAD_COUNT)?;
        let encoded = m.cstr(encoded, count as usize * ManeuverEdge::WIDTH)?;
        let Some(routes) = enabled_routes(m, &encoded, count)? else {
            return Ok(Vec::new());
        };
        Ok::<_, MemFault>(
            all(m)?
                .into_iter()
                .filter_map(|actor| routes.get(&actor.squad).map(|&to| (actor, to)))
                .collect::<Vec<_>>(),
        )
    })?;
    for (actor, to) in moves {
        move_actor(cx, actor, to).await?;
    }
    Ok(())
}

fn enabled_routes(
    m: &dyn GuestMemory,
    encoded: &[u8],
    count: u32,
) -> R<Option<BTreeMap<u32, u32>>> {
    let rows = encoded.chunks_exact(ManeuverEdge::WIDTH);
    if !rows.remainder().is_empty() {
        return Ok(None);
    }
    let mut routes = BTreeMap::new();
    for record in rows {
        let Some(route) = ManeuverEdge::decode(record) else {
            return Ok(None);
        };
        let (from, to) = (u32::from(route.from), u32::from(route.to));
        if from >= count || to >= count {
            return Ok(None);
        }
        if let Some(index) = route.enabled_by
            && !super::state::boolean(m, index)?
        {
            continue;
        }
        routes.insert(from, to);
    }
    Ok(Some(routes))
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

async fn supported(cx: &Cx, position: [f32; 3]) -> Result<bool, EntryError> {
    let data = cx.entry().esp.wrapping_sub(24 + 0x68);
    let ray = [position[0], position[1], position[2] + 0.4, 0.0, 0.0, -2.0];
    let args = [FRONT_FACING_STRUCTURE, data, data + 12, NONE, NONE, data + 24]
        .into_iter().chain(ray.into_iter().map(f32::to_bits)).chain([0; 0x68 / 4]);
    Ok(cx.call(GuestCall::new(WORLD_VECTOR_TEST).args(args)).await?.eax as u8 != 0)
}

fn teleportable(m: &dyn GuestMemory, reference: u32) -> R<Vec<(Actor, [f32; 3], bool)>> {
    let mut ready = Vec::new();
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
        ready.push((actor, object.position(m)?, object.parent(m)?.is_some()));
    }
    Ok(ready)
}

pub(super) async fn teleport_reserved(
    cx: &Cx,
    reference: u32,
    unsupported_only: bool,
    pools: &BTreeMap<u32, super::teleport::Pool>,
) -> Result<(), EntryError> {
    for (actor, position, seated) in cx.read(|m| teleportable(m, reference))? {
        let Some(pool) = pools.get(&actor.squad) else {
            continue;
        };
        if unsupported_only && (seated || supported(cx, position).await?) {
            continue;
        }
        let pose = cx.write(|m| {
            let Some(record) = squad_record(m, actor.squad)? else {
                return Ok(None);
            };
            let count = m.u32(record + 0x48)?;
            let Some(index) = pool.reserve(m, count)? else {
                return Ok(None);
            };
            let at = m.u32(record + 0x4c)? + index * 0x64;
            Ok::<_, MemFault>(Some(Teleport {
                unit: actor.unit,
                position: m.vec3(at + 4)?,
                yaw: m.f32(at + 0x14)?,
                pitch: 0.0,
                unseat: seated,
            }))
        })?;
        if let Some(pose) = pose {
            teleport_unit(cx, pose).await?;
        }
    }
    Ok(())
}

fn starting_pose(
    m: &dyn GuestMemory,
    actor: Actor,
    position: [f32; 3],
    seated: bool,
    claimed: &mut BTreeSet<u32>,
) -> R<Option<Teleport>> {
    let Some(record) = squad_record(m, u32::from(actor.squad as u16))? else {
        return Ok(None);
    };
    let own = m.u32(actor.at + ACTOR_STARTING_LOCATION)?;
    let count = m.u32(record + 0x48)?;
    if count == 0 {
        return Ok(None);
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
            return Ok(Some(Teleport {
                unit: actor.unit,
                position,
                yaw,
                pitch,
                unseat: seated,
            }));
        }
    }
    Ok(None)
}

pub(super) async fn teleport(
    cx: &Cx,
    reference: u32,
    unsupported_only: bool,
) -> Result<(), EntryError> {
    let mut claimed = BTreeSet::new();
    for (actor, position, seated) in cx.read(|m| teleportable(m, reference))? {
        if unsupported_only && (seated || supported(cx, position).await?) {
            continue;
        }
        if let Some(pose) = cx.read(|m| starting_pose(m, actor, position, seated, &mut claimed))? {
            teleport_unit(cx, pose).await?;
        }
    }
    Ok(())
}

async fn teleport_unit(cx: &Cx, motion: Teleport) -> Result<(), EntryError> {
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn unsupported_teleport_uses_a_front_facing_bsp_ray() {
        use guest_layout::entry::{EntryFuture, Regs, Ret, testing};

        const STACK: u32 = 0x800100;
        let run = testing::run(
            &|cx| -> EntryFuture {
                Box::pin(async move {
                    Ok(Ret::new(u32::from(supported(&cx, [1.0, 2.0, 3.0]).await?), 0))
                })
            },
            FakeGuest::new(),
            Regs { esp: STACK, ..Default::default() },
            |call, _| {
                assert_eq!(call.target, WORLD_VECTOR_TEST);
                let data = STACK - 24 - 0x68;
                assert_eq!(call.args[..6], [FRONT_FACING_STRUCTURE, data, data + 12, NONE, NONE, data + 24]);
                let ray = call.args[6..12].iter().map(|&bits| f32::from_bits(bits)).collect::<Vec<_>>();
                assert_eq!(ray, [1.0, 2.0, 3.4, 0.0, 0.0, -2.0]);
                Regs { eax: 1, ..Default::default() }
            },
        );
        assert_eq!(run.result, Ok(Ret::new(1, 0)));
    }

    #[test]
    fn teleports_unseat_transform_and_wake_the_same_unit() {
        use guest_layout::entry::{EntryFuture, Regs, Ret, testing};

        const STACK: u32 = 0x800100;
        const UNIT: u32 = 0x8a00_0123;
        let run = testing::run(
            &|cx| -> EntryFuture {
                Box::pin(async move {
                    teleport_unit(&cx, Teleport {
                        unit: UNIT,
                        position: [1.0, 2.0, 3.0],
                        yaw: 0.0,
                        pitch: 0.0,
                        unseat: true,
                    }).await?;
                    Ok(Ret::new(0, 0))
                })
            },
            FakeGuest::new(),
            Regs { esp: STACK, ..Default::default() },
            |call, _| match call.target {
                UNIT_DETACH_FROM_PARENT | OBJECT_WAKE_PHYSICS => {
                    assert_eq!(call.eax, Some(UNIT));
                    Regs::default()
                }
                OBJECT_SET_TRANSFORM => {
                    let pose = STACK - 36;
                    assert_eq!(call.args[..9], [UNIT, pose, pose + 12, pose + 24, 0, 1, 1, 0, 0]);
                    let floats: Vec<_> = call.args[9..].iter().map(|&bits| f32::from_bits(bits)).collect();
                    assert_eq!(floats, [1.0, 2.0, 3.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
                    Regs { eax: 1, ..Default::default() }
                }
                _ => panic!("unexpected native call"),
            },
        );
        assert_eq!(run.calls.iter().map(|call| call.target).collect::<Vec<_>>(), [
            UNIT_DETACH_FROM_PARENT, OBJECT_SET_TRANSFORM, OBJECT_WAKE_PHYSICS,
        ]);
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
    }

    #[test]
    fn maneuver_keeps_cyclic_edges_and_skips_a_disabled_platoon() {
        const GLOBALS: u32 = 0x004f_9380;
        const ARRAY: u32 = 0x800100;
        const DATA: u32 = 0x810000;
        let mut guest = FakeGuest::new();
        guest
            .put_u32(GLOBALS, ARRAY)
            .put_u32(ARRAY + 0x44, DATA)
            .put_u8(DATA + (0x41d + 13) * 8 + 4, 0);
        let routes = b"00000001ffff00010000ffff00020001000d";
        assert_eq!(
            enabled_routes(&guest, routes, 3).unwrap().unwrap(),
            BTreeMap::from([(0, 1), (1, 0)])
        );
    }
}
