//! The party lobby's modes (docs/game/party-lobby.md): MULTIPLAYER,
//! CUSTOM GAMES, FORGE, FIREFIGHT and CO-OP. The lobby's header names the
//! mode it is in, and X opens the GAME MODE picker, which switches it.
//!
//! **A mode is a name for lobby state the game already has.** It is two
//! parts ([`Tab::parts`]): the retail lobby mode (`LOBBY_MODE_SET`: 0
//! co-op, 2 custom game, 3 matchmaking) and the lobby's mode of ours
//! ([`crate::lobby_start::Mode`]). [`Tab::of`] is the inverse, and the
//! lobby's mode is always its answer for the lobby as it stands: nothing
//! here stores a mode of its own. Whether MULTIPLAYER's playlist is a
//! Clanker one is not a mode: it is the playlist the lobby holds, chosen on
//! the one playlist page (`crate::clankermatch::Scope::All`).
//!
//! **Where each mode runs** ([`available`]): by the lobby's [`Place`] and
//! a few [`Facts`], a mode is on or off with a [`Reason`]. A mode whose
//! feature is switched off is absent ([`Present`]); nothing else removes
//! one. What a mode's game lacks (FIREFIGHT with no map that carries the
//! kit) does not turn the mode off: the lobby switches, and Start Game says
//! what is missing (`crate::lobby_start`, `crate::lobby_mode::start_refusal`).
//!
//! **Changing mode** ([`picker`]): X, for the party leader, opens the GAME
//! MODE picker over the lobby, its values the modes present of
//! [`picker::MODES`], focused on the lobby's. A mode that is off says why
//! on the right and, picked, gives its reason to the status line for four
//! seconds ([`Notice`]); any other is the switch ([`switch`]). The Xbox
//! Live menu's Custom Games, Forge and Firefight rows ask for the same
//! switch once their custom lobby is up ([`request`]).
//!
//! The lobby's header and its prompt line, drawn after retail's panel
//! update, and the lobby as `ui.lobby.tabs` reads it are [`header`]; X is
//! [`input`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::lobby_start::Mode;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::Glyph;

/// The modes' texts: the labels (the lobby's header), the prompt line, the
/// reasons.
pub const IDS: Family = ids::family("lobby tabs");

pub mod header;
pub mod input;
pub mod picker;
pub mod switch;

// ---- the modes ----

/// One mode of the lobby, in the order the picker lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    /// Matchmaking: the playlists, ordinary and Clanker ones on one page.
    Multiplayer,
    CustomGames,
    Forge,
    Firefight,
    CoOp,
}

/// The retail lobby mode (`LOBBY_MODE_SET`'s argument).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetailMode {
    /// 0: the co-op campaign lobby.
    CoOp,
    /// 2: the custom game lobby.
    Custom,
    /// 3: the matchmaking lobby.
    Matchmaking,
}

impl RetailMode {
    /// `LOBBY_MODE_SET`'s argument for it.
    pub const fn value(self) -> u32 {
        match self {
            RetailMode::CoOp => 0,
            RetailMode::Custom => 2,
            RetailMode::Matchmaking => 3,
        }
    }
    /// The mode `LOBBY_MODE_SET(value)` sets; `None` for a value it does
    /// not take.
    pub const fn from_value(value: u32) -> Option<RetailMode> {
        match value {
            0 => Some(RetailMode::CoOp),
            2 => Some(RetailMode::Custom),
            3 => Some(RetailMode::Matchmaking),
            _ => None,
        }
    }
}

/// The lobby state a tab names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts {
    pub retail: RetailMode,
    pub mode: Mode,
}

impl Tab {
    /// Every mode, in the picker's order (CO-OP last: the picker does not
    /// list it, [`picker::MODES`]).
    pub const ALL: [Tab; 5] = [
        Tab::Multiplayer,
        Tab::CustomGames,
        Tab::Forge,
        Tab::Firefight,
        Tab::CoOp,
    ];

    /// Its label in the lobby's header, in capitals as retail's headers
    /// are.
    pub const fn label(self) -> &'static str {
        match self {
            Tab::Multiplayer => "MULTIPLAYER",
            Tab::CustomGames => "CUSTOM GAMES",
            Tab::Forge => "FORGE",
            Tab::Firefight => "FIREFIGHT",
            Tab::CoOp => "CO-OP",
        }
    }

    /// Its label in title case, as a list's row reads (the GAME MODE
    /// picker, [`picker`]).
    pub const fn title_case(self) -> &'static str {
        match self {
            Tab::Multiplayer => "Multiplayer",
            Tab::CustomGames => "Custom Games",
            Tab::Forge => "Forge",
            Tab::Firefight => "Firefight",
            Tab::CoOp => "Co-Op",
        }
    }

    /// Its name in the agent API (`ui.lobby.tabs`).
    pub const fn name(self) -> &'static str {
        match self {
            Tab::Multiplayer => "multiplayer",
            Tab::CustomGames => "custom_games",
            Tab::Forge => "forge",
            Tab::Firefight => "firefight",
            Tab::CoOp => "co_op",
        }
    }

    /// Its label's string id.
    pub const fn id(self) -> u32 {
        IDS.id(self as u32)
    }

    /// What the mode plays, one line: the picker's right side while the
    /// mode has the focus and is on.
    pub const fn line(self) -> &'static str {
        match self {
            Tab::Multiplayer => "Pick a playlist and search for a game, against people or Clankers.",
            Tab::CustomGames => "Your map, your rules, your players.",
            Tab::Forge => "Edit a map together, then play on it.",
            Tab::Firefight => "Hold out together against waves of enemies.",
            Tab::CoOp => "Play the campaign together.",
        }
    }

    /// The lobby state it names.
    pub const fn parts(self) -> Parts {
        let (retail, mode) = match self {
            Tab::Multiplayer => (RetailMode::Matchmaking, Mode::Multiplayer),
            Tab::CustomGames => (RetailMode::Custom, Mode::Multiplayer),
            Tab::Forge => (RetailMode::Custom, Mode::Forge),
            Tab::Firefight => (RetailMode::Custom, Mode::Firefight),
            Tab::CoOp => (RetailMode::CoOp, Mode::Multiplayer),
        };
        Parts { retail, mode }
    }

    /// The tab the lobby is on: the inverse of [`Tab::parts`]. Our mode
    /// counts only in the custom lobby, so every lobby state has its tab.
    pub const fn of(retail: RetailMode, mode: Mode) -> Tab {
        match (retail, mode) {
            (RetailMode::CoOp, _) => Tab::CoOp,
            (RetailMode::Matchmaking, _) => Tab::Multiplayer,
            (RetailMode::Custom, Mode::Multiplayer) => Tab::CustomGames,
            (RetailMode::Custom, Mode::Forge) => Tab::Forge,
            (RetailMode::Custom, Mode::Firefight) => Tab::Firefight,
        }
    }
}

/// Forge is a local lobby choice. A guest in a custom lobby sees Custom Games.
pub fn seen_mode(leader: bool, own: Mode) -> Mode {
    if leader && own == Mode::Forge { Mode::Forge } else { Mode::Multiplayer }
}

// ---- which tabs there are ----

/// The modes whose feature is on. MULTIPLAYER, CUSTOM GAMES and CO-OP are
/// retail's lobby modes and always present: the co-op lobby is retail's
/// own (Switch To: Co-Op Game), whether or not co-op over Xbox Live
/// (`crate::coop_live`) is installed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Present {
    pub forge: bool,
    pub firefight: bool,
}

impl Present {
    /// Every feature on.
    pub const ALL: Present = Present {
        forge: true,
        firefight: true,
    };

    pub const fn has(self, tab: Tab) -> bool {
        match tab {
            Tab::Multiplayer | Tab::CustomGames | Tab::CoOp => true,
            Tab::Forge => self.forge,
            Tab::Firefight => self.firefight,
        }
    }

    /// The modes present, in [`Tab::ALL`]'s order.
    pub fn tabs(self) -> Vec<Tab> {
        Tab::ALL.into_iter().filter(|&t| t != Tab::Firefight && self.has(t)).collect()
    }
}

static PRESENT: Mutex<Present> = Mutex::new(Present {
    forge: false,
    firefight: false,
});

/// The modes present in this run: set once at boot, as each feature is
/// installed.
pub fn present() -> Present {
    *PRESENT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Boot's record of a feature behind a mode: `f` sets its flag.
pub fn set_present(f: impl FnOnce(&mut Present)) {
    f(&mut PRESENT.lock().unwrap_or_else(|e| e.into_inner()));
}

// ---- where each mode runs ----

/// Where the lobby is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// The session is not networked (`bots::lobby::NETWORKING`).
    SplitScreen,
    SystemLink,
    XboxLive,
}

/// What the availability reads besides the place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The server lists a matchmaking playlist of either kind: not known
    /// to be empty ([`switch::no_playlists_seen`]).
    pub playlists: bool,
    /// Co-op over Xbox Live is installed (`coop_live::lobby`): the Xbox
    /// Live co-op lobby needs it; Split Screen's is retail's alone.
    pub live_co_op: bool,
    /// A member is on another console (`lobby_mode::remote_members`).
    pub remote_members: bool,
}

/// Why a mode is off, or a switch does nothing; each its status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    MatchmakingNeedsLive,
    NoPlaylists,
    CoOpNotOnSystemLink,
    CoOpRefused,
    NoKitMap,
    ForgeRemote,
    Starting,
    Searching,
    NoMapList,
    NoForgeMap,
    MapRefused,
    NoSettings,
    NotKitMap,
    CoOpLiveOff,
}

/// The first reason's string id.
const FIRST_REASON: u32 = 8;
/// The lobby's prompt line: the X glyph and "Change Game Mode", the text
/// of the line the lobby's tag holds for it ([`header`]).
pub const CHANGE_MODE_ID: u32 = IDS.id(5);
pub const CHANGE_MODE_TEXT: &str = "\u{e102} Change Game Mode";

impl Reason {
    /// Every reason, for the string table.
    pub const ALL: [Reason; 14] = [
        Reason::MatchmakingNeedsLive,
        Reason::NoPlaylists,
        Reason::CoOpNotOnSystemLink,
        Reason::CoOpRefused,
        Reason::NoKitMap,
        Reason::ForgeRemote,
        Reason::Starting,
        Reason::Searching,
        Reason::NoMapList,
        Reason::NoForgeMap,
        Reason::MapRefused,
        Reason::NoSettings,
        Reason::NotKitMap,
        Reason::CoOpLiveOff,
    ];

    /// The status line's sentence.
    pub const fn text(self) -> &'static str {
        match self {
            Reason::MatchmakingNeedsLive => "Matchmaking needs Xbox Live.",
            Reason::NoPlaylists => "No playlists are available.",
            Reason::CoOpNotOnSystemLink => "Co-op campaign is not available on System Link.",
            Reason::CoOpRefused => "Co-op campaign is not available in this lobby.",
            Reason::NoKitMap => "No installed map supports Firefight yet.",
            Reason::ForgeRemote => crate::lobby_mode::REMOTE_REFUSAL,
            Reason::Starting => "The game is starting.",
            Reason::Searching => "Cancel the search to change mode.",
            Reason::NoMapList => "The map list is not read yet.",
            Reason::NoForgeMap => "No original map is installed for Forge.",
            Reason::MapRefused => "The game did not take the map for this mode.",
            Reason::NoSettings => "No settings store holds Firefight's options.",
            Reason::NotKitMap => "This map does not support Firefight.",
            Reason::CoOpLiveOff => "Co-op over Xbox Live is turned off.",
        }
    }

    /// Its string id, on the lobby's status line (`crate::lobby_status`).
    pub const fn id(self) -> u32 {
        IDS.id(FIRST_REASON + self as u32)
    }
}

/// Whether a mode can be switched to here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Avail {
    On,
    Off(Reason),
}

impl Avail {
    pub fn is_on(self) -> bool {
        self == Avail::On
    }
    pub fn reason(self) -> Option<Reason> {
        match self {
            Avail::On => None,
            Avail::Off(r) => Some(r),
        }
    }
}

/// Whether mode `tab` runs in a lobby at `place` with `facts`.
pub fn available(tab: Tab, place: Place, facts: Facts) -> Avail {
    use Avail::{Off, On};
    match tab {
        Tab::Multiplayer => match place {
            Place::XboxLive if facts.playlists => On,
            Place::XboxLive => Off(Reason::NoPlaylists),
            Place::SplitScreen | Place::SystemLink => Off(Reason::MatchmakingNeedsLive),
        },
        Tab::CustomGames => On,
        Tab::Forge if facts.remote_members => Off(Reason::ForgeRemote),
        Tab::Forge => On,
        // With no map that carries the kit, the lobby still switches:
        // Start Game is what says so (`lobby_mode::start_refusal`).
        Tab::Firefight => On,
        Tab::CoOp => match place {
            Place::SystemLink => Off(Reason::CoOpNotOnSystemLink),
            Place::XboxLive if !facts.live_co_op => Off(Reason::CoOpLiveOff),
            Place::SplitScreen | Place::XboxLive => On,
        },
    }
}

/// The modes present, each with whether it runs here.
pub fn row(present: Present, place: Place, facts: Facts) -> Vec<(Tab, Avail)> {
    present
        .tabs()
        .into_iter()
        .map(|t| (t, available(t, place, facts)))
        .collect()
}

/// A mode's state as `ui.lobby.tabs` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    /// The lobby's mode: the header names it.
    Selected,
    /// Another mode the lobby can switch to.
    On,
    /// A mode that cannot run here, with its reason.
    Off,
}

/// `tab`'s state with `selected` the lobby's mode.
pub fn look(tab: Tab, selected: Tab, avail: Avail) -> Look {
    if tab == selected {
        Look::Selected
    } else if avail.is_on() {
        Look::On
    } else {
        Look::Off
    }
}

// ---- the reason line ----

/// How long a reason stays on the status line.
pub const NOTICE_FOR: Duration = Duration::from_secs(4);

/// A reason shown on the status line since `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notice {
    pub reason: Reason,
    pub at: Instant,
}

impl Notice {
    /// The reason while it shows at `now`: [`NOTICE_FOR`] from `at`.
    pub fn shown(&self, now: Instant) -> Option<Reason> {
        (now.saturating_duration_since(self.at) < NOTICE_FOR).then_some(self.reason)
    }
}

static NOTICE: Mutex<Option<Notice>> = Mutex::new(None);

/// Shows `reason` on the lobby's status line for [`NOTICE_FOR`].
pub fn notice(reason: Reason) {
    eprintln!("lobby tabs: {}", reason.text());
    *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Notice {
        reason,
        at: Instant::now(),
    });
}

/// The reason the status line shows now, if any.
pub fn notice_now() -> Option<Reason> {
    let n = *NOTICE.lock().unwrap_or_else(|e| e.into_inner());
    n.and_then(|n| n.shown(Instant::now()))
}

/// The lobby was left: no reason carries over to the next.
pub fn clear_notice() {
    *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ---- a switch asked for before the lobby is up ----

/// How long a [`request`] waits for its lobby.
pub const REQUEST_FOR: Duration = Duration::from_secs(60);

/// A switch asked for at `at`, to `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub to: Tab,
    pub at: Instant,
}

impl Request {
    /// The mode still wanted at `now`: [`REQUEST_FOR`] from `at`.
    pub fn wanted(&self, now: Instant) -> Option<Tab> {
        (now.saturating_duration_since(self.at) < REQUEST_FOR).then_some(self.to)
    }
}

#[cfg(not(test))]
static REQUEST: Mutex<Option<Request>> = Mutex::new(None);

#[cfg(not(test))]
fn with_request<T>(f: impl FnOnce(&mut Option<Request>) -> T) -> T {
    f(&mut REQUEST.lock().unwrap_or_else(|e| e.into_inner()))
}

// Tests run side by side, each on its own thread: each has its own
// request, so one test's lobby can't take another's.
#[cfg(test)]
thread_local! {
    static REQUEST: Mutex<Option<Request>> = const { Mutex::new(None) };
}

#[cfg(test)]
fn with_request<T>(f: impl FnOnce(&mut Option<Request>) -> T) -> T {
    REQUEST.with(|r| f(&mut r.lock().unwrap_or_else(|e| e.into_inner())))
}

/// Asks for a switch to `to` at the custom lobby's next panel update
/// ([`header::update`]): the Xbox Live menu's Custom Games, Forge and
/// Firefight rows, whose press makes a custom lobby and leaves the mode to
/// this.
pub fn request(to: Tab) {
    with_request(|r| {
        *r = Some(Request {
            to,
            at: Instant::now(),
        })
    });
}

/// Drops a switch asked for and not yet run: another Live menu row.
pub fn cancel_request() {
    with_request(|r| *r = None);
}

/// Takes the switch asked for, while it is still wanted.
pub fn take_request() -> Option<Tab> {
    with_request(|r| r.take()).and_then(|r| r.wanted(Instant::now()))
}

// ---- the strings ----

/// Every text of the lobby's modes, served from our string table
/// (`crate::layout_glue`): the labels, the prompt line and the reasons.
pub fn strings() -> Vec<(u32, &'static str)> {
    let mut out: Vec<(u32, &'static str)> = Tab::ALL.iter().map(|t| (t.id(), t.label())).collect();
    out.push((CHANGE_MODE_ID, CHANGE_MODE_TEXT));
    out.extend(Reason::ALL.iter().map(|r| (r.id(), r.text())));
    out
}

/// The prompt line's glyph: X ([`Glyph::char`]).
pub const CHANGE_MODE_GLYPH: Glyph = Glyph::X;

#[cfg(test)]
mod tests;
