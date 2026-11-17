use super::*;
use engine::entry::Regs;
use engine::entry::testing::run;
use engine::testing::FakeGuest;

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
        .map(STACK - 0x100, 0x200)
        .map(SCRATCH, 0x10);
    g
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
fn a_blocked_pick_becomes_a_spot_only_for_players_three_and_up() {
    assert_eq!(location_fallback(2, NO_LOCATION, 2), Some(0));
    assert_eq!(location_fallback(3, NO_LOCATION, 2), Some(1));
    assert_eq!(location_fallback(15, NO_LOCATION, 2), Some(1));
    assert_eq!(
        location_fallback(5, NO_LOCATION, 1),
        Some(0),
        "00a: one spot"
    );
    assert_eq!(
        location_fallback(1, NO_LOCATION, 2),
        None,
        "player 2 waits as retail"
    );
    assert_eq!(location_fallback(0, NO_LOCATION, 2), None);
    assert_eq!(location_fallback(4, 1, 2), None, "retail found one");
    assert_eq!(
        location_fallback(4, NO_LOCATION, 0),
        None,
        "no spots at all"
    );
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
fn a_third_players_first_spawn_is_placed_next_to_player_0_in_the_same_call() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([10.0, 20.0, 30.0]), NONE);
    add_player(&mut g, 1, 1, Some([11.0, 20.0, 30.0]), NONE);
    add_player(&mut g, 2, 1 | FIRST_SPAWN, None, NONE);
    let regs = entered_with(&mut g, handle_of(2));
    let state: Arc<Mutex<State>> = Arc::default();
    let reentry = Arc::new(AtomicBool::new(false));
    let (st, re) = (Arc::clone(&state), Arc::clone(&reentry));
    let f = move |cx: Cx| -> EntryFuture {
        Box::pin(spawn(cx, Arc::clone(&st), Arc::clone(&re), SCRATCH))
    };
    let (st, re) = (Arc::clone(&state), Arc::clone(&reentry));
    let r = run(&f, g, regs, move |c, m| {
        if c.target == points::PLAYER_SPAWN.base {
            // The original runs with the fallback armed for this player and
            // its own hook out of the way.
            assert_eq!(lock(&st).spawning, Some(handle_of(2)));
            assert!(re.load(Ordering::SeqCst));
            assert_eq!(c.args, vec![handle_of(2)]);
            // It spawned: a unit on start spot 0 (where player 0 stands).
            let at = PLAYERS_DATA + 2 * PLAYER_SIZE;
            m.set_u32(at + PLAYER_UNIT, unit_of(2)).unwrap();
            m.set_u8(at + PLAYER_FLAGS, 1).unwrap();
            return Regs {
                eax: 1,
                ..Regs::default()
            };
        }
        assert_eq!(c.target, points::PLAYER_PLACE_NEAR.base);
        assert_eq!(c.ebx, Some(handle_of(2)));
        assert_eq!(c.esi, Some(SCRATCH));
        assert_eq!(c.args, vec![unit_of(0)]);
        assert_eq!(
            m.vec3(SCRATCH).unwrap(),
            [10.0, 20.0, 30.0],
            "player 0's position"
        );
        Regs {
            eax: 1,
            ..Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(1, 4)));
    assert_eq!(
        r.calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        vec![points::PLAYER_SPAWN.base, points::PLAYER_PLACE_NEAR.base]
    );
    assert_eq!(lock(&state).spawning, None, "the fallback is off again");
    assert!(!reentry.load(Ordering::SeqCst));
}

#[test]
fn a_spawn_that_failed_is_not_placed() {
    let mut g = world(0);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 2, 1 | FIRST_SPAWN, None, NONE);
    let regs = entered_with(&mut g, handle_of(2));
    let f =
        |cx: Cx| -> EntryFuture { Box::pin(spawn(cx, Arc::default(), Arc::default(), SCRATCH)) };
    let r = run(&f, g, regs, |_, _| Regs::default());
    assert_eq!(r.result, Ok(Ret::new(0, 4)));
    assert_eq!(r.calls.len(), 1, "only the original");
}

#[test]
fn stragglers_are_brought_over_before_the_player_update() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 1, 1, Some([1.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 2, 1, Some([90.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 3, 1, Some([2.0, 0.0, 0.0]), NONE);
    add_player(&mut g, 4, 1, Some([0.0, 70.0, 0.0]), NONE);
    let regs = entered_with(&mut g, 0x0abc_0000);
    let f = |cx: Cx| -> EntryFuture { Box::pin(gather(cx, Arc::default(), SCRATCH)) };
    let r = run(&f, g, regs, |c, _| {
        if c.target == points::PLAYER_UPDATE.base {
            assert_eq!(c.args, vec![0x0abc_0000]);
        } else {
            assert_eq!(c.args, vec![unit_of(0)]);
        }
        Regs {
            eax: 1,
            ..Regs::default()
        }
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
}

#[test]
fn with_two_players_nothing_moves() {
    let mut g = world(2);
    add_player(&mut g, 0, 1, Some([0.0; 3]), NONE);
    add_player(&mut g, 1, 1, Some([500.0, 0.0, 0.0]), NONE);
    let regs = entered_with(&mut g, 7);
    let f = |cx: Cx| -> EntryFuture { Box::pin(gather(cx, Arc::default(), SCRATCH)) };
    let r = run(&f, g, regs, |_, _| Regs::default());
    assert_eq!(
        r.calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        vec![points::PLAYER_UPDATE.base]
    );
}

#[test]
fn the_feature_is_on_the_plan_with_the_co_op_hooks() {
    let rt = runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::dispatch::Dispatch::default(),
    );
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let arm = crate::coop_probe::install_live(&rt, &mut r, std::path::Path::new("/nonexistent"))
        .unwrap()
        .unwrap();
    assert!(!arm.active(), "off until a co-op session arms it");
    let text = r.build(&feature::Base).unwrap().text();
    assert!(text.contains(FEATURE), "{text}");
    for p in HOOKED {
        assert!(text.contains(p.name), "{} missing:\n{text}", p.name);
    }
    // H2_FEATURES=-coop-players leaves the rest of co-op on.
    let mut r = feature::Registry::new(feature::Switches::parse("-coop-players").unwrap());
    assert!(
        crate::coop_probe::install_live(&rt, &mut r, std::path::Path::new("/x"))
            .unwrap()
            .is_some()
    );
    let text = r.build(&feature::Base).unwrap().text();
    assert!(!text.contains("players 3+: follow player 0"), "{text}");
}
