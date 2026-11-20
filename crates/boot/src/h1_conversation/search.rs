//! Halo 1's periodic search query and the second actor's reply.

use std::sync::Arc;

use map::{H1_SEARCH_QUERY_LINE, H1_SEARCH_REPLY_LINE};

use super::{
    automatic::UNIT_SPEAK,
    cast,
    dialogue::{self, Line},
    leaders::guest_fault,
    physical,
    reactions::{Queue, Scheduled},
    replies, visibility, *,
};

const ACTOR_COUNT: u32 = hs::h1_conversation::ACTOR_CAPACITY;
const RECORD_SIZE: u32 = 0x10;
const BYTES: u32 = ACTOR_COUNT * RECORD_SIZE;
const AI_GLOBALS: u32 = 0x004f_55d0;
const BEHAVIOR_DEPTH: u32 = 0x190;
const BEHAVIOR_FRAMES: u32 = 0x90;
const BEHAVIOR_STRIDE: u32 = 0x40;
pub(super) const PRESEARCH: u16 = 27;
pub(super) const SEARCH: u16 = 34;
pub(super) const UNCOVER: u16 = 35;
pub(super) const INVESTIGATE: u16 = 36;
pub(super) const POSTSEARCH: u16 = 39;
const FOCUS: u32 = 0x338;
pub(super) const SQUAD_INDEX: u32 = 0x30;
const SQUAD_COUNT: u32 = 0x160;
const SQUAD_ROWS: u32 = 0x164;
const SQUAD_STRIDE: u32 = 0x74;
const SQUAD_GROUP: u32 = 0x26;
const RESPONSE_RANGE: f32 = 9.0;
const PRIORITY: u32 = 3;

pub(super) fn behavior(m: &dyn GuestMemory, actor: u32) -> Result<u16, MemFault> {
    let depth = u32::from(m.u16(actor + BEHAVIOR_DEPTH)?);
    if depth >= 4 {
        return Ok(u16::MAX);
    }
    m.u16(actor + BEHAVIOR_FRAMES + depth * BEHAVIOR_STRIDE)
}

pub(super) fn encounter(m: &dyn GuestMemory, actor: u32) -> Result<Option<u16>, MemFault> {
    let scnr = m.u32(points::SCENARIO.base)?;
    let squad = m.u32(actor + SQUAD_INDEX)?;
    if scnr == 0 || squad >= m.u32(scnr + SQUAD_COUNT)? {
        return Ok(None);
    }
    let squads = m.u32(scnr + SQUAD_ROWS)?;
    let group = m.u16(squads + squad * SQUAD_STRIDE + SQUAD_GROUP)?;
    Ok((group != u16::MAX).then_some(group))
}

pub(super) fn unit_encounter(m: &dyn GuestMemory, unit: u32) -> Result<Option<u16>, MemFault> {
    let Some(actor) = cast::actor_handle(m, unit)? else {
        return Ok(None);
    };
    match datum(m, cast::ACTORS, actor)? {
        Some(at) => encounter(m, at),
        None => Ok(None),
    }
}

fn reply(m: &dyn GuestMemory, unit: u32, actor: u32) -> Result<Option<Line>, MemFault> {
    let Some(source) = cast::Unit::read(m, unit)? else {
        return Ok(None);
    };
    let source_encounter = encounter(m, actor)?;
    let pool = DatumArray::at(m.u32(cast::ACTORS)?);
    let mut next = 0;
    let mut best = None;
    let mut nearest = RESPONSE_RANGE;
    while let Some(index) = pool.next_index(m, next)? {
        next = index + 1;
        let Some(other) = pool.get_absolute(m, index)? else {
            continue;
        };
        if actor == other
            || (source_encounter.is_some() && encounter(m, other)? != source_encounter)
        {
            continue;
        }
        let mode = behavior(m, other)?;
        // Halo 1 accepts search and the uncover variant with a tracked foe.
        let searching = matches!(mode, SEARCH | INVESTIGATE)
            || (mode == UNCOVER && m.u32(other + FOCUS)? != NONE);
        if !searching {
            continue;
        }
        let speaker = m.u32(other + cast::ACTOR_UNIT)?;
        let Some(candidate) = cast::Unit::read(m, speaker)? else {
            continue;
        };
        if !cast::allied(m, source.team, candidate.team)?
            || !visibility::in_pvs(m, source.cluster, candidate.cluster)?
        {
            continue;
        }
        let distance = cast::distance(source.pos, candidate.pos);
        if distance <= nearest
            && let Some(row) = dialogue::source_offset_row(m, speaker, H1_SEARCH_REPLY_LINE)?
        {
            nearest = distance;
            best = Some(Line {
                speaker,
                row,
                priority: PRIORITY,
            });
        }
    }
    Ok(best)
}

fn reached(now: u32, at: u32) -> bool {
    at != 0 && now.wrapping_sub(at) < 0x8000_0000
}

fn active_query(m: &dyn GuestMemory, speech: u32, expected: u32) -> Result<bool, MemFault> {
    Ok(m.u16(speech + 0xc)? != 0 && u32::from(m.u16(speech + 0xe)?) + 1 == expected)
}

fn tick(m: &mut dyn GuestMemory, records: u32, unit: u32) -> Result<Option<Line>, MemFault> {
    let Some(actor_id) = cast::actor_handle(m, unit)? else {
        return Ok(None);
    };
    let index = actor_id & 0xffff;
    if index >= ACTOR_COUNT {
        return Ok(None);
    }
    let Some(actor) = datum(m, cast::ACTORS, actor_id)? else {
        return Ok(None);
    };
    let Some(speech) = dialogue::speech(m, unit)? else {
        return Ok(None);
    };
    let ai = m.u32(AI_GLOBALS)?;
    let clock = m.u32(points::GAME_TIME.base)?;
    if ai == 0 || clock == 0 || m.u8(ai + 0x20)? == 0 || read_i16(m, ai + 0x22)? > 0 {
        return Ok(None);
    }
    let rate = u32::from(m.u16(clock + 2)?);
    if rate == 0 {
        return Ok(None);
    }
    let now = m.u32(clock + 8)?;
    let record = records + index * RECORD_SIZE;
    if m.u32(record)? != actor_id {
        m.fill(record, RECORD_SIZE, 0)?;
        m.set_u32(record, actor_id)?;
    }

    let reply_due = m.u32(record + 0xc)?;
    if reached(now, reply_due) {
        m.set_u32(record + 0xc, 0)?;
        if let Some(line) = reply(m, unit, actor)? {
            return Ok(Some(line));
        }
    }
    let expected = m.u32(record + 8)?;
    if expected != 0 {
        if !active_query(m, speech, expected)? {
            m.set_u32(record + 8, 0)?;
        } else if m.u8(speech + 0x48)? != 0 && m.u16(speech + 0x4c)? <= 1 {
            m.set_u32(record + 8, 0)?;
            m.set_u32(record + 0xc, now.wrapping_add(rate.div_ceil(2)))?;
        }
    }

    // Halo 1 asks from its wait mode. These Halo 2 phases hold the
    // actor before or after the active investigation of a contact.
    let at = m.u32(record + 4)?;
    if !matches!(behavior(m, actor)?, PRESEARCH | POSTSEARCH) {
        if at != 0 {
            m.set_u32(record + 4, 0)?;
        }
        return Ok(None);
    }
    if at != 0 && !reached(now, at) {
        return Ok(None);
    }
    let Some(row) = dialogue::source_offset_row(m, unit, H1_SEARCH_QUERY_LINE)? else {
        return Ok(None);
    };
    let period = rate * 10;
    let jitter = cast::draw_voice(m, period as usize)? as u32;
    m.set_u32(record + 4, now.wrapping_add(period + jitter))?;
    if at == 0 {
        return Ok(None);
    }
    m.set_u32(record + 8, row + 1)?;
    Ok(Some(Line {
        speaker: unit,
        row,
        priority: PRIORITY,
    }))
}

pub(super) fn install(
    b: &mut Builder,
    reactions: Queue,
    physical_history: physical::History,
) -> Result<(), BuildError> {
    let records = b.guest_alloc(BYTES, Permissions::RW)?;
    b.raw().before(
        &points::HS_CONVERSATION_MAP_DISPOSE,
        "clear Halo 1 search timers",
        move |call| {
            call.ctx
                .memory()
                .fill(records, BYTES, 0)
                .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().before(
        &points::UNIT_DIALOGUE_UPDATE,
        "advance Halo 1 search dialogue",
        move |call| {
            let rt = Arc::clone(call.ctx.runtime());
            let saved = *call.cpu;
            let scheduled = {
                let m = &mut *rt.memory();
                replies::on_speech_tick(m, reactions, saved.eax).map_err(guest_fault)?;
                match reactions.take(m, saved.eax).map_err(guest_fault)? {
                    Some(line) => Some(line),
                    None => tick(m, records, saved.eax)
                        .map_err(guest_fault)?
                        .map(|line| Scheduled { line, other: NONE }),
                }
            };
            let Some(Scheduled { line, other }) = scheduled else {
                let fall = physical::falling(&mut *rt.memory(), physical_history, saved.eax)
                    .map_err(guest_fault)?;
                return match fall {
                    Some(fall) => physical::probe(call, fall, reactions),
                    None => Ok(Flow::Continue),
                };
            };
            let addr = call.addr;
            let original = rt
                .resolve_original(addr)
                .expect("speech update is a generated hook");
            let mut m = rt.memory();
            let stack = saved.esp.wrapping_sub(12);
            m.set_u32(stack, line.speaker).map_err(guest_fault)?;
            m.set_u32(stack + 4, line.priority).map_err(guest_fault)?;
            m.set_u32(stack + 8, 0).map_err(guest_fault)?;
            call.cpu.esp = stack;
            call.cpu.eax = line.row;
            call.ctx
                .call_guest_named(
                    call.cpu,
                    &mut m,
                    UNIT_SPEAK,
                    "halo 1 automatic speech",
                    move |cpu, ctx| {
                        if cpu.eax as u8 != 0 {
                            reactions
                                .associate(&mut *ctx.memory(), line, other)
                                .map_err(guest_fault)?;
                        }
                        *cpu = saved;
                        Ok(original.invoke(cpu, ctx, addr))
                    },
                )
                .map(Flow::Return)
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_timer_wraps_and_zero_is_inactive() {
        assert!(!reached(400, 401));
        assert!(reached(401, 401));
        assert!(reached(2, u32::MAX));
        assert!(!reached(2, 0));
    }
}
