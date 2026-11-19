//! Converted Halo 1 combat holds run while the stock perception and unit
//! control code continue. The timer and its reveal flag belong to the squad.

use std::collections::BTreeSet;

use engine::{
    DatumHandle,
    entry::Cx,
    memory::{GuestMemory, MemFault},
};
use hs::h1_ai_slots::timers::{
    self, BLOCKS, HEADER, REVEAL_PLAYERS, SQUADS, TIME_UNITS_PER_SECOND,
};

use super::{NONE, actors, points, squad_control, squads, state};

const COMBAT_STATUS: u32 = 0x86;
const FIRST_ENEMY_NOTICE: u16 = 3;
const VISIBLE_ENEMY: u16 = 6;
const THROTTLE: u32 = 0x814;

type R<T> = Result<T, MemFault>;

fn timer_row(m: &dyn GuestMemory, base: u32, squad: u32) -> R<Option<u32>> {
    if squad >= u32::from(SQUADS) {
        return Ok(None);
    }
    let Some(start) = state::registry(m, base, HEADER)? else {
        return Ok(None);
    };
    let block = start + (1 + 2 * (squad / 32)) * state::STRIDE;
    let bit = 1 << (squad % 32);
    let mask = m.u32(block)?;
    if mask & bit == 0 {
        return Ok(None);
    }
    let prior = m.u32(block + state::STRIDE)?;
    let rank = prior + (mask & (bit - 1)).count_ones();
    state::registry(m, base + HEADER + rank, 1)
}

pub(super) fn hold_movement(m: &mut dyn GuestMemory, handle: u32) -> R<()> {
    let Some(base) = state::timers_base(m)? else {
        return Ok(());
    };
    let Some(actor) = actors::by_handle(m, DatumHandle(handle))? else {
        return Ok(());
    };
    if let Some(at) = timer_row(m, base, actor.squad)?
        && timers::active(m.u32(at)?)
        && m.u16(actor.at + COMBAT_STATUS)? < VISIBLE_ENEMY
    {
        m.write(actor.at + THROTTLE, &[0; 12])?;
    }
    Ok(())
}

pub(super) fn expire(cx: &Cx, subject: u32) -> R<Vec<u32>> {
    cx.write(|m| {
        let Some(base) = state::timers_base(m)? else {
            return Ok(Vec::new());
        };
        let mut revealed = Vec::new();
        for squad in squads(m, subject)? {
            if let Some(at) = timer_row(m, base, squad)? {
                let value = timers::expire(m.u32(at)?);
                m.set_u32(at, value)?;
                if value & REVEAL_PLAYERS != 0 {
                    revealed.push(squad);
                }
            }
        }
        Ok(revealed)
    })
}

pub(super) fn tick(cx: &Cx, base: u32) -> R<Vec<u32>> {
    cx.write(|m| {
        let Some(start) = state::registry(m, base, HEADER)? else {
            return Ok(Vec::new());
        };
        let clock = m.u32(points::GAME_TIME.base)?;
        if clock == 0 {
            return Ok(Vec::new());
        }
        let rate = u32::from(m.u16(clock + 2)?);
        let tick = m.u32(clock + 8)?;
        let previous = m.u32(start)?;
        if tick == previous || rate == 0 {
            return Ok(Vec::new());
        }
        let elapsed = if previous == NONE {
            0
        } else {
            tick.wrapping_sub(previous)
                .saturating_mul(TIME_UNITS_PER_SECOND)
                / rate
        };
        m.set_u32(start, tick)?;

        let mut noticed = BTreeSet::new();
        let mut visible = BTreeSet::new();
        for actor in actors::all(m)? {
            if actor.squad == NONE {
                continue;
            }
            let status = m.u16(actor.at + COMBAT_STATUS)?;
            if status >= FIRST_ENEMY_NOTICE
                && let Some(group) = squad_control::encounter(m, actor.squad)?
            {
                noticed.insert(group);
            }
            if status >= VISIBLE_ENEMY {
                visible.insert(actor.squad);
            }
        }

        let mut revealed = Vec::new();
        for block in 0..BLOCKS {
            let at = start + (1 + block * 2) * state::STRIDE;
            let mut mask = m.u32(at)?;
            let mut index = base + HEADER + m.u32(at + state::STRIDE)?;
            while mask != 0 {
                let squad = block * 32 + mask.trailing_zeros();
                mask &= mask - 1;
                let Some(at) = state::registry(m, index, 1)? else {
                    break;
                };
                index += 1;
                let value = m.u32(at)?;
                if !timers::active(value) {
                    continue;
                }
                let alert = squad_control::encounter(m, squad)?
                    .is_some_and(|group| noticed.contains(&group));
                let next = timers::advance(value, elapsed, alert, visible.contains(&squad));
                if next != value {
                    m.set_u32(at, next)?;
                }
                if !timers::active(next) && next & REVEAL_PLAYERS != 0 {
                    revealed.push(squad);
                }
            }
        }
        Ok(revealed)
    })
}
