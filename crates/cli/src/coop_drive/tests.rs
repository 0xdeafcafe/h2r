use super::*;
use crate::coop_verdict::parse_line;

/// A model of the menus as runs showed them (20261118-092343 and the
/// static trace in `boot::ui_watch`): the game reaches the main menu by
/// itself, which reads as no screen and says `coop-live: lobby mode`;
/// Xbox Live opens the players' screen (`0x1e`) in mode 3, whose A opens
/// the Live menu; B on the Live menu goes back to the players' screen and
/// B there to the main menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Boot,
    Main,
    Join(u32),
    Campaign,
    Settings,
    Live,
    Lobby,
    Browser,
    Dialog,
    Level,
}

struct Game {
    tag: String,
    host: bool,
    at: At,
    /// The cursor on the screen shown (main menu, Live menu, lobby).
    row: u32,
    /// The main menu's cursor, kept while away from it.
    main_row: u32,
    /// The row the Live menu restores (`0x510a18`).
    live_row: u32,
    accepted: bool,
    joined: bool,
}

const LOBBY: u32 = 0x40;
const DIALOG: u32 = 0x7d;
const SETTINGS: u32 = 0x55;

struct World {
    games: Vec<Game>,
    /// Lines due: (at ms, game, text).
    due: Vec<(u64, usize, String)>,
    /// The main menu's invitation reader works on a cursor move.
    reader_works: bool,
    /// The main menu puts its cursor back on Campaign when it comes back.
    main_resets: bool,
    late_accept_ms: u64,
    started: bool,
    lines: Vec<String>,
}

impl World {
    fn new(tags: &[&str], reader_works: bool) -> Self {
        let games = tags
            .iter()
            .enumerate()
            .map(|(i, t)| Game {
                tag: t.to_string(),
                host: i == 0,
                at: At::Boot,
                row: 0,
                main_row: 0,
                live_row: 0,
                accepted: false,
                joined: false,
            })
            .collect();
        World {
            games,
            due: Vec::new(),
            reader_works,
            main_resets: false,
            late_accept_ms: 0,
            started: false,
            lines: Vec::new(),
        }
    }

    fn emit(&mut self, at: u64, g: usize, text: impl Into<String>) {
        self.due.push((at, g, text.into()));
    }

    /// Game `g` moves to `to`; its `ui: screen` line follows.
    fn go(&mut self, now: u64, g: usize, to: At) {
        let from = self.games[g].at;
        if from == At::Main {
            self.games[g].main_row = self.games[g].row;
        }
        self.games[g].at = to;
        let line = match to {
            At::Boot | At::Main | At::Level => {
                "ui: screen none (slot 4: no window; slot 0: no window)".to_string()
            }
            At::Join(m) => format!("ui: screen 0x1e (window 5, slot 4), mode {m}"),
            At::Campaign => "ui: screen 0xee (window 1, slot 4)".into(),
            At::Settings => format!("ui: screen 0x{SETTINGS:02x} (window 5, slot 4)"),
            At::Live => format!(
                "ui: screen 0xba (window 5, slot 4), live menu row {}",
                self.games[g].live_row
            ),
            At::Lobby => format!("ui: screen 0x{LOBBY:02x} (window 5, slot 4)"),
            At::Browser => "ui: screen 0x0d (window 5, slot 4)".into(),
            At::Dialog => format!("ui: screen 0x{DIALOG:02x} (window 0, slot 4)"),
        };
        self.games[g].row = match to {
            At::Main if self.main_resets => 0,
            At::Main => self.games[g].main_row,
            At::Live => {
                let r = self.games[g].live_row;
                if r <= COOP_ROW { r } else { 0 }
            }
            _ => 0,
        };
        self.emit(now + 50, g, line);
    }

    /// Game `g` gets `button` at `now`.
    fn press(&mut self, now: u64, g: usize, button: &str) {
        let at = self.games[g].at;
        let row = self.games[g].row;
        match (at, button) {
            (At::Main, "down") => {
                self.games[g].row = (row + 1).min(4);
                self.nudge(now, g);
            }
            (At::Main, "up") => {
                self.games[g].row = row.saturating_sub(1);
                self.nudge(now, g);
            }
            (At::Main, "a") => {
                let to = match row {
                    0 => At::Campaign,
                    1 => At::Join(MODE_LIVE),
                    2 => At::Join(MODE_SPLIT),
                    3 => At::Join(MODE_SYSTEM_LINK),
                    _ => At::Settings,
                };
                self.go(now, g, to);
            }
            (At::Campaign | At::Settings, "b") => self.go(now, g, At::Main),
            (At::Join(MODE_LIVE), "a") => self.go(now, g, At::Live),
            (At::Join(MODE_INVITE), "a") => self.join(now, g),
            (At::Join(_), "b") => self.go(now, g, At::Main),
            (At::Dialog, "a") => self.go(now, g, At::Join(MODE_INVITE)),
            (At::Live, "down") => self.games[g].row = (row + 1).min(6),
            (At::Live, "up") => self.games[g].row = row.saturating_sub(1),
            (At::Live, "b") => self.go(now, g, At::Join(MODE_LIVE)),
            (At::Live, "a") => match row {
                COOP_ROW => {
                    self.games[g].live_row = COOP_ROW;
                    self.emit(now + 10, g, "custom games: row chosen active");
                    self.emit(
                        now + 20,
                        g,
                        "coop-live: hosting a co-op campaign in the game's own lobby",
                    );
                    self.go(now, g, At::Lobby);
                    if self.games[g].host {
                        for other in self.games.iter().skip(1) {
                            let line = format!("coop-auto: invited {} to session 839a", other.tag);
                            self.due.push((now + 300, 0, line));
                        }
                    }
                }
                BROWSER_ROW => {
                    // The browser's row 4: its press stores it, and the
                    // menu's build puts the cursor back on it.
                    self.games[g].live_row = BROWSER_ROW;
                    self.emit(now + 10, g, "custom games: opening the browser");
                    self.emit(
                        now + 400,
                        g,
                        "live: join: qos lookup: 10.0.3.7:31002 session 839a: contacted, 172 bytes of data",
                    );
                    self.go(now, g, At::Browser);
                }
                _ => {}
            },
            (At::Browser, "a") if self.games[0].at == At::Lobby || self.started => {
                self.join(now, g);
            }
            (At::Browser, "b") => self.go(now, g, At::Live),
            (At::Lobby, "a") if self.games[g].host && row == 0 => {
                let guests = self.games.iter().filter(|o| o.joined).count();
                self.emit(
                    now + 50,
                    g,
                    format!(
                        "coop-live: lobby start state 7 (co-op wants one console) answered 9 for {} consoles",
                        guests + 1
                    ),
                );
                self.emit(now + 100, g, "session start returned 1");
                self.emit(
                    now + 200,
                    g,
                    "coop-probe: simulation mode sync-server (2) at tick 0",
                );
                self.started = true;
                self.go(now, g, At::Level);
            }
            _ => {}
        }
    }

    /// A cursor move on the main menu: the invitation reader's event.
    fn nudge(&mut self, now: u64, g: usize) {
        if self.reader_works && self.games[g].accepted && !self.games[g].joined {
            self.emit(
                now + 10,
                g,
                "coop-auto: joining Alpha's session 839a (the game took the invitation)",
            );
            self.go(now, g, At::Dialog);
        }
    }

    fn join(&mut self, now: u64, g: usize) {
        if self.games[g].joined {
            return;
        }
        self.games[g].joined = true;
        self.emit(
            now + 500,
            g,
            "coop-live: guest: joined Alpha's co-op session 839a",
        );
        self.go(now + 500, g, At::Lobby);
    }

    /// Runs every driver against the model until all are done or `limit`.
    /// Each game reaches the main menu by itself, as runs do: a moment of
    /// `0xb6`, then no screen and `coop-live: lobby mode`.
    fn run(&mut self, drivers: &mut [Driver], limit: u64) -> u64 {
        for g in 0..self.games.len() {
            let t = 17_000 + 1000 * g as u64;
            self.emit(t, g, "ui: screen 0xb6 (window 1, slot 4)");
            self.due.push((t + 300, g, "\u{0}main".into()));
            self.emit(t + 700, g, "coop-live: lobby mode 4294967295");
        }
        let mut now = 0;
        while now < limit {
            self.due.sort_by_key(|d| d.0);
            while let Some(i) = self.due.iter().position(|d| d.0 <= now) {
                let (_, g, text) = self.due.remove(i);
                if text == "\u{0}main" {
                    self.go(now, g, At::Main);
                    continue;
                }
                let tag = self.games[g].tag.clone();
                // A guest accepts the host's invitation by itself.
                if let Some(rest) = text.strip_prefix("coop-auto: invited ") {
                    let who = rest.split(' ').next().unwrap().to_string();
                    if let Some(j) = self.games.iter().position(|o| o.tag == who) {
                        let late = j == self.games.len() - 1 && self.late_accept_ms > 0;
                        let at = now + if late { self.late_accept_ms } else { 200 };
                        self.games[j].accepted = true;
                        self.emit(
                            at,
                            j,
                            "coop-auto: accepting Alpha's invitation to session 839a",
                        );
                    }
                }
                self.lines.push(format!("{tag} | {text}"));
                for d in drivers.iter_mut() {
                    d.line(now, &tag, &text);
                }
            }
            for (g, d) in drivers.iter_mut().enumerate() {
                if let Some(p) = d.poll(now) {
                    self.lines.push(d.press_line(&p));
                    self.press(now, g, p.button);
                }
                for s in d.take_said() {
                    self.lines.push(s);
                }
            }
            if drivers.iter().all(Driver::done) {
                return now;
            }
            now += 100;
        }
        now
    }
}

fn drivers(tags: &[&str], late: bool) -> Vec<Driver> {
    let n = tags.len();
    let lobby: Vec<String> = tags[1..n - usize::from(late)]
        .iter()
        .map(|t| t.to_string())
        .collect();
    tags.iter()
        .enumerate()
        .map(|(i, t)| {
            let role = if i == 0 {
                Role::Host {
                    lobby: lobby.clone(),
                }
            } else if late && i == n - 1 {
                Role::Late
            } else {
                Role::Guest
            };
            Driver::new(t, role)
        })
        .collect()
}

fn has(lines: &[String], p: &str) -> bool {
    lines.iter().any(|l| l.contains(p))
}

fn at_of(lines: &[String], p: &str) -> usize {
    lines
        .iter()
        .position(|l| l.contains(p))
        .unwrap_or_else(|| panic!("no {p:?} in\n{}", lines.join("\n")))
}

#[test]
fn screen_lines_parse() {
    assert_eq!(
        parse_screen("ui: screen 0x09 (window 5, slot 4)"),
        Some((Some(9), None, None))
    );
    assert_eq!(
        parse_screen("ui: screen 0xba (window 5, slot 4), live menu row 3"),
        Some((Some(0xba), Some(3), None))
    );
    assert_eq!(
        parse_screen("ui: screen 0x1e (window 5, slot 4), mode 3"),
        Some((Some(0x1e), None, Some(3)))
    );
    assert_eq!(parse_screen("ui: screen none"), Some((None, None, None)));
    assert_eq!(
        parse_screen(
            "ui: screen none (slot 4: window 1 active 0x0 pending 0x1 list []; slot 0: no window)"
        ),
        Some((None, None, None))
    );
    assert_eq!(parse_screen("coop-live: hosting"), None);
}

#[test]
fn three_games_with_no_invitation_reader_join_through_the_browser_and_start() {
    let tags = ["Alpha", "Bravo", "Charlie"];
    let mut w = World::new(&tags, false);
    let mut d = drivers(&tags, false);
    let end = w.run(&mut d, 600_000);
    let all = w.lines.join("\n");
    assert!(d.iter().all(|d| matches!(d.step, Step::Done)), "{all}");
    assert!(end < 200_000, "took {end} ms\n{all}");
    // The host started only once both guests were in.
    let start = at_of(&w.lines, "Alpha: press a on screen 0x40");
    for g in ["Bravo", "Charlie"] {
        assert!(
            at_of(&w.lines, &format!("{g} | coop-live: guest: joined")) < start,
            "{all}"
        );
        assert!(
            has(
                &w.lines,
                &format!("{g}: press a on screen 0xba row 4 (Custom Game Browser)")
            ),
            "{all}"
        );
        assert!(
            has(&w.lines, &format!("{g}: the invitation wasn't taken")),
            "{all}"
        );
    }
    assert!(
        has(
            &w.lines,
            "Alpha: press a on screen 0xba row 5 (Co-op Campaign)"
        ),
        "{all}"
    );
    assert!(has(&w.lines, "Alpha: done: the game started"), "{all}");
    // Every press says where it was made.
    assert!(
        w.lines
            .iter()
            .filter(|l| l.contains(": press "))
            .all(|l| l.contains(" on screen "))
    );
}

#[test]
fn a_working_invitation_reader_needs_no_browser() {
    let tags = ["Alpha", "Bravo"];
    let mut w = World::new(&tags, true);
    let mut d = drivers(&tags, false);
    w.run(&mut d, 600_000);
    let all = w.lines.join("\n");
    assert!(d.iter().all(|d| matches!(d.step, Step::Done)), "{all}");
    assert!(!has(&w.lines, "Custom Game Browser"), "{all}");
    // The dialog's A, then the players' screen in mode 4: A.
    assert!(
        has(
            &w.lines,
            "Bravo: press a on screen 0x7d (the invitation: go on)"
        ),
        "{all}"
    );
    assert!(
        has(
            &w.lines,
            "Bravo: press a on screen 0x1e (the invitation: go on)"
        ),
        "{all}"
    );
    assert!(
        has(
            &w.lines,
            "Bravo: press down on screen none (a cursor move for the invitation reader)"
        ),
        "{all}"
    );
}

#[test]
fn the_late_guest_joins_after_the_start() {
    let tags = ["Alpha", "Bravo", "Charlie"];
    let mut w = World::new(&tags, false);
    w.late_accept_ms = LATE_MS;
    let mut d = drivers(&tags, true);
    w.run(&mut d, 900_000);
    let all = w.lines.join("\n");
    assert!(d.iter().all(|d| matches!(d.step, Step::Done)), "{all}");
    let started = at_of(&w.lines, "Alpha | session start returned 1");
    let joined = at_of(&w.lines, "Charlie | coop-live: guest: joined");
    assert!(started < joined, "{all}");
    // Charlie's browser press came at least LATE_MS after the invitation.
    let invited_at = |d: &Driver| d.invited.unwrap();
    assert!(invited_at(&d[2]) > 0);
    assert!(
        has(
            &w.lines,
            "Charlie: press a on screen 0xba row 4 (Custom Game Browser)"
        ),
        "{all}"
    );
}

#[test]
fn no_boot_press_for_a_minute_then_three_then_stuck() {
    let mut d = Driver::new("Bravo", Role::Guest);
    d.line(0, "Bravo", "ui: screen 0x77 (window 5, slot 4)");
    let mut presses = Vec::new();
    for t in (0..200_000).step_by(100) {
        if let Some(p) = d.poll(t) {
            presses.push((t, p.button));
        }
    }
    assert!(presses[0].0 >= BOOT_WAIT_MS, "{presses:?}");
    let buttons: Vec<_> = presses.iter().map(|p| p.1).collect();
    assert_eq!(buttons, ["start", "a", "b"]);
    assert!(d.done());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("stuck: no main menu; screen 0x77"))
    );
}

#[test]
fn a_main_menu_that_comes_back_on_campaign_is_corrected() {
    let tags = ["Alpha", "Bravo"];
    let mut w = World::new(&tags, false);
    w.main_resets = true;
    let mut d = drivers(&tags, false);
    w.run(&mut d, 600_000);
    let all = w.lines.join("\n");
    assert!(d.iter().all(|d| matches!(d.step, Step::Done)), "{all}");
    // Bravo's fallback A landed on Campaign: B, DOWN, A.
    assert!(
        has(
            &w.lines,
            "Bravo: press b on screen 0xee (the campaign: that was Campaign)"
        ),
        "{all}"
    );
    assert!(!has(&w.lines, "Alpha | ui: screen 0xd1"), "{all}");
}

#[test]
fn the_players_screen_in_another_mode_says_which_row_it_was() {
    let mut d = Driver::new("Bravo", Role::Guest);
    d.line(0, "Bravo", "ui: screen none");
    d.line(10, "Bravo", "coop-live: lobby mode 4294967295");
    let mut t = 0;
    let mut pressed = Vec::new();
    while t < 5000 {
        if let Some(p) = d.poll(t) {
            pressed.push(p.button);
        }
        t += 100;
    }
    assert_eq!(pressed, ["down", "a"]);
    // That was System Link (row 3): back, up twice, A.
    d.line(t, "Bravo", "ui: screen 0x1e (window 5, slot 4), mode 2");
    while t < 12_000 {
        if let Some(p) = d.poll(t) {
            pressed.push(p.button);
        }
        t += 100;
    }
    assert_eq!(pressed, ["down", "a", "b", "up", "up", "a"]);
}

#[test]
fn a_wrong_row_on_the_live_menu_is_backed_out_of() {
    let mut d = Driver::new(
        "Alpha",
        Role::Host {
            lobby: vec!["Bravo".into()],
        },
    );
    d.line(
        0,
        "Alpha",
        "ui: screen 0xba (window 5, slot 4), live menu row 0",
    );
    let mut pressed = Vec::new();
    let mut t = 0;
    while t < 10_000 {
        if let Some(p) = d.poll(t) {
            pressed.push(p.button);
        }
        t += 100;
    }
    assert_eq!(pressed, ["down", "down", "down", "down", "down", "a"]);
    // The game opened the browser: row 4 was where row 5 should be.
    d.line(t, "Alpha", "custom games: opening the browser");
    let p = d.poll(t + 1000).unwrap();
    assert_eq!(p.button, "b");
}

/// The 2026-11-18 (023244) run's own lines, with the screens the drivers
/// would have seen added: what each driver asks for at the run's times.
#[test]
fn the_recorded_run_leads_to_the_presses_it_needed() {
    let log = include_str!("run-023244.log");
    let roles = crate::coop_verdict::Roles::read(log).unwrap();
    assert_eq!(roles.host, "Alpha");
    let mut host = Driver::new(
        "Alpha",
        Role::Host {
            lobby: roles.guests.clone(),
        },
    );
    let mut bravo = Driver::new("Bravo", Role::Guest);
    // Before the run's lines: both signed in and on their menus, Bravo
    // back on the main menu.
    host.line(
        60_000,
        "Alpha",
        "ui: screen 0xba (window 5, slot 4), live menu row 0",
    );
    host.step = Step::Hosting {
        at: 60_000,
        tries: 0,
    };
    bravo.fallback = true;
    bravo.line(60_000, "Bravo", "ui: screen 0x09 (window 5, slot 4)");
    bravo.step = Step::Waiting {
        since: 60_000,
        nudged: false,
    };
    let mut bravo_presses = Vec::new();
    let mut host_presses = Vec::new();
    for line in log.lines() {
        let Some((tag, text)) = parse_line(line) else {
            continue;
        };
        let secs: f64 = line[2..line.find(']').unwrap()].parse().unwrap();
        let now = (secs * 1000.0) as u64;
        host.line(now, tag, text);
        bravo.line(now, tag, text);
        if let Some(p) = host.poll(now) {
            host_presses.push((now, p.button));
        }
        if let Some(p) = bravo.poll(now) {
            bravo_presses.push((now, p.button));
        }
    }
    // The host is hosting, waiting for Bravo, who never joined: no Start.
    assert!(matches!(host.step, Step::Lobby { .. }), "{:?}", host.step);
    assert!(host.invited.is_some());
    assert!(host_presses.is_empty(), "{host_presses:?}");
    // Bravo accepted at +82.589 and was nudged on the main menu.
    assert_eq!(bravo.accepted, Some(82_589));
    assert!(
        bravo_presses.iter().any(|(_, b)| *b == "down"),
        "{bravo_presses:?}"
    );
    // 20 s after the acceptance (the run's own line says the same at
    // +102.092), Bravo goes for the browser: A on Xbox Live.
    let (at, button) = *bravo_presses.last().unwrap();
    assert!(at >= 82_589 + INVITE_WAIT_MS, "{bravo_presses:?}");
    assert_eq!(button, "a");
    assert!(
        matches!(bravo.step, Step::ToLive { .. }),
        "{:?}",
        bravo.step
    );
}

/// The 2026-11-18 (092343) run, the first with `ui: screen` lines (its
/// presses were made by hand): no press before the main menu, Xbox Live
/// once it is there, and A on the players' screen.
#[test]
fn the_first_screen_run_presses_only_at_the_main_menu() {
    let log = include_str!("run-092343.log");
    let mut bravo = Driver::new("Bravo", Role::Guest);
    let mut presses = Vec::new();
    let lines: Vec<(u64, &str, &str)> = log
        .lines()
        .filter_map(|l| {
            let (tag, text) = parse_line(l)?;
            let secs: f64 = l[2..l.find(']')?].parse().ok()?;
            Some(((secs * 1000.0) as u64, tag, text))
        })
        .collect();
    let mut next = 0;
    // The harness polls from the socket's line on (+0.913 for Bravo).
    for now in (913..90_000).step_by(100) {
        while next < lines.len() && lines[next].0 <= now {
            let (at, tag, text) = lines[next];
            bravo.line(at, tag, text);
            next += 1;
        }
        if let Some(p) = bravo.poll(now) {
            presses.push((now, p.button, bravo.press_line(&p)));
        }
    }
    let all: Vec<String> = presses.iter().map(|p| format!("{} {}", p.0, p.2)).collect();
    let all = all.join("\n");
    // Nothing before the main menu (none after `lobby mode` at +19.456).
    assert!(presses[0].0 >= 19_456 + 1000, "{all}");
    assert_eq!((presses[0].1, presses[1].1), ("down", "a"), "{all}");
    assert!(presses[1].2.contains("on screen none (Xbox Live"), "{all}");
    // The players' screen at +73.754: A on to the Live menu.
    let join = presses.iter().find(|p| p.0 >= 73_754).expect(&all);
    assert_eq!(join.1, "a", "{all}");
    assert!(
        join.2
            .contains("on screen 0x1e (the players' screen for Xbox Live"),
        "{all}"
    );
    assert!(!bravo.done(), "{all}");
}

#[test]
fn a_press_is_a_short_pad_step() {
    let v = pad_params(&press("a", "x"));
    let p: agent_proto::PadParams = serde_json::from_value(v).unwrap();
    assert_eq!(p.steps[0].buttons, ["a"]);
    assert!(p.steps[0].check().is_ok());
    assert_eq!(p.steps[0].ms, Some(150));
}
