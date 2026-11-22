use super::*;

use engine::testing::FakeGuest;

use crate::lobby_start::Mode;

const ARRAY: u32 = 0x2000;
const NAME: u32 = 0x3000;

fn menu(capacity: u32, words: &[u16]) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(ARRAY, 0x1000).map(NAME, 0x20);
    let rows = DatumArray::at(ARRAY);
    rows.initialize_contiguous(&mut g, NAME, capacity as i32, 4, 0, 0)
        .unwrap();
    rows.make_valid(&mut g).unwrap();
    for &w in words {
        let handle = rows.new_datum(&mut g).unwrap().unwrap();
        let datum = rows.try_get(&g, handle).unwrap().unwrap();
        g.set_u16(datum + ACTION_WORD, w).unwrap();
    }
    g
}

const HOST: Menu = Menu {
    networking: true,
    host: true,
    matchmaking: false,
};

#[test]
fn game_menus_array_grows_by_one_only_while_the_feature_is_built() {
    assert_eq!(grown(ARRAY_NAME, 6, true), 7);
    assert_eq!(grown(ARRAY_NAME, 11, true), 12, "on top of the Forge rows");
    assert_eq!(grown(ARRAY_NAME, 6, false), 6);
    assert_eq!(grown(0x1234, 6, true), 6);
}

#[test]
fn only_the_party_leader_who_runs_the_game_gets_the_row() {
    let leader = [0, 1, 2, 3, 4, END_GAME];
    assert!(offered(&leader, HOST, true));
    // Offline (split screen): no host to be.
    let offline = Menu {
        networking: false,
        host: false,
        matchmaking: false,
    };
    assert!(offered(&leader, offline, true));
    // Not the leader, or matchmaking: retail leaves End Game out.
    assert!(!offered(&[0, 1, 2], HOST, true), "no End Game");
    assert!(!offered(&[0, 1, 2, 3], HOST, true), "no End Game");
    let matchmaking = Menu {
        matchmaking: true,
        ..HOST
    };
    assert!(!offered(&leader, matchmaking, true), "matchmaking");
    let member = Menu {
        host: false,
        ..HOST
    };
    assert!(!offered(&leader, member, true), "a networked game hosted elsewhere");
    assert!(!offered(&leader, HOST, false), "nothing to pick");
    assert!(
        !offered(&[0, 1, 2, CHANGE_MAP_ACTION, END_GAME], HOST, true),
        "already there"
    );
}

#[test]
fn the_row_goes_just_above_end_game() {
    assert_eq!(
        placed(&[0, 1, 2, 3, 4, END_GAME]),
        [0, 1, 2, 3, 4, CHANGE_MAP_ACTION, END_GAME]
    );
    assert_eq!(placed(&[0, 1, 2, END_GAME]), [0, 1, 2, CHANGE_MAP_ACTION, END_GAME]);
    assert_eq!(placed(&[0, 1, 2]), [0, 1, 2, CHANGE_MAP_ACTION]);
}

#[test]
fn adding_the_row_writes_every_word_in_slot_order() {
    let retail = [0u16, 1, 2, 4, END_GAME];
    let mut g = menu(grown(ARRAY_NAME, 6, true), &retail);
    assert_eq!(add_row(&mut g, ARRAY).unwrap(), 6);
    assert_eq!(actions(&g, ARRAY).unwrap(), [0, 1, 2, 4, CHANGE_MAP_ACTION, END_GAME]);
    // A second build of the list never adds a second row.
    assert!(!offered(&actions(&g, ARRAY).unwrap(), HOST, true));
    // Without the room grown, refused by name.
    let mut full = menu(6, &[0, 1, 2, 3, 4, END_GAME]);
    let err = add_row(&mut full, ARRAY).unwrap_err();
    assert!(err.contains("room was not grown"), "{err}");
}

fn kit_map() -> Map {
    Map {
        id: 0x2_0010,
        name: "Firefight \u{2013} Lockout".into(),
        sort: 1000,
        path: r"t:\$C\4d53006400020010\lockout".into(),
    }
}

#[test]
fn the_row_opens_what_game_setups_change_map_opens() {
    let cat = map_select::sample();
    assert_eq!(opening(Mode::Multiplayer, &cat), Some(Opening::Categories));
    assert_eq!(opening(Mode::Forge, &cat), Some(Opening::Maps(Category::Halo2)));
    assert_eq!(opening(Mode::Firefight, &cat), None, "no kit map installed");
    assert_eq!(
        categories(&cat),
        [Category::Halo2, Category::MapPack, Category::Mods, Category::Halo1]
    );
    let names: Vec<String> = maps(&cat, Category::Halo2).into_iter().map(|m| m.name).collect();
    assert_eq!(names[0], "Lockout", "the game's order");

    let mut with_kit: Vec<Map> = cat.all().cloned().collect();
    with_kit.push(kit_map());
    let cat = Catalogue::new(with_kit);
    assert_eq!(opening(Mode::Firefight, &cat), Some(Opening::Maps(Category::Firefight)));
    assert!(!categories(&cat).contains(&Category::Firefight), "only a Firefight lobby's");

    // One category: its maps straight away; none: no row.
    let one = Catalogue::new(cat.maps(Category::Halo2).to_vec());
    assert_eq!(opening(Mode::Multiplayer, &one), Some(Opening::Maps(Category::Halo2)));
    assert_eq!(opening(Mode::Multiplayer, &Catalogue::default()), None);
    assert_eq!(Opening::Categories.screen(), CATEGORIES_SCREEN);
    assert_eq!(Opening::Maps(Category::Halo2).screen(), MAPS_SCREEN);
}

#[test]
fn every_text_has_its_own_slot() {
    assert!(MAX_ROWS >= map_select::MAX_MAPS, "{MAX_ROWS} map rows");
    let mut slots = vec![ROW_SLOT, CATEGORIES_TITLE_SLOT, MAPS_TITLE_SLOT];
    slots.extend(Category::ALL.into_iter().map(category_slot));
    slots.extend((0..MAX_ROWS as u32).map(|r| MAP_SLOTS + r));
    let unique: std::collections::BTreeSet<_> = slots.iter().collect();
    assert_eq!(unique.len(), slots.len());
    assert!(slots.iter().all(|&s| IDS.owns(IDS.id(s))));
}

#[test]
fn the_picker_remembers_the_opened_category_and_the_pressing_controller() {
    let p = Picker::default();
    p.open(Opening::Maps(Category::Halo2), 2);
    assert_eq!((p.category(), p.port()), (Some(Category::Halo2), 2));
    *lock(&p.pressed) = Some(0x0a);
    p.open(Opening::Categories, 0);
    assert_eq!(p.take_pressed(), None, "a new open forgets an old press");
    assert_eq!(p.port(), 0);
}

const MAP: u32 = 0x0a;

fn lobby(map: u32) -> Lobby {
    Lobby {
        menu: true,
        lobby_on: true,
        networking: true,
        session_type: 2,
        map,
    }
}

#[test]
fn the_lobby_is_back_only_in_its_pregame() {
    assert!(lobby(MAP).pregame());
    assert!(!Lobby { menu: false, session_type: 3, ..lobby(MAP) }.pregame(), "the game");
    assert!(!Lobby { session_type: 4, ..lobby(MAP) }.pregame(), "the postgame");
    assert!(!Lobby { lobby_on: false, ..lobby(MAP) }.pregame(), "no lobby");
    assert!(
        Lobby { networking: false, session_type: 0, ..lobby(MAP) }.pregame(),
        "an offline lobby"
    );
}

#[test]
fn after_the_game_the_lobby_takes_the_map_then_the_countdown_starts() {
    let t0 = Instant::now();
    let mut p = Pending::new(MAP, "Lockout".into(), 1, t0);
    // The game ends and the postgame shows: nothing yet.
    let in_game = Lobby { menu: false, session_type: 3, ..lobby(0x14) };
    assert_eq!(p.next(&in_game, t0), Next::Wait);
    let postgame = Lobby { session_type: 4, ..lobby(0x14) };
    assert_eq!(p.next(&postgame, t0 + Duration::from_secs(5)), Next::Wait);
    // Back in the lobby on the old map: set it, once a retry.
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.next(&lobby(0x14), t1), Next::SetMap(MAP));
    assert_eq!(p.next(&lobby(0x14), t1 + Duration::from_millis(100)), Next::Wait);
    // Refused: again after the retry.
    p.map_set(false, t1);
    assert_eq!(p.next(&lobby(0x14), t1 + RETRY), Next::SetMap(MAP));
    p.map_set(true, t1 + RETRY);
    // Set: the other consoles get a moment, then the start check.
    assert_eq!(p.next(&lobby(MAP), t1 + RETRY + Duration::from_secs(1)), Next::Wait);
    let t2 = t1 + RETRY + SETTLE;
    assert_eq!(p.next(&lobby(MAP), t2), Next::StartCheck);
    assert_eq!(p.next(&lobby(MAP), t2), Next::Wait, "one call a retry");
    assert_eq!(p.start_state(bots::lobby::READY), Start::Countdown(1));
    assert!(matches!(p.countdown(true), Start::Done(_)));
    assert_eq!(p.countdown(false), Start::Wait, "refused: tried again");
}

#[test]
fn a_lobby_already_on_the_map_goes_straight_to_the_countdown() {
    let t0 = Instant::now();
    let mut p = Pending::new(MAP, "Lockout".into(), 0, t0);
    assert_eq!(p.next(&lobby(MAP), t0), Next::Wait);
    assert_eq!(p.step, Step::MapSet { at: t0 });
    assert_eq!(p.next(&lobby(MAP), t0 + SETTLE), Next::StartCheck);
    // Someone changes it before the countdown: set ours again.
    let mut q = Pending::new(MAP, "Lockout".into(), 0, t0);
    q.next(&lobby(MAP), t0);
    assert_eq!(q.next(&lobby(0x14), t0 + SETTLE), Next::Wait);
    assert_eq!(q.next(&lobby(0x14), t0 + SETTLE), Next::SetMap(MAP));
}

#[test]
fn the_start_state_says_whether_to_wait_start_or_stop() {
    let p = Pending::new(MAP, "Lockout".into(), 3, Instant::now());
    assert_eq!(p.start_state(9), Start::Countdown(3));
    assert!(matches!(p.start_state(10), Start::Done(_)), "already counting down");
    assert!(matches!(p.start_state(8), Start::GiveUp(_)), "not the party leader");
    for waiting in [0, 1, 4, 5, 6, 7] {
        assert_eq!(p.start_state(waiting), Start::Wait, "state {waiting}");
    }
}

#[test]
fn a_next_game_that_never_starts_gives_up_and_leaves_the_lobby_as_it_is() {
    let t0 = Instant::now();
    let mut p = Pending::new(MAP, "Lockout".into(), 0, t0);
    let postgame = Lobby { session_type: 4, ..lobby(0x14) };
    assert!(matches!(p.next(&postgame, t0 + TIMEOUT), Next::GiveUp(_)));
}

#[test]
fn the_row_and_its_picker_plan_with_the_other_game_menu_rows() {
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    assert!(registry.add(&crate::pause_menu::Settings(crate::pause_menu::Mode::Multiplayer)).unwrap());
    crate::forge_screens::install(&mut registry).unwrap();
    assert!(install(&mut registry).unwrap());
    let plan = registry.build(&hooks::Base).unwrap().text();
    for (point, label) in [
        ("MP_MENU_LIST_CTOR", "pause-change-map:add the Change Map row"),
        ("MP_MENU_ROW_TEXT", "pause-change-map:name the Change Map row"),
        ("MP_MENU_ROW_CHOSEN", "pause-change-map:open the Change Map picker"),
        ("UI_UPDATE", "pause-change-map:pause change map: start the next game"),
    ] {
        let line = plan
            .lines()
            .find(|l| l.contains(&format!(" {point}: ")) && l.contains(label))
            .unwrap_or_else(|| panic!("no {point} line with {label}\n{plan}"));
        if let Some(forge) = line.find("forge-screens:") {
            assert!(forge < line.find(label).unwrap(), "inside the Forge rows' hook: {line}");
        }
    }
}

#[test]
fn the_switch_turns_the_row_and_its_picker_off() {
    let switches = hooks::Switches::defaults().with(FEATURE, false);
    let mut registry = hooks::Registry::new(switches);
    assert!(!install(&mut registry).unwrap());
    let plan = registry.build(&hooks::Base).unwrap().text();
    assert!(!plan.contains("pause-change-map"), "{plan}");
}
