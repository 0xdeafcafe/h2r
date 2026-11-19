use super::*;

/// Polls every 100 ms from `from` to `to`; the presses made, as
/// `button why`.
fn presses(d: &mut Driver, from: u64, to: u64) -> Vec<String> {
    let mut out = Vec::new();
    let mut t = from;
    while t <= to {
        if let Some(p) = d.poll(t) {
            out.push(format!("{} {}", p.button, p.why));
        }
        t += 100;
    }
    out
}

/// The run's lines (20261118-232735, `/tmp/brf/bots.log`).
const TITLE: &str = "ui: screen none (slot 4: window 5 active 0x82537e90 pending 0x0 list [0xffff/0x4588b0]; slot 0: no window)";
const DIALOG_0A: &str = "ui: screen none (slot 4: window 3 active 0x82539070 pending 0x0 list [0x1/0xec700000]; slot 0: no window)";
const DIALOG_A7: &str = "ui: screen none (slot 4: window 2 active 0x82545500 pending 0x0 list [0x1/0x4588b0]; slot 0: no window)";
const DIALOG_14: &str = "ui: screen none (slot 4: window 3 active 0x82539070 pending 0x0 list [0x1/0xe1670000]; slot 0: no window)";
const MAIN_ROW_0: &str = "ui: screen none (slot 4: window 5 active 0x82539070 pending 0x0 list [0x1/0xe16d0000]; slot 0: no window)";
const MAIN_ROW_1: &str = "ui: screen none (slot 4: window 5 active 0x82539070 pending 0x0 list [0x1/0xe16e0001]; slot 0: no window)";
const MAIN_ROW_2: &str = "ui: screen none (slot 4: window 5 active 0x82539070 pending 0x0 list [0x1/0xe16f0002]; slot 0: no window)";
const LOBBY: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0x1/0x4588b0]; slot 0: no window)";
const ADVERTISED: &str = "live: session 81c63b1f2e66da94 advertised for 127.0.0.1:31000";

/// A driver at the main menu on row 0 at 1 s.
fn at_main() -> Driver {
    let mut d = Driver::new(3);
    d.line(900, MAIN_ROW_0);
    d.line(1000, "coop-live: lobby mode 4294967295");
    d.status(1000, "menu");
    d
}

#[test]
fn the_none_lines_are_read() {
    assert_eq!(none_window(TITLE), Some((5, Some(0x4588b0))));
    assert_eq!(none_window(DIALOG_0A), Some((3, Some(0xec70_0000))));
    assert_eq!(none_window(MAIN_ROW_2), Some((5, Some(0xe16f_0002))));
    assert_eq!(
        none_window("ui: screen none (slot 4: no window; slot 0: no window)"),
        None
    );
    assert_eq!(
        none_window("ui: screen 0x1e (window 5, slot 4), mode 1"),
        None
    );
    assert!(advertised(ADVERTISED));
    assert!(!advertised("coop-live: lobby mode 2"));
}

#[test]
fn a_fresh_profile_boots_with_a_presses_then_goes_to_xbox_live() {
    // The run: the title screen, then three dialogs, each taking A.
    let mut d = Driver::new(3);
    d.status(500, "menu");
    assert!(presses(&mut d, 0, 10_000).is_empty(), "no screen yet");
    d.line(10_000, TITLE);
    assert!(presses(&mut d, 10_000, 13_900).is_empty());
    assert_eq!(
        presses(&mut d, 14_000, 14_000),
        ["a the title screen: on to the main menu"]
    );
    d.line(14_600, DIALOG_0A);
    let p = presses(&mut d, 14_700, 18_700);
    assert_eq!(
        p,
        ["a a boot dialog, an unnamed dialog (window 3; the game named none): on to the main menu"]
    );
    d.line(19_000, DIALOG_A7);
    assert_eq!(presses(&mut d, 19_100, 23_000).len(), 1);
    d.line(23_500, DIALOG_14);
    assert_eq!(presses(&mut d, 23_600, 27_500).len(), 1);
    // The main menu: the window 5 line, then the lobby mode a moment after.
    d.line(30_000, MAIN_ROW_0);
    d.line(30_300, "coop-live: lobby mode 4294967295");
    let p = presses(&mut d, 30_400, 33_000);
    assert_eq!(
        p,
        [
            "down to Xbox Live (row 1, from 0)",
            "a Xbox Live (signs in)"
        ]
    );
}

#[test]
fn the_main_menu_row_is_read_off_the_list() {
    let mut d = Driver::new(3);
    d.line(900, MAIN_ROW_2);
    d.line(1000, "coop-live: lobby mode 4294967295");
    let p = presses(&mut d, 2500, 4000);
    assert_eq!(
        p,
        ["up to Xbox Live (row 1, from 2)", "a Xbox Live (signs in)"]
    );
    let mut d = Driver::new(3);
    d.line(900, MAIN_ROW_1);
    d.line(1000, "coop-live: lobby mode 4294967295");
    let p = presses(&mut d, 2500, 4000);
    assert_eq!(p, ["a Xbox Live (signs in)"]);
}

#[test]
fn the_lobby_mode_alone_is_not_the_main_menu() {
    // A dialog over it, or the title screen before it, is not.
    let mut d = Driver::new(3);
    d.line(0, DIALOG_0A);
    d.line(100, "coop-live: lobby mode 4294967295");
    let p = presses(&mut d, 2000, 4500);
    assert_eq!(
        p,
        ["a a boot dialog, an unnamed dialog (window 3; the game named none): on to the main menu"]
    );
}

#[test]
fn main_menu_to_the_lobby_to_start() {
    let mut d = at_main();
    assert_eq!(d.poll(1000), None);
    let p = presses(&mut d, 2500, 5000);
    assert_eq!(p.len(), 2, "{p:?}");
    d.line(3300, MAIN_ROW_1);
    // A dialog on the way (a game's line from before the dialog part,
    // and no gate line): A, said as unnamed.
    d.line(5000, DIALOG_14);
    let p = presses(&mut d, 5100, 8500);
    assert_eq!(
        p,
        ["a an unnamed dialog (window 3; the game named none) on the way to Xbox Live"]
    );
    // Said once, with the row its cursor is on.
    assert_eq!(
        d.take_said(),
        [
            "bots-drive: an unnamed dialog (window 3; the game named none) on the way to Xbox \
          Live (row 0): A, its first button"
        ]
    );
    // The players' screen in Xbox Live mode: A on to the Live menu.
    d.line(9100, "ui: screen 0x1e (window 5, slot 4), mode 3");
    let p = presses(&mut d, 9200, 11000);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("a the players' screen"), "{p:?}");
    // The Live menu restored on row 0: four downs (past Clankermatch and
    // Co-op Campaign) and A on Custom Game.
    d.line(11100, "ui: screen 0xba (window 5, slot 4), live menu row 0");
    let p = presses(&mut d, 11200, 20_000);
    assert_eq!(p.len(), 5, "{p:?}");
    assert!(
        p[0].starts_with("down toward Custom Game (row 4, from 0)"),
        "{p:?}"
    );
    assert!(p[4].starts_with("a Custom Game"), "{p:?}");
    // The lobby: lobby mode 2 alone is not a Live lobby.
    d.line(21_100, LOBBY);
    d.line(21_200, "coop-live: lobby mode 2");
    assert!(d.take_said().is_empty());
    d.line(22_000, ADVERTISED);
    assert!(
        d.take_said()
            .iter()
            .any(|s| s == "bots-drive: in the Live custom game lobby")
    );
    assert!(presses(&mut d, 22_100, 27_000).is_empty());
    for p in 1..=3 {
        d.line(
            27_000 + p,
            &format!(
                "bots: Bot {p} on port {p} is on Xbox Live as its own bot account, beside port 0's"
            ),
        );
    }
    let p = presses(&mut d, 27_100, 29_000);
    assert_eq!(p, ["a Start Game: 3 local bots on Xbox Live"]);
    d.status(32_000, "loading");
    assert!(d.done() && !d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("done: the game left the menus"))
    );
    assert!(presses(&mut d, 32_100, 90000).is_empty());
}

#[test]
fn split_screen_is_backed_out_of_and_said() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0x1e (window 5, slot 4), mode 1");
    let p = presses(&mut d, 5200, 9000);
    assert_eq!(p.len(), 3, "{p:?}");
    assert!(p[0].starts_with("b that was Split Screen"));
    assert!(p[2].starts_with("a Xbox Live"));
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("opened in Split Screen (mode 1), not Xbox Live"))
    );
}

#[test]
fn a_lobby_never_advertised_gives_up_saying_so() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 4");
    assert_eq!(presses(&mut d, 5200, 8000), ["a Custom Game"]);
    d.line(8100, LOBBY);
    d.line(8200, "coop-live: lobby mode 2");
    presses(&mut d, 8300, 8300 + 5 * STEP_MS);
    assert!(d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("advertised no Live session"))
    );
}

#[test]
fn a_code_prompt_stops_the_drive() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(
        5100,
        "live: Tester needs a code to sign in on http://127.0.0.1:8077; asking in game",
    );
    assert!(d.stuck());
    assert!(d.take_said().iter().any(|s| s.contains("6-digit code")));
}

#[test]
fn back_on_the_main_menu_tries_again() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    // Nothing opened: still the main menu, now on row 1.
    d.line(3300, MAIN_ROW_1);
    let p = presses(&mut d, 3400, 3300 + STEP_MS + 2000);
    assert_eq!(p, ["a Xbox Live (signs in)"]);
    assert!(d.take_said().iter().any(|s| s.contains("Xbox Live again")));
}

/// Run 20261119-001305's picker: the join gate names it, the list is on
/// row 0 (the no-account row).
const GATE_14: &str = "bots: join gate port 1: screen none (top none), dialog 0x14 (window 3, slot 4), signed_in false (slot 0x1): waits";
const GATE_NONE: &str = "bots: join gate port 1: screen none (top none), dialog none, signed_in false (slot 0x1): waits";
/// The same list one row down (the tester's account).
const DIALOG_14_ROW_1: &str = "ui: screen none (slot 4: window 3 active 0x82539070 pending 0x0 list [0x1/0xe1680001]; slot 0: no window)";

#[test]
fn the_join_gate_names_the_dialog() {
    assert_eq!(gate_dialog(GATE_14), Some(Some(0x14)));
    assert_eq!(gate_dialog(GATE_NONE), Some(None));
    assert_eq!(gate_dialog("bots: Clanker 1 on port 1"), None);
}

#[test]
fn the_account_picker_gets_down_then_a_not_a_on_no_account() {
    let mut d = at_main();
    let p = presses(&mut d, 2500, 4000);
    assert_eq!(p.last().unwrap(), "a Xbox Live (signs in)");
    // The picker over the main menu, on row 0 (run 2: A here signed out).
    d.line(4100, DIALOG_14);
    d.line(4100, GATE_14);
    let p = presses(&mut d, 4200, 7000);
    assert_eq!(
        p,
        ["down the account picker: off the no-account row (row 0) to the account"]
    );
    d.line(7100, DIALOG_14_ROW_1);
    let p = presses(&mut d, 7200, 10_000);
    assert_eq!(p, ["a the account picker's account row 1: sign in"]);
    assert!(d.take_said().iter().any(|s| s.contains("account picker")));
    // Signed in: the players' screen in Xbox Live mode.
    d.line(10_100, GATE_NONE);
    d.line(10_100, "ui: screen 0x1e (window 5, slot 4), mode 3");
    let p = presses(&mut d, 10_200, 13_000);
    assert_eq!(
        p,
        ["a the players' screen for Xbox Live: on to the Live menu"]
    );
}

#[test]
fn the_picker_at_boot_picks_the_account_too() {
    let mut d = Driver::new(3);
    d.status(500, "menu");
    d.line(1000, DIALOG_14);
    d.line(1000, GATE_14);
    let p = presses(&mut d, 1100, 3000);
    assert_eq!(
        p,
        ["down the account picker: off the no-account row (row 0) to the account"]
    );
    d.line(3000, DIALOG_14_ROW_1);
    assert_eq!(
        presses(&mut d, 3100, 6000),
        ["a the account picker's account row 1: sign in"]
    );
    // One A per opening: the same line held gets no second press.
    assert!(presses(&mut d, 6100, 20_000).is_empty());
}

#[test]
fn a_picker_whose_cursor_does_not_move_gets_a_after_one_down() {
    let mut d = at_main();
    presses(&mut d, 2500, 4000);
    d.line(4100, DIALOG_14);
    d.line(4100, GATE_14);
    let p = presses(&mut d, 4200, 9000);
    assert_eq!(
        p,
        [
            "down the account picker: off the no-account row (row 0) to the account",
            "a the account picker: A after DOWN, row unconfirmed"
        ]
    );
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("did not leave row 0"))
    );
}

#[test]
fn a_picker_that_keeps_coming_back_gives_up() {
    let mut d = at_main();
    presses(&mut d, 2500, 4000);
    let mut t = 4100;
    for _ in 0..PICKER_PICKS {
        d.line(t, DIALOG_14_ROW_1);
        d.line(t, GATE_14);
        let p = presses(&mut d, t + 100, t + 3000);
        assert_eq!(p, ["a the account picker's account row 1: sign in"]);
        // Closed, then open again.
        d.line(t + 3100, GATE_NONE);
        d.line(t + 3100, MAIN_ROW_1);
        t += 3200;
    }
    d.line(t, DIALOG_14_ROW_1);
    d.line(t, GATE_14);
    assert!(presses(&mut d, t + 100, t + 3000).is_empty());
    assert!(d.stuck());
    assert!(d.take_said().iter().any(|s| s.contains("never signed in")));
}

#[test]
fn a_picker_that_stays_up_after_a_gives_up() {
    let mut d = at_main();
    presses(&mut d, 2500, 4000);
    d.line(4100, DIALOG_14_ROW_1);
    d.line(4100, GATE_14);
    assert_eq!(presses(&mut d, 4200, 6000).len(), 1);
    assert!(presses(&mut d, 6100, 5600 + 2 * STEP_MS + 500).is_empty());
    assert!(d.stuck());
    assert!(d.take_said().iter().any(|s| s.contains("stayed up")));
}

#[test]
fn the_live_menu_cursor_left_lower_goes_up() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 5");
    let p = presses(&mut d, 5200, 12000);
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(
        p[0].starts_with("up toward Custom Game (row 4, from 5)"),
        "{p:?}"
    );
}

#[test]
fn the_press_line_names_the_screen_and_rows() {
    let mut d = at_main();
    let p = d.poll(2500).unwrap();
    assert_eq!(
        d.press_line(&p),
        "bots-drive: press down on screen none main menu row 0 (to Xbox Live (row 1, from 0))"
    );
    presses(&mut d, 2600, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 0");
    let p = d.poll(6700).unwrap();
    assert_eq!(
        d.press_line(&p),
        "bots-drive: press down on screen 0xba row 0 (toward Custom Game (row 4, from 0))"
    );
}

#[test]
fn the_lobby_starts_anyway_after_its_wait() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 4");
    presses(&mut d, 5200, 8000);
    d.line(8050, "coop-live: lobby mode 2");
    d.line(8100, ADVERTISED);
    d.line(
        9000,
        "bots: Bot 1 on port 1 is on Xbox Live as its own bot account, beside port 0's",
    );
    assert!(presses(&mut d, 9000, 8100 + LOBBY_WAIT_MS).is_empty());
    let p = presses(&mut d, 8200 + LOBBY_WAIT_MS, 8300 + LOBBY_WAIT_MS);
    assert_eq!(p, ["a Start Game: not every bot came"]);
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("only 1 of 3 local bots"))
    );
}

#[test]
fn start_again_then_stuck() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 4");
    presses(&mut d, 5200, 8000);
    d.line(8050, "coop-live: lobby mode 2");
    d.line(8100, ADVERTISED);
    for p in 1..=3 {
        d.line(
            8200,
            &format!("bots: B{p} on port {p} is on Xbox Live as guest 1 of port 0's account"),
        );
    }
    assert_eq!(presses(&mut d, 9700, 10000).len(), 1);
    let p = presses(&mut d, 10000, 10000 + START_MS + 2000);
    assert_eq!(p, ["up to Start Game", "a Start Game again"]);
    presses(&mut d, 12000 + START_MS, 20000 + 4 * START_MS);
    assert!(d.stuck());
}

#[test]
fn no_main_menu_gives_up_after_its_presses() {
    let mut d = Driver::new(3);
    d.line(0, TITLE);
    let p = presses(&mut d, 0, 200_000);
    assert_eq!(p.len(), BOOT_PRESSES as usize, "{p:?}");
    assert!(d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("no main menu after 12 presses"))
    );
}

/// Run 20261119-003044's Xbox Live menu: a window 5 `none` screen with
/// list mask 0xf, under the main menu's lobby mode still.
const LIVE_ROW_0: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0xf/0xe2780000]; slot 0: no window)";
const LIVE_ROW_1: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0xf/0xe2790001]; slot 0: no window)";
const LIVE_ROW_2: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0xf/0xe27a0002]; slot 0: no window)";
/// Row 3 is Co-op Campaign's; Custom Game's row 4 sits below it.
const LIVE_ROW_3: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0xf/0xe27b0003]; slot 0: no window)";
const LIVE_ROW_4: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0xf/0xe27c0004]; slot 0: no window)";
const LIVE_NO_ROW: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0xf/0x4588b0]; slot 0: no window)";
/// What A on its row 1 (OptiMatch) opened: mask 0x1 like the main menu.
const OPTIMATCH: &str = "ui: screen none (slot 4: window 5 active 0x82540730 pending 0x0 list [0x1/0xec700000]; slot 0: no window)";
/// The main menu coming in under the title screen (pending), as run 3.
const TITLE_TO_MAIN: &str = "ui: screen none (slot 4: window 5 active 0x82537e90 pending 0x82539070 list [0xffff/0x4588b0]; slot 0: no window)";

/// A driver on the players' screen in Xbox Live mode, A sent, at 4 s.
fn to_live_menu() -> Driver {
    let mut d = at_main();
    presses(&mut d, 2500, 4000);
    d.line(4100, "ui: screen 0x1e (window 5, slot 4), mode 3");
    assert_eq!(
        presses(&mut d, 4200, 6000),
        ["a the players' screen for Xbox Live: on to the Live menu"]
    );
    d
}

#[test]
fn the_none_line_parts_are_read() {
    let n = none_line(LIVE_ROW_1).unwrap();
    assert_eq!(n.window, 5);
    assert_eq!(n.active, Some(0x8253_9a80));
    assert_eq!(n.pending, Some(0));
    assert_eq!(n.mask, Some(0xf));
    assert_eq!(n.value, Some(0xe279_0001));
    assert_eq!(none_line(TITLE_TO_MAIN).unwrap().pending, Some(0x8253_9070));
    assert!(deleted("live: session 84098833205b4d5e deleted"));
    assert!(!deleted(ADVERTISED));
}

#[test]
fn the_live_menu_is_not_the_main_menu() {
    // Run 3: the Live menu came under lobby mode -1 and took DOWN, A as
    // "main menu row 1", which opened OptiMatch.
    let mut d = to_live_menu();
    d.line(6100, LIVE_ROW_0);
    let p = presses(&mut d, 6200, 7600);
    assert_eq!(p, ["down toward Custom Game (row 4, from 0)"]);
    d.line(7700, LIVE_ROW_1);
    assert_eq!(
        presses(&mut d, 7800, 9200),
        ["down toward Custom Game (row 4, from 1)"]
    );
    d.line(9300, LIVE_ROW_2);
    assert_eq!(
        presses(&mut d, 9400, 10_800),
        ["down toward Custom Game (row 4, from 2)"]
    );
    d.line(10_900, LIVE_ROW_3);
    assert_eq!(
        presses(&mut d, 11_000, 12_400),
        ["down toward Custom Game (row 4, from 3)"]
    );
    d.line(12_500, LIVE_ROW_4);
    let p = d.poll(14_000).unwrap();
    assert_eq!(
        d.press_line(&p),
        "bots-drive: press a on screen none row 4 (Custom Game)"
    );
}

#[test]
fn optimatch_under_the_main_menus_mode_is_not_the_main_menu() {
    let mut d = Driver::new(3);
    d.line(900, TITLE_TO_MAIN);
    d.line(950, "coop-live: lobby mode 4294967295");
    d.line(1000, MAIN_ROW_0);
    assert!(d.at_main());
    d.line(5000, LIVE_ROW_1);
    assert!(!d.at_main() && d.live_menu());
    d.line(6000, OPTIMATCH);
    assert!(!d.at_main() && !d.live_menu());
    assert_eq!(d.main_row(), None);
}

#[test]
fn the_live_menu_with_no_row_yet_gets_down_to_show_one() {
    let mut d = to_live_menu();
    d.line(6100, LIVE_NO_ROW);
    assert_eq!(
        presses(&mut d, 6200, 7600),
        ["down the Xbox Live menu shows no row yet: DOWN"]
    );
    d.line(7700, LIVE_ROW_4);
    assert_eq!(presses(&mut d, 7800, 9200), ["a Custom Game"]);
}

#[test]
fn an_optimatch_lobby_advertising_is_not_hosting() {
    // Run 3 again: lobby mode 3 and a Live session advertised; not the
    // custom game lobby, so B back to the Live menu and Custom Game again.
    let mut d = to_live_menu();
    d.line(6100, LIVE_ROW_4);
    assert_eq!(presses(&mut d, 6200, 7600), ["a Custom Game"]);
    d.line(7700, OPTIMATCH);
    d.line(7800, "coop-live: lobby mode 3");
    d.line(7900, ADVERTISED);
    assert!(
        !d.take_said()
            .iter()
            .any(|s| s.contains("in the Live custom game lobby"))
    );
    let p = presses(&mut d, 8000, 7600 + STEP_MS + 500);
    assert_eq!(p, ["b a lobby in mode 3, not Custom Game's: back"]);
    d.line(23_000, "live: session 0123456789abcdef deleted");
    d.line(23_000, LIVE_ROW_4);
    let p = presses(&mut d, 23_100, 23_000 + STEP_MS + 2000);
    assert_eq!(p, ["a Custom Game"]);
    // This time the custom game lobby.
    d.line(40_000, LOBBY);
    d.line(40_100, "coop-live: lobby mode 2");
    d.line(40_200, ADVERTISED);
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("in the Live custom game lobby"))
    );
}

/// Run 20261119-013507: with the local bots joining in the lobby, not on
/// the players' screen, the Live menu's list mask is `0x1` (the one
/// signed-in player's), as the main menu's. It is learnt as the screen
/// that settles after A on the players' screen, and driven to Custom Game.
const SOLO_LIVE_ROW_0: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0x1/0xe2780000]; slot 0: no window)";
const SOLO_LIVE_ROW_1: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0x1/0xe2790001]; slot 0: no window)";
const SOLO_LIVE_ROW_2: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0x1/0xe27a0002]; slot 0: no window)";
const SOLO_LIVE_ROW_3: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0x1/0xe27b0003]; slot 0: no window)";
const SOLO_LIVE_ROW_4: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0x1/0xe27c0004]; slot 0: no window)";

#[test]
fn the_live_menu_alone_is_learnt_after_the_players_screen() {
    let mut d = to_live_menu();
    d.line(6100, SOLO_LIVE_ROW_0);
    assert!(d.live_menu() && !d.at_main());
    assert_eq!(
        presses(&mut d, 6200, 7600),
        ["down toward Custom Game (row 4, from 0)"]
    );
    d.line(7700, SOLO_LIVE_ROW_1);
    assert_eq!(
        presses(&mut d, 7800, 9200),
        ["down toward Custom Game (row 4, from 1)"]
    );
    d.line(9300, SOLO_LIVE_ROW_2);
    assert_eq!(
        presses(&mut d, 9400, 10_800),
        ["down toward Custom Game (row 4, from 2)"]
    );
    d.line(10_900, SOLO_LIVE_ROW_3);
    assert_eq!(
        presses(&mut d, 11_000, 12_400),
        ["down toward Custom Game (row 4, from 3)"]
    );
    d.line(12_500, SOLO_LIVE_ROW_4);
    assert_eq!(presses(&mut d, 12_600, 14_000), ["a Custom Game"]);
    // The lobby on the same address, in Custom Game's lobby mode, is not
    // the Live menu.
    d.line(14_100, "coop-live: lobby mode 2");
    d.line(14_200, LOBBY);
    assert!(!d.live_menu());
}

#[test]
fn the_main_menu_alone_is_not_the_live_menu() {
    let d = at_main();
    assert!(d.at_main() && !d.live_menu());
}

/// Run 20261119-115144 (`h2 bots test --clankermatch`): no local bot is
/// placed before the roster, so no join gate line ever names the picker;
/// the game's own line does now.
const CM_PICKER: &str = "ui: screen none (slot 4: window 3 active 0x8253b0b0 pending 0x0 list [0x1/0xe1670000]; slot 0: no window), dialog 0x14 dialog_live_signin (window 3, slot 4)";
const CM_PICKER_ROW_1: &str = "ui: screen none (slot 4: window 3 active 0x8253b0b0 pending 0x0 list [0x1/0xe1680001]; slot 0: no window), dialog 0x14 dialog_live_signin (window 3, slot 4)";
const CM_BOOT_0A: &str = "ui: screen none (slot 4: window 3 active 0x82539070 pending 0x0 list [0x1/0xec700000]; slot 0: no window), dialog 0x0a (window 3, slot 4)";
const CM_CONFIRM: &str = "ui: screen none (slot 4: window 3 active 0x8253c000 pending 0x0 list [0x1/0xe1700000]; slot 0: no window), dialog 0x21 confirmation_dialog (window 3, slot 4)";

#[test]
fn the_dialog_part_of_a_none_line_is_read() {
    assert_eq!(
        line_dialog(CM_PICKER),
        Some((
            "dialog 0x14 dialog_live_signin (window 3, slot 4)",
            Some(0x14)
        ))
    );
    assert_eq!(
        line_dialog(CM_BOOT_0A),
        Some(("dialog 0x0a (window 3, slot 4)", Some(0x0a)))
    );
    assert_eq!(
        line_dialog(
            "ui: screen none (slot 4: no window; slot 0: no window), dialog ? (window 0, slot 4)"
        ),
        Some(("dialog ? (window 0, slot 4)", None))
    );
    assert_eq!(line_dialog(DIALOG_14), None, "a line from before the part");
    assert_eq!(line_dialog("ui: screen 0xb6 (window 1, slot 4)"), None);
    // The part leaves the slot 4 parts as they were.
    let n = none_line(CM_PICKER).unwrap();
    assert_eq!((n.window, n.mask, n.value), (3, Some(1), Some(0xe167_0000)));
    assert_eq!(n.active, Some(0x8253_b0b0));
}

#[test]
fn the_picker_is_known_by_the_games_line_without_a_gate_line() {
    // Boot: the new profile's dialog 0x0a (named in the press), then the
    // picker: DOWN and A on the account, not A on its no-account row.
    let mut d = Driver::new(3);
    d.status(500, "menu");
    d.line(1000, CM_BOOT_0A);
    assert_eq!(
        presses(&mut d, 1100, 5100),
        ["a a boot dialog, dialog 0x0a (window 3, slot 4): on to the main menu"]
    );
    d.line(6000, CM_PICKER);
    assert_eq!(
        presses(&mut d, 6100, 8000),
        ["down the account picker: off the no-account row (row 0) to the account"]
    );
    d.line(8100, CM_PICKER_ROW_1);
    assert_eq!(
        presses(&mut d, 8200, 10_000),
        ["a the account picker's account row 1: sign in"]
    );
    // From the main menu's Xbox Live row too (the run's four tries).
    let mut d = at_main();
    assert_eq!(
        presses(&mut d, 2500, 4000).last().unwrap(),
        "a Xbox Live (signs in)"
    );
    d.line(4100, CM_PICKER);
    assert_eq!(
        presses(&mut d, 4200, 7000),
        ["down the account picker: off the no-account row (row 0) to the account"]
    );
    // Closed: the main menu's line clears the dialog.
    d.line(7100, MAIN_ROW_1);
    assert!(!d.picker());
}

#[test]
fn a_dialog_on_the_way_is_named_in_its_press_and_the_give_up() {
    let mut d = at_main();
    presses(&mut d, 2500, 4000);
    d.line(4100, CM_CONFIRM);
    let p = presses(&mut d, 4200, 7500);
    assert_eq!(
        p,
        ["a dialog 0x21 confirmation_dialog (window 3, slot 4) on the way to Xbox Live"]
    );
    assert!(d.take_said().iter().any(|s| s
        == "bots-drive: dialog 0x21 confirmation_dialog (window 3, slot 4) on the way to \
            Xbox Live (row 0): A, its first button"));
    // Back on the main menu each time, until the tries run out: the
    // give-up names the dialogs it answered.
    let mut t = 7600;
    while !d.stuck() && t < 200_000 {
        d.line(t, MAIN_ROW_1);
        presses(&mut d, t + 100, t + STEP_MS + 2000);
        t += STEP_MS + 2100;
        if d.stuck() {
            break;
        }
        d.line(t, CM_CONFIRM);
        presses(&mut d, t + 100, t + 3500);
        t += 3600;
    }
    assert!(d.stuck());
    let said = d.take_said();
    assert!(
        said.iter().any(|s| s.contains("no Xbox Live menu after")
            && s.contains("the dialogs on the way: dialog 0x21 confirmation_dialog")),
        "{said:?}"
    );
}
