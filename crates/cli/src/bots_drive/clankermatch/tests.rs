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

/// The custom game test's lines (`bots_drive::tests`): the main menu, the
/// Live menu on row 0 (a window 5 `none` screen with mask `0xf`).
const MAIN_ROW_0: &str = "ui: screen none (slot 4: window 5 active 0x82539070 pending 0x0 list [0x1/0xe16d0000]; slot 0: no window)";
const MAIN_ROW_1: &str = "ui: screen none (slot 4: window 5 active 0x82539070 pending 0x0 list [0x1/0xe16e0001]; slot 0: no window)";
const LIVE_ROW_0: &str = "ui: screen none (slot 4: window 5 active 0x82539a80 pending 0x0 list [0xf/0xe2780000]; slot 0: no window)";
/// The Clankermatch playlist list (a window 5 screen of its own), on rows
/// 0 to 2, and with a value that reads as no row.
const LIST_ROW_0: &str = "ui: screen none (slot 4: window 5 active 0x82541000 pending 0x0 list [0x1/0xe3000000]; slot 0: no window)";
const LIST_ROW_1: &str = "ui: screen none (slot 4: window 5 active 0x82541000 pending 0x0 list [0x1/0xe3010001]; slot 0: no window)";
const LIST_ROW_2: &str = "ui: screen none (slot 4: window 5 active 0x82541000 pending 0x0 list [0x1/0xe3020002]; slot 0: no window)";
const LIST_NO_ROW: &str = "ui: screen none (slot 4: window 5 active 0x82541000 pending 0x0 list [0x1/0x4588b0]; slot 0: no window)";
const DIALOG: &str = "ui: screen none (slot 4: window 3 active 0x82539070 pending 0x0 list [0x1/0xec700000]; slot 0: no window), dialog 0x21 confirmation_dialog (window 3, slot 4)";
const SEARCH_LINE: &str =
    "live: clankermatch: searching clanker playlist 10: no one to find; the console hosts in 6.1 s";
const ADVERTISED: &str = "live: session 0123456789abcdef advertised for 127.0.0.1:1000";
const HOSTING_LINE: &str = "clankers: clankermatch: hosting clanker playlist 10 (session 0123456789abcdef): 3 bots join over 5.0 s, 2 teams, the host on team 0";

/// A Clankermatch driver for the playlist on `row`, from the main menu to
/// A on the Live menu's Clankermatch row; the time of that A.
fn to_clankermatch(row: u32) -> (Driver, u64) {
    let mut d = Driver::clankermatch(1, row);
    d.line(900, MAIN_ROW_0);
    d.line(1000, "coop-live: lobby mode 4294967295");
    d.status(1000, "menu");
    assert_eq!(
        presses(&mut d, 2500, 5000),
        [
            "down to Xbox Live (row 1, from 0)",
            "a Xbox Live (signs in)"
        ]
    );
    d.line(3300, MAIN_ROW_1);
    d.line(9100, "ui: screen 0x1e (window 5, slot 4), mode 3");
    let p = presses(&mut d, 9200, 11_000);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("a the players' screen"), "{p:?}");
    // The Live menu on row 0: DOWN, DOWN, A on Clankermatch (row 2), not
    // on to Custom Game (row 4).
    d.line(11_100, LIVE_ROW_0);
    assert_eq!(
        presses(&mut d, 11_200, 20_000),
        [
            "down toward Clankermatch (row 2, from 0)",
            "down toward Clankermatch (row 2, from 1)",
            "a Clankermatch"
        ]
    );
    d.line(
        20_100,
        "clankermatch: the playlists are the clanker hoppers",
    );
    (d, 20_000)
}

#[test]
fn the_live_menu_row_is_clankermatch_and_the_playlist_row_is_read() {
    let (mut d, _) = to_clankermatch(2);
    // The list: from row 0 to row 2, each read back off its line.
    d.line(20_500, LIST_ROW_0);
    assert_eq!(
        presses(&mut d, 20_600, 22_500),
        ["down toward the clanker playlist (row 2, from 0)"]
    );
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.starts_with("bots-drive: on the Clankermatch playlist list: ui: screen")),
    );
    d.line(22_600, LIST_ROW_1);
    assert_eq!(
        presses(&mut d, 22_700, 24_500),
        ["down toward the clanker playlist (row 2, from 1)"]
    );
    d.line(24_600, LIST_ROW_2);
    assert_eq!(
        presses(&mut d, 24_700, 26_500),
        ["a the clanker playlist on row 2: search"]
    );
    // The search, the lobby: nothing pressed while the bots fill it.
    d.line(27_000, SEARCH_LINE);
    assert!(presses(&mut d, 27_100, 30_000).is_empty());
    d.line(33_000, ADVERTISED);
    d.line(33_100, HOSTING_LINE);
    assert!(presses(&mut d, 33_200, 120_000).is_empty());
    let said = d.take_said();
    assert!(
        said.contains(&"bots-drive: the clanker search runs".to_string()),
        "{said:?}"
    );
    assert!(
        said.contains(&"bots-drive: hosting the Clankermatch lobby: the bots fill it".to_string()),
        "{said:?}"
    );
    // The countdown, the game, its end.
    d.status(121_000, "loading");
    d.status(130_000, "gameplay");
    assert!(presses(&mut d, 130_100, 400_000).is_empty());
    d.status(400_000, "menu");
    let said = d.take_said();
    assert!(
        said.iter()
            .any(|s| s.contains("the Clankermatch game started (phase loading)")),
        "{said:?}"
    );
    assert!(
        said.iter().any(|s| s.contains("the game is over")),
        "{said:?}"
    );
    // Out: B, A on the dialog it opens, then no session and the Live menu.
    assert_eq!(
        presses(&mut d, 400_000, 400_400),
        ["b back out of the Clankermatch lobby"]
    );
    d.line(400_500, DIALOG);
    assert_eq!(
        presses(&mut d, 400_600, 403_600),
        ["a dialog 0x21 confirmation_dialog (window 3, slot 4) on the way out: A"]
    );
    d.line(404_000, "live: session 0123456789abcdef deleted");
    d.line(404_100, LIVE_ROW_0);
    assert!(presses(&mut d, 404_200, 409_000).is_empty());
    assert!(!d.done());
    assert!(presses(&mut d, 409_100, 409_200).is_empty());
    assert!(d.done() && !d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.starts_with("bots-drive: left the Clankermatch")),
    );
}

#[test]
fn an_unread_list_row_counts_the_downs() {
    let (mut d, _) = to_clankermatch(1);
    d.line(20_500, LIST_NO_ROW);
    assert_eq!(
        presses(&mut d, 20_600, 22_500),
        ["down toward the clanker playlist (row 1, from 0, the row unread)"]
    );
    // The line didn't change (the value reads as no row still): the DOWN
    // sent counts.
    assert_eq!(
        presses(&mut d, 22_600, 24_500),
        ["a the clanker playlist on row 1: search"]
    );
}

#[test]
fn row_0_is_a_at_once() {
    let (mut d, _) = to_clankermatch(0);
    d.line(20_500, LIST_ROW_0);
    assert_eq!(
        presses(&mut d, 20_600, 22_500),
        ["a the clanker playlist on row 0: search"]
    );
}

#[test]
fn a_human_search_stops_the_drive() {
    let (mut d, _) = to_clankermatch(0);
    d.line(20_500, LIST_ROW_0);
    presses(&mut d, 20_600, 22_500);
    d.line(
        23_000,
        "live: optimatch: searching playlist 2 for a party of 1, mean level 1, wide",
    );
    assert!(presses(&mut d, 23_100, 24_000).is_empty());
    assert!(d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("a human playlist's search started, not a clanker one")),
    );
}

#[test]
fn a_playlist_a_without_a_search_is_pressed_again_then_given_up() {
    let (mut d, _) = to_clankermatch(0);
    d.line(20_500, LIST_ROW_0);
    presses(&mut d, 20_600, 22_500);
    let p = presses(&mut d, 22_600, 22_600 + 4 * STEP_MS + 2_000);
    assert_eq!(
        p,
        [
            "a the clanker playlist again: no search seen",
            "a the clanker playlist again: no search seen",
            "a the clanker playlist again: no search seen"
        ]
    );
    assert!(d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("started no search after 4 tries")),
    );
}

#[test]
fn a_search_that_never_hosts_or_a_lobby_that_never_starts_is_given_up() {
    let (mut d, _) = to_clankermatch(0);
    d.line(20_500, LIST_ROW_0);
    presses(&mut d, 20_600, 22_500);
    d.line(23_000, SEARCH_LINE);
    presses(&mut d, 23_100, 23_100 + HOST_MS + 1_000);
    assert!(d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("never hosted its lobby")),
    );

    let (mut d, _) = to_clankermatch(0);
    d.line(20_500, LIST_ROW_0);
    presses(&mut d, 20_600, 22_500);
    d.line(23_000, SEARCH_LINE);
    d.line(25_000, ADVERTISED);
    d.line(25_100, HOSTING_LINE);
    presses(&mut d, 25_200, 26_000);
    presses(&mut d, 26_000, 26_000 + MATCH_START_MS + 1_000);
    assert!(d.stuck());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("the Clankermatch lobby did not start in 180 s (0 of 1 local bots")),
    );
}

#[test]
fn b_that_changes_nothing_is_followed_by_a() {
    let (mut d, _) = to_clankermatch(0);
    d.line(20_500, LIST_ROW_0);
    presses(&mut d, 20_600, 22_500);
    d.line(23_000, SEARCH_LINE);
    d.line(25_000, ADVERTISED);
    d.line(25_100, HOSTING_LINE);
    presses(&mut d, 25_200, 26_000);
    d.status(30_000, "gameplay");
    d.status(60_000, "menu");
    // The carnage report takes no B: A after it.
    assert_eq!(
        presses(&mut d, 60_000, 64_900),
        [
            "b back out of the Clankermatch lobby",
            "a B changed nothing (the carnage report?): A"
        ]
    );
    // A second game before the drive got out: wait for its end again.
    d.status(66_000, "gameplay");
    assert!(presses(&mut d, 66_100, 90_000).is_empty());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("another game started before the drive left")),
    );
}

#[test]
fn the_custom_game_target_is_unchanged() {
    let d = Driver::new(3);
    assert_eq!(d.live_row(), (CUSTOM_GAME_ROW, "Custom Game"));
    let d = Driver::clankermatch(3, 1);
    assert_eq!(d.live_row(), (CLANKERMATCH_ROW, "Clankermatch"));
}
