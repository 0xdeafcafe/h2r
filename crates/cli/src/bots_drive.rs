//! `h2 bots test`'s menu driver: from boot to a Live custom game's
//! pregame lobby and Start Game, on what the game says (docs/game/
//! bots.md, "Testing"). The game runs with `H2_UI_WATCH=1`, so it prints
//! `ui: screen ...` whenever player one's screen changes; the harness
//! feeds those lines, the bots' lines and the agent API's phase
//! (`session.status`) to a [`Driver`], and sends each press it asks for as
//! `input.pad` on the game's agent socket. Every press waits on a screen,
//! a line or the phase, and every wait has a timeout; each press is said
//! as `bots-drive: press a on screen 0xba row 2 (Custom Game)`.
//!
//! What the screens read as (`boot::ui_watch`, and the first real run,
//! 20261118-232735): the main menu, the title screen, the boot's profile
//! dialogs and the pregame lobby all read as `ui: screen none (slot 4:
//! window W active ... list [mask/value]; ...)`. Windows 0 to 4 hold
//! dialogs, window 5 the screens. The main menu is the one the game names
//! with `coop-live: lobby mode 4294967295` (the lobby mode it sets on
//! reaching it; a custom game's lobby sets 2, Live or not), and on it the
//! list value's low 16 bits are the cursor's row (`0xe16d0000` on row 0,
//! `0xe16f0002` on row 2 in that run). The rows are 0 Campaign, 1 Xbox
//! Live, 2 Split Screen, 3 System Link, 4 Settings (`0x230827`).
//!
//! The path, as `h2 coop test --drive`'s host takes it
//! ([`crate::coop_drive`]), with the Live menu's retail Custom Game row
//! (row 2) in place of Co-op Campaign:
//!
//! * boot: a fresh data folder stops on the title screen and the new
//!   profile's dialogs, so once the screen has held [`BOOT_HOLD_MS`]
//!   without the main menu, A (at most [`BOOT_PRESSES`]);
//! * the account picker (`dialog_live_signin`, dialog `0x14`): the game
//!   opens it at boot (a new profile) and from the main menu's Xbox Live
//!   row while the profile is not signed in (`0x2308e0`). Its rows
//!   (`0x24af1a`): row 0 is the no-account row (item `0x1023`, there
//!   while not signed in; A on it closes the picker signed out, which is
//!   what run 20261119-001305 did four times), then one row per account
//!   `XOnlineGetUsers` lists (the run's `live/accounts.json`: the tester),
//!   then New Account. A on an account row (`0x24b2ea`) copies it into
//!   the profile and logs on (`0x24b664`). So: DOWN to row 1, A. The
//!   dialog is known by the bots' join gate line (`dialog 0x14 (window
//!   3, ...)`), its cursor by the list value's low 16 bits;
//! * the main menu: from its row to Xbox Live (row 1), A; signed in, that
//!   opens the players' screen (`0x1e`) in mode 3 (the local bots no
//!   longer join there: they join in the lobby, docs/game/bots.md §4.3);
//!   A there opens the Xbox Live menu (`0xba`). Not signed in, it
//!   opens the picker first. Another dialog on the way gets A; a players'
//!   screen in another mode is backed out of and corrected; Live asking
//!   for a 6-digit code stops the drive;
//! * the Live menu: a window 5 `none` screen with list mask `0xf` when
//!   four local players are signed in, else the screen that settles first
//!   after A on the players' screen (the mask is the signed-in players',
//!   `0x1` alone, as the main menu's), under the main menu's lobby mode still; the
//!   main menu is told by its screen address too, learnt when the game
//!   sets lobby mode -1 (run 20261119-003044 took the Live menu for the
//!   main menu and went into OptiMatch). One press at a time from the row
//!   its list value shows to row 2 (Custom Game), A; the lobby is the Live
//!   custom game only once the HLE says `live: session ... advertised`
//!   (not since deleted) in lobby mode 2. A lobby in another mode
//!   (OptiMatch's 3) gets B;
//! * the lobby: once every local bot is on Xbox Live (`bots: NAME on port
//!   P is on Xbox Live as ...`), or after [`LOBBY_WAIT_MS`], A on Start
//!   Game (row 0); again (UP, A) if the phase hasn't left `menu` in
//!   [`START_MS`].
//!
//! [`Driver::clankermatch`] takes the same way to the Live menu, then its
//! Clankermatch row (row 2) and a clanker playlist instead
//! ([`clankermatch`]).

use std::collections::BTreeSet;

use crate::coop_drive::{
    CAMPAIGN_SCREENS, GAP_MS, JOIN_SCREEN, LIVE_MENU, MAIN_LIVE_ROW, MAIN_MENU, MODE_LIVE,
    MODE_SPLIT, MODE_SYSTEM_LINK, Press, SETTLE_MS, STEP_MS, TRIES, parse_screen,
};

/// The Xbox Live menu's Custom Game row (retail row 2). The rows, with
/// ours (`boot::custom_games`): 0 Quickmatch, 1 OptiMatch, 2 Clankermatch,
/// 3 Co-op Campaign, 4 Custom Game, 5 Custom Game Browser, 6 Content
/// Download.
pub const CUSTOM_GAME_ROW: u32 = 4;
/// The Xbox Live menu's last row.
pub const LIVE_LAST_ROW: u32 = 6;
/// The Xbox Live menu's list mask on its `ui: screen none` line
/// (`list [0xf/...]`); the main menu's, the OptiMatch playlists' and the
/// lobby's are `0x1` (run 20261119-003044).
pub const LIVE_MENU_MASK: u32 = 0xf;
/// The lobby mode Custom Game sets (`0x199a03(2)`); OptiMatch's lobby
/// set 3 in run 20261119-003044.
pub const CUSTOM_GAME_MODE: u32 = 2;
/// Presses on the Live menu toward Custom Game before giving up.
pub const LIVE_MENU_PRESSES: u32 = 10;
/// The lobby mode the game sets on reaching the main menu (-1).
pub const MAIN_MENU_MODE: u32 = u32::MAX;
/// A boot screen (title, profile dialog) held this long gets A.
pub const BOOT_HOLD_MS: u64 = 4_000;
/// Boot presses before the driver gives up on reaching the main menu.
pub const BOOT_PRESSES: u32 = 12;
/// A dialog on the way to the Live menu held this long gets A.
pub const DIALOG_HOLD_MS: u64 = 3_000;
/// Dialog presses on the way to the Live menu before giving up.
pub const DIALOG_PRESSES: u32 = 6;
/// The account picker, `dialog_live_signin` (screen 20, loader
/// `0x24b4a9`).
pub const ACCOUNT_PICKER: u32 = 0x14;
/// The picker's first account row; row 0 is the no-account row.
pub const PICKER_ACCOUNT_ROW: u32 = 1;
/// Accounts picked before the driver gives up on signing in.
pub const PICKER_PICKS: u32 = 3;
/// How long the host waits in the lobby for its local bots to be on Xbox
/// Live before it starts anyway (the verdict then says which did not).
pub const LOBBY_WAIT_MS: u64 = 90_000;
/// How long Start Game may take to leave the menus (the countdown, then
/// the load).
pub const START_MS: u64 = 40_000;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Boot,
    ToLive {
        at: u64,
        tries: u32,
    },
    /// On the Live menu: one press at a time to Custom Game, each read
    /// back off the next `ui: screen` line.
    ToCustom {
        at: u64,
        presses: u32,
        tries: u32,
    },
    Hosting {
        at: u64,
        tries: u32,
    },
    Lobby {
        since: u64,
    },
    Starting {
        at: u64,
        tries: u32,
    },
    /// Clankermatch ([`clankermatch`]): A on the Live menu's Clankermatch
    /// row went at `at`; on the playlist list that opens, to the planned
    /// playlist's row (`downs` DOWN presses sent when the row can't be
    /// read), A.
    Playlists {
        at: u64,
        tries: u32,
        downs: u32,
        presses: u32,
    },
    /// A on the playlist went at `at`; waiting for the clanker search.
    Searching {
        at: u64,
        tries: u32,
    },
    /// The search runs (since `since`); waiting for this console to host
    /// the clanker playlist's lobby.
    Matching {
        since: u64,
    },
    /// Hosting the Clankermatch lobby (since `since`): nothing to press,
    /// the bots fill it and its countdown starts the game.
    MatchLobby {
        since: u64,
    },
    /// The game runs (`played`: phase gameplay seen); it ends back in the
    /// menus.
    Playing {
        played: bool,
    },
    /// The game is over: B (A on a dialog) until this console hosts no
    /// session, so the fill ends and the bots go. `last_b`: the last press
    /// was B; `dialogs`: dialogs answered on the way out.
    Leaving {
        presses: u32,
        last_b: bool,
        dialogs: u32,
    },
    Done,
    Stuck,
}

/// What the drive goes for on the Xbox Live menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Custom Game (row 4), Start Game in its lobby.
    CustomGame,
    /// Clankermatch (row 2), the clanker playlist on `row` of its list,
    /// the game, then out again ([`clankermatch`]).
    Clankermatch { row: u32 },
}

/// The host's driver (module docs). Time is milliseconds since the run
/// started.
#[derive(Debug, Clone)]
pub struct Driver {
    /// Local bots to wait for in the lobby.
    local: usize,
    target: Target,
    step: Step,
    /// The screen id (`None` for every `ui: screen none` line).
    screen: Option<u32>,
    screen_since: u64,
    /// A `none` line's window (5: a screen; 0 to 4: a dialog).
    window: Option<u32>,
    /// A `none` line's first list value.
    list_value: Option<u32>,
    /// A `none` line's first list mask (`0xf` on the Live menu).
    list_mask: Option<u32>,
    /// A `none` line's window's active screen.
    active: Option<u32>,
    /// The main menu's screen (its window 5 active address), learnt when
    /// the game says `coop-live: lobby mode 4294967295`: the Live menu,
    /// the OptiMatch playlists and the lobby are other window 5 screens
    /// under that lobby mode.
    main_screen: Option<u32>,
    /// The main menu's lobby mode came; its screen is the next window 5
    /// line's.
    learn_main: bool,
    /// The Xbox Live menu's screen (its window 5 active address), learnt
    /// from the first settled window 5 screen after A on the players'
    /// screen in Xbox Live mode. Its list mask is the signed-in local
    /// players' (`0xf` with three local bots signed in there, `0x1` alone),
    /// so the mask alone can't tell it from the main menu (run
    /// 20261119-013507).
    live_screen: Option<u32>,
    /// A on the players' screen for Xbox Live went; the next settled
    /// window 5 screen is the Live menu's.
    learn_live: bool,
    /// The top dialog the bots' join gate last named (`dialog 0x14
    /// (window 3, ...)`); `None` for `dialog none`.
    dialog_id: Option<u32>,
    /// Account rows picked in the account picker.
    picks: u32,
    /// DOWN presses in the picker since its last row change.
    picker_downs: u32,
    /// When the `ui: screen` line the last account pick was made on came.
    picked_on: Option<u64>,
    /// The last `ui: screen` line, and when it came.
    ui: String,
    ui_since: u64,
    mode: Option<u32>,
    /// The last `coop-live: lobby mode N`.
    lobby_mode: Option<u32>,
    phase: Option<String>,
    acted: Option<u64>,
    cursor: Option<u32>,
    queue: Vec<Press>,
    last_press: u64,
    boot_presses: u32,
    dialog_presses: u32,
    /// Ports whose bot is on Xbox Live (as an account or a guest).
    on_live: BTreeSet<u8>,
    /// The HLE advertised a hosted session (and has not deleted it).
    hosting: bool,
    /// Clankermatch: the HLE's clanker search started
    /// (`live: clankermatch: searching clanker playlist ...`).
    cm_search: bool,
    /// Clankermatch: a human playlist's search line, if one came.
    human_search: Option<String>,
    /// Clankermatch: the Clankers driver fills a hosted clanker lobby
    /// (`clankers: clankermatch: hosting clanker playlist ...`).
    cm_hosting: bool,
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

/// A `ui: screen none (slot 4: window W ... list [M/V ...]; ...)` line's
/// window and first list value.
pub fn none_window(text: &str) -> Option<(u32, Option<u32>)> {
    let rest = text.strip_prefix("ui: screen none (slot 4: window ")?;
    let (w, rest) = rest.split_once(' ')?;
    let window = w.parse().ok()?;
    let value = rest
        .split_once("list [")
        .and_then(|(_, r)| r.split_once('/'))
        .and_then(|(_, r)| {
            let v = r.trim_start_matches("0x");
            let end = v.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(v.len());
            u32::from_str_radix(&v[..end], 16).ok()
        });
    Some((window, value))
}

/// A `ui: screen none` line's parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoneLine {
    pub window: u32,
    pub active: Option<u32>,
    pub pending: Option<u32>,
    pub mask: Option<u32>,
    pub value: Option<u32>,
}

fn hex_after(text: &str, key: &str) -> Option<u32> {
    let (_, r) = text.split_once(key)?;
    let v = r.trim_start_matches("0x");
    let end = v.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(v.len());
    u32::from_str_radix(&v[..end], 16).ok()
}

/// A `ui: screen none (slot 4: window W active A pending P list [M/V];
/// ...)` line: slot 4's part.
pub fn none_line(text: &str) -> Option<NoneLine> {
    let (window, value) = none_window(text)?;
    let slot4 = text.split(';').next().unwrap_or(text);
    Some(NoneLine {
        window,
        active: hex_after(slot4, " active "),
        pending: hex_after(slot4, " pending "),
        mask: hex_after(slot4, "list ["),
        value,
    })
}

/// `live: session ID deleted`: the hosted session is gone.
pub fn deleted(text: &str) -> bool {
    text.starts_with("live: session ") && text.ends_with(" deleted")
}

/// A `bots: join gate port P: ..., dialog D, ...` line's dialog: `Some(None)`
/// for `dialog none`, `Some(Some(id))` for `dialog 0xNN (window ...)`.
pub fn gate_dialog(text: &str) -> Option<Option<u32>> {
    let rest = text.strip_prefix("bots: join gate port ")?;
    let (_, d) = rest.split_once(", dialog ")?;
    if d.starts_with("none") {
        return Some(None);
    }
    let hex = d.strip_prefix("0x")?;
    let end = hex
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(hex.len());
    Some(u32::from_str_radix(&hex[..end], 16).ok())
}

/// A players' screen mode by name, for the log.
pub fn mode_name(mode: u32) -> &'static str {
    match mode {
        MODE_SPLIT => "Split Screen",
        MODE_SYSTEM_LINK => "System Link",
        MODE_LIVE => "Xbox Live",
        _ => "another mode",
    }
}

/// The `live: session ... advertised` line: a hosted Live session.
pub fn advertised(text: &str) -> bool {
    text.starts_with("live: session ") && text.contains(" advertised")
}

impl Driver {
    pub fn new(local: usize) -> Self {
        Driver {
            local,
            target: Target::CustomGame,
            step: Step::Boot,
            screen: None,
            screen_since: 0,
            window: None,
            list_value: None,
            list_mask: None,
            active: None,
            main_screen: None,
            learn_main: false,
            live_screen: None,
            learn_live: false,
            dialog_id: None,
            picks: 0,
            picker_downs: 0,
            picked_on: None,
            ui: String::new(),
            ui_since: 0,
            mode: None,
            lobby_mode: None,
            phase: None,
            acted: None,
            cursor: None,
            queue: Vec::new(),
            last_press: 0,
            boot_presses: 0,
            dialog_presses: 0,
            on_live: BTreeSet::new(),
            hosting: false,
            cm_search: false,
            human_search: None,
            cm_hosting: false,
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
            if text != self.ui {
                self.ui = text.to_string();
                self.ui_since = now;
            }
            let none = none_line(text);
            let (window, value) = none.map_or((None, None), |n| (Some(n.window), n.value));
            if value != self.list_value || window != self.window {
                self.picker_downs = 0;
            }
            self.window = window;
            self.list_value = value;
            self.list_mask = none.and_then(|n| n.mask);
            self.active = none.and_then(|n| n.active);
            self.mode = mode;
            if self.learn_live
                && let Some(n) = none
                && n.window == 5
                && n.pending.is_none_or(|p| p == 0)
                && let Some(a) = n.active
                && Some(a) != self.main_screen
            {
                self.live_screen = Some(a);
                self.learn_live = false;
            }
            if self.learn_main
                && let Some(n) = none
                && n.window == 5
                && n.mask != Some(LIVE_MENU_MASK)
                && (n.active.is_none() || n.active != self.live_screen)
            {
                // The main menu on screen, or on its way in (pending).
                self.main_screen = match n.pending.filter(|p| *p != 0) {
                    Some(p) => Some(p),
                    None if n.mask == Some(1) => n.active,
                    None => None,
                };
                self.learn_main = self.main_screen.is_none();
            }
            if screen != self.screen || screen.is_none() {
                self.screen = screen;
                self.screen_since = now;
            }
            self.cursor = if screen == Some(LIVE_MENU) {
                Some(row.filter(|r| *r <= LIVE_LAST_ROW).unwrap_or(0))
            } else if self.live_menu() {
                value.map(|v| v & 0xffff).filter(|r| *r <= LIVE_LAST_ROW)
            } else {
                None
            };
            return;
        }
        if let Some(d) = gate_dialog(text) {
            self.dialog_id = d;
            return;
        }
        if let Some(m) = text.strip_prefix("coop-live: lobby mode ") {
            self.lobby_mode = m.split(' ').next().and_then(|m| m.parse().ok());
            if self.lobby_mode == Some(MAIN_MENU_MODE) {
                // The main menu: the window 5 screen now, or the one on
                // its way in.
                self.main_screen = None;
                self.learn_main = true;
                if self.screen.is_none() && self.window == Some(5) {
                    let ui = self.ui.clone();
                    if let Some(n) = none_line(&ui) {
                        self.main_screen = match n.pending.filter(|p| *p != 0) {
                            Some(p) => Some(p),
                            None if n.mask == Some(1) => n.active,
                            None => None,
                        };
                        self.learn_main = self.main_screen.is_none();
                    }
                }
            }
        }
        if advertised(text) {
            self.hosting = true;
        }
        if deleted(text) {
            self.hosting = false;
        }
        if let Some(p) = live_port(text) {
            self.on_live.insert(p);
        }
        if text.starts_with(clankermatch::SEARCH) {
            self.cm_search = true;
        }
        if text.starts_with(clankermatch::HUMAN_SEARCH) && self.human_search.is_none() {
            self.human_search = Some(text.to_string());
        }
        if text.starts_with(clankermatch::HOSTING) {
            self.cm_hosting = true;
        }
        if text.contains("needs a code to sign in") {
            self.give_up(
                "Xbox Live asks for the account's 6-digit code (the run's Live directory has no \
                 key for it)"
                    .into(),
            );
            return;
        }
        match self.step {
            Step::Hosting { .. } | Step::ToCustom { .. }
                if self.target == Target::CustomGame && self.hosting_live() =>
            {
                self.queue.clear();
                self.step = Step::Lobby { since: now };
                self.say("in the Live custom game lobby".into());
            }
            Step::Hosting { .. } if text.starts_with("custom games: opening the browser") => {
                self.queue = vec![press("b", "back from the browser: that was row 4")];
            }
            _ => {}
        }
    }

    /// The agent API's phase (`session.status`), when it changes.
    pub fn status(&mut self, now: u64, phase: &str) {
        if self.phase.as_deref() != Some(phase) {
            self.phase = Some(phase.to_string());
        }
        if matches!(self.step, Step::Starting { .. }) && phase != "menu" {
            self.queue.clear();
            self.step = Step::Done;
            self.say(format!("done: the game left the menus (phase {phase})"));
        }
        self.match_phase(now, phase);
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

    /// No press, and no screen change, for `ms`.
    fn quiet(&self, now: u64, ms: u64) -> bool {
        now.saturating_sub(self.ui_since) >= ms && now.saturating_sub(self.last_press) >= ms
    }

    /// A dialog (windows 0 to 4) takes player one's input.
    fn dialog(&self) -> bool {
        self.screen.is_none() && self.window.is_some_and(|w| w < 5)
    }

    /// The account picker is up and takes player one's input.
    fn picker(&self) -> bool {
        self.dialog() && self.dialog_id == Some(ACCOUNT_PICKER)
    }

    /// In the account picker: DOWN off the no-account row, then A on the
    /// first account. Whether the picker is up.
    fn in_picker(&mut self, now: u64) -> bool {
        if !self.picker() {
            return false;
        }
        if self.picked_on == Some(self.ui_since) {
            // A sent on this opening: it closes, or it is stuck.
            if now.saturating_sub(self.last_press) > 2 * STEP_MS {
                let ui = self.ui.clone();
                self.give_up(format!(
                    "the account picker (dialog 0x14) stayed up after A on its account row; \
                     the last screen: {ui}"
                ));
            }
            return true;
        }
        if !self.quiet(now, SETTLE_MS) {
            return true;
        }
        if self.picks >= PICKER_PICKS {
            self.give_up(format!(
                "the account picker (dialog 0x14) is back after {PICKER_PICKS} picks of its \
                 account row: the game never signed in to Xbox Live (if the cursor never left \
                 row 0, the picker lists no account: XOnlineGetUsers reads the run's \
                 live/accounts.json; otherwise the logon failed, and the game's full log has \
                 the HLE's `live:` lines)"
            ));
            return true;
        }
        let row = self.list_value.map(|v| v & 0xffff);
        let why = match row {
            Some(r) if r >= PICKER_ACCOUNT_ROW => {
                format!("the account picker's account row {r}: sign in")
            }
            _ if self.picker_downs >= 1 => {
                self.say(
                    "the account picker's cursor did not leave row 0 after DOWN (one row only: \
                     no account listed?); A anyway"
                        .into(),
                );
                "the account picker: A after DOWN, row unconfirmed".into()
            }
            _ => {
                self.picker_downs += 1;
                self.queue = vec![press(
                    "down",
                    "the account picker: off the no-account row (row 0) to the account",
                )];
                return true;
            }
        };
        self.picks += 1;
        self.picked_on = Some(self.ui_since);
        self.say(format!(
            "the account picker (dialog 0x14): A on its account row (pick {})",
            self.picks
        ));
        self.queue = vec![press("a", &why)];
        true
    }

    /// Hosting the Live custom game: a session advertised (and not
    /// deleted) in Custom Game's lobby mode (2). OptiMatch's lobby (mode
    /// 3) advertises too.
    fn hosting_live(&self) -> bool {
        self.hosting && self.lobby_mode == Some(CUSTOM_GAME_MODE)
    }

    /// On the Xbox Live menu: screen `0xba`, or the window 5 `none` screen
    /// with the Live menu's list mask.
    fn live_menu(&self) -> bool {
        self.screen == Some(LIVE_MENU)
            || (self.screen.is_none()
                && self.window == Some(5)
                && (self.list_mask == Some(LIVE_MENU_MASK)
                    || (self.live_screen.is_some()
                        && self.active == self.live_screen
                        // The lobby reuses the address; the Live menu
                        // sits under the main menu's lobby mode.
                        && self.lobby_mode.is_none_or(|m| m == MAIN_MENU_MODE))))
    }

    /// On the main menu: screen 9, or the window 5 `none` screen the game
    /// was on when it set the main menu's lobby mode (not the Live menu,
    /// nor another screen opened from there under that lobby mode).
    fn at_main(&self) -> bool {
        self.screen == Some(MAIN_MENU)
            || (self.screen.is_none()
                && self.window == Some(5)
                && self.lobby_mode == Some(MAIN_MENU_MODE)
                && !self.live_menu()
                && self.main_screen.is_none_or(|m| self.active == Some(m)))
    }

    /// The main menu's cursor row, read off its list value.
    fn main_row(&self) -> Option<u32> {
        if !self.at_main() {
            return None;
        }
        self.list_value.map(|v| v & 0xffff).filter(|r| *r <= 4)
    }

    fn fresh(&mut self, now: u64) -> bool {
        if !self.held(now, SETTLE_MS) || self.acted == Some(self.screen_since) {
            return false;
        }
        self.acted = Some(self.screen_since);
        true
    }

    /// On the main menu: to Xbox Live from the cursor's row, A.
    fn go_live(&mut self, now: u64, tries: u32) {
        let from = self.main_row().unwrap_or(0);
        let (button, n) = if from <= MAIN_LIVE_ROW {
            ("down", MAIN_LIVE_ROW - from)
        } else {
            ("up", from - MAIN_LIVE_ROW)
        };
        let mut q: Vec<Press> = (0..n)
            .map(|_| press(button, &format!("to Xbox Live (row 1, from {from})")))
            .collect();
        q.push(press("a", "Xbox Live (signs in)"));
        self.queue = q;
        self.step = Step::ToLive { at: now, tries };
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
        if let (false, Some(m)) = (campaign, self.mode)
            && m != MODE_LIVE
        {
            self.say(format!(
                "the players' screen opened in {} (mode {m}), not Xbox Live; backing out",
                mode_name(m)
            ));
        }
        self.queue = match (campaign, self.mode) {
            (true, _) => vec![
                press("b", "the campaign: that was Campaign"),
                press("down", "to Xbox Live"),
                press("a", "Xbox Live"),
            ],
            (false, Some(MODE_LIVE) | None) => {
                self.learn_live = true;
                vec![press(
                    "a",
                    "the players' screen for Xbox Live: on to the Live menu",
                )]
            }
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

    fn at_live_menu(&mut self, now: u64, tries: u32) {
        self.queue.clear();
        self.step = Step::ToCustom {
            at: now,
            presses: 0,
            tries,
        };
        self.to_custom(now, now, 0, tries);
    }

    /// On the Live menu: the next press toward Custom Game, from the row
    /// the last line showed.
    fn to_custom(&mut self, now: u64, at: u64, presses: u32, tries: u32) {
        if !self.live_menu() {
            // Something opened without our A (or a line is on its way).
            if now.saturating_sub(at.max(self.last_press)) > STEP_MS {
                let ui = self.ui.clone();
                self.queue = vec![press(
                    "b",
                    "back to the Xbox Live menu: off it before Custom Game",
                )];
                self.say(format!("left the Xbox Live menu before Custom Game: {ui}"));
                self.step = Step::ToCustom {
                    at: now,
                    presses,
                    tries,
                };
            }
            return;
        }
        if !self.quiet(now, SETTLE_MS) {
            return;
        }
        let (target, name) = self.live_row();
        if presses >= LIVE_MENU_PRESSES {
            let ui = self.ui.clone();
            self.give_up(format!(
                "no {name} row after {LIVE_MENU_PRESSES} presses on the Xbox Live menu; \
                 the last screen: {ui}"
            ));
            return;
        }
        let from = self.cursor;
        let (button, why) = match from {
            None => (
                "down",
                "the Xbox Live menu shows no row yet: DOWN".to_string(),
            ),
            Some(r) if r < target => ("down", format!("toward {name} (row {target}, from {r})")),
            Some(r) if r > target => ("up", format!("toward {name} (row {target}, from {r})")),
            Some(_) => ("a", name.to_string()),
        };
        self.queue = vec![press(button, &why)];
        self.step = if button == "a" {
            match self.target {
                Target::CustomGame => Step::Hosting { at: now, tries },
                Target::Clankermatch { .. } => Step::Playlists {
                    at: now,
                    tries,
                    downs: 0,
                    presses: 0,
                },
            }
        } else {
            Step::ToCustom {
                at: now,
                presses: presses + 1,
                tries,
            }
        };
    }

    fn plan(&mut self, now: u64) {
        let screen = self.screen;
        match self.step.clone() {
            Step::Boot => {
                if self.hosting_live() {
                    self.step = Step::Lobby { since: now };
                    return;
                }
                if self.at_main() {
                    if self.held(now, SETTLE_MS) {
                        self.go_live(now, 0);
                    }
                    return;
                }
                if self.live_menu() {
                    if self.held(now, SETTLE_MS) {
                        self.at_live_menu(now, 0);
                    }
                    return;
                }
                if self.on_the_way(now) {
                    self.step = Step::ToLive { at: now, tries: 0 };
                    return;
                }
                if self.in_picker(now) {
                    return;
                }
                // The title screen and a fresh profile's dialogs wait for
                // A; nothing before the game shows a screen.
                if self.ui.is_empty() || !self.quiet(now, BOOT_HOLD_MS) {
                    return;
                }
                if self.boot_presses >= BOOT_PRESSES {
                    let ui = self.ui.clone();
                    self.give_up(format!(
                        "no main menu after {BOOT_PRESSES} presses; the last screen: {ui}"
                    ));
                    return;
                }
                self.boot_presses += 1;
                let what = if self.dialog() {
                    "a boot dialog (the new profile's)"
                } else {
                    "the title screen"
                };
                self.queue = vec![press("a", &format!("{what}: on to the main menu"))];
            }
            Step::ToLive { at, tries } => {
                if self.live_menu() {
                    if self.held(now, SETTLE_MS) {
                        self.at_live_menu(now, 0);
                    }
                } else if self.on_the_way(now) {
                    if !self.queue.is_empty() {
                        self.step = Step::ToLive { at: now, tries };
                    }
                } else if self.in_picker(now) {
                    if !self.queue.is_empty() {
                        self.step = Step::ToLive { at: now, tries };
                    }
                } else if self.dialog() {
                    // Another dialog on the way: A, its first button.
                    if self.quiet(now, DIALOG_HOLD_MS) {
                        if self.dialog_presses >= DIALOG_PRESSES {
                            let ui = self.ui.clone();
                            self.give_up(format!(
                                "dialogs on the way to Xbox Live after {DIALOG_PRESSES} presses; \
                                 the last: {ui}"
                            ));
                        } else {
                            self.dialog_presses += 1;
                            self.queue = vec![press("a", "a dialog on the way to Xbox Live")];
                            self.step = Step::ToLive { at: now, tries };
                        }
                    }
                } else if self.at_main() && self.held(now, SETTLE_MS) && self.quiet(now, STEP_MS) {
                    // Back on the main menu (a sign-in that failed, or a
                    // press it missed): again from its row.
                    if tries + 1 >= TRIES {
                        let why = if self.picks > 0 {
                            format!(
                                "no Xbox Live menu after {TRIES} tries; back on the main menu \
                                 after picking the account {} time(s) in the account picker \
                                 (the logon did not take)",
                                self.picks
                            )
                        } else {
                            format!("no Xbox Live menu after {TRIES} tries; back on the main menu")
                        };
                        self.give_up(why);
                    } else {
                        self.say("back on the main menu; Xbox Live again".into());
                        self.go_live(now, tries + 1);
                    }
                } else if now.saturating_sub(at.max(self.ui_since)) > 2 * STEP_MS {
                    if tries + 1 >= TRIES {
                        let ui = self.ui.clone();
                        self.give_up(format!(
                            "no Xbox Live menu after {TRIES} tries; the last screen: {ui}"
                        ));
                    } else {
                        self.queue = vec![press("b", "a screen on the way to Xbox Live")];
                        self.step = Step::ToLive {
                            at: now,
                            tries: tries + 1,
                        };
                    }
                }
            }
            Step::ToCustom { at, presses, tries } => {
                self.to_custom(now, at, presses, tries);
            }
            Step::Hosting { at, tries } => {
                if self.hosting_live() {
                    self.step = Step::Lobby { since: now };
                    self.say("in the Live custom game lobby".into());
                } else if now.saturating_sub(at) > STEP_MS {
                    let other = self
                        .lobby_mode
                        .filter(|m| *m != CUSTOM_GAME_MODE && *m != MAIN_MENU_MODE);
                    if tries + 1 >= TRIES {
                        self.give_up(match (self.lobby_mode, other) {
                            (Some(CUSTOM_GAME_MODE), _) => {
                                "a custom game lobby opened but the HLE advertised no Live \
                                 session (not signed in to Xbox Live?)"
                                    .into()
                            }
                            (_, Some(m)) => format!(
                                "Custom Game opened lobby mode {m}, not the custom game lobby \
                                 (mode {CUSTOM_GAME_MODE}; OptiMatch's \
                                 set 3 in run 20261119-003044)"
                            ),
                            _ => "Custom Game never opened a Live lobby".into(),
                        });
                    } else if self.live_menu() && self.held(now, SETTLE_MS) {
                        self.say("back on the Xbox Live menu; Custom Game again".into());
                        self.at_live_menu(now, tries + 1);
                    } else if let Some(m) = other
                        && !self.live_menu()
                    {
                        self.queue = vec![press(
                            "b",
                            &format!("a lobby in mode {m}, not Custom Game's: back"),
                        )];
                        self.step = Step::Hosting {
                            at: now,
                            tries: tries + 1,
                        };
                    } else if !self.live_menu()
                        && (screen.is_some()
                            || (self.window == Some(5) && self.lobby_mode == Some(MAIN_MENU_MODE)))
                    {
                        // A screen, or a window 5 one that is no lobby
                        // (the OptiMatch playlists in run 3).
                        self.queue = vec![press("b", "back to the Xbox Live menu")];
                        self.step = Step::Hosting {
                            at: now,
                            tries: tries + 1,
                        };
                    } else {
                        // The lobby (a none screen) without an
                        // advertisement yet: wait again.
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
            Step::Playlists {
                at,
                tries,
                downs,
                presses,
            } => self.on_playlists(now, at, tries, downs, presses),
            Step::Searching { at, tries } => self.searching(now, at, tries),
            Step::Matching { since } => self.matching(now, since),
            Step::MatchLobby { since } => self.match_lobby(now, since),
            Step::Playing { .. } => {}
            Step::Leaving {
                presses,
                last_b,
                dialogs,
            } => self.leaving(now, presses, last_b, dialogs),
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
        if self.live_menu()
            && let Some(r) = p.row
        {
            s.push_str(&format!(" row {r}"));
        }
        if let Some(r) = self.main_row() {
            s.push_str(&format!(" main menu row {r}"));
        }
        s.push_str(&format!(" ({})", p.why));
        s
    }
}

pub mod clankermatch;

#[cfg(test)]
mod tests;
