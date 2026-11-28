use super::*;
use crate::h1_options_row::{OPTIONS_ACTION, QUICK_OPTIONS};
use guest_layout::datum::DatumArray;
use guest_layout::testing::FakeGuest;

const CLANKERS: i16 = crate::clankers::ROW_ACTION;
const FIREFIGHT: i16 = ff::ROW_ACTION;
/// A custom game's list as retail and the rows of ours build it: Change
/// Map, Change Rules, Quick Options, Clanker Options, Firefight Options,
/// Switch To: Co-Op Game, Switch To: Matchmaking.
const CUSTOM: [i16; 7] = [
    CHANGE_MAP,
    CHANGE_RULES,
    QUICK_OPTIONS,
    CLANKERS,
    FIREFIGHT,
    6,
    8,
];
const ALL: Offer = Offer {
    forge: true,
    firefight: true,
};

/// A row's text, as the Game Setup list draws it.
fn label(action: i16) -> &'static str {
    match action {
        MAP_VARIANT_ACTION => MAP_VARIANT_LABEL,
        CHANGE_MAP => "Change Map...",
        CHANGE_RULES => "Change Rules...",
        QUICK_OPTIONS => "Quick Options...",
        OPTIONS_ACTION => "Halo 1 Options...",
        CLANKERS => "Clanker Options...",
        FIREFIGHT => "Firefight Options...",
        crate::clankermatch::CHANGE_PLAYLIST => CHANGE_PLAYLIST_LABEL,
        6 => "Switch To: Co-Op Game",
        7 => "Switch To: Custom Game",
        8 => "Switch To: Matchmaking",
        crate::clankermatch::SWITCH_ACTION => "Switch To: Clankermatch",
        _ => "?",
    }
}

fn page(mode: Mode, offer: Offer, actions: &[i16]) -> Vec<&'static str> {
    arrange(mode, offer, actions)
        .unwrap_or_else(|| actions.to_vec())
        .into_iter()
        .map(label)
        .collect()
}

/// Game Setup in each mode: the mode's rows, with no Mode row and no
/// Switch To row (the GAME MODE picker picks the mode, `crate::lobby_tabs`).
#[test]
fn game_setup_lists_the_rows_of_each_mode() {
    let table: [(Mode, &[&str]); 3] = [
        (
            Mode::Multiplayer,
            &[
                "Change Map...",
                MAP_VARIANT_LABEL,
                "Change Rules...",
                "Quick Options...",
                "Clanker Options...",
            ],
        ),
        (
            Mode::Forge,
            &[
                "Change Map...",
                MAP_VARIANT_LABEL,
                "Change Rules...",
                "Quick Options...",
                "Clanker Options...",
            ],
        ),
        (
            Mode::Firefight,
            &[
                "Change Map...",
                "Firefight Options...",
                "Quick Options...",
                "Clanker Options...",
            ],
        ),
    ];
    for (mode, want) in table {
        assert_eq!(page(mode, ALL, &CUSTOM), want, "{mode:?}");
    }
    assert_eq!(
        Opener::GAME_SETUP_FORGE.rows,
        page(
            Mode::Forge,
            ALL,
            &[CHANGE_MAP, CHANGE_RULES, QUICK_OPTIONS, CLANKERS, 6, 8]
        )
        .as_slice()
    );
    assert_eq!(
        Opener::GAME_SETUP_FORGE.rows[Opener::GAME_SETUP_FORGE.focus],
        MAP_VARIANT_LABEL
    );
    assert_eq!(
        Opener::GAME_SETUP_FIREFIGHT.rows,
        page(Mode::Firefight, ALL, &CUSTOM).as_slice()
    );
}

/// Clanker Options is on Game Setup in CUSTOM GAMES, FORGE and FIREFIGHT
/// (docs/game/party-lobby.md, "Clankers by tab").
#[test]
fn clanker_options_is_on_every_custom_tab() {
    for tab in [
        crate::lobby_tabs::Tab::CustomGames,
        crate::lobby_tabs::Tab::Forge,
        crate::lobby_tabs::Tab::Firefight,
    ] {
        let mode = tab.parts().mode;
        assert!(shown(mode, CLANKERS), "{tab:?}");
        assert!(
            page(mode, ALL, &CUSTOM).contains(&"Clanker Options..."),
            "{tab:?}"
        );
    }
    assert_eq!(
        Opener::GAME_SETUP_CLANKERS.rows[Opener::GAME_SETUP_CLANKERS.focus],
        "Clanker Options..."
    );
}

#[test]
fn forge_mode_lists_change_map_and_forge_map() {
    let rows = arrange(Mode::Forge, ALL, &CUSTOM).unwrap();
    let at = |a: i16| rows.iter().position(|&r| r == a);
    assert_eq!(at(CHANGE_MAP), Some(0));
    assert_eq!(
        at(MAP_VARIANT_ACTION),
        Some(1),
        "Forge Map follows Change Map"
    );
    assert!(at(CHANGE_RULES).is_some(), "the game type row stays");
    assert!(at(CLANKERS).is_some(), "Clanker Options stays");
    assert_eq!(at(FIREFIGHT), None);
    // Built again (the dialog opens anew), the rows are the same.
    assert_eq!(arrange(Mode::Forge, ALL, &rows), None);
    // A Halo 1 map's row is not the mode's to hide.
    let h1 = arrange(
        Mode::Forge,
        ALL,
        &[CHANGE_MAP, CHANGE_RULES, OPTIONS_ACTION],
    )
    .unwrap();
    assert_eq!(
        h1,
        [CHANGE_MAP, MAP_VARIANT_ACTION, CHANGE_RULES, OPTIONS_ACTION]
    );
}

/// The Switch To rows go from every Game Setup list: the GAME MODE picker
/// changes the mode.
#[test]
fn arrange_drops_the_switch_to_rows() {
    use crate::clankermatch::{CHANGE_PLAYLIST, SWITCH_ACTION};
    // A custom game's list, with Clankermatch's row and kind 6's 9.
    assert_eq!(
        arrange(
            Mode::Multiplayer,
            ALL,
            &[CHANGE_MAP, CHANGE_RULES, 9, SWITCH_ACTION, 6]
        ),
        Some(vec![CHANGE_MAP, MAP_VARIANT_ACTION, CHANGE_RULES])
    );
    // A matchmaking lobby's list keeps Change Playlist.
    assert_eq!(
        arrange(Mode::Multiplayer, ALL, &[9, SWITCH_ACTION, 6]),
        Some(vec![CHANGE_PLAYLIST])
    );
    assert_eq!(
        page(Mode::Multiplayer, ALL, &[9, 6]),
        [CHANGE_PLAYLIST_LABEL]
    );
    // The co-op lobby's list keeps its level and difficulty rows.
    assert_eq!(
        arrange(Mode::Multiplayer, ALL, &[2, 3, 7, 8, SWITCH_ACTION]),
        Some(vec![2, 3])
    );
    // Our mode does not reach a list that is not a custom game's.
    assert_eq!(
        arrange(Mode::Firefight, ALL, &[CHANGE_RULES, 9, 6]),
        Some(vec![CHANGE_RULES, CHANGE_PLAYLIST])
    );
    assert!(switches_mode(9, true) && !switches_mode(9, false));
    for a in [6, 7, 8, SWITCH_ACTION] {
        assert!(switches_mode(a, true) && switches_mode(a, false), "{a}");
    }
    assert!(!switches_mode(CLANKERS, true));
}

#[test]
fn a_list_with_nothing_to_change_is_left_as_built() {
    let plain = [CHANGE_MAP, CHANGE_RULES, QUICK_OPTIONS, CLANKERS];
    // Map Variant... joins a custom game's list with the `forge` feature on.
    assert!(arrange(Mode::Multiplayer, ALL, &plain).is_some());
    let firefight_only = Offer {
        forge: false,
        firefight: true,
    };
    assert_eq!(arrange(Mode::Multiplayer, firefight_only, &plain), None);
    let with_row = arrange(Mode::Multiplayer, ALL, &plain).unwrap();
    assert_eq!(arrange(Mode::Multiplayer, ALL, &with_row), None);
    // No mode besides Multiplayer on offer: the held mode is not applied,
    // and the Switch To rows still go.
    assert_eq!(arrange(Mode::Forge, Offer::default(), &plain), None);
    assert_eq!(
        arrange(Mode::Forge, Offer::default(), &CUSTOM),
        Some(plain.to_vec())
    );
    let forge_only = Offer {
        forge: true,
        firefight: false,
    };
    assert!(
        arrange(Mode::Forge, forge_only, &plain).is_some(),
        "one mode besides Multiplayer is enough"
    );
}

/// No mode's Game Setup has a Mode row or a Switch To row: the GAME MODE
/// picker is the only way to pick the mode (docs/game/party-lobby.md, "The
/// lobby by mode").
#[test]
fn game_setup_has_no_mode_row() {
    for tab in [
        crate::lobby_tabs::Tab::CustomGames,
        crate::lobby_tabs::Tab::Forge,
        crate::lobby_tabs::Tab::Firefight,
    ] {
        let rows = page(tab.parts().mode, ALL, &CUSTOM);
        assert!(
            rows.iter()
                .all(|r| !r.contains("Mode") && !r.starts_with("Switch To")),
            "{tab:?}: {rows:?}"
        );
        assert_eq!(rows[0], "Change Map...", "{tab:?}");
    }
    for opener in [
        Opener::GAME_SETUP,
        Opener::GAME_SETUP_FIREFIGHT,
        Opener::GAME_SETUP_FORGE,
    ] {
        assert!(
            opener
                .rows
                .iter()
                .all(|r| !r.contains("Mode") && !r.starts_with("Switch To")),
            "{}",
            opener.name
        );
    }
}

fn entry(file: &str, name: &str, sentence: Option<&str>) -> VariantEntry {
    VariantEntry {
        file: file.into(),
        name: name.into(),
        sentence: sentence.map(str::to_owned),
        detail: format!("By Builder. Adds 2 Crates ({name})."),
        id: Some(content::Sha256::of(file.as_bytes())),
        suggested_for: None,
        ground: true,
        waves: false,
    }
}

/// A choice that records what was picked: `Some(file)` or Original.
fn recorder() -> (Choose, Arc<Mutex<Vec<Option<String>>>>) {
    let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let log = Arc::clone(&seen);
    let choose: Choose = Arc::new(move |file| {
        log.lock().unwrap().push(file.map(str::to_owned));
        "chosen".into()
    });
    (choose, seen)
}

#[test]
fn the_map_variant_picker_offers_original_first_and_the_variants_by_name() {
    let listed = vec![
        entry("Castle", "Castle", None),
        entry("Two_Towers", "Two Towers", None),
    ];
    let (choose, seen) = recorder();
    let pick = map_variant_pick(Ok(listed), Some("Two_Towers"), choose);
    // The row shows names; the lobby holds the file stem.
    assert_eq!(pick.values, [ORIGINAL, "Castle", "Two Towers"]);
    assert_eq!(pick.current, 2);
    assert_eq!(pick.title, "MAP VARIANT");
    assert_eq!(pick.line(0), "Play the map as it is.");
    assert!(pick.line(1).contains("Adds 2 Crates"));
    (pick.set)(2);
    (pick.set)(1);
    (pick.set)(0);
    assert_eq!(
        *seen.lock().unwrap(),
        [Some("Two_Towers".to_owned()), Some("Castle".to_owned()), None]
    );
    // A list that cannot be read is named, and Original still works.
    let pick = map_variant_pick(Err("permission denied".into()), None, recorder().0);
    assert_eq!(pick.values, [ORIGINAL]);
    assert!(pick.description.contains("permission denied"));
    // A held variant no longer saved focuses Original.
    assert_eq!(map_variant_pick(Ok(vec![]), Some("Gone"), recorder().0).current, 0);
}

/// A King of the Hill game variant named `Hill` suggesting the map
/// variants whose entries (`entry`) have these files, on the base `base`.
fn suggesting(base: &content::Sha256, files: &[&str]) -> variants::portable::Bundle {
    use variants::portable::MapSuggestion;
    let mut v = variants::GameVariant::blank(variants::GameType::King);
    v.set_name("Hill").unwrap();
    let maps = files
        .iter()
        .map(|f| MapSuggestion {
            base: base.to_string(),
            variant: content::Sha256::of(f.as_bytes()).to_string(),
            name: (*f).to_owned(),
        })
        .collect();
    variants::portable::Bundle::new(&v, "Hill", "", None)
        .unwrap()
        .with_maps(maps)
        .unwrap()
}

#[test]
fn the_picker_opens_on_a_suggested_variant_and_keeps_its_order() {
    let base = content::Sha256::of(b"base map");
    let mut listed = vec![
        entry("Castle", "Castle", None),
        entry("Two_Towers", "Two Towers", None),
    ];
    crate::map_variant::suggest(&mut listed, &suggesting(&base, &["Two_Towers"]), &base);
    assert_eq!(listed[0].suggested_for, None);
    assert_eq!(listed[1].suggested_for.as_deref(), Some("Hill"));
    let pick = map_variant_pick(Ok(listed.clone()), None, recorder().0);
    // Original, then by name, as ever; the focus is on the suggestion.
    assert_eq!(pick.values, [ORIGINAL, "Castle", "Two Towers"]);
    assert_eq!(pick.current, 2);
    assert!(pick.line(2).ends_with("Made for Hill."));
    assert!(!pick.line(1).contains("Made for"));
    // A held pick wins over the suggestion.
    assert_eq!(map_variant_pick(Ok(listed), Some("Castle"), recorder().0).current, 1);
}

#[test]
fn a_suggestion_for_another_base_or_an_unusable_variant_does_not_focus() {
    let base = content::Sha256::of(b"base map");
    let other = content::Sha256::of(b"another map");
    let mut listed = vec![entry("Castle", "Castle", None)];
    crate::map_variant::suggest(&mut listed, &suggesting(&other, &["Castle"]), &base);
    assert_eq!(listed[0].suggested_for, None);
    assert_eq!(map_variant_pick(Ok(listed), None, recorder().0).current, 0);
    // A variant not on this machine marks nothing.
    let mut listed = vec![entry("Castle", "Castle", None)];
    crate::map_variant::suggest(&mut listed, &suggesting(&base, &["Gone"]), &base);
    assert_eq!(listed[0].suggested_for, None);
    // A suggested one that cannot be used is not opened on.
    let mut listed = vec![entry("Old", "Old", Some("Old was made for another version."))];
    crate::map_variant::suggest(&mut listed, &suggesting(&base, &["Old"]), &base);
    assert_eq!(map_variant_pick(Ok(listed), None, recorder().0).current, 0);
}

#[test]
fn an_unusable_variant_shows_its_sentence_and_cannot_be_chosen() {
    let why = "Old Keep was made for another version of court.";
    let listed = vec![entry("ok", "Fine", None), entry("old", "Old Keep", Some(why))];
    let pick = map_variant_pick(Ok(listed), None, recorder().0);
    assert_eq!(pick.values, [ORIGINAL, "Fine", "Old Keep"]);
    assert_eq!(pick.line(2), why);
    assert!(!pick.is_blocked(0) && !pick.is_blocked(1));
    assert!(pick.is_blocked(2));
}

/// FORGE: the choice is the file the session opens; Original is a new map.
#[test]
fn in_forge_the_choice_is_the_map_the_session_opens() {
    lobby_start::set_mode(Mode::Forge);
    let choose = choose_in_lobby(std::env::temp_dir(), None);
    assert_eq!(choose(Some("Castle")), "lobby mode: Forge map variant Castle");
    assert_eq!(lobby_start::forge_map().as_deref(), Some("Castle"));
    assert_eq!(choose(None), "lobby mode: a new Forge map");
    assert_eq!(lobby_start::forge_map(), None);
    lobby_start::clear();
}

/// CUSTOM GAMES: Original clears the lobby's pick; a map the lobby does not
/// know cannot be picked from, by name.
#[test]
fn in_a_custom_game_the_choice_is_the_variant_built() {
    lobby_start::clear();
    let choose = choose_in_lobby(std::env::temp_dir(), None);
    assert_eq!(
        choose(Some("Castle")),
        "lobby mode: the lobby's map is not known"
    );
    assert!(crate::map_variant::picked().is_none());
    assert_eq!(choose(None), "lobby mode: the original map");
}

#[test]
fn forge_with_a_remote_member_is_refused() {
    let kit = KitMaps {
        lobby: true,
        installed: true,
    };
    let refused = |mode, remote| refusal(mode, remote, kit).is_some();
    assert_eq!(refusal(Mode::Forge, Some(1), kit), Some(Reason::ForgeRemote));
    assert!(refused(Mode::Forge, None), "a session that cannot be read");
    assert!(
        !refused(Mode::Forge, Some(0)),
        "Split Screen, or only Clankers"
    );
    assert!(!refused(Mode::Multiplayer, Some(3)));
    assert!(!refused(Mode::Firefight, Some(3)));
    assert_eq!(start_state(READY_STATE, true), REFUSED_STATE);
    assert_eq!(start_state(READY_STATE, false), READY_STATE);
    assert_eq!(
        start_state(10, true),
        10,
        "only ready is held: a countdown already running is the game's"
    );
    assert!(REMOTE_REFUSAL.ends_with('.') && !REMOTE_REFUSAL.contains("  "));
}

/// The squad session's peers: this console, a Clanker's phantom and, with
/// `remote`, a console of a person.
fn session(networking: u8, remote: bool) -> FakeGuest {
    use bots::lobby;
    use bots::phantom::session::at;
    const S: u32 = 0x0010_0000;
    let mut g = FakeGuest::new();
    g.map(0x0052_7000, 0x1000).map(S, 0x8000);
    g.put_u8(lobby::NETWORKING, networking)
        .put_u32(lobby::SESSION, S)
        .put_u32(S + at::LOCAL_PEER, 0)
        .put_u32(S + at::PEER_COUNT, 2 + u32::from(remote));
    let mac = |p: u32| S + at::PEERS + p * at::PEER_SIZE + at::PEER_MAC;
    g.put(mac(0), &[0, 0x50, 0xf2, 1, 2, 3]);
    g.put(
        mac(1),
        &bots::phantom::machine_id(bots::phantom::PhantomHandle(7)),
    );
    g.put(mac(2), &[0, 0x50, 0xf2, 9, 9, 9]);
    g
}

#[test]
fn a_remote_member_is_a_peer_of_a_person_on_another_console() {
    assert_eq!(
        remote_members(&session(1, false)),
        Some(0),
        "a Clanker is not one"
    );
    assert_eq!(remote_members(&session(1, true)), Some(1));
    assert_eq!(
        remote_members(&session(0, true)),
        Some(0),
        "not networked: Split Screen"
    );
    let mut none = session(1, true);
    none.put_u32(bots::lobby::SESSION, 0);
    assert_eq!(remote_members(&none), None);
}

/// FIREFIGHT opens with no kit map installed; Start Game is what refuses,
/// and the status line and the panel say why in one line each.
#[test]
fn firefight_without_a_kit_map_refuses_the_start_and_says_why() {
    let none = KitMaps::default();
    let elsewhere = KitMaps {
        lobby: false,
        installed: true,
    };
    let here = KitMaps {
        lobby: true,
        installed: true,
    };
    assert_eq!(refusal(Mode::Firefight, Some(0), none), Some(Reason::NoKitMap));
    assert_eq!(
        refusal(Mode::Firefight, Some(0), elsewhere),
        Some(Reason::NotKitMap)
    );
    assert_eq!(refusal(Mode::Firefight, Some(0), here), None);
    assert_eq!(refusal(Mode::Firefight, Some(2), here), None, "Firefight runs across consoles");
    for mode in [Mode::Multiplayer, Mode::Forge] {
        assert_eq!(refusal(mode, Some(0), none), None, "{mode:?} needs no kit");
    }
    assert_eq!(Reason::NoKitMap.text(), "No installed map supports Firefight yet.");
    assert_eq!(Reason::NotKitMap.text(), "Pick a map variant with AI ground, or a Firefight map.");
    // The panel: one reason line and what to do, in place of the options.
    assert_eq!(
        firefight_map_lines(none).unwrap(),
        ["No installed map supports Firefight yet", "Make one with h2 firefight kit"]
    );
    assert_eq!(
        firefight_map_lines(elsewhere).unwrap()[0],
        "This map has no AI ground"
    );
    assert_eq!(firefight_map_lines(here), None);
    for kit in [none, elsewhere] {
        let lines = firefight_map_lines(kit).unwrap();
        assert!(lines.len() <= MAX_PANEL_LINES);
        for l in lines {
            assert!(!l.contains("  ") && !l.ends_with('.') && !l.ends_with(' '), "{l:?}");
        }
    }
    // The lobby's start state is held off ready while refused.
    assert_eq!(start_state(READY_STATE, true), REFUSED_STATE);
}

#[test]
fn the_start_is_refused_only_in_a_forge_lobby() {
    let g = session(1, true);
    lobby_start::clear();
    assert!(!start_refused(&g), "a Multiplayer lobby");
    lobby_start::set_mode(Mode::Forge);
    assert!(start_refused(&g));
    assert!(!start_refused(&session(1, false)));
    lobby_start::restore(lobby_start::State::default());
    assert!(!start_refused(&g), "no lobby: the level boot's game");
}

#[test]
fn the_help_lines_name_the_map() {
    assert_eq!(map_variant_help(None), "Original plays the map as it is.");
    assert!(map_variant_help(Some("Castle")).contains("Castle"));
}

/// Rows, values and lines read as the game's: no colon between a label
/// and its value, single spaces, no "..." cut of a sentence.
#[test]
fn the_copy_has_no_colon_values_or_repeated_spaces() {
    let options = ff::Options::default();
    let mut texts: Vec<String> = vec![MAP_VARIANT_LABEL.into(), ORIGINAL.into()];
    texts.extend(Mode::ALL.iter().map(|m| m.label().to_owned()));
    texts.extend(panel_lines(Mode::Forge, None, None));
    texts.extend(panel_lines(Mode::Forge, Some("Castle"), None));
    texts.extend(panel_lines(Mode::Firefight, None, Some(Ok(options))));
    for t in &texts {
        assert!(!t.contains(':'), "{t:?} joins a value with a colon");
        assert!(!t.contains("  "), "{t:?} has two spaces");
        assert!(!t.trim_end_matches("...").contains("..."), "{t:?}");
    }
    let picks = [map_variant_pick(
        Ok(vec![entry("Castle", "Castle", None)]),
        None,
        recorder().0,
    )];
    for p in &picks {
        assert!(!p.description.contains("  "), "{}", p.description);
        assert!(p.values.iter().all(|v| !v.contains(':')), "{:?}", p.values);
        assert_eq!(p.title, p.title.to_uppercase());
    }
}

#[test]
fn the_panel_lines_follow_the_mode() {
    assert!(panel_lines(Mode::Multiplayer, Some("Castle"), None).is_empty());
    assert_eq!(panel_lines(Mode::Forge, None, None), ["New Forge map"]);
    assert_eq!(
        panel_lines(Mode::Forge, Some("Castle"), None),
        ["Forge map Castle"]
    );
    let mut o = ff::Options::default();
    assert_eq!(
        panel_lines(Mode::Firefight, None, Some(Ok(o.clone()))),
        [
            "Holdout waves",
            "Normal difficulty",
            "The wave set's own lives"
        ]
    );
    o.wave_set = 2;
    o.difficulty = ::firefight::Difficulty::Legendary;
    o.lives = Some(1);
    assert_eq!(
        panel_lines(Mode::Firefight, None, Some(Ok(o.clone()))),
        [
            "Last Stand waves",
            "Legendary difficulty",
            "1 starting life"
        ]
    );
    o.lives = Some(5);
    assert_eq!(
        panel_lines(Mode::Firefight, None, Some(Ok(o.clone())))[2],
        "5 starting lives"
    );
    // Two teams say so on the wave set's line: Crossfire's own, or a set
    // the Teams row splits.
    o.wave_set = 3;
    assert_eq!(
        panel_lines(Mode::Firefight, None, Some(Ok(o.clone())))[0],
        "Crossfire waves, 2 teams"
    );
    o.teams = Some(1);
    assert_eq!(
        panel_lines(Mode::Firefight, None, Some(Ok(o.clone())))[0],
        "Crossfire waves"
    );
    o.wave_set = 2;
    o.teams = Some(2);
    assert_eq!(
        panel_lines(Mode::Firefight, None, Some(Ok(o.clone())))[0],
        "Last Stand waves, 2 teams"
    );
    let unread = panel_lines(
        Mode::Firefight,
        None,
        Some(Err("firefight.lives is None".into())),
    );
    assert_eq!(unread.len(), 1);
    assert!(unread[0].contains("firefight.lives"), "the reason is named");
    for mode in Mode::ALL {
        assert!(panel_lines(mode, None, Some(Ok(o.clone()))).len() <= MAX_PANEL_LINES);
    }
}

const LIST: u32 = 0x1000;
const ARRAY: u32 = 0x2000;
const NAME: u32 = 0x3000;

fn list(actions: &[i16]) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(LIST, 0x100).map(ARRAY, 0x1000).map(NAME, 0x20);
    g.put_u32(LIST + h1row::SETUP_ROWS, ARRAY);
    let rows = DatumArray::at(ARRAY);
    rows.initialize_contiguous(&mut g, NAME, 16, 4, 0, 0)
        .unwrap();
    rows.make_valid(&mut g).unwrap();
    for &a in actions {
        let handle = rows.new_datum(&mut g).unwrap().unwrap();
        let datum = rows.try_get(&g, handle).unwrap().unwrap();
        g.set_u16(datum + h1row::SETUP_ACTION, a as u16).unwrap();
    }
    g
}

/// The list is rewritten to the mode's rows, adding and removing datums.
#[test]
fn the_list_takes_the_modes_rows() {
    let mut g = list(&CUSTOM);
    for mode in [Mode::Forge, Mode::Firefight, Mode::Multiplayer, Mode::Forge] {
        let actions = h1row::setup_actions(&g, LIST).unwrap();
        // `None` is "nothing to change": the list is already the mode's.
        let want = arrange(mode, ALL, &actions).unwrap_or_else(|| actions.clone());
        assert_eq!(h1row::rewrite_rows(&mut g, LIST, &want), Ok(want.len()));
        assert_eq!(h1row::setup_actions(&g, LIST).unwrap(), want, "{mode:?}");
    }
    // Fewer rows: the last datums are deleted.
    assert_eq!(h1row::rewrite_rows(&mut g, LIST, &[CHANGE_MAP]), Ok(1));
    assert_eq!(h1row::setup_actions(&g, LIST).unwrap(), [CHANGE_MAP]);
}

#[test]
fn a_snapshot_keeps_the_lobbys_mode() {
    lobby_start::set_mode(Mode::Forge);
    lobby_start::set_forge_map(Some("Castle".into()));
    let bytes = save_state();
    lobby_start::clear();
    lobby_start::set_kit_map(Some("old-kit".into()));
    load_state(&bytes).unwrap();
    assert_eq!(
        lobby_start::state(),
        lobby_start::State {
            lobby: true,
            mode: Mode::Forge,
            forge_map: Some("Castle".into()),
            map_variant: None,
            kit_map: None,
            kit_classes: None,
        }
    );
    assert!(load_state(&[1]).is_err());
    assert!(load_state(&[1, 9]).is_err(), "no mode 9");
}

/// The header has its own hook on the panel update, inside
/// `lobby-options`'s, and keeps it with that feature off.
#[test]
fn the_header_has_its_own_panel_hook_inside_lobby_options() {
    let panel = |switches: &str| -> String {
        let mut r = hooks::Registry::new(hooks::Switches::parse(switches).unwrap());
        let store = Arc::new(Store::defaults(&ff::DEFS));
        r.add(&crate::lobby_options::LobbyOptions::new(
            Arc::clone(&store),
            None,
            false,
        ))
        .unwrap();
        r.add(&LobbyMode {
            theater: None,
            offer: ALL,
            store: Some(store),
            data: std::env::temp_dir(),
            vfs: None,
        })
        .unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        text.lines()
            .find(|l| l.contains("LOBBY_VARIANT_PANEL"))
            .unwrap_or_else(|| panic!("no panel hook: {text}"))
            .to_owned()
    };
    let on = panel("");
    let (theirs, ours) = (
        on.find("lobby-options:").unwrap(),
        on.find("party-lobby:").unwrap(),
    );
    assert!(theirs < ours, "the header sits inside lobby-options: {on}");
    let off = panel("lobby-options=off");
    assert!(off.contains("party-lobby:"), "{off}");
    assert!(!off.contains("lobby-options:"), "{off}");
}

/// The hooks come after every other Game Setup row's on the plan.
#[test]
fn the_rows_go_after_the_other_rows_on_the_plan() {
    use crate::ui_kit::screen::KitTexts;
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    r.add(&h1row::SetupRow {
        data_root: std::env::temp_dir(),
        page: crate::h1_options_row::test_page(),
    })
    .unwrap();
    let ff_store = Arc::new(Store::defaults(&ff::DEFS));
    let page = Arc::new(ff::page(Arc::clone(&ff_store)));
    r.add(&KitTexts(Arc::clone(&page))).unwrap();
    r.add(&page.class()).unwrap();
    r.add(&ff::SetupRow { page }).unwrap();
    r.add(&LobbyMode {
        theater: None,
        offer: ALL,
        store: Some(ff_store),
        data: std::env::temp_dir(),
        vfs: None,
    })
    .unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    for point in [
        "SQUAD_SETTINGS_LIST_CTOR",
        "SQUAD_SETTINGS_ROW_TEXT",
        "SQUAD_SETTINGS_ROW_CHOSEN",
    ] {
        let line = text.lines().find(|l| l.contains(point)).unwrap();
        let (h1, firefight, ours) = (
            line.find("h1-options-row:").unwrap(),
            line.find("firefight-row:").unwrap(),
            line.find("party-lobby:").unwrap(),
        );
        assert!(h1 < ours && firefight < ours, "{line}");
    }
    assert!(text.contains("LOBBY_START_STATE"), "{text}");
    assert!(text.contains("PARTY_LEAVE"), "{text}");
}

/// The list's arrangement runs last of the constructor's `after` hooks
/// (they run in reverse, so it is the outermost), after Clanker Options,
/// Firefight Options, Halo 1 Options and Switch To: Clankermatch are
/// added: a matchmaking lobby's Game Setup is Change Playlist... alone,
/// with no Switch To: Clankermatch beside it.
#[test]
fn the_arrangement_runs_after_every_row_is_added() {
    use crate::ui_kit::screen::KitTexts;
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    r.add(&h1row::SetupRow {
        data_root: std::env::temp_dir(),
        page: crate::h1_options_row::test_page(),
    })
    .unwrap();
    let bots = Arc::new(crate::clankers::page(Arc::new(Store::defaults(
        &crate::clankers::DEFS,
    ))));
    r.add(&KitTexts(Arc::clone(&bots))).unwrap();
    r.add(&bots.class()).unwrap();
    r.add(&crate::clankers::SetupRow { page: bots }).unwrap();
    r.add(&crate::clankermatch::SwitchRow { hle: None }).unwrap();
    let ff_store = Arc::new(Store::defaults(&ff::DEFS));
    let page = Arc::new(ff::page(Arc::clone(&ff_store)));
    r.add(&KitTexts(Arc::clone(&page))).unwrap();
    r.add(&page.class()).unwrap();
    r.add(&ff::SetupRow { page }).unwrap();
    r.add(&LobbyMode {
        theater: None,
        offer: ALL,
        store: Some(ff_store),
        data: std::env::temp_dir(),
        vfs: None,
    })
    .unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    let line = text
        .lines()
        .find(|l| l.contains("SQUAD_SETTINGS_LIST_CTOR"))
        .unwrap();
    // The plan's text lists the `after` hooks in the order they run.
    let after = &line[line.find("after [").expect("after hooks")..];
    let ours = after.find("party-lobby:").expect("the arrangement");
    for theirs in [
        "h1-options-row:",
        "clankers-row:",
        "clankermatch-switch:",
        "firefight-row:",
    ] {
        let at = after.find(theirs).unwrap_or_else(|| panic!("{theirs} {line}"));
        assert!(at < ours, "{theirs} runs before the arrangement: {line}");
    }
    // What the arrangement then makes of each list is
    // `arrange_drops_the_switch_to_rows`'s: the matchmaking lobby's
    // (retail's kind 6 with Switch To: Clankermatch added) keeps Change
    // Playlist... alone.
}

/// Our actions are past retail's and every other row of ours.
#[test]
fn the_rows_actions_are_their_own() {
    for theirs in [
        OPTIONS_ACTION,
        CLANKERS,
        crate::clankermatch::SWITCH_ACTION,
        FIREFIGHT,
    ] {
        assert!(MAP_VARIANT_ACTION > theirs);
    }
}

#[test]
fn the_gate_text_names_the_first_problem_and_counts_the_rest() {
    assert_eq!(gate_text(&[]), None);
    assert_eq!(gate_text(&["a".into()]).as_deref(), Some("a"));
    assert_eq!(
        gate_text(&["a".into(), "b".into(), "c".into()]).as_deref(),
        Some("a (and 2 more)")
    );
    assert!(GATE_SLOT >= PANEL_SLOTS + MAX_PANEL_LINES as u32);
    assert!(GATE_SLOT < IDS.len);
}

/// The lobby's start gate (docs/game/rules-start-gate.md): over invented
/// guest memory (the lobby's map and its variant record) and a map
/// fixture (a census of netgame flags).
#[cfg(feature = "rules")]
mod gate {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use ::rules::ir::{Ir, LabelDef, LabelKind, label_flags};
    use ::rules::labels::Census;
    use agent_proto::rules as wire;
    use forge_doc::{BlockKind, CensusOf, FlagRow, MapRows};
    use variants::ext::{AT, Ext, Mode as ScriptMode, Script};

    use super::*;
    use crate::rules::gate::{self as g, Input, Sources, Verdict};

    const POINTS: &str = "rules \"Double Points\" 1\ntype slayer\n\non kill(victim, killer) { if killer != none { score(killer, 2) } }\n";
    const SLAYER: u32 = 2;
    const VARIANT: u32 = 0x0070_0000;
    const MAP: u32 = 0x10;
    const OTHER_MAP: u32 = 0x11;
    const GOAL: &str = "the map has no zone labelled \"goal\"; Double Points needs exactly 1";
    const CORE: &str = "the map has no zone labelled \"core\"; Double Points needs exactly 1";

    fn zone(text: &str) -> LabelDef {
        LabelDef {
            text: text.into(),
            kind: LabelKind::Zone,
            min: 1,
            max: 1,
            flags: label_flags::SINGLE,
            lo: 0,
            hi: 0,
        }
    }

    /// Double Points with two zones a map must hold one each of.
    fn script() -> Ir {
        let mut ir = ::rules::compile(POINTS).unwrap();
        ir.labels.push(zone("goal"));
        ir.labels.push(zone("core"));
        ir
    }

    fn prefix(ir: &Ir) -> [u8; 8] {
        let mut p = [0; 8];
        p.copy_from_slice(&ir.id().0[..8]);
        p
    }

    /// The map fixture: one territory flag, and nothing labelled `goal` or
    /// `core`.
    fn territory_map() -> Census {
        let rows = MapRows {
            counts: [(BlockKind::NetgameFlags, 1)].into_iter().collect(),
            flags: vec![FlagRow {
                kind: engine::objectives::kind::TERRITORY,
                team: 0,
            }],
            ..MapRows::default()
        };
        Census::of_game(&rows, None).unwrap()
    }

    /// Sources over one census and one blob, counting the census reads.
    struct Fake {
        census: Census,
        blob: Vec<u8>,
        prefix: [u8; 8],
        reads: AtomicU32,
        forge_maps: Mutex<Vec<Option<String>>>,
        /// The layout script each census read named.
        layouts: Mutex<Vec<Option<String>>>,
    }

    impl Fake {
        fn new(census: Census, ir: &Ir) -> Self {
            Fake {
                census,
                blob: ir.encode(),
                prefix: prefix(ir),
                reads: AtomicU32::new(0),
                forge_maps: Mutex::new(Vec::new()),
                layouts: Mutex::new(Vec::new()),
            }
        }

        fn reads(&self) -> u32 {
            self.reads.load(Ordering::SeqCst)
        }

        fn last_forge_map(&self) -> Option<Option<String>> {
            self.forge_maps.lock().unwrap().last().cloned()
        }
    }

    impl Sources for Fake {
        fn census(&self, _map: u32, forge_map: Option<&str>, script: Option<&str>) -> Result<Census, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.layouts.lock().unwrap().push(script.map(str::to_owned));
            self.forge_maps
                .lock()
                .unwrap()
                .push(forge_map.map(str::to_owned));
            Ok(self.census.clone())
        }

        fn blob(&self, prefix: [u8; 8]) -> Option<Vec<u8>> {
            (prefix == self.prefix).then(|| self.blob.clone())
        }
    }

    /// A Split Screen lobby on `map` whose variant record is a Slayer
    /// variant naming the script with prefix `script`, team play or not.
    fn lobby(map: u32, script: [u8; 8], team_play: bool) -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_u8(bots::lobby::NETWORKING, 0)
            .put_u32(crate::h1_options_row::LOBBY_MAP, map);
        let block = Ext {
            script: Script {
                mode: ScriptMode::Rules,
                prefix: script,
                options: 0,
            },
            ..Ext::default()
        }
        .encode();
        m.map(VARIANT, 0x1000);
        m.put(VARIANT + 4, b"Lobby gate test");
        m.put_u32(VARIANT + 0x44, SLAYER)
            .put_u32(VARIANT + 0x48, u32::from(team_play));
        m.put(VARIANT + AT as u32, &block);
        m
    }

    fn refresh(m: &FakeGuest, host: Option<Vec<String>>, sources: &Fake) {
        let input = g::read(m, VARIANT);
        assert!(input.as_ref().is_some_and(|i| i.leads), "a Split Screen lobby leads");
        g::with_lobby(|gate| gate.refresh(input, host, sources));
    }

    fn fresh(mode: Option<Mode>) {
        lobby_start::restore(lobby_start::State::default());
        g::clear();
        if let Some(mode) = mode {
            lobby_start::set_mode(mode);
        }
    }

    #[test]
    fn siege_picker_and_start_check_the_same_selected_record_and_objectives() {
        let siege = crate::community_variants::listed_named("Siege").unwrap();
        let blob = crate::rules::feature::local_blob(siege.ext().script.prefix).unwrap();
        let (ir, _) = ::rules::verify::load(&blob).unwrap();
        let mut guest = lobby(MAP, siege.ext().script.prefix, true);
        guest.put(VARIANT, siege.record.bytes());
        let input = g::read(&guest, VARIANT).unwrap();

        for (census, supported) in [(Census::default(), false), (territory_map(), true)] {
            let sources = Fake::new(census, &ir);
            let offers = g::map_filter_using(&siege.record, &sources).unwrap();
            assert_eq!(offers(MAP), supported);
            assert_eq!(g::check(&input.key, &sources) == Verdict::Clear, supported);
        }
    }

    /// A census holding one flag of each `(label, team)`.
    fn flags_labelled(rows: &[(&str, Option<u8>)]) -> Census {
        use ::rules::labels::{Class, Member};
        let mut census = Census::default();
        for &(label, team) in rows {
            census.labels.entry(label.into()).or_default().push(Member {
                class: Class::Flag,
                number: None,
                team,
            });
        }
        census
    }

    /// The gate refuses a map with no site and no final stand (the script
    /// would draw at its start), and clears one with any of the four
    /// places the script reads: a territory, a placed site, a placed goal
    /// or the defenders' flag stand. The attackers' stand alone is none.
    #[test]
    fn siege_starts_on_a_site_or_a_final_stand_and_nothing_less() {
        let siege = crate::community_variants::listed_named("Siege").unwrap();
        let blob = crate::rules::feature::local_blob(siege.ext().script.prefix).unwrap();
        let (ir, _) = ::rules::verify::load(&blob).unwrap();
        let mut guest = lobby(MAP, siege.ext().script.prefix, true);
        guest.put(VARIANT, siege.record.bytes());
        let input = g::read(&guest, VARIANT).unwrap();

        let cases: [(&str, Census, bool); 7] = [
            ("nothing", flags_labelled(&[]), false),
            ("the attackers' stand alone", flags_labelled(&[("ctf_flag_spawn", Some(0))]), false),
            ("a territory", flags_labelled(&[("territory", None)]), true),
            ("a placed site", flags_labelled(&[("siege_site", None)]), true),
            ("a placed goal", flags_labelled(&[("siege_goal", None)]), true),
            ("the defenders' stand", flags_labelled(&[("ctf_flag_spawn", Some(1))]), true),
            ("both stands", flags_labelled(&[("ctf_flag_spawn", Some(0)), ("ctf_flag_spawn", Some(1))]), true),
        ];
        for (what, census, starts) in cases {
            let sources = Fake::new(census, &ir);
            let verdict = g::check(&input.key, &sources);
            assert_eq!(verdict == Verdict::Clear, starts, "{what}: {verdict:?}");
            let offers = g::map_filter_using(&siege.record, &sources).unwrap();
            assert_eq!(offers(MAP), starts, "{what}: the picker agrees with the start");
        }
    }

    /// Siege's check names its layout to the census; another script's names
    /// none. A map with no layout keeps the labels its flags carry, so the
    /// census is what it was without one.
    #[test]
    fn no_layout_keeps_builtin_labels() {
        let siege = crate::community_variants::listed_named("Siege").unwrap();
        let blob = crate::rules::feature::local_blob(siege.ext().script.prefix).unwrap();
        let (ir, _) = ::rules::verify::load(&blob).unwrap();
        let mut guest = lobby(MAP, siege.ext().script.prefix, true);
        guest.put(VARIANT, siege.record.bytes());
        let input = g::read(&guest, VARIANT).unwrap();
        let sources = Fake::new(territory_map(), &ir);
        assert_eq!(g::check(&input.key, &sources), Verdict::Clear);
        assert_eq!(*sources.layouts.lock().unwrap(), [Some("siege".to_string())]);

        let ir = script();
        let sources = Fake::new(territory_map(), &ir);
        let key = g::key_of(MAP, None, None);
        assert_eq!(g::check(&key, &sources), Verdict::Clear);
        let other = lobby(MAP, prefix(&ir), false);
        let input = g::read(&other, VARIANT).unwrap();
        let _ = g::check(&input.key, &sources);
        assert!(sources.layouts.lock().unwrap().iter().all(Option::is_none));

        // A map with no layout reads as it did: the territory keeps its label.
        let rows = MapRows {
            counts: [(BlockKind::NetgameFlags, 1)].into_iter().collect(),
            flags: vec![FlagRow { kind: engine::objectives::kind::TERRITORY, team: 0 }],
            ..MapRows::default()
        };
        assert!(crate::rules::census::layout_doc(Some("siege"), "lockout", &rows.counts).is_none());
        let things = forge_doc::labelled(&rows, None).unwrap();
        assert_eq!(things.len(), 1);
        assert_eq!(things[0].label.text, "territory");
    }

    #[test]
    fn a_changed_map_forge_map_or_variant_runs_the_check_once() {
        fresh(Some(Mode::Multiplayer));
        let ir = script();
        let p = prefix(&ir);
        let fake = Fake::new(territory_map(), &ir);
        let m = lobby(MAP, p, false);
        refresh(&m, None, &fake);
        refresh(&m, None, &fake);
        assert_eq!(fake.reads(), 1, "the same key keeps its verdict");
        // Another map.
        let m = lobby(OTHER_MAP, p, false);
        refresh(&m, None, &fake);
        refresh(&m, None, &fake);
        assert_eq!(fake.reads(), 2);
        // Forge mode with a new map: the census is the same, so no check.
        lobby_start::set_mode(Mode::Forge);
        refresh(&m, None, &fake);
        assert_eq!(fake.reads(), 2);
        // A saved Forge map: its document applies, so the check runs.
        lobby_start::set_forge_map(Some("Castle".into()));
        refresh(&m, None, &fake);
        refresh(&m, None, &fake);
        assert_eq!(fake.reads(), 3);
        assert_eq!(fake.last_forge_map(), Some(Some(String::from("Castle"))));
        // Back to Multiplayer: the Forge map no longer applies.
        lobby_start::set_mode(Mode::Multiplayer);
        refresh(&m, None, &fake);
        assert_eq!(fake.reads(), 4);
        assert_eq!(fake.last_forge_map(), Some(None));
        // Another variant: team play on.
        let m = lobby(OTHER_MAP, p, true);
        refresh(&m, None, &fake);
        refresh(&m, None, &fake);
        assert_eq!(fake.reads(), 5);
        // A script not on this machine is the host's to check.
        let m = lobby(OTHER_MAP, [0; 8], true);
        refresh(&m, None, &fake);
        assert_eq!(fake.reads(), 5);
        assert_eq!(g::with_lobby(|gate| gate.verdict().clone()), Verdict::Clear);
    }

    #[test]
    fn a_script_index_variant_requires_its_scripts_map_labels() {
        let ir = script();
        let fake = Fake::new(territory_map(), &ir);
        let mut m = lobby(MAP, prefix(&ir), false);
        m.put_u32(VARIANT + 0x44, u32::from(::rules::SCRIPT_INDEX));
        let input = g::read(&m, VARIANT).unwrap();
        assert_eq!(
            g::check(&input.key, &fake),
            Verdict::Blocked(vec![GOAL.to_string(), CORE.to_string()])
        );
        assert_eq!(fake.reads(), 1);
    }

    #[test]
    fn a_blocked_pair_holds_start_game_and_says_why() {
        fresh(Some(Mode::Multiplayer));
        let ir = script();
        let fake = Fake::new(territory_map(), &ir);
        let mut m = lobby(MAP, prefix(&ir), false);
        refresh(&m, None, &fake);
        assert_eq!(g::blocked(), Some(vec![GOAL.to_string(), CORE.to_string()]));
        assert!(start_refused(&m));
        assert_eq!(start_state(READY_STATE, start_refused(&m)), REFUSED_STATE);
        let text = gate_text(&g::blocked().unwrap()).unwrap();
        assert_eq!(text, format!("{GOAL} (and 1 more)"));
        assert_eq!(gate_text_id(&mut m), Some(IDS.id(GATE_SLOT)));
        // A map that has what the script needs starts.
        let fake = Fake::new(::rules::labels::minimal(&ir, &[]), &ir);
        let mut m = lobby(OTHER_MAP, prefix(&ir), false);
        refresh(&m, None, &fake);
        assert_eq!(g::blocked(), None);
        assert!(!start_refused(&m));
        assert_eq!(start_state(READY_STATE, start_refused(&m)), READY_STATE);
        assert_eq!(gate_text_id(&mut m), None);
    }

    #[test]
    fn forge_mode_is_not_held() {
        fresh(Some(Mode::Forge));
        let ir = script();
        let fake = Fake::new(territory_map(), &ir);
        let m = lobby(MAP, prefix(&ir), false);
        refresh(&m, None, &fake);
        assert!(
            matches!(g::with_lobby(|gate| gate.verdict().clone()), Verdict::Blocked(_)),
            "the check still runs"
        );
        assert_eq!(g::blocked(), None);
        assert!(!start_refused(&m), "Split Screen Forge starts");
        // No lobby (the level boot's game) holds nothing either.
        lobby_start::restore(lobby_start::State::default());
        assert_eq!(g::blocked(), None);
    }

    #[test]
    fn a_host_block_becomes_the_lobbys_verdict() {
        fresh(Some(Mode::Multiplayer));
        let ir = script();
        let fake = Fake::new(::rules::labels::minimal(&ir, &[]), &ir);
        let m = lobby(MAP, prefix(&ir), false);
        refresh(&m, None, &fake);
        assert_eq!(g::blocked(), None, "the file's census passes");
        // The game's census differed: the host's block names the problem.
        let host = vec![GOAL.to_string()];
        refresh(&m, Some(host.clone()), &fake);
        assert_eq!(g::blocked(), Some(host.clone()));
        assert_eq!(fake.reads(), 1, "adopted, not checked");
        refresh(&m, Some(host.clone()), &fake);
        assert_eq!(g::blocked(), Some(host.clone()), "kept for the key");
        // Another map checks again and replaces it.
        let m = lobby(OTHER_MAP, prefix(&ir), false);
        refresh(&m, Some(host), &fake);
        assert_eq!(fake.reads(), 2);
        assert_eq!(g::blocked(), None);
    }

    #[test]
    fn a_guest_checks_nothing() {
        fresh(Some(Mode::Multiplayer));
        let ir = script();
        let fake = Fake::new(territory_map(), &ir);
        let m = lobby(MAP, prefix(&ir), false);
        let input = g::read(&m, VARIANT).map(|i| Input { leads: false, ..i });
        g::with_lobby(|gate| gate.refresh(input, Some(vec![GOAL.into()]), &fake));
        assert_eq!(fake.reads(), 0);
        assert_eq!(g::blocked(), None);
    }

    #[test]
    fn rules_state_lists_the_problems_in_the_lobby() {
        fresh(Some(Mode::Multiplayer));
        let ir = script();
        let fake = Fake::new(territory_map(), &ir);
        let m = lobby(MAP, prefix(&ir), false);
        refresh(&m, None, &fake);
        let mut s = wire::State {
            refused: Some("the game ended".into()),
            ..wire::State::default()
        };
        g::annotate(&mut s);
        assert_eq!(s.blocked, [GOAL, CORE]);
        assert!(s.refused.unwrap().contains("Start Game is held"));
        // In Forge mode the problems are listed, and nothing is held.
        lobby_start::set_mode(Mode::Forge);
        let mut s = wire::State::default();
        g::annotate(&mut s);
        assert_eq!(s.blocked, [GOAL, CORE]);
        assert!(s.refused.unwrap().contains("Forge session"));
        // A running script, or the host's own block, is left as it is.
        let mut s = wire::State {
            running: true,
            ..wire::State::default()
        };
        g::annotate(&mut s);
        assert!(s.blocked.is_empty());
        let mut s = wire::State {
            blocked: vec!["the host's".into()],
            ..wire::State::default()
        };
        g::annotate(&mut s);
        assert_eq!(s.blocked, ["the host's"]);
        // Leaving the lobby clears the verdict.
        g::clear();
        let mut s = wire::State::default();
        g::annotate(&mut s);
        assert!(s.blocked.is_empty());
    }
}

#[test]
fn the_map_variant_list_names_a_lobby_map_it_does_not_know() {
    let dir = std::env::temp_dir();
    let why = map_variant_list(&dir, None).unwrap_err();
    assert!(why.contains("not known"), "{why}");
}

#[test]
fn the_picker_refuses_original_and_groundless_variants_in_firefight() {
    let mut basin = entry("Basin", "Basin", None);
    let mut plain = entry("Castle", "Castle", None);
    let mut broken = entry("Old", "Old", Some("Old was made for another version of court."));
    basin.ground = true;
    plain.ground = false;
    broken.ground = true;
    let listed = vec![plain, basin, broken];
    let (choose, seen) = recorder();
    let pick = map_variant_pick_for(Ok(listed.clone()), None, choose, true);
    assert_eq!(pick.values, [ORIGINAL, "Castle", "Basin", "Old"]);
    assert_eq!(pick.blocked, [true, true, false, true]);
    assert_eq!(pick.line(0), NO_GROUND);
    assert_eq!(pick.line(1), NO_GROUND);
    assert!(pick.line(2).contains("Adds 2 Crates"));
    assert!(pick.line(3).contains("another version"), "its own sentence stays");
    assert_eq!(pick.current, 2, "opens on the first variant it can take");
    // A held pick is where it opens, whatever else is listed.
    let held = map_variant_pick_for(Ok(listed.clone()), Some("Castle"), recorder().0, true);
    assert_eq!(held.current, 1);
    // Outside Firefight nothing changes.
    let other = map_variant_pick_for(Ok(listed), None, recorder().0, false);
    assert_eq!(other.blocked, [false, false, false, true]);
    assert_eq!(other.current, 0);
    assert!(seen.lock().unwrap().is_empty());
}

/// A variant with a wave set of its own says so on its picker line, in a
/// Firefight lobby only, after its detail and any suggestion.
#[test]
fn the_picker_says_which_variants_have_their_own_waves() {
    let mut basin = entry("Basin", "Basin", None);
    basin.waves = true;
    let mut plain = entry("Castle", "Castle", None);
    plain.waves = false;
    let mut nothing = entry("Dune", "Dune", None);
    nothing.ground = false;
    nothing.waves = true;
    let listed = vec![basin, plain, nothing];
    let pick = map_variant_pick_for(Ok(listed.clone()), None, recorder().0, true);
    assert_eq!(pick.values, [ORIGINAL, "Basin", "Castle", "Dune"]);
    assert_eq!(pick.line(1), "By Builder. Adds 2 Crates (Basin). Has its own waves.");
    assert_eq!(pick.line(2), "By Builder. Adds 2 Crates (Castle).");
    assert_eq!(pick.line(3), NO_GROUND, "no ground, no line about waves");
    assert_eq!(OWN_WAVES, "Has its own waves.");
    let other = map_variant_pick_for(Ok(listed), None, recorder().0, false);
    assert_eq!(other.line(1), "By Builder. Adds 2 Crates (Basin).");
}

/// With Map Waves the panel names the set the pick carries, not the Wave
/// set row's.
#[test]
fn the_panel_names_the_set_map_waves_plays() {
    let mut own = ::firefight::builtin::definitions()[2].clone();
    own.name = "Invented Basin".into();
    let o = ff::Options { waves: Some(own), ..ff::Options::default() };
    assert_eq!(
        panel_lines(Mode::Firefight, None, Some(Ok(o)))[0],
        "Invented Basin waves"
    );
}

/// Theater is never a Game Setup row, in any mode (the main menu opens it).
#[test]
fn theater_is_not_a_game_setup_row() {
    let mut actions = CUSTOM.to_vec();
    actions.push(THEATER_ACTION);
    for mode in Mode::ALL {
        let rows = arrange(mode, ALL, &actions).unwrap_or_else(|| actions.clone());
        assert!(!rows.contains(&THEATER_ACTION), "{mode:?}");
        assert!(!shown(mode, THEATER_ACTION), "{mode:?}");
    }
}

/// While the built-in kits build, a Firefight lobby with no kit map says it
/// is preparing, in the status line and the panel: never "no installed map
/// supports Firefight". It is refused (Start Game waits) until they land.
#[test]
fn the_lobby_says_preparing_while_kits_build() {
    let none = KitMaps::default();
    let here = KitMaps {
        lobby: true,
        installed: true,
    };
    lobby_start::kit_build_started();
    assert_eq!(
        refusal(Mode::Firefight, Some(0), none),
        Some(Reason::KitBuilding)
    );
    assert_eq!(Reason::KitBuilding.text(), "Preparing Firefight maps.");
    let lines = firefight_map_lines(none).unwrap();
    assert_eq!(lines[0], "Preparing Firefight maps");
    for l in &lines {
        assert!(!l.contains("No installed map") && !l.ends_with('.'), "{l:?}");
    }
    assert!(lines.len() <= MAX_PANEL_LINES);
    // A lobby on a kit map is not held by a build still making another.
    assert_eq!(refusal(Mode::Firefight, Some(0), here), None);
    // Other modes never wait on the kits.
    for mode in [Mode::Multiplayer, Mode::Forge] {
        assert_eq!(refusal(mode, Some(0), none), None);
    }
    // Done with no failure: the build is over and the lobby is looked at
    // once to land on a kit map.
    lobby_start::kit_build_finished(vec![]);
    assert_eq!(lobby_start::kit_build(), KitBuild::Idle);
    assert!(lobby_start::take_kit_landing());
    assert!(!lobby_start::take_kit_landing(), "asked once");
    assert_eq!(
        firefight_map_lines(none).unwrap()[1],
        "Make one with h2 firefight kit"
    );
}

/// A kit that cannot be made is "no map" again, and the panel's second
/// line is that kit's refusal in place of the hint.
#[test]
fn a_failed_build_names_its_reason_in_the_panel() {
    let none = KitMaps::default();
    lobby_start::kit_build_started();
    lobby_start::kit_build_finished(vec![
        "Zanzibar: 6 KB over, sectors".into(),
        "Coagulation: the disc has no maps\\coagulation.map".into(),
    ]);
    assert_eq!(refusal(Mode::Firefight, Some(0), none), Some(Reason::NoKitMap));
    let lines = firefight_map_lines(none).unwrap();
    assert_eq!(
        lines,
        ["No installed map supports Firefight yet", "Zanzibar: 6 KB over, sectors"]
    );
    assert!(lines.len() <= MAX_PANEL_LINES);
    // The next build starts clean.
    lobby_start::kit_build_started();
    assert_eq!(lobby_start::kit_build(), KitBuild::Building);
}

/// A custom game's Game Setup list keeps its map and game type rows in
/// every mode the game type row shows in, and a list the game built empty
/// is named, not written: Siege's lobby once showed an empty Game Setup.
#[test]
fn the_setup_list_keeps_its_rows_and_an_empty_list_is_named() {
    for mode in [Mode::Multiplayer, Mode::Forge] {
        let rows = rows_to_write(mode, ALL, &CUSTOM).unwrap().unwrap_or_else(|| CUSTOM.to_vec());
        assert!(rows.contains(&CHANGE_MAP) && rows.contains(&CHANGE_RULES), "{mode:?}: {rows:?}");
    }
    let firefight = rows_to_write(Mode::Firefight, ALL, &CUSTOM).unwrap().unwrap_or_else(|| CUSTOM.to_vec());
    assert!(firefight.contains(&CHANGE_MAP), "{firefight:?}");
    let e = rows_to_write(Mode::Multiplayer, ALL, &[]).unwrap_err();
    assert!(e.contains("no rows"), "{e}");
}
