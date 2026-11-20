//! `h2 coop test --drive`: the menus pressed for every game, on what each
//! game says (docs/live/live-coop.md section 12.6). Each game runs with
//! `H2_UI_WATCH=1`, so it prints `ui: screen 0x..` whenever the screen
//! player one's input goes to changes (`boot::ui_watch`); a [`Driver`] per
//! game reads those and the co-op lines of every game, and asks for one
//! press at a time, which the harness sends as `input.pad` on the game's
//! agent socket. Every press waits on a screen or a line, never on time
//! alone, and every wait has a timeout; each press is said as
//! `coop-drive: Alpha: press a on screen 0x09 (Xbox Live)`.
//!
//! The paths (screens as `boot::ui_watch` names them; the main menu
//! reads as no screen, so "the main menu" is no screen once the game has
//! said `coop-live: lobby mode`, which it does on reaching it):
//!
//! * Every game: no press until the main menu, which the coop-test data
//!   reaches by itself (boot presses only if it hasn't [`BOOT_WAIT_MS`]
//!   after the first chance to press); then DOWN, A for Xbox Live, which
//!   opens the players' screen (`0x1e`) in mode 3; A there opens the Xbox
//!   Live menu (`0xba`). A wrong main menu row shows in what it opened
//!   (the players' screen in mode 1 or 2, or the campaign's `0xee`), and
//!   is backed out of and corrected.
//! * The host: from the row the Live menu restores to Co-op Campaign (row
//!   4), A; once `coop-live: hosting`, it waits for every lobby guest's
//!   `coop-live: guest: joined`, then A on Start Game (the lobby's row 0,
//!   `0x250fb3`), again if the start isn't seen.
//! * A guest: B back to the main menu (through the players' screen),
//!   where the invitation reader runs (12.1), and a cursor move there once
//!   it has accepted (the reader runs on an event). A dialog or the
//!   players' screen in mode 4 after that gets A (that is the invitation
//!   being taken). If the game hasn't joined within [`INVITE_WAIT_MS`] of
//!   its acceptance, Xbox Live again, Custom Game Browser (row 5), and A on
//!   the first game once the browser has probed one.
//! * The late guest: the same, its fallback held until
//!   [`LATE_MS`] after the host invited it, so it joins in progress.

use std::collections::BTreeSet;

use agent_proto::sim::menu::{MAIN_MENU_THEATER_MARKER, Mode};

/// Screen ids, the players' screen's modes and the menu rows, shared with
/// `h2 agent sim lobby` (`agent_proto::sim::menu`).
pub use agent_proto::sim::menu::{
    BROWSER, BROWSER_ROW, CAMPAIGN_SCREENS, COOP_ROW, JOIN_SCREEN, LIVE_MENU, MAIN_LIVE_ROW,
    MAIN_MENU, MODE_INVITE, MODE_LIVE, MODE_SPLIT, MODE_SYSTEM_LINK,
};

/// Between two presses: the menus take a press per frame at a few frames
/// a second with three games on one Mac.
pub const GAP_MS: u64 = 700;
/// A screen must hold this long before a press on it.
pub const SETTLE_MS: u64 = 1500;
/// A boot screen that holds this long gets its press.
pub const BOOT_SETTLE_MS: u64 = 4000;
/// No boot presses until this long after the first chance to press: the
/// coop-test data reaches the main menu by itself in about 19 s.
pub const BOOT_WAIT_MS: u64 = 60_000;
/// How long a press may take to show its screen or line.
pub const STEP_MS: u64 = 15_000;
/// A guest waits this long after its acceptance for the game to take the
/// invitation before it joins through the browser.
pub const INVITE_WAIT_MS: u64 = 20_000;
/// A guest that never accepted an invitation looks in the browser after
/// this long on the main menu.
pub const NO_INVITE_MS: u64 = 300_000;
/// The late guest joins this long after the host invited it.
pub const LATE_MS: u64 = 120_000;
/// How long the host waits for its guests before it starts anyway.
pub const LOBBY_WAIT_MS: u64 = 600_000;
/// Retries of one step before the driver gives up on that game.
pub const TRIES: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// Waits for these guests in the lobby.
    Host {
        lobby: Vec<String>,
    },
    Guest,
    Late,
}

/// One press to send now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Press {
    pub button: &'static str,
    pub why: String,
    /// The Live menu row the cursor was on, as far as the driver knows.
    pub row: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// To the main menu.
    Boot,
    /// DOWN, A sent on the main menu; waiting for the Live menu.
    ToLive {
        at: u64,
        tries: u32,
    },
    /// The host's row press sent; waiting for `coop-live: hosting`.
    Hosting {
        at: u64,
        tries: u32,
    },
    /// The host waits for its guests.
    Lobby {
        since: u64,
    },
    /// Start Game pressed; waiting for the start.
    Starting {
        at: u64,
        tries: u32,
    },
    /// A guest went back to the main menu; waiting for the invitation.
    Waiting {
        since: u64,
        nudged: bool,
    },
    /// The browser row pressed; waiting for the browser.
    ToBrowser {
        at: u64,
        tries: u32,
    },
    /// The browser is open: A on the first game once it has probed one.
    Browsing {
        since: u64,
        pressed: Option<u64>,
        tries: u32,
    },
    Done,
    Stuck,
}

/// One game's driver (module docs). Time is milliseconds since the run
/// started, as the merged log counts it.
#[derive(Debug, Clone)]
pub struct Driver {
    pub tag: String,
    role: Role,
    step: Step,
    screen: Option<u32>,
    screen_since: u64,
    live_row: Option<u32>,
    /// The players' screen's mode.
    mode: Option<u32>,
    main_menu_theater: bool,
    /// The game said `coop-live: lobby mode`: it has been at the main menu.
    menu_seen: bool,
    /// Presses on screens met while waiting for the invitation.
    dialog_presses: u32,
    /// The first poll (the harness polls once the agent socket is up).
    first_poll: Option<u64>,
    /// The screen appearance (its `screen_since`) a press was planned on.
    acted: Option<u64>,
    /// The Live menu's cursor: the restored row, then our moves.
    cursor: Option<u32>,
    /// Presses queued for this step, sent [`GAP_MS`] apart.
    queue: Vec<Press>,
    last_press: u64,
    /// Boot presses so far, per screen.
    boot: Vec<(Option<u32>, u32)>,
    /// When the game accepted the host's invitation, and when the host
    /// invited this game.
    accepted: Option<u64>,
    invited: Option<u64>,
    /// The game took the invitation (`coop-auto: joining`).
    took: Option<u64>,
    /// The browser probed a game.
    probed: Option<u64>,
    /// The lobby guests seen in the host's session.
    joined: BTreeSet<String>,
    /// Said once each, and what is still to say.
    said: BTreeSet<String>,
    out: Vec<String>,
    /// Signed in once: going back to the Live menu is the fallback.
    fallback: bool,
}

impl Driver {
    pub fn new(tag: &str, role: Role) -> Self {
        Driver {
            tag: tag.into(),
            role,
            step: Step::Boot,
            screen: None,
            screen_since: 0,
            live_row: None,
            mode: None,
            main_menu_theater: false,
            menu_seen: false,
            first_poll: None,
            dialog_presses: 0,
            acted: None,
            cursor: None,
            queue: Vec::new(),
            last_press: 0,
            boot: Vec::new(),
            accepted: None,
            invited: None,
            took: None,
            probed: None,
            joined: BTreeSet::new(),
            said: BTreeSet::new(),
            out: Vec::new(),
            fallback: false,
        }
    }

    pub fn done(&self) -> bool {
        matches!(self.step, Step::Done | Step::Stuck)
    }

    /// A line of game `from` (any game's: the host counts its guests).
    pub fn line(&mut self, now: u64, from: &str, text: &str) {
        let mine = from == self.tag;
        if mine {
            if let Some((screen, row, mode)) = parse_screen(text) {
                self.mode = mode;
                self.main_menu_theater = text.contains(MAIN_MENU_THEATER_MARKER);
                if screen != self.screen {
                    self.screen = screen;
                    self.screen_since = now;
                    self.cursor = (screen == Some(LIVE_MENU))
                        .then(|| row.filter(|r| *r <= BROWSER_ROW).unwrap_or(0));
                }
                self.live_row = row;
                return;
            }
            if text.starts_with("coop-live: lobby mode ") {
                self.menu_seen = true;
            }
            if text.starts_with("coop-auto: accepting ") {
                self.accepted = Some(now);
            } else if text.starts_with("coop-auto: joining ")
                || text.starts_with("live: join: the game took")
            {
                self.took = Some(now);
            } else if text.starts_with("coop-live: guest: joined ") {
                self.finish(now, "joined the host's session");
            } else if text.starts_with("live: join: qos lookup: ")
                && matches!(self.step, Step::Browsing { .. })
            {
                self.probed.get_or_insert(now);
            }
        }
        if let Role::Host { lobby } = &self.role {
            if !mine
                && lobby.iter().any(|g| g == from)
                && text.starts_with("coop-live: guest: joined ")
            {
                self.joined.insert(from.to_string());
            }
            if mine && text.starts_with("coop-auto: invited ") {
                self.invited.get_or_insert(now);
            }
        } else if !mine && text.starts_with("coop-auto: invited ") {
            // "coop-auto: invited Charlie to session ..."
            let who = text["coop-auto: invited ".len()..].split(' ').next();
            if who == Some(self.tag.as_str()) {
                self.invited.get_or_insert(now);
            }
        }
        if mine {
            match &self.step {
                Step::Hosting { .. } if text.starts_with("coop-live: hosting ") => {
                    self.step = Step::Lobby { since: now };
                }
                Step::Hosting { .. } if text.starts_with("custom games: opening the browser") => {
                    // The browser's row, not co-op's: back to the Live menu
                    // and again.
                    self.queue = vec![press("b", "back from the browser: that was row 5")];
                }
                Step::Starting { .. }
                    if text.contains("session start returned 1")
                        || text.contains("simulation mode sync-server") =>
                {
                    self.finish(now, "the game started");
                }
                Step::ToBrowser { .. } if text.starts_with("custom games: opening the browser") => {
                    self.step = Step::Browsing {
                        since: now,
                        pressed: None,
                        tries: 0,
                    };
                }
                _ => {}
            }
        }
    }

    fn finish(&mut self, _now: u64, why: &str) {
        self.queue.clear();
        self.step = Step::Done;
        self.say(format!("done: {why}"));
    }

    fn say(&mut self, what: String) {
        if self.said.insert(what.clone()) {
            self.out.push(what);
        }
    }

    /// What it has to say since last asked (`coop-drive: <tag>: ...`).
    pub fn take_said(&mut self) -> Vec<String> {
        std::mem::take(&mut self.out)
            .into_iter()
            .map(|s| format!("coop-drive: {}: {s}", self.tag))
            .collect()
    }

    fn stuck(&mut self, why: String) {
        self.queue.clear();
        self.step = Step::Stuck;
        self.say(format!("stuck: {why}; press on its window"));
    }

    fn held(&self, now: u64, ms: u64) -> bool {
        now.saturating_sub(self.screen_since) >= ms
    }

    /// At the main menu: screen 9, or no screen once the game has said
    /// `coop-live: lobby mode`.
    fn at_main(&self) -> bool {
        self.screen == Some(MAIN_MENU) || (self.screen.is_none() && self.menu_seen)
    }

    /// The screen has held [`SETTLE_MS`] and nothing was planned on this
    /// appearance of it yet; marks it planned.
    fn fresh(&mut self, now: u64) -> bool {
        if !self.held(now, SETTLE_MS) || self.acted == Some(self.screen_since) {
            return false;
        }
        self.acted = Some(self.screen_since);
        true
    }

    /// On the way to the Live menu, what the screen shown asks for: the
    /// players' screen goes on in Xbox Live mode, a wrong main menu row is
    /// backed out of and corrected. Whether it planned anything.
    fn on_the_way(&mut self, now: u64) -> bool {
        let screen = self.screen;
        let campaign = screen.is_some_and(|s| CAMPAIGN_SCREENS.contains(&s));
        if screen != Some(JOIN_SCREEN) && !campaign {
            return false;
        }
        if !self.fresh(now) {
            return true;
        }
        self.queue = match (campaign, self.mode) {
            (true, _) => vec![
                press("b", "the campaign: that was Campaign"),
                press("down", "to Xbox Live"),
                press("a", "Xbox Live"),
            ],
            (false, Some(MODE_LIVE) | None) => {
                vec![press(
                    "a",
                    "the players' screen for Xbox Live: on to the Live menu",
                )]
            }
            (false, Some(mode)) => back_to_live(mode, self.main_menu_theater),
        };
        true
    }

    /// The presses from the Live menu's restored row to `row`, then A.
    fn to_row(&self, row: u32, what: &str) -> Vec<Press> {
        let from = self.cursor.unwrap_or(0);
        let (button, n) = if from <= row {
            ("down", row - from)
        } else {
            ("up", from - row)
        };
        let mut v: Vec<Press> = (0..n)
            .map(|_| press(button, &format!("toward {what} (row {row}, from {from})")))
            .collect();
        v.push(press("a", what));
        v
    }

    /// The next press, if one is due at `now`.
    pub fn poll(&mut self, now: u64) -> Option<Press> {
        if self.done() {
            return None;
        }
        if self.queue.is_empty() {
            self.plan(now);
        }
        if self.queue.is_empty() || now.saturating_sub(self.last_press) < GAP_MS {
            return None;
        }
        self.last_press = now;
        let mut p = self.queue.remove(0);
        p.row = self.cursor;
        if let Some(c) = &mut self.cursor {
            match p.button {
                "down" => *c = (*c + 1).min(5),
                "up" => *c = c.saturating_sub(1),
                _ => {}
            }
        }
        Some(p)
    }

    /// Fills the queue for the step at `now`.
    fn plan(&mut self, now: u64) {
        let screen = self.screen;
        match self.step.clone() {
            Step::Boot => {
                let first = *self.first_poll.get_or_insert(now);
                if self.at_main() {
                    if self.held(now, SETTLE_MS) {
                        self.go_live(now, 0);
                    }
                    return;
                }
                if screen == Some(LIVE_MENU) {
                    if self.held(now, SETTLE_MS) {
                        self.at_live_menu(now);
                    }
                    return;
                }
                if self.on_the_way(now) {
                    self.step = Step::ToLive { at: now, tries: 0 };
                    return;
                }
                // The game reaches the main menu by itself; press only if
                // it hasn't for a long while.
                if now.saturating_sub(first) < BOOT_WAIT_MS || !self.held(now, BOOT_SETTLE_MS) {
                    return;
                }
                let n = match self.boot.iter_mut().find(|(s, _)| *s == screen) {
                    Some((_, n)) => n,
                    None => {
                        self.boot.push((screen, 0));
                        &mut self.boot.last_mut().unwrap().1
                    }
                };
                let k = *n;
                *n += 1;
                // Hold off another round until the screen has held again.
                self.screen_since = now;
                self.queue = match k {
                    0 => vec![press("start", "no main menu after a minute")],
                    1 => vec![press("a", "no main menu after a minute")],
                    2 => vec![press("b", "no main menu after a minute")],
                    _ => {
                        self.stuck(format!(
                            "no main menu; screen {} held after 3 presses",
                            show(screen)
                        ));
                        Vec::new()
                    }
                };
            }
            Step::ToLive { at, tries } => {
                if screen == Some(LIVE_MENU) {
                    if self.held(now, SETTLE_MS) {
                        self.at_live_menu(now);
                    }
                } else if self.on_the_way(now) {
                    if !self.queue.is_empty() {
                        self.step = Step::ToLive { at: now, tries };
                    }
                } else if now.saturating_sub(at) > STEP_MS {
                    if tries + 1 >= TRIES {
                        self.stuck(format!(
                            "no Xbox Live menu after {TRIES} tries (on {})",
                            show(screen)
                        ));
                    } else if self.at_main() {
                        // A again: what it opens says where the cursor was.
                        self.queue = vec![press("a", "Xbox Live, again")];
                        self.step = Step::ToLive {
                            at: now,
                            tries: tries + 1,
                        };
                    } else {
                        self.queue = vec![press("b", "a screen on the way to Xbox Live")];
                        self.step = Step::ToLive {
                            at: now,
                            tries: tries + 1,
                        };
                    }
                }
            }
            Step::Hosting { at, tries } => {
                if now.saturating_sub(at) > STEP_MS {
                    if tries + 1 >= TRIES {
                        self.stuck("Co-op Campaign never said `coop-live: hosting`".into());
                    } else if screen == Some(LIVE_MENU) && self.held(now, SETTLE_MS) {
                        self.queue = self.to_row(COOP_ROW, "Co-op Campaign");
                        self.step = Step::Hosting {
                            at: now,
                            tries: tries + 1,
                        };
                    } else if screen != Some(LIVE_MENU) {
                        self.queue = vec![press("b", "back to the Xbox Live menu")];
                        self.step = Step::Hosting {
                            at: now,
                            tries: tries + 1,
                        };
                    }
                }
            }
            Step::Lobby { since } => {
                let Role::Host { lobby } = &self.role else {
                    return;
                };
                let all = lobby.iter().all(|g| self.joined.contains(g));
                let late = now.saturating_sub(since) > LOBBY_WAIT_MS;
                if (all || late) && self.held(now, SETTLE_MS) {
                    let why = if all {
                        format!("Start Game: {} in the lobby", lobby.len())
                    } else {
                        "Start Game: not every guest came; starting anyway".into()
                    };
                    self.queue = vec![press("a", &why)];
                    self.step = Step::Starting { at: now, tries: 0 };
                }
            }
            Step::Starting { at, tries } => {
                if now.saturating_sub(at) > STEP_MS {
                    if tries + 1 >= TRIES {
                        self.stuck("Start Game didn't start the game".into());
                    } else {
                        // Focus off Start Game: up to row 0, A.
                        self.queue =
                            vec![press("up", "to Start Game"), press("a", "Start Game again")];
                        self.step = Step::Starting {
                            at: now,
                            tries: tries + 1,
                        };
                    }
                }
            }
            Step::Waiting { since, nudged } => {
                if screen == Some(LIVE_MENU) && self.held(now, SETTLE_MS) {
                    // Still on the Live menu: back to the main menu.
                    self.queue = vec![press("b", "back to the main menu for the invitation")];
                    self.screen_since = now;
                    return;
                }
                let other = screen.is_some() && !self.at_main();
                if screen == Some(JOIN_SCREEN) || (self.accepted.is_some() && other) {
                    // The players' screen in mode 4 is the invitation being
                    // taken: A. Any other screen once accepted is the
                    // invitation dialog: A, its first button. The players'
                    // screen otherwise: B on to the main menu.
                    if self.dialog_presses < TRIES && self.fresh(now) {
                        self.dialog_presses += 1;
                        let join = self.accepted.is_some()
                            && (screen != Some(JOIN_SCREEN) || self.mode == Some(MODE_INVITE));
                        self.queue = if join {
                            vec![press("a", "the invitation: go on")]
                        } else {
                            vec![press("b", "back to the main menu for the invitation")]
                        };
                    }
                    return;
                }
                if !self.at_main() || !self.held(now, SETTLE_MS) {
                    return;
                }
                if !nudged && self.accepted.is_some() {
                    // The reader runs on an event: a cursor move.
                    self.queue = vec![
                        press("down", "a cursor move for the invitation reader"),
                        press("up", "a cursor move for the invitation reader"),
                    ];
                    self.step = Step::Waiting {
                        since,
                        nudged: true,
                    };
                    return;
                }
                let gate = match self.role {
                    Role::Late => self.invited.map(|i| i + LATE_MS),
                    _ => Some(since),
                };
                // Without an acceptance there is no lobby to look for yet,
                // unless none came for a long while.
                let waited = match self.accepted {
                    Some(a) => now.saturating_sub(a.max(since)) >= INVITE_WAIT_MS,
                    None => now.saturating_sub(since) >= NO_INVITE_MS,
                };
                let ready = gate.is_some_and(|g| now >= g)
                    && waited
                    && self
                        .took
                        .is_none_or(|t| now.saturating_sub(t) >= INVITE_WAIT_MS);
                if ready {
                    self.say(format!(
                        "the invitation wasn't taken; joining through the browser{}",
                        if self.accepted.is_none() {
                            " (never accepted)"
                        } else {
                            ""
                        }
                    ));
                    // The cursor was left on Xbox Live.
                    self.queue = vec![press("a", "Xbox Live, for the browser")];
                    self.step = Step::ToLive { at: now, tries: 0 };
                }
            }
            Step::ToBrowser { at, tries } => {
                if now.saturating_sub(at) > STEP_MS {
                    if tries + 1 >= TRIES {
                        self.stuck("the browser never opened".into());
                    } else if screen == Some(LIVE_MENU) && self.held(now, SETTLE_MS) {
                        self.queue = self.to_row(BROWSER_ROW, "Custom Game Browser");
                        self.step = Step::ToBrowser {
                            at: now,
                            tries: tries + 1,
                        };
                    } else if screen != Some(LIVE_MENU) {
                        self.queue = vec![press("b", "back to the Xbox Live menu")];
                        self.step = Step::ToBrowser {
                            at: now,
                            tries: tries + 1,
                        };
                    }
                }
            }
            Step::Browsing {
                since,
                pressed,
                tries,
            } => {
                let probed = self
                    .probed
                    .is_some_and(|p| now.saturating_sub(p) >= SETTLE_MS);
                match pressed {
                    None if probed || now.saturating_sub(since) > STEP_MS => {
                        self.queue = vec![press("a", "the first game in the browser")];
                        self.step = Step::Browsing {
                            since,
                            pressed: Some(now),
                            tries,
                        };
                    }
                    Some(p) if now.saturating_sub(p) > STEP_MS => {
                        if tries + 1 >= TRIES {
                            self.stuck("the browser's game never let it join".into());
                        } else if screen == Some(BROWSER) || screen == Some(LIVE_MENU) {
                            self.queue = vec![press("a", "the first game in the browser, again")];
                            self.step = Step::Browsing {
                                since,
                                pressed: Some(now),
                                tries: tries + 1,
                            };
                        } else {
                            // A dialog (unable to join): close it.
                            self.queue = vec![press("a", "close a dialog")];
                            self.step = Step::Browsing {
                                since,
                                pressed: Some(now),
                                tries: tries + 1,
                            };
                        }
                    }
                    _ => {}
                }
            }
            Step::Done | Step::Stuck => {}
        }
    }

    /// On the main menu: DOWN to Xbox Live, A.
    fn go_live(&mut self, now: u64, tries: u32) {
        let mut q: Vec<Press> = (0..MAIN_LIVE_ROW)
            .map(|_| press("down", "to Xbox Live"))
            .collect();
        q.push(press("a", "Xbox Live (signs in)"));
        self.queue = q;
        self.step = Step::ToLive { at: now, tries };
    }

    /// On the Live menu: what this game does there.
    fn at_live_menu(&mut self, now: u64) {
        match self.role {
            Role::Host { .. } => {
                self.queue = self.to_row(COOP_ROW, "Co-op Campaign");
                self.step = Step::Hosting { at: now, tries: 0 };
            }
            Role::Guest | Role::Late if !self.fallback => {
                self.fallback = true;
                self.queue = vec![press(
                    "b",
                    "signed in; back to the main menu for the invitation",
                )];
                self.step = Step::Waiting {
                    since: now,
                    nudged: false,
                };
            }
            Role::Guest | Role::Late => {
                self.queue = self.to_row(BROWSER_ROW, "Custom Game Browser");
                self.step = Step::ToBrowser { at: now, tries: 0 };
            }
        }
    }

    /// The log line for `p`, sent on this game's current screen.
    pub fn press_line(&self, p: &Press) -> String {
        let mut s = format!(
            "coop-drive: {}: press {} on screen {}",
            self.tag,
            p.button,
            show(self.screen)
        );
        if self.screen == Some(LIVE_MENU)
            && let Some(r) = p.row
        {
            s.push_str(&format!(" row {r}"));
        }
        s.push_str(&format!(" ({})", p.why));
        s
    }
}

fn press(button: &'static str, why: &str) -> Press {
    Press {
        button,
        why: why.into(),
        row: None,
    }
}

/// Back from the players' screen to its main-menu row, then up to Xbox Live.
pub(crate) fn back_to_live(mode: u32, main_menu_theater: bool) -> Vec<Press> {
    let (mode, label) = match mode {
        MODE_SPLIT => (Mode::SplitScreen, "Split Screen"),
        MODE_SYSTEM_LINK => (Mode::SystemLink, "System Link"),
        _ => return vec![press("b", &format!("the players' screen in mode {mode}"))],
    };
    let mut presses = vec![press("b", &format!("that was {label}"))];
    presses.extend((MAIN_LIVE_ROW..mode.row(main_menu_theater)).map(|_| press("up", "to Xbox Live")));
    presses.push(press("a", "Xbox Live"));
    presses
}

fn show(screen: Option<u32>) -> String {
    screen.map_or_else(|| "none".into(), |s| format!("0x{s:02x}"))
}

/// A `ui: screen` line's screen, Live menu row and players' screen mode
/// (`boot::ui_watch::Read::line`).
pub fn parse_screen(text: &str) -> Option<(Option<u32>, Option<u32>, Option<u32>)> {
    let rest = text.strip_prefix("ui: screen ")?;
    if rest.starts_with("none") {
        return Some((None, None, None));
    }
    let hex = rest.strip_prefix("0x")?;
    let end = hex
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(hex.len());
    let id = u32::from_str_radix(&hex[..end], 16).ok()?;
    let after = |key: &str| {
        rest.split_once(key).and_then(|(_, r)| {
            r.split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|n| n.parse().ok())
        })
    };
    Some((Some(id), after("live menu row "), after(", mode ")))
}

/// The `input.pad` params for one press: held 150 ms and two reads, so a
/// slow frame still sees it.
pub fn pad_params(p: &Press) -> serde_json::Value {
    serde_json::json!({ "steps": [agent_proto::sim::menu::press_step(p.button)] })
}

#[cfg(test)]
mod tests;
