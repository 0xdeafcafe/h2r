//! Automatic source reactions captured at Halo 2's AI notifications.

use super::{
    automatic::source_damage,
    cast,
    dialogue::{self, Line},
    leaders::guest_fault,
    *,
};

const OBJECT_CAPACITY: u32 = 2048;
const RECORD_SIZE: u32 = 12;
const BYTES: u32 = OBJECT_CAPACITY * RECORD_SIZE;
const AI_GLOBALS: u32 = 0x004f_55d0;
const INJURY_FROM_DAMAGE: u32 = 0x000c_a517;
const HOSTILE_DAMAGE_MIN: f32 = 0.3;
const INJURY_PRIORITY: u32 = 3;

/// Capture a reaction without re-entering an unfinished damage event.
/// The next per-unit speech update consumes it using the current voice.
#[derive(Clone, Copy)]
pub(super) struct Queue(u32);

impl Queue {
    fn record(self, unit: u32) -> Option<u32> {
        let index = unit & 0xffff;
        (index < OBJECT_CAPACITY).then_some(self.0 + index * RECORD_SIZE)
    }

    fn push(
        self,
        m: &mut dyn GuestMemory,
        unit: u32,
        offset: usize,
        priority: u32,
    ) -> Result<(), MemFault> {
        if let Some(record) = self.record(unit)
            && (m.u32(record)? != unit || m.u32(record + 8)? < priority)
        {
            m.set_u32(record, unit)?;
            m.set_u32(record + 4, offset as u32)?;
            m.set_u32(record + 8, priority)?;
        }
        Ok(())
    }

    pub(super) fn take(self, m: &mut dyn GuestMemory, unit: u32) -> Result<Option<Line>, MemFault> {
        let Some(record) = self.record(unit) else {
            return Ok(None);
        };
        let priority = m.u32(record + 8)?;
        if priority == 0 || m.u32(record)? != unit {
            return Ok(None);
        }
        let offset = m.u32(record + 4)? as usize;
        m.set_u32(record + 8, 0)?;
        let row = dialogue::source_offset_row(m, unit, offset)?;
        Ok(row.map(|row| Line {
            speaker: unit,
            row,
            priority,
        }))
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
        queue.push(m, speaker, offset, INJURY_PRIORITY)?;
    }
    Ok(())
}

pub(super) fn install(b: &mut Builder) -> Result<Queue, BuildError> {
    let queue = Queue(b.guest_alloc(BYTES, Permissions::RW)?);
    b.raw().before(
        &points::HS_CONVERSATION_MAP_DISPOSE,
        "clear queued Halo 1 reactions",
        move |call| {
            call.ctx
                .memory()
                .fill(queue.0, BYTES, 0)
                .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_INJURY_NOTICE,
        "capture the original attacker and victim injury stimuli",
        move |call| {
            let cpu = *call.cpu;
            let m = &mut *call.ctx.runtime().memory();
            (|| {
                if m.u32(cpu.esp)? != INJURY_FROM_DAMAGE || m.u8(cpu.esp + 0x10)? != 0 {
                    return Ok(());
                }
                let ai = m.u32(AI_GLOBALS)?;
                if ai == 0
                    || m.u8(ai + 1)? == 0
                    || m.u8(ai + 0x20)? == 0
                    || read_i16(m, ai + 0x22)? > 0
                {
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
    Ok(queue)
}

#[cfg(test)]
mod tests {
    use engine::testing::FakeGuest;

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
        assert_eq!(queue.take(&mut m, replacement).unwrap(), None);
        queue.push(&mut m, replacement, 0x160, 3).unwrap();
        assert_eq!(m.u32(record).unwrap(), replacement);
        assert_eq!(m.u32(record + 4).unwrap(), 0x160);
    }
}
