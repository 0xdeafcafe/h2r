//! Halo 1 follow-driven migration. Participants remain members of the
//! encounter, even when a script disables one as a destination.

use std::collections::BTreeSet;

use engine::{
    DatumHandle,
    entry::Cx,
    memory::{GuestMemory, MemFault},
    view::objects,
};
use hs::h1_ai_slots::{FOLLOW_WORDS, MIGRATION_WORDS, Registry};

use super::{
    actors::{self, Actor},
    points, squad_control, squads, state,
};

const WORDS: u32 = Registry::Follow.words() as u32;
const ALLOWED: u32 = FOLLOW_WORDS as u32;
const ENABLED: u32 = ALLOWED + MIGRATION_WORDS as u32;

type R<T> = Result<T, MemFault>;

fn bit(start: u32, section: u32, squad: u32) -> Option<(u32, u32)> {
    let word = squad / 32;
    (word < u32::from(MIGRATION_WORDS))
        .then_some((start + (section + word) * state::STRIDE, 1 << (squad % 32)))
}

fn has(m: &dyn GuestMemory, start: u32, section: u32, squad: u32) -> R<bool> {
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
        for squad in squads(m, reference)? {
            if let Some((at, mask)) = bit(start, ENABLED, squad) {
                let old = m.u32(at)?;
                m.set_u32(at, if enabled { old | mask } else { old & !mask })?;
            }
        }
        Ok(())
    })
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

fn anchor(
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

fn nearest_firing_position(m: &dyn GuestMemory, squad: u32, anchor: [f32; 3]) -> R<Option<f32>> {
    let Some(definition) = actors::squad_record(m, squad)? else {
        return Ok(None);
    };
    let Some(runtime) = squad_control::runtime(m, squad)? else {
        return Ok(None);
    };
    let mut order = u32::from(m.u16(runtime + 0x2a)?);
    if order == u32::from(u16::MAX) {
        order = u32::from(m.u16(definition + 0x42)?);
    }
    let scenario = m.u32(points::SCENARIO.base)?;
    if order >= m.u32(scenario + 0x240)? {
        return Ok(None);
    }
    let order = m.u32(scenario + 0x244)? + order * 0x7c;
    let area_set = order
        + if m.u8(runtime + 0x60)? == 0 {
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

fn destination(choices: &[(u32, f32, bool)], radius: f32) -> Option<u32> {
    let &(squad, nearest, _) = choices.iter().min_by(|a, b| a.1.total_cmp(&b.1))?;
    let farthest = choices
        .iter()
        .filter(|c| c.2)
        .map(|c| c.1)
        .max_by(f32::total_cmp);
    farthest
        .is_none_or(|old| old - nearest > radius)
        .then_some(squad)
}

pub(super) fn plan(
    m: &dyn GuestMemory,
    start: u32,
    members: &[u32],
    anchors: &[[f32; 3]],
    radius: f32,
) -> R<Vec<(Actor, u32)>> {
    if anchors.is_empty() {
        return Ok(Vec::new());
    }
    let mut participants = BTreeSet::new();
    let mut candidates = Vec::new();
    for &squad in members {
        if has(m, start, ALLOWED, squad)? {
            participants.insert(squad);
            if has(m, start, ENABLED, squad)? {
                candidates.push(squad);
            }
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let population: Vec<_> = actors::all(m)?
        .into_iter()
        .filter(|actor| participants.contains(&actor.squad))
        .collect();
    if population.is_empty() {
        return Ok(Vec::new());
    }
    let Some(anchor) = anchor(m, &population, anchors)? else {
        return Ok(Vec::new());
    };
    let mut choices = Vec::new();
    for squad in candidates {
        if let Some(distance) = nearest_firing_position(m, squad, anchor)? {
            choices.push((
                squad,
                distance,
                population.iter().any(|actor| actor.squad == squad),
            ));
        }
    }
    let Some(to) = destination(&choices, radius) else {
        return Ok(Vec::new());
    };
    Ok(population
        .into_iter()
        .filter(|actor| actor.squad != to)
        .map(|actor| (actor, to))
        .collect())
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
