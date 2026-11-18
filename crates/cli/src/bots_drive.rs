//! `h2 bots test`'s menu driver: from the main menu to a Live custom
//! game's pregame lobby and Start Game, on what the game says (docs/game/
//! bots.md, "Testing"). The game runs with `H2_UI_WATCH=1`, so it prints
//! `ui: screen 0x..` whenever player one's screen changes; the harness
//! feeds those lines, the bots' lines and the agent API's phase
//! (`session.status`) to a [`Driver`], and sends each press it asks for as
//! `input.pad` on the game's agent socket. Every press waits on a screen,
//! a line or the phase, and every wait has a timeout; each press is said
//! as `bots-drive: press a on screen 0xba row 2 (Custom Game)`.
//!
//! The path, as `h2 coop test --drive`'s host takes it
//! ([`crate::coop_drive`]), with the Live menu's retail Custom Game row
//! (row 2, `boot::custom_games::RETAIL_CUSTOM_GAME_ROW`) in place of
//! Co-op Campaign:
//!
//! * the main menu (screen 9, or no screen once the game said
//!   `coop-live: lobby mode` or has been in phase `menu` a while): DOWN,
//!   A for Xbox Live, which opens the players' screen (`0x1e`) in mode 3
//!   (the local bots join there); A there opens the Xbox Live menu
//!   (`0xba`); a wrong main menu row shows in what it opened and is
//!   backed out of;
//! * the Live menu: from its restored row to row 2, A; the lobby is up on
//!   `coop-live: lobby mode 2` (or the session's `live: session ...
//!   advertised`);
//! * the lobby: once every local bot is on Xbox Live (`bots: NAME on port
//!   P is on Xbox Live as ...`), or after [`LOBBY_WAIT_MS`], A on Start
//!   Game (row 0); again (UP, A) if the phase hasn't left `menu` in
//!   [`START_MS`].

use std::collections::BTreeSet;

use crate::coop_drive::{
    BOOT_SETTLE_MS, BOOT_WAIT_MS, CAMPAIGN_SCREENS, GAP_MS, JOIN_SCREEN, LIVE_MENU, MAIN_LIVE_ROW,
    MAIN_MENU, MODE_LIVE, MODE_SPLIT, MODE_SYSTEM_LINK, Press, SETTLE_MS, STEP_MS, TRIES,
    parse_screen,
};

/// The Xbox Live menu's Custom Game row (retail row 2).
pub const CUSTOM_GAME_ROW: u32 = 2;
/// The phase `menu` with no screen this long counts as the main menu.
pub const MENU_PHASE_MS: u64 = 5_000;
/// How long the host waits in the lobby for its local bots to be on Xbox
/// Live before it starts anyway (the verdict then says which did not).
pub const LOBBY_WAIT_MS: u64 = 90_000;
/// How long Start Game may take to leave the menus (the countdown, then
/// the load).
pub const START_MS: u64 = 40_000;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Boot,
    ToLive { at: u64, tries: u32 },
    Hosting { at: u64, tries: u32 },
    Lobby { since: u64 },
    Starting { at: u64, tries: u32 },
    Done,
    Stuck,
}

/// The host's driver (module docs). Time is milliseconds since the run
/// started.
#[derive(Debug, Clone)]
pub struct Driver {
    /// Local bots to wait for in the lobby.
    local: usize,
    step: Step,
    screen: Option<u32>,
    screen_since: u64,
    mode: Option<u32>,
    /// The game said `coop-live: lobby mode`: it has been at the main menu.
    menu_seen: bool,
    /// The agent API's phase and since when.
    phase: Option<String>,
    phase_since: u64,
    first_poll: Option<u64>,
    acted: Option<u64>,
    cursor: Option<u32>,
    queue: Vec<Press>,
    last_press: u64,
    boot: Vec<(Option<u32>, u32)>,
    /// Ports whose bot is on Xbox Live (as an account or a guest).
    on_live: BTreeSet<u8>,
    hosting: bool,
    said: BTreeSet<String>,
    out: Vec<String>,
}

fn press(button: &'static str, why: &str) -> Press {
    Press {
        button,
        why: why.into(),
        row: None,
    }
}

fn show(screen: Option<u32>) -> String {
    screen.map_or_else(|| "none".into(), |s| format!("0x{s:02x}"))
}

/// `bots: NAME on port P is on Xbox Live as ...`: the port.
fn live_port(text: &str) -> Option<u8> {
    let rest = text.strip_prefix("bots: ")?;
    let (_, after) = rest.rsplit_once(" on port ")?;
    let (port, after) = after.split_once(' ')?;
    after
        .starts_with("is on Xbox Live as")
        .then(|| port.parse::<u8>().ok())?
}

impl Driver {
    pub fn new(local: usize) -> Self {
        Driver {
            local,
            step: Step::Boot,
            screen: None,
            screen_since: 0,
            mode: None,
            menu_seen: false,
            phase: None,
            phase_since: 0,
            first_poll: None,
            acted: None,
            cursor: None,
            queue: Vec::new(),
            last_press: 0,
            boot: Vec::new(),
            on_live: BTreeSet::new(),
            hosting: false,
            said: BTreeSet::new(),
            out: Vec::new(),
        }
    }

    pub fn done(&self) -> bool {
        matches!(self.step, Step::Done | Step::Stuck)
    }

    pub fn stuck(&self) -> bool {
        self.step == Step::Stuck
    }

    /// A line of the game's.
    pub fn line(&mut self, now: u64, text: &str) {
        if let Some((screen, row, mode)) = parse_screen(text) {
            self.mode = mode;
            if screen != self.screen {
                self.screen = screen;
                self.screen_since = now;
                self.cursor =
                    (screen == Some(LIVE_MENU)).then(|| row.filter(|r| *r <= 5).unwrap_or(0));
            }
            return;
        }
        if text.starts_with("coop-live: lobby mode ") {
            self.menu_seen = true;
        }
        if text.starts_with("coop-live: lobby mode 2")
            || (text.starts_with("live: session ") && text.contains(" advertised"))
        {
            self.hosting = true;
        }
        if let Some(p) = live_port(text) {
            self.on_live.insert(p);
        }
        match self.step {
            Step::Hosting { .. } if self.hosting => {
                self.queue.clear();
                self.step = Step::Lobby { since: now };
                self.say("in the custom game lobby".into());
            }
            Step::Hosting { .. } if text.starts_with("custom games: opening the browser") => {
                self.queue = vec![press("b", "back from the browser: that was row 3")];
            }
            _ => {}
        }
    }

    /// The agent API's phase (`session.status`), when it changes.
    pub fn status(&mut self, now: u64, phase: &str) {
        if self.phase.as_deref() != Some(phase) {
            self.phase = Some(phase.to_string());
            self.phase_since = now;
        }
        if matches!(self.step, Step::Starting { .. }) && phase != "menu" {
            self.queue.clear();
            self.step = Step::Done;
            self.say(format!("done: the game left the menus (phase {phase})"));
        }
    }

    fn say(&mut self, what: String) {
        if self.said.insert(what.clone()) {
            self.out.push(what);
        }
    }

    /// What it has to say since last asked (`bots-drive: ...`).
    pub fn take_said(&mut self) -> Vec<String> {
        std::mem::take(&mut self.out)
            .into_iter()
            .map(|s| format!("bots-drive: {s}"))
            .collect()
    }

    fn give_up(&mut self, why: String) {
        self.queue.clear();
        self.step = Step::Stuck;
        self.say(format!("stuck: {why}"));
    }

    fn held(&self, now: u64, ms: u64) -> bool {
        now.saturating_sub(self.screen_since) >= ms
    }

    fn at_main(&self, now: u64) -> bool {
        self.screen == Some(MAIN_MENU)
            || (self.screen.is_none()
                && (self.menu_seen
                    || (self.phase.as_deref() == Some("menu")
                        && now.saturating_sub(self.phase_since) >= MENU_PHASE_MS)))
    }

    fn fresh(&mut self, now: u64) -> bool {
        if !self.held(now, SETTLE_MS) || self.acted == Some(self.screen_since) {
            return false;
        }
        self.acted = Some(self.screen_since);
        true
    }

    /// On the way to the Live menu: the players' screen goes on in Xbox
    /// Live mode, a wrong main menu row is backed out of. Whether the
    /// screen is one of those.
    fn on_the_way(&mut self, now: u64) -> bool {
        let campaign = self.screen.is_some_and(|s| CAMPAIGN_SCREENS.contains(&s));
        if self.screen != Some(JOIN_SCREEN) && !campaign {
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
            (false, Some(MODE_LIVE) | None) => vec![press(
                "a",
                "the players' screen for Xbox Live: on to the Live menu",
            )],
            (false, Some(MODE_SPLIT)) => vec![
                press("b", "that was Split Screen"),
                press("up", "to Xbox Live"),
                press("a", "Xbox Live"),
            ],
            (false, Some(MODE_SYSTEM_LINK)) => vec![
                press("b", "that was System Link"),
                press("up", "to Xbox Live"),
                press("up", "to Xbox Live"),
                press("a", "Xbox Live"),
            ],
            (false, Some(m)) => vec![press("b", &format!("the players' screen in mode {m}"))],
        };
        true
    }

    /// From the Live menu's cursor to `row`, then A.
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

    fn at_live_menu(&mut self, now: u64) {
        self.queue = self.to_row(CUSTOM_GAME_ROW, "Custom Game");
        self.step = Step::Hosting { at: now, tries: 0 };
    }

    fn plan(&mut self, now: u64) {
        let screen = self.screen;
        match self.step.clone() {
            Step::Boot => {
                let first = *self.first_poll.get_or_insert(now);
                if self.hosting {
                    self.step = Step::Lobby { since: now };
                    return;
                }
                if self.at_main(now) {
                    if self.held(now, SETTLE_MS) {
                        let mut q: Vec<Press> = (0..MAIN_LIVE_ROW)
                            .map(|_| press("down", "to Xbox Live"))
                            .collect();
                        q.push(press("a", "Xbox Live (signs in; the bots join here)"));
                        self.queue = q;
                        self.step = Step::ToLive { at: now, tries: 0 };
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
                if now.saturating_sub(first) < BOOT_WAIT_MS || !self.held(now, BOOT_SETTLE_MS) {
                    return;
                }
                let n = match self.boot.iter_mut().find(|(s, _)| *s == screen) {
                    Some((_, n)) => n,
                    None => {
                        self.boot.push((screen, 0));
                        &mut self.boot.last_mut().expect("pushed").1
                    }
                };
                let k = *n;
                *n += 1;
                self.screen_since = now;
                self.queue = match k {
                    0 => vec![press("start", "no main menu after a minute")],
                    1 => vec![press("a", "no main menu after a minute")],
                    2 => vec![press("b", "no main menu after a minute")],
                    _ => {
                        self.give_up(format!(
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
                        self.give_up(format!(
                            "no Xbox Live menu after {TRIES} tries (on {})",
                            show(screen)
                        ));
                    } else {
                        self.queue = if self.at_main(now) {
                            vec![press("a", "Xbox Live, again")]
                        } else {
                            vec![press("b", "a screen on the way to Xbox Live")]
                        };
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
                        self.give_up("Custom Game never opened a lobby".into());
                    } else if screen == Some(LIVE_MENU) && self.held(now, SETTLE_MS) {
                        self.queue = self.to_row(CUSTOM_GAME_ROW, "Custom Game, again");
                        self.step = Step::Hosting {
                            at: now,
                            tries: tries + 1,
                        };
                    } else if screen.is_some() && screen != Some(LIVE_MENU) {
                        self.queue = vec![press("b", "back to the Xbox Live menu")];
                        self.step = Step::Hosting {
                            at: now,
                            tries: tries + 1,
                        };
                    }
                }
            }
            Step::Lobby { since } => {
                let all = self.on_live.len() >= self.local;
                let late = now.saturating_sub(since) > LOBBY_WAIT_MS;
                if (all || late) && now.saturating_sub(since) >= SETTLE_MS {
                    let why = if all {
                        format!("Start Game: {} local bots on Xbox Live", self.on_live.len())
                    } else {
                        self.say(format!(
                            "only {} of {} local bots on Xbox Live after {} s; starting anyway",
                            self.on_live.len(),
                            self.local,
                            LOBBY_WAIT_MS / 1000
                        ));
                        "Start Game: not every bot came".into()
                    };
                    self.queue = vec![press("a", &why)];
                    self.step = Step::Starting { at: now, tries: 0 };
                }
            }
            Step::Starting { at, tries } => {
                if now.saturating_sub(at) > START_MS {
                    if tries + 1 >= TRIES {
                        self.give_up(
                            "Start Game didn't start the game (the start state says why)".into(),
                        );
                    } else {
                        self.queue =
                            vec![press("up", "to Start Game"), press("a", "Start Game again")];
                        self.step = Step::Starting {
                            at: now,
                            tries: tries + 1,
                        };
                    }
                }
            }
            Step::Done | Step::Stuck => {}
        }
    }

    /// The log line for `p`, sent on the current screen.
    pub fn press_line(&self, p: &Press) -> String {
        let mut s = format!(
            "bots-drive: press {} on screen {}",
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

#[cfg(test)]
mod tests;
