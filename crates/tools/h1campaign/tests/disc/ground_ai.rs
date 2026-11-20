//! The a30 scenario after map serialization, with its actual BSP and tag tables.
use std::collections::HashSet;
use std::io::Cursor;

use h1campaign::ai::h2::ScenarioAi;
use h1map::H1Map;
use map::{FourCC, Map, TagId};
use tags::{pathfinding::PathfindingData, schema::Memory};

use super::count_at;

/// Retail characters can inherit the firing parameters for a weapon from a parent.
fn weapon_profiles(m: &Map<Cursor<Vec<u8>>>, mem: &Memory, mut id: TagId) -> HashSet<TagId> {
    let mut seen = HashSet::new();
    let mut weapons = HashSet::new();
    while id != TagId::NULL {
        assert!(seen.insert(id), "character parent cycle");
        let t = m.tag(id).unwrap();
        assert_eq!(t.group, FourCC(*b"char"));
        let root = t.location.unwrap().address;
        let count = count_at(mem, root + 0xcc);
        let entries = count_at(mem, root + 0xd0);
        for i in 0..count {
            weapons.insert(TagId(count_at(mem, entries + 0xcc * i + 8)));
        }
        id = TagId(count_at(mem, root + 8));
    }
    weapons
}

pub(super) fn check(
    h1: &H1Map,
    m: &Map<Cursor<Vec<u8>>>,
    ai: &ScenarioAi,
    decoded: &[PathfindingData],
    mem: &Memory,
) {
    let source = h1campaign::ai::h1::H1Ai::read(h1).unwrap();
    assert_eq!(source.encounters.len(), ai.zones.len());
    let (mut ground, mut assigned) = (0, 0);
    for (enc, zone) in source.encounters.iter().zip(&ai.zones) {
        if enc.firing_positions.is_empty() || enc.flags & 0x20 != 0 {
            continue;
        }
        assert_eq!(zone.manual_bsp, enc.bsp, "{}", zone.name);
        let mesh = &decoded[enc.bsp as usize];
        let mut positions: Vec<_> = enc.firing_positions.iter().collect();
        positions.sort_by_key(|p| p.group);
        assert_eq!(positions.len(), zone.firing_positions.len());
        for (src, dst) in positions.into_iter().zip(&zone.firing_positions) {
            // The roster pass projects ground points and queries the cluster
            // just above the floor. Keep checking the serialized cluster mask.
            if dst.cluster >= 0 {
                let c = dst.cluster as usize;
                let area = &zone.areas[dst.area as usize];
                assert_ne!(area.clusters[c / 32] & (1 << (c % 32)), 0);
            }
            ground += 1;
            let [x, y, _] = src.position;
            let expected = mesh.sector_for_surface(src.surface as usize, x, y);
            if let Some(sector) = expected {
                use pathfind::geom::{contains_xy, dot, newell, sub};
                let ring: Vec<_> = mesh.sector_polygon(sector as usize).unwrap()
                    .into_iter().map(|i| mesh.vertices[i]).collect();
                let normal = newell(&ring);
                if contains_xy(&ring, x, y) && normal[2].abs() > f32::EPSILON {
                    let dz = dot(normal, sub(src.position, ring[0])) / normal[2];
                    if dz.abs() <= 0.25 && normal[2] > 0.0 {
                        assert_eq!(dst.sector, sector, "{}", zone.name);
                        assert_eq!(dst.position[..2], src.position[..2]);
                        assert!((dst.position[2] - (src.position[2] - dz)).abs() < 0.001);
                    }
                }
            }
            if dst.sector != u32::MAX {
                assert!((dst.sector as usize) < mesh.sectors.len());
                assigned += 1;
            }
        }
    }
    assert!(
        ground > 0 && assigned * 100 >= ground * 98,
        "{assigned}/{ground}"
    );
    // The early fight and the later plateau both need paths between
    // positions, in different BSPs. Follow the emitted two-sided links.
    for (name, expected_bsp, minimum_connected) in [("first_wave", 0, 510), ("mid_cliff_1", 1, 54)]
    {
        let zone = ai.zones.iter().find(|z| z.name == name).unwrap();
        assert_eq!(zone.manual_bsp, expected_bsp);
        let components = pathfind::compare::components(&decoded[expected_bsp as usize]);
        let mut counts = std::collections::BTreeMap::new();
        for f in &zone.firing_positions {
            if let Some(component) = components.get(f.sector as usize) {
                *counts.entry(*component).or_insert(0) += 1;
            }
        }
        assert!(
            counts.values().max().copied().unwrap_or(0) >= minimum_connected,
            "{name}"
        );
    }
    let (_, field, _) = tags::object::OBJECT_TYPES[2].placement.unwrap();
    let address = m.scenario().unwrap().location.unwrap().address + u32::from(field);
    let palette = count_at(mem, address + 4);
    let weapon_count = count_at(mem, address);
    let profiles: Vec<_> = ai
        .character_palette
        .iter()
        .map(|&c| weapon_profiles(m, mem, c))
        .collect();
    for enc in source.encounters.iter().filter(|e| e.flags & 0x20 == 0) {
        for src in enc
            .squads
            .iter()
            .filter(|s| !s.starting_locations.is_empty())
        {
            let name: String = format!("{}/{}", enc.name, src.name)
                .chars()
                .take(31)
                .collect();
            let squad = ai.squads.iter().find(|s| s.name == name).unwrap();
            assert_eq!(squad.starting_locations.len(), src.starting_locations.len());
            for (start, loc) in src.starting_locations.iter().zip(&squad.starting_locations) {
                assert_eq!(loc.position, start.position);
                let actor = if start.actor < 0 {
                    src.actor
                } else {
                    start.actor
                };
                let index = if loc.initial_weapon < 0 {
                    squad.initial_weapon
                } else {
                    loc.initial_weapon
                };
                if actor < 0 {
                    assert_eq!(index, -1);
                    continue;
                }
                let variant = source.actor_palette[actor as usize].as_ref().unwrap();
                let tag = h1.tag(variant.weapon.id).unwrap();
                let h1campaign::h2assets::Swap::Retail(retail) =
                    h1campaign::h2assets::swap_for(b"weap", &tag.name)
                else {
                    panic!("no a30 weapon counterpart for {}", tag.name);
                };
                // Retail characters carry the retail counterpart. A rebuilt
                // character (a30's `crewman pistol`) carries Halo 1's own
                // weapon in the Halo 1-assets variant (docs/halo1/ai.md,
                // Built — crew), converted under its source name.
                let rebuilt = source
                    .actor_of(actor as usize)
                    .is_some_and(|a| h1campaign::ai::convert::rebuilds_character(a.kind));
                let expected = if rebuilt {
                    tag.name.rsplit('\\').next().unwrap()
                } else {
                    retail
                };
                let slot = u32::try_from(index).unwrap();
                assert!(slot < weapon_count);
                let id = TagId(count_at(mem, palette + slot * 0x28 + 4));
                let t = m.tag(id).unwrap();
                assert_eq!(t.group, FourCC(*b"weap"));
                let character = if loc.character < 0 {
                    squad.character
                } else {
                    loc.character
                };
                assert!(
                    profiles[character as usize].contains(&id),
                    "missing firing profile for {name}"
                );
                assert_eq!(
                    t.name.as_deref().unwrap().rsplit('\\').next(),
                    Some(expected),
                    "{name}"
                );
            }
        }
    }
}
