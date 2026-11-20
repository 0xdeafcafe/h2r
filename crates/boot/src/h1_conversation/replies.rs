//! Replies triggered by the end of a source utterance (Halo 1 0x47073).

use super::{
    cast, dialogue,
    reactions::{self, Queue},
    search, tactics, visibility, *,
};

const RESPONSE_RANGE: f32 = 9.0;
const RECOGNITION_RANGE: f32 = 5.0;

#[derive(Clone, Copy)]
struct Reply {
    offset: usize,
    chance: usize,
    priority: u32,
    range: f32,
}

impl Reply {
    fn after(offset: usize) -> Option<Self> {
        let (offset, chance, priority) = match offset {
            0x180 => (0x190, 80, 3),
            0x320 => (0x330, 70, 3),
            0x340 => (0x350, 100, 4),
            0x360 => (0x370, 60, 3),
            0x380 => (0x390, 100, 4),
            0x3a0 => (0x3b0, 80, 3),
            0x3c0 => (0x3d0, 80, 3),
            0x3e0 => (0x3f0, 80, 3),
            0x400 => (0x410, 80, 3),
            0x6d0 | 0x6e0 | 0x6f0 => (0x7c0, 80, 3),
            0x800 => (0x810, 70, 3),
            0x8a0 => (0x8b0, 100, 3),
            0x8e0 => (0x8f0, 70, 4),
            0x9d0 => (0x9e0, 50, 3),
            _ => return None,
        };
        Some(Self {
            offset,
            chance,
            priority,
            range: if matches!(offset, 0x190 | 0x810) {
                RESPONSE_RANGE
            } else {
                RECOGNITION_RANGE
            },
        })
    }
}

fn eligible(
    m: &dyn GuestMemory,
    source_actor: Option<u32>,
    actor: u32,
    distance: f32,
    offset: usize,
) -> Result<bool, MemFault> {
    match offset {
        0x7d0 => Ok(distance > RECOGNITION_RANGE),
        0x810 => {
            Ok(search::behavior(m, actor)? == tactics::GUARD && tactics::awareness(m, actor)? < 4)
        }
        0x8b0 => {
            let Some(source_actor) = source_actor else {
                return Ok(false);
            };
            let target = cast::focus_object(m, source_actor)?;
            Ok(target != NONE && cast::focus_object(m, actor)? == target)
        }
        0x8f0 => {
            let Some(source_actor) = source_actor else {
                return Ok(false);
            };
            let squad = m.u32(source_actor + search::SQUAD_INDEX)?;
            Ok(squad != NONE && m.u32(actor + search::SQUAD_INDEX)? == squad)
        }
        0x9e0 => {
            Ok(tactics::awareness(m, actor)? >= 7 && search::behavior(m, actor)? != tactics::FLEE)
        }
        _ => Ok(true),
    }
}

fn responder(
    m: &dyn GuestMemory,
    unit: u32,
    source: cast::Unit,
    reply: Reply,
) -> Result<Option<u32>, MemFault> {
    let source_actor = match cast::actor_handle(m, unit)? {
        Some(handle) => datum(m, cast::ACTORS, handle)?,
        None => None,
    };
    let source_encounter = match source_actor {
        Some(actor) => search::encounter(m, actor)?,
        None => None,
    };
    let mut nearest = reply.range;
    let pool = DatumArray::at(m.u32(cast::ACTORS)?);
    let mut next = 0;
    let mut best = None;
    while let Some(index) = pool.next_index(m, next)? {
        next = index + 1;
        let Some(actor) = pool.get_absolute(m, index)? else {
            continue;
        };
        let speaker = m.u32(actor + cast::ACTOR_UNIT)?;
        if speaker == unit
            || (source_encounter.is_some() && search::encounter(m, actor)? != source_encounter)
        {
            continue;
        }
        let Some(candidate) = cast::Unit::read(m, speaker)? else {
            continue;
        };
        let distance = cast::distance(source.pos, candidate.pos);
        if distance < nearest
            && cast::allied(m, source.team, candidate.team)?
            && visibility::in_pvs(m, source.cluster, candidate.cluster)?
            && eligible(m, source_actor, actor, distance, reply.offset)?
            && dialogue::source_offset_row(m, speaker, reply.offset)?.is_some()
        {
            best = Some(speaker);
            nearest = distance;
        }
    }
    Ok(best)
}

fn queue_reply(
    m: &mut dyn GuestMemory,
    queue: Queue,
    speaker: u32,
    reply: Reply,
) -> Result<bool, MemFault> {
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock == 0 || (reply.chance < 100 && cast::draw_voice(m, 100)? >= reply.chance) {
        return Ok(false);
    }
    let delay = u32::from(m.u16(clock + 2)?).div_ceil(2);
    let due = m.u32(clock + 8)?.wrapping_add(delay).max(1);
    queue.push_at(m, speaker, reply.offset, reply.priority, due)?;
    Ok(true)
}

fn silent_player_reply(
    m: &mut dyn GuestMemory,
    queue: Queue,
    player: u32,
    offset: usize,
    chance: usize,
) -> Result<(), MemFault> {
    let Some(source) = cast::Unit::read(m, player)? else {
        return Ok(());
    };
    let reply = Reply {
        offset,
        chance,
        priority: 3,
        range: RECOGNITION_RANGE,
    };
    if let Some(speaker) = responder(m, player, source, reply)? {
        queue_reply(m, queue, speaker, reply)?;
    }
    Ok(())
}

pub(super) fn player_spree(
    m: &mut dyn GuestMemory,
    queue: Queue,
    player: u32,
) -> Result<(), MemFault> {
    silent_player_reply(m, queue, player, map::H1_KILLING_SPREE_REPLY_LINE, 100)
}

pub(super) fn player_kill(
    m: &mut dyn GuestMemory,
    queue: Queue,
    player: u32,
    category: u16,
) -> Result<(), MemFault> {
    let (offset, chance) = match category {
        3 => (0x560, 90),
        7 => (0x590, 90),
        9 => (0x5b0, 80),
        _ => return Ok(()),
    };
    silent_player_reply(m, queue, player, offset, chance)
}

pub(super) fn on_speech_tick(
    m: &mut dyn GuestMemory,
    queue: Queue,
    unit: u32,
) -> Result<(), MemFault> {
    let Some(speech) = dialogue::speech(m, unit)? else {
        return Ok(());
    };
    // The native update decrements the remaining play time and clears
    // the instance at zero. Only this tick can dispatch a reply.
    if m.u16(speech + 0xc)? == 0
        || m.u8(speech + 0x48)? == 0
        || m.u16(speech + 0x4c)? != 1
        || m.u32(speech + 0x54)? == NONE
    {
        return Ok(());
    }
    let index = m.u16(speech + 0xe)?;
    let Some(offset) = dialogue::offset_at_row(m, m.u32(speech)?, index)? else {
        return Ok(());
    };
    if !reactions::ai_vocalizes(m)? {
        return Ok(());
    }
    let directed_reply = match offset {
        0x1e0 => Some((0x1f0, 70)),
        0x7e0 => Some((0x7f0, 70)),
        0x820 => Some((0x830, 50)),
        _ => None,
    };
    if let Some((offset, chance)) = directed_reply {
        let other = queue.reply_to(m, unit, index)?;
        if cast::Unit::read(m, other)?.is_some()
            && !cast::is_player(m, other)?
            && dialogue::source_offset_row(m, other, offset)?.is_some()
        {
            let reply = Reply {
                offset,
                chance,
                priority: 3,
                range: 0.0,
            };
            queue_reply(m, queue, other, reply)?;
        }
        return Ok(());
    }
    let Some(reply) = Reply::after(offset) else {
        return Ok(());
    };
    let Some(source) = cast::Unit::read(m, unit)? else {
        return Ok(());
    };
    if offset == 0x360 && queue.reply_to(m, unit, index)? == 3 {
        let grenade = Reply {
            offset: 0x560,
            chance: 50,
            ..reply
        };
        if let Some(speaker) = responder(m, unit, source, grenade)?
            && queue_reply(m, queue, speaker, grenade)?
        {
            return Ok(());
        }
    }
    if let Some(speaker) = responder(m, unit, source, reply)?
        && queue_reply(m, queue, speaker, reply)?
    {
        return Ok(());
    }
    if reply.offset == 0x7c0 {
        let distant = Reply {
            offset: 0x7d0,
            range: RESPONSE_RANGE,
            ..reply
        };
        if let Some(speaker) = responder(m, unit, source, distant)? {
            queue_reply(m, queue, speaker, distant)?;
        }
    }
    Ok(())
}
