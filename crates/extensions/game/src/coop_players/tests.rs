use super::*;
use guest_layout::entry::Regs;
use guest_layout::entry::testing::run;
use guest_layout::testing::FakeGuest;

const PLAYERS_HDR: u32 = 0x0010_0000;
const PLAYERS_DATA: u32 = 0x0011_0000;
const PLAYERS_BITS: u32 = 0x0012_0000;
const OBJECTS_HDR: u32 = 0x0020_0000;
const OBJECTS_DATA: u32 = 0x0021_0000;
const OBJECT_BASE: u32 = 0x0030_0000;
const GLOBALS: u32 = 0x0040_0000;
const SCENARIO: u32 = 0x0050_0000;
const STACK: u32 = 0x0060_0000;
const SCRATCH: u32 = 0x0070_0000;
const START_BASE: u32 = 0x0090_0000;
const START_STRIDE: u32 = 0x34;

/// A campaign with `count` starting locations and no players.
fn world(count: u32) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.put_u32(points::PLAYERS.base, PLAYERS_HDR)
        .put_u32(PLAYERS_HDR + ARRAY_DATA, PLAYERS_DATA)
        .put_u32(PLAYERS_HDR + ARRAY_LIMIT, 16)
        .put_u32(PLAYERS_HDR + ARRAY_IN_USE, PLAYERS_BITS)
        .put_u32(PLAYERS_BITS, 0)
        .map(PLAYERS_DATA, 16 * PLAYER_SIZE)
        .put_u32(points::OBJECTS.base, OBJECTS_HDR)
        .put_u32(OBJECTS_HDR + ARRAY_DATA, OBJECTS_DATA)
        .map(OBJECTS_DATA, 64 * OBJECT_HEADER)
        .put_u32(points::GAME_GLOBALS.base, GLOBALS)
        .put_u32(GLOBALS + OPTIONS_MODE, CAMPAIGN)
        .put_u32(points::SCENARIO.base, SCENARIO)
        .put_u32(SCENARIO + SCENARIO_STARTS, count)
        .put_u32(SCENARIO + SCENARIO_STARTS + 4, START_BASE)
        .map(START_BASE, 16 * START_STRIDE)
        .map(STACK - 0x100, 0x200)
        .map(SCRATCH, 0x20);
    g
}

/// Starting location `i` at `p`.
fn put_start(g: &mut FakeGuest, i: u32, p: [f32; 3]) {
    for (k, v) in p.iter().enumerate() {
        g.put_f32(START_BASE + i * START_STRIDE + 4 * k as u32, *v);
    }
}

fn start_at(m: &dyn GuestMemory, i: u32) -> [f32; 3] {
    m.vec3(START_BASE + i * START_STRIDE).unwrap()
}

/// Unit handle for player `i`'s unit (object index `0x10 + i`).
fn unit_of(i: u16) -> u32 {
    0x00ab_0010 + u32::from(i)
}

fn handle_of(i: u16) -> u32 {
    0x00e1_0000 | u32::from(i)
}

/// Adds player `i`: in use, with `flags`, and a unit at `pos` (parented to
/// `parent`) unless `pos` is `None`.
fn add_player(g: &mut FakeGuest, i: u16, flags: u8, pos: Option<[f32; 3]>, parent: u32) {
    let bits = g.u32(PLAYERS_BITS).unwrap() | (1 << i);
    g.put_u32(PLAYERS_BITS, bits);
    let at = PLAYERS_DATA + u32::from(i) * PLAYER_SIZE;
    g.put_u16(at, 0x00e1).put_u8(at + PLAYER_FLAGS, flags);
    match pos {
        None => {
            g.put_u32(at + PLAYER_UNIT, NONE);
        }
        Some(p) => {
            let unit = unit_of(i);
            g.put_u32(at + PLAYER_UNIT, unit);
            put_object(g, unit, p, parent);
        }
    }
}

fn put_object(g: &mut FakeGuest, unit: u32, p: [f32; 3], parent: u32) {
    let idx = unit & 0xffff;
    let obj = OBJECT_BASE + idx * 0x100;
    g.put_u32(OBJECTS_DATA + idx * OBJECT_HEADER + OBJECT_ADDRESS, obj)
        .put_u32(obj + OBJECT_PARENT, parent);
    for (k, v) in p.iter().enumerate() {
        g.put(obj + OBJECT_POSITION + 4 * k as u32, &v.to_le_bytes());
    }
}

fn entered_with(g: &mut FakeGuest, arg: u32) -> Regs {
    g.put_u32(STACK, 0xdead_beef).put_u32(STACK + 4, arg);
    Regs {
        esp: STACK,
        ..Regs::default()
    }
}

#[test]
fn players_objects_and_the_campaign_are_read_from_the_tables() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([1.0, 2.0, 3.0]), NONE);
    add_player(&mut g, 2, 1 | FIRST_SPAWN, None, NONE);
    let ps = players(&g);
    assert_eq!(ps.len(), 2, "{ps:?}");
    assert_eq!(ps[0].handle, handle_of(0));
    assert_eq!(ps[0].unit, unit_of(0));
    assert_eq!((ps[1].index, ps[1].unit), (2, NONE));
    assert!(takes_spawn(&ps[1]) && !takes_spawn(&ps[0]));
    assert_eq!(
        object(&g, unit_of(0)),
        Some(Object {
            parent: NONE,
            position: [1.0, 2.0, 3.0]
        })
    );
    assert!(is_campaign(&g));
    assert_eq!(start_count(&g), 2);
    g.put_u32(GLOBALS + OPTIONS_MODE, 2);
    assert!(!is_campaign(&g), "multiplayer");
}

#[test]
fn the_anchor_is_player_0_on_foot_else_player_1() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([1.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 1, 1, Some([2.0, 0.0, 0.0]), NONE);
    assert_eq!(
        anchor(&g, &players(&g)),
        Some((unit_of(0), [1.0, 0.0, 0.0]))
    );
    // Player 0 drives: player 1 anchors.
    put_object(&mut g, unit_of(0), [1.0, 0.0, 0.0], 0x00ab_0030);
    assert_eq!(
        anchor(&g, &players(&g)),
        Some((unit_of(1), [2.0, 0.0, 0.0]))
    );
    // Both in vehicles: none. Player 2 never anchors.
    put_object(&mut g, unit_of(1), [2.0, 0.0, 0.0], 0x00ab_0030);
    add_player(&mut g, 2, 1, Some([9.0, 0.0, 0.0]), NONE);
    assert_eq!(anchor(&g, &players(&g)), None);
}

#[test]
fn stragglers_are_players_three_and_up_on_foot_past_the_leash() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 1, 1, Some([100.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, Some([0.0, LEASH + 1.0, 0.0]), NONE);
    add_player(&mut g, 3, 1, Some([0.0, LEASH - 1.0, 0.0]), NONE);
    add_player(&mut g, 4, 1, Some([0.0, 0.0, 50.0]), 0x00ab_0030);
    add_player(&mut g, 5, 1, None, NONE);
    let ps = players(&g);
    assert_eq!(
        stragglers(&g, &ps, [0.0, 0.0, 0.0]),
        vec![handle_of(2)],
        "player 1 is the script's; 3 is near; 4 drives; 5 is dead"
    );
}

#[test]
fn the_scripts_player0_and_player1_are_the_first_two_with_units() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 2, 1, Some([0.0; 3]), NONE);
    let ps = players(&g);
    assert!(is_named_player_unit(&ps, unit_of(0)));
    assert!(is_named_player_unit(&ps, unit_of(1)));
    assert!(!is_named_player_unit(&ps, unit_of(2)));
    assert!(!is_named_player_unit(&ps, 0x1234_5678), "a marine");
    // Player 1 dead: (list_get (players) 1) is player 2.
    add_player(&mut g, 1, 1, None, NONE);
    assert!(is_named_player_unit(&players(&g), unit_of(2)));
}

#[test]
fn a_ring_is_nearest_first_and_the_starts_take_turns() {
    assert_eq!(
        spots::ring(1),
        vec![
            (0, -1),
            (-1, 0),
            (1, 0),
            (0, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (1, 1)
        ]
    );
    assert_eq!(spots::ring(2).len(), 16);
    assert_eq!(spots::ring(3).len(), 24);
    let c = spots::start_candidates(&[[0.0; 3], [10.0, 0.0, 5.0], [99.0; 3]]);
    assert_eq!(c.len(), 2 * (8 + 16 + 24), "two starts only");
    assert_eq!(
        (c[0].start, c[0].ring, c[0].at),
        (0, 1, [0.0, -spots::SPACING, 0.0])
    );
    assert_eq!(
        (c[1].start, c[1].ring, c[1].at),
        (1, 1, [10.0, -spots::SPACING, 5.0])
    );
    assert!(
        c[..16].iter().all(|c| c.ring == 1) && c[16].ring == 2,
        "ring by ring"
    );
    let one = spots::start_candidates(&[[0.0; 3]]);
    assert_eq!(one.len(), 48, "00a: one start");
    // Fourteen extras fit with room to spare, none on another.
    for (i, a) in c.iter().enumerate() {
        for b in &c[i + 1..] {
            assert!(distance(a.at, b.at) >= spots::MIN_GAP, "{a:?} {b:?}");
        }
    }
}

#[test]
fn players_objects_and_the_campaign_are_read_from_the_tables_with_riders_by_their_vehicle() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([1.0, 2.0, 3.0]), NONE);
    add_player(&mut g, 1, 1, Some([0.0; 3]), 0x00ab_0030);
    put_object(&mut g, 0x00ab_0030, [7.0, 8.0, 9.0], NONE);
    let ps = players(&g);
    assert_eq!(world_position(&g, unit_of(0)), Some([1.0, 2.0, 3.0]));
    assert_eq!(world_position(&g, unit_of(1)), Some([7.0, 8.0, 9.0]));
    assert!(!has_extras(&ps));
    add_player(&mut g, 5, 1, None, NONE);
    assert!(has_extras(&players(&g)));
}

/// Enters `pick` for player `i` (ECX), the chooser's arguments on the stack.
fn entered_pick(g: &mut FakeGuest, i: u16) -> Regs {
    g.put_u32(STACK, 0xdead_beef)
        .put_u32(STACK + 4, UNIT_TYPE)
        .put_u32(STACK + 8, 1)
        .put_u32(STACK + 12, 0);
    Regs {
        esp: STACK,
        ecx: handle_of(i),
        ..Regs::default()
    }
}

const UNIT_TYPE: u32 = 0x0e00_1234;

/// Two starts (origin and 10 units along x), players 0 and 1 on them,
/// player 2 already on the first grid spot of start 0, player 3 to spawn.
fn level_start() -> FakeGuest {
    let mut g = world(2);
    put_start(&mut g, 0, [0.0; 3]);
    put_start(&mut g, 1, [10.0, 0.0, 0.0]);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([10.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, Some([0.0, -spots::SPACING, 0.0]), NONE);
    add_player(&mut g, 3, 1 | FIRST_SPAWN, None, NONE);
    g
}

/// The free-spot test as a fake: blocked below the ground (z < 0), and at
/// any of `walls`.
fn free(c: &GuestCall, m: &dyn GuestMemory, walls: &[[f32; 3]]) -> u32 {
    let at = m.vec3(c.ecx.unwrap()).unwrap();
    u32::from(at[2] >= 0.0 && !walls.iter().any(|w| distance(*w, at) < 0.01))
}

#[test]
fn a_free_retail_start_is_kept() {
    let mut g = level_start();
    let regs = entered_pick(&mut g, 3);
    let st: Arc<Mutex<State>> = Arc::default();
    let s2 = Arc::clone(&st);
    let f = move |cx: Cx| -> EntryFuture { Box::pin(pick(cx, Arc::clone(&s2), Arc::default())) };
    let r = run(&f, g, regs, |c, _| {
        assert_eq!(c.target, points::PLAYER_SPAWN_LOCATION.base);
        assert_eq!(
            (c.ecx, c.args.clone()),
            (Some(handle_of(3)), vec![UNIT_TYPE, 1, 0])
        );
        Regs {
            eax: 1,
            ..Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(1, 12)));
    assert_eq!(r.calls.len(), 1);
    assert_eq!(lock(&st).moved_start, None);
}

#[test]
fn a_taken_start_spawns_on_the_first_free_grid_spot_never_on_a_player() {
    let mut g = level_start();
    let regs = entered_pick(&mut g, 3);
    let st: Arc<Mutex<State>> = Arc::default();
    let s2 = Arc::clone(&st);
    let f = move |cx: Cx| -> EntryFuture { Box::pin(pick(cx, Arc::clone(&s2), Arc::default())) };
    // A wall on start 1's first spot.
    let walls = [[10.0, -spots::SPACING, 0.0]];
    let r = run(&f, g, regs, move |c, m| {
        if c.target == points::PLAYER_SPAWN_LOCATION.base {
            return Regs {
                eax: u32::from(NO_LOCATION),
                ..Regs::default()
            };
        }
        assert_eq!(c.target, points::SPAWN_LOCATION_FREE.base);
        assert_eq!((c.eax, c.edi), (Some(UNIT_TYPE), Some(handle_of(3))));
        Regs {
            eax: free(c, m, &walls),
            ..Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(0, 12)), "start 0's entry");
    // Start 0's first spot (player 2 stands there) is never asked; start
    // 1's first has ground but a wall; start 0's second passes.
    let free_calls: Vec<u32> = r
        .calls
        .iter()
        .filter(|c| c.target == points::SPAWN_LOCATION_FREE.base)
        .map(|c| c.ecx.unwrap())
        .collect();
    assert_eq!(
        free_calls,
        vec![
            START_BASE + START_STRIDE,
            START_BASE + START_STRIDE,
            START_BASE,
            START_BASE
        ],
        "ground then spot, start 1 then start 0"
    );
    assert_eq!(
        start_at(&r.guest, 0),
        [-spots::SPACING, 0.0, 0.0],
        "spawns there"
    );
    assert_eq!(start_at(&r.guest, 1), [10.0, 0.0, 0.0], "start 1 put back");
    assert_eq!(lock(&st).moved_start, Some((START_BASE, [0.0; 3])));
}

#[test]
fn no_free_spot_on_any_ring_is_a_named_refusal_and_the_spawn_waits() {
    let mut g = level_start();
    let regs = entered_pick(&mut g, 3);
    let st: Arc<Mutex<State>> = Arc::default();
    let s2 = Arc::clone(&st);
    let f = move |cx: Cx| -> EntryFuture { Box::pin(pick(cx, Arc::clone(&s2), Arc::default())) };
    let r = run(&f, g, regs, |c, _| {
        if c.target == points::PLAYER_SPAWN_LOCATION.base {
            return Regs {
                eax: 0x7700_ffff,
                ..Regs::default()
            };
        }
        Regs::default()
    });
    assert_eq!(r.result, Ok(Ret::new(0x7700_ffff, 12)), "retail's 0xffff");
    // Every clear spot (95: one has player 2 on it) is asked three times:
    // the ground pass asks below it (not free, so ground), then at it, and
    // the second pass asks at it again.
    let tests = r
        .calls
        .iter()
        .filter(|c| c.target == points::SPAWN_LOCATION_FREE.base)
        .count();
    assert_eq!(tests, 3 * 95);
    assert_eq!(start_at(&r.guest, 0), [0.0; 3]);
    assert_eq!(start_at(&r.guest, 1), [10.0, 0.0, 0.0]);
    let s = lock(&st);
    assert_eq!(s.moved_start, None);
    assert_ne!(s.refused & (1 << 3), 0, "logged once");
}

const TIME: u32 = 0x00a0_0000;

/// A host that started alone, 20 units from the starts at tick `tick`,
/// and player 1 (a friend who joined in progress) to spawn.
fn joined_at(tick: u32) -> FakeGuest {
    let mut g = world(2);
    put_start(&mut g, 0, [0.0; 3]);
    put_start(&mut g, 1, [10.0, 0.0, 0.0]);
    add_player(&mut g, 0, 1, Some([20.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 1, 1 | FIRST_SPAWN, None, NONE);
    g.put_u32(points::GAME_TIME.base, TIME)
        .put_u32(TIME + 8, tick);
    g
}

#[test]
fn a_first_spawn_once_the_game_is_under_way_joins_in_progress() {
    let g = joined_at(JOIN_SPAWN_TICKS);
    let ps = players(&g);
    assert_eq!(game_tick(&g), Some(JOIN_SPAWN_TICKS));
    assert!(joins_in_progress(&g, &ps, &ps[1]), "player 1, any index");
    assert!(!takes_spawn(&ps[1]), "not one of the level start's extras");
    assert!(!joins_in_progress(&g, &ps, &ps[0]), "no first spawn pending");
    // The level's start: everyone spawns on its first ticks.
    let g = joined_at(JOIN_SPAWN_TICKS - 1);
    assert!(!joins_in_progress(&g, &players(&g), &players(&g)[1]));
    // Nobody alive to stand by: retail's pick.
    let mut g = joined_at(JOIN_SPAWN_TICKS);
    add_player(&mut g, 0, 1, None, NONE);
    let ps = players(&g);
    assert!(!joins_in_progress(&g, &ps, &ps[1]));
    // No game time read: never.
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([20.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 1, 1 | FIRST_SPAWN, None, NONE);
    let ps = players(&g);
    assert!(!joins_in_progress(&g, &ps, &ps[1]));
}

#[test]
fn a_join_in_progress_spawns_next_to_the_squad_not_at_the_level_start() {
    let mut g = joined_at(600);
    let regs = entered_pick(&mut g, 1);
    let st: Arc<Mutex<State>> = Arc::default();
    let s2 = Arc::clone(&st);
    let f = move |cx: Cx| -> EntryFuture { Box::pin(pick(cx, Arc::clone(&s2), Arc::default())) };
    let r = run(&f, g, regs, |c, m| {
        if c.target == points::PLAYER_SPAWN_LOCATION.base {
            // Retail finds the level's start free.
            return Regs {
                eax: 0x5500_0001,
                ..Regs::default()
            };
        }
        assert_eq!(c.target, points::SPAWN_LOCATION_FREE.base);
        assert_eq!(c.ecx, Some(START_BASE), "starting location 0 moved");
        Regs {
            eax: free(c, m, &[]),
            ..Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(0x5500_0000, 12)), "location 0");
    // Ring 1's first spot by player 0, the ground under it first.
    let spot = [20.0, -spots::SPACING, 0.0];
    assert_eq!(start_at(&r.guest, 0), spot, "spawns there");
    assert_eq!(start_at(&r.guest, 1), [10.0, 0.0, 0.0]);
    assert_eq!(
        r.calls
            .iter()
            .filter(|c| c.target == points::SPAWN_LOCATION_FREE.base)
            .count(),
        2
    );
    assert_eq!(lock(&st).moved_start, Some((START_BASE, [0.0; 3])));
}

#[test]
fn a_join_with_no_spot_by_the_squad_takes_retails_pick_and_says_so_once() {
    let mut g = joined_at(600);
    let regs = entered_pick(&mut g, 1);
    let st: Arc<Mutex<State>> = Arc::default();
    let s2 = Arc::clone(&st);
    let f = move |cx: Cx| -> EntryFuture { Box::pin(pick(cx, Arc::clone(&s2), Arc::default())) };
    let r = run(&f, g, regs, |c, _| {
        let eax = if c.target == points::PLAYER_SPAWN_LOCATION.base {
            1
        } else {
            0
        };
        Regs {
            eax,
            ..Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(1, 12)), "retail's free start");
    assert_eq!(start_at(&r.guest, 0), [0.0; 3], "start 0 put back");
    let s = lock(&st);
    assert_eq!(s.moved_start, None);
    assert_ne!(s.join_refused & (1 << 1), 0, "logged once");
}

#[test]
fn with_no_ground_found_a_spot_with_none_is_taken() {
    let mut g = level_start();
    let regs = entered_pick(&mut g, 3);
    let f = |cx: Cx| -> EntryFuture { Box::pin(pick(cx, Arc::default(), Arc::default())) };
    // The probe never hits anything.
    let r = run(&f, g, regs, |c, _| {
        let eax = if c.target == points::PLAYER_SPAWN_LOCATION.base {
            0xffff
        } else {
            1
        };
        Regs {
            eax,
            ..Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(1, 12)), "start 1's first spot");
    assert_eq!(start_at(&r.guest, 1), [10.0, -spots::SPACING, 0.0]);
}

#[test]
fn the_spawn_puts_back_the_start_it_spawned_on() {
    let mut g = level_start();
    let regs = entered_with(&mut g, handle_of(3));
    let state: Arc<Mutex<State>> = Arc::default();
    let reentry = Arc::new(AtomicBool::new(false));
    let (st, re) = (Arc::clone(&state), Arc::clone(&reentry));
    let f = move |cx: Cx| -> EntryFuture { Box::pin(spawn(cx, Arc::clone(&st), Arc::clone(&re))) };
    let (st, re) = (Arc::clone(&state), Arc::clone(&reentry));
    let r = run(&f, g, regs, move |c, m| {
        assert_eq!(c.target, points::PLAYER_SPAWN.base);
        assert_eq!(lock(&st).spawning, Some(handle_of(3)));
        assert!(re.load(Ordering::SeqCst));
        // As `pick` would: start 0 moved to a grid spot.
        for (k, v) in [-0.6f32, 0.0, 0.0].iter().enumerate() {
            m.set_f32(START_BASE + 4 * k as u32, *v).unwrap();
        }
        lock(&st).moved_start = Some((START_BASE, [0.0; 3]));
        Regs {
            eax: 1,
            ..Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(1, 4)));
    assert_eq!(
        r.calls.len(),
        1,
        "no placement after: it spawned on its spot"
    );
    assert_eq!(start_at(&r.guest, 0), [0.0; 3], "put back");
    let s = lock(&state);
    assert_eq!((s.spawning, s.moved_start), (None, None));
    assert!(!reentry.load(Ordering::SeqCst));
}

#[test]
fn a_spawn_that_failed_is_not_placed() {
    let mut g = world(0);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 2, 1 | FIRST_SPAWN, None, NONE);
    let regs = entered_with(&mut g, handle_of(2));
    let f = |cx: Cx| -> EntryFuture { Box::pin(spawn(cx, Arc::default(), Arc::default())) };
    let r = run(&f, g, regs, |_, _| Regs::default());
    assert_eq!(r.result, Ok(Ret::new(0, 4)));
    assert_eq!(r.calls.len(), 1, "only the original");
}

/// A `PLAYER_PLACE_NEAR` that places: the unit moves to the point.
fn placing(c: &GuestCall, m: &mut dyn GuestMemory) -> Regs {
    let at = m.vec3(c.esi.unwrap()).unwrap();
    let unit = unit_of(index_of(c.ebx.unwrap()));
    let obj = OBJECT_BASE + (unit & 0xffff) * 0x100;
    for (k, v) in at.iter().enumerate() {
        m.set_f32(obj + OBJECT_POSITION + 4 * k as u32, *v).unwrap();
    }
    Regs {
        eax: 1,
        ..Regs::default()
    }
}

#[test]
fn stragglers_are_brought_over_to_their_own_spots_before_the_player_update() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 1, 1, Some([1.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, Some([90.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 3, 1, Some([2.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 4, 1, Some([0.0, 70.0, 0.0]), NONE);
    let regs = entered_with(&mut g, 0x0abc_0000);
    let f = |cx: Cx| -> EntryFuture { Box::pin(gather(cx, Arc::default(), SCRATCH, true)) };
    let r = run(&f, g, regs, |c, m| {
        if c.target == points::PLAYER_UPDATE.base {
            assert_eq!(c.args, vec![0x0abc_0000]);
            return Regs {
                eax: 1,
                ..Regs::default()
            };
        }
        assert_eq!(c.args, vec![unit_of(0)]);
        placing(c, m)
    });
    assert_eq!(r.result, Ok(Ret::new(1, 4)));
    let moved: Vec<_> = r
        .calls
        .iter()
        .filter(|c| c.target == points::PLAYER_PLACE_NEAR.base)
        .map(|c| c.ebx)
        .collect();
    assert_eq!(moved, vec![Some(handle_of(2)), Some(handle_of(4))]);
    assert_eq!(r.calls.last().unwrap().target, points::PLAYER_UPDATE.base);
    let units: Vec<[f32; 3]> = (0..5)
        .map(|i| world_position(&r.guest, unit_of(i)).unwrap())
        .collect();
    assert_eq!(units[2], [0.0, -spots::SPACING, 0.0]);
    assert_eq!(
        units[4],
        [-spots::SPACING, 0.0, 0.0],
        "its own spot, not player 2's"
    );
    for (i, a) in units.iter().enumerate() {
        for b in &units[i + 1..] {
            assert!(distance(*a, *b) >= spots::MIN_GAP, "{units:?}");
        }
    }
}

#[test]
fn a_refused_spot_falls_to_the_next() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 2, 1, Some([90.0, 0.0, 0.0]), NONE);
    let regs = entered_with(&mut g, 1);
    let f = |cx: Cx| -> EntryFuture { Box::pin(gather(cx, Arc::default(), SCRATCH, true)) };
    let mut n = 0;
    let r = run(&f, g, regs, move |c, m| {
        if c.target == points::PLAYER_PLACE_NEAR.base {
            n += 1;
            if n <= 3 {
                return Regs::default();
            }
            return placing(c, m);
        }
        Regs::default()
    });
    let tries = r
        .calls
        .iter()
        .filter(|c| c.target == points::PLAYER_PLACE_NEAR.base)
        .count();
    assert_eq!(tries, 4);
    assert_eq!(
        world_position(&r.guest, unit_of(2)),
        Some([0.0, spots::SPACING, 0.0]),
        "ring 1's fourth"
    );
}

#[test]
fn with_two_players_nothing_moves() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([500.0, 0.0, 0.0]), NONE);
    let regs = entered_with(&mut g, 7);
    let f = |cx: Cx| -> EntryFuture { Box::pin(gather(cx, Arc::default(), SCRATCH, true)) };
    let r = run(&f, g, regs, |_, _| Regs::default());
    assert_eq!(
        r.calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        vec![points::PLAYER_UPDATE.base]
    );
}

// ---- volumes ----

const VOLUMES: u32 = 0x00a0_0000;
const KILLS: u32 = 0x00a1_0000;
const SWITCHES: u32 = 0x00a2_0000;

/// An axis-aligned volume `i`: corner `o`, size `z`, parented or not.
fn put_volume(g: &mut FakeGuest, i: u32, o: [f32; 3], z: [f32; 3], parent: i16) {
    let at = VOLUMES + i * volumes::VOLUME_STRIDE;
    g.put_u16(at + volumes::VOLUME_PARENT, parent as u16);
    for (field, v) in [
        (volumes::VOLUME_FORWARD, [1.0, 0.0, 0.0]),
        (volumes::VOLUME_UP, [0.0, 0.0, 1.0]),
        (volumes::VOLUME_ORIGIN, o),
        (volumes::VOLUME_SIZE, z),
    ] {
        for (k, f) in v.iter().enumerate() {
            g.put_f32(at + field + 4 * k as u32, *f);
        }
    }
}

/// Volumes: 0 a room at x 15..25, 1 a kill trigger, 2 parented, 3 a BSP
/// switch, 4 a room at x -5..5 (the anchor's).
fn with_volumes(g: &mut FakeGuest) {
    g.put_u32(SCENARIO + volumes::TRIGGER_VOLUMES, 5)
        .put_u32(SCENARIO + volumes::TRIGGER_VOLUMES + 4, VOLUMES)
        .put_u32(SCENARIO + volumes::KILL_TRIGGERS, 1)
        .put_u32(SCENARIO + volumes::KILL_TRIGGERS + 4, KILLS)
        .put_u16(KILLS, 1)
        .put_u32(SCENARIO + volumes::BSP_SWITCHES, 1)
        .put_u32(SCENARIO + volumes::BSP_SWITCHES + 4, SWITCHES)
        .put_u16(SWITCHES, 3);
    let room = [10.0, 10.0, 10.0];
    put_volume(g, 0, [15.0, -5.0, -5.0], room, -1);
    put_volume(g, 1, [15.0, -5.0, -5.0], room, -1);
    put_volume(g, 2, [15.0, -5.0, -5.0], room, 4);
    put_volume(g, 3, [15.0, -5.0, -5.0], room, -1);
    put_volume(g, 4, [-5.0, -5.0, -5.0], room, -1);
}

#[test]
fn a_box_holds_what_is_inside_it_rotated_or_not() {
    let v = volumes::Volume::new(
        0,
        [0.0; 3],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [2.0, 3.0, 4.0],
    )
    .unwrap();
    assert!(v.contains([1.0, 2.0, 3.0]));
    assert!(!v.contains([1.0, -0.1, 3.0]), "left is +y");
    assert!(!v.contains([2.1, 1.0, 1.0]));
    // Turned a quarter: forward +y, so left is -x.
    let v = volumes::Volume::new(
        0,
        [0.0; 3],
        [0.0, 2.0, 0.0],
        [0.0, 0.0, 1.0],
        [2.0, 3.0, 4.0],
    )
    .unwrap();
    assert!(v.contains([-2.0, 1.0, 1.0]));
    assert!(!v.contains([2.0, 1.0, 1.0]));
    assert_eq!(
        volumes::Volume::new(0, [0.0; 3], [0.0; 3], [0.0, 0.0, 1.0], [1.0; 3]),
        None
    );
}

#[test]
fn the_gates_are_the_unparented_volumes_that_neither_kill_nor_switch() {
    let mut g = world(2);
    with_volumes(&mut g);
    let idx: Vec<u16> = volumes::gates(&g).iter().map(|v| v.index).collect();
    assert_eq!(idx, vec![0, 4]);
    assert!(volumes::gates(&world(2)).is_empty(), "no block");
}

#[test]
fn a_gate_is_entered_once_and_not_on_the_first_update_seen() {
    assert!(!volumes::entered(None, &[0]), "a console that just joined");
    assert!(volumes::entered(Some(&[]), &[0]));
    assert!(volumes::entered(Some(&[4]), &[0, 4]));
    assert!(
        !volumes::entered(Some(&[0, 4]), &[0]),
        "leaving is no event"
    );
}

#[test]
fn a_far_player_in_a_gate_the_scripts_players_are_not_in_is_ahead() {
    let mut g = world(2);
    with_volumes(&mut g);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([1.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, Some([20.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 3, 1, Some([-30.0, 0.0, 0.0]), NONE);
    let vols = volumes::gates(&g);
    let ps = players(&g);
    assert_eq!(volumes::named_inside(&g, &ps, &vols), vec![4]);
    assert_eq!(
        volumes::ahead(&g, &ps, &vols, [0.0; 3]),
        vec![handle_of(2)],
        "3 is far but in no gate"
    );
    // Player 1 in the gate too: 2 is not ahead of them.
    add_player(&mut g, 1, 1, Some([16.0, 0.0, 0.0]), NONE);
    let ps = players(&g);
    assert!(volumes::ahead(&g, &ps, &vols, [0.0; 3]).is_empty());
}

#[test]
fn a_gather_without_stragglers_due_brings_back_only_the_runners_ahead() {
    let mut g = world(2);
    with_volumes(&mut g);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([1.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, Some([20.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 3, 1, Some([-30.0, 0.0, 0.0]), NONE);
    let regs = entered_with(&mut g, 1);
    let f = |cx: Cx| -> EntryFuture { Box::pin(gather(cx, Arc::default(), SCRATCH, false)) };
    let r = run(&f, g, regs, |c, m| {
        if c.target == points::PLAYER_PLACE_NEAR.base {
            return placing(c, m);
        }
        Regs::default()
    });
    let moved: Vec<_> = r
        .calls
        .iter()
        .filter(|c| c.target == points::PLAYER_PLACE_NEAR.base)
        .map(|c| c.ebx)
        .collect();
    assert_eq!(
        moved,
        vec![Some(handle_of(2))],
        "3 stays: no gather was due"
    );
}

// ---- respawn ----

#[test]
fn the_group_anchor_is_the_player_on_foot_nearest_the_middle() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([100.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, Some([1.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 3, 1, Some([2.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 4, 1, Some([30.0, 0.0, 0.0]), 0x00ab_0030);
    put_object(&mut g, 0x00ab_0030, [30.0, 0.0, 0.0], NONE);
    let ps = players(&g);
    // Middle (0+100+1+2+30)/5 = 26.6: player 4 is nearest but rides.
    assert_eq!(
        spots::group_anchor(&g, &ps, NONE),
        Some((unit_of(3), [2.0, 0.0, 0.0]))
    );
    assert_eq!(
        spots::group_anchor(&g, &ps, handle_of(3)).map(|a| a.0),
        Some(unit_of(2))
    );
    // A tie goes to the lower index.
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([-1.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 1, 1, Some([1.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, None, NONE);
    assert_eq!(
        spots::group_anchor(&g, &players(&g), NONE).map(|a| a.0),
        Some(unit_of(0))
    );
}

fn entered_place(g: &mut FakeGuest, player: u16, retail_at: u32, retail_anchor: u32) -> Regs {
    g.put_u32(STACK, RESPAWN_PLACE_RETURN)
        .put_u32(STACK + 4, retail_anchor);
    Regs {
        esp: STACK,
        ebx: handle_of(player),
        esi: retail_at,
        ..Regs::default()
    }
}

#[test]
fn a_respawn_lands_by_the_squads_middle_on_a_spot_of_its_own() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([100.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, Some([2.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 3, 1, Some([3.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 15, 1, Some([500.0, 0.0, 0.0]), NONE);
    // Player 4 just spawned, at the retail spot.
    add_player(&mut g, 4, 1, Some([0.0; 3]), NONE);
    let regs = entered_place(&mut g, 4, 0x1234, unit_of(15));
    let f = |cx: Cx| -> EntryFuture { Box::pin(respawn_place(cx, Arc::default(), SCRATCH + 0x10)) };
    let r = run(&f, g, regs, |c, m| {
        assert_eq!(c.target, points::PLAYER_PLACE_NEAR.base);
        placing(c, m)
    });
    assert_eq!(r.result, Ok(Ret::new(1, 4)));
    let c = &r.calls[0];
    // Middle (0+100+2+3+500)/5 = 121: player 1 is nearest, not player 15.
    assert_eq!(
        (c.ebx, c.esi, c.args.clone()),
        (Some(handle_of(4)), Some(SCRATCH + 0x10), vec![unit_of(1)])
    );
    assert_eq!(
        world_position(&r.guest, unit_of(4)),
        Some([100.0, -spots::SPACING, 0.0])
    );
}

#[test]
fn a_respawn_with_every_spot_refused_is_retails() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 2, 1, Some([0.0; 3]), NONE);
    let regs = entered_place(&mut g, 2, 0x1234, unit_of(0));
    let f = |cx: Cx| -> EntryFuture { Box::pin(respawn_place(cx, Arc::default(), SCRATCH + 0x10)) };
    let r = run(&f, g, regs, |c, _| {
        let retail = c.esi == Some(0x1234);
        Regs {
            eax: u32::from(retail),
            ..Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(1, 4)));
    let last = r.calls.last().unwrap();
    assert_eq!(
        (last.esi, last.args.clone()),
        (Some(0x1234), vec![unit_of(0)]),
        "retail's call"
    );
    assert_eq!(r.calls.len(), 48 + 1, "every spot, then retail");
}

#[test]
fn the_feature_is_on_the_plan_with_the_co_op_hooks() {
    let rt = runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::dispatch::Dispatch::default(),
    );
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let arm = crate::coop_probe::install_live(&rt, &mut r, std::path::Path::new("/nonexistent"))
        .unwrap()
        .unwrap();
    assert!(!arm.active(), "off until a co-op session arms it");
    let text = r.build(&hooks::Base).unwrap().text();
    assert!(text.contains(FEATURE), "{text}");
    for p in HOOKED {
        assert!(text.contains(p.name), "{} missing:\n{text}", p.name);
    }
    // H2_FEATURES=-coop-players leaves the rest of co-op on.
    let mut r = hooks::Registry::new(hooks::Switches::parse("-coop-players").unwrap());
    assert!(
        crate::coop_probe::install_live(&rt, &mut r, std::path::Path::new("/x"))
            .unwrap()
            .is_some()
    );
    let text = r.build(&hooks::Base).unwrap().text();
    assert!(!text.contains("players 3+: follow player 0"), "{text}");
}

// ---- checkpoints ----

const PG: u32 = 0x0080_0000;

/// A campaign with players 0 and 1 on foot at the origin, "some player
/// dead" at `some_dead`.
fn squad(some_dead: u8) -> FakeGuest {
    let mut g = world(2);
    g.put_u32(save::PLAYERS_GLOBALS, PG)
        .put_u8(PG + save::SOME_DEAD, some_dead);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([1.0, 0.0, 0.0]), NONE);
    g
}

fn in_use(m: &dyn GuestMemory, i: u32) -> bool {
    m.u32(PLAYERS_BITS).unwrap() & (1 << i) != 0
}

/// One retail check as the fake saw it: the players 2+ in use, the dead
/// flag.
type Seen = (Vec<u32>, u8);

/// Runs the checkpoint entry for `sq`; the fake check answers `answer` of
/// what it sees.
fn check(
    g: FakeGuest,
    sq: save::Squad,
    answer: impl Fn(&[u32]) -> bool + Send + 'static,
) -> (guest_layout::entry::testing::Run, Vec<Seen>) {
    let regs = Regs {
        esp: STACK,
        ..Regs::default()
    };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = Arc::clone(&seen);
    let f = move |cx: Cx| -> EntryFuture {
        Box::pin(save::safe_to_save(
            cx,
            sq.clone(),
            Arc::default(),
            Arc::default(),
        ))
    };
    let r = run(&f, g, regs, move |c, m| {
        assert_eq!(c.target, points::SAVE_SAFE.base);
        let extras: Vec<u32> = (2..16).filter(|&i| in_use(m, i)).collect();
        let safe = answer(&extras);
        s.lock()
            .unwrap()
            .push((extras, m.u8(PG + save::SOME_DEAD).unwrap()));
        Regs {
            eax: u32::from(safe),
            ..Regs::default()
        }
    });
    let seen = seen.lock().unwrap().clone();
    (r, seen)
}

fn sq(left_out: &[u16], near: &[u16]) -> save::Squad {
    save::Squad {
        left_out: left_out.to_vec(),
        near: near.to_vec(),
    }
}

#[test]
fn two_player_checkpoints_are_retail() {
    let g = squad(0);
    assert_eq!(save::squad(&g, &players(&g)), None);
    let mut g = squad(1);
    add_player(&mut g, 1, 1, None, NONE);
    assert_eq!(save::squad(&g, &players(&g)), None, "player 1 dead: retail");
}

#[test]
fn the_squad_is_who_is_near_and_alive_riders_by_their_vehicle() {
    let mut g = squad(0);
    add_player(&mut g, 2, 1, None, NONE);
    add_player(&mut g, 3, 1, Some([0.0, LEASH - 1.0, 0.0]), NONE);
    add_player(&mut g, 4, 1, Some([0.0, LEASH + 5.0, 0.0]), NONE);
    // 5 rides a vehicle near; 6 one far.
    add_player(&mut g, 5, 1, Some([0.0; 3]), 0x00ab_0030);
    put_object(&mut g, 0x00ab_0030, [2.0, 0.0, 0.0], NONE);
    add_player(&mut g, 6, 1, Some([0.0; 3]), 0x00ab_0031);
    put_object(&mut g, 0x00ab_0031, [90.0, 0.0, 0.0], NONE);
    assert_eq!(save::squad(&g, &players(&g)), Some(sq(&[2, 4, 6], &[3, 5])));
    // The anchor rides: its vehicle's place counts.
    put_object(&mut g, unit_of(0), [0.0; 3], 0x00ab_0031);
    assert_eq!(save::squad(&g, &players(&g)), Some(sq(&[2, 3, 4, 5], &[6])));
}

#[test]
fn more_than_half_the_squad_near_must_be_safe() {
    assert!(
        save::majority(2, 1, 0),
        "one unsafe player 3 no longer holds the save"
    );
    assert!(save::majority(2, 2, 1));
    assert!(!save::majority(2, 2, 0), "two of four safe: not a majority");
    assert!(!save::majority(2, 14, 6), "8 of 16");
    assert!(save::majority(2, 14, 7), "9 of 16");
    assert!(save::majority(2, 0, 0));
}

#[test]
fn a_dead_third_player_no_longer_blocks_a_checkpoint() {
    let mut g = squad(1);
    add_player(&mut g, 2, 1, None, NONE);
    let s = save::squad(&g, &players(&g)).unwrap();
    assert_eq!(s, sq(&[2], &[]));
    let (r, seen) = check(g, s, |_| true);
    assert_eq!(
        seen,
        vec![(vec![], 0)],
        "hidden and not dead for the one check"
    );
    assert_eq!(r.result, Ok(Ret::new(1, 0)));
    assert!(in_use(&r.guest, 2), "back in use after");
    assert_eq!(
        r.guest.u8(PG + save::SOME_DEAD).unwrap(),
        1,
        "the flag is put back"
    );
}

#[test]
fn a_dead_first_or_second_player_still_blocks() {
    let mut g = squad(1);
    add_player(&mut g, 1, 1, None, NONE);
    add_player(&mut g, 2, 1, None, NONE);
    let s = save::squad(&g, &players(&g)).unwrap();
    let (r, seen) = check(g, s, |_| false);
    assert_eq!(seen, vec![(vec![], 1)], "player 1 is dead: the flag stays");
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
}

#[test]
fn each_near_player_votes_with_its_own_check() {
    let mut g = squad(0);
    add_player(&mut g, 2, 1, Some([0.0, 1.0, 0.0]), NONE);
    add_player(&mut g, 3, 1, Some([0.0, 2.0, 0.0]), NONE);
    add_player(&mut g, 4, 1, Some([0.0, 90.0, 0.0]), NONE);
    let s = save::squad(&g, &players(&g)).unwrap();
    assert_eq!(s, sq(&[4], &[2, 3]));
    // Player 2 is unsafe (in combat, say); 3 is safe.
    let (r, seen) = check(g, s, |extras| !extras.contains(&2));
    assert_eq!(
        seen.iter().map(|s| s.0.clone()).collect::<Vec<_>>(),
        vec![vec![], vec![2], vec![3]],
        "players 0 and 1 alone, then each near one with them; 4 never"
    );
    assert_eq!(r.result, Ok(Ret::new(1, 0)), "3 of 4 safe");
    assert!((2..5).all(|i| in_use(&r.guest, i)));
}

#[test]
fn an_unsafe_majority_holds_the_save_and_an_unsafe_core_holds_it_at_once() {
    let mut g = squad(0);
    for i in 2..6 {
        add_player(&mut g, i, 1, Some([0.0, 1.0, 0.0]), NONE);
    }
    let s = save::squad(&g, &players(&g)).unwrap();
    // 2 to 4 unsafe, 5 safe: 3 of 6.
    let (r, seen) = check(g, s.clone(), |e| e.is_empty() || e == [5]);
    assert_eq!(seen.len(), 5);
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    let mut g = squad(0);
    for i in 2..6 {
        add_player(&mut g, i, 1, Some([0.0, 1.0, 0.0]), NONE);
    }
    let (r, seen) = check(g, s, |_| false);
    assert_eq!(seen.len(), 1, "players 0 and 1 unsafe: no votes asked");
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
}

#[test]
fn a_third_player_waiting_for_its_first_spawn_is_not_dead() {
    assert_eq!(
        save::some_dead_counted(
            &[Player {
                index: 0,
                handle: 0,
                flags: 1 | FIRST_SPAWN,
                unit: NONE
            }],
            &[],
            1
        ),
        0
    );
    let ps = [Player {
        index: 0,
        handle: 0,
        flags: 1,
        unit: NONE,
    }];
    assert_eq!(save::some_dead_counted(&ps, &[], 1), 1);
    assert_eq!(save::some_dead_counted(&ps, &[], 0), 0, "never raised");
}

#[test]
fn the_checkpoint_lines_are_logged_once_per_change() {
    let last = Mutex::new(None);
    let line = save::note(&last, &[2]).unwrap();
    assert!(
        line.starts_with("coop-players: checkpoint checks leave out player(s) 2 "),
        "{line}"
    );
    assert_eq!(save::note(&last, &[2]), None);
    assert!(save::note(&last, &[2, 5]).unwrap().contains("2, 5"));
    assert_eq!(save::note(&last, &[]), None, "nobody left out: no line");
    let last = Mutex::new(None);
    let line = save::verdict_note(&last, &[2, 3], &[2], true).unwrap();
    assert_eq!(
        line,
        "coop-players: checkpoint: 1 of 2 near player(s) of index 2 or more safe (unsafe: 2); the save goes on (majority)"
    );
    assert_eq!(save::verdict_note(&last, &[2, 3], &[2], true), None);
}

/// docs/runtime/snapshots.md: a snapshot waits while co-op is armed or a
/// spawn of ours is running; disarmed and idle it doesn't.
#[test]
fn players_block_snapshots_while_armed_or_spawning() {
    let arm = Arm::session();
    let st = Mutex::new(State::default());
    assert_eq!(players_live(&arm, &st), None);
    arm.arm(None);
    assert_eq!(
        players_live(&arm, &st).as_deref(),
        Some("a co-op session has armed the co-op hooks")
    );
    arm.disarm();
    lock(&st).spawning = Some(0x0002_0002);
    assert_eq!(
        players_live(&arm, &st),
        Some(format!(
            "player {}'s spawn is running",
            index_of(0x0002_0002)
        ))
    );
    lock(&st).spawning = None;
    // Flags left from the session: not a blocker once disarmed.
    lock(&st).pending = true;
    lock(&st).cinematic = true;
    assert_eq!(players_live(&arm, &st), None);
}
