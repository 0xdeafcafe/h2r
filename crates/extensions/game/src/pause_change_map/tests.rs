use super::*;

use std::collections::HashMap;

use guest_layout::testing::FakeGuest;
use variants::record::SettingId;

use crate::community_variants::Row;
use crate::lobby_start::Mode;
use super::game_types::{Group, Listed};
use super::quick::{Applies, Setting};

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
    assert_eq!(grown(ARRAY_NAME, 11, true), 12, "whatever the room asked for");
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
    assert!(!offered(&leader, HOST, false), "nothing on Game Setup");
    assert!(
        !offered(&[0, 1, 2, SETUP_ACTION, END_GAME], HOST, true),
        "already there"
    );
}

#[test]
fn the_row_goes_just_above_end_game() {
    assert_eq!(
        placed(&[0, 1, 2, 3, 4, END_GAME]),
        [0, 1, 2, 3, 4, SETUP_ACTION, END_GAME]
    );
    assert_eq!(placed(&[0, 1, 2, END_GAME]), [0, 1, 2, SETUP_ACTION, END_GAME]);
    assert_eq!(placed(&[0, 1, 2]), [0, 1, 2, SETUP_ACTION]);
}

#[test]
fn adding_the_row_writes_every_word_in_slot_order() {
    let retail = [0u16, 1, 2, 4, END_GAME];
    let mut g = menu(grown(ARRAY_NAME, 6, true), &retail);
    assert_eq!(add_row(&mut g, ARRAY).unwrap(), 6);
    assert_eq!(actions(&g, ARRAY).unwrap(), [0, 1, 2, 4, SETUP_ACTION, END_GAME]);
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
fn change_map_opens_what_game_setups_change_map_opens() {
    let cat = map_select::sample();
    assert_eq!(opening(Mode::Multiplayer, &cat), Some(Opening::Categories));
    assert_eq!(opening(Mode::Forge, &cat), opening(Mode::Multiplayer, &cat));
    assert_eq!(opening(Mode::Firefight, &cat), Some(Opening::Categories), "one list in every mode");
    assert_eq!(
        categories(&cat),
        [Category::Halo2, Category::MapPack, Category::Mods, Category::Halo1]
    );
    let names: Vec<String> = maps(&cat, Category::Halo2).into_iter().map(|m| m.name).collect();
    assert_eq!(names[0], "Lockout", "the game's order");

    let mut with_kit: Vec<Map> = cat.all().cloned().collect();
    with_kit.push(kit_map());
    let cat = Catalogue::new(with_kit);
    assert_eq!(opening(Mode::Firefight, &cat), Some(Opening::Categories), "one list in every mode");
    assert!(categories(&cat).contains(&Category::Mods), "installed kit maps are playable maps");

    // One category: its maps straight away; none: no row.
    let one = Catalogue::new(cat.maps(Category::Halo2).to_vec());
    assert_eq!(opening(Mode::Multiplayer, &one), Some(Opening::Maps(Category::Halo2)));
    assert_eq!(opening(Mode::Multiplayer, &Catalogue::default()), None);
}

#[test]
fn every_text_has_its_own_slot() {
    assert!(SLOTS <= IDS.len, "{SLOTS} slots in a family of {}", IDS.len);
    assert_eq!(CLANKER_ROWS as usize, crate::clankers::DEFS.len());
    let mut slots = vec![
        ROW_SLOT,
        SETUP_TITLE_SLOT,
        CATEGORIES_TITLE_SLOT,
        TYPES_TITLE_SLOT,
        QUICK_TITLE_SLOT,
        CLANKER_TITLE_SLOT,
        RULES_TITLE_SLOT,
        FOOTER_SLOT,
    ];
    slots.extend((0..Category::ALL.len() as u32).map(|i| CATEGORY_SLOTS + i));
    slots.extend((0..game_types::ORDER.len() as u32).map(|i| TYPE_SLOTS + i));
    slots.extend((0..SetupRow::ALL.len() as u32).map(|i| SETUP_SLOTS + i));
    slots.extend((0..Setting::ALL.len() as u32).map(|i| QUICK_SLOTS + i));
    slots.extend((0..crate::clankers::DEFS.len() as u32).map(|i| CLANKER_SLOTS + i));
    slots.extend((0..LIST_ROWS as u32).map(|i| RULES_SLOTS + i));
    let unique: std::collections::BTreeSet<_> = slots.iter().collect();
    assert_eq!(unique.len(), slots.len());
    assert_eq!(slots.len() as u32, SLOTS);
    assert!(slots.iter().all(|&s| IDS.owns(IDS.id(s))));
}

// ---- Game Setup's rows ----

const ALL_ROWS: Avail = Avail {
    map: Some(Opening::Categories),
    rules: true,
    quick: true,
    clankers: true,
};

#[test]
fn game_setup_lists_the_lobbys_rows_in_its_order() {
    assert_eq!(
        setup_rows(ALL_ROWS),
        [
            SetupRow::ChangeMap,
            SetupRow::ChangeRules,
            SetupRow::QuickOptions,
            SetupRow::ClankerOptions
        ]
    );
    let labels: Vec<&str> = setup_rows(ALL_ROWS).iter().map(|r| r.label()).collect();
    assert_eq!(
        labels,
        ["Change Map...", "Change Rules...", "Quick Options...", "Clanker Options..."]
    );
    // Without Clankers, without a variant to read, with nothing at all.
    let no_bots = Avail {
        clankers: false,
        ..ALL_ROWS
    };
    assert_eq!(
        setup_rows(no_bots),
        [SetupRow::ChangeMap, SetupRow::ChangeRules, SetupRow::QuickOptions]
    );
    let only_bots = Avail {
        clankers: true,
        ..Avail::default()
    };
    assert_eq!(setup_rows(only_bots), [SetupRow::ClankerOptions]);
    assert!(setup_rows(Avail::default()).is_empty(), "no row on GAME MENU");
}

fn listed(name: &str, ty: GameType, row: Option<Row>) -> Listed {
    let mut v = GameVariant::blank(ty);
    v.set_name(name).unwrap();
    if let Some(row) = row {
        let listed = crate::community_variants::listed_named(name).unwrap();
        assert_eq!(Row::of_type(listed.game_type), Some(row));
        v.set_ext(&listed.ext()).unwrap();
    }
    game_types::listed(v).unwrap()
}

#[test]
fn each_lobby_mode_offers_its_own_rows() {
    let cat = map_select::sample();
    let variants = [listed("Team Slayer", GameType::Slayer, None)];
    let base = GameVariant::blank(GameType::Slayer);
    let a = avail(Mode::Multiplayer, &cat, &variants, Some(&base), true);
    assert_eq!(a, ALL_ROWS);
    // A Firefight lobby's Game Setup has no Change Rules.
    let ff = avail(Mode::Firefight, &cat, &variants, Some(&base), true);
    assert!(!ff.rules);
    assert!(ff.quick && ff.clankers);
    // No built-ins listed: no Change Rules; no lobby variant: no Quick Options.
    let bare = avail(Mode::Multiplayer, &cat, &[], None, false);
    assert_eq!(
        setup_rows(bare),
        [SetupRow::ChangeMap],
        "Change Map alone"
    );
}

#[test]
fn a_rows_line_shows_what_the_draft_picked() {
    let mut d = Draft::default();
    assert_eq!(SetupRow::ChangeMap.line(&d), "Change Map...");
    d.pick_map(0x0a, "Lockout");
    d.pick_rules(&listed("Team Slayer", GameType::Slayer, None));
    assert_eq!(SetupRow::ChangeMap.line(&d), "Change Map  Lockout");
    assert_eq!(SetupRow::ChangeRules.line(&d), "Change Rules  Team Slayer");
    assert_eq!(SetupRow::QuickOptions.line(&d), "Quick Options...");
    assert_eq!(SetupRow::ClankerOptions.line(&d), "Clanker Options...");
}

// ---- Change Rules ----

#[test]
fn the_game_types_are_the_lobbys_with_our_rows_last() {
    let list = vec![
        listed("Team Slayer\u{e008}", GameType::Slayer, None),
        listed("Zombies", GameType::Slayer, Some(Row::PartyGames)),
        listed("CTF Classic", GameType::Ctf, None),
        listed("Crazy King", GameType::King, None),
        listed("Superbounce", GameType::Slayer, Some(Row::PartyGames)),
    ];
    assert_eq!(
        game_types::groups(&list),
        [
            Group::Type(GameType::Slayer),
            Group::Type(GameType::King),
            Group::Type(GameType::Ctf),
            Group::Row(Row::PartyGames),
        ]
    );
    assert_eq!(list[0].name, "Team Slayer", "the list's icon dropped");
    let slayer = game_types::of_group(&list, Group::Type(GameType::Slayer));
    assert_eq!(slayer.len(), 1, "a row's variants are not Slayer's");
    assert_eq!(Group::Type(GameType::Ctf).label(), "Capture the Flag");
    assert_eq!(Group::Row(Row::PartyGames).label(), "Party Games");
    assert!(
        game_types::listed(GameVariant::blank(GameType::Other(5))).is_none(),
        "an engine the list has no row for"
    );
}

#[test]
fn the_built_in_list_is_read_from_the_game() {
    use crate::tweakers::{BUILTIN_LIST, BUILTIN_RECORD, BUILTIN_VARIANT};
    const RECORDS: u32 = 0x0060_0000;
    let mut g = FakeGuest::new();
    g.map(BUILTIN_LIST & !0xfff, 0x1000).map(RECORDS, 0x1000);
    assert!(game_types::read(&g).is_empty(), "no list yet");
    let mut ctf = GameVariant::blank(GameType::Ctf);
    ctf.set_name("CTF Classic").unwrap();
    let mut odd = GameVariant::blank(GameType::Oddball);
    odd.set_name("Rockets Oddball").unwrap();
    let siege = ::rules::compile(::rules::builtin::SIEGE).unwrap();
    let script = variants::portable::record_for_named(&siege, "My Siege", &[]).unwrap();
    // A team script's record is on Slayer's index outside a game; the
    // list files it under its script's row all the same.
    assert_eq!(script.game_type(), GameType::Slayer);
    g.put_u32(BUILTIN_LIST, RECORDS).put_u32(RECORDS, 3);
    for (i, v) in [&ctf, &odd, &script].into_iter().enumerate() {
        let at = RECORDS + 4 + i as u32 * BUILTIN_RECORD + BUILTIN_VARIANT;
        g.write(at, v.bytes()).unwrap();
    }
    let list = game_types::read(&g);
    let names: Vec<&str> = list.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["CTF Classic", "Rockets Oddball", "My Siege"]);
    assert_eq!(list[2].group, Group::Row(Row::Siege));
    assert_eq!(list[2].variant, script);
    assert_eq!(list[1].group, Group::Type(GameType::Oddball));
    assert_eq!(list[0].variant, ctf, "the whole record");
}

// ---- the draft ----

#[test]
fn the_draft_holds_a_map_a_variant_or_both() {
    let t0 = Instant::now();
    let mut d = Draft::default();
    assert!(d.is_empty());
    assert_eq!(d.pending(0, t0), None, "nothing to restart for");

    d.pick_map(0x0a, "Lockout");
    let p = d.pending(1, t0).unwrap();
    assert_eq!((p.map, p.variant.is_none(), p.port), (Some(0x0a), true, 1));
    assert!(d.summary().contains("starts on Lockout."), "{}", d.summary());

    let mut v = Draft::default();
    let team = listed("Team Slayer", GameType::Slayer, None);
    v.pick_rules(&team);
    let p = v.pending(0, t0).unwrap();
    assert_eq!(p.map, None, "the lobby keeps its map");
    assert_eq!(p.variant.as_deref(), Some(&team.variant.to_bytes()));
    assert!(v.summary().ends_with("starts with Team Slayer."), "{}", v.summary());

    d.pick_rules(&team);
    let p = d.pending(0, t0).unwrap();
    assert!(p.map.is_some() && p.variant.is_some());
    assert_eq!(p.name, "Lockout, Team Slayer");
    assert!(d.summary().contains("on Lockout with Team Slayer"), "{}", d.summary());
}

#[test]
fn quick_options_change_the_lobbys_variant_or_the_picked_one() {
    let base = GameVariant::blank(GameType::Slayer);
    let score = Setting::ScoreToWin.id(GameType::Slayer).unwrap();
    let mut d = Draft::default();
    d.set_quick(Some(&base), Setting::ScoreToWin, score, 50).unwrap();
    let v = d.variant.clone().unwrap();
    assert_eq!(v.get(score), Some(50));
    assert_eq!(d.quick, ["Score to Win"]);
    assert_eq!(SetupRow::QuickOptions.line(&d), "Quick Options  Changed");
    assert!(d.summary().contains("this game's rules and your Quick Options"), "{}", d.summary());
    // A value the clean-up refuses leaves the draft as it was.
    assert!(d.set_quick(Some(&base), Setting::ScoreToWin, score, 0x8000).is_err());
    assert_eq!(d.variant.as_ref().unwrap().get(score), Some(50));
    // Picking rules after drops the changes made to the old variant.
    d.pick_rules(&listed("CTF Classic", GameType::Ctf, None));
    assert!(d.quick.is_empty());
    assert_eq!(d.working(Some(&base)).unwrap().game_type(), GameType::Ctf);
}

#[test]
fn quick_options_rows_offer_the_discs_values_else_the_editors_steps() {
    let mut v = GameVariant::blank(GameType::Slayer);
    let score = Setting::ScoreToWin.id(GameType::Slayer).unwrap();
    v.set(score, 30).unwrap();
    v.set(SettingId(10), 600).unwrap();
    let rows = quick::rows(&v, None);
    let labels: Vec<&str> = rows.iter().map(|r| r.setting.label()).collect();
    assert_eq!(labels, ["Score to Win", "Time Limit", "Lives per Round", "Respawn Time"]);
    // 30 is not a fallback step: it is listed, in order, and focused.
    assert!(rows[0].values.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(rows[0].values[rows[0].current], 30);
    assert_eq!(rows[0].line(), "Score to Win  30");
    assert_eq!(rows[1].line(), "Time Limit  10 Minutes");

    let offered = HashMap::from([(score.0, vec![0, 25, 50])]);
    let rows = quick::rows(&v, Some(&offered));
    assert_eq!(rows[0].values, [0, 25, 30, 50]);
    assert_eq!(rows[0].texts()[0], "Unlimited");
    // Each type's own score setting.
    assert_eq!(Setting::ScoreToWin.id(GameType::Ctf), Some(SettingId(1)));
    assert_eq!(Setting::TimeLimit.text(0), "None");
    assert_eq!(Setting::RespawnTime.text(1), "1 Second");
    assert_eq!(Setting::TimeLimit.text(90), "90 Seconds");
}

#[test]
fn quick_options_wait_for_the_restart_and_clanker_options_apply_live() {
    for s in Setting::ALL {
        assert_eq!(quick::applies(s), Applies::AtRestart, "{s:?}");
    }
    for def in crate::clankers::DEFS {
        let want = if def.key == crate::clankers::TEAM.key {
            Applies::AtRestart
        } else {
            Applies::Live
        };
        assert_eq!(clanker_rows::applies(def), want, "{}", def.key);
    }
    // Every key the driver follows is a row of the page.
    for key in crate::clankers::LIVE_KEYS {
        assert!(crate::clankers::DEFS.iter().any(|d| d.key == key), "{key}");
    }
}

#[test]
fn a_clanker_row_is_its_setting_and_its_held_choice() {
    let store = Arc::new(settings::Store::defaults(&crate::clankers::DEFS));
    let line = clanker_rows::line(&crate::clankers::COUNT, &store);
    assert_eq!(line, "Count  1");
    let pick = clanker_rows::pick(&crate::clankers::DIFFICULTY, &store);
    assert_eq!(pick.values[pick.current], "Normal");
    let said = (pick.set)(3);
    assert!(said.contains("Legendary"), "{said}");
    assert_eq!(
        clanker_rows::line(&crate::clankers::DIFFICULTY, &store),
        "Difficulty  Legendary",
        "stored at once"
    );
}

#[test]
fn the_confirm_dialog_focuses_keep_playing() {
    let visit = Arc::new(Visit::default());
    let mut d = Draft::default();
    d.pick_map(0x0a, "Lockout");
    let pick = setup::confirm_pick(&visit, &d);
    assert_eq!(pick.title, "RESTART THE GAME");
    assert_eq!(pick.values, ["Restart with these settings", "Keep playing"]);
    assert_eq!(pick.values[pick.current], "Keep playing");
    assert!(pick.description.contains("Lockout"));
    assert!(pick.after.is_some());
    (pick.set)(0);
    assert_eq!(visit.with(|s| s.confirm), Some(Confirm::Restart));
}

#[test]
fn a_new_visit_starts_with_an_empty_draft() {
    let visit = Visit::default();
    visit.with(|s| s.draft.pick_map(0x0a, "Lockout"));
    visit.open(2, ALL_ROWS, Vec::new(), None);
    assert!(visit.draft().is_empty());
    assert_eq!((visit.port(), visit.with(|s| s.avail)), (2, ALL_ROWS));
}

#[test]
fn picking_a_map_or_a_variant_fills_the_draft() {
    let visit = Arc::new(Visit::default());
    let team = listed("Team Slayer", GameType::Slayer, None);
    visit.open(0, ALL_ROWS, vec![team.clone()], None);
    let p = setup::rules_pick(&visit, Group::Type(GameType::Slayer)).unwrap();
    assert_eq!(p.values, ["Team Slayer"]);
    assert!(p.after.is_some(), "closes the game types list");
    (p.set)(0);
    assert_eq!(visit.draft().rules.as_deref(), Some("Team Slayer"));
    assert!(setup::rules_pick(&visit, Group::Type(GameType::Ctf)).is_none());
}

// ---- the next game ----

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

fn map_only(port: u32, t0: Instant) -> Pending {
    Pending::new(Some(MAP), None, "Lockout".into(), port, t0)
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
    let mut p = map_only(1, t0);
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
    let mut p = map_only(0, t0);
    assert_eq!(p.next(&lobby(MAP), t0), Next::Wait);
    assert_eq!(p.step, Step::Set { at: t0 });
    assert_eq!(p.next(&lobby(MAP), t0 + SETTLE), Next::StartCheck);
    // Someone changes it before the countdown: set ours again.
    let mut q = map_only(0, t0);
    q.next(&lobby(MAP), t0);
    assert_eq!(q.next(&lobby(0x14), t0 + SETTLE), Next::Wait);
    assert_eq!(q.next(&lobby(0x14), t0 + SETTLE), Next::SetMap(MAP));
}

fn record() -> Box<[u8; RECORD_SIZE]> {
    let mut v = GameVariant::blank(GameType::Ctf);
    v.set_name("CTF Classic").unwrap();
    Box::new(v.to_bytes())
}

#[test]
fn a_variant_alone_is_set_on_the_lobbys_map_then_the_countdown_starts() {
    let t0 = Instant::now();
    let mut p = Pending::new(None, Some(record()), "CTF Classic".into(), 0, t0);
    // Whatever map the lobby has stays.
    assert_eq!(p.next(&lobby(0x14), t0), Next::SetVariant(record()));
    assert_eq!(p.next(&lobby(0x14), t0), Next::Wait, "one call a retry");
    p.variant_taken(false, t0);
    assert_eq!(p.next(&lobby(0x14), t0 + RETRY), Next::SetVariant(record()), "refused: again");
    p.variant_taken(true, t0 + RETRY);
    assert!(p.variant_set);
    assert_eq!(p.next(&lobby(0x14), t0 + RETRY + Duration::from_secs(1)), Next::Wait);
    assert_eq!(p.next(&lobby(0x14), t0 + RETRY + SETTLE), Next::StartCheck);
}

#[test]
fn a_map_and_a_variant_are_set_map_first() {
    let t0 = Instant::now();
    let mut p = Pending::new(Some(MAP), Some(record()), "Lockout, CTF Classic".into(), 0, t0);
    assert_eq!(p.next(&lobby(0x14), t0), Next::SetMap(MAP));
    p.map_set(true, t0);
    let t1 = t0 + RETRY;
    assert_eq!(p.next(&lobby(MAP), t1), Next::SetVariant(record()));
    p.variant_taken(true, t1);
    // The wait runs from the variant.
    assert_eq!(p.next(&lobby(MAP), t1 + RETRY), Next::Wait);
    assert_eq!(p.next(&lobby(MAP), t1 + SETTLE), Next::StartCheck);
}

#[test]
fn the_start_state_says_whether_to_wait_start_or_stop() {
    let p = map_only(3, Instant::now());
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
    let mut p = map_only(0, t0);
    let postgame = Lobby { session_type: 4, ..lobby(0x14) };
    assert!(matches!(p.next(&postgame, t0 + TIMEOUT), Next::GiveUp(_)));
}

#[test]
fn the_row_and_its_screens_plan_with_the_other_game_menu_rows() {
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    assert!(registry.add(&crate::pause_menu::Settings(crate::pause_menu::Mode::Multiplayer)).unwrap());
    crate::forge_screens::install(&mut registry).unwrap();
    assert!(install(&mut registry, None).unwrap());
    let plan = registry.build(&hooks::Base).unwrap().text();
    for (point, label) in [
        ("MP_MENU_LIST_CTOR", "pause-change-map:add the Game Setup row"),
        ("MP_MENU_ROW_TEXT", "pause-change-map:name the Game Setup row"),
        ("MP_MENU_ROW_CHOSEN", "pause-change-map:open Game Setup"),
        ("UI_UPDATE", "pause-change-map:pause change map: start the next game"),
    ] {
        let line = plan
            .lines()
            .find(|l| l.contains(&format!(" {point}: ")) && l.contains(label))
            .unwrap_or_else(|| panic!("no {point} line with {label}\n{plan}"));
        if let Some(forge) = line.find("forge-screens:") {
            assert!(forge < line.find(label).unwrap(), "inside Forge's Leave Game hook: {line}");
        }
    }
}

#[test]
fn clanker_options_is_a_screen_only_with_the_clankers_store() {
    let visit = Arc::new(Visit::default());
    let names = |c: &[list_class::ListClass]| c.iter().map(|c| c.screen).collect::<Vec<_>>();
    assert_eq!(
        names(&setup::classes(&visit, None)),
        [SETUP_SCREEN, CATEGORIES_SCREEN, TYPES_SCREEN, RULES_SCREEN, QUICK_SCREEN]
    );
    let store = Arc::new(settings::Store::defaults(&crate::clankers::DEFS));
    assert_eq!(setup::classes(&visit, Some(store)).len(), 6);
}

#[test]
fn the_switch_turns_the_row_and_its_screens_off() {
    let switches = hooks::Switches::defaults().with(FEATURE, false);
    let mut registry = hooks::Registry::new(switches);
    assert!(!install(&mut registry, None).unwrap());
    let plan = registry.build(&hooks::Base).unwrap().text();
    assert!(!plan.contains("pause-change-map"), "{plan}");
}

#[test]
fn a_change_rules_press_limits_the_map_rows_to_the_chosen_wave_set() {
    let visit = Arc::new(Visit::default());
    let basin = crate::community_variants::listed_named("Basin").unwrap();
    let variant = game_types::listed(basin.record.clone()).unwrap();
    visit.open(0, ALL_ROWS, vec![variant], None);
    let pick = setup::rules_pick(&visit, Group::Row(Row::Firefight)).unwrap();
    assert_eq!(pick.values, ["Basin"]);
    (pick.set)(0);
    let draft = visit.draft();
    let chosen = draft.variant.as_ref();
    let creek = Map {
        id: 73,
        name: "Firefight – Beaver Creek".into(),
        path: r"t:\$C\4d53006400020011\beavercreek".into(),
        ..Map::default()
    };
    let cat = Arc::new(Catalogue::new(map_select::sample().all().cloned()
        .chain([kit_map(), creek.clone()])));
    let mode = map_select::variant_mode(Mode::Multiplayer, chosen);
    let maps = cat.for_variant(mode, chosen);
    assert_eq!(maps.all().count(), cat.all().count(), "every map stays in the list");
    assert!(maps.all().any(|m| m.id == creek.id));
    assert_eq!(cat.for_variant(Mode::Forge, chosen).categories(), cat.categories());
}

/// Where a Quick Options dialog's list ends for `l`: its anchor less one
/// 26-unit item a row (`view::Geometry::item_height`).
fn list_bottom(l: list_class::ListLayout) -> i32 {
    i32::from(l.anchor[1]) - i32::from(l.visible) * 26
}

#[test]
fn select_game_shows_every_game_type_with_no_scrolling() {
    let rows = game_types::ORDER.len();
    assert!(rows <= LIST_ROWS, "{rows} game types in a window of {LIST_ROWS}");
    let l = setup::list_layout(rows);
    assert_eq!(l.visible as usize, rows, "visible rows are the rows");
    // The dialog's panel (Quick Options', which holds eleven rows) ends
    // below the list, and the list inside the 640 x 480 safe area.
    let safe = crate::ui_kit::layout::SAFE;
    assert!(list_bottom(l) >= i32::from(safe.bottom), "bottom {}", list_bottom(l));
    assert!(-240 <= list_bottom(l));
}

#[test]
fn change_rules_shows_its_variants_up_to_what_fits_at_four_by_three() {
    for n in 1..=LIST_ROWS {
        assert_eq!(setup::list_layout(n).visible as usize, n, "{n} variants");
    }
    // A longer list is capped where the safe area ends, and scrolls.
    let capped = setup::list_layout(LIST_ROWS + 20);
    assert_eq!(capped.visible as usize, LIST_ROWS);
    assert!(list_bottom(capped) >= i32::from(crate::ui_kit::layout::SAFE.bottom));
}
