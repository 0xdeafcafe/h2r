//! Halo 1 follow-driven migration. Participants remain members of the
//! encounter, even when a script disables one as a destination.

use std::collections::BTreeSet;

use guest_layout::{DatumHandle, entry::Cx, memory::{GuestMemory, MemFault}, view::objects};
use hs::h1_ai_slots::{FOLLOW_WORDS, MIGRATION_WORDS, Registry};

use super::{
    actors::{self, Actor},
    points, squad_control, squads, state,
};

const WORDS: u32 = Registry::Follow.words() as u32;
pub(super) const ALLOWED: u32 = FOLLOW_WORDS as u32;
pub(super) const ENABLED: u32 = ALLOWED + MIGRATION_WORDS as u32;

type R<T> = Result<T, MemFault>;

fn bit(start: u32, section: u32, squad: u32) -> Option<(u32, u32)> {
    let word = squad / 32;
    (word < u32::from(MIGRATION_WORDS))
        .then_some((start + (section + word) * state::STRIDE, 1 << (squad % 32)))
}

pub(super) fn has(m: &dyn GuestMemory, start: u32, section: u32, squad: u32) -> R<bool> {
    let Some((at, mask)) = bit(start, section, squad) else {
        return Ok(false);
    };
    Ok(m.u32(at)? & mask != 0)
}

pub(super) fn set(cx: &Cx, reference: u32, enabled: bool, base: u32) -> R<()> {
    cx.write(|m| {
        let Some(start) = state::registry(m, base, WORDS)? else {
            return Ok(());
        };
        super::follow::set_virtual_destinations(m, start, reference, enabled)?;
        for squad in squads(m, reference)? {
            if let Some((at, mask)) = bit(start, ENABLED, squad) {
                let old = m.u32(at)?;
                m.set_u32(at, if enabled { old | mask } else { old & !mask })?;
            }
        }
        Ok(())
    })
}

pub(super) fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

pub(super) fn anchor(
    m: &dyn GuestMemory,
    population: &[Actor],
    candidates: &[[f32; 3]],
) -> R<Option<[f32; 3]>> {
    if candidates.len() <= 1 {
        return Ok(candidates.first().copied());
    }
    let mut nearest = None::<(f32, [f32; 3])>;
    for actor in population {
        let Some((_, unit)) = objects::get(m, DatumHandle(actor.unit))? else {
            continue;
        };
        let position = unit.position(m)?;
        for target in candidates.iter().take(8).copied() {
            let d = distance(position, target);
            if d.is_finite() && nearest.is_none_or(|(old, _)| d < old) {
                nearest = Some((d, target));
            }
        }
    }
    Ok(nearest.map(|(_, target)| target))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Destination {
    pub squad: u32,
    pub order: u16,
}

fn nearest_firing_position(
    m: &dyn GuestMemory,
    destination: Destination,
    anchor: [f32; 3],
) -> R<Option<f32>> {
    let Some(runtime) = squad_control::runtime(m, destination.squad)? else {
        return Ok(None);
    };
    let order = u32::from(destination.order);
    let scenario = m.u32(points::SCENARIO.base)?;
    if order >= m.u32(scenario + 0x240)? {
        return Ok(None);
    }
    let order = m.u32(scenario + 0x244)? + order * 0x7c;
    let area_set = order
        + if m.u16(runtime + 0x2a)? != destination.order || m.u8(runtime + 0x60)? == 0 {
            0x54
        } else {
            0x5c
        };
    let zones = m.u32(scenario + 0x16c)?;
    let zone_count = m.u32(scenario + 0x168)?;
    let mut nearest = f32::INFINITY;
    let areas = m.u32(area_set + 4)?;
    for index in 0..m.u32(area_set)? {
        let reference = areas + index * 8;
        let zone = u32::from(m.u16(reference + 4)?);
        let area = u32::from(m.u16(reference + 6)?);
        if zone >= zone_count {
            continue;
        }
        let zone = zones + zone * 0x38;
        if area >= m.u32(zone + 0x30)? {
            continue;
        }
        let area = m.u32(zone + 0x34)? + area * 0x88;
        let first = u32::from(m.u16(area + 0x38)?);
        let count = u32::from(m.u16(area + 0x3a)?);
        if first + count > m.u32(zone + 0x28)? {
            continue;
        }
        let positions = m.u32(zone + 0x2c)?;
        for position in first..first + count {
            let d = distance(anchor, m.vec3(positions + position * 0x20)?);
            if d.is_finite() {
                nearest = nearest.min(d);
            }
        }
    }
    Ok(nearest.is_finite().then(|| nearest.sqrt()))
}

pub(super) fn destination<T: Copy>(choices: &[(T, f32, bool)], radius: f32) -> Option<T> {
    let &(dest, nearest, _) = choices.iter().min_by(|a, b| a.1.total_cmp(&b.1))?;
    let farthest = choices
        .iter()
        .filter(|c| c.2)
        .map(|c| c.1)
        .max_by(f32::total_cmp);
    farthest
        .is_none_or(|old| old - nearest > radius)
        .then_some(dest)
}

pub(super) struct Migration {
    pub destination: Destination,
    pub actors: Vec<Actor>,
}

pub(super) fn read_destinations(m: &dyn GuestMemory, at: u32) -> R<Vec<Destination>> {
    if at == super::NONE {
        return Ok(Vec::new());
    }
    let parse = hs::h1_ai_slots::decode_words;
    let Some(length) = parse(&m.bytes(at, 4)?).and_then(|words| words.first().copied()) else {
        return Ok(Vec::new());
    };
    let Some(words) = parse(&m.bytes(at + 4, usize::from(length))?) else {
        return Ok(Vec::new());
    };
    if words.len() % 2 != 0 {
        return Ok(Vec::new());
    }
    Ok(words.chunks_exact(2).map(|pair| Destination {
        squad: u32::from(pair[0]), order: pair[1],
    }).collect())
}

pub(super) fn plan(
    m: &dyn GuestMemory,
    start: u32,
    members: &[u32],
    deferred: &[Destination],
    anchors: &[[f32; 3]],
    radius: f32,
) -> R<Option<Migration>> {
    if anchors.is_empty() {
        return Ok(None);
    }
    let carriers: BTreeSet<_> = deferred.iter().filter(|d| members.contains(&d.squad))
        .map(|d| d.squad).collect();
    let mut participants = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut occupied_orders = std::collections::BTreeMap::new();
    for &squad in members {
        let allowed = has(m, start, ALLOWED, squad)?;
        if !allowed && !carriers.contains(&squad) {
            continue;
        }
        let (Some(definition), Some(runtime)) = (
            actors::squad_record(m, squad)?,
            squad_control::runtime(m, squad)?,
        ) else {
            continue;
        };
        let current = m.u16(runtime + 0x2a)?;
        let initial = m.u16(definition + 0x42)?;
        let virtual_order = deferred.contains(&Destination { squad, order: current });
        if allowed || virtual_order {
            participants.insert(squad);
            occupied_orders.insert(squad, if current == u16::MAX { initial } else { current });
        }
        if allowed && has(m, start, ENABLED, squad)? {
            // A carrier's authored order stays eligible after a migration
            // has installed one of the orders it hosts.
            let order = if current == u16::MAX || virtual_order { initial } else { current };
            candidates.push(Destination { squad, order });
        }
    }
    candidates.extend(deferred.iter().filter(|entry| carriers.contains(&entry.squad)).copied());
    let population: Vec<_> = actors::all(m)?
        .into_iter()
        .filter(|actor| participants.contains(&actor.squad))
        .collect();
    if population.is_empty() || candidates.is_empty() {
        return Ok(None);
    }
    let Some(anchor) = anchor(m, &population, anchors)? else {
        return Ok(None);
    };
    let mut choices = Vec::new();
    for candidate in candidates {
        if let Some(distance) = nearest_firing_position(m, candidate, anchor)? {
            choices.push((
                candidate,
                distance,
                population.iter().any(|actor| {
                    actor.squad == candidate.squad
                        && (occupied_orders.get(&actor.squad).copied() == Some(candidate.order)
                            || !carriers.contains(&actor.squad))
                }),
            ));
        }
    }
    let Some(destination) = destination(&choices, radius) else {
        return Ok(None);
    };
    let actors = population.into_iter().filter(|actor| actor.squad != destination.squad).collect();
    Ok(Some(Migration { destination, actors }))
}

pub(super) async fn apply(cx: &Cx, migration: Migration) -> Result<(), guest_layout::entry::EntryError> {
    let dest = migration.destination;
    let current = cx.read(|m| {
        squad_control::runtime(m, dest.squad)?.map(|at| m.u16(at + 0x2a)).transpose()
    })?;
    if current != Some(dest.order) {
        cx.call(guest_layout::entry::GuestCall::new(0x0027_5cb0)
            .eax(dest.squad).ecx(u32::from(dest.order))).await?;
    }
    for actor in migration.actors {
        actors::move_actor(cx, actor, dest.squad).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_requires_a_better_firing_position_outside_the_follow_radius() {
        let choices = [(17, 5.0, true), (18, 1.0, false)];
        assert_eq!(destination(&choices, 3.5), Some(18));
        assert_eq!(destination(&choices, 4.0), None);
        assert_eq!(destination(&[(17, 5.0, false)], 4.0), Some(17));
    }
}
