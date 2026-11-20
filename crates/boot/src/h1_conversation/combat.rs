//! Combat action lines with no matching native Halo 2 vocabulary.

use super::{
    cast, dialogue,
    leaders::guest_fault,
    reactions::{self, Queue},
    *,
};

const RECORD_SIZE: u32 = 8;
const BYTES: u32 = hs::h1_conversation::ACTOR_CAPACITY * RECORD_SIZE;
const BERSERKING: u32 = 0x226;
const COMBAT_STATUS: u32 = 0x86;

fn firing_slot(m: &dyn GuestMemory, actor: u32, body: u32) -> Result<(usize, u32, u32), MemFault> {
    // Halo 1 0x162a3a selects these in order from the firing actor.
    Ok(if m.u8(actor + BERSERKING)? != 0 {
        (0x980, 3, 20)
    } else if cast::focusing_on_traitor(m, actor)? {
        (0x9a0, 2, 30)
    } else if cast::sees_hostile_group(m, actor)? {
        (0x990, 3, 10)
    } else if cast::mounted(m, body)? {
        (0x970, 2, 20)
    } else {
        (0x960, 2, 10)
    })
}

fn firing(
    m: &mut dyn GuestMemory,
    actor_id: u32,
    records: u32,
    queue: Queue,
) -> Result<(), MemFault> {
    let index = actor_id & 0xffff;
    if index >= hs::h1_conversation::ACTOR_CAPACITY || !reactions::ai_vocalizes(m)? {
        return Ok(());
    }
    let Some(actor) = datum(m, cast::ACTORS, actor_id)? else {
        return Ok(());
    };
    let unit = m.u32(actor + cast::ACTOR_UNIT)?;
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock == 0 || m.u16(actor + COMBAT_STATUS)? < 6 {
        return Ok(());
    }
    let now = m.u32(clock + 8)?;
    let record = records + index * RECORD_SIZE;
    if m.u32(record)? == actor_id && now.wrapping_sub(m.u32(record + 4)?) >= 0x8000_0000 {
        return Ok(());
    }
    let Some(body) = object_address(m, unit)? else {
        return Ok(());
    };
    let (offset, priority, cooldown) = firing_slot(m, actor, body)?;
    if dialogue::source_offset_row(m, unit, offset)?.is_some() {
        let rate = u32::from(m.u16(clock + 2)?);
        m.set_u32(record, actor_id)?;
        m.set_u32(record + 4, now.wrapping_add(rate * cooldown))?;
        queue.push(m, unit, offset, priority)?;
    }
    Ok(())
}

pub(super) fn install(b: &mut Builder, queue: Queue) -> Result<(), BuildError> {
    let records = b.guest_alloc(BYTES, Permissions::RW)?;
    b.raw().before(
        &points::HS_CONVERSATION_MAP_DISPOSE,
        "clear Halo 1 firing remark timers",
        move |call| {
            call.ctx
                .memory()
                .fill(records, BYTES, 0)
                .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_FIRE_BURST,
        "select the Halo 1 firing remark",
        move |call| {
            let mut m = call.ctx.memory();
            firing(&mut *m, call.cpu.eax, records, queue).map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    )
    // The authored command firing interval is chosen first; the remark
    // follows the burst the command atoms set up.
    .after_feature("h1-command-atoms");
    b.raw().after(
        &points::AI_MELEE_LEAP_ENTER,
        "vocalize a successful melee leap",
        move |call| {
            if call.cpu.eax as u8 != 0 {
                let mut m = call.ctx.memory();
                let actor = m.u32(call.cpu.esp.wrapping_sub(8))?;
                reactions::actor_notice(&mut *m, queue, actor, 0xb70, 7).map_err(guest_fault)?;
            }
            Ok(())
        },
    );
    Ok(())
}
