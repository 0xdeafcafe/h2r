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
fn the_tick_comes_from_the_game_time() {
    let mut g = three();
    g.map(0x0068_0000, 0x10)
        .put_u32(points::GAME_TIME.base, 0x0068_0000)
        .put_u32(0x0068_0000 + GAME_TIME_TICK, 1234);
    assert_eq!(perceive(&g, 1).unwrap().tick, 1234);
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
    let b = bot_block(&human, "Clanker 1 with a long name");
    assert_eq!(&b[BLOCK_NAME..BLOCK_NAME + 16], b"Clanker 1 with \0");
    let flags = u32::from_le_bytes(b[BLOCK_CONTROLS..BLOCK_CONTROLS + 4].try_into().unwrap());
    assert_eq!(flags, 0xffff_fffa, "bits 0 and 2 off, the rest kept");
    assert_eq!(
        &b[BLOCK_CONTROLS + 4..BLOCK_CONTROLS + 7],
        &[0, 0, BOT_SENSITIVITY]
    );
    for (i, (x, y)) in human.iter().zip(&b).enumerate() {
        let ours = (BLOCK_NAME..BLOCK_NAME + 16).contains(&i)
            || (BLOCK_CONTROLS..BLOCK_CONTROLS + 7).contains(&i);
        if !ours {
            assert_eq!(x, y, "byte {i:#x} is the human's");
        }
    }
    // Short names are padded with NULs.
    assert_eq!(
        &bot_block(&human, "B")[BLOCK_NAME..BLOCK_NAME + 3],
        b"B\0\0"
    );
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
