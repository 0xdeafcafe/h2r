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

/// A driver whose game has reached the main menu at 1 s (no screen, the
/// `coop-live: lobby mode` line).
fn at_main() -> Driver {
    let mut d = Driver::new(3);
    d.line(900, "ui: screen none");
    d.line(1000, "coop-live: lobby mode 4294967295");
    d.status(1000, "menu");
    d
}

#[test]
fn main_menu_to_the_lobby_to_start() {
    let mut d = at_main();
    // Nothing before the screen has settled.
    assert_eq!(d.poll(1000), None);
    let p = presses(&mut d, 2500, 5000);
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(
        p[0].starts_with("down") && p[1].starts_with("a Xbox Live"),
        "{p:?}"
    );
    // The players' screen in Xbox Live mode: A on to the Live menu.
    d.line(5100, "ui: screen 0x1e (window 5, slot 4), mode 3");
    let p = presses(&mut d, 5200, 7000);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("a the players' screen"), "{p:?}");
    // The Live menu restored on row 0: two downs and A on Custom Game.
    d.line(7100, "ui: screen 0xba (window 5, slot 4), live menu row 0");
    let p = presses(&mut d, 7200, 12000);
    assert_eq!(p.len(), 3, "{p:?}");
    assert!(
        p[0].starts_with("down toward Custom Game (row 2, from 0)"),
        "{p:?}"
    );
    assert!(p[2].starts_with("a Custom Game"), "{p:?}");
    // The lobby: wait for the local bots on Xbox Live.
    d.line(12100, "ui: screen none");
    d.line(12200, "coop-live: lobby mode 2");
    assert!(
        d.take_said()
            .iter()
            .any(|s| s == "bots-drive: in the custom game lobby")
    );
    assert!(presses(&mut d, 12300, 20000).is_empty());
    for p in 1..=3 {
        d.line(
            20000 + p,
            &format!(
                "bots: Bot {p} on port {p} is on Xbox Live as its own bot account, beside port 0's"
            ),
        );
    }
    let p = presses(&mut d, 20100, 22000);
    assert_eq!(p, ["a Start Game: 3 local bots on Xbox Live"]);
    // The phase leaves the menus: done.
    d.status(25000, "loading");
    assert!(d.done() && !d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("done: the game left the menus"))
    );
    assert!(presses(&mut d, 25100, 90000).is_empty());
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
    assert!(p[2].starts_with("a Custom Game"));
}

#[test]
fn the_press_line_names_the_screen_and_row() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 0");
    let p = d.poll(6700).unwrap();
    assert_eq!(
        d.press_line(&p),
        "bots-drive: press down on screen 0xba row 0 (toward Custom Game (row 2, from 0))"
    );
}

#[test]
fn the_main_menu_by_phase_alone() {
    // No `coop-live: lobby mode` line: the phase `menu` with no screen
    // for a while is the main menu.
    let mut d = Driver::new(1);
    d.line(0, "ui: screen none");
    d.status(1000, "menu");
    assert!(presses(&mut d, 1000, 5000).is_empty());
    let p = presses(&mut d, 6100, 8000);
    assert_eq!(p.len(), 2, "{p:?}");
}

#[test]
fn the_lobby_starts_anyway_after_its_wait() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 2");
    let p = presses(&mut d, 5200, 8000);
    assert_eq!(p, ["a Custom Game"]);
    d.line(8100, "live: session 0123 advertised for 127.0.0.1:1000");
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
    d.line(8100, "coop-live: lobby mode 2");
    for p in 1..=3 {
        d.line(
            8200,
            &format!("bots: B{p} on port {p} is on Xbox Live as guest 1 of port 0's account"),
        );
    }
    let p = presses(&mut d, 9700, 10000);
    assert_eq!(p.len(), 1, "{p:?}");
    let p = presses(&mut d, 10000, 10000 + START_MS + 2000);
    assert_eq!(p, ["up to Start Game", "a Start Game again"]);
    presses(&mut d, 12000 + START_MS, 20000 + 4 * START_MS);
    assert!(d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.starts_with("bots-drive: stuck: Start Game"))
    );
}

#[test]
fn the_browser_row_is_backed_out_of() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0xba (window 5, slot 4), live menu row 2");
    presses(&mut d, 5200, 8000);
    d.line(8100, "custom games: opening the browser");
    let p = presses(&mut d, 8200, 9000);
    assert_eq!(p, ["b back from the browser: that was row 3"]);
}

#[test]
fn the_wrong_main_menu_row_is_corrected() {
    let mut d = at_main();
    presses(&mut d, 2500, 5000);
    d.line(5100, "ui: screen 0x1e (window 5, slot 4), mode 1");
    let p = presses(&mut d, 5200, 9000);
    assert_eq!(p.len(), 3, "{p:?}");
    assert!(p[0].starts_with("b that was Split Screen"));
    assert!(p[2].starts_with("a Xbox Live"));
}

#[test]
fn no_main_menu_presses_then_gives_up() {
    let mut d = Driver::new(3);
    d.line(0, "ui: screen 0xb6 (window 1, slot 4)");
    assert!(presses(&mut d, 0, BOOT_WAIT_MS - 100).is_empty());
    let p = presses(&mut d, BOOT_WAIT_MS, BOOT_WAIT_MS + 30_000);
    assert_eq!(
        p,
        [
            "start no main menu after a minute",
            "a no main menu after a minute",
            "b no main menu after a minute"
        ]
    );
    assert!(d.stuck());
}
