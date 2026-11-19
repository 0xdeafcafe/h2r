//! Keep the identity of explicitly named migration targets. The compiled
//! HS literal contains the encounter and local squad indices, even when
//! the display name is longer than the Halo 2 scenario string.
use std::collections::HashSet;

use h1ai::h1::{FiringPosition, H1Ai, Squad, StartingLocation};
use h1scripts::{Tables, catalog::types::AI};

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
                if !node.global() && node.opcode() == AI {
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

fn select(src: &mut H1Ai, uses: &Uses, groups: usize) -> HashSet<usize> {
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
        let mut si = 0;
        let initially_created = e.flags & 1 == 0;
        e.squads.retain_mut(|q| {
            let datum = id(ei, si);
            si += 1;
            let direct = uses.places.contains(&datum);
            let named = uses.named.contains(&datum);
            if q.starting_locations.is_empty() {
                if direct {
                    spawn_locations(q, &e.firing_positions);
                    if !q.starting_locations.is_empty() {
                        generated.insert(output_index);
                    }
                }
                output_index += usize::from(named);
                return named;
            }
            if engine::squad_pool::partition(count, groups).is_none()
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
            output_index += 1;
            true
        });
    }
    generated
}

pub(crate) fn prepare(
    src: &mut H1Ai,
    tables: &Tables,
    requests: &[h1ai::control::Request],
    options: &mut h1ai::convert::Options,
) -> HashSet<usize> {
    let mut uses = Uses::read(tables);
    let mut maneuver = h1ai::control::maneuver_targets(src, requests);
    maneuver.extend(h1ai::control::automatic_migration_targets(src));
    let destinations: HashSet<u32> = requests.iter()
        .filter(|r| r.operation == h1ai::control::Operation::Migrate)
        .filter_map(|r| r.target)
        .collect();
    for (ei, encounter) in src.encounters.iter().enumerate() {
        for (pi, platoon) in encounter.platoons.iter().enumerate() {
            let datum = 0x4000_0000 | ((pi as u32) << 16) | ei as u32;
            if uses.named.contains(&datum) {
                options.platoon_groups.insert(format!("{}/{}", encounter.name, platoon.name).to_ascii_lowercase());
            }
        }
        for (si, squad) in encounter.squads.iter().enumerate() {
            let platoon = 0x4000_0000 | ((squad.platoon as u32) << 16) | ei as u32;
            if (squad.platoon >= 0 && uses.named.contains(&platoon))
                || destinations.contains(&(ei as u32))
                || maneuver.contains(&format!("{}/{}", encounter.name, squad.name).to_ascii_lowercase())
            {
                uses.named.insert(0x8000_0000 | ((si as u32) << 16) | ei as u32);
            }
        }
    }
    let groups = src.encounters.len() + options.platoon_groups.len();
    select(src, &uses, groups)
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
    #[ignore = "reads H1_ISO"]
    fn campaign_scripts_keep_their_squad_targets_and_fit_the_pools() {
        use std::io::{BufReader, Cursor};
        let path = std::env::var("H1_ISO")
            .unwrap_or_else(|_| format!("{}/../../disc/halo1.iso", env!("CARGO_MANIFEST_DIR")));
        let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
        let disc = h1map::disc::Disc::open(&mut iso).unwrap();
        for level in h1map::disc::CAMPAIGN_LEVELS {
            let entry = disc.map(level).unwrap();
            let map = h1map::H1Map::from_reader(Cursor::new(iso.read(entry).unwrap())).unwrap();
            let mut ai = H1Ai::read(&map).unwrap();
            let before = ai.encounters.clone();
            let uses = Uses::read(&Tables::read(&map).unwrap());
            let groups = ai.encounters.len();
            select(&mut ai, &uses, groups);
            for &id in &uses.named {
                if id >> 31 == 0 || id == u32::MAX {
                    continue;
                }
                let ei = (id & 65535) as usize;
                let si = ((id >> 16) & 0x7fff) as usize;
                let q = &before[ei].squads[si];
                let target = ai.encounters[ei]
                    .squads
                    .iter()
                    .find(|s| s.name == q.name)
                    .unwrap();
                if q.starting_locations.is_empty() && q.actor >= 0 && uses.places.contains(&id) {
                    assert!(!target.starting_locations.is_empty(), "{level} {}", q.name);
                }
            }
            let count: usize = ai.encounters.iter().map(|e| e.squads.len()).sum();
            assert!(
                engine::squad_pool::partition(count, ai.encounters.len()).is_some(),
                "{level} {count}"
            );
            if level == "a50" {
                assert!(count > 335);
            }
        }
    }
}
