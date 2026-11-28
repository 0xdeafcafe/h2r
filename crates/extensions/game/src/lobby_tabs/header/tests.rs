use super::*;
use crate::lobby_tabs::Reason;
use guest_layout::testing::FakeGuest;

const SCREEN: u32 = 0x1000;
const AREA: u32 = 0x10000;
const HEADER_AT: u32 = AREA;
const FOOTER_AT: u32 = AREA + 0x100;
const BITMAP_AT: u32 = AREA + 0x300;
const PROMPT_AT: u32 = AREA + 0x400;

/// Split Screen, no kit map, no Live: MULTIPLAYER off, every other mode on.
fn split_screen(leader: bool, selected: Tab) -> Shown {
    Shown {
        place: Place::SplitScreen,
        leader,
        selected,
        row: crate::lobby_tabs::row(Present::ALL, Place::SplitScreen, Facts::default()),
        clanker: None,
    }
}

fn text(g: &mut FakeGuest, at: u32, index: u16, next: u32) {
    g.put_u32(at, TEXT_VTABLE)
        .put_u32(at + WIDGET_KIND, TEXT_KIND)
        .put_u32(at + WIDGET_NEXT, next);
    g.set_u16(at + WIDGET_INDEX, index).unwrap();
    g.put_u8(at + WIDGET_VISIBLE, 1);
}

/// A lobby screen on `pane`: retail's header, footer and a bitmap, then,
/// with `prompt`, the prompt line's block.
fn lobby(pane: u16, prompt: bool) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x2000).map(AREA, 0x10000);
    g.set_u16(SCREEN + SCREEN_PANE, pane).unwrap();
    g.put_u32(SCREEN + WIDGET_CHILD, HEADER_AT);
    text(&mut g, HEADER_AT, 0, FOOTER_AT);
    text(&mut g, FOOTER_AT, 1, BITMAP_AT);
    g.put_u32(BITMAP_AT + WIDGET_KIND, 8)
        .put_u32(BITMAP_AT + WIDGET_NEXT, if prompt { PROMPT_AT } else { 0 });
    if prompt {
        text(&mut g, PROMPT_AT, PROMPT_BLOCK, 0);
    }
    g
}

fn visible(g: &FakeGuest, at: u32) -> bool {
    g.u8(at + WIDGET_VISIBLE).unwrap() != 0
}

/// The header is the first text widget; the prompt line is block 48's, on
/// pane 0 only; a lobby without the tag edit has none.
#[test]
fn the_header_and_the_prompt_line_are_found_by_their_place_and_index() {
    let f = find(&lobby(0, true), SCREEN).unwrap();
    assert_eq!(f.header, Some(HEADER_AT));
    assert_eq!(f.prompt, Some(PROMPT_AT));
    let bare = find(&lobby(0, false), SCREEN).unwrap();
    assert_eq!(bare.header, Some(HEADER_AT));
    assert_eq!(bare.prompt, None, "no tag edit, no prompt line");
    let other_pane = find(&lobby(1, true), SCREEN).unwrap();
    assert_eq!(other_pane.prompt, None, "pane 0's block only");
}

/// The prompt shows for the leader while no lock holds the lobby.
#[test]
fn the_prompt_line_is_the_leaders_and_hides_while_locked() {
    assert!(split_screen(true, Tab::CustomGames).prompt_visible(false));
    assert!(!split_screen(true, Tab::CustomGames).prompt_visible(true), "counting down");
    assert!(!split_screen(false, Tab::CustomGames).prompt_visible(false), "a member's");
}

/// A lobby as the guest leaves it: `networking`, and a squad session in
/// the pregame stage (`+0x4980` 1, as `0x5a220` leaves every lobby) with
/// network class `class` at `+0x18`.
fn lobby_at(networking: bool, class: u32) -> FakeGuest {
    use bots::phantom::session::at;
    const SQUAD: u32 = 0x10_0000;
    let mut g = FakeGuest::new();
    g.map(lobby::NETWORKING, 0x1000)
        .map(SQUAD, 0x8000)
        .put_u8(lobby::NETWORKING, u8::from(networking))
        .put_u32(lobby::SESSION, SQUAD)
        .put_u32(SQUAD + at::STATE, 5)
        .put_u32(SQUAD + at::CLASS, 1)
        .put_u32(SQUAD + crate::variant_ext::SESSION_NETWORK_CLASS, class);
    g
}

#[test]
fn the_place_is_the_squad_sessions_network_class_not_its_stage() {
    // A Live party lobby: the game's Live paths create the squad with
    // `0x59890(2)`; our Live HLE leaves session creation to the guest.
    // Its stage byte is 1 like any lobby's, and must not read as System
    // Link (CO-OP refused).
    assert_eq!(place_now(&lobby_at(true, 2)), Some(Place::XboxLive));
    assert_eq!(place_now(&lobby_at(true, 1)), Some(Place::SystemLink));
    assert_eq!(place_now(&lobby_at(false, 0)), Some(Place::SplitScreen));
    assert_eq!(place_now(&lobby_at(true, 0)), None, "an offline class while networked");
    // And so CO-OP on a Live party is on, not refused for System Link.
    let facts = Facts {
        live_co_op: true,
        ..Facts::default()
    };
    let at = |class| place_now(&lobby_at(true, class)).unwrap();
    assert_eq!(crate::lobby_tabs::available(Tab::CoOp, at(2), facts), Avail::On);
    assert_eq!(
        crate::lobby_tabs::available(Tab::CoOp, at(1), facts),
        Avail::Off(Reason::CoOpNotOnSystemLink)
    );
}

#[test]
fn the_place_and_the_retail_mode_come_from_the_session() {
    assert_eq!(place(false, None), Some(Place::SplitScreen));
    assert_eq!(place(false, Some(2)), Some(Place::SplitScreen));
    assert_eq!(place(true, Some(2)), Some(Place::XboxLive));
    assert_eq!(place(true, Some(1)), Some(Place::SystemLink));
    assert_eq!(place(true, None), None, "a networked lobby with no session");
    assert_eq!(retail_mode(false, 0, None, false), RetailMode::Custom);
    assert_eq!(retail_mode(true, 6, None, false), RetailMode::Matchmaking);
    assert_eq!(retail_mode(true, 7, None, true), RetailMode::Matchmaking);
    assert_eq!(retail_mode(true, 1, None, true), RetailMode::CoOp);
    // A matchmaking lobby before its search: the squad session's mode 2.
    assert_eq!(
        retail_mode(true, 1, Some(SQUAD_MATCHMAKING), false),
        RetailMode::Matchmaking
    );
    assert_eq!(retail_mode(true, 1, Some(1), false), RetailMode::Custom);
    assert_eq!(
        retail_mode(false, 6, Some(SQUAD_MATCHMAKING), false),
        RetailMode::Custom,
        "not networked"
    );
}

/// The squad session's mode is read only while networking is on and the
/// session is in states 3 to 8, as `0x64160` reads it.
#[test]
fn the_squad_mode_is_read_in_a_lobby_state_only() {
    const S: u32 = 0x0010_0000;
    let mut g = FakeGuest::new();
    g.map(0x0052_7000, 0x1000).map(S, 0x8000);
    g.put_u8(lobby::NETWORKING, 1)
        .put_u32(lobby::SESSION, S)
        .put_u32(S + lobby::STATE, 5)
        .put_u32(S + SQUAD_MODE, SQUAD_MATCHMAKING);
    assert_eq!(squad_mode(&g), Some(SQUAD_MATCHMAKING));
    g.put_u32(S + lobby::STATE, 9);
    assert_eq!(squad_mode(&g), None, "not a lobby state");
    g.put_u32(S + lobby::STATE, 5).put_u8(lobby::NETWORKING, 0);
    assert_eq!(squad_mode(&g), None, "Split Screen");
}

#[test]
fn a_member_reads_its_mode_from_the_lobby() {
    let reading = Reading {
        place: Place::SystemLink,
        leader: false,
        retail: RetailMode::Custom,
        clanker: None,
        facts: Facts::default(),
    };
    let s = shown(&reading, Present::ALL, Mode::Multiplayer);
    assert_eq!(s.selected, Tab::CustomGames);
    let leader = Reading {
        leader: true,
        ..reading
    };
    assert_eq!(
        shown(&leader, Present::ALL, Mode::Forge).selected,
        Tab::Forge
    );
}

/// MULTIPLAYER is one mode for both kinds of playlist: the lobby's playlist
/// says which, and only MULTIPLAYER reports it.
#[test]
fn multiplayer_reports_whether_its_playlist_is_a_clanker_one() {
    let live = Reading {
        place: Place::XboxLive,
        leader: true,
        retail: RetailMode::Matchmaking,
        clanker: Some(true),
        facts: Facts {
            playlists: true,
            ..Facts::default()
        },
    };
    let s = shown(&live, Present::ALL, Mode::Multiplayer);
    assert_eq!(s.selected, Tab::Multiplayer);
    assert_eq!(s.clanker, Some(true));
    assert_eq!(json_of(&s, None)["clanker"], true);
    let ordinary = Reading {
        clanker: Some(false),
        ..live
    };
    assert_eq!(
        shown(&ordinary, Present::ALL, Mode::Forge).selected,
        Tab::Multiplayer
    );
    assert_eq!(
        json_of(&shown(&ordinary, Present::ALL, Mode::Multiplayer), None)["clanker"],
        false
    );
    let custom = Reading {
        retail: RetailMode::Custom,
        ..live
    };
    assert_eq!(
        shown(&custom, Present::ALL, Mode::Multiplayer).clanker,
        None
    );
}

#[test]
fn ui_lobby_tabs_answers_each_mode_with_its_state_and_reason() {
    let s = split_screen(true, Tab::CustomGames);
    let v = json_of(&s, Some(Reason::MatchmakingNeedsLive));
    assert_eq!(v["place"], "split_screen");
    assert_eq!(v["leader"], true);
    assert_eq!(v["selected"], "custom_games");
    assert_eq!(v["header"], "CUSTOM GAMES", "the header names the mode");
    assert_eq!(v["notice"], "Matchmaking needs Xbox Live.");
    let tabs = v["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 4);
    assert_eq!(tabs[0]["tab"], "multiplayer");
    assert_eq!(tabs[0]["label"], "MULTIPLAYER");
    assert_eq!(tabs[0]["state"], "off");
    assert_eq!(tabs[0]["reason"], "Matchmaking needs Xbox Live.");
    assert_eq!(tabs[1]["state"], "selected");
    assert_eq!(tabs[1]["label"], "CUSTOM GAMES");
    assert!(tabs[1]["reason"].is_null());
    assert_eq!(tabs[2]["state"], "on");
    assert_eq!(tabs[3]["tab"], "co_op");
    assert_eq!(tabs[3]["state"], "on", "Split Screen's co-op is retail's");
    assert!(v["clanker"].is_null(), "not on MULTIPLAYER");
    assert!(json_of(&s, None)["notice"].is_null());
}

/// The party lobby's own panel hook runs with no other feature on the
/// point (`lobby-options=off`): retail's update, then the header set to
/// the mode and the prompt line's text, its lock read, and it shown for
/// the leader; retail's answer returned. Nothing is added to the pane:
/// there is no strip, and the header stays visible.
#[test]
fn the_header_names_the_mode_after_retails_panel_update() {
    use guest_layout::entry::{Regs, testing};
    install();
    let mut g = lobby(0, true);
    // Split Screen: not networked, a custom game's session type.
    g.map(0x0052_7000, 0x1000);
    g.put_u8(lobby::NETWORKING, 0)
        .put_u32(lobby::SESSION_TYPE, 0)
        .put_u32(lobby::SESSION, 0);
    let entry = |cx: Cx| -> EntryFuture { Box::pin(panel_entry(cx, SCREEN, 3, None)) };
    let run = testing::run(&entry, g, Regs::default(), |call, _| {
        let eax = if call.target == points::LOBBY_VARIANT_PANEL.base { 7 } else { 1 };
        Regs {
            eax,
            ..Regs::default()
        }
    });
    assert_eq!(run.result.unwrap(), Ret::new(7, 4), "retail's answer");
    let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
    assert_eq!(
        targets,
        [
            points::LOBBY_VARIANT_PANEL.base,
            points::TEXT_SET.base,
            points::TEXT_SET.base,
            points::LOBBY_START_STATE.base,
        ],
        "retail's update first; no PANE_TEXT_ADD"
    );
    assert_eq!(run.calls[0].eax, Some(SCREEN));
    assert_eq!(run.calls[0].args, [3]);
    assert_eq!(run.calls[1].edi, Some(HEADER_AT));
    assert_eq!(
        run.calls[1].args,
        [Tab::CustomGames.id()],
        "the header names the lobby's mode"
    );
    assert_eq!(run.calls[2].edi, Some(PROMPT_AT));
    assert_eq!(run.calls[2].args, [crate::lobby_tabs::CHANGE_MODE_ID]);
    assert!(visible(&run.guest, HEADER_AT), "the header stays");
    assert!(visible(&run.guest, PROMPT_AT), "Split Screen's player leads");
}

// ---- against the disc ----

fn disc() -> crate::ui_kit::preview::Disc {
    let iso = std::path::PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    crate::ui_kit::preview::Disc::open(&iso).unwrap()
}

/// Every mode's label fits the lobby's header rectangle in its own font.
#[test]
#[ignore = "reads the disc named by H2_DISC"]
fn every_mode_label_fits_the_header() {
    let d = disc();
    let lobby = d
        .screen(crate::mods_screen::page_layout::LOBBY_SCREEN_ID as i16)
        .expect("pregame_lobby");
    let header = lobby.panes[0]
        .text_blocks
        .iter()
        .find(|b| [b.rect.top, b.rect.left, b.rect.bottom, b.rect.right] == HEADER)
        .unwrap_or_else(|| {
            let rects: Vec<_> = lobby.panes[0]
                .text_blocks
                .iter()
                .map(|b| (b.font, [b.rect.top, b.rect.left, b.rect.bottom, b.rect.right]))
                .collect();
            panic!("the lobby's header block {HEADER:?} is not among {rects:?}")
        });
    let room = i32::from(HEADER[3] - HEADER[1]);
    for t in Tab::ALL {
        let w = d.fonts.width(header.font, t.label());
        assert!(w <= room, "{}: {w} of {room}", t.label());
    }
}

/// The game line names the mode and the variant, never the record's base
/// game type, and a kit package by its map.
#[test]
fn a_game_with_its_own_mode_is_named_by_it_not_by_the_base_game_type() {
    use variants::ext::Mode as Script;
    let kit = "Firefight \u{2013} Lockout";
    let line = |lobby, script, siege, record: &str, waves| {
        own_mode(lobby, script, siege, record, waves).map(|(mode, variant)| title(mode, &variant, kit))
    };
    // A Firefight variant picked in Choose Rules.
    assert_eq!(
        line(Mode::Multiplayer, Script::Firefight, false, "Holdout", None).as_deref(),
        Some("Firefight: Holdout on Lockout")
    );
    // A Firefight lobby plays the options page's set on the Slayer record.
    assert_eq!(
        line(Mode::Firefight, Script::None, false, "Slayer", Some("Covenant")).as_deref(),
        Some("Firefight: Covenant on Lockout")
    );
    assert_eq!(line(Mode::Firefight, Script::None, false, "Slayer", None).as_deref(), Some("Firefight on Lockout"));
    // Siege's variant has no other name; one saved under a name has.
    assert_eq!(line(Mode::Multiplayer, Script::Rules, true, "Siege", None).as_deref(), Some("Siege on Lockout"));
    assert_eq!(
        line(Mode::Multiplayer, Script::Rules, true, "Long Siege", None).as_deref(),
        Some("Siege: Long Siege on Lockout")
    );
    // Any other rules script is its variant's name.
    assert_eq!(
        line(Mode::Multiplayer, Script::Rules, false, "Bounce Practice", None).as_deref(),
        Some("Bounce Practice on Lockout")
    );
    // A record that plays as its own game type keeps retail's line.
    assert_eq!(line(Mode::Multiplayer, Script::None, false, "Team Slayer", None), None);
    assert_eq!(line(Mode::Forge, Script::Unknown(9), false, "Slayer", None), None);
    for script in [Script::Firefight, Script::Rules] {
        for (siege, record) in [(false, "Holdout"), (true, "Siege")] {
            let said = line(Mode::Multiplayer, script, siege, record, None).unwrap();
            assert!(!said.contains("Slayer") && !said.contains('\u{2013}'), "{said}");
        }
    }
    assert_eq!(title(Some("Firefight"), " Holdout ", "Lockout"), "Firefight: Holdout on Lockout");
    assert_eq!(title(None, "", "Lockout"), "Lockout");
}

/// Retail's game line ends with the map slot's name and says more.
#[test]
fn retails_game_line_is_told_by_the_map_slots_name() {
    let kit = "Firefight \u{2013} Lockout";
    assert!(is_game_line("Slayer on Firefight \u{2013} Lockout", kit));
    assert!(!is_game_line(kit, kit), "the map's name alone is another widget");
    assert!(!is_game_line("Kill the other players.", kit));
    assert!(!is_game_line("Slayer on Lockout", ""));
    // A game with no mode of its own on a kit package names the map plainly.
    assert_eq!(on_plain_map("Team Slayer on Firefight \u{2013} Lockout", kit).as_deref(), Some("Team Slayer on Lockout"));
    assert_eq!(on_plain_map("Team Slayer on Lockout", "Lockout"), None, "no kit package, retail's line");
}

/// The two texts retail's panel update writes are the fourth and fifth
/// text children, counted as its lookup counts them.
#[test]
fn the_game_lines_are_found_by_their_place_among_the_texts() {
    let mut g = lobby(0, false);
    let at = |n: u32| AREA + 0x1000 + n * 0x100;
    g.put_u32(BITMAP_AT + WIDGET_NEXT, at(0));
    for n in 0..4 {
        text(&mut g, at(n), 2 + n as u16, if n == 3 { 0 } else { at(n + 1) });
    }
    // Texts 0 and 1 are the header and the footer; 2 is at(0).
    let f = find(&g, SCREEN).unwrap();
    assert_eq!(f.lines, [Some(at(1)), Some(at(2))]);
    assert_eq!(find(&lobby(0, true), SCREEN).unwrap().lines, [None, None]);
}
