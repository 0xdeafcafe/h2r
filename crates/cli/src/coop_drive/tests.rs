use super::*;
use crate::coop_verdict::parse_line;

/// A small model of the menus the drivers press through: enough of each
/// screen to answer the presses the drivers make, and the lines the games
/// print on the way.
struct Game {
    tag: String,
    host: bool,
    screen: Option<u32>,
    /// The cursor on the screen shown (main menu, Live menu, lobby).
    row: u32,
    /// The row the Live menu restores (`0x510a18`).
    live_row: u32,
    signed_in: bool,
    accepted: bool,
    joined: bool,
    /// The boot screens: intro, profile picker.
    boot: u32,
}

const INTRO: u32 = 0x50;
const PROFILES: u32 = 0x51;
const LOBBY: u32 = 0x40;

struct World {
    games: Vec<Game>,
    /// Lines due: (at ms, game, text).
    due: Vec<(u64, usize, String)>,
    /// The main menu's invitation reader works on a cursor move.
    reader_works: bool,
    late_accept_ms: u64,
    started: bool,
    lines: Vec<String>,
    presses: Vec<String>,
}

impl World {
    fn new(tags: &[&str], reader_works: bool) -> Self {
        let games = tags
            .iter()
            .enumerate()
            .map(|(i, t)| Game {
                tag: t.to_string(),
                host: i == 0,
                screen: None,
                row: 0,
                live_row: 0,
                signed_in: false,
                accepted: false,
                joined: false,
                boot: 0,
            })
            .collect();
        World {
            games,
            due: Vec::new(),
            reader_works,
            late_accept_ms: 0,
            started: false,
            lines: Vec::new(),
            presses: Vec::new(),
        }
    }

    fn emit(&mut self, at: u64, g: usize, text: impl Into<String>) {
        self.due.push((at, g, text.into()));
    }

    fn show(&mut self, now: u64, g: usize, screen: Option<u32>) {
        self.games[g].screen = screen;
        let line = match screen {
            None => "ui: screen none".to_string(),
            Some(s) if s == LIVE_MENU => format!(
                "ui: screen 0x{s:02x} (window 5, slot 4), live menu row {}",
                self.games[g].live_row
            ),
            Some(s) => format!("ui: screen 0x{s:02x} (window 5, slot 4)"),
        };
        self.emit(now + 50, g, line);
    }

    /// Game `g` gets `button` at `now`.
    fn press(&mut self, now: u64, g: usize, button: &str) {
        let screen = self.games[g].screen;
        let tag = self.games[g].tag.clone();
        self.presses.push(format!("{tag} {button} {screen:?}"));
        match (screen, button) {
            (Some(INTRO), "start") => {
                self.games[g].boot = 1;
                self.show(now, g, Some(PROFILES));
            }
            (Some(PROFILES), "down") => self.games[g].row = 1,
            (Some(PROFILES), "a") if self.games[g].row == 1 => {
                self.games[g].row = 0;
                self.show(now, g, Some(MAIN_MENU));
            }
            (Some(MAIN_MENU), "down") => {
                self.games[g].row = (self.games[g].row + 1).min(4);
                self.nudge(now, g);
            }
            (Some(MAIN_MENU), "up") => {
                self.games[g].row = self.games[g].row.saturating_sub(1);
                self.nudge(now, g);
            }
            (Some(MAIN_MENU), "a") if self.games[g].row == MAIN_LIVE_ROW => {
                if !self.games[g].signed_in {
                    self.games[g].signed_in = true;
                    self.emit(now + 100, g, "coop-live: lobby mode 4294967295");
                }
                let r = self.games[g].live_row;
                self.games[g].row = if r <= 3 { r } else { 0 };
                self.show(now, g, Some(LIVE_MENU));
            }
            (Some(LIVE_MENU), "down") => self.games[g].row = (self.games[g].row + 1).min(5),
            (Some(LIVE_MENU), "up") => self.games[g].row = self.games[g].row.saturating_sub(1),
            (Some(LIVE_MENU), "b") => {
                // The main menu keeps its cursor on Xbox Live.
                self.games[g].row = MAIN_LIVE_ROW;
                self.show(now, g, Some(MAIN_MENU));
            }
            (Some(LIVE_MENU), "a") => match self.games[g].row {
                COOP_ROW => {
                    self.games[g].live_row = 2;
                    self.games[g].row = 0;
                    self.emit(now + 10, g, "custom games: row chosen active");
                    self.emit(
                        now + 20,
                        g,
                        "coop-live: hosting a co-op campaign in the game's own lobby",
                    );
                    self.show(now, g, Some(LOBBY));
                    if self.games[g].host {
                        for (i, other) in self.games.iter().enumerate().skip(1) {
                            let _ = i;
                            let line = format!("coop-auto: invited {} to session 839a", other.tag);
                            self.due.push((now + 300, 0, line));
                        }
                    }
                }
                BROWSER_ROW => {
                    self.games[g].live_row = 3;
                    self.emit(now + 10, g, "custom games: opening the browser");
                    self.emit(
                        now + 400,
                        g,
                        "live: join: qos lookup: 10.0.3.7:31002 session 839a: contacted, 172 bytes of data",
                    );
                    self.show(now, g, Some(BROWSER));
                }
                _ => {}
            },
            (Some(BROWSER), "a") if self.games[0].screen == Some(LOBBY) || self.started => {
                self.join(now, g);
            }
            (Some(BROWSER), "b") => {
                let r = self.games[g].live_row;
                self.games[g].row = r;
                self.show(now, g, Some(LIVE_MENU));
            }
            (Some(LOBBY), "a") if self.games[g].host && self.games[g].row == 0 => {
                let guests = self.games.iter().filter(|o| o.joined).count();
                self.emit(now + 50, g, format!("coop-live: lobby start state 7 (co-op wants one console) answered 9 for {} consoles", guests + 1));
                self.emit(now + 100, g, "session start returned 1");
                self.emit(
                    now + 200,
                    g,
                    "coop-probe: simulation mode sync-server (2) at tick 0",
                );
                self.started = true;
                self.show(now, g, None);
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
            self.join(now, g);
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
        self.show(now + 500, g, Some(LOBBY));
    }

    /// Runs every driver against the model until all are done or `limit`.
    fn run(&mut self, drivers: &mut [Driver], limit: u64) -> u64 {
        for g in 0..self.games.len() {
            self.games[g].screen = Some(INTRO);
            self.emit(
                1000,
                g,
                format!("ui: screen 0x{INTRO:02x} (window 0, slot 4)"),
            );
        }
        let mut now = 0;
        while now < limit {
            // Lines due now, in order, to every driver.
            self.due.sort_by_key(|d| d.0);
            while let Some(i) = self.due.iter().position(|d| d.0 <= now) {
                let (_, g, text) = self.due.remove(i);
                let tag = self.games[g].tag.clone();
                // A guest accepts the host's invitation once signed in.
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
        Some((Some(9), None))
    );
    assert_eq!(
        parse_screen("ui: screen 0xba (window 5, slot 4), live menu row 3"),
        Some((Some(0xba), Some(3)))
    );
    assert_eq!(parse_screen("ui: screen none"), Some((None, None)));
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
                &format!("{g}: press a on screen 0xba row 3 (Custom Game Browser)")
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
            "Alpha: press a on screen 0xba row 4 (Co-op Campaign)"
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
    assert!(
        has(
            &w.lines,
            "Bravo: press down on screen 0x09 (a cursor move for the invitation reader)"
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
            "Charlie: press a on screen 0xba row 3 (Custom Game Browser)"
        ),
        "{all}"
    );
}

#[test]
fn a_boot_screen_that_never_moves_is_given_up_on() {
    let mut d = Driver::new("Bravo", Role::Guest);
    d.line(0, "Bravo", "ui: screen 0x77 (window 5, slot 4)");
    let mut presses = Vec::new();
    for t in (0..120_000).step_by(100) {
        if let Some(p) = d.poll(t) {
            presses.push(p.button);
        }
    }
    assert_eq!(presses, ["start", "a", "start"]);
    assert!(d.done());
    assert!(
        d.take_said()
            .iter()
            .any(|s| s.contains("stuck: boot screen 0x77"))
    );
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
    assert_eq!(pressed, ["down", "down", "down", "down", "a"]);
    // The game opened the browser: row 3 was where row 4 should be.
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

#[test]
fn a_press_is_a_short_pad_step() {
    let v = pad_params(&press("a", "x"));
    let p: agent_proto::PadParams = serde_json::from_value(v).unwrap();
    assert_eq!(p.steps[0].buttons, ["a"]);
    assert!(p.steps[0].check().is_ok());
    assert_eq!(p.steps[0].ms, Some(150));
}
