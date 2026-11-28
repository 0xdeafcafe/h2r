//! Reproduce Halo 1's automatic follow scoring with its authored
//! coordinates. Navigation can use a different BSP from the ranking data.

use std::collections::{BTreeMap, BTreeSet};

use guest_layout::memory::{GuestMemory, MemFault};
use hs::h1_ai_slots::{decode_words, migration as wire};

use super::{actors, auto_migration as migration, points, squad_control, state};

const NO_SQUAD: u16 = u16::MAX;
const NATIVE_ORDER: u16 = u16::MAX;
const GROUPS: usize = 26;

type R<T> = Result<T, MemFault>;

#[derive(Clone, Copy)]
pub(super) struct Data {
    at: u32,
    len: u32,
}

impl Data {
    pub(super) fn from_scenario(m: &dyn GuestMemory, offset: u32) -> R<Option<Self>> {
        let Some((at, remaining)) = state::script_strings(m, offset)? else {
            return Ok(None);
        };
        if remaining < wire::DIRECTORY as u32 {
            return Ok(None);
        }
        Ok(Some(Self {
            at,
            len: remaining,
        }))
    }

    fn directory(
        self,
        m: &dyn GuestMemory,
        group: u32,
    ) -> R<Option<(wire::Location, wire::Location)>> {
        if group as usize >= wire::GROUPS {
            return Ok(None);
        }
        let at = self.at + group * (2 * wire::HEADER) as u32;
        let bytes = m.bytes(at, 2 * wire::HEADER)?;
        Ok(wire::Location::decode(&bytes[..wire::HEADER])
            .zip(wire::Location::decode(&bytes[wire::HEADER..])))
    }
}

fn rows<T>(
    m: &dyn GuestMemory,
    data: Data,
    block: wire::Location,
    width: usize,
    decode: impl Fn(&[u16]) -> Option<T>,
) -> R<Vec<T>> {
    let bytes = usize::from(block.rows) * width;
    if block.rows == 0
        || block.offset < wire::DIRECTORY as u32
        || block.offset > data.len
        || bytes as u32 > data.len - block.offset
    {
        return Ok(Vec::new());
    }
    let bytes = m.bytes(data.at + block.offset, bytes)?;
    let Some(words) = decode_words(&bytes) else {
        return Ok(Vec::new());
    };
    Ok(words.chunks_exact(width / 4).filter_map(decode).collect())
}

struct Squad {
    current: u16,
    initial: u16,
    allowed: bool,
    enabled: bool,
    on_virtual_route: bool,
}

impl Squad {
    fn own_order(&self) -> u16 {
        if self.on_virtual_route || self.current == NATIVE_ORDER {
            self.initial
        } else {
            self.current
        }
    }

    fn occupied_order(&self) -> u16 {
        if self.current == NATIVE_ORDER {
            self.initial
        } else {
            self.current
        }
    }
}

fn defensive_order(m: &dyn GuestMemory, order: u16, initial: bool) -> R<bool> {
    let scenario = m.u32(points::SCENARIO.base)?;
    if u32::from(order) >= m.u32(scenario + 0x240)? {
        return Ok(initial);
    }
    let orders = m.u32(scenario + 0x244)?;
    let name = m.bytes(orders + u32::from(order) * 0x7c, 10)?;
    if name.starts_with(b"h1_defend_") {
        Ok(true)
    } else if name.starts_with(b"h1_auto_") {
        Ok(initial)
    } else {
        Ok(false)
    }
}

fn group_distances(
    points: impl IntoIterator<Item = (usize, [f32; 3])>,
    anchor: [f32; 3],
) -> [f32; GROUPS] {
    let mut distances = [f32::INFINITY; GROUPS];
    for (group, point) in points {
        let distance = migration::distance(anchor, point);
        if distance.is_finite() {
            distances[group] = distances[group].min(distance);
        }
    }
    distances
}

fn nearest(mask: u32, distances: &[f32; GROUPS]) -> Option<f32> {
    let nearest = distances
        .iter()
        .enumerate()
        .filter_map(|(group, &distance)| (mask & (1 << group) != 0).then_some(distance))
        .fold(f32::INFINITY, f32::min);
    nearest.is_finite().then(|| nearest.sqrt())
}

pub(super) fn plan(
    m: &dyn GuestMemory,
    source: Data,
    group: u32,
    start: u32,
    members: &[u32],
    anchors: &[[f32; 3]],
    radius: f32,
    virtual_enabled: bool,
) -> R<Option<migration::Migration>> {
    if anchors.is_empty() || members.is_empty() {
        return Ok(None);
    }
    let Some((route_block, point_block)) = source.directory(m, group)? else {
        return Ok(None);
    };
    let routes = rows(
        m,
        source,
        route_block,
        wire::Route::WIDTH,
        wire::Route::decode,
    )?;
    if routes.is_empty() {
        return Ok(None);
    }
    let mut virtual_orders = BTreeMap::<_, BTreeSet<_>>::new();
    for route in &routes {
        if route.orders != [NATIVE_ORDER; 2] {
            virtual_orders
                .entry(u32::from(route.squad))
                .or_default()
                .extend(route.orders);
        }
    }
    let mut squads = BTreeMap::new();
    for &id in members {
        let (Some(definition), Some(runtime)) =
            (actors::squad_record(m, id)?, squad_control::runtime(m, id)?)
        else {
            continue;
        };
        let current = m.u16(runtime + 0x2a)?;
        let on_virtual_route = virtual_orders
            .get(&id)
            .is_some_and(|orders| orders.contains(&current));
        squads.insert(
            id,
            Squad {
                current,
                initial: m.u16(definition + 0x42)?,
                allowed: migration::has(m, start, migration::ALLOWED, id)?,
                enabled: migration::has(m, start, migration::ENABLED, id)?,
                on_virtual_route,
            },
        );
    }
    let population: Vec<_> = actors::all(m)?
        .into_iter()
        .filter(|actor| {
            squads
                .get(&actor.squad)
                .is_some_and(|squad| squad.allowed || squad.on_virtual_route)
        })
        .collect();
    if population.is_empty() {
        return Ok(None);
    }
    let Some(anchor) = migration::anchor(m, &population, anchors)? else {
        return Ok(None);
    };
    let points = rows(m, source, point_block, wire::POINT_WIDTH, wire::read_point)?;
    let distances = group_distances(points, anchor);
    let mut choices = Vec::new();
    for route in routes {
        if route.squad == NO_SQUAD {
            continue;
        }
        let squad = u32::from(route.squad);
        let Some(state) = squads.get(&squad) else {
            continue;
        };
        let virtual_route = route.orders != [NATIVE_ORDER; 2];
        if virtual_route && !virtual_enabled || !virtual_route && (!state.allowed || !state.enabled)
        {
            continue;
        }
        let defending = if let Some(member) = squads.get(&u32::from(route.posture_squad)) {
            defensive_order(m, member.own_order(), route.defending)?
        } else {
            route.defending
        };
        let posture = usize::from(defending);
        let order = if virtual_route {
            route.orders[posture]
        } else {
            state.own_order()
        };
        let Some(distance) = nearest(route.masks[posture], &distances) else {
            continue;
        };
        let occupied =
            state.occupied_order() == order && population.iter().any(|actor| actor.squad == squad);
        choices.push((migration::Destination { squad, order }, distance, occupied));
    }
    let Some(destination) = migration::destination(&choices, radius) else {
        return Ok(None);
    };
    let actors = population
        .into_iter()
        .filter(|actor| actor.squad != destination.squad)
        .collect();
    Ok(Some(migration::Migration {
        destination,
        actors,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_follow_scoring_uses_all_positions_without_consulting_the_active_bsp() {
        let points = [
            (3, [20.0, 0.0, 0.0]),
            (17, [2.0, 0.0, 0.0]),
            (17, [3.0, 0.0, 0.0]),
        ];
        let distances = group_distances(points, [0.0; 3]);
        assert_eq!(nearest((1 << 3) | (1 << 17), &distances), Some(2.0));
        assert_eq!(nearest(1 << 18, &distances), None);
        // Exact ties keep the first destination in authored squad order.
        assert_eq!(
            migration::destination(&[(350, 2., false), (21, 2., false)], 2.0),
            Some(350)
        );
    }
}
