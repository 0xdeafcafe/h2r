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
        ["a a boot dialog (the new profile's): on to the main menu"]
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
            "a Xbox Live (signs in; the bots join here)"
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
        [
            "up to Xbox Live (row 1, from 2)",
            "a Xbox Live (signs in; the bots join here)"
        ]
    );
    let mut d = Driver::new(3);
    d.line(900, MAIN_ROW_1);
    d.line(1000, "coop-live: lobby mode 4294967295");
    let p = presses(&mut d, 2500, 4000);
    assert_eq!(p, ["a Xbox Live (signs in; the bots join here)"]);
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
        ["a a boot dialog (the new profile's): on to the main menu"]
    );
}

#[test]
fn main_menu_to_the_lobby_to_start() {
    let mut d = at_main();
    assert_eq!(d.poll(1000), None);
    let p = presses(&mut d, 2500, 5000);
    assert_eq!(p.len(), 2, "{p:?}");
    d.line(3300, MAIN_ROW_1);
    // A sign-in dialog on the way: A.
    d.line(5000, DIALOG_14);
    let p = presses(&mut d, 5100, 8500);
    assert_eq!(p, ["a a dialog on the way to Xbox Live"]);
    // The players' screen in Xbox Live mode: A on to the Live menu.
    d.line(9100, "ui: screen 0x1e (window 5, slot 4), mode 3");
    let p = presses(&mut d, 9200, 11000);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("a the players' screen"), "{p:?}");
    // The Live menu restored on row 0: two downs and A on Custom Game.
    d.line(11100, "ui: screen 0xba (window 5, slot 4), live menu row 0");
    let p = presses(&mut d, 11200, 16000);
    assert_eq!(p.len(), 3, "{p:?}");
    assert!(
        p[0].starts_with("down toward Custom Game (row 2, from 0)"),
        "{p:?}"
    );
    assert!(p[2].starts_with("a Custom Game"), "{p:?}");
    // The lobby: lobby mode 2 alone is not a Live lobby.
    d.line(16100, LOBBY);
    d.line(16200, "coop-live: lobby mode 2");
    assert!(d.take_said().is_empty());
    d.line(17000, ADVERTISED);
    assert!(
        d.take_said()
            .iter()
            .any(|s| s == "bots-drive: in the Live custom game lobby")
    );
    assert!(presses(&mut d, 17100, 24000).is_empty());
    for p in 1..=3 {
        d.line(
            24000 + p,
            &format!(
                "bots: Bot {p} on port {p} is on Xbox Live as its own bot account, beside port 0's"
            ),
        );
    }
    let p = presses(&mut d, 24100, 26000);
    assert_eq!(p, ["a Start Game: 3 local bots on Xbox Live"]);
    d.status(29000, "loading");
    assert!(d.done() && !d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("done: the game left the menus"))
    );
    assert!(presses(&mut d, 29100, 90000).is_empty());
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
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 2");
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
    assert_eq!(p, ["a Xbox Live (signs in; the bots join here)"]);
    assert!(d.take_said().iter().any(|s| s.contains("Xbox Live again")));
}

#[test]
fn the_live_menu_cursor_left_lower_goes_up() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 4");
    let p = presses(&mut d, 5200, 12000);
    assert_eq!(p.len(), 3, "{p:?}");
    assert!(
        p[0].starts_with("up toward Custom Game (row 2, from 4)"),
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
        "bots-drive: press down on screen 0xba row 0 (toward Custom Game (row 2, from 0))"
    );
}

#[test]
fn the_lobby_starts_anyway_after_its_wait() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 2");
    presses(&mut d, 5200, 8000);
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
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 2");
    presses(&mut d, 5200, 8000);
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
