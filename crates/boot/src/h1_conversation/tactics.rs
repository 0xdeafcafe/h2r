//! Source perception and behavior changes observed at native actor updates.

use super::{
    cast, dialogue,
    leaders::guest_fault,
    reactions::{self, Queue},
    search, visibility, *,
};

pub(super) const GUARD: u16 = 5;
const COVER_FRIEND: u16 = 11;
const COVER: u16 = 43;
const COVER_PEEK: u16 = 44;
const RETREAT: u16 = 56;
pub(super) const FLEE: u16 = 58;
const COWER: u16 = 59;
const AMBUSH: u16 = 70;
const COORDINATED_AMBUSH: u16 = 71;
const COMBAT_STATUS: u32 = 0x86;
const VISIBLE_ENEMY: u16 = 6;
const FIRST_SIGHT: u16 = 10;
const FIRST_SIGHT_FROM_PERCEPTION: u32 = 0x0025_b391;

const DANGER_KIND: u32 = 0x358;
const DANGER_RELATION: u32 = 0x35a;
const DANGER_OBJECT: u32 = 0x360;
const DANGER_FUSE: u32 = 0x3ac;
const PROJECTILE_DANGER: u16 = 2;
const THROTTLE: u32 = 0x814;
const BODY_RADIUS: u32 = 0x3c;
const BODY_POSITION: u32 = 0x64;
const BODY_FORWARD: u32 = 0x70;
const BODY_VELOCITY: u32 = 0x88;

const GRENADE: u32 = 20;
const BLOCKED: u32 = 32;
const RECORD_SIZE: u32 = 40;
const ACTORS: u32 = hs::h1_conversation::ACTOR_CAPACITY;
const BYTES: u32 = ACTORS * RECORD_SIZE;

pub(super) fn awareness(m: &dyn GuestMemory, actor: u32) -> Result<u16, MemFault> {
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock == 0 {
        return Ok(0);
    }
    let rate = u32::from(m.u16(clock + 2)?);
    if rate == 0 {
        return Ok(0);
    }
    let id = m.u32(actor + cast::ACTOR_UNIT)?;
    let Some(handle) = cast::actor_handle(m, id)? else {
        return Ok(0);
    };
    crate::h1_ai_control::dialogue_awareness(m, handle, rate)
}

fn record(records: u32, actor_id: u32) -> Option<u32> {
    let index = actor_id & 0xffff;
    (index < ACTORS).then_some(records + index * RECORD_SIZE)
}

fn alert_recipient(
    m: &dyn GuestMemory,
    actor: u32,
    unit: u32,
    target: u32,
) -> Result<Option<u32>, MemFault> {
    let Some(source) = cast::Unit::read(m, unit)? else {
        return Ok(None);
    };
    let group = search::encounter(m, actor)?;
    let pool = DatumArray::at(m.u32(cast::ACTORS)?);
    let mut next = 0;
    let mut nearest = 9.0;
    let mut recipient = None;
    while let Some(index) = pool.next_index(m, next)? {
        next = index + 1;
        let Some(other) = pool.get_absolute(m, index)? else {
            continue;
        };
        if actor == other || (group.is_some() && search::encounter(m, other)? != group) {
            continue;
        }
        let buddy = m.u32(other + cast::ACTOR_UNIT)?;
        let Some(ally) = cast::Unit::read(m, buddy)? else {
            continue;
        };
        let distance = cast::distance(source.pos, ally.pos);
        if distance < nearest
            && cast::allied(m, source.team, ally.team)?
            && visibility::in_pvs(m, source.cluster, ally.cluster)?
            && cast::focus_object(m, other)? != target
            && awareness(m, other)? < 3
        {
            nearest = distance;
            recipient = Some(buddy);
        }
    }
    Ok(recipient)
}

fn sight(
    m: &mut dyn GuestMemory,
    queue: Queue,
    records: u32,
    actor_id: u32,
    target: u32,
) -> Result<(), MemFault> {
    let Some(at) = record(records, actor_id) else {
        return Ok(());
    };
    let Some(actor) = datum(m, cast::ACTORS, actor_id)? else {
        return Ok(());
    };
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock == 0 || !reactions::ai_vocalizes(m)? {
        return Ok(());
    }
    let unit = m.u32(actor + cast::ACTOR_UNIT)?;
    let now = m.u32(clock + 8)?;
    let rate = u32::from(m.u16(clock + 2)?);
    let had_combat = m.u32(at)? == actor_id && m.u32(at + 8)? != 0;
    let recent = had_combat && now.wrapping_sub(m.u32(at + 8)?) < rate * 6;
    if recent && dialogue::source_offset_row(m, unit, 0x6e0)?.is_some() {
        queue.push(m, unit, 0x6e0, 5)?;
    } else if dialogue::source_offset_row(m, unit, 0x7e0)?.is_some()
        && let Some(recipient) = alert_recipient(m, actor, unit, target)?
    {
        queue.push_with_other(m, unit, 0x7e0, 4, 0, recipient)?;
    }
    Ok(())
}

fn in_cover(behavior: u16) -> bool {
    matches!(
        behavior,
        COVER | COVER_PEEK | COVER_FRIEND | COWER | AMBUSH | COORDINATED_AMBUSH
    )
}

fn grenade(
    m: &mut dyn GuestMemory,
    queue: Queue,
    record: u32,
    actor: u32,
    unit: u32,
    clock: (u32, u32),
    retreat: bool,
) -> Result<(), MemFault> {
    let (now, rate) = clock;
    if m.u16(actor + DANGER_KIND)? != PROJECTILE_DANGER {
        if m.u32(record)? != NONE {
            m.set_u32(record, NONE)?;
        }
        return Ok(());
    }
    let projectile = m.u32(actor + DANGER_OBJECT)?;
    let relation = m.u16(actor + DANGER_RELATION)?;
    if projectile == NONE || object_address(m, projectile)?.is_none() {
        return Ok(());
    }
    if m.u32(record)? != projectile {
        m.set_u32(record, projectile)?;
        m.set_u32(record + 4, now)?;
        m.set_u32(record + 8, 0)?;
        if relation == 0 && dialogue::source_offset_row(m, unit, 0x760)?.is_some() {
            queue.push(m, unit, 0x760, 3)?;
        }
    }

    if retreat
        && relation == 0
        && dialogue::source_offset_row(m, unit, 0x280)?.is_some()
        && cast::draw_voice(m, 2)? == 0
    {
        queue.push(m, unit, 0x280, 9)?;
    }

    if m.u32(record + 8)? != 0 {
        return Ok(());
    }
    let fuse = m.u16(actor + DANGER_FUSE)?;
    let (offset, priority) = match relation {
        0 if now.wrapping_sub(m.u32(record + 4)?) >= rate.div_ceil(2) => (0x770, 5),
        1 if u32::from(fuse) * 3 < 2 * rate => (0x790, 5),
        2 if u32::from(fuse) < rate => (0x780, 6),
        _ => return Ok(()),
    };
    m.set_u32(record + 8, 1)?;
    if dialogue::source_offset_row(m, unit, offset)?.is_some() {
        queue.push(m, unit, offset, priority)?;
    }
    Ok(())
}

fn blocking_ally(
    m: &dyn GuestMemory,
    speaker: u32,
    position: [f32; 3],
    movement: [f32; 2],
    radius: f32,
) -> Result<u32, MemFault> {
    let Some(team) = cast::team(m, speaker)? else {
        return Ok(NONE);
    };
    let pool = DatumArray::at(m.u32(cast::ACTORS)?);
    let mut next = 0;
    let mut closest = f32::INFINITY;
    let mut blocker = NONE;
    while let Some(index) = pool.next_index(m, next)? {
        next = index + 1;
        let Some(actor) = pool.get_absolute(m, index)? else {
            continue;
        };
        let candidate = m.u32(actor + cast::ACTOR_UNIT)?;
        if candidate == speaker {
            continue;
        }
        let Some(ally) = cast::Unit::read(m, candidate)? else {
            continue;
        };
        let Some(body) = object_address(m, candidate)? else {
            continue;
        };
        let dx = ally.pos[0] - position[0];
        let dy = ally.pos[1] - position[1];
        let along = dx * movement[0] + dy * movement[1];
        let lateral = (dx * movement[1] - dy * movement[0]).abs();
        let width = radius + m.f32(body + BODY_RADIUS)?;
        if along > 0.0
            && along < closest
            && along <= width + 0.3
            && lateral < width
            && (ally.pos[2] - position[2]).abs() < 0.75
            && !cast::mounted(m, body)?
            && cast::allied(m, team, ally.team)?
        {
            closest = along;
            blocker = candidate;
        }
    }
    Ok(blocker)
}

fn blocked(
    m: &mut dyn GuestMemory,
    queue: Queue,
    record: u32,
    actor: u32,
    unit: u32,
    clock: (u32, u32),
) -> Result<(), MemFault> {
    if dialogue::source_offset_row(m, unit, 0x820)?.is_none() {
        return Ok(());
    }
    let Some(body) = object_address(m, unit)? else {
        return Ok(());
    };
    let [forward, sidestep, _] = m.vec3(actor + THROTTLE)?;
    let vx = m.f32(body + BODY_VELOCITY)?;
    let vy = m.f32(body + BODY_VELOCITY + 4)?;
    let (now, rate) = clock;
    let effort = (forward * forward + sidestep * sidestep).sqrt();
    if effort <= 0.1
        || (vx * vx + vy * vy) > 0.0025
        || cast::mounted(m, body)?
        || awareness(m, actor)? != 7
    {
        m.set_u32(record, 0)?;
        m.set_u32(record + 4, 0)?;
        return Ok(());
    }
    let started = m.u32(record)?;
    if started == 0 {
        m.set_u32(record, now.max(1))?;
    } else if m.u32(record + 4)? == 0 && now.wrapping_sub(started) >= rate * 3 / 2 {
        let [x, y, _] = m.vec3(body + BODY_FORWARD)?;
        let movement = [
            (forward * x - sidestep * y) / effort,
            (forward * y + sidestep * x) / effort,
        ];
        let origin = m.vec3(body + BODY_POSITION)?;
        let radius = m.f32(body + BODY_RADIUS)?;
        let other = blocking_ally(m, unit, origin, movement, radius)?;
        if other == NONE {
            m.set_u32(record, now.max(1))?;
        } else {
            queue.push_with_other(m, unit, 0x820, 3, 0, other)?;
            m.set_u32(record + 4, 1)?;
        }
    }
    Ok(())
}

fn observe(
    m: &mut dyn GuestMemory,
    queue: Queue,
    records: u32,
    actor_id: u32,
) -> Result<(), MemFault> {
    let Some(at) = record(records, actor_id) else {
        return Ok(());
    };
    let Some(actor) = datum(m, cast::ACTORS, actor_id)? else {
        return Ok(());
    };
    let unit = m.u32(actor + cast::ACTOR_UNIT)?;
    let Some(speech) = dialogue::speech(m, unit)? else {
        return Ok(());
    };
    if !dialogue::converted_bank(m, m.u32(speech)?)? || !reactions::ai_vocalizes(m)? {
        return Ok(());
    }
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock == 0 {
        return Ok(());
    }
    let now = m.u32(clock + 8)?;
    let rate = u32::from(m.u16(clock + 2)?);
    if rate == 0 {
        return Ok(());
    }
    let behavior = search::behavior(m, actor)?;
    let status = m.u16(actor + COMBAT_STATUS)?;
    let target = cast::focus_object(m, actor)?;
    let mut old_behavior = behavior;
    let mut old_status = status;
    let mut old_target = target;
    if m.u32(at)? == actor_id {
        old_behavior = m.u16(at + 4)?;
        old_status = m.u16(at + 6)?;
        old_target = m.u32(at + 12)?;
    } else {
        m.fill(at, RECORD_SIZE, 0)?;
        m.set_u32(at, actor_id)?;
    }
    m.set_u16(at + 4, behavior)?;
    m.set_u16(at + 6, status)?;
    m.set_u32(at + 12, target)?;
    if status >= VISIBLE_ENEMY {
        m.set_u32(at + 8, now.max(1))?;
    }
    let searching = matches!(
        behavior,
        search::PRESEARCH | search::SEARCH | search::INVESTIGATE | search::POSTSEARCH | GUARD
    );
    let changed = old_behavior != behavior;
    grenade(
        m,
        queue,
        at + GRENADE,
        actor,
        unit,
        (now, rate),
        changed && matches!(behavior, RETREAT | FLEE),
    )?;
    if status >= VISIBLE_ENEMY {
        blocked(m, queue, at + BLOCKED, actor, unit, (now, rate))?;
    } else if m.u32(at + BLOCKED)? != 0 {
        m.set_u32(at + BLOCKED, 0)?;
        m.set_u32(at + BLOCKED + 4, 0)?;
    }
    let lost =
        old_status >= VISIBLE_ENEMY && status < VISIBLE_ENEMY && old_target != NONE && searching;
    let candidate = if lost {
        // A guarded actor takes the alert remark. Other actors report
        // the same contact loss in the general action line.
        let guarded = old_behavior == GUARD || behavior == GUARD;
        Some((if guarded { 0x800 } else { 0xa20 }, 4, 10))
    } else if changed && behavior == search::UNCOVER && target != NONE && awareness(m, actor)? < 3 {
        Some((0x8a0, 3, 10))
    } else if changed && !in_cover(old_behavior) && matches!(behavior, COVER | COVER_FRIEND) {
        Some((0x900, 3, 10))
    } else if changed
        && in_cover(old_behavior)
        && !in_cover(behavior)
        && target != NONE
        && awareness(m, actor)? >= 2
    {
        Some((0xa30, 3, 10))
    } else {
        None
    };
    if let Some((offset, priority, seconds)) = candidate
        && now.wrapping_sub(m.u32(at + 16)?) < 0x8000_0000
        && dialogue::source_offset_row(m, unit, offset)?.is_some()
    {
        m.set_u32(at + 16, now.wrapping_add(seconds * rate))?;
        queue.push(m, unit, offset, priority)?;
    }
    Ok(())
}

pub(super) fn install(b: &mut Builder, queue: Queue) -> Result<(), BuildError> {
    let records = b.guest_alloc(BYTES, Permissions::RW)?;
    b.raw().before(
        &points::HS_CONVERSATION_MAP_DISPOSE,
        "clear Halo 1 tactical dialogue history",
        move |call| {
            call.ctx
                .memory()
                .fill(records, BYTES, 0)
                .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_SUBMIT_UNIT_CONTROL,
        "observe the actor's search and cover transitions",
        move |call| {
            let mut m = call.ctx.memory();
            observe(&mut *m, queue, records, call.cpu.eax).map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_ACTOR_VOCALIZE,
        "report a new sighting after recent combat",
        move |call| {
            if call.cpu.eax as u16 == FIRST_SIGHT {
                let mut m = call.ctx.memory();
                if m.u32(call.cpu.esp)? == FIRST_SIGHT_FROM_PERCEPTION {
                    let target = m.u32(call.cpu.esp + 4)?;
                    sight(&mut *m, queue, records, call.cpu.ecx, target).map_err(guest_fault)?;
                }
            }
            Ok(Flow::Continue)
        },
    );
    Ok(())
}
