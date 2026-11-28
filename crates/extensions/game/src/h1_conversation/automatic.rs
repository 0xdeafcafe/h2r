//! Source-only lines selected by Halo 2's automatic dialogue scheduler.

use map::H1DialogueClass;

use super::{
    cast,
    dialogue::{self, Line},
    leaders::guest_fault,
    reactions::Queue,
    *,
};

pub(super) const UNIT_SPEAK: u32 = 0x0011_4b60;
const DAMAGE_EFFECT: GroupTag = GroupTag::from_chars(*b"jpt!");

// H2 1.0 call chain from the body-depleted notification to the winning
// candidate. The saved EBP in the AI kill wrapper owns the damage report.
const ENQUEUE_FROM_STIMULUS: u32 = 0x0020_bb4e;
const STIMULUS_FROM_KILL: u32 = 0x001c_9761;
const KILL_FROM_DAMAGE: u32 = 0x000c_a4f2;
const STIMULUS_FRAME: u32 = 0x808;
const KILL_RETURN: u32 = 0x48;
const SAVED_DAMAGE_REPORT: u32 = 0x28;

// H2 1.0 checks the victim's speech before handling the killer's
// reaction. This is the check made by the damage speech function.
const CHECK_FROM_DAMAGE_SPEECH: u32 = 0x0011_4a0b;
const DAMAGE_SPEECH_FROM_REPORT: u32 = 0x000c_a462;
const DAMAGE_SPEECH_FRAME: u32 = 0x54;
const SAVED_VICTIM_REPORT: u32 = 0x18;
const DEATH_PRIORITY: u16 = 15;

pub(super) fn source_damage(
    m: &dyn GuestMemory,
    report: u32,
) -> Result<Option<(u32, u16)>, MemFault> {
    let damage = DatumHandle(m.u32(report + 8)?);
    let Some(tag) = guest_layout::view::Tags.entry(m, damage)? else {
        return Ok(None);
    };
    if tag.group(m)? != DAMAGE_EFFECT {
        return Ok(None);
    }
    let root = tag.address(m)?;
    if root == 0 {
        return Ok(None);
    }
    let category = m.u32(root + 0x34)?;
    Ok((category & 0xffff_0000 == map::H1_DAMAGE_CATEGORY).then_some((root, category as u16)))
}

pub(super) fn actor_class(m: &dyn GuestMemory, unit: u32) -> Result<H1DialogueClass, MemFault> {
    if cast::is_player(m, unit)? {
        return Ok(H1DialogueClass::Player);
    }
    let actor = cast::actor_handle(m, unit)?.unwrap_or(NONE);
    let Some(at) = datum(m, cast::ACTORS, actor)? else {
        return Ok(H1DialogueClass::Other);
    };
    Ok(H1DialogueClass::actor(m.u16(at + 4)?))
}

fn kill_line(m: &dyn GuestMemory, stack: u32) -> Result<Option<Line>, MemFault> {
    if m.u32(stack)? != ENQUEUE_FROM_STIMULUS || m.u16(stack + 0xc)? != 0 {
        return Ok(None);
    }
    let candidate = m.u32(stack + 0x10)?;
    let vocalization = m.u16(candidate)?;
    // The event has selected a hostile-attacker cheer. H2's separate
    // witness, lament and player-praise candidates use other entries.
    if !matches!(vocalization, 244..=253) {
        return Ok(None);
    }
    let stimulus = stack + STIMULUS_FRAME;
    if m.u32(stimulus)? != STIMULUS_FROM_KILL || m.u32(stimulus + KILL_RETURN)? != KILL_FROM_DAMAGE
    {
        return Ok(None);
    }
    let speaker = m.u32(candidate + 4)?;
    if speaker != m.u32(stimulus + 0xc)? {
        return Ok(None);
    }
    let report = m.u32(stimulus + SAVED_DAMAGE_REPORT)?;
    let weapon = source_damage(m, report)?.and_then(|(_, category)| {
        map::H1_WEAPON_KILL_LINES
            .iter()
            .find_map(|&(kind, offset)| (kind == category).then_some(offset))
    });
    let row_for = |offset| dialogue::source_offset_row(m, speaker, offset);
    let mut row = weapon.map(row_for).transpose()?.flatten();
    // Keep the existing explicit grenade cheer. Otherwise, a classified
    // victim can select a variant whenever the voice has that line.
    if row.is_none() && vocalization != 248 {
        let victim = m.u32(stimulus + 8)?;
        if let Some(offset) = actor_class(m, victim)?.hostile_kill_line() {
            row = row_for(offset)?;
        }
    }
    if row.is_none() && vocalization == 248 && row_for(0x460)?.is_none() {
        row = row_for(0x360)?;
    }
    let Some(row) = row else { return Ok(None) };
    let adlg = m.u32(stack + 4)?;
    let vocabulary = m.u32(adlg + 4)?;
    let priority = u32::from(m.u16(vocabulary + u32::from(vocalization) * 0x60 + 0xa)?);
    Ok(Some(Line {
        speaker,
        row,
        priority,
    }))
}

fn flying_death(m: &dyn GuestMemory, stack: u32, unit: u32) -> Result<Option<u32>, MemFault> {
    if m.u32(stack)? != CHECK_FROM_DAMAGE_SPEECH
        || m.u16(stack + 4)? != DEATH_PRIORITY
        || m.u32(stack + DAMAGE_SPEECH_FRAME)? != DAMAGE_SPEECH_FROM_REPORT
    {
        return Ok(None);
    }
    let report = m.u32(stack + SAVED_VICTIM_REPORT)?;
    let Some((root, category)) = source_damage(m, report)? else {
        return Ok(None);
    };
    // H1 0x47275 checks falling and burn categories first. For other
    // categories it uses deathflying when jpt! +0x1f4 is at least 2.0.
    if !matches!(category, 1 | 7) && m.f32(root + 0x38)? >= 2.0 {
        return dialogue::source_offset_row(m, unit, map::H1_DEATH_FLYING_LINE);
    }
    Ok(None)
}

pub(super) fn install(b: &mut Builder, reactions: Queue) {
    b.raw().before(
        &points::UNIT_REANIMATE,
        "queue Halo 1 Flood revival dialogue",
        move |call| {
            let unit = call.cpu.eax;
            let m: &mut dyn GuestMemory = &mut *call.ctx.runtime().memory();
            (|| {
                if dialogue::source_offset_row(m, unit, map::H1_REVIVAL_LINE)?.is_some()
                    && let Some(body) = object_address(m, unit)?
                    && m.u8(body + 0x10a)? & 4 != 0
                {
                    reactions.push(m, unit, map::H1_REVIVAL_LINE, 14)?;
                }
                Ok(())
            })()
            .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::UNIT_DIALOGUE_CHECK,
        "resolve the Halo 1 flying-death line for the victim",
        |call| {
            let stack = call.cpu.esp;
            let unit = call.cpu.eax;
            let row =
                flying_death(&*call.ctx.runtime().memory(), stack, unit).map_err(guest_fault)?;
            if let Some(row) = row {
                call.cpu.edx = row;
            }
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::AI_DIALOGUE_ENQUEUE,
        "queue the Halo 1 weapon or victim-specific kill line",
        move |call| {
            let stack = call.cpu.esp;
            let (line, grenade_fallback) = {
                let m = call.ctx.runtime().memory();
                let Some(line) = kill_line(&*m, stack).map_err(guest_fault)? else {
                    return Ok(Flow::Continue);
                };
                let generic =
                    dialogue::source_offset_row(&*m, line.speaker, 0x360).map_err(guest_fault)?;
                (line, generic == Some(line.row))
            };
            let Line {
                speaker,
                row,
                priority,
            } = line;
            call.answer_async(move |cx| {
                Box::pin(async move {
                    let queued = cx.arg(6)?;
                    if queued != 0 {
                        cx.write(|m| m.set_u32(queued, NONE))?;
                    }
                    let result = cx
                        .call(
                            GuestCall::new(UNIT_SPEAK)
                                .eax(row)
                                .args([speaker, priority, 0]),
                        )
                        .await?;
                    if grenade_fallback && result.eax as u8 != 0 {
                        cx.write(|m| reactions.associate(m, line, 3))?;
                    }
                    Ok(Ret::new(result.eax & 0xff, 28))
                })
            })
        },
    );
}

#[cfg(test)]
mod tests {
    use guest_layout::datum::layout;
    use guest_layout::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, entry_layout, index_layout};
    use guest_layout::testing::FakeGuest;

    use super::*;

    #[test]
    fn damage_dialogue_uses_the_killers_and_victims_separate_lines() {
        let stack = 0x10000;
        let stimulus = stack + STIMULUS_FRAME;
        let candidate = 0x11000;
        let adlg = 0x12000;
        let vocabulary = 0x13000;
        let report = 0x19000;
        let index = 0x20000;
        let entries = 0x21000;
        let objects = 0x22000;
        let object_rows = 0x23000;
        let killer = 0x8400_0000;
        let body = 0x24000;
        let udlg = 0x25000;
        let rows = 0x26000;
        let jpt = 0x27000;
        let tag = |i| 0xe000_0000 | i;
        let sniper_slot = 0x450 / 0x10 - 1;

        let mut m = FakeGuest::new();
        m.put_u32(stack, ENQUEUE_FROM_STIMULUS)
            .put_u32(stack + 4, adlg)
            .put_u16(stack + 0xc, 0)
            .put_u32(stack + 0x10, candidate)
            .put_u32(stimulus, STIMULUS_FROM_KILL)
            .put_u32(stimulus + 0xc, killer)
            .put_u32(stimulus + SAVED_DAMAGE_REPORT, report)
            .put_u32(stimulus + KILL_RETURN, KILL_FROM_DAMAGE)
            .put_u16(candidate, 246)
            .put_u32(candidate + 4, killer)
            .put_u32(adlg + 4, vocabulary)
            .put_u16(vocabulary + 246 * 0x60 + 0xa, 7)
            .put_u32(report + 8, tag(1))
            .put_u32(TAG_INDEX_GLOBAL, index)
            .put_u32(TAG_ENTRIES_GLOBAL, entries)
            .put_u32(index + index_layout::TAG_COUNT, 3)
            .put_u32(points::OBJECTS.base, objects)
            .put_u32(objects + layout::DATA, object_rows)
            .put_u32(objects + layout::HIGH_WATER, 1)
            .put_u32(objects + layout::ELEMENT_SIZE, 12)
            .put_u16(object_rows, 0x8400)
            .put_u32(object_rows + 8, body)
            .put_u16(body + 0x342, 0x500)
            .put_u32(body + 0x500, tag(0))
            .put_u32(udlg + 0xc, 2)
            .put_u32(udlg + 0x10, rows)
            .put_u32(rows + 0x10, map::H1_DIALOGUE_SLOT | sniper_slot)
            .put_u32(rows + 0x1c, tag(2))
            .put_u16(jpt + 0x12, 2)
            .put_u32(jpt + 0x34, map::H1_DAMAGE_CATEGORY | 5);
        for (i, name, address) in [
            (0, *b"udlg", udlg),
            (1, *b"jpt!", jpt),
            (2, *b"snd!", 0x28000),
        ] {
            let entry = entries + i * entry_layout::STRIDE;
            m.put_u32(entry + entry_layout::GROUP, GroupTag::from_chars(name).0)
                .put_u32(entry + entry_layout::DATUM, tag(i))
                .put_u32(entry + entry_layout::ADDRESS, address);
        }
        assert_eq!(
            kill_line(&m, stack).unwrap(),
            Some(Line {
                speaker: killer,
                row: 1,
                priority: 7
            })
        );

        m.put_u32(candidate + 4, 0x8400_0001);
        assert_eq!(kill_line(&m, stack).unwrap(), None);
        m.put_u32(candidate + 4, killer).put_u16(candidate, 254);
        assert_eq!(kill_line(&m, stack).unwrap(), None);
        m.put_u16(candidate, 246).put_u16(stack + 0xc, 3);
        assert_eq!(kill_line(&m, stack).unwrap(), None);
        m.put_u16(stack + 0xc, 0).put_u32(jpt + 0x34, 0);
        assert_eq!(kill_line(&m, stack).unwrap(), None);
        m.put_u32(jpt + 0x34, map::H1_DAMAGE_CATEGORY | 2);
        assert_eq!(kill_line(&m, stack).unwrap(), None);

        let victim = 0x8400_0001;
        let victim_body = 0x29000;
        let actors = 0x2a000;
        let actor_rows = 0x2b000;
        let actor = 0x8500_0000;
        m.put_u32(stimulus + 8, victim)
            .put_u32(objects + layout::HIGH_WATER, 2)
            .put_u16(object_rows + 12, 0x8400)
            .put_u32(object_rows + 12 + 8, victim_body)
            .put_u32(victim_body + 0x13c, NONE)
            .put_u32(victim_body + cast::UNIT_ACTOR, actor)
            .put_u32(cast::ACTORS, actors)
            .put_u32(actors + layout::DATA, actor_rows)
            .put_u32(actors + layout::HIGH_WATER, 1)
            .put_u32(actors + layout::ELEMENT_SIZE, 0x888)
            .put_u16(actor_rows, 0x8500)
            .put_u32(actor_rows + cast::ACTOR_UNIT, victim)
            .put_u16(actor_rows + 4, 11)
            .put_u32(rows + 0x10, map::H1_DIALOGUE_SLOT | (0x3e0 / 0x10 - 1));
        assert_eq!(
            kill_line(&m, stack).unwrap(),
            Some(Line {
                speaker: killer,
                row: 1,
                priority: 7
            })
        );
        m.put_u16(actor_rows + 4, 9);
        assert_eq!(kill_line(&m, stack).unwrap(), None);

        m.put_u32(jpt + 0x34, map::H1_DAMAGE_CATEGORY | 5)
            .put_u32(stimulus + KILL_RETURN, 0);
        assert_eq!(kill_line(&m, stack).unwrap(), None);

        let flying_slot = (map::H1_DEATH_FLYING_LINE / 0x10 - 1) as u32;
        m.put_u32(stack, CHECK_FROM_DAMAGE_SPEECH)
            .put_u16(stack + 4, DEATH_PRIORITY)
            .put_u32(stack + DAMAGE_SPEECH_FRAME, DAMAGE_SPEECH_FROM_REPORT)
            .put_u32(stack + SAVED_VICTIM_REPORT, report)
            .put_u32(rows + 0x10, map::H1_DIALOGUE_SLOT | flying_slot)
            .put_u32(jpt + 0x38, 2.0f32.to_bits());
        assert_eq!(flying_death(&m, stack, killer).unwrap(), Some(1));
        m.put_u32(jpt + 0x38, 1.999f32.to_bits());
        assert_eq!(flying_death(&m, stack, killer).unwrap(), None);
        m.put_u32(jpt + 0x38, 2.0f32.to_bits())
            .put_u32(jpt + 0x34, map::H1_DAMAGE_CATEGORY | 1);
        assert_eq!(flying_death(&m, stack, killer).unwrap(), None);
        m.put_u32(jpt + 0x34, map::H1_DAMAGE_CATEGORY | 5)
            .put_u16(stack + 4, 12);
        assert_eq!(flying_death(&m, stack, killer).unwrap(), None);
    }
}
