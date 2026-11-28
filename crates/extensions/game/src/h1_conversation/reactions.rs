//! Automatic source reactions captured at Halo 2's AI notifications.

use map::H1DialogueClass;

use super::{
    automatic::{actor_class, source_damage},
    cast,
    dialogue::{self, Line},
    leaders::guest_fault,
    replies, search, visibility, *,
};

const OBJECT_CAPACITY: u32 = 2048;
const RECORD_SIZE: u32 = 28;
const BYTES: u32 = OBJECT_CAPACITY * RECORD_SIZE;
const AI_GLOBALS: u32 = 0x004f_55d0;
const INJURY_FROM_DAMAGE: u32 = 0x000c_a517;
const KILL_FROM_DAMAGE: u32 = 0x000c_a4f2;
const HOSTILE_DAMAGE_MIN: f32 = 0.3;
const INJURY_PRIORITY: u32 = 3;

/// Capture a reaction without re-entering an unfinished damage event.
/// The next per-unit speech update consumes it using the current voice.
#[derive(Clone, Copy)]
pub(super) struct Queue(u32);

pub(super) struct Scheduled {
    pub(super) line: Line,
    pub(super) other: u32,
}

impl Queue {
    fn record(self, unit: u32) -> Option<u32> {
        let index = unit & 0xffff;
        (index < OBJECT_CAPACITY).then_some(self.0 + index * RECORD_SIZE)
    }

    pub(super) fn push(
        self,
        m: &mut dyn GuestMemory,
        unit: u32,
        offset: usize,
        priority: u32,
    ) -> Result<(), MemFault> {
        self.push_at(m, unit, offset, priority, 0)
    }

    pub(super) fn push_at(
        self,
        m: &mut dyn GuestMemory,
        unit: u32,
        offset: usize,
        priority: u32,
        due: u32,
    ) -> Result<(), MemFault> {
        self.push_with_other(m, unit, offset, priority, due, NONE)
    }

    pub(super) fn push_with_other(
        self,
        m: &mut dyn GuestMemory,
        unit: u32,
        offset: usize,
        priority: u32,
        due: u32,
        other: u32,
    ) -> Result<(), MemFault> {
        if let Some(record) = self.record(unit)
            && (m.u32(record)? != unit || m.u32(record + 8)? < priority)
        {
            if m.u32(record)? != unit {
                m.fill(record, RECORD_SIZE, 0)?;
            }
            m.set_u32(record, unit)?;
            m.set_u32(record + 4, offset as u32)?;
            m.set_u32(record + 8, priority)?;
            m.set_u32(record + 12, due)?;
            m.set_u32(record + 16, other)?;
        }
        Ok(())
    }

    pub(super) fn take(
        self,
        m: &mut dyn GuestMemory,
        unit: u32,
    ) -> Result<Option<Scheduled>, MemFault> {
        let Some(record) = self.record(unit) else {
            return Ok(None);
        };
        let priority = m.u32(record + 8)?;
        if priority == 0 || m.u32(record)? != unit {
            return Ok(None);
        }
        let due = m.u32(record + 12)?;
        if due != 0 {
            let clock = m.u32(points::GAME_TIME.base)?;
            if clock == 0 || m.u32(clock + 8)?.wrapping_sub(due) >= 0x8000_0000 {
                return Ok(None);
            }
        }
        let offset = m.u32(record + 4)? as usize;
        m.set_u32(record + 8, 0)?;
        let row = dialogue::source_offset_row(m, unit, offset)?;
        let other = m.u32(record + 16)?;
        Ok(row.map(|row| Scheduled {
            line: Line {
                speaker: unit,
                row,
                priority,
            },
            other,
        }))
    }

    pub(super) fn associate(
        self,
        m: &mut dyn GuestMemory,
        line: Line,
        other: u32,
    ) -> Result<(), MemFault> {
        if other != NONE
            && let Some(record) = self.record(line.speaker)
        {
            if m.u32(record)? != line.speaker {
                m.fill(record, RECORD_SIZE, 0)?;
                m.set_u32(record, line.speaker)?;
            }
            m.set_u32(record + 20, line.row + 1)?;
            m.set_u32(record + 24, other)?;
        }
        Ok(())
    }

    pub(super) fn reply_to(
        self,
        m: &mut dyn GuestMemory,
        unit: u32,
        index: u16,
    ) -> Result<u32, MemFault> {
        let Some(record) = self.record(unit) else {
            return Ok(NONE);
        };
        if m.u32(record)? != unit || m.u32(record + 20)? != u32::from(index) + 1 {
            return Ok(NONE);
        }
        m.set_u32(record + 20, 0)?;
        m.u32(record + 24)
    }
}

fn hostile_victim_line(category: u16) -> Option<usize> {
    match category {
        6 => Some(0x2a0),  // melee
        8 => Some(0x2e0),  // mounted weapon
        10 => Some(0x260), // plasma
        11 => Some(0x250), // needler
        _ => None,
    }
}

fn pick_offset(
    m: &dyn GuestMemory,
    speaker: u32,
    special: Option<usize>,
    general: usize,
) -> Result<Option<usize>, MemFault> {
    if let Some(special) = special
        && dialogue::source_offset_row(m, speaker, special)?.is_some()
    {
        return Ok(Some(special));
    }
    Ok(dialogue::source_offset_row(m, speaker, general)?.map(|_| general))
}

fn injury(
    m: &mut dyn GuestMemory,
    queue: Queue,
    victim: u32,
    owner: u32,
    category: u16,
    amount: f32,
) -> Result<(), MemFault> {
    let Some(attacker) = cast::damage_owner(m, owner, category == 9)? else {
        return Ok(());
    };
    if victim == attacker {
        return Ok(());
    }
    let (Some(victim_team), Some(attacker_team)) =
        (cast::team(m, victim)?, cast::team(m, attacker)?)
    else {
        return Ok(());
    };
    let allied = cast::allied(m, victim_team, attacker_team)?;
    let (victim_slot, attacker_slot) = if allied {
        let source_is_player = cast::is_player(m, attacker)?;
        let victim_is_player = cast::is_player(m, victim)?;
        // The native reprimand owns the player variant when it is authored.
        let victim_slot = match pick_offset(m, victim, source_is_player.then_some(0x200), 0x1e0)? {
            Some(0x200) => None,
            other => other,
        };
        let attacker_slot = pick_offset(m, attacker, victim_is_player.then_some(0x170), 0x160)?;
        (victim_slot, attacker_slot)
    } else if amount >= HOSTILE_DAMAGE_MIN {
        (
            pick_offset(m, victim, hostile_victim_line(category), 0x210)?,
            pick_offset(m, attacker, None, 0x180)?,
        )
    } else {
        return Ok(());
    };
    let candidate = match (victim_slot, attacker_slot) {
        (Some(victim_slot), Some(attacker_slot)) => {
            let choices = [(victim, victim_slot), (attacker, attacker_slot)];
            Some(choices[cast::draw_voice(m, choices.len())?])
        }
        (Some(slot), None) => Some((victim, slot)),
        (None, Some(slot)) => Some((attacker, slot)),
        (None, None) => None,
    };
    if let Some((speaker, offset)) = candidate {
        let other = if offset == 0x1e0 { attacker } else { NONE };
        queue.push_with_other(m, speaker, offset, INJURY_PRIORITY, 0, other)?;
    }
    Ok(())
}

pub(super) fn ai_vocalizes(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    let ai = m.u32(AI_GLOBALS)?;
    Ok(ai != 0 && m.u8(ai + 1)? != 0 && m.u8(ai + 0x20)? != 0 && read_i16(m, ai + 0x22)? <= 0)
}

fn witness(
    m: &mut dyn GuestMemory,
    queue: Queue,
    victim: u32,
    attacker: u32,
    allied: bool,
) -> Result<(), MemFault> {
    let Some(anchor) = cast::Unit::read_unit(m, victim, false)? else {
        return Ok(());
    };
    let encounter = search::unit_encounter(m, victim)?;
    let kind = actor_class(m, attacker)?;
    let general = if allied { 0x630 } else { 0x650 };
    let specific = match (allied, kind) {
        (true, H1DialogueClass::Player) => Some(0x640),
        (false, H1DialogueClass::Player) => Some(0x660),
        (false, H1DialogueClass::Covenant) => Some(0x670),
        (
            false,
            H1DialogueClass::CombatForm | H1DialogueClass::Carrier | H1DialogueClass::InfectionForm,
        ) => Some(0x680),
        (false, H1DialogueClass::Sentinel) => Some(0x690),
        _ => None,
    };
    let pool = DatumArray::at(m.u32(cast::ACTORS)?);
    let mut next = 0;
    let mut nearest = 10.0;
    let mut best = None;
    while let Some(index) = pool.next_index(m, next)? {
        next = index + 1;
        let Some(actor) = pool.get_absolute(m, index)? else {
            continue;
        };
        let speaker = m.u32(actor + cast::ACTOR_UNIT)?;
        if speaker == attacker
            || speaker == victim
            || (encounter.is_some() && search::encounter(m, actor)? != encounter)
        {
            continue;
        }
        let Some(unit) = cast::Unit::read(m, speaker)? else {
            continue;
        };
        let distance = cast::distance(anchor.pos, unit.pos);
        if distance < nearest
            && cast::allied(m, anchor.team, unit.team)?
            && visibility::in_pvs(m, anchor.cluster, unit.cluster)?
            && let Some(offset) = pick_offset(m, speaker, specific, general)?
        {
            nearest = distance;
            best = Some((speaker, offset));
        }
    }
    if let Some((speaker, offset)) = best {
        queue.push(m, speaker, offset, if offset == 0x640 { 4 } else { 3 })?;
    }
    Ok(())
}

const SPREE_RECORD: u32 = 12;
const SPREE_BYTES: u32 = OBJECT_CAPACITY * SPREE_RECORD;

fn spree(
    m: &mut dyn GuestMemory,
    queue: Queue,
    records: u32,
    attacker: u32,
) -> Result<(), MemFault> {
    let index = attacker & 0xffff;
    let clock = m.u32(points::GAME_TIME.base)?;
    if index >= OBJECT_CAPACITY || clock == 0 {
        return Ok(());
    }
    let player = cast::is_player(m, attacker)?;
    if !player && dialogue::source_offset_row(m, attacker, map::H1_KILLING_SPREE_LINE)?.is_none() {
        return Ok(());
    }
    let now = m.u32(clock + 8)?;
    let rate = u32::from(m.u16(clock + 2)?);
    let record = records + index * SPREE_RECORD;
    // The original counter resets after 120 Halo 1 ticks between kills.
    let recent = m.u32(record)? == attacker && now.wrapping_sub(m.u32(record + 4)?) <= rate * 4;
    let count = if recent { m.u32(record + 8)? + 1 } else { 1 };
    let threshold = if player { 5 } else { 3 };
    m.set_u32(record, attacker)?;
    m.set_u32(record + 4, now)?;
    m.set_u32(record + 8, if count >= threshold { 0 } else { count })?;
    if count >= threshold {
        if player {
            replies::player_spree(m, queue, attacker)?;
        } else {
            queue.push(m, attacker, map::H1_KILLING_SPREE_LINE, 4)?;
        }
    }
    Ok(())
}

fn kill(
    m: &mut dyn GuestMemory,
    queue: Queue,
    spree_records: u32,
    victim: u32,
    owner: u32,
    category: u16,
) -> Result<(), MemFault> {
    let Some(attacker) = cast::damage_owner(m, owner, category == 9)? else {
        return Ok(());
    };
    if attacker == victim {
        return Ok(());
    }
    let (Some(victim_team), Some(attacker_team)) =
        (cast::team(m, victim)?, cast::team(m, attacker)?)
    else {
        return Ok(());
    };
    let allied = cast::allied(m, victim_team, attacker_team)?;
    if allied {
        if !cast::is_player(m, attacker)? {
            let player = cast::is_player(m, victim)?.then_some(0x340);
            if let Some(offset) = pick_offset(m, attacker, player, 0x320)? {
                queue.push(m, attacker, offset, 5)?;
            }
        }
    } else {
        spree(m, queue, spree_records, attacker)?;
        if cast::is_player(m, attacker)? && !cast::is_player(m, victim)? {
            replies::player_kill(m, queue, attacker, category)?;
        }
    }
    witness(m, queue, victim, attacker, allied)
}

pub(super) fn actor_notice(
    m: &mut dyn GuestMemory,
    queue: Queue,
    actor_id: u32,
    offset: usize,
    priority: u32,
) -> Result<(), MemFault> {
    if ai_vocalizes(m)?
        && let Some(actor) = datum(m, cast::ACTORS, actor_id)?
    {
        let speaker = m.u32(actor + cast::ACTOR_UNIT)?;
        if dialogue::source_offset_row(m, speaker, offset)?.is_some() {
            queue.push(m, speaker, offset, priority)?;
        }
    }
    Ok(())
}

pub(super) fn install(b: &mut Builder) -> Result<Queue, BuildError> {
    let queue = Queue(b.guest_alloc(BYTES, Permissions::RW)?);
    let spree_records = b.guest_alloc(SPREE_BYTES, Permissions::RW)?;
    b.raw().before(
        &points::HS_CONVERSATION_MAP_DISPOSE,
        "clear queued Halo 1 reactions",
        move |call| {
            let mut m = call.ctx.memory();
            m.fill(queue.0, BYTES, 0).map_err(guest_fault)?;
            m.fill(spree_records, SPREE_BYTES, 0).map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_INJURY_NOTICE,
        "capture the original attacker and victim injury stimuli",
        move |call| {
            let cpu = *call.cpu;
            let m: &mut dyn GuestMemory = &mut *call.ctx.runtime().memory();
            (|| {
                if m.u32(cpu.esp)? != INJURY_FROM_DAMAGE || m.u8(cpu.esp + 0x10)? != 0 {
                    return Ok(());
                }
                if !ai_vocalizes(m)? {
                    return Ok(());
                }
                let native = m.u16(cpu.esp + 4)?;
                let category = source_damage(m, cpu.ebp)?.map_or(native, |(_, category)| category);
                let amount = m.f32(cpu.esp + 8)?;
                injury(m, queue, cpu.eax, cpu.edx, category, amount)
            })()
            .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_KILL_NOTICE,
        "capture the original lethal damage reactions",
        move |call| {
            let cpu = *call.cpu;
            let m: &mut dyn GuestMemory = &mut *call.ctx.runtime().memory();
            (|| {
                if m.u32(cpu.esp)? != KILL_FROM_DAMAGE || !ai_vocalizes(m)? {
                    return Ok(());
                }
                let victim = m.u32(cpu.esp + 4)?;
                let owner = m.u32(cpu.esp + 8)?;
                let native = m.u16(cpu.esp + 12)?;
                let category = source_damage(m, cpu.ebp)?.map_or(native, |(_, category)| category);
                kill(m, queue, spree_records, victim, owner, category)
            })()
            .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::UNIT_DIALOGUE_SET_LINE,
        "retire the previous Halo 1 reply context",
        move |call| {
            let unit = call.cpu.eax;
            if let Some(record) = queue.record(unit) {
                let mut m = call.ctx.memory();
                if m.u32(record)? == unit {
                    m.set_u32(record + 20, 0)?;
                }
            }
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_ACTOR_VOCALIZE,
        "report a Halo 1 alliance break",
        move |call| {
            if call.cpu.eax as u16 == 194 {
                let mut m = call.ctx.memory();
                actor_notice(&mut *m, queue, call.cpu.ecx, 0x720, 6).map_err(guest_fault)?;
            }
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_DISMOUNT_NOTICE,
        "report a Halo 1 actor leaving a vehicle",
        move |call| {
            let mut m = call.ctx.memory();
            actor_notice(&mut *m, queue, call.cpu.eax, 0xa50, 3).map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    Ok(queue)
}

#[cfg(test)]
mod tests {
    use guest_layout::testing::FakeGuest;

    use super::*;

    #[test]
    fn queued_reactions_keep_the_salted_speaker_and_higher_priority() {
        let queue = Queue(0x20000);
        let mut m = FakeGuest::new();
        m.map(queue.0, BYTES);
        let original = 0x8400_0001;
        let replacement = 0x8401_0001;
        let record = queue.record(original).unwrap();
        queue.push(&mut m, original, 0x210, 4).unwrap();
        queue.push(&mut m, original, 0x160, 3).unwrap();
        assert_eq!(m.u32(record + 4).unwrap(), 0x210);
        assert!(queue.take(&mut m, replacement).unwrap().is_none());
        queue.push(&mut m, replacement, 0x160, 3).unwrap();
        assert_eq!(m.u32(record).unwrap(), replacement);
        assert_eq!(m.u32(record + 4).unwrap(), 0x160);
    }
}
