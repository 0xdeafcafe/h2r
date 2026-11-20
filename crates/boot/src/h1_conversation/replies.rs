//! Replies triggered by the end of a source utterance (Halo 1 0x47073).

use super::{
    cast, dialogue,
    reactions::{self, Queue},
    search, visibility, *,
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
            _ => return None,
        };
        Some(Self {
            offset,
            chance,
            priority,
            range: if offset == 0x190 {
                RESPONSE_RANGE
            } else {
                RECOGNITION_RANGE
            },
        })
    }
}

fn responder(
    m: &dyn GuestMemory,
    unit: u32,
    source: cast::Unit,
    reply: Reply,
) -> Result<Option<u32>, MemFault> {
    let source_encounter = match cast::actor_handle(m, unit)? {
        Some(id) => match datum(m, cast::ACTORS, id)? {
            Some(actor) => search::encounter(m, actor)?,
            None => None,
        },
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
            && dialogue::source_offset_row(m, speaker, reply.offset)?.is_some()
        {
            best = Some(speaker);
            nearest = distance;
        }
    }
    Ok(best)
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
    let Some(reply) = dialogue::offset_at_row(m, m.u32(speech)?, index)?.and_then(Reply::after)
    else {
        return Ok(());
    };
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock == 0 || !reactions::ai_vocalizes(m)? {
        return Ok(());
    }
    let Some(source) = cast::Unit::read(m, unit)? else {
        return Ok(());
    };
    let Some(speaker) = responder(m, unit, source, reply)? else {
        return Ok(());
    };
    if reply.chance == 100 || cast::draw_voice(m, 100)? < reply.chance {
        let delay = u32::from(m.u16(clock + 2)?).div_ceil(2);
        let due = m.u32(clock + 8)?.wrapping_add(delay).max(1);
        queue.push_at(m, speaker, reply.offset, reply.priority, due)?;
    }
    Ok(())
}
