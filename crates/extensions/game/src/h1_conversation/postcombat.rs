//! Encounter outcomes and corpse actions use the Halo 1 dialogue slots.

use map::H1DialogueClass;

use super::{
    automatic::actor_class,
    cast, dialogue,
    leaders::guest_fault,
    reactions::{self, Queue},
    search, *,
};

const GROUP_CAPACITY: u32 = 100;
const SQUAD_CAPACITY: u32 = 384;
const ACTOR_CAPACITY: u32 = hs::h1_conversation::ACTOR_CAPACITY;
const GROUP_BYTES: u32 = (GROUP_CAPACITY + SQUAD_CAPACITY + ACTOR_CAPACITY) * 4;
const VITALITY_STRIDE: u32 = 16;
const VITALITY_BYTES: u32 = ACTOR_CAPACITY * VITALITY_STRIDE;
const COMBAT_STATUS: u32 = 0x86;
const CONTACT: u16 = 6;
const INITIAL_VITALITY: u32 = 4;
const LAST_SAFE_VITALITY: u32 = 8;
const ENGAGED: u32 = 12;
const CHECK_FRIEND: u16 = 98;
const CHECK_FRIEND_NOTICE: u32 = 0x001b_3c43;
const SHOOT_CORPSE: u16 = 96;
const SHOOT_CORPSE_NOTICE: u32 = 0x001b_39d3;

#[derive(Clone, Copy)]
struct Battles {
    groups: u32,
    vitality: u32,
}

impl Battles {
    fn vitality(self, actor: u32) -> Option<u32> {
        let index = actor & 0xffff;
        (index < ACTOR_CAPACITY).then_some(self.vitality + index * VITALITY_STRIDE)
    }

    fn group(self, m: &dyn GuestMemory, actor: u32, id: u32) -> Result<u32, MemFault> {
        if let Some(group) = search::encounter(m, actor)?
            && u32::from(group) < GROUP_CAPACITY
        {
            return Ok(self.groups + u32::from(group) * 4);
        }
        let squad = m.u32(actor + search::SQUAD_INDEX)?;
        let index = if squad < SQUAD_CAPACITY {
            GROUP_CAPACITY + squad
        } else {
            GROUP_CAPACITY + SQUAD_CAPACITY + (id & 0xffff)
        };
        Ok(self.groups + index * 4)
    }
}

fn outcome(start: u32, alive: u32, initial_vitality: f32, vitality: f32) -> Option<usize> {
    // The encounter counts and the chosen speaker's health are tested
    // in this order by Halo 1 0x13d66c–0x13d760.
    if vitality < 0.5 && initial_vitality - vitality > 0.3 {
        Some(0xc80)
    } else if alive == 1 && start > 1 {
        Some(0xc60)
    } else if alive >= 2 && start >= alive + alive.min(2) {
        Some(0xca0)
    } else if alive >= 2 && alive >= start.saturating_sub(1) {
        Some(0xcc0)
    } else if vitality > 0.8 {
        Some(0xc70)
    } else {
        None
    }
}

fn observe(m: &mut dyn GuestMemory, battles: Battles, actor_id: u32) -> Result<(), MemFault> {
    let Some(vitals) = battles.vitality(actor_id) else {
        return Ok(());
    };
    let Some(actor) = datum(m, cast::ACTORS, actor_id)? else {
        return Ok(());
    };
    let unit = m.u32(actor + cast::ACTOR_UNIT)?;
    let Some(body) = object_address(m, unit)? else {
        return Ok(());
    };
    if !dialogue::converted_bank(m, dialogue::active(m, unit)?)? {
        return Ok(());
    }
    let vitality = m.f32(body + crate::health::BODY_HEALTH)?;
    if m.u32(vitals)? != actor_id {
        m.fill(vitals, VITALITY_STRIDE, 0)?;
        m.set_u32(vitals, actor_id)?;
        m.set_f32(vitals + LAST_SAFE_VITALITY, vitality)?;
    }
    let group = battles.group(m, actor, actor_id)?;
    if m.u16(actor + COMBAT_STATUS)? < CONTACT {
        m.set_u32(vitals + ENGAGED, 0)?;
        m.set_f32(vitals + LAST_SAFE_VITALITY, vitality)?;
        return Ok(());
    }
    if m.u32(vitals + ENGAGED)? == 0 {
        m.set_u32(vitals + ENGAGED, 1)?;
        let initial = m.f32(vitals + LAST_SAFE_VITALITY)?;
        m.set_f32(vitals + INITIAL_VITALITY, initial)?;
    }
    if m.u32(group)? == 0 {
        let alive = living_members(m, battles, group, cast::team(m, unit)?)?.0;
        m.set_u32(group, alive)?;
    }
    Ok(())
}

fn living_members(
    m: &dyn GuestMemory,
    battles: Battles,
    group: u32,
    source_team: Option<i16>,
) -> Result<(u32, bool), MemFault> {
    let pool = DatumArray::at(m.u32(cast::ACTORS)?);
    let mut next = 0;
    let mut alive = 0;
    let mut engagement = false;
    while let Some(index) = pool.next_index(m, next)? {
        next = index + 1;
        let Some(actor) = pool.get_absolute(m, index)? else {
            continue;
        };
        let id = (u32::from(m.u16(actor)?) << 16) | index as u32;
        if index as u32 >= ACTOR_CAPACITY || battles.group(m, actor, id)? != group {
            continue;
        }
        if let Some(unit) = cast::Unit::read(m, m.u32(actor + cast::ACTOR_UNIT)?)?
            && source_team.is_none_or(|team| team == unit.team)
        {
            alive += 1;
            engagement |= m.u16(actor + COMBAT_STATUS)? >= CONTACT;
        }
    }
    Ok((alive, engagement))
}

fn enter(
    m: &mut dyn GuestMemory,
    battles: Battles,
    queue: Queue,
    actor_id: u32,
) -> Result<(), MemFault> {
    if !reactions::ai_vocalizes(m)? {
        return Ok(());
    }
    let Some(actor) = datum(m, cast::ACTORS, actor_id)? else {
        return Ok(());
    };
    let Some(vitals) = battles.vitality(actor_id) else {
        return Ok(());
    };
    if m.u32(vitals)? != actor_id {
        return Ok(());
    }
    let speaker = m.u32(actor + cast::ACTOR_UNIT)?;
    if actor_class(m, speaker)? != H1DialogueClass::Human {
        return Ok(());
    }
    let group = battles.group(m, actor, actor_id)?;
    let started = m.u32(group)?;
    if started == 0 {
        return Ok(());
    }
    let Some(body) = object_address(m, speaker)? else {
        return Ok(());
    };
    let vitality = m.f32(body + crate::health::BODY_HEALTH)?;
    if vitality <= 0.0 {
        return Ok(());
    }
    let (alive, engagement) = living_members(m, battles, group, cast::team(m, speaker)?)?;
    if engagement {
        return Ok(());
    }
    let initial = m.f32(vitals + INITIAL_VITALITY)?;
    if let Some(offset) = outcome(started, alive, initial, vitality)
        && dialogue::source_offset_row(m, speaker, offset)?.is_some()
    {
        m.set_u32(group, 0)?;
        queue.push(m, speaker, offset, 3)?;
    }
    Ok(())
}

pub(super) fn install(b: &mut Builder, queue: Queue) -> Result<(), BuildError> {
    let battles = Battles {
        groups: b.guest_alloc(GROUP_BYTES, Permissions::RW)?,
        vitality: b.guest_alloc(VITALITY_BYTES, Permissions::RW)?,
    };
    b.raw().before(
        &points::HS_CONVERSATION_MAP_DISPOSE,
        "clear Halo 1 encounter outcome history",
        move |call| {
            let mut m = call.ctx.memory();
            m.fill(battles.groups, GROUP_BYTES, 0)
                .map_err(guest_fault)?;
            m.fill(battles.vitality, VITALITY_BYTES, 0)
                .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_SUBMIT_UNIT_CONTROL,
        "record the Halo 1 encounter's starting vitality and strength",
        move |call| {
            let mut m = call.ctx.memory();
            observe(&mut *m, battles, call.cpu.eax).map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    )
    // AI control's combat-timer hold runs first; this sees the control it left.
    .after_feature("h1-ai-control");
    b.raw().before(
        &points::AI_POSTCOMBAT_ENTER,
        "select the Halo 1 encounter outcome remark",
        move |call| {
            let mut m = call.ctx.memory();
            let actor = m.u32(call.cpu.esp + 4)?;
            enter(&mut *m, battles, queue, actor).map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_DIALOGUE_STIMULUS,
        "play the Halo 1 remark when checking an allied body",
        move |call| {
            let m: &mut dyn GuestMemory = &mut *call.ctx.memory();
            (|| {
                if m.u32(call.cpu.esp)? == CHECK_FRIEND_NOTICE
                    && m.u16(call.cpu.esp + 4)? == CHECK_FRIEND
                    && reactions::ai_vocalizes(&*m)?
                {
                    let unit = m.u32(call.cpu.esp + 8)?;
                    if dialogue::source_offset_row(&*m, unit, 0xbf0)?.is_some() {
                        queue.push(&mut *m, unit, 0xbf0, 3)?;
                    }
                }
                Ok(())
            })()
            .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_ACTOR_VOCALIZE,
        "select the Halo 1 remark for shooting a player's corpse",
        move |call| {
            if call.cpu.eax as u16 == SHOOT_CORPSE {
                let m: &mut dyn GuestMemory = &mut *call.ctx.memory();
                (|| {
                    if m.u32(call.cpu.esp)? == SHOOT_CORPSE_NOTICE
                        && cast::is_player(&*m, m.u32(call.cpu.esp + 4)?)?
                    {
                        reactions::actor_notice(&mut *m, queue, call.cpu.ecx, 0xc10, 6)?;
                    }
                    Ok(())
                })()
                .map_err(guest_fault)?;
            }
            Ok(Flow::Continue)
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aftermath_prioritizes_new_wounds_over_casualty_count() {
        assert_eq!(outcome(5, 1, 1.0, 0.4), Some(0xc80));
        assert_eq!(outcome(5, 1, 0.6, 0.4), Some(0xc60));
        assert_eq!(outcome(4, 2, 1.0, 0.9), Some(0xca0));
        assert_eq!(outcome(3, 2, 1.0, 0.9), Some(0xcc0));
        assert_eq!(outcome(1, 1, 1.0, 1.0), Some(0xc70));
        assert_eq!(outcome(1, 1, 1.0, 0.8), None);
    }
}
