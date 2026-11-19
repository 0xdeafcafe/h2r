//! The converter on invented Halo 1 AI data, built into a map and read back
//! through the Halo 2 decoders. No disc needed.

mod common;

use common::build_and_read;
use h1ai::convert::{Options, character_for, convert, grenade};
use h1ai::h1::{
    Actor, ActorType, ActorVariant, Atom, CommandList, Encounter, FiringPosition, H1Ai, Point,
    Squad, StartingLocation,
};
use h1map::{Group, TagId, TagRef};

fn tag_ref(group: &[u8; 4], id: u32) -> TagRef {
    TagRef {
        group: Group::new(group),
        name_address: 0,
        name_len: 0,
        id: TagId(id),
    }
}

fn variant(name: &str, actor: u32) -> Option<ActorVariant> {
    Some(ActorVariant {
        name: name.into(),
        flags: 0,
        actor: tag_ref(b"actr", actor),
        unit: tag_ref(b"bipd", 100),
        major_variant: tag_ref(b"actv", u32::MAX),
        weapon: tag_ref(b"weap", u32::MAX),
    })
}

fn start(x: f32, actor: i16) -> StartingLocation {
    StartingLocation {
        position: [x, 0.0, 1.0],
        facing: 0.5,
        sequence_id: 0,
        flags: 0,
        return_state: 0,
        initial_state: 0,
        actor,
        command_list: -1,
    }
}

fn squad(name: &str, actor: i16, attacking: u32, starts: Vec<StartingLocation>) -> Squad {
    Squad {
        name: name.into(),
        actor,
        platoon: -1,
        initial_state: 0,
        return_state: 0,
        flags: 0,
        unique_leader: 0,
        maneuver_to: -1,
        delay: 2.0,
        groups: [attacking, 0, 0, 0, 0, 0, 0],
        normal_count: 2,
        insane_count: 3,
        major_upgrade: 1,
        respawn: [0; 3],
        respawn_delay: [0.0; 2],
        move_positions: Vec::new(),
        starting_locations: starts,
    }
}

fn fp(x: f32, group: u16) -> FiringPosition {
    FiringPosition {
        position: [x, 1.0, 0.0],
        group,
        cluster: 0,
        surface: 7,
    }
}

fn invented() -> H1Ai {
    H1Ai {
        actor_palette: vec![
            variant("characters\\elite\\elite major", 1),
            variant("characters\\grunt\\grunt minor", 2),
            variant("characters\\elite\\elite major plasma rifle", 1),
        ],
        actors: vec![
            (
                TagId(1),
                Actor {
                    name: "characters\\elite\\elite".into(),
                    flags: 0,
                    more_flags: 0,
                    kind: ActorType(0),
                },
            ),
            (
                TagId(2),
                Actor {
                    name: "characters\\grunt\\grunt".into(),
                    flags: 0,
                    more_flags: 0,
                    kind: ActorType(2),
                },
            ),
        ],
        encounters: vec![Encounter {
            name: "first".into(),
            flags: 0,
            team: 3,
            search_behavior: 0,
            manual_bsp: -1,
            bsp: 1,
            respawn_delay: [0.0; 2],
            squads: vec![
                // Attacks from group C only.
                squad("elites", 0, 1 << 2, vec![start(1.0, -1), start(2.0, 1)]),
                // No attacking groups: every area.
                squad("grunts", 1, 0, vec![start(3.0, -1)]),
                // Never spawns.
                squad("empty", 1, 0, Vec::new()),
            ],
            platoons: Vec::new(),
            // Groups C, A, C, A: areas a (0, 1) and c (2, 3) after sorting.
            firing_positions: vec![fp(10.0, 2), fp(0.0, 0), fp(14.0, 2), fp(2.0, 0)],
            player_starts: Vec::new(),
        }],
        command_lists: vec![CommandList {
            name: "walk".into(),
            flags: 0,
            manual_bsp: 0,
            bsp: 2,
            atoms: vec![Atom {
                kind: 1,
                modifier: 0,
                parameters: [0.0; 2],
                points: [1, -1],
                animation: -1,
                script: -1,
                recording: -1,
                command: -1,
                object_name: -1,
            }],
            points: vec![
                Point {
                    position: [1.0, 2.0, 3.0],
                    surface: 4,
                },
                Point {
                    position: [5.0, 6.0, 7.0],
                    surface: 8,
                },
            ],
        }],
        conversations: Vec::new(),
        reference_counts: [0; 3],
    }
}

#[test]
fn encounters_become_groups_zones_squads_and_orders() {
    let src = invented();
    assert!(src.check_indices().is_empty());
    let plan = convert(&src, &Options::default());
    let ai = &plan.ai;
    // Two palette entries name one character.
    let chars: Vec<_> = plan
        .characters
        .iter()
        .map(|c| c.h2_character.as_str())
        .collect();
    assert_eq!(
        chars,
        [
            "objects\\characters\\elite\\ai\\elite_major",
            "objects\\characters\\grunt\\ai\\grunt"
        ]
    );
    assert_eq!(ai.squad_groups.len(), 1);
    assert_eq!(ai.squad_groups[0].name, "first");

    // One area per group letter, firing positions grouped by area.
    let z = &ai.zones[0];
    let names: Vec<_> = z.areas.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["a", "c"]);
    let xs: Vec<f32> = z.firing_positions.iter().map(|f| f.position[0]).collect();
    assert_eq!(xs, [0.0, 2.0, 10.0, 14.0]);
    assert_eq!((z.areas[1].first, z.areas[1].count), (2, 2));
    assert_eq!(z.areas[1].mean, [12.0, 1.0, 0.0]);
    assert!((z.areas[1].deviation - 2.0).abs() < 1e-6);
    assert!(
        z.firing_positions
            .iter()
            .all(|f| f.sector == u32::MAX && f.cluster == 0)
    );

    // The squad with no starting locations is kept, empty, with a note:
    // scripts name it.
    assert_eq!(ai.squads.len(), 3);
    assert!(plan.notes.iter().any(|n| n.contains("first/empty")));
    let empty = &ai.squads[2];
    assert_eq!(empty.name, "first/empty");
    assert!(empty.starting_locations.is_empty());
    assert_eq!((empty.normal_count, empty.insane_count), (0, 0));
    assert_eq!(ai.orders[empty.initial_order as usize].name, empty.name);
    let elites = &ai.squads[0];
    assert_eq!(elites.name, "first/elites");
    assert_eq!((elites.parent, elites.initial_zone, elites.team), (0, 0, 3));
    assert_eq!(
        (elites.normal_count, elites.insane_count, elites.delay),
        (2, 3, 2.0)
    );
    assert_eq!(elites.grenade_type, grenade::COVENANT);
    assert_eq!(elites.character, 0);
    // A starting location naming the squad's own actor keeps -1; one naming
    // another actor gets that actor's character.
    let chars: Vec<i16> = elites
        .starting_locations
        .iter()
        .map(|l| l.character)
        .collect();
    assert_eq!(chars, [-1, 1]);
    assert_eq!(elites.starting_locations[0].facing, [0.5, 0.0]);

    // Orders: the elites hold area c; the grunts, with no groups, hold all.
    let o = &ai.orders[elites.initial_order as usize];
    assert_eq!(
        o.primary_areas.iter().map(|r| r.area).collect::<Vec<_>>(),
        [1]
    );
    let o = &ai.orders[ai.squads[1].initial_order as usize];
    assert_eq!(
        o.primary_areas.iter().map(|r| r.area).collect::<Vec<_>>(),
        [0, 1]
    );

    // Commands use the scripting point set. Only squad orders occupy the order table.
    assert_eq!(ai.point_sets[0].name, "walk");
    assert_eq!(ai.point_sets[0].points[1].position, [5.0, 6.0, 7.0]);
    assert_eq!(ai.point_sets[0].points[1].surface, 8);
    assert!(!ai.orders.iter().any(|o| o.name == "walk"));
}

#[test]
fn variant_loadouts_and_compiled_bsp_survive_serialization() {
    use tags::pathfinding::{Link, PathfindingData, Ref, Sector};
    let mesh = |sector: u16| PathfindingData {
        sectors: vec![
            Sector {
                flags: 0x19,
                hint: u16::MAX,
                first_link: 0
            };
            2
        ],
        refs: vec![Ref::sector(u32::from(sector)); 8],
        links: (0..3)
            .map(|i| Link {
                vertex1: i,
                vertex2: (i + 1) % 3,
                flags: 1,
                hint: u16::MAX,
                forward_link: (i + 1) % 3,
                reverse_link: u16::MAX,
                left_sector: sector,
                right_sector: u16::MAX,
            })
            .collect(),
        // A floor rising towards positive Y.
        vertices: vec![[0.0; 3], [20.0, 0.0, 0.0], [0.0, 20.0, 10.0]],
        bsp2d_nodes: vec![],
        object_refs: vec![],
        hints: vec![],
        instance_refs: vec![],
        checksum: 0,
        user_hints: vec![],
    };
    let mut src = invented();
    src.actors[1].1.kind = ActorType(7);
    let encounter = &mut src.encounters[0];
    encounter.squads[0].starting_locations.push(start(3.0, 1));
    encounter.manual_bsp = 0;
    encounter.firing_positions[0].cluster = 39;
    encounter.firing_positions[2].surface = u32::MAX;
    encounter.firing_positions[2].cluster = -1;
    // Two variants use the same character with different starting weapons.
    encounter.squads[0].starting_locations[1].actor = 2;
    let plan = h1ai::convert::convert_for_map(
        &src,
        &Options::default(),
        &[mesh(0), mesh(1)],
        &[Some(4), None, Some(9)],
    );
    let (ai, _) = build_and_read(&plan);
    let squad = &ai.squads[0];
    assert_eq!(squad.initial_weapon, 4);
    assert_eq!(squad.starting_locations[0].initial_weapon, -1);
    assert_eq!(squad.starting_locations[1].character, squad.character);
    assert_eq!(squad.starting_locations[1].initial_weapon, 9);
    assert_eq!(squad.starting_locations[1].grenade_type, grenade::COVENANT);
    assert_eq!(squad.starting_locations[2].grenade_type, grenade::HUMAN);
    let z = &ai.zones[0];
    assert_eq!(z.manual_bsp, 1);
    assert_eq!(z.firing_positions[2].sector, 1);
    let [yaw, pitch] = z.firing_positions[2].normal;
    assert!((yaw + std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    assert!((pitch - 2.0f32.atan()).abs() < 1e-5);
    assert_eq!(z.firing_positions[3].sector, u32::MAX);
    assert_eq!(z.areas[1].clusters[1], 1 << 7);
    assert_eq!(z.areas[0].clusters[0], 1);
}

#[test]
fn a_built_plan_reads_back_unchanged() {
    let plan = convert(&invented(), &Options::default());
    let (back, palette) = build_and_read(&plan);
    let mut want = plan.ai.clone();
    want.character_palette = palette;
    assert_eq!(back, want);
    assert!(
        back.check_indices().is_empty(),
        "{:#?}",
        back.check_indices()
    );
}

#[test]
fn placement_list_reads_back_as_a_command_script() {
    let mut src = invented();
    src.encounters[0].squads[0].starting_locations[0].command_list = 0;
    let plan = convert(&src, &Options { first_command_script: Some(23), ..Default::default() });
    let (back, _) = build_and_read(&plan);
    let starts = &back.squads[0].starting_locations;
    assert_eq!(starts[0].placement_script, "h1_cmd_0");
    assert_eq!(starts[0].placement_script_index, 23);
    assert_eq!(starts[1].placement_script_index, -1);
    assert!(starts[1].placement_script.is_empty());
    assert_eq!(back.point_sets[0].bsp, 2);
}

#[test]
fn an_unresolved_character_is_an_error() {
    let plan = convert(&invented(), &Options::default());
    assert!(plan.blocks(|_| None).is_err());
}

#[test]
fn long_names_are_cut_to_31_characters() {
    let mut src = invented();
    src.encounters[0].name = "x".repeat(40);
    let plan = convert(&src, &Options::default());
    assert_eq!(plan.ai.zones[0].name.len(), 31);
    assert_eq!(plan.ai.squads[0].name.len(), 31);
    assert_ne!(plan.squad_names[0], plan.squad_names[1]);
    assert_eq!(plan.ai.squads[0].name, plan.ai.squads[1].name);
    assert_eq!(plan.group_names[0], src.encounters[0].name);
    let (back, _) = build_and_read(&plan);
    assert_eq!(back.squads[0].name, plan.ai.squads[0].name);
}

#[test]
fn the_character_table_picks_by_species_and_rank() {
    let c = |k, v| character_for(ActorType(k), v).0;
    assert_eq!(
        c(0, "characters\\elite\\elite stealth major"),
        "objects\\characters\\elite\\ai\\elite_stealth_major"
    );
    assert_eq!(
        c(0, "characters\\elite\\elite minor"),
        "objects\\characters\\elite\\ai\\elite"
    );
    assert_eq!(
        c(1, "characters\\jackal\\jackal major"),
        "objects\\characters\\jackal\\ai\\jackal_major"
    );
    assert_eq!(
        c(2, "characters\\grunt\\grunt specops needler"),
        "objects\\characters\\grunt\\ai\\grunt_specops"
    );
    assert_eq!(
        c(3, "characters\\hunter\\hunter"),
        "objects\\characters\\hunter\\ai\\hunter"
    );
    assert_eq!(
        c(8, "characters\\crewman\\crewman"),
        "objects\\characters\\marine\\ai\\crewman"
    );
    assert_eq!(
        c(7, "characters\\johnson\\johnson"),
        "objects\\characters\\marine\\ai\\marine_johnson"
    );
    assert_eq!(
        c(9, "characters\\floodcombat_human\\x"),
        "objects\\characters\\floodcombat_human\\ai\\flood_combat_human"
    );
    assert_eq!(
        c(13, "characters\\sentinel\\sentinel"),
        "objects\\characters\\sentinel_aggressor\\ai\\sentinel_aggressor_halo1"
    );
    assert_eq!(c(14, "anything"), "ai\\generic");
    assert_eq!(character_for(ActorType(7), "").1, grenade::HUMAN);
    assert_eq!(character_for(ActorType(10), "").1, grenade::NONE);
}

/// Platoons the scripts name become child squad groups of the encounter's,
/// holding exactly their squads; the others keep the encounter's group, and
/// an empty platoon set converts nothing (as before).
#[test]
fn named_platoons_become_child_squad_groups() {
    use h1ai::h1::Platoon;
    let mut src = invented();
    let e = &mut src.encounters[0];
    e.platoons = vec![
        Platoon {
            name: "lz_toon".into(),
            flags: 0,
            change: h1ai::h1::PlatoonTrigger::DISABLED,
            maneuver: h1ai::h1::PlatoonTrigger::DISABLED,
        },
        Platoon {
            name: "unnamed_toon".into(),
            flags: 0,
            change: h1ai::h1::PlatoonTrigger::DISABLED,
            maneuver: h1ai::h1::PlatoonTrigger::DISABLED,
        },
    ];
    // elites and grunts in the named platoon, empty squad left in the
    // encounter's.
    e.squads[0].platoon = 0;
    e.squads[1].platoon = 0;
    let named = ["first/lz_toon".to_string()].into_iter().collect();
    let plan = convert(
        &src,
        &Options {
            platoon_groups: named,
            ..Default::default()
        },
    );
    let ai = &plan.ai;
    // The encounter's group, then the named platoon's as its child.
    assert_eq!(ai.squad_groups.len(), 2);
    assert_eq!(ai.squad_groups[0].name, "first");
    assert_eq!(ai.squad_groups[0].parent, -1);
    assert_eq!(ai.squad_groups[1].name, "first/lz_toon");
    assert_eq!(ai.squad_groups[1].parent, 0);
    let by_name = |n: &str| ai.squads.iter().position(|q| q.name == n).unwrap();
    assert_eq!(ai.squads[by_name("first/elites")].parent, 1);
    assert_eq!(ai.squads[by_name("first/grunts")].parent, 1);
    // A squad of no named platoon stays in the encounter's group.
    assert_eq!(ai.squads[by_name("first/empty")].parent, 0);

    // Without the set, every squad is in the encounter's group, as before.
    let plain = convert(&src, &Options::default());
    assert_eq!(plain.ai.squad_groups.len(), 1);
    assert!(plain.ai.squads.iter().all(|q| q.parent == 0));
}

#[test]
fn scripted_platoon_controls_follow_source_indices_and_round_trip() {
    use h1ai::control::{Action, Operation, Platoon, Request, maneuver_targets, prepare};

    let mut source = invented();
    let encounter = &mut source.encounters[0];
    encounter.platoons.push(h1ai::h1::Platoon {
        name: "front".into(),
        flags: 0,
        change: h1ai::h1::PlatoonTrigger::DISABLED,
        maneuver: h1ai::h1::PlatoonTrigger::DISABLED,
    });
    encounter.squads[0].platoon = 0;
    encounter.squads[0].groups[3] = 1;
    encounter.squads[0].maneuver_to = 2;
    encounter.squads[2].groups[0] = 1 << 2;

    let request = |operation, reference, target| Request {
        operation,
        reference,
        target,
    };
    let defend = request(Operation::Defend, 0x8000_0000, None);
    let maneuver = request(Operation::Maneuver, 0x4000_0000, None);
    let enable = request(Operation::ManeuverEnable, 0, None);
    let follow = request(Operation::FollowPlayers, 0x8000_0000, None);
    let leader = request(Operation::FollowAi, 0, Some(0x8001_0000));
    let stop = request(Operation::FollowDisable, 0, None);
    let requests = [defend, maneuver, enable, follow, leader, stop];

    assert_eq!(
        maneuver_targets(&source, &requests)
            .into_iter()
            .collect::<Vec<_>>(),
        ["first/empty"]
    );
    let opts = Options::default();
    let mut plan = convert(&source, &opts);
    let actions = prepare(&source, &mut plan, &opts, &requests);
    let platoon = Platoon {
        encounter: 0,
        index: 0,
    };
    assert_eq!(
        actions.actions[&maneuver],
        [Action::Migrate {
            from: 0,
            to: 2,
            platoon: Some(platoon)
        }]
    );
    assert_eq!(actions.actions[&enable], [Action::Enable(platoon)]);
    assert!(actions.switches.contains(&platoon));

    let (back, _) = build_and_read(&plan);
    let [Action::SetOrder { squad: 0, order }] = actions.actions[&defend].as_slice() else {
        panic!("one defending squad");
    };
    assert_eq!(back.orders[usize::from(*order)].primary_areas[0].area, 0);
    let destination = back.squads[2].initial_order as usize;
    assert_eq!(back.orders[destination].primary_areas[0].area, 1);

    let order_for = |request, target| {
        actions.actions[&request]
            .iter()
            .find_map(|action| match *action {
                Action::SetOrder { squad, order } if squad == target => {
                    Some(&back.orders[usize::from(order)])
                }
                _ => None,
            })
            .unwrap()
    };
    let player_order = order_for(follow, 0);
    assert_eq!((player_order.flags, player_order.follow_squad), (0x10, -1));
    assert_eq!(player_order.follow_radius, 4.0);
    let ai_order = order_for(leader, 0);
    assert_eq!(
        (
            ai_order.flags,
            ai_order.follow_squad,
            ai_order.entry_script_index
        ),
        (0x20, 1, -1)
    );
    assert_eq!(order_for(leader, 1).flags, 0);
    assert_eq!(order_for(stop, 0).flags, 0);
    assert!(back.check_indices().is_empty());
}

#[test]
fn automatic_platoon_rules_retain_routes_and_use_the_selected_platoons_health() {
    use h1ai::{
        control::{Trigger, maneuver_targets, prepare},
        h1::{Platoon, PlatoonTrigger},
    };
    let mut source = invented();
    let encounter = &mut source.encounters[0];
    encounter.platoons = vec![
        Platoon {
            name: "front".into(),
            flags: 4,
            change: PlatoonTrigger {
                condition: 2,
                subject: 1,
            },
            maneuver: PlatoonTrigger {
                condition: 6,
                subject: -1,
            },
        },
        Platoon {
            name: "rear".into(),
            flags: 0,
            change: PlatoonTrigger::DISABLED,
            maneuver: PlatoonTrigger::DISABLED,
        },
    ];
    encounter.squads[0].platoon = 0;
    encounter.squads[0].groups[3] = 1 << 2;
    encounter.squads[0].maneuver_to = 2;
    encounter.squads[1].platoon = 1;
    assert!(maneuver_targets(&source, &[]).contains("first/empty"));

    let opts = Options::default();
    let mut plan = convert(&source, &opts);
    let control = prepare(&source, &mut plan, &opts, &[]);
    let [rule] = control.platoon_rules.as_slice() else {
        panic!("one platoon has automatic rules");
    };
    assert_eq!(rule.members, [0]);
    assert_eq!(rule.routes, [(0, 2)]);
    assert_eq!(
        rule.change,
        Some(Trigger {
            condition: 2,
            squads: vec![1]
        })
    );
    assert_eq!(
        rule.maneuver,
        Some(Trigger {
            condition: 6,
            squads: vec![0, 1, 2]
        })
    );

    let (back, _) = build_and_read(&plan);
    assert!(back.squads[2].initial_order >= 0);
    let initial = &back.orders[back.squads[0].initial_order as usize];
    assert_eq!(initial.primary_areas[0].area, 1);
    assert_ne!(rule.orders[0].1, back.squads[0].initial_order as u16);
}

#[test]
fn encounter_migration_chooses_a_destination_for_each_character() {
    use h1ai::control::{Action, Operation, Request, prepare};

    let mut source = invented();
    let mut target = source.encounters[0].clone();
    target.name = "second".into();
    target.squads.pop();
    target.squads.reverse();
    source.encounters.push(target);
    let opts = Options::default();
    let mut plan = convert(&source, &opts);
    let migrate = Request {
        operation: Operation::Migrate,
        reference: 0,
        target: Some(1),
    };
    let routes = prepare(&source, &mut plan, &opts, &[migrate]);
    assert_eq!(
        routes.actions[&migrate][0],
        Action::Migrate {
            from: 0,
            to: 4,
            platoon: None
        }
    );
    assert_eq!(
        routes.actions[&migrate][1],
        Action::Migrate {
            from: 1,
            to: 3,
            platoon: None
        }
    );
}

#[test]
fn airborne_encounters_keep_authored_altitude_and_use_no_sector() {
    let mut src = invented();
    let e = &mut src.encounters[0];
    e.bsp = 3;
    e.flags |= 0x20;
    e.firing_positions[0].position[2] = 127.0;
    let (ai, _) = build_and_read(&convert(&src, &Options::default()));
    let z = &ai.zones[0];
    assert_eq!(z.manual_bsp, 3);
    assert!(
        z.firing_positions
            .iter()
            .all(|p| p.sector == u32::MAX && p.flags == 0)
    );
    assert!(z.firing_positions.iter().any(|p| p.position[2] == 127.0));
}
