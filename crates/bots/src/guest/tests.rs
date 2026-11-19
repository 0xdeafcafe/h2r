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

/// The walk of `SOUND_OWNER_USER` (`0x18d4f0`) that decides whose own
/// sound a starting sound is: the first mapped local user whose player's
/// unit (`+0x2c`) is the sound's object. (The game also takes what the
/// unit carries, `0xbafb0`, and needs the user in first person; both are
/// the same for a bot and a human.) `Some(user)` plays the sound with no
/// position, in that user's ears.
fn sound_owner(g: &FakeGuest, object: u32) -> Option<u32> {
    (0..4u32).find(|&u| {
        let handle = g.u32(LOCALS + LOCAL_MAP + 4 * u).unwrap();
        handle != u32::MAX
            && player_at(g, handle)
                .unwrap()
                .and_then(|p| p.unit(g).unwrap())
                .is_some_and(|unit| unit.raw() == object)
    })
}

fn unit_of(g: &FakeGuest, player: u32) -> u32 {
    g.u32(PLAYERS_DATA + player * 0x21c + 0x2c).unwrap()
}

#[test]
fn a_bots_own_sound_starts_as_a_world_sound_and_the_humans_stays_in_their_ears() {
    let mut g = three();
    let (human, bot1, bot2) = (unit_of(&g, 0), unit_of(&g, 1), unit_of(&g, 2));
    // As the game ran it: each bot's own sounds (its gun, footsteps,
    // voice) were a local user's, so they played in the ears, unpositioned.
    assert_eq!(
        [human, bot1, bot2].map(|u| sound_owner(&g, u)),
        [Some(0), Some(1), Some(2)]
    );
    // Inside the hook's window the walk finds the human only: a bot's
    // sound has no owner and starts positioned, where the bot is.
    let h = hide(&mut g, 0b110).unwrap().expect("bots to hide");
    assert_eq!(
        [human, bot1, bot2].map(|u| sound_owner(&g, u)),
        [Some(0), None, None]
    );
    restore(&mut g, &h).unwrap();
    assert_eq!(sound_owner(&g, bot1), Some(1), "put back after the call");
    // Without bots the window does nothing.
    assert_eq!(hide(&mut g, 0).unwrap(), None);
}

const SOUND: u32 = 0x0068_0000;

fn listener_active(g: &FakeGuest, u: u32) -> u8 {
    g.u8(SOUND + LISTENER_ACTIVE + LISTENER_STRIDE * u).unwrap()
}

#[test]
fn a_bot_users_listener_goes_quiet_and_the_humans_stays() {
    // The human (user 0) and bots (users 1 and 2), each with an active
    // listener, as the listener update outside SOUND_UPDATE left them.
    let mut g = three();
    g.map(SOUND, 0x200).put_u32(SOUND_GLOBALS, SOUND);
    for u in 0..3 {
        g.put_u8(SOUND + LISTENER_ACTIVE + LISTENER_STRIDE * u, 1);
    }
    let h = hide(&mut g, 0b110).unwrap().expect("bots to hide");
    assert_eq!(h.users(), 0b110);
    assert_eq!(quiet_listeners(&mut g, &h).unwrap(), 0b110);
    assert_eq!(
        [0, 1, 2].map(|u| listener_active(&g, u)),
        [1, 0, 0],
        "only the human's ears"
    );
    // Restoring the users does not wake the listeners: the next update
    // runs hidden again.
    restore(&mut g, &h).unwrap();
    assert_eq!(listener_active(&g, 1), 0);
    // Already quiet: nothing to say.
    let h = hide(&mut g, 0b110).unwrap().unwrap();
    assert_eq!(quiet_listeners(&mut g, &h).unwrap(), 0);
    restore(&mut g, &h).unwrap();
}

#[test]
fn no_sound_globals_quiets_nothing() {
    let mut g = three();
    g.map(SOUND_GLOBALS, 4);
    let h = hide(&mut g, 0b10).unwrap().unwrap();
    assert_eq!(quiet_listeners(&mut g, &h).unwrap(), 0);
}

/// The game's controller handles: the human's pad on port 0, bots on 1
/// and 2, nothing on 3.
fn pads() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(PAD_HANDLES, 16)
        .put_u32(PAD_HANDLES, 0x8100_0010)
        .put_u32(PAD_HANDLES + 4, 0x8100_0020)
        .put_u32(PAD_HANDLES + 8, 0x8100_0030);
    g
}

fn handles(g: &FakeGuest) -> [u32; 4] {
    [0, 1, 2, 3].map(|p| g.u32(PAD_HANDLES + 4 * p).unwrap())
}

#[test]
fn the_players_screen_sees_no_controller_on_a_bot_port_and_the_humans_as_it_is() {
    let mut g = pads();
    let h = hide_pads(&mut g, 0b0110)
        .unwrap()
        .expect("bot pads to hide");
    assert_eq!(h.ports(), 0b0110);
    assert_eq!(
        handles(&g),
        [0x8100_0010, 0, 0, 0],
        "the human's pane still has its controller"
    );
    restore_pads(&mut g, &h).unwrap();
    assert_eq!(handles(&g), [0x8100_0010, 0x8100_0020, 0x8100_0030, 0]);
}

#[test]
fn no_bot_controller_hides_nothing_and_a_handle_the_game_wrote_is_kept() {
    let mut g = pads();
    assert_eq!(hide_pads(&mut g, 0).unwrap(), None);
    assert_eq!(
        hide_pads(&mut g, 0b1000).unwrap(),
        None,
        "a bot on port 3 with no pad plugged yet"
    );
    assert_eq!(handles(&g), [0x8100_0010, 0x8100_0020, 0x8100_0030, 0]);
    let h = hide_pads(&mut g, 0b0010).unwrap().unwrap();
    // The game opened port 1 again in the meantime: its handle stays.
    g.put_u32(PAD_HANDLES + 4, 0x8100_0040);
    restore_pads(&mut g, &h).unwrap();
    assert_eq!(handles(&g)[1], 0x8100_0040);
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
            team: 1,
            shield: 0.0,
            health: 0.0,
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
fn the_main_menu_ticks_but_is_not_a_game() {
    // The h2 bots test run: `mode main_menu` with a game tick on every
    // menu screen.
    const TIME: u32 = 0x0068_0000;
    const GLOBALS: u32 = 0x0069_0000;
    let mut g = FakeGuest::new();
    assert!(!in_game(&g), "nothing loaded");
    g.map(points::GAME_TIME.base & !0xfff, 0x1000)
        .map(TIME, 0x10)
        .put_u32(points::GAME_TIME.base, TIME)
        .put_u32(TIME + GAME_TIME_TICK, 42);
    g.map(points::GAME_GLOBALS.base & !0xfff, 0x1000)
        .map(GLOBALS, 0x400)
        .put_u32(points::GAME_GLOBALS.base, GLOBALS)
        .put_u32(GLOBALS + OPTIONS, MODE_MAIN_MENU);
    assert_eq!(game_tick(&g), Some(42));
    assert_eq!(game_mode(&g), Some(MODE_MAIN_MENU));
    assert!(!in_game(&g), "the main menu's game");
    g.put_u32(GLOBALS + OPTIONS, MODE_MULTIPLAYER);
    assert!(in_game(&g), "a multiplayer game");
    g.put_u32(GLOBALS + OPTIONS, 1);
    assert!(in_game(&g), "a campaign");
    g.put_u32(points::GAME_GLOBALS.base, 0);
    assert!(in_game(&g), "a tick and no options read: a game");
    g.put_u32(points::GAME_TIME.base, 0);
    assert!(!in_game(&g), "no tick");
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
fn signing_out_calls_the_games_whole_slot_sign_out_per_port_then_the_frame() {
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
    // The whole slot's sign-out (Live, local user, profile), not the
    // profile's alone: with its local user kept, the lobby's session kept
    // the bot's player (the user's run: off, and they stayed in the lobby).
    let out = points::SLOT_SIGN_OUT.base;
    assert_eq!(calls, vec![(out, Some(1)), (out, Some(3)), (THEN, None)]);
    assert!(
        run.calls
            .iter()
            .all(|c| c.target != points::PROFILE_SIGN_OUT.base),
        "never the profile's sign-out alone"
    );
}

#[test]
fn a_slot_is_held_while_it_has_a_profile_or_xbox_live() {
    let mut g = FakeGuest::new();
    slots(&mut g);
    assert!(!slot_held(&g, 1).unwrap(), "empty");
    g.put_u32(slot(1), SLOT_PROFILE);
    assert!(slot_held(&g, 1).unwrap(), "a profile");
    g.put_u32(slot(1), SLOT_PROFILE | SLOT_LIVE);
    assert!(slot_held(&g, 1).unwrap(), "a profile on Live");
    // A profile signed out with the Live flag left (what the profile-only
    // sign-out left behind): still held, so it is signed out again.
    g.put_u32(slot(1), SLOT_LIVE);
    assert!(slot_held(&g, 1).unwrap(), "Live with no profile");
    g.put_u32(slot(1), 0);
    assert!(!slot_held(&g, 1).unwrap(), "signed out");
    assert!(!slot_held(&g, 2).unwrap(), "another port");
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
    assert_eq!(c.args, vec![1, BUFFER, NO_FILE], "no file: nothing saved");
    assert_eq!(NO_FILE, u32::MAX, "the game's -1");
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
fn a_bot_is_never_signed_in_over_a_profile_its_slot_holds() {
    // Port 1's slot holds the user's saved profile (file handle 0x1230):
    // the game's sign-in over it would put the bot's block in its place.
    // Nothing is called and the slot keeps its profile and file.
    let mut g = FakeGuest::new();
    slots(&mut g);
    g.put_u32(slot(0), SLOT_PROFILE)
        .put(slot(0) + SLOT_BLOCK, &halo0001());
    g.put_u32(slot(1), SLOT_PROFILE)
        .put(slot(1) + SLOT_BLOCK, &halo0001())
        .put_u32(slot(1) + SLOT_FILE, 0x1230);
    let run = sign_in_then_run(g, Some(SCREEN));
    let sign_in = points::PROFILE_SIGN_IN.base;
    let ports: Vec<u32> = run
        .calls
        .iter()
        .filter(|c| c.target == sign_in)
        .map(|c| c.args[0])
        .collect();
    assert_eq!(ports, vec![3], "only the empty slot");
    let g = run.guest;
    assert_eq!(g.u32(slot(1) + SLOT_FILE).unwrap(), 0x1230);
    assert_eq!(
        g.bytes(slot(1) + SLOT_BLOCK, BLOCK_LEN).unwrap(),
        halo0001().to_vec(),
        "the user's profile untouched"
    );
    assert_eq!(
        g.u16(SCREEN + SCREEN_MASK).unwrap(),
        0b1001,
        "port 1 not joined"
    );
}

#[test]
fn a_saved_profile_is_a_signed_in_slot_with_a_file() {
    let mut g = FakeGuest::new();
    slots(&mut g);
    for port in 0..4 {
        g.put_u32(slot(port) + SLOT_FILE, NO_FILE);
    }
    assert_eq!(saved_profile(&g, 1).unwrap(), None, "empty");
    g.put_u32(slot(1) + SLOT_FILE, 0x1230);
    assert_eq!(saved_profile(&g, 1).unwrap(), None, "no profile flag");
    g.put_u32(slot(1), SLOT_PROFILE);
    assert_eq!(saved_profile(&g, 1).unwrap(), Some(0x1230));
    // A bot's: signed in with no file.
    g.put_u32(slot(2), SLOT_PROFILE);
    assert_eq!(saved_profile(&g, 2).unwrap(), None);
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
    // The pregame lobby's join (bot_glue::Screen::Lobby): the game's
    // profile sign-in for each bot, then the frame; the session's
    // local-player pass (0x65770) adds them, so nothing else is called.
    let mut g = FakeGuest::new();
    slots(&mut g);
    g.put_u32(slot(0), SLOT_PROFILE);
    let run = sign_in_then_run(g, None);
    assert_eq!(run.result, Ok(Ret::new(0x77, 0)));
    let targets: Vec<(u32, u32)> = run
        .calls
        .iter()
        .map(|c| (c.target, c.args.first().copied().unwrap_or(0)))
        .collect();
    let sign_in = points::PROFILE_SIGN_IN.base;
    assert_eq!(targets[..2], [(sign_in, 1), (sign_in, 3)]);
    assert_eq!(targets.len(), 3, "then the frame, nothing else");
    assert_eq!(targets[2].0, 0x0012_5b40);
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

// ---- Xbox Live guests ----

const AFR: u64 = 0x0009_0000_0000_0afa;

/// The human's account record: its id, user flags `flags` (0 the account's
/// own player), "afr", and a logon result.
fn afr_account(flags: u32) -> [u8; ACCOUNT_LEN] {
    let mut a = [0u8; ACCOUNT_LEN];
    a[..8].copy_from_slice(&AFR.to_le_bytes());
    a[ACCOUNT_FLAGS..ACCOUNT_FLAGS + 4].copy_from_slice(&(0x0001_0000 | flags).to_le_bytes());
    a[0xc..0xf].copy_from_slice(b"afr");
    a[0x6c..0x70].copy_from_slice(&0x1234u32.to_le_bytes());
    a
}

/// Slots: the human on port 0 (on Xbox Live when `live`), and a signed-in
/// bot on each of `bots` with local user `port` (in use) and an id the
/// profile sign-in made up.
fn lobby(live: bool, bots: &[u32]) -> FakeGuest {
    let mut g = FakeGuest::new();
    slots(&mut g);
    g.map(LOCAL_USERS, LOCAL_USER_COUNT * LOCAL_USER_STRIDE);
    let flags = if live {
        SLOT_PROFILE | SLOT_LIVE
    } else {
        SLOT_PROFILE
    };
    g.put_u32(slot(0), flags)
        .put_u32(slot(0) + SLOT_LOCAL_USER, 0)
        .put(slot(0) + SLOT_ACCOUNT, &afr_account(0))
        .put_u8(LOCAL_USERS, 1);
    for &p in bots {
        g.put_u32(slot(p), SLOT_PROFILE)
            .put_u32(slot(p) + SLOT_LOCAL_USER, p)
            .put(slot(p) + SLOT_ID, &[0xee; ID_LEN])
            .put_u8(LOCAL_USERS + p * LOCAL_USER_STRIDE, 1)
            .put(
                LOCAL_USERS + p * LOCAL_USER_STRIDE + LOCAL_USER_ID,
                &[0xee; ID_LEN],
            );
        set_slot_name(&mut g, p, &format!("Clanker {p}")).unwrap();
    }
    g
}

#[test]
fn a_bot_goes_on_live_as_the_humans_guest_with_the_games_writes() {
    let mut g = lobby(true, &[1, 2, 3]);
    for p in 1..=3 {
        assert_eq!(
            go_live(&mut g, p, 0b1110, &format!("Clanker {p}")).unwrap(),
            GoLive::Guest { host: 0, number: p },
            "port {p}"
        );
    }
    for p in 1..=3u32 {
        let flags = g.u32(slot(p)).unwrap();
        assert_eq!(
            flags,
            SLOT_PROFILE | SLOT_LIVE,
            "port {p}: 0x190074(port, 1)"
        );
        let a = account(&g, p).unwrap();
        assert_eq!(account_xuid(&a), AFR, "the human's account");
        assert_eq!(guest_number(&a), p, "guest {p}");
        assert_eq!(a, guest_account(&afr_account(0), p));
        assert_eq!(&a[0xc..0xf], b"afr", "the host's gamertag, as on an Xbox");
        assert_eq!(
            &a[0x6c..0x70],
            &0x1234u32.to_le_bytes(),
            "the host's logon result"
        );
        // The player id the session knows: the slot's and its local user's.
        let id = g.bytes(slot(p) + SLOT_ID, ID_LEN).unwrap();
        assert_eq!(id, a[..ID_LEN].to_vec(), "0x18fee9: +0x470 to +8");
        let user = LOCAL_USERS + p * LOCAL_USER_STRIDE + LOCAL_USER_ID;
        assert_eq!(
            g.bytes(user, ID_LEN).unwrap(),
            id,
            "0x64820: to the local user"
        );
        // The flags are the host's but for the guest number.
        let f = u32::from_le_bytes(id[8..12].try_into().unwrap());
        assert_eq!(f, 0x0001_0000 | p);
        // Its own name, not "afr(1)".
        assert_eq!(
            g.utf16(slot(p) + SLOT_NAME, SLOT_NAME_UNITS).unwrap(),
            format!("Clanker {p}")
        );
    }
    // The human is untouched.
    assert_eq!(account(&g, 0).unwrap(), afr_account(0));
    assert_eq!(g.u32(slot(0)).unwrap(), SLOT_PROFILE | SLOT_LIVE);
}

#[test]
fn offline_or_system_link_nothing_changes() {
    let mut g = lobby(false, &[1, 2]);
    let before = (
        g.u32(slot(1)).unwrap(),
        g.bytes(slot(1) + SLOT_ID, ID_LEN).unwrap(),
    );
    assert_eq!(
        go_live(&mut g, 1, 0b110, "Clanker 1").unwrap(),
        GoLive::NoHost
    );
    assert_eq!(
        (
            g.u32(slot(1)).unwrap(),
            g.bytes(slot(1) + SLOT_ID, ID_LEN).unwrap()
        ),
        before
    );
    assert_eq!(account(&g, 1).unwrap(), [0; ACCOUNT_LEN]);
    assert_eq!(live_host(&g, 0b110).unwrap(), None);
}

#[test]
fn guest_numbers_skip_the_ones_taken_and_run_out_at_three() {
    let mut g = lobby(true, &[1, 2, 3]);
    // A real guest of the human's on port 2 already: guest 1.
    g.put_u32(slot(2), SLOT_PROFILE | SLOT_LIVE)
        .put(slot(2) + SLOT_ACCOUNT, &afr_account(1));
    assert_eq!(free_guest_number(&g, 0).unwrap(), Some(2));
    assert_eq!(
        go_live(&mut g, 1, 0b1010, "Clanker 1").unwrap(),
        GoLive::Guest { host: 0, number: 2 }
    );
    assert_eq!(
        go_live(&mut g, 3, 0b1010, "Clanker 3").unwrap(),
        GoLive::Guest { host: 0, number: 3 }
    );
    assert_eq!(free_guest_number(&g, 0).unwrap(), None);
    // Another account's guest does not take a number from the human's.
    let mut other = afr_account(1);
    other[0] ^= 0xff;
    g.put(slot(2) + SLOT_ACCOUNT, &other);
    assert_eq!(free_guest_number(&g, 0).unwrap(), Some(1));
}

#[test]
fn going_live_needs_a_profile_a_local_user_and_a_free_number() {
    let mut g = lobby(true, &[1]);
    assert_eq!(
        go_live(&mut g, 2, 0b110, "Clanker 2").unwrap(),
        GoLive::NoProfile
    );
    g.put_u32(slot(1) + SLOT_LOCAL_USER, u32::MAX);
    assert_eq!(
        go_live(&mut g, 1, 0b10, "Clanker 1").unwrap(),
        GoLive::NoLocalUser
    );
    g.put_u32(slot(1) + SLOT_LOCAL_USER, 1)
        .put_u8(LOCAL_USERS + LOCAL_USER_STRIDE, 0);
    assert_eq!(
        go_live(&mut g, 1, 0b10, "Clanker 1").unwrap(),
        GoLive::NoLocalUser,
        "a local user not in use"
    );
    assert_eq!(g.u32(slot(1)).unwrap(), SLOT_PROFILE, "nothing written");
    g.put_u8(LOCAL_USERS + LOCAL_USER_STRIDE, 1);
    assert!(matches!(
        go_live(&mut g, 1, 0b10, "Clanker 1").unwrap(),
        GoLive::Guest { .. }
    ));
    assert_eq!(
        go_live(&mut g, 1, 0b10, "Clanker 1").unwrap(),
        GoLive::Already
    );
}

#[test]
fn the_host_is_a_human_on_live_as_their_own_account() {
    let mut g = lobby(true, &[1]);
    assert_eq!(live_host(&g, 0b10).unwrap(), Some(0));
    assert_eq!(live_host(&g, 0b11).unwrap(), None, "port 0 a bot's");
    // A human guest is no host; a second human on Live after it is.
    g.put(slot(0) + SLOT_ACCOUNT, &afr_account(2))
        .put_u32(slot(2), SLOT_PROFILE | SLOT_LIVE)
        .put(slot(2) + SLOT_ACCOUNT, &afr_account(0));
    assert_eq!(live_host(&g, 0b10).unwrap(), Some(2));
}

#[test]
fn the_slot_summary_says_profile_live_and_guest() {
    let mut g = lobby(true, &[1, 2]);
    go_live(&mut g, 1, 0b110, "Clanker 1").unwrap();
    assert_eq!(slot_summary(&g, 0), "port 0 0x30 Live");
    assert_eq!(slot_summary(&g, 1), "port 1 0x30 Live guest 1");
    assert_eq!(slot_summary(&g, 2), "port 2 0x10 offline");
    assert_eq!(slot_summary(&g, 3), "port 3 0x0 empty");
}

fn bot(n: u64) -> BotAccount {
    BotAccount {
        xuid: 0x000b_0000_0000_0100 | n,
        gamertag: format!("Rusty Lugnut {n}"),
    }
}

/// docs/game/bots.md §9.5: a bot with a leased account is on Live as that
/// account: the game's writes as for a guest, but its own xuid, user
/// flags with no guest number and its own gamertag, so the guest numbers
/// stay free and a real guest of the human still gets guest 1.
#[test]
fn a_bot_goes_on_live_as_its_own_account_not_a_guest() {
    let mut g = lobby(true, &[1, 2]);
    for p in 1..=2u32 {
        assert_eq!(
            go_live_as(&mut g, p, 0b0110, &bot(u64::from(p))).unwrap(),
            GoLive::Account { host: 0 },
            "port {p}"
        );
        assert_eq!(g.u32(slot(p)).unwrap(), SLOT_PROFILE | SLOT_LIVE);
        let a = account(&g, p).unwrap();
        assert_eq!(account_xuid(&a), bot(u64::from(p)).xuid);
        assert_eq!(guest_number(&a), 0, "an account's own player");
        let flags = u32::from_le_bytes(a[ACCOUNT_FLAGS..ACCOUNT_FLAGS + 4].try_into().unwrap());
        assert_eq!(flags, 0x0001_0000, "the host's flags, no guest number");
        let tag = format!("Rusty Lugnut {p}");
        assert_eq!(&a[0xc..0xc + tag.len()], tag.as_bytes());
        assert_eq!(a[0xc + tag.len()], 0);
        assert_eq!(&a[0x6c..0x70], &0x1234u32.to_le_bytes(), "the logon result");
        let id = g.bytes(slot(p) + SLOT_ID, ID_LEN).unwrap();
        assert_eq!(id, a[..ID_LEN].to_vec());
        let user = LOCAL_USERS + p * LOCAL_USER_STRIDE + LOCAL_USER_ID;
        assert_eq!(g.bytes(user, ID_LEN).unwrap(), id);
        assert_eq!(
            g.utf16(slot(p) + SLOT_NAME, SLOT_NAME_UNITS).unwrap(),
            tag,
            "shown by its gamertag"
        );
    }
    // Twice is once.
    assert_eq!(
        go_live_as(&mut g, 1, 0b0110, &bot(1)).unwrap(),
        GoLive::Already
    );
    // Every guest number is still free for a real guest.
    assert_eq!(free_guest_number(&g, 0).unwrap(), Some(1));
    // The human is untouched.
    assert_eq!(account(&g, 0).unwrap(), afr_account(0));
}

/// A bot on Live as a guest (no account leased yet) moves to its account
/// when one comes, giving its guest number back; and a new roster's
/// account replaces an old one.
#[test]
fn a_guest_bot_moves_to_its_account_and_a_new_account_replaces_the_old() {
    let mut g = lobby(true, &[1]);
    assert_eq!(
        go_live(&mut g, 1, 0b10, "Clanker 1").unwrap(),
        GoLive::Guest { host: 0, number: 1 }
    );
    assert_eq!(free_guest_number(&g, 0).unwrap(), Some(2));
    assert_eq!(
        go_live_as(&mut g, 1, 0b10, &bot(1)).unwrap(),
        GoLive::Account { host: 0 }
    );
    assert_eq!(free_guest_number(&g, 0).unwrap(), Some(1));
    assert_eq!(
        go_live_as(&mut g, 1, 0b10, &bot(7)).unwrap(),
        GoLive::Account { host: 0 }
    );
    assert_eq!(account_xuid(&account(&g, 1).unwrap()), bot(7).xuid);
}

/// Offline and in System Link nothing changes; with the human gone from
/// Live the bot leaves too, with its offline id and name back.
#[test]
fn without_a_human_on_live_a_bot_account_stays_off_or_leaves() {
    let mut g = lobby(false, &[1]);
    assert_eq!(
        go_live_as(&mut g, 1, 0b10, &bot(1)).unwrap(),
        GoLive::NoHost
    );
    assert_eq!(g.u32(slot(1)).unwrap(), SLOT_PROFILE);
    assert_eq!(account(&g, 1).unwrap(), [0; ACCOUNT_LEN]);

    let mut g = lobby(true, &[1]);
    let offline = slot_id(&g, 1).unwrap();
    assert_eq!(offline, [0xee; ID_LEN]);
    go_live_as(&mut g, 1, 0b10, &bot(1)).unwrap();
    // The host's logon fails: the game signs out the slots with its id.
    g.put_u32(slot(0), SLOT_PROFILE);
    assert!(leave_live(&mut g, 1, &offline, "Clanker 1").unwrap());
    assert_eq!(g.u32(slot(1)).unwrap(), SLOT_PROFILE);
    assert_eq!(account(&g, 1).unwrap(), [0; ACCOUNT_LEN]);
    assert_eq!(slot_id(&g, 1).unwrap(), offline);
    let user = LOCAL_USERS + LOCAL_USER_STRIDE + LOCAL_USER_ID;
    assert_eq!(g.bytes(user, ID_LEN).unwrap(), offline.to_vec());
    assert_eq!(
        g.utf16(slot(1) + SLOT_NAME, SLOT_NAME_UNITS).unwrap(),
        "Clanker 1"
    );
    assert!(
        !leave_live(&mut g, 1, &offline, "Clanker 1").unwrap(),
        "once"
    );
}

#[test]
fn a_long_gamertag_is_cut_to_fit_the_record() {
    let a = bot_account_record(
        &afr_account(2),
        &BotAccount {
            xuid: 0x000b_0000_0000_0001,
            gamertag: "ABCDEFGHIJKLMNOPQRS".into(),
        },
    );
    assert_eq!(&a[0xc..0x1b], b"ABCDEFGHIJKLMNO");
    assert_eq!(a[0x1b], 0);
    assert_eq!(guest_number(&a), 0);
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

    /// A human (0, red) and the bot on port 1 (1, red) at X 5, and a blue
    /// enemy (2) at X 50; objects 3 and 4 are weapons at `at3` and `at4`.
    fn ctf_game(at3: [f32; 3], at4: [f32; 3]) -> FakeGuest {
        let mut g = game(
            &[
                (0, 0, 0, [0.0; 3], X, false),
                (1, 1, 0, [5.0, 0.0, 0.0], X, false),
                (-1, -1, 1, [50.0, 0.0, 0.0], X, false),
            ],
            true,
            &[0, 1],
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
        let p = perceive(&g, 1).unwrap();
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
        // With no designator bound the designator is the team.
        g.put_u16(ENGINE + DESIGNATOR_MASK, 0);
        let o = perceive(&g, 1).unwrap().objective.unwrap();
        assert_eq!(o.spots[0].team, Some(0));
    }

    #[test]
    fn slayer_or_a_fault_reads_no_objective_and_the_bot_still_sees() {
        let mut g = ctf_game([0.0; 3], [0.0; 3]);
        g.put_u32(ENGINE + ENGINE_INDEX, 2);
        let p = perceive(&g, 1).unwrap();
        assert!(p.me.is_some());
        assert_eq!(p.objective, None, "Slayer");
        // A record pointing at unmapped memory: no objective, not blind.
        g.put_u32(ENGINE + ENGINE_INDEX, 1)
            .put_u32(ENGINE + OBJECTIVE_COUNT, 1);
        g.put_u32(points::SCENARIO.base, 0x0dea_d000);
        let p = perceive(&g, 1).unwrap();
        assert!(p.me.is_some());
        assert_eq!(p.objective, None);
        // No engine globals at all.
        g.put_u32(GAME_ENGINE, 0);
        assert_eq!(perceive(&g, 1).unwrap().objective, None);
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
            ],
        );
        g.put_u32(KOTH_GLOBALS, KOTH).map(KOTH, 0x200);
        g.put_u32(KOTH + KOTH_HILL, 1);
        let o = perceive(&g, 1).unwrap().objective.unwrap();
        assert_eq!(o.game, Game::Koth);
        assert!(o.spots.is_empty());
        let h: Hill = o.hill.expect("a hill");
        assert_eq!(h.corners.len(), 4);
        assert_eq!(h.center(), [22.0, 2.0, 1.0]);
        // The hill moves.
        g.put_u32(KOTH + KOTH_HILL, 0);
        let h = perceive(&g, 1).unwrap().objective.unwrap().hill.unwrap();
        assert_eq!(h.center(), [2.0, 0.0, 0.0]);
        // Out of range: no hill.
        g.put_u32(KOTH + KOTH_HILL, 9);
        assert_eq!(perceive(&g, 1).unwrap().objective.unwrap().hill, None);
    }
}

#[test]
fn decisions_observe_player_slot_while_dead_and_only_live_contacts() {
    let players = [
        (0, 0, 0, [0.0; 3], [0.0, 1.0, 0.0], false),
        (1, 1, 1, [4.0, 0.0, 0.0], X, false),
        (-1, -1, 1, [5.0, 0.0, 0.0], X, true),
    ];
    let mut g = game(&players, true, &[0, 1]);
    g.put_f32(BODIES + 0xf0, 0.3);
    let p = perceive(&g, 1).unwrap();
    assert_eq!(p.observations.player, Some((1, 1)));
    assert_eq!(
        p.observations.contacts,
        [crate::brain::Contact {
            id: 0,
            aim: [0.0, 1.0, 0.0],
            shield: None,
            empty_magazine: false,
        }]
    );
    assert!(p.observations.pickups_scanned);
    assert_eq!(p.observations.supplies, Some(Vec::new()));
    g.put_u16(BODIES + 0x400 + 0x10a, BODY_DEPLETED);
    let p = perceive(&g, 1).unwrap();
    assert!(p.me.is_none());
    assert_eq!(p.observations.player, Some((1, 1)));
    assert!(!p.observations.pickups_scanned);
    assert_eq!(p.observations.supplies, None);
}
