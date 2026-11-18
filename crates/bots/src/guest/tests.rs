use super::*;
use engine::datum::layout as dl;
use engine::entry::{EntryFuture, Regs, testing};
use engine::testing::FakeGuest;

const GAME: u32 = 0x0060_0000;
const PLAYERS_HEADER: u32 = 0x0061_0000;
const PLAYERS_DATA: u32 = 0x0061_1000;
const OBJECTS_HEADER: u32 = 0x0062_0000;
const OBJECTS_DATA: u32 = 0x0062_1000;
const BODIES: u32 = 0x0063_0000;
const LOCALS: u32 = 0x0064_0000;
const SCREEN: u32 = 0x0065_0000;
const EVENT: u32 = 0x0065_1000;
const BUFFER: u32 = 0x0066_0000;
const STACK: u32 = 0x0067_0800;

fn array(g: &mut FakeGuest, header: u32, data: u32, size: u32, count: u32) {
    g.map(header, 0x48)
        .put_u32(header + dl::ELEMENT_SIZE, size)
        .put_u32(header + dl::HIGH_WATER, count)
        .put_u32(header + dl::DATA, data)
        .map(data, size * count.max(1));
}

/// One player per entry: (controller, local user, team, position, aim,
/// dead).
type P = (i32, i16, i8, [f32; 3], [f32; 3], bool);

/// A multiplayer game with these players, each with a biped; local users
/// mapped in `locals` order (player indices).
fn game(ps: &[P], teams: bool, locals: &[usize]) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(GAME, 0x400)
        .put_u32(points::GAME_GLOBALS.base, GAME)
        .put_u32(GAME + OPTIONS, MODE_MULTIPLAYER)
        .put_u32(GAME + OPTIONS + VARIANT_FLAGS, teams as u32);
    g.put_u32(points::PLAYERS.base, PLAYERS_HEADER);
    array(&mut g, PLAYERS_HEADER, PLAYERS_DATA, 0x21c, ps.len() as u32);
    g.put_u32(points::OBJECTS.base, OBJECTS_HEADER);
    array(&mut g, OBJECTS_HEADER, OBJECTS_DATA, 0xc, ps.len() as u32);
    for (i, &(controller, user, team, pos, aim, dead)) in ps.iter().enumerate() {
        let i = i as u32;
        let salt = 0xe000 + i as u16;
        let p = PLAYERS_DATA + i * 0x21c;
        let unit = DatumHandle::new(0xb000 + i as u16, i);
        g.put_u16(p, salt)
            .put_u16(p + 0x28, user as u16)
            .put_u32(p + PLAYER_CONTROLLER, controller as u32)
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
    g.put_u32(points::PLAYER_GLOBALS.base, LOCALS)
        .map(LOCALS, 0x100);
    g.put_u16(LOCALS + LOCAL_COUNT, locals.len() as u16);
    for u in 0..4u32 {
        let v = match locals.get(u as usize) {
            Some(&i) => DatumHandle::new(0xe000 + i as u16, i as u32).raw(),
            None => u32::MAX,
        };
        g.put_u32(LOCALS + LOCAL_MAP + 4 * u, v);
    }
    g
}

const X: [f32; 3] = [1.0, 0.0, 0.0];

/// A human on port 0 (user 0) and two bots on ports 1 and 2 (users 1, 2).
fn three() -> FakeGuest {
    game(
        &[
            (0, 0, 0, [0.0; 3], X, false),
            (1, 1, 1, [5.0, 0.0, 0.0], X, false),
            (2, 2, 1, [9.0, 0.0, 0.0], X, false),
        ],
        false,
        &[0, 1, 2],
    )
}

fn locals(g: &FakeGuest) -> (u16, [u32; 4]) {
    let mut map = [0; 4];
    for (u, v) in (0u32..).zip(map.iter_mut()) {
        *v = g.u32(LOCALS + LOCAL_MAP + 4 * u).unwrap();
    }
    (g.u16(LOCALS + LOCAL_COUNT).unwrap(), map)
}

#[test]
fn the_bot_users_are_the_ones_on_bot_ports() {
    let g = three();
    assert_eq!(bot_users(&g, 0b110).unwrap(), vec![1, 2]);
    assert_eq!(bot_users(&g, 0b100).unwrap(), vec![2]);
    assert_eq!(bot_users(&g, 0).unwrap(), Vec::<u32>::new());
}

#[test]
fn hiding_leaves_the_human_alone_and_restoring_puts_everything_back() {
    let mut g = three();
    let before = locals(&g);
    let h = hide(&mut g, 0b110).unwrap().expect("bots to hide");
    let (count, map) = locals(&g);
    assert_eq!(count, 1);
    assert_eq!(map[0], before.1[0], "the human keeps user 0");
    assert_eq!(&map[1..], &[u32::MAX; 3]);
    restore(&mut g, &h).unwrap();
    assert_eq!(locals(&g), before);
}

#[test]
fn a_human_after_the_bots_keeps_their_entry() {
    // Bot on port 1 as user 0, the human on port 0 as user 1.
    let mut g = game(
        &[
            (1, 0, 0, [0.0; 3], X, false),
            (0, 1, 0, [1.0, 0.0, 0.0], X, false),
        ],
        false,
        &[0, 1],
    );
    let before = locals(&g);
    let h = hide(&mut g, 0b10).unwrap().unwrap();
    let (count, map) = locals(&g);
    assert_eq!((count, map[0], map[1]), (1, u32::MAX, before.1[1]));
    restore(&mut g, &h).unwrap();
    assert_eq!(locals(&g), before);
}

#[test]
fn nothing_is_hidden_without_bots_or_without_a_human() {
    let mut g = three();
    let before = locals(&g);
    assert_eq!(hide(&mut g, 0).unwrap(), None);
    assert_eq!(hide(&mut g, 0b1000).unwrap(), None, "no user on port 3");
    // Every user a bot: keep the views.
    assert_eq!(hide(&mut g, 0b111).unwrap(), None);
    assert_eq!(locals(&g), before);
}

#[test]
fn perception_finds_the_bot_and_the_living_others() {
    let mut ps = vec![
        (0, 0, 0, [0.0; 3], X, false),
        (1, 1, 1, [5.0, 1.0, 0.0], [0.0, 1.0, 0.0], false),
        (-1, -1, 2, [7.0, 0.0, 0.0], X, true),
    ];
    let g = game(&ps, true, &[0, 1]);
    let p = perceive(&g, 1).unwrap();
    assert!(p.teams);
    assert_eq!(
        p.me,
        Some(Me {
            position: [5.0, 1.0, 0.0],
            aim: [0.0, 1.0, 0.0],
            team: 1
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
    ps[1].5 = true;
    assert_eq!(perceive(&game(&ps, true, &[0, 1]), 1).unwrap().me, None);
}

#[test]
fn a_phantom_perceives_itself_by_machine_id() {
    let ps = vec![
        (0, 0, 0, [0.0; 3], X, false),
        (-1, -1, 1, [5.0, 1.0, 0.0], [0.0, 1.0, 0.0], false),
        (-1, -1, 2, [9.0, 0.0, 0.0], X, false),
    ];
    let mut g = game(&ps, true, &[0]);
    let mine = [0xc1, 0xa4, 0, 0, 0, 7];
    for (k, b) in (0u32..).zip(mine) {
        g.put_u8(PLAYERS_DATA + 0x21c + PLAYER_MACHINE + k, b);
        g.put_u8(
            PLAYERS_DATA + 2 * 0x21c + PLAYER_MACHINE + k,
            if k == 5 { 8 } else { b },
        );
    }
    let p = perceive_as(&g, Who::Machine(mine)).unwrap();
    assert_eq!(p.me.map(|m| m.position), Some([5.0, 1.0, 0.0]));
    let ids: Vec<u32> = p.others.iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![0, 2], "the human and the other phantom");
    // A remote player is never a port's bot, whatever its controller field.
    assert_eq!(perceive(&g, 0xffff_ffff).unwrap().me, None);
}

#[test]
fn the_tick_comes_from_the_game_time() {
    let mut g = three();
    g.map(0x0068_0000, 0x10)
        .put_u32(points::GAME_TIME.base, 0x0068_0000)
        .put_u32(0x0068_0000 + GAME_TIME_TICK, 1234);
    assert_eq!(perceive(&g, 1).unwrap().tick, 1234);
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
    assert_eq!(perceive(&g, 1).unwrap(), Perception::default());
    g.put_u32(points::GAME_GLOBALS.base, 0);
    assert_eq!(perceive(&g, 1).unwrap(), Perception::default());
}

#[test]
fn the_bot_block_is_the_humans_with_a_name_and_fixed_controls() {
    let mut human = [0x5au8; BLOCK_LEN];
    human[BLOCK_CONTROLS..BLOCK_CONTROLS + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    human[BLOCK_CONTROLS + 4] = 2; // Boxer
    human[BLOCK_CONTROLS + 5] = 1;
    human[BLOCK_CONTROLS + 6] = 9;
    let b = bot_block(&human, "Clanker 1 with a long name", 1);
    assert_eq!(&b[BLOCK_NAME..BLOCK_NAME + 16], b"Clanker 1 with \0");
    let flags = u32::from_le_bytes(b[BLOCK_CONTROLS..BLOCK_CONTROLS + 4].try_into().unwrap());
    assert_eq!(flags, 0xffff_fffa, "bits 0 and 2 off, the rest kept");
    assert_eq!(
        &b[BLOCK_CONTROLS + 4..BLOCK_CONTROLS + 7],
        &[0, 0, BOT_SENSITIVITY]
    );
    for (i, (x, y)) in human.iter().zip(&b).enumerate() {
        let ours = (BLOCK_NAME..BLOCK_NAME + 16).contains(&i)
            || (BLOCK_PROFILE_NAME..BLOCK_PROFILE_NAME + 2 * BLOCK_PROFILE_NAME_UNITS).contains(&i)
            || (BLOCK_CONTROLS..BLOCK_CONTROLS + 7).contains(&i)
            || (BLOCK_LOOK..BLOCK_LOOK + 8).contains(&i);
        if !ours {
            assert_eq!(x, y, "byte {i:#x} is the human's");
        }
    }
    // Short names are padded with NULs.
    assert_eq!(
        &bot_block(&human, "B", 1)[BLOCK_NAME..BLOCK_NAME + 3],
        b"B\0\0"
    );
}

fn utf16(units: &[u8]) -> String {
    let u: Vec<u16> = units
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&u)
}

/// The human's saved profile as its file starts: 8 bytes, then the name in
/// UTF-16 ("Halo0001", the user's), then zeros.
fn halo0001() -> [u8; BLOCK_LEN] {
    let mut human = [0u8; BLOCK_LEN];
    for (i, u) in "Halo0001".encode_utf16().enumerate() {
        human[BLOCK_PROFILE_NAME + 2 * i..][..2].copy_from_slice(&u.to_le_bytes());
    }
    human
}

/// The game's display-name refresh for a slot with a profile and no Live
/// account (`0x190eb3` at `0x190f71`): slot `+0x20` (the block's profile
/// name), 32 units, to `+0xc1c`.
fn refresh_name(m: &mut FakeGuest, port: u32) {
    let name = m
        .bytes(
            slot(port) + SLOT_BLOCK + BLOCK_PROFILE_NAME as u32,
            2 * SLOT_NAME_UNITS,
        )
        .unwrap();
    m.write(slot(port) + SLOT_NAME, &name).unwrap();
}

#[test]
fn the_bot_block_carries_the_bots_name_as_its_profile_name() {
    let human = halo0001();
    let b = bot_block(&human, "Clanker 2", 2);
    let name = &b[BLOCK_PROFILE_NAME..BLOCK_PROFILE_NAME + 2 * BLOCK_PROFILE_NAME_UNITS];
    assert_eq!(utf16(name), "Clanker 2", "not the human's Halo0001");
    assert!(name[2 * 9..].iter().all(|&b| b == 0), "NUL to the end");
    // At most 31 units and a NUL, whatever the name.
    let long = "x".repeat(40);
    let b = bot_block(&human, &long, 1);
    let name = &b[BLOCK_PROFILE_NAME..BLOCK_PROFILE_NAME + 2 * BLOCK_PROFILE_NAME_UNITS];
    assert_eq!(utf16(name).len(), BLOCK_PROFILE_NAME_UNITS - 1);
    assert_eq!(&name[2 * 31..], &[0, 0]);
    // Each bot is its own: no two share a name the session would rename.
    let names: Vec<String> = (1..=3)
        .map(|p| {
            let b = bot_block(&human, &format!("Clanker {p}"), p);
            utf16(&b[BLOCK_PROFILE_NAME..BLOCK_PROFILE_NAME + 2 * BLOCK_PROFILE_NAME_UNITS])
        })
        .collect();
    assert_eq!(names, ["Clanker 1", "Clanker 2", "Clanker 3"]);
}

#[test]
fn the_games_name_refresh_keeps_a_signed_in_bots_name() {
    // The user's Live lobby: the human "afr" and three bots shown as
    // "Halo0001", "Shadow" and "Wilshire". The game rebuilt each bot's
    // display name from its block's profile name, the human's, and the
    // session renamed the copies. Signed in now, a refresh says the bot's.
    let mut g = FakeGuest::new();
    slots(&mut g);
    g.put_u32(slot(0), SLOT_PROFILE);
    g.put(slot(0) + SLOT_BLOCK, &halo0001());
    let mut run = sign_in_run(g);
    assert_eq!(run.result, Ok(Ret::new(1, 4)));
    refresh_name(&mut run.guest, 1);
    assert_eq!(
        run.guest
            .utf16(slot(1) + SLOT_NAME, SLOT_NAME_UNITS)
            .unwrap(),
        "Clanker 1"
    );
}

/// What `0x153750` accepts: the block check zeroes a look it does not.
fn look_is_valid(l: &[u8]) -> bool {
    l[..4].iter().all(|&c| c == 0xff || c < COLOURS)
        && l[4] < 4
        && l[5] < EMBLEM_FOREGROUNDS
        && l[6] < EMBLEM_BACKGROUNDS
        && l[7] & 0xf0 == 0
}

#[test]
fn each_bot_has_its_own_colours_and_emblem_never_the_humans() {
    let mut human = [0u8; BLOCK_LEN];
    // The human: colours 3 and 1 (port 1's), model 1, emblem 9 on 4.
    human[BLOCK_LOOK..BLOCK_LOOK + 8].copy_from_slice(&[3, 1, 3, 1, 1, 9, 4, 0x0f]);
    human[BLOCK_LOOK + 8] = 0x77;
    let looks: Vec<[u8; 8]> = (1..=3)
        .map(|port| {
            let b = bot_block(&human, "B", port);
            assert_eq!(b[BLOCK_LOOK + 8], 0x77, "the unchecked half is the human's");
            b[BLOCK_LOOK..BLOCK_LOOK + 8].try_into().unwrap()
        })
        .collect();
    for (port, l) in (1..).zip(&looks) {
        assert!(look_is_valid(l), "port {port}: {l:?}");
        assert_eq!(l[4], 1, "the human's model");
        assert_ne!(l[0], 3, "port {port}: not the human's primary");
        assert_ne!(l[1], 1, "port {port}: not the human's secondary");
        assert_ne!((l[5], l[6]), (9, 4), "port {port}: not the human's emblem");
    }
    assert_eq!(&looks[0][..2], &[4, 2], "port 1's colours moved one on");
    assert_ne!(looks[0], looks[1]);
    assert_ne!(looks[1], looks[2]);
    assert_ne!(looks[0], looks[2]);
    // A bad model is not copied.
    human[BLOCK_LOOK + 4] = 9;
    let b = bot_block(&human, "B", 2);
    assert!(look_is_valid(&b[BLOCK_LOOK..BLOCK_LOOK + 8]));
    for (port, (_, fg, bg)) in (1..).zip(BOT_LOOKS) {
        assert!(
            fg < EMBLEM_FOREGROUNDS && bg < EMBLEM_BACKGROUNDS,
            "port {port}"
        );
    }
}

#[test]
fn signing_out_calls_the_games_sign_out_per_port_then_the_frame() {
    const THEN: u32 = 0x0012_5b40;
    let f = |cx: Cx| -> EntryFuture { Box::pin(sign_out_then(cx, vec![1, 3], THEN)) };
    let regs = Regs {
        esp: STACK,
        ..Regs::default()
    };
    let mut g = FakeGuest::new();
    g.map(STACK - 0x100, 0x200);
    let run = testing::run(&f, g, regs, |c, _| Regs {
        eax: if c.target == THEN { 0x77 } else { 0 },
        ..Regs::default()
    });
    assert_eq!(run.result, Ok(Ret::new(0x77, 0)), "the frame's own return");
    let calls: Vec<(u32, Option<u32>)> = run.calls.iter().map(|c| (c.target, c.eax)).collect();
    let out = points::PROFILE_SIGN_OUT.base;
    assert_eq!(calls, vec![(out, Some(1)), (out, Some(3)), (THEN, None)]);
}

fn slots(g: &mut FakeGuest) {
    g.map(SLOTS, 4 * SLOT_STRIDE);
}

#[test]
fn a_bots_a_on_the_players_screen_is_a_join_until_it_has_a_profile() {
    let mut g = FakeGuest::new();
    slots(&mut g);
    g.map(EVENT, 0x10)
        .put_u32(EVENT + EVENT_CONTROLLER, 1)
        .put_u32(EVENT + EVENT_BUTTON, BUTTON_A);
    assert_eq!(bot_join(&g, EVENT, 0b10).unwrap(), Some(1));
    assert_eq!(
        bot_join(&g, EVENT, 0b100).unwrap(),
        None,
        "not a bot's port"
    );
    g.put_u32(EVENT + EVENT_BUTTON, BUTTON_START);
    assert_eq!(bot_join(&g, EVENT, 0b10).unwrap(), Some(1));
    g.put_u32(EVENT + EVENT_BUTTON, 1); // B
    assert_eq!(bot_join(&g, EVENT, 0b10).unwrap(), None);
    g.put_u32(EVENT + EVENT_BUTTON, BUTTON_A)
        .put_u32(slot(1), SLOT_PROFILE);
    assert_eq!(
        bot_join(&g, EVENT, 0b10).unwrap(),
        None,
        "already signed in"
    );
}

fn sign_in_run(mut g: FakeGuest) -> testing::Run {
    g.map(STACK - 0x100, 0x200).put_u32(STACK + 4, SCREEN);
    g.map(SCREEN, 0x20).put_u16(SCREEN + SCREEN_MASK, 0b1);
    g.map(BUFFER, BLOCK_LEN as u32);
    let f = |cx: Cx| -> EntryFuture { Box::pin(sign_in(cx, 1, 0b10, "Clanker 1".into(), BUFFER)) };
    let regs = Regs {
        esp: STACK,
        ..Regs::default()
    };
    testing::run(&f, g, regs, |c, m| {
        // The game's sign-in: copy the block and set the flag.
        let (port, block) = (c.args[0], c.args[1]);
        let b = m.bytes(block, BLOCK_LEN).unwrap();
        m.write(slot(port) + SLOT_BLOCK, &b).unwrap();
        let f = m.u32(slot(port)).unwrap();
        m.set_u32(slot(port), f | SLOT_PROFILE).unwrap();
        Regs::default()
    })
}

#[test]
fn sign_in_calls_the_games_sign_in_with_the_bot_block_and_joins_the_screen() {
    let mut g = FakeGuest::new();
    slots(&mut g);
    g.put_u32(slot(0), SLOT_PROFILE);
    g.put(slot(0) + SLOT_BLOCK, &[0x11; BLOCK_LEN]);
    let run = sign_in_run(g);
    assert_eq!(run.result, Ok(Ret::new(1, 4)));
    assert_eq!(run.calls.len(), 1);
    let c = &run.calls[0];
    assert_eq!(c.target, points::PROFILE_SIGN_IN.base);
    assert_eq!(c.args, vec![1, BUFFER, u32::MAX], "no file: nothing saved");
    let g = run.guest;
    assert!(signed_in(&g, 1).unwrap());
    let block = g.bytes(slot(1) + SLOT_BLOCK, BLOCK_LEN).unwrap();
    assert_eq!(&block[BLOCK_NAME..BLOCK_NAME + 10], b"Clanker 1\0");
    assert_eq!(block[0], 0x11, "made from the human's");
    assert_eq!(
        g.utf16(slot(1) + SLOT_NAME, SLOT_NAME_UNITS).unwrap(),
        "Clanker 1"
    );
    assert_eq!(g.u16(SCREEN + SCREEN_MASK).unwrap(), 0b11);
}

fn sign_in_then_run(mut g: FakeGuest, screen: Option<u32>) -> testing::Run {
    const THEN: u32 = 0x0012_5b40;
    g.map(STACK - 0x100, 0x200);
    g.map(SCREEN, 0x20).put_u16(SCREEN + SCREEN_MASK, 0b1);
    g.map(BUFFER, BLOCK_LEN as u32);
    let f = move |cx: Cx| -> EntryFuture {
        Box::pin(sign_in_then(
            cx,
            vec![(1, "Clanker 1".into()), (3, "Clanker 3".into())],
            0b1010,
            BUFFER,
            screen,
            THEN,
        ))
    };
    let regs = Regs {
        esp: STACK,
        ..Regs::default()
    };
    testing::run(&f, g, regs, |c, m| {
        if c.target == THEN {
            return Regs {
                eax: 0x77,
                ..Regs::default()
            };
        }
        let (port, block) = (c.args[0], c.args[1]);
        let b = m.bytes(block, BLOCK_LEN).unwrap();
        m.write(slot(port) + SLOT_BLOCK, &b).unwrap();
        let f = m.u32(slot(port)).unwrap();
        m.set_u32(slot(port), f | SLOT_PROFILE).unwrap();
        Regs::default()
    })
}

#[test]
fn the_glue_signs_bots_in_with_no_press_then_runs_the_frame() {
    let mut g = FakeGuest::new();
    slots(&mut g);
    g.put_u32(slot(0), SLOT_PROFILE);
    g.put(slot(0) + SLOT_BLOCK, &halo0001());
    let run = sign_in_then_run(g, Some(SCREEN));
    assert_eq!(run.result, Ok(Ret::new(0x77, 0)), "the frame's own return");
    let calls: Vec<(u32, Vec<u32>)> = run
        .calls
        .iter()
        .map(|c| (c.target, c.args.clone()))
        .collect();
    let sign_in = points::PROFILE_SIGN_IN.base;
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0], (sign_in, vec![1, BUFFER, u32::MAX]));
    assert_eq!(calls[1], (sign_in, vec![3, BUFFER, u32::MAX]));
    assert_eq!(calls[2].0, 0x0012_5b40, "then the frame");
    let mut g = run.guest;
    for (port, name) in [(1, "Clanker 1"), (3, "Clanker 3")] {
        assert!(signed_in(&g, port).unwrap());
        assert_eq!(
            g.utf16(slot(port) + SLOT_NAME, SLOT_NAME_UNITS).unwrap(),
            name
        );
        refresh_name(&mut g, port);
        assert_eq!(
            g.utf16(slot(port) + SLOT_NAME, SLOT_NAME_UNITS).unwrap(),
            name
        );
    }
    assert!(!signed_in(&g, 2).unwrap());
    assert_eq!(
        g.u16(SCREEN + SCREEN_MASK).unwrap(),
        0b1011,
        "joined on the screen"
    );
}

#[test]
fn with_no_players_screen_the_bots_sign_in_and_no_mask_is_touched() {
    let mut g = FakeGuest::new();
    slots(&mut g);
    g.put_u32(slot(0), SLOT_PROFILE);
    let run = sign_in_then_run(g, None);
    assert_eq!(run.result, Ok(Ret::new(0x77, 0)));
    assert!(signed_in(&run.guest, 1).unwrap() && signed_in(&run.guest, 3).unwrap());
    assert_eq!(run.guest.u16(SCREEN + SCREEN_MASK).unwrap(), 0b1);
}

#[test]
fn with_no_human_profile_the_glue_signs_no_one_in_and_runs_the_frame() {
    let mut g = FakeGuest::new();
    slots(&mut g);
    let run = sign_in_then_run(g, Some(SCREEN));
    assert_eq!(run.result, Ok(Ret::new(0x77, 0)));
    assert_eq!(run.calls.len(), 1, "only the frame");
    assert!(!signed_in(&run.guest, 1).unwrap());
}

#[test]
fn with_no_human_profile_there_is_nothing_to_copy_and_no_sign_in() {
    let mut g = FakeGuest::new();
    slots(&mut g);
    let run = sign_in_run(g);
    assert_eq!(run.result, Ok(Ret::new(1, 4)));
    assert!(run.calls.is_empty());
    assert!(!signed_in(&run.guest, 1).unwrap());
    assert_eq!(run.guest.u16(SCREEN + SCREEN_MASK).unwrap(), 0b1);
}
