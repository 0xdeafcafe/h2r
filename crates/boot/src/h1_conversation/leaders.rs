//! Apply the source squad's leader rule at the native placement wave and
//! assign the resulting voice to the unit constructor's salted datum.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use hs::h1_conversation::ACTOR_CAPACITY;
use runtime::guest::{Access, FaultReason};

use super::{cast, voices, *};

const AI_GLOBALS: u32 = 0x004f_55d0;
const SQUADS: u32 = 0x0051_e9d8;
const FIRST_ACTOR: u32 = 0x68;
const NEXT_ACTOR: u32 = 0x20;
const PLACEMENT_RETURN: u32 = 0x0020_1917;
const WAVE_REMAINING: u32 = 12 + 0x34;

#[derive(Default)]
struct Encounter {
    living: u32,
    unique_voices: u32,
}

fn unique_voice<E>(
    mode: u16,
    encounter: &Encounter,
    draw: impl FnOnce() -> Result<i16, E>,
) -> Result<i16, E> {
    match mode {
        0 if (encounter.unique_voices == 0 && encounter.living >= 4)
            || (encounter.unique_voices == 1 && encounter.living >= 10) =>
        {
            draw()
        }
        2 => draw(),
        3 => Ok(100),
        4 => Ok(101),
        _ => Ok(0),
    }
}

fn encounter(
    m: &dyn GuestMemory,
    registry: voices::Registry,
    table: voices::Table,
    id: u16,
    spawning: u32,
) -> Result<Encounter, MemFault> {
    let mut result = Encounter {
        living: spawning,
        unique_voices: 0,
    };
    let squads = m.u32(SQUADS)?;
    if squads == 0 {
        return Ok(result);
    }
    for index in 0..registry.squads {
        let Some(squad) = registry.squad(m, index as u16)? else {
            continue;
        };
        if squad.encounter != id {
            continue;
        }
        let Some(squad) = DatumArray::at(squads).get_absolute(m, index as i32)? else {
            continue;
        };
        let mut handle = m.u32(squad + FIRST_ACTOR)?;
        for _ in 0..ACTOR_CAPACITY {
            let Some(actor) = datum(m, cast::ACTORS, handle)? else {
                break;
            };
            let unit = m.u32(actor + cast::ACTOR_UNIT)?;
            if cast::Unit::read(m, unit)?.is_some() {
                result.living += 1;
                result.unique_voices += u32::from(table.get(m, unit)?.leader);
            }
            handle = m.u32(actor + NEXT_ACTOR)?;
        }
    }
    Ok(result)
}

fn wave_size(
    m: &dyn GuestMemory,
    squad: u16,
    location: u16,
    requested: i16,
) -> Result<u32, MemFault> {
    if location != u16::MAX {
        return Ok(1);
    }
    if requested != -1 {
        return Ok(requested.max(0) as u32);
    }
    let scenario = m.u32(points::SCENARIO.base)?;
    let row = m.u32(scenario + 0x164)? + u32::from(squad) * 0x74;
    let easy = i32::from(m.i16(row + 0x2c)?);
    let hard = i32::from(m.i16(row + 0x2e)?);
    let game = m.u32(points::GAME_GLOBALS.base)?;
    let count = match m.u16(game + 0x132)? {
        0 | 1 => easy,
        2 => (easy + hard) / 2,
        _ => hard,
    };
    Ok(count.max(0) as u32)
}

#[derive(Clone, Copy)]
struct Wave {
    count: u32,
    voice: i16,
}

#[derive(Clone, Copy)]
struct Birth {
    bank: u16,
    voice: i16,
}

#[derive(Default)]
struct InFlight {
    waves: BTreeMap<(u32, u32), Wave>,
    births: BTreeMap<(u32, u32), Option<Birth>>,
}

fn parent<T>(map: &mut BTreeMap<(u32, u32), T>, thread: u32, esp: u32) -> Option<&mut T> {
    map.range_mut((thread, esp.saturating_add(1))..=(thread, u32::MAX))
        .next()
        .map(|(_, data)| data)
}

fn guest_fault(fault: MemFault) -> runtime::GuestFault {
    runtime::GuestFault {
        addr: fault.addr,
        len: fault.len as usize,
        access: if fault.write {
            Access::Write
        } else {
            Access::Read
        },
        reason: FaultReason::InvalidOperation,
    }
}

pub(super) fn install(b: &mut Builder, pending: u32) {
    let calls = Arc::new(Mutex::new(InFlight::default()));
    let state = Arc::clone(&calls);
    b.raw().before(
        &points::HS_CONVERSATION_SPAWN_WAVE,
        "choose the squad leader voice",
        move |call| {
            let (thread, esp, requested) = (call.ctx.thread_id, call.cpu.esp, call.cpu.eax as i16);
            let m = &mut *call.ctx.runtime().memory();
            let wave = (|| {
                let Some(r) = voices::Registry::read(m)? else {
                    return Ok::<_, MemFault>(None);
                };
                let ai = m.u32(AI_GLOBALS)?;
                if ai == 0 || m.u8(ai + 1)? == 0 {
                    return Ok(None);
                }
                let squad = m.u16(esp + 4)?;
                let Some(s) = r.squad(m, squad)? else {
                    return Ok(None);
                };
                let count = wave_size(m, squad, m.u16(esp + 8)?, requested)?;
                if count == 0 || s.rule == 1 {
                    return Ok(None);
                }
                let e = if s.rule == 0 {
                    encounter(
                        m,
                        r,
                        voices::Table::current(m, r, pending)?,
                        s.encounter,
                        count,
                    )?
                } else {
                    Encounter::default()
                };
                let voice = unique_voice(s.rule, &e, || {
                    Ok::<_, MemFault>(100 + cast::draw_voice(m, 2)? as i16)
                })?;
                Ok(Some(Wave { count, voice }))
            })()
            .map_err(guest_fault)?;
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            state.waves.remove(&(thread, esp));
            if let Some(wave) = wave {
                state.waves.insert((thread, esp), wave);
            }
            Ok(Flow::Continue)
        },
    );

    let state = Arc::clone(&calls);
    b.raw().after(
        &points::HS_CONVERSATION_SPAWN_WAVE,
        "complete the squad placement wave",
        move |call| {
            let key = (call.ctx.thread_id, call.cpu.esp.wrapping_sub(20));
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .waves
                .remove(&key);
            Ok(())
        },
    );

    let state = Arc::clone(&calls);
    b.raw().before(
        &points::AI_SPAWN_AT_SQUAD_START,
        "select the source unit's voice bank",
        move |call| {
            let (thread, esp, start) = (call.ctx.thread_id, call.cpu.esp, call.cpu.ecx as u16);
            let m = &*call.ctx.runtime().memory();
            let Some(r) = voices::Registry::read(m).map_err(guest_fault)? else {
                return Ok(Flow::Continue);
            };
            let squad = m.u16(esp + 4).map_err(guest_fault)?;
            let bank = match r.squad(m, squad).map_err(guest_fault)? {
                Some(squad) => r.spawn_bank(m, squad, start).map_err(guest_fault)?,
                None => hs::h1_conversation::spawn::NO_BANK,
            };
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            let mut voice = 0;
            if m.u32(esp).map_err(guest_fault)? == PLACEMENT_RETURN
                && let Some(wave) = parent(&mut state.waves, thread, esp)
                && m.u32(esp + WAVE_REMAINING).map_err(guest_fault)? == wave.count
            {
                voice = wave.voice;
            }
            state
                .births
                .insert((thread, esp), Some(Birth { bank, voice }));
            Ok(Flow::Continue)
        },
    );

    let state = Arc::clone(&calls);
    b.raw().after(
        &points::AI_SPAWN_AT_SQUAD_START,
        "complete the actor placement",
        move |call| {
            let key = (call.ctx.thread_id, call.cpu.esp.wrapping_sub(12));
            let birth = state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .births
                .remove(&key)
                .flatten();
            // Some placement paths attach an actor to an existing unit.
            // A new unit already consumed this context in its constructor.
            let Some(Birth { voice, .. }) = birth.filter(|birth| birth.voice != 0) else {
                return Ok(());
            };
            let m = &mut *call.ctx.runtime().memory();
            (|| {
                let Some(r) = voices::Registry::read(m)? else {
                    return Ok::<_, MemFault>(());
                };
                let Some(actor) = datum(m, cast::ACTORS, call.cpu.eax)? else {
                    return Ok(());
                };
                let unit = m.u32(actor + cast::ACTOR_UNIT)?;
                if object_address(m, unit)?.is_some() {
                    let table = voices::Table::current(m, r, pending)?;
                    table.put(
                        m,
                        unit,
                        voices::Profile {
                            voice,
                            leader: true,
                        },
                    )?;
                }
                Ok(())
            })()
            .map_err(guest_fault)
        },
    );

    let state = Arc::clone(&calls);
    b.raw().before(
        &points::HS_CONVERSATION_UNIT_NEW,
        "assign the new unit's voice",
        move |call| {
            let (thread, esp) = (call.ctx.thread_id, call.cpu.esp);
            let m = &mut *call.ctx.runtime().memory();
            (|| {
                let Some(r) = voices::Registry::read(m)? else {
                    return Ok::<_, MemFault>(());
                };
                let unit = m.u32(esp + 4)?;
                let Some(body) = object_address(m, unit)? else {
                    return Ok(());
                };
                let birth = {
                    let mut calls = state.lock().unwrap_or_else(|e| e.into_inner());
                    parent(&mut calls.births, thread, esp).and_then(Option::take)
                };
                let (bank, voice) = match birth {
                    Some(birth) => (birth.bank, birth.voice),
                    None => (r.bank_for_tag(m, m.u32(body)?)?, 0),
                };
                let table = voices::Table::current(m, r, pending)?;
                voices::bind(m, r, table, pending + 4, unit, bank, voice)
            })()
            .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );

    b.raw().after(
        &points::HS_NEW_MAP,
        "publish early unit voices",
        move |call| voices::init(&mut *call.ctx.runtime().memory(), pending).map_err(guest_fault),
    );
    b.raw().before(
        &points::HS_CONVERSATION_MAP_DISPOSE,
        "clear the voice bootstrap",
        move |call| {
            let mut state = calls.lock().unwrap_or_else(|e| e.into_inner());
            state.waves.clear();
            state.births.clear();
            drop(state);
            let mut m = call.ctx.runtime().memory();
            m.set_u32(pending, 0)?;
            m.write_bytes(pending + 8, &[0; voices::BOOTSTRAP_BYTES as usize - 8], 1)?;
            Ok(Flow::Continue)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_speaker_rules_use_source_thresholds_and_fixed_personas() {
        let mut e = Encounter {
            living: 3,
            unique_voices: 0,
        };
        assert_eq!(unique_voice::<()>(0, &e, || panic!()), Ok(0));
        e.living = 4;
        assert_eq!(unique_voice::<()>(0, &e, || Ok(101)), Ok(101));
        e.unique_voices = 1;
        assert_eq!(unique_voice::<()>(0, &e, || panic!()), Ok(0));
        e.living = 10;
        assert_eq!(unique_voice::<()>(0, &e, || Ok(100)), Ok(100));
        assert_eq!(unique_voice::<()>(1, &e, || panic!()), Ok(0));
        assert_eq!(unique_voice::<()>(2, &e, || Ok(101)), Ok(101));
        assert_eq!(unique_voice::<()>(3, &e, || panic!()), Ok(100));
        assert_eq!(unique_voice::<()>(4, &e, || panic!()), Ok(101));
    }
}
