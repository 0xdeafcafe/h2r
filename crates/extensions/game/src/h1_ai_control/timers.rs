//! Converted Halo 1 combat holds run while the stock perception and unit
//! control code continue. The timer and its reveal flag belong to the squad.

use std::collections::{BTreeMap, BTreeSet};

use guest_layout::{DatumHandle, entry::Cx, memory::{GuestMemory, MemFault}, view::objects};
use hs::h1_ai_slots::timers::{
    self, BLOCKS, HEADER, REVEAL_PLAYERS, SQUADS, TIME_UNITS_PER_SECOND, UPDATE_TIME,
};

use super::{NONE, actors, contact, reports, squad_control, squads, state, uncover_limits};

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
    if !phased {
        return Ok(m.u16(actor.at + COMBAT_STATUS)? >= VISIBLE_ENEMY);
    }
    if contact::enabled(m)? && let Some((tick, rate)) = state::clock(m)? {
        let now = state::authored_tick(tick, rate);
        return Ok(reports::forced(m, actor, now)? || contact::focused(m, actor, rate)?.releases_timer());
    }
    Ok(legacy_combat_level(m.u16(actor.at + INTEREST_CLASS)?) >= RELEASE_COMBAT_LEVEL)
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

/// The Halo 1 movement hold: the squad's timer is still counting and no
/// member has reached combat level 5 (or a forced level, the event reports
/// of `0x001506e7` supply). Halo 1 checks this at the individual behaviour
/// call sites, not only on the resulting throttle.
pub(super) fn held(m: &dyn GuestMemory, actor: actors::Actor) -> R<bool> {
    let Some(base) = state::timers_base(m)? else {
        return Ok(false);
    };
    if let Some(at) = timer_row(m, base, actor.squad)?
        && timers::active(m.u32(at)?)
        && !releases(m, actor, state::phased_timers(m)?)?
    {
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn hold_movement(m: &mut dyn GuestMemory, handle: u32) -> R<()> {
    let Some(actor) = actors::by_handle(m, DatumHandle(handle))? else {
        return Ok(());
    };
    if held(m, actor)? {
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
        // First pass: each actor's own contact, and the strongest alert
        // report its encounter produced (Halo 1 0x14e8a0 copies the alert
        // bits and sighting timestamps between the linked units).
        let mut spread = BTreeMap::new();
        let mut population = Vec::new();
        for actor in actors::all(m)? {
            if actor.squad == NONE {
                continue;
            }
            if phased {
                // The phased bridge walks only living units, as before;
                // the legacy ladder below keeps its wider population.
                let Some((_, unit)) = objects::get(m, DatumHandle(actor.unit))? else {
                    continue;
                };
                if unit.damage_flags(m)? & 4 != 0 {
                    continue;
                }
            }
            let baseline = guard_baseline(m, guard_bits, actor.squad)?;
            let forced = source_contact && reports::forced(m, actor, now)?;
            let mut own = if phased {
                if source_contact {
                    contact::focused(m, actor, rate)?
                } else {
                    let level = legacy_combat_level(m.u16(actor.at + INTEREST_CLASS)?);
                    contact::Contact {
                        level,
                        recognized_at: None,
                        alarm: false,
                    }
                }
            } else {
                let status = m.u16(actor.at + COMBAT_STATUS)?;
                let level = if status >= VISIBLE_ENEMY {
                    RELEASE_COMBAT_LEVEL
                } else if status >= FIRST_ENEMY_NOTICE {
                    1
                } else {
                    0
                };
                contact::Contact {
                    level,
                    recognized_at: None,
                    alarm: false,
                }
            };
            if forced {
                // Halo 1's forced combat level (0x1506e7) raises the actor
                // to the release level and alerts it, report or not.
                own.level = own.level.max(RELEASE_COMBAT_LEVEL);
                own.alarm = true;
            }
            let group = squad_control::encounter(m, actor.squad)?;
            population.push((actor, baseline, own, group));
            reports::collect(&mut spread, group, own);
        }
        for (actor, baseline, own, group) in population {
            let mut contact = own;
            if let Some(group) = group
                && let Some(report) = spread.get(&group)
            {
                contact = reports::merge(contact, *report);
            }
            let (informed, releases_hold) = if phased && source_contact {
                (
                    uncover_limits::timer_starts(m, actor, contact, baseline)?,
                    contact.releases_timer(),
                )
            } else {
                (
                    contact.level > baseline,
                    contact.level >= RELEASE_COMBAT_LEVEL,
                )
            };
            if informed && let Some(group) = group {
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
    use guest_layout::testing::FakeGuest;
    use hooks::points;

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
        assert_eq!(
            timers::encounter_updates(
                Some(state::authored_tick(33, 60)),
                state::authored_tick(34, 60),
                2
            ),
            1
        );
        assert_eq!(
            timers::encounter_updates(
                Some(state::authored_tick(34, 60)),
                state::authored_tick(35, 60),
                2
            ),
            0
        );
    }

    fn timers_guest(value: u32) -> (FakeGuest, actors::Actor) {
        const GLOBALS: u32 = 0x004f_9380;
        const ARRAY: u32 = 0x0083_0000;
        const VALUES: u32 = 0x0084_0000;
        const SCENARIO: u32 = 0x0085_0000;
        const DECLARATIONS: u32 = 0x0086_0000;
        const ACTORS: u32 = 0x0087_0000;
        const ACTOR_ROWS: u32 = 0x0088_0000;
        const CLOCK: u32 = 0x0089_0000;
        // One squad's table, with the guard masks appended as the compiler
        // does, and the phased marker global holding base 0 at the end.
        let mut words = timers::build([(0, 0x8, 2.0)]).unwrap();
        words[timers::HEADER as usize] = value;
        words.extend(std::iter::repeat_n(0, timers::BLOCKS as usize));
        let mut guest = FakeGuest::new();
        guest
            .put_u32(GLOBALS, ARRAY)
            .put_u32(ARRAY + guest_layout::datum::layout::MAXIMUM_COUNT, 0x1000)
            .put_u32(ARRAY + guest_layout::datum::layout::DATA, VALUES)
            .put_u32(SCENARIO + 0x1c0, (words.len() + 1) as u32)
            .put_u32(SCENARIO + 0x1c4, DECLARATIONS)
            .put(
                DECLARATIONS + 40 * words.len() as u32,
                timers::PHASED_GLOBAL.as_bytes(),
            )
            .put_u32(points::SCENARIO.base, SCENARIO)
            .put_u32(points::GAME_TIME.base, CLOCK)
            .put_u16(CLOCK + 2, 30)
            .put_u32(CLOCK + 8, 600);
        for (index, word) in words.iter().enumerate() {
            guest.put_u32(VALUES + (state::VALUE_START + index as u32) * 8 + 4, *word);
        }
        guest.put_u32(
            VALUES + (state::VALUE_START + words.len() as u32) * 8 + 4,
            0,
        );
        guest
            .put_u32(actors::ACTORS, ACTORS)
            .put_u32(ACTORS + guest_layout::datum::layout::ELEMENT_SIZE, 0x100)
            .put_u32(ACTORS + guest_layout::datum::layout::HIGH_WATER, 1)
            .put_u32(ACTORS + guest_layout::datum::layout::DATA, ACTOR_ROWS)
            .put_u16(ACTOR_ROWS, 0x8123)
            .put_u32(ACTOR_ROWS + 0x18, NONE)
            .put_u32(ACTOR_ROWS + 0x30, 0)
            .put_u16(ACTOR_ROWS + INTEREST_CLASS, 0);
        let actor = actors::Actor {
            handle: 0x8123_0000,
            at: ACTOR_ROWS,
            unit: NONE,
            squad: 0,
        };
        (guest, actor)
    }

    #[test]
    fn the_hold_covers_behaviour_updates_until_the_timer_ends() {
        let (guest, actor) = timers_guest(timers::SCRIPT_HELD | 900);
        assert!(held(&guest, actor).unwrap());
        let (guest, actor) = timers_guest(timers::REVEAL_PLAYERS);
        assert!(!held(&guest, actor).unwrap());
    }
}
