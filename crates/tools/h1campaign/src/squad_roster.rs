//! Keep the identity of explicitly named migration targets. The compiled
//! HS literal contains the encounter and local squad indices, even when
//! the display name is longer than the Halo 2 scenario string.
use std::collections::HashSet;

use h1ai::h1::{FiringPosition, H1Ai, Squad, StartingLocation};
use h1scripts::{
    Tables,
    catalog::types::{AI, FUNCTION_NAME},
};

#[derive(Default)]
struct Uses {
    named: HashSet<u32>,
    places: HashSet<u32>,
    dynamic_place: bool,
}

impl Uses {
    fn read(t: &Tables) -> Self {
        let mut uses = Self::default();
        for node in t.nodes.iter().filter(|n| n.live()) {
            if node.atom() {
                // A call's head holds a builtin or script index instead.
                if !node.global() && node.value_type() != FUNCTION_NAME && node.opcode() == AI {
                    uses.named.insert(node.data());
                }
                continue;
            }
            let Ok(head) = t.node(hs::Datum(node.data())) else {
                continue;
            };
            if t.string(head.string_offset()).ok() != Some("ai_place") {
                continue;
            }
            let Ok(arg) = t.node(head.next()) else {
                continue;
            };
            if arg.atom() && !arg.global() && arg.opcode() == AI {
                uses.places.insert(arg.data());
            } else {
                uses.dynamic_place = true;
            }
        }
        uses
    }
}

fn spawn_locations(q: &mut Squad, fps: &[FiringPosition]) {
    if q.actor < 0 || !q.starting_locations.is_empty() {
        return;
    }
    let mask = q.groups.iter().fold(0, |a, &g| a | g);
    let sites: Vec<_> = if q.move_positions.is_empty() {
        fps.iter()
            .filter(|f| f.group < 32 && mask & (1 << f.group) != 0)
            .map(|f| (f.position, 0.0))
            .collect()
    } else {
        q.move_positions
            .iter()
            .map(|p| (p.position, p.facing))
            .collect()
    };
    // Each authored site is a candidate. The squad's normal/legendary
    // counts select the number of actors, including explicit placement.
    for (position, facing) in sites {
        q.starting_locations.push(StartingLocation {
            position,
            facing,
            sequence_id: -1,
            flags: 0,
            return_state: q.return_state,
            initial_state: q.initial_state,
            actor: -1,
            command_list: -1,
        });
    }
}

pub(crate) struct Selection {
    pub generated: HashSet<usize>,
    pub indices: Vec<Vec<Option<u16>>>,
    pub required_squads: HashSet<(usize, usize)>,
}

fn select(src: &mut H1Ai, uses: &Uses, groups: usize) -> crate::Result<Selection> {
    let mut indices = Vec::with_capacity(src.encounters.len());
    let mut generated = HashSet::new();
    let mut output_index = 0;
    let id = |e: usize, s: usize| 0x8000_0000 | ((s as u32) << 16) | e as u32;
    let mut count = 0;
    for (ei, e) in src.encounters.iter().enumerate() {
        for (si, q) in e.squads.iter().enumerate() {
            count +=
                usize::from(!q.starting_locations.is_empty() || uses.named.contains(&id(ei, si)));
        }
    }
    for (ei, e) in src.encounters.iter_mut().enumerate() {
        let mut rows = Vec::with_capacity(e.squads.len());
        let initially_created = e.flags & 1 == 0;
        e.squads.retain_mut(|q| {
            let datum = id(ei, rows.len());
            rows.push(None);
            let direct = uses.places.contains(&datum);
            let named = uses.named.contains(&datum);
            if q.starting_locations.is_empty() {
                if direct {
                    spawn_locations(q, &e.firing_positions);
                    if !q.starting_locations.is_empty() {
                        generated.insert(output_index);
                    }
                }
                if !named {
                    return false;
                }
            } else if !engine::campaign_squad_pool::holds(count, groups)
                && !named
                && !initially_created
                && !uses.dynamic_place
                && !uses.places.contains(&(ei as u32))
                && !(q.platoon >= 0
                    && uses
                        .places
                        .contains(&(0x4000_0000 | ((q.platoon as u32) << 16) | ei as u32)))
            {
                count -= 1;
                return false;
            }
            *rows.last_mut().unwrap() = u16::try_from(output_index).ok();
            output_index += 1;
            true
        });
        indices.push(rows);
    }
    if output_index > usize::from(u16::MAX) {
        return Err(crate::invalid(
            "AI roster exceeds the native squad index range",
        ));
    }
    Ok(Selection {
        generated,
        indices,
        required_squads: HashSet::new(),
    })
}

pub(crate) fn prepare(
    src: &mut H1Ai,
    tables: &Tables,
    requests: &[h1ai::control::Request],
    options: &mut h1ai::convert::Options,
    followed: Option<&std::collections::BTreeSet<u16>>,
    scripted: bool,
) -> crate::Result<Selection> {
    // Without converted scripts nothing names or places a squad, and the
    // automatic migration orders (installed with the scripts' subjects) are
    // not written. Keeping those squads would only fill the native pool:
    // a50 has 485 squads that way, past the native reservation.
    let mut uses = if scripted { Uses::read(tables) } else { Uses::default() };
    if scripted {
        for role in src
            .conversations
            .iter()
            .flat_map(|scene| &scene.participants)
        {
            if !role.radio()
                && role.existing_name.is_none()
                && let Some(selector) = role.encounter
            {
                uses.named.insert(selector & 0xc0ff_ffff);
            }
        }
    }
    // Preserve directly referenced rows before expanding membership to
    // optional empty squads. These ordinals must survive the pool budget.
    let mut required_squads: HashSet<_> = uses
        .named
        .iter()
        .filter(|&&id| id >> 30 == 2)
        .map(|&id| (usize::from(id as u16), usize::from((id >> 16) as u8)))
        .collect();
    // Counted spawns of a platoon address its first source squad, even
    // when that squad starts empty and has no direct script literal.
    for request in requests {
        if request.operation == h1ai::control::Operation::Spawn && request.reference >> 30 == 1 {
            let ei = usize::from(request.reference as u16);
            let platoon = i16::from((request.reference >> 16) as u8);
            if let Some(si) = src
                .encounters
                .get(ei)
                .and_then(|e| e.squads.iter().position(|s| s.platoon == platoon))
            {
                required_squads.insert((ei, si));
            }
        }
    }
    let mut maneuver = h1ai::control::maneuver_targets(src, requests);
    if scripted {
        maneuver.extend(h1ai::control::automatic_migration_targets(src, followed));
    }
    let destinations: HashSet<u32> = requests.iter()
        .filter(|r| r.operation == h1ai::control::Operation::Migrate)
        .filter_map(|r| r.target)
        .collect();
    for (ei, encounter) in src.encounters.iter().enumerate() {
        for pi in 0..encounter.platoons.len() {
            let datum = 0x4000_0000 | ((pi as u32) << 16) | ei as u32;
            if uses.named.contains(&datum) {
                options.platoon_groups.insert((ei, pi));
            }
        }
        for (si, squad) in encounter.squads.iter().enumerate() {
            let platoon = 0x4000_0000 | ((squad.platoon as u32) << 16) | ei as u32;
            if (squad.platoon >= 0 && uses.named.contains(&platoon))
                || destinations.contains(&(ei as u32))
                || maneuver.contains(&(ei, si))
            {
                uses.named.insert(0x8000_0000 | ((si as u32) << 16) | ei as u32);
            }
        }
    }
    let groups = src.encounters.len() + options.platoon_groups.len();
    let mut selection = select(src, &uses, groups)?;
    selection.required_squads = required_squads;
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_start_inherits_its_actor_and_uses_its_own_group() {
        let mut q = Squad {
            name: "guards".into(),
            actor: 2,
            platoon: -1,
            initial_state: 6,
            return_state: 3,
            flags: 0,
            unique_leader: 0,
            maneuver_to: -1,
            delay: 1.,
            groups: [2, 0, 0, 0, 0, 0, 0],
            normal_count: 1,
            insane_count: 1,
            major_upgrade: 0,
            respawn: [0; 3],
            respawn_delay: [0.; 2],
            move_positions: vec![],
            starting_locations: vec![],
        };
        let p = |i| FiringPosition {
            position: [i as f32, 1., 2.],
            group: i,
            cluster: 2,
            surface: 13,
        };
        spawn_locations(&mut q, &[p(0), p(1)]);
        assert_eq!(q.starting_locations.len(), 1);
        let start = &q.starting_locations[0];
        assert_eq!(
            (
                start.position,
                start.actor,
                start.initial_state,
                start.return_state
            ),
            ([1., 1., 2.], -1, 6, 3)
        );
        spawn_locations(&mut q, &[p(1)]);
        assert_eq!(q.starting_locations.len(), 1);
        q.actor = -1;
        q.starting_locations.clear();
        spawn_locations(&mut q, &[p(1)]);
        assert!(q.starting_locations.is_empty());
    }

    #[test]
    fn a_squad_with_no_authored_site_spawns_nothing() {
        // b40's a2_top_cov/grunt_sniper: group Y holds no firing position of
        // its encounter and the squad has no move positions, so no site can
        // be authored and the native selector (Halo 2 0x002039d0, both
        // passes falling through to `orl $-1` at 0x00203bb3) selects no
        // start: the squad stays empty and addressable.
        let mut q = Squad {
            name: "grunt_sniper".into(),
            actor: 2,
            platoon: -1,
            initial_state: 6,
            return_state: 3,
            flags: 0,
            unique_leader: 0,
            maneuver_to: -1,
            delay: 1.,
            groups: [1 << 24, 0, 0, 0, 0, 0, 0],
            normal_count: 4,
            insane_count: 4,
            major_upgrade: 0,
            respawn: [0; 3],
            respawn_delay: [0.; 2],
            move_positions: vec![],
            starting_locations: vec![],
        };
        let elsewhere = [FiringPosition {
            position: [0., 1., 2.],
            group: 1,
            cluster: 2,
            surface: 13,
        }];
        spawn_locations(&mut q, &elsewhere);
        assert!(q.starting_locations.is_empty());
    }

    #[test]
    #[ignore = "reads H1_ISO"]
    fn campaign_scripts_keep_their_squad_targets_and_fit_the_pools() {
        use std::io::{BufReader, Cursor};
        let path = std::env::var("H1_ISO")
            .unwrap_or_else(|_| format!("{}/../../../disc/halo1.iso", env!("CARGO_MANIFEST_DIR")));
        let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
        let disc = h1map::disc::Disc::open(&mut iso).unwrap();
        for level in h1map::disc::CAMPAIGN_LEVELS {
            let entry = disc.map(level).unwrap();
            let map = h1map::H1Map::from_reader(Cursor::new(iso.read(entry).unwrap())).unwrap();
            let mut ai = H1Ai::read(&map).unwrap();
            let before = ai.encounters.clone();
            let uses = Uses::read(&Tables::read(&map).unwrap());
            let groups = ai.encounters.len();
            let selection = select(&mut ai, &uses, groups).unwrap();
            for &id in &uses.named {
                if id >> 31 == 0 || id == u32::MAX {
                    continue;
                }
                let ei = (id & 65535) as usize;
                let si = ((id >> 16) & 0x7fff) as usize;
                let q = &before[ei].squads[si];
                let index = selection.indices[ei][si].expect("named squad retained");
                let target = ai
                    .encounters
                    .iter()
                    .flat_map(|e| &e.squads)
                    .nth(usize::from(index))
                    .unwrap();
                assert_eq!(target.name, q.name);
                // A placed empty squad takes its sites from its move
                // positions or its firing groups. b40's
                // `a2_top_cov/grunt_sniper` names only group Y, which holds
                // no firing position in its encounter, so it has none and
                // stays an empty, addressable squad: the native selector
                // (Halo 2 0x002039d0) selects no start for a squad with no
                // starting locations and places nothing.
                let mask = q.groups.iter().fold(0, |a, &g| a | g);
                let authored = !q.move_positions.is_empty()
                    || before[ei]
                        .firing_positions
                        .iter()
                        .any(|f| f.group < 32 && mask & (1 << f.group) != 0);
                if q.starting_locations.is_empty()
                    && q.actor >= 0
                    && uses.places.contains(&id)
                    && authored
                {
                    assert!(!target.starting_locations.is_empty(), "{level} {}", q.name);
                }
                if level == "b40"
                    && !authored
                    && q.starting_locations.is_empty()
                    && q.actor >= 0
                {
                    // The squad with no authored site of any kind stays
                    // named and empty: placement selects no start for it.
                    assert!(target.starting_locations.is_empty(), "{level} {}", q.name);
                }
            }
            let count: usize = ai.encounters.iter().map(|e| e.squads.len()).sum();
            assert!(
                engine::campaign_squad_pool::holds(count, ai.encounters.len()),
                "{level} {count}"
            );
            if level == "a50" {
                assert!(count > 335);
            }
        }
    }
}
