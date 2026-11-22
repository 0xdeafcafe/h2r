use super::*;
use crate::h1_options_row::{OPTIONS_ACTION, QUICK_OPTIONS};
use engine::datum::DatumArray;
use engine::testing::FakeGuest;

const CLANKERS: i16 = crate::clankers::ROW_ACTION;
const FIREFIGHT: i16 = ff::ROW_ACTION;
/// A custom game's list as the rows of ours leave it: Change Map, Change
/// Rules, Quick Options, Clanker Options, Firefight Options, Switch To:
/// Co-Op Game, Switch To: Matchmaking.
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
        FORGE_MAP_ACTION => FORGE_MAP_LABEL,
        CHANGE_MAP => "Change Map...",
        CHANGE_RULES => "Change Rules...",
        QUICK_OPTIONS => "Quick Options...",
        OPTIONS_ACTION => "Halo 1 Options...",
        CLANKERS => "Clanker Options...",
        FIREFIGHT => "Firefight Options...",
        6 => "Switch To: Co-Op Game",
        8 => "Switch To: Matchmaking",
        _ => "?",
    }
}

fn page(mode: Mode, offer: Offer, actions: &[i16]) -> Vec<&'static str> {
    arrange(mode, offer, actions)
        .expect("a custom game's list")
        .into_iter()
        .map(label)
        .collect()
}

/// Game Setup in each mode: the mode's rows, with no Mode row (the mode
/// strip picks the mode, `crate::lobby_tabs`).
#[test]
fn game_setup_lists_the_rows_of_each_mode() {
    let table: [(Mode, &[&str]); 3] = [
        (
            Mode::Multiplayer,
            &[
                "Change Map...",
                "Change Rules...",
                "Quick Options...",
                "Clanker Options...",
                "Firefight Options...",
                "Switch To: Co-Op Game",
                "Switch To: Matchmaking",
            ],
        ),
        (
            Mode::Forge,
            &[
                "Change Map...",
                FORGE_MAP_LABEL,
                "Change Rules...",
                "Quick Options...",
                "Switch To: Co-Op Game",
                "Switch To: Matchmaking",
            ],
        ),
        (
            Mode::Firefight,
            &[
                "Change Map...",
                "Firefight Options...",
                "Quick Options...",
                "Clanker Options...",
                "Switch To: Co-Op Game",
                "Switch To: Matchmaking",
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
            &[CHANGE_MAP, CHANGE_RULES, QUICK_OPTIONS, 6, 8]
        )
        .as_slice()
    );
    assert_eq!(
        Opener::GAME_SETUP_FORGE.rows[Opener::GAME_SETUP_FORGE.focus],
        FORGE_MAP_LABEL
    );
}

#[test]
fn forge_mode_lists_change_map_and_forge_map() {
    let rows = arrange(Mode::Forge, ALL, &CUSTOM).unwrap();
    let at = |a: i16| rows.iter().position(|&r| r == a);
    assert_eq!(at(CHANGE_MAP), Some(0));
    assert_eq!(
        at(FORGE_MAP_ACTION),
        Some(1),
        "Forge Map follows Change Map"
    );
    assert!(at(CHANGE_RULES).is_some(), "the game type row stays");
    assert_eq!(at(CLANKERS), None);
    assert_eq!(at(FIREFIGHT), None);
    // Built again (the dialog opens anew), the rows are the same.
    assert_eq!(arrange(Mode::Forge, ALL, &rows).unwrap(), rows);
    // A Halo 1 map's row is not the mode's to hide.
    let h1 = arrange(
        Mode::Forge,
        ALL,
        &[CHANGE_MAP, CHANGE_RULES, OPTIONS_ACTION],
    )
    .unwrap();
    assert_eq!(
        h1,
        [CHANGE_MAP, FORGE_MAP_ACTION, CHANGE_RULES, OPTIONS_ACTION]
    );
}

#[test]
fn a_lobby_with_no_mode_on_offer_or_no_change_map_is_retails() {
    assert_eq!(arrange(Mode::Multiplayer, Offer::default(), &CUSTOM), None);
    assert_eq!(
        arrange(Mode::Forge, ALL, &[CHANGE_RULES, QUICK_OPTIONS]),
        None,
        "a list without Change Map (matchmaking) is not a custom game's"
    );
    let forge_only = Offer {
        forge: true,
        firefight: false,
    };
    assert!(
        arrange(Mode::Forge, forge_only, &CUSTOM).is_some(),
        "one mode besides Multiplayer is enough"
    );
}

/// No tab's Game Setup has a Mode row: the strip is the only way to pick
/// the mode (docs/game/party-lobby.md, "The lobby by tab").
#[test]
fn game_setup_has_no_mode_row() {
    for tab in [
        crate::lobby_tabs::Tab::CustomGames,
        crate::lobby_tabs::Tab::Forge,
        crate::lobby_tabs::Tab::Firefight,
    ] {
        let rows = page(tab.parts().mode, ALL, &CUSTOM);
        assert!(
            rows.iter().all(|r| !r.contains("Mode")),
            "{tab:?}: {rows:?}"
        );
        assert_eq!(rows[0], "Change Map...", "{tab:?}");
    }
    assert!(
        Opener::GAME_SETUP_FORGE
            .rows
            .iter()
            .all(|r| !r.contains("Mode"))
    );
}

#[test]
fn the_forge_map_picker_offers_new_and_the_saved_maps() {
    let pick = forge_map_pick(Ok(vec!["Castle".into(), "Towers".into()]), Some("Towers"));
    assert_eq!(pick.values, [NEW_MAP, "Castle", "Towers"]);
    assert_eq!(pick.current, 2);
    assert_eq!(pick.title, "FORGE MAP");
    assert_eq!((pick.set)(1), "lobby mode: Forge map Castle");
    assert_eq!(lobby_start::forge_map().as_deref(), Some("Castle"));
    assert_eq!((pick.set)(0), "lobby mode: a new Forge map");
    assert_eq!(lobby_start::forge_map(), None);
    // A list that cannot be read is named, and New still works.
    let pick = forge_map_pick(Err("permission denied".into()), None);
    assert_eq!(pick.values, [NEW_MAP]);
    assert!(pick.description.contains("permission denied"));
    // A held map no longer saved focuses New.
    assert_eq!(forge_map_pick(Ok(vec![]), Some("Gone")).current, 0);
}

#[test]
fn forge_with_a_remote_member_is_refused() {
    assert!(refused(Mode::Forge, Some(1)));
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
    assert_eq!(forge_map_help(None), "New starts from the map as it is.");
    assert!(forge_map_help(Some("Castle")).contains("Castle"));
}

/// Rows, values and lines read as the game's: no colon between a label
/// and its value, single spaces, no "..." cut of a sentence.
#[test]
fn the_copy_has_no_colon_values_or_repeated_spaces() {
    let options = ff::Options::default();
    let mut texts: Vec<String> = vec![FORGE_MAP_LABEL.into(), NEW_MAP.into()];
    texts.extend(Mode::ALL.iter().map(|m| m.label().to_owned()));
    texts.extend(panel_lines(Mode::Forge, None, None));
    texts.extend(panel_lines(Mode::Forge, Some("Castle"), None));
    texts.extend(panel_lines(Mode::Firefight, None, Some(Ok(options))));
    for t in &texts {
        assert!(!t.contains(':'), "{t:?} joins a value with a colon");
        assert!(!t.contains("  "), "{t:?} has two spaces");
        assert!(!t.trim_end_matches("...").contains("..."), "{t:?}");
    }
    let picks = [forge_map_pick(Ok(vec!["Castle".into()]), None)];
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
        panel_lines(Mode::Firefight, None, Some(Ok(o))),
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
        panel_lines(Mode::Firefight, None, Some(Ok(o))),
        [
            "Last Stand waves",
            "Legendary difficulty",
            "1 starting life"
        ]
    );
    o.lives = Some(5);
    assert_eq!(
        panel_lines(Mode::Firefight, None, Some(Ok(o)))[2],
        "5 starting lives"
    );
    let unread = panel_lines(
        Mode::Firefight,
        None,
        Some(Err("firefight.lives is None".into())),
    );
    assert_eq!(unread.len(), 1);
    assert!(unread[0].contains("firefight.lives"), "the reason is named");
    for mode in Mode::ALL {
        assert!(panel_lines(mode, None, Some(Ok(o))).len() <= MAX_PANEL_LINES);
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
        let want = arrange(mode, ALL, &actions).unwrap();
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
    load_state(&bytes).unwrap();
    assert_eq!(
        lobby_start::state(),
        lobby_start::State {
            lobby: true,
            mode: Mode::Forge,
            forge_map: Some("Castle".into())
        }
    );
    assert!(load_state(&[1]).is_err());
    assert!(load_state(&[1, 9]).is_err(), "no mode 9");
}

/// The strip has its own hook on the panel update, inside
/// `lobby-options`'s, and keeps it with that feature off.
#[test]
fn the_strip_has_its_own_panel_hook_inside_lobby_options() {
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
    assert!(theirs < ours, "the strip sits inside lobby-options: {on}");
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
    })
    .unwrap();
    let ff_store = Arc::new(Store::defaults(&ff::DEFS));
    let page = Arc::new(ff::page(Arc::clone(&ff_store)));
    r.add(&KitTexts(Arc::clone(&page))).unwrap();
    r.add(&page.class()).unwrap();
    r.add(&ff::SetupRow { page }).unwrap();
    r.add(&LobbyMode {
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

/// Our actions are past retail's and every other row of ours.
#[test]
fn the_rows_actions_are_their_own() {
    for theirs in [
        OPTIONS_ACTION,
        CLANKERS,
        crate::clankermatch::SWITCH_ACTION,
        FIREFIGHT,
    ] {
        assert!(FORGE_MAP_ACTION > theirs);
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

    const POINTS: &str = "rules \"Double Points\" 1\ncarrier slayer\n\non kill(victim, killer) { if killer != none { score(killer, 2) } }\n";
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
    }

    impl Fake {
        fn new(census: Census, ir: &Ir) -> Self {
            Fake {
                census,
                blob: ir.encode(),
                prefix: prefix(ir),
                reads: AtomicU32::new(0),
                forge_maps: Mutex::new(Vec::new()),
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
        fn census(&self, _map: u32, forge_map: Option<&str>) -> Result<Census, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
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
