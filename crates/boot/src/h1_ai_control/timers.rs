//! Converted Halo 1 combat holds run while the stock perception and unit
//! control code continue. The timer and its reveal flag belong to the squad.

use std::collections::BTreeSet;

use engine::{
    DatumHandle,
    entry::Cx,
    memory::{GuestMemory, MemFault},
    view::objects,
};
use hs::h1_ai_slots::timers::{
    self, BLOCKS, HEADER, REVEAL_PLAYERS, SQUADS, TIME_UNITS_PER_SECOND, UPDATE_TIME,
};

use super::{NONE, actors, contact, squad_control, squads, state, uncover_limits};

const COMBAT_STATUS: u32 = 0x86;
const FIRST_ENEMY_NOTICE: u16 = 3;
const VISIBLE_ENEMY: u16 = 6;
const INTEREST_CLASS: u32 = 0x328;
const RELEASE_COMBAT_LEVEL: u16 = 5;
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

// Previously converted maps carry the phased timer table but no perception
// marker. Preserve their original runtime behavior.
fn legacy_combat_level(interest: u16) -> u16 {
    match interest {
        3 => 1,
        4 | 5 => 2,
        6 => 4,
        7..=10 => 5,
        11 | 12 => 7,
        _ => 0,
    }
}

fn releases(m: &dyn GuestMemory, actor: actors::Actor, phased: bool) -> R<bool> {
    if phased {
        if contact::enabled(m)? && let Some((_, rate)) = state::clock(m)? {
            Ok(contact::focused(m, actor, rate)?.releases_timer())
        } else {
            Ok(legacy_combat_level(m.u16(actor.at + INTEREST_CLASS)?) >= RELEASE_COMBAT_LEVEL)
        }
    } else {
        Ok(m.u16(actor.at + COMBAT_STATUS)? >= VISIBLE_ENEMY)
    }
}

fn guard_baseline(m: &dyn GuestMemory, guard_bits: Option<u32>, squad: u32) -> R<u16> {
    let Some(bits) = guard_bits else {
        return Ok(0);
    };
    if squad >= u32::from(SQUADS) {
        return Ok(0);
    }
    let word = m.u32(bits + (squad / 32) * state::STRIDE)?;
    Ok(if word & (1 << (squad % 32)) != 0 { 2 } else { 0 })
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
        && !releases(m, actor, state::phased_timers(m)?)?
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
        let Some((tick, rate)) = state::clock(m)? else {
            return Ok(Vec::new());
        };
        let previous = m.u32(start)?;
        if tick == previous {
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

        let phased = state::phased_timers(m)?;
        let source_contact = phased && contact::enabled(m)?;
        let guard_bits = if phased {
            let last = start + (1 + (BLOCKS - 1) * 2) * state::STRIDE;
            let rows = m.u32(last + state::STRIDE)? + m.u32(last)?.count_ones();
            state::registry(m, base + HEADER + rows, BLOCKS)?
        } else {
            None
        };
        let now = state::authored_tick(tick, rate);
        let then = (previous != NONE).then(|| state::authored_tick(previous, rate));
        let mut noticed = BTreeSet::new();
        let mut release = BTreeSet::new();
        for actor in actors::all(m)? {
            if actor.squad == NONE {
                continue;
            }
            let (informed, releases_hold) = if phased {
                let Some((_, unit)) = objects::get(m, DatumHandle(actor.unit))? else {
                    continue;
                };
                if unit.damage_flags(m)? & 4 != 0 {
                    continue;
                }
                let baseline = guard_baseline(m, guard_bits, actor.squad)?;
                if source_contact {
                    let focused = contact::focused(m, actor, rate)?;
                    (
                        uncover_limits::timer_starts(m, actor, focused, baseline)?,
                        focused.releases_timer(),
                    )
                } else {
                    let level = legacy_combat_level(m.u16(actor.at + INTEREST_CLASS)?);
                    (level > baseline, level >= RELEASE_COMBAT_LEVEL)
                }
            } else {
                let status = m.u16(actor.at + COMBAT_STATUS)?;
                (status >= FIRST_ENEMY_NOTICE, status >= VISIBLE_ENEMY)
            };
            if informed && let Some(group) = squad_control::encounter(m, actor.squad)? {
                noticed.insert(group);
            }
            if releases_hold {
                release.insert(actor.squad);
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
                let group = squad_control::encounter(m, squad)?;
                let alert = group.is_some_and(|group| noticed.contains(&group));
                let contact = release.contains(&squad);
                let elapsed = if phased {
                    let steps = group.map_or(0, |group| timers::encounter_updates(then, now, group));
                    if steps == 0 && !contact {
                        continue;
                    }
                    steps.saturating_mul(UPDATE_TIME)
                } else {
                    elapsed
                };
                let next = timers::advance(value, elapsed, alert, contact);
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_contact_starts_the_clock_but_does_not_release_the_hold() {
        assert_eq!(legacy_combat_level(6), 4);
        assert!(legacy_combat_level(6) < RELEASE_COMBAT_LEVEL);
        assert!(legacy_combat_level(7) >= RELEASE_COMBAT_LEVEL);
        assert_eq!(legacy_combat_level(2), 0);
    }

    #[test]
    fn the_same_encounter_phase_is_used_at_both_frame_rates() {
        assert_eq!(state::authored_tick(34, 60), state::authored_tick(17, 30));
        assert_eq!(timers::encounter_updates(Some(state::authored_tick(33, 60)), state::authored_tick(34, 60), 2), 1);
        assert_eq!(timers::encounter_updates(Some(state::authored_tick(34, 60)), state::authored_tick(35, 60), 2), 0);
    }
}
