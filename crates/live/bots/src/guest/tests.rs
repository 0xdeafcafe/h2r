use super::*;
use guest_layout::datum::layout as dl;
use guest_layout::testing::FakeGuest;

const GAME: u32 = 0x0060_0000;
const PLAYERS_HEADER: u32 = 0x0061_0000;
const PLAYERS_DATA: u32 = 0x0061_1000;
const OBJECTS_HEADER: u32 = 0x0062_0000;
const OBJECTS_DATA: u32 = 0x0062_1000;
const BODIES: u32 = 0x0063_0000;
const STACK: u32 = 0x0067_0800;

fn array(g: &mut FakeGuest, header: u32, data: u32, size: u32, count: u32) {
    g.map(header, 0x48)
        .put_u32(header + dl::ELEMENT_SIZE, size)
        .put_u32(header + dl::HIGH_WATER, count)
        .put_u32(header + dl::DATA, data)
        .map(data, size * count.max(1));
}

/// One player per entry: (team, position, aim, dead).
type P = (i8, [f32; 3], [f32; 3], bool);

const fn machine(player: u32) -> [u8; 6] {
    [0xc1, 0xa4, 0, 0, 0, player as u8]
}

fn game(ps: &[P], teams: bool) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(GAME, 0x400)
        .put_u32(points::GAME_GLOBALS.base, GAME)
        .put_u32(GAME + OPTIONS, MODE_MULTIPLAYER)
        .put_u32(GAME + OPTIONS + VARIANT_FLAGS, teams as u32);
    g.put_u32(points::PLAYERS.base, PLAYERS_HEADER);
    array(&mut g, PLAYERS_HEADER, PLAYERS_DATA, 0x21c, ps.len() as u32);
    g.put_u32(points::OBJECTS.base, OBJECTS_HEADER);
    array(&mut g, OBJECTS_HEADER, OBJECTS_DATA, 0xc, ps.len() as u32);
    for (i, &(team, pos, aim, dead)) in ps.iter().enumerate() {
        let i = i as u32;
        let salt = 0xe000 + i as u16;
        let p = PLAYERS_DATA + i * 0x21c;
        let unit = DatumHandle::new(0xb000 + i as u16, i);
        g.put_u16(p, salt)
            .put_u16(p + 0x28, u16::MAX)
            .put(p + PLAYER_MACHINE, &machine(i))
            .put_u32(p + 0x2c, unit.raw())
            .put_u8(p + 0xc0, team as u8);
        let h = OBJECTS_DATA + i * 0xc;
        let body = BODIES + i * 0x400;
        g.put_u16(h, unit.salt())
            .put_u8(h + 3, objects::BIPED)
            .put_u32(h + 8, body);
        g.map(body, 0x400);
        for k in 0..3 {
            g.put_f32(body + 0x64 + 4 * k, pos[k as usize]);
            g.put_f32(body + UNIT_AIM + 4 * k, aim[k as usize]);
        }
        g.put_u16(body + 0x10a, if dead { BODY_DEPLETED } else { 0 });
    }
    g
}

const X: [f32; 3] = [1.0, 0.0, 0.0];

/// A free-for-all with three players.
fn three() -> FakeGuest {
    game(
        &[
            (0, [0.0; 3], X, false),
            (1, [5.0, 0.0, 0.0], X, false),
            (1, [9.0, 0.0, 0.0], X, false),
        ],
        false,
    )
}

#[test]
fn perception_finds_the_bot_and_the_living_others() {
    let mut ps = vec![
        (0, [0.0; 3], X, false),
        (1, [5.0, 1.0, 0.0], [0.0, 1.0, 0.0], false),
        (2, [7.0, 0.0, 0.0], X, true),
    ];
    let g = game(&ps, true);
    let p = perceive(&g, machine(1)).unwrap();
    assert!(p.teams);
    assert_eq!(
        p.me,
        Some(Me {
            position: [5.0, 1.0, 0.0],
            aim: [0.0, 1.0, 0.0],
            team: 1,
            shield: 0.0,
            health: 0.0,
            shield_full: 1.0,
            arms: Some(Default::default()),
        })
    );
    assert_eq!(
        p.others,
        vec![Seen {
            id: 0,
            position: [0.0; 3],
            team: 0
        }],
        "the dead are not seen"
    );
    // The bot dead: no `me`.
    ps[1].3 = true;
    assert_eq!(perceive(&game(&ps, true), machine(1)).unwrap().me, None);
}

/// The full shield is `0x1588b0`'s: the variant's shields times the
/// handicap's factor, so a bot's shield thresholds can read a fraction of it.
#[test]
fn the_full_shield_follows_the_variant_and_the_handicap() {
    let ps = vec![(0, [0.0; 3], X, false), (1, [5.0, 0.0, 0.0], X, false)];
    let full = |shields: u32, handicap: u8| {
        let mut g = game(&ps, false);
        g.put_u32(GAME + GAME_SHIELDS, shields)
            .put_u8(PLAYERS_DATA + 0x21c + PLAYER_HANDICAP, handicap);
        g.map(OVERSHIELD_SCALE & !0xfff, 0x1000)
            .put_f32(OVERSHIELD_SCALE, 3.0);
        perceive(&g, machine(1)).unwrap().me.unwrap().shield_full
    };
    assert_eq!(full(0, 0), 1.0);
    assert_eq!(full(0, 1), 0.75);
    assert_eq!(full(0, 2), 0.5);
    assert_eq!(full(0, 3), 0.25);
    assert_eq!(full(1, 0), 0.0, "no shields");
    assert_eq!(full(1, 3), 0.0);
    assert_eq!(full(2, 0), 3.0, "overshields: the image's scale");
    assert_eq!(full(2, 3), 0.75);
}

#[test]
fn a_phantom_perceives_itself_by_machine_id() {
    let ps = vec![
        (0, [0.0; 3], X, false),
        (1, [5.0, 1.0, 0.0], [0.0, 1.0, 0.0], false),
        (2, [9.0, 0.0, 0.0], X, false),
    ];
    let mut g = game(&ps, true);
    let mine = [0xc1, 0xa4, 0, 0, 0, 7];
    for (k, b) in (0u32..).zip(mine) {
        g.put_u8(PLAYERS_DATA + 0x21c + PLAYER_MACHINE + k, b);
        g.put_u8(
            PLAYERS_DATA + 2 * 0x21c + PLAYER_MACHINE + k,
            if k == 5 { 8 } else { b },
        );
    }
    let p = perceive(&g, mine).unwrap();
    assert_eq!(p.me.map(|m| m.position), Some([5.0, 1.0, 0.0]));
    let ids: Vec<u32> = p.others.iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![0, 2], "the other players");
    assert_eq!(perceive(&g, [9; 6]).unwrap().me, None);
}

#[test]
fn the_tick_comes_from_the_game_time() {
    let mut g = three();
    g.map(0x0068_0000, 0x10)
        .put_u32(points::GAME_TIME.base, 0x0068_0000)
        .put_u32(0x0068_0000 + GAME_TIME_TICK, 1234);
    assert_eq!(perceive(&g, machine(1)).unwrap().tick, 1234);
}

#[test]
fn there_is_no_game_tick_in_the_menus() {
    assert_eq!(game_tick(&FakeGuest::new()), None, "unmapped");
    let mut g = three();
    g.put_u32(points::GAME_TIME.base, 0);
    assert_eq!(game_tick(&g), None, "no game-time record");
    g.map(0x0068_0000, 0x10)
        .put_u32(points::GAME_TIME.base, 0x0068_0000)
        .put_u32(0x0068_0000 + GAME_TIME_TICK, 7);
    assert_eq!(game_tick(&g), Some(7));
}

#[test]
fn outside_a_multiplayer_game_the_bot_sees_nothing() {
    let mut g = three();
    g.put_u32(GAME + OPTIONS, 1); // campaign
    assert_eq!(perceive(&g, machine(1)).unwrap(), Perception::default());
    g.put_u32(points::GAME_GLOBALS.base, 0);
    assert_eq!(perceive(&g, machine(1)).unwrap(), Perception::default());
}

// ---- the objective (section 3.7) ----

mod objective_reads {
    use super::*;
    use crate::objective::{
        DESIGNATOR_MASK, DESIGNATOR_TEAMS, ENGINE_INDEX, GAME_ENGINE, Game, Hill, Item,
        KOTH_GLOBALS, KOTH_HILL, NETGAME_FLAGS, OBJECTIVE_COUNT, OBJECTIVES, Spot, kind,
    };

    const ENGINE: u32 = 0x0070_0000;
    const SCN: u32 = 0x0071_0000;
    const FLAGS: u32 = 0x0072_0000;
    const KOTH: u32 = 0x0073_0000;

    /// A human (0, red) and the phantom (1, red) at X 5, and a blue
    /// enemy (2) at X 50; objects 3 and 4 are weapons at `at3` and `at4`.
    fn ctf_game(at3: [f32; 3], at4: [f32; 3]) -> FakeGuest {
        let mut g = game(
            &[
                (0, [0.0; 3], X, false),
                (0, [5.0, 0.0, 0.0], X, false),
                (1, [50.0, 0.0, 0.0], X, false),
            ],
            true,
        );
        g.put_u32(OBJECTS_HEADER + dl::HIGH_WATER, 5);
        for (i, at) in [(3u32, at3), (4, at4)] {
            let h = OBJECTS_DATA + i * 0xc;
            let body = BODIES + i * 0x400;
            g.put_u16(h, 0xb000 + i as u16)
                .put_u8(h + 3, objects::WEAPON)
                .put_u32(h + 8, body);
            g.map(body, 0x400);
            for k in 0..3 {
                g.put_f32(body + 0x64 + 4 * k, at[k as usize]);
            }
        }
        g.put_u32(GAME_ENGINE, ENGINE).map(ENGINE, 0x1000);
        g.put_u32(ENGINE + ENGINE_INDEX, 1);
        // Designator 0 is blue's, 1 red's: the netgame flags' teams are
        // designators, not teams.
        g.put_u16(ENGINE + DESIGNATOR_MASK, 0b11)
            .put_u16(ENGINE + DESIGNATOR_TEAMS, 1)
            .put_u16(ENGINE + DESIGNATOR_TEAMS + 2, 0);
        g.put_u32(points::SCENARIO.base, SCN).map(SCN, 0x200);
        g
    }

    fn record(g: &mut FakeGuest, i: u32, object: u32, team: i16, unit: u32, player: u32) {
        let r = ENGINE + OBJECTIVES + i * 0x10;
        g.put_u32(r, object)
            .put_u16(r + 4, team as u16)
            .put_u32(r + 8, unit)
            .put_u32(r + 0xc, player);
    }

    fn flags(g: &mut FakeGuest, fs: &[([f32; 3], u16, u16)]) {
        g.put_u32(SCN + NETGAME_FLAGS, fs.len() as u32)
            .put_u32(SCN + NETGAME_FLAGS + 4, FLAGS);
        for (i, &(at, ty, team)) in (0u32..).zip(fs) {
            let f = FLAGS + i * 0x20;
            for k in 0..3 {
                g.put_f32(f + 4 * k, at[k as usize]);
            }
            g.put_u16(f + 0x10, ty).put_u16(f + 0x12, team);
        }
    }

    #[test]
    fn ctf_flags_their_carriers_and_spawns_are_read() {
        let mut g = ctf_game([1.0, 2.0, 3.0], [0.0; 3]);
        g.put_u32(ENGINE + OBJECTIVE_COUNT, 2);
        let red_flag = DatumHandle::new(0xb003, 3).raw();
        let blue_flag = DatumHandle::new(0xb004, 4).raw();
        // Red's lies at 1,2,3; blue's is carried by the enemy (player 2,
        // unit 2): it is where they are.
        record(&mut g, 0, red_flag, 0, u32::MAX, u32::MAX);
        record(
            &mut g,
            1,
            blue_flag,
            1,
            DatumHandle::new(0xb002, 2).raw(),
            DatumHandle::new(0xe002, 2).raw(),
        );
        flags(
            &mut g,
            &[
                ([10.0, 0.0, 0.0], kind::CTF_SPAWN, 0),
                ([-10.0, 0.0, 0.0], kind::CTF_SPAWN, 1),
                ([-9.0, 1.0, 0.0], kind::CTF_RETURN, 1),
                ([0.0, 0.0, 0.0], 7, 0),
            ],
        );
        let p = perceive(&g, machine(1)).unwrap();
        let o = p.objective.expect("an objective");
        assert_eq!((o.game, o.me), (Game::Ctf, 1));
        assert_eq!(
            o.items,
            vec![
                Item {
                    id: red_flag,
                    team: 0,
                    position: [1.0, 2.0, 3.0],
                    carrier: None,
                },
                Item {
                    id: blue_flag,
                    team: 1,
                    position: [50.0, 0.0, 0.0],
                    carrier: Some(2),
                },
            ]
        );
        assert_eq!(
            o.spots,
            vec![
                Spot {
                    kind: kind::CTF_SPAWN,
                    team: Some(1),
                    position: [10.0, 0.0, 0.0],
                },
                Spot {
                    kind: kind::CTF_SPAWN,
                    team: Some(0),
                    position: [-10.0, 0.0, 0.0],
                },
                Spot {
                    kind: kind::CTF_RETURN,
                    team: Some(0),
                    position: [-9.0, 1.0, 0.0],
                },
            ],
            "the teleporter is not a spot"
        );
        // Clearing the mask removes the binding, even if the table still has a value.
        g.put_u16(ENGINE + DESIGNATOR_MASK, 0);
        let o = perceive(&g, machine(1)).unwrap().objective.unwrap();
        assert_eq!(o.spots[0].team, None);
    }

    #[test]
    fn slayer_or_a_fault_reads_no_objective_and_the_bot_still_sees() {
        let mut g = ctf_game([0.0; 3], [0.0; 3]);
        g.put_u32(ENGINE + ENGINE_INDEX, 2);
        let p = perceive(&g, machine(1)).unwrap();
        assert!(p.me.is_some());
        assert_eq!(p.objective, None, "Slayer");
        // A record pointing at unmapped memory: no objective, not blind.
        g.put_u32(ENGINE + ENGINE_INDEX, 1)
            .put_u32(ENGINE + OBJECTIVE_COUNT, 1);
        g.put_u32(points::SCENARIO.base, 0x0dea_d000);
        let p = perceive(&g, machine(1)).unwrap();
        assert!(p.me.is_some());
        assert_eq!(p.objective, None);
        // No engine globals at all.
        g.put_u32(GAME_ENGINE, 0);
        assert_eq!(perceive(&g, machine(1)).unwrap().objective, None);
    }

    #[test]
    fn territories_resolve_the_marker_owner_salt_and_contesting_players() {
        use crate::objective::{State, Territory};
        let mut g = ctf_game([0.0; 3], [0.0; 3]);
        let t = ENGINE + 0xfc;
        g.put_u32(ENGINE + ENGINE_INDEX, 8)
            .put_u32(points::TERRITORIES_GLOBALS.base, t)
            .put_u16(t + 0x114, 2)
            .put_u16(t + 0x60, 1)
            .put_u16(t + 0x62, 0)
            .put_u32(t + 0x70, 0xe0000000)
            .put_u32(t + 0x74, 0xe0010002)
            .put_u8(t + 0x95, 255)
            .put_u8(t + 0x9d, 0)
            .put_u8(t + 0xa5, 0);
        flags(
            &mut g,
            &[
                ([9.0; 3], kind::TERRITORY, 8),
                ([1.0; 3], kind::TERRITORY, 8),
            ],
        );
        let read = perceive(&g, machine(1)).unwrap().objective.unwrap();
        assert_eq!(
            read.state,
            State::Territories(vec![
                Territory {
                    at: [1.0; 3],
                    owner: Some(0),
                    owner_team: Some(0),
                    occupants: 6
                },
                Territory {
                    at: [9.0; 3],
                    owner: None,
                    owner_team: None,
                    occupants: 0
                },
            ])
        );
    }

    #[test]
    fn assault_reads_the_bomb_team_active_plates_and_receiving_team() {
        use crate::objective::State;
        let mut g = ctf_game([1.0; 3], [0.0; 3]);
        let a = ENGINE + 0xfc;
        g.put_u32(ENGINE + ENGINE_INDEX, 9)
            .put_u32(points::CTF_ASSAULT_GLOBALS.base, a)
            .put_u16(ENGINE + 0xc, 3)
            .put_u32(GAME + 0x240, 1)
            .put_u32(a + 0x1f4, 1)
            .put_u32(ENGINE + OBJECTIVE_COUNT, 1);
        record(&mut g, 0, 0xb0030003, 8, u32::MAX, u32::MAX);
        flags(
            &mut g,
            &[
                ([1.0; 3], kind::BOMB_RETURN, 0),
                ([2.0; 3], kind::BOMB_RETURN, 0),
                ([3.0; 3], kind::BOMB_RETURN, 1),
            ],
        );
        for d in 0..9 {
            g.put_u16(a + 0x14 + 2 * d, u16::MAX)
                .put_u16(a + 0x26 + 2 * d, u16::MAX)
                .put_u16(a + 0x1a0 + 2 * d, u16::MAX);
        }
        g.put_u16(a + 0x26, 1)
            .put_u16(a + 0x16, 2)
            .put_u16(a + 0x1b0, 18)
            .put_u8(a + 0x178, 10);
        let read = perceive(&g, machine(1)).unwrap().objective.unwrap();
        assert_eq!(read.items[0].team, 8);
        assert_eq!(
            read.state,
            State::Assault {
                defending: Some(1),
                armed: 256,
                arming: 256,
                planted: 256
            }
        );
        assert_eq!(
            read.spots
                .iter()
                .map(|s| (s.team, s.position))
                .collect::<Vec<_>>(),
            [(Some(1), [2.0; 3]), (Some(0), [3.0; 3])]
        );
        g.put_u32(GAME + 0x240, 0);
        assert!(matches!(
            perceive(&g, machine(1)).unwrap().objective.unwrap().state,
            State::Assault {
                defending: None,
                ..
            }
        ));
    }

    #[test]
    fn juggernaut_mask_transfers_and_oddball_accepts_neutral_type_four() {
        let mut g = ctf_game([0.0; 3], [0.0; 3]);
        g.put_u32(ENGINE + ENGINE_INDEX, 7)
            .put_u32(points::JUGGERNAUT_GLOBALS.base, ENGINE + 0xfc);
        for slot in [1, 2] {
            g.put_u16(ENGINE + 0xfc, 1 << slot);
            assert_eq!(
                perceive(&g, machine(1)).unwrap().objective.unwrap().am_juggernaut(),
                slot == 1
            );
        }
        g.put_u32(ENGINE + ENGINE_INDEX, 3)
            .put_u32(points::ODDBALL_GLOBALS.base, ENGINE + 0xfc)
            .put_u32(ENGINE + 0xfc + 12, 0)
            .put_u32(ENGINE + 0xfc + 16, 1)
            .put_u32(ENGINE + 0xfc + 20, 2);
        flags(
            &mut g,
            &[([1.0; 3], 4, 8), ([2.0; 3], 4, 0), ([3.0; 3], 3, 8)],
        );
        let read = perceive(&g, machine(1)).unwrap().objective.unwrap();
        assert_eq!(read.spots.len(), 1);
        assert_eq!(read.spots[0].position, [1.0; 3]);
    }

    #[test]
    fn the_live_spawner_counts_seconds_at_each_placements_tick_phase() {
        use crate::decisions::supplies::Clock;
        let mut g = ctf_game([0.0; 3], [0.0; 3]);
        let time = KOTH;
        g.put_u32(points::GAME_TIME.base, time)
            .put_u16(time + 2, 20)
            .put_u32(SCN + 0x120, 4);
        for (i, n) in (0..4).zip([2, 1, 0, u16::MAX]) {
            g.put_u16(ENGINE + 0x7e0 + 8 * i, n);
        }
        assert_eq!(
            intel::clocks(&g, 40).unwrap(),
            [
                Clock::Due(80),
                Clock::Due(41),
                Clock::Idle {
                    service: 42,
                    rate: 20,
                    deleted: false
                },
                Clock::Disabled
            ]
        );
        assert_eq!(intel::clocks(&g, 39).unwrap()[0], Clock::Due(60));
        let observe = |g: &FakeGuest| perceive_for_brain(g, machine(1)).unwrap().0;
        let before = observe(&g);
        g.put_u16(ENGINE + 0x7e0, 29);
        assert_ne!(intel::clocks(&g, 40).unwrap()[0], Clock::Due(80));
        crate::perceive::honesty::paired_worlds(
            Some(&crate::cache::MapData::default()), [(before, observe(&g))],
        );
    }

    #[test]
    fn observations_read_both_held_object_handles_and_powerup_effects() {
        let mut g = ctf_game([0.0; 3], [0.0; 3]);
        let body = BODIES + 2 * 0x400;
        g.put_u8(body + 0x212, 1)
            .put_u8(body + 0x213, 2)
            .put_u32(body + 0x21c, 0xb0030003)
            .put_u32(body + 0x220, 0xb0040004)
            .put_f32(body + 0xf0, 1.6)
            .put_u16(PLAYERS_DATA + 2 * 0x21c + 0x198, 45);
        let p = perceive(&g, machine(1)).unwrap();
        let holder = p
            .observations
            .supply_players
            .iter()
            .find(|p| p.id == 2)
            .unwrap();
        assert_eq!(holder.hands, [Some(0xb0030003), Some(0xb0040004)]);
        assert!(holder.overshield && holder.camo);
        assert_eq!(holder.unit, 0xb0020002);
    }

    #[test]
    fn successful_weapon_and_equipment_events_identify_the_exact_placement_player_and_tick() {
        let mut g = ctf_game([1.0; 3], [0.0; 3]);
        let object = 0xb0030003;
        let unit = 0xb0010001;
        let body = BODIES + 3 * 0x400;
        let unit_body = BODIES + 0x400;
        let site = FLAGS + 0x90;
        g.put_u32(points::GAME_TIME.base, KOTH)
            .put_u32(KOTH + 8, 113)
            .put_u32(SCN + 0x120, 2)
            .put_u32(SCN + 0x124, FLAGS)
            .put_u32(ENGINE + 0x7dc + 8, object)
            .put_u8(body + 0xaf, 1)
            .put_u32(body + 0x14, u32::MAX)
            .put_u32(unit_body + 0x13c, 0xe0010001);
        for k in 0..3 {
            g.put_f32(site + 0x40 + 4 * k, 1.0);
        }
        let expected = crate::decisions::supplies::Take {
            at: [1.; 3],
            site: 1,
            object,
            unit,
            player: 1,
            tick: 113,
        };
        assert_eq!(
            pickups::take(&g, object, unit, objects::WEAPON, None).unwrap(),
            Some(expected)
        );
        g.put_u32(ENGINE + 0x7dc + 8, object ^ (1 << 16));
        assert_eq!(
            pickups::take(&g, object, unit, objects::WEAPON, None).unwrap(),
            None
        );
        g.put_u32(ENGINE + 0x7dc + 8, object)
            .put_u8(OBJECTS_DATA + 3 * 0xc + 3, intel::EQUIPMENT)
            .put_u32(body, 0xd0040004)
            .put_u32(STACK, 0x151fd2)
            .put_u32(STACK + 0x20, object);
        assert_eq!(
            pickups::equipment_event(&g, STACK, unit, 0xd0040004).unwrap(),
            Some(expected)
        );
        assert_eq!(
            pickups::equipment_event(&g, STACK, unit, 0xd0050004).unwrap(),
            None
        );
        g.put_u32(STACK, 0x111111);
        assert_eq!(
            pickups::equipment_event(&g, STACK, unit, 0xd0040004).unwrap(),
            None
        );
    }

    #[test]
    fn koth_reads_the_hill_in_play_from_its_corners() {
        let mut g = ctf_game([0.0; 3], [0.0; 3]);
        g.put_u32(ENGINE + ENGINE_INDEX, 4);
        flags(
            &mut g,
            &[
                ([0.0, 0.0, 0.0], kind::HILL, 8),
                ([4.0, 0.0, 0.0], kind::HILL, 8),
                ([20.0, 0.0, 1.0], kind::HILL + 1, 8),
                ([24.0, 4.0, 1.0], kind::HILL + 1, 8),
                ([20.0, 4.0, 1.0], kind::HILL + 1, 8),
                ([24.0, 0.0, 1.0], kind::HILL + 1, 8),
                ([2.0, 2.0, 0.0], kind::HILL, 8),
                ([2.0, -2.0, 0.0], kind::HILL, 8),
            ],
        );
        g.put_u32(KOTH_GLOBALS, KOTH).map(KOTH, 0x200);
        g.put_u32(KOTH + KOTH_HILL, 1)
            .put_u32(KOTH + 0x60, 4)
            .put_u16(KOTH + 0x1a8, 2);
        for i in 0..4 {
            g.put_u32(KOTH + 4 * i, i + 2);
        }
        let o = perceive(&g, machine(1)).unwrap().objective.unwrap();
        assert_eq!(o.game, Game::Koth);
        assert!(o.spots.is_empty());
        assert!(o.in_hill(1, [0.0; 3]));
        assert!(!o.in_hill(2, [22.0, 2.0, 1.0]));
        let h: Hill = o.hill.expect("a hill");
        assert_eq!(h.corners.len(), 4);
        assert_eq!(h.center(), [22.0, 2.0, 1.0]);
        // The hill moves.
        g.put_u32(KOTH + KOTH_HILL, 0).put_u16(KOTH + 0x1a8, 0);
        for (i, f) in (0..4).zip([0, 1, 6, 7]) {
            g.put_u32(KOTH + 4 * i, f);
        }
        let h = perceive(&g, machine(1)).unwrap().objective.unwrap().hill.unwrap();
        assert_eq!(h.center(), [2.0, 0.0, 0.0]);
        // Out of range: no hill.
        g.put_u32(KOTH + KOTH_HILL, 9);
        assert_eq!(perceive(&g, machine(1)).unwrap().objective.unwrap().hill, None);
    }
}

#[test]
fn decisions_observe_player_slot_while_dead_and_only_live_contacts() {
    let players = [
        (0, [0.0; 3], [0.0, 1.0, 0.0], false),
        (1, [4.0, 0.0, 0.0], X, false),
        (1, [5.0, 0.0, 0.0], X, true),
    ];
    let mut g = game(&players, true);
    g.put_f32(BODIES + 0xf0, 0.3);
    g.put_u32(BODIES + BIPED_UNIT_FLAGS, BIPED_CROUCH_REQUEST);
    let p = perceive(&g, machine(1)).unwrap();
    assert_eq!(p.observations.player, Some((1, 1)));
    assert_eq!(
        p.observations.contacts,
        [crate::brain::Contact {
            weapon: None,
            id: 0,
            aim: [0.0, 1.0, 0.0],
            shield: None,
            empty_magazine: false,
            crouching: true,
        }]
    );
    assert!(p.observations.pickups_scanned);
    assert_eq!(p.observations.supplies, Some(Vec::new()));
    g.put_u16(BODIES + 0x400 + 0x10a, BODY_DEPLETED);
    let p = perceive(&g, machine(1)).unwrap();
    assert!(p.me.is_none());
    assert_eq!(p.observations.player, Some((1, 1)));
    assert!(!p.observations.pickups_scanned);
    assert_eq!(p.observations.supplies, None);
}

#[test]
fn decisions_lead_reads_only_competing_scoreboard_rows_including_dead_players() {
    let roster = [(0, 0), (1, 0), (2, 1)];
    let mut g = game(&[(0, [0.; 3], X, false)], true);
    let engine = 0x006b0000;
    g.put_u32(crate::objective::GAME_ENGINE, engine);
    g.put_u32(engine + 0x304, 1);
    g.put_u16(engine + 0x4c8, 3);
    g.put_u16(engine + 0x4c8 + 0x12, 2);
    assert!(intel::ahead(&g, 0, true, &roster).unwrap());
    g.put_u16(engine + 0x4c8 + 0x12, 3);
    assert!(!intel::ahead(&g, 0, true, &roster).unwrap());
    g.put_u16(engine + 0x308, 5);
    g.put_u16(engine + 0x308 + 0x1c, 6);
    g.put_u16(engine + 0x308 + 2 * 0x1c, 2);
    assert!(!intel::ahead(&g, 0, false, &roster).unwrap());
}

#[test]
fn decisions_honesty_replays_hidden_changes_from_the_guest_object_table() {
    let players = [
        (0, [0.; 3], X, false),
        (1, [5., 0., 0.], X, false),
    ];
    let live = game(&players, false);
    let mut missing = live.clone();
    missing.put_u16(BODIES + 0x400 + 0x10a, BODY_DEPLETED);
    let (left, _) = perceive_for_brain(&live, machine(0)).unwrap();
    let (right, _) = perceive_for_brain(&missing, machine(0)).unwrap();
    assert_eq!(left.others.len(), 1);
    assert!(right.others.is_empty());
    let map = crate::cache::MapData {
        walls: crate::los::Walls::new(vec![[[2., -5., -3.], [2., 5., -3.], [2., 0., 5.]]]),
        ..Default::default()
    };
    crate::perceive::honesty::paired_worlds(Some(&map), [(left, right)]);
}

#[test]
fn hidden_motion_makes_an_anonymous_horizontal_blip_only_while_the_sensor_can_show_it() {
    use live_proto::hopper::variant::flag::{MOTION_SENSOR, TEAM_PLAY};

    let mut g = game(&[
        (0, [0.; 3], X, false),
        (0, [0., 1., 0.], X, false),
        (1, [0., 4.3, 9.], X, false),
    ], true);
    const HUD: u32 = 0x006b_0000;
    const MATG: u32 = 0x006c_0000;
    const CONTROLS: u32 = 0x006d_0000;
    let enemy = BODIES + 0x800;
    let flags = GAME + OPTIONS + VARIANT_FLAGS;
    g.put_u32(flags, TEAM_PLAY | MOTION_SENSOR)
        .put_u32(points::HUD_GLOBALS_TAG.base, HUD)
        .put_f32(HUD + 0x290, 6.0)
        .put_u32(points::GLOBALS_TAG.base, MATG)
        .put_u32(MATG + 0x134, CONTROLS)
        .put_f32(CONTROLS + 0x3c, 1.0)
        .put_f32(0x0045_e1f0, 0.5)
        .put_f32(0x0045_e1f4, 0.5)
        .put_u8(BODIES + radar::UNIT_ZOOM, u8::MAX)
        .put_f32(BODIES + 0x400 + 0x88, 1.0)
        .put_f32(enemy + 0x88, 1.0);
    let observed = |g: &FakeGuest| {
        let mut p = perceive(g, machine(0)).unwrap();
        crate::perceive::observe(&mut p, &mut Default::default(), &mut [], None);
        assert!(p.others.iter().all(|o| p.teams && o.team == 0));
        assert!(p.observations.contacts.is_empty());
        p.observations.radar
    };
    let none: Vec<[f32; 3]> = vec![];
    assert_eq!(observed(&g), [[0., 4.5, 0.]]);
    g.put_u32(flags, MOTION_SENSOR);
    assert_eq!(
        observed(&g),
        [[0., 1., 0.], [0., 4.5, 0.]],
        "FFA: both, nearer first"
    );
    g.put_u32(flags, TEAM_PLAY | MOTION_SENSOR)
        .put_u32(enemy + BIPED_UNIT_FLAGS, BIPED_CROUCH_REQUEST);
    assert_eq!(observed(&g), none);
    g.put_u32(enemy + BIPED_UNIT_FLAGS, 0).put_f32(enemy + 0x88, 0.25);
    assert_eq!(observed(&g), none);
    g.put_f32(enemy + 0x88, 3.0).put_f32(HUD + 0x290, 4.0);
    assert_eq!(observed(&g), none);
    g.put_f32(HUD + 0x290, 6.0).put_u8(BODIES + radar::UNIT_ZOOM, 0);
    assert_eq!(observed(&g), none);
    g.put_u8(BODIES + radar::UNIT_ZOOM, u8::MAX).put_u32(flags, TEAM_PLAY);
    assert_eq!(observed(&g), none);
}

#[test]
fn a_firing_stander_shows_and_a_crouch_walker_does_not() {
    use live_proto::hopper::variant::flag::{MOTION_SENSOR, TEAM_PLAY};

    let mut g = game(
        &[(0, [0.; 3], X, false), (1, [0., 4.3, 0.], X, false)],
        true,
    );
    const HUD: u32 = 0x006b_0000;
    const MATG: u32 = 0x006c_0000;
    const CONTROLS: u32 = 0x006d_0000;
    let enemy = BODIES + 0x400;
    g.put_u32(GAME + OPTIONS + VARIANT_FLAGS, TEAM_PLAY | MOTION_SENSOR)
        .put_u32(points::HUD_GLOBALS_TAG.base, HUD)
        .put_f32(HUD + 0x290, 6.0)
        .put_u32(points::GLOBALS_TAG.base, MATG)
        .put_u32(MATG + 0x134, CONTROLS)
        .put_f32(CONTROLS + 0x3c, 1.0)
        .put_f32(0x0045_e1f0, 0.5)
        .put_f32(0x0045_e1f4, 0.5)
        .put_u8(BODIES + radar::UNIT_ZOOM, u8::MAX);
    let observed = |g: &FakeGuest, firing: fn(u32) -> u16| {
        let mut p = Perception::default();
        perception_with(g, machine(0), false, &mut p, &mut Vec::new(), None, firing).unwrap();
        crate::perceive::observe(&mut p, &mut Default::default(), &mut [], None);
        assert_eq!(p.observations.radar_range, Some(6.0));
        p.observations.radar
    };
    let none: Vec<[f32; 3]> = vec![];
    // Standing still: no dot until it fires.
    assert_eq!(observed(&g, |_| 0), none);
    assert_eq!(observed(&g, |_| 1 << 1), [[0., 4.5, 0.]]);
    // Another slot's fire shows nothing of this one.
    assert_eq!(observed(&g, |_| 1 << 3), none);
    // A crouch-walker at walking speed: no dot while it holds its fire.
    g.put_f32(enemy + 0x88, 1.0)
        .put_u32(enemy + BIPED_UNIT_FLAGS, BIPED_CROUCH_REQUEST);
    assert_eq!(observed(&g, |_| 0), none);
    // Upright at the same speed it shows.
    g.put_u32(enemy + BIPED_UNIT_FLAGS, 0);
    assert_eq!(observed(&g, |_| 0), [[0., 4.5, 0.]]);
}

#[test]
fn only_a_visible_enemy_with_depleted_shields_gives_a_melee_finishing_cue() {
    let mut g = three();
    let map = crate::cache::MapData::default();
    let shield = BODIES + 0x400 + 0xf0;
    let observed = |g: &FakeGuest, geometry| {
        let mut p = perceive(g, machine(0)).unwrap();
        crate::perceive::observe(&mut p, &mut Default::default(), &mut [], geometry);
        p.observations.contacts.into_iter().find(|c| c.id == 1).and_then(|c| c.shield)
    };
    g.put_f32(shield, 0.);
    assert_eq!(observed(&g, Some(&map)), Some(0.));
    assert_eq!(observed(&g, None), None);
    g.put_f32(shield, 0.35);
    assert_eq!(observed(&g, Some(&map)), None);
}

#[test]
fn the_breakable_state_reads_each_bsps_mask_after_the_enable_byte() {
    const STATE: u32 = 0x0068_0000;
    let mut g = FakeGuest::new();
    g.put_u32(BREAKABLE_STATE, 0);
    assert_eq!(breakable(&g, 0).unwrap(), None, "no map: a null pointer");
    g.put_u32(BREAKABLE_STATE, STATE).map(STATE, 0x100);
    // Everything whole on BSP 0, then index 3 and index 255 broken: the
    // mask starts one byte in, unaligned.
    for i in 0..32 {
        g.put_u8(STATE + 1 + i, 0xff);
    }
    g.put_u8(STATE, 1);
    g.put_u8(STATE + 1, 0xff & !(1 << 3));
    g.put_u8(STATE + 1 + 31, 0x7f);
    // BSP 1's mask follows, 32 bytes on.
    g.put_u8(STATE + 1 + 32, 0x01);
    let b = breakable(&g, 0).unwrap().unwrap();
    assert!(b.enabled);
    assert!(!crate::glass::intact(&b.mask, 3));
    assert!(crate::glass::intact(&b.mask, 4));
    assert_eq!(b.broken(), vec![3, 255]);
    let hex = b.mask_hex();
    assert_eq!(hex.len(), 64);
    assert!(hex.starts_with("f7ff") && hex.ends_with("ff7f"), "{hex}");
    let one = breakable(&g, 1).unwrap().unwrap();
    assert!(crate::glass::intact(&one.mask, 0));
    assert!(!crate::glass::intact(&one.mask, 1));
    g.put_u8(STATE, 0);
    assert!(!breakable(&g, 0).unwrap().unwrap().enabled);
}
