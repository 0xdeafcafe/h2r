//! The party lobby's mode strip (docs/game/party-lobby.md): MATCHMAKING,
//! CUSTOM GAMES, FORGE, FIREFIGHT, CO-OP and CLANKERS across the pregame
//! lobby's header band, moved along with the bumpers.
//!
//! **A tab is a name for lobby state the game already has.** It is three
//! parts ([`Tab::parts`]): the retail lobby mode (`LOBBY_MODE_SET`: 0
//! co-op, 2 custom game, 3 matchmaking), the lobby's mode of ours
//! ([`crate::lobby_start::Mode`]) and whether the playlists are the Clanker
//! ones ([`crate::clankermatch::active`]). [`Tab::of`] is the inverse, and
//! the selected tab is always its answer for the lobby as it stands: the
//! strip stores no tab of its own.
//!
//! **Where each tab runs** ([`available`]): by the lobby's [`Place`] and a
//! few [`Facts`], a tab is on or off with a [`Reason`]. A tab whose
//! feature is switched off is absent ([`Present`]); nothing else removes
//! one.
//!
//! **Moving** ([`step`]): White to the previous tab that is on, Black to
//! the next; the ends do not wrap, and the first tab skipped (or the one
//! that stops the move) gives its reason to the status line for four
//! seconds ([`Notice`]).
//!
//! **The cells** ([`cells`]): one per tab present, cut for its label,
//! 5 units apart from -244, on the header band.
//!
//! The strip drawn on the lobby and read by the agent API is [`strip`],
//! the bumpers [`input`] and the switch [`switch`]; its preview (`h2 ui
//! preview lobby.tabs`) is [`preview`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::lobby_start::Mode;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::Glyph;

/// The strip's texts: the labels, the two glyphs, the reasons.
pub const IDS: Family = ids::family("lobby tabs");

pub mod input;
pub mod preview;
pub mod strip;
pub mod switch;

// ---- the tabs ----

/// One mode of the strip, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    Matchmaking,
    CustomGames,
    Forge,
    Firefight,
    CoOp,
    Clankers,
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
    /// The playlists are the Clanker ones (`clankermatch::active`).
    pub clanker: bool,
}

impl Tab {
    /// Every tab, in the strip's order.
    pub const ALL: [Tab; 6] = [
        Tab::Matchmaking,
        Tab::CustomGames,
        Tab::Forge,
        Tab::Firefight,
        Tab::CoOp,
        Tab::Clankers,
    ];

    /// Its label on the strip, in capitals as retail's headers are.
    pub const fn label(self) -> &'static str {
        match self {
            Tab::Matchmaking => "MATCHMAKING",
            Tab::CustomGames => "CUSTOM GAMES",
            Tab::Forge => "FORGE",
            Tab::Firefight => "FIREFIGHT",
            Tab::CoOp => "CO-OP",
            Tab::Clankers => "CLANKERS",
        }
    }

    /// Its name in the agent API (`ui.lobby.tabs`).
    pub const fn name(self) -> &'static str {
        match self {
            Tab::Matchmaking => "matchmaking",
            Tab::CustomGames => "custom_games",
            Tab::Forge => "forge",
            Tab::Firefight => "firefight",
            Tab::CoOp => "co_op",
            Tab::Clankers => "clankers",
        }
    }

    /// Its label's string id.
    pub const fn id(self) -> u32 {
        IDS.id(self as u32)
    }

    /// The width of its cell, cut for its label in font 1
    /// (docs/game/party-lobby.md, "The strip").
    pub const fn width(self) -> i16 {
        match self {
            Tab::Matchmaking => 84,
            Tab::CustomGames => 92,
            Tab::Forge => 40,
            Tab::Firefight => 70,
            Tab::CoOp => 40,
            Tab::Clankers => 62,
        }
    }

    /// The lobby state it names.
    pub const fn parts(self) -> Parts {
        let (retail, mode, clanker) = match self {
            Tab::Matchmaking => (RetailMode::Matchmaking, Mode::Multiplayer, false),
            Tab::CustomGames => (RetailMode::Custom, Mode::Multiplayer, false),
            Tab::Forge => (RetailMode::Custom, Mode::Forge, false),
            Tab::Firefight => (RetailMode::Custom, Mode::Firefight, false),
            Tab::CoOp => (RetailMode::CoOp, Mode::Multiplayer, false),
            Tab::Clankers => (RetailMode::Matchmaking, Mode::Multiplayer, true),
        };
        Parts {
            retail,
            mode,
            clanker,
        }
    }

    /// The tab the lobby is on: the inverse of [`Tab::parts`]. Our mode
    /// counts only in the custom lobby, the Clanker flag only in the
    /// matchmaking one, so every lobby state has its tab.
    pub const fn of(retail: RetailMode, mode: Mode, clanker: bool) -> Tab {
        match (retail, mode, clanker) {
            (RetailMode::CoOp, _, _) => Tab::CoOp,
            (RetailMode::Matchmaking, _, false) => Tab::Matchmaking,
            (RetailMode::Matchmaking, _, true) => Tab::Clankers,
            (RetailMode::Custom, Mode::Multiplayer, _) => Tab::CustomGames,
            (RetailMode::Custom, Mode::Forge, _) => Tab::Forge,
            (RetailMode::Custom, Mode::Firefight, _) => Tab::Firefight,
        }
    }
}

/// Our mode as a member's console reads it: the leader's own, else
/// Firefight when the custom lobby's map carries the kit, else
/// Multiplayer. A member cannot tell Forge from retail's data
/// (docs/game/party-lobby.md, "Other machines").
pub fn seen_mode(leader: bool, own: Mode, kit_map: bool) -> Mode {
    match (leader, kit_map) {
        (true, _) => own,
        (false, true) => Mode::Firefight,
        (false, false) => Mode::Multiplayer,
    }
}

// ---- which tabs there are ----

/// The tabs whose feature is on. MATCHMAKING and CUSTOM GAMES are retail's
/// and always present.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Present {
    pub forge: bool,
    pub firefight: bool,
    pub co_op: bool,
    pub clankers: bool,
}

impl Present {
    /// Every feature on.
    pub const ALL: Present = Present {
        forge: true,
        firefight: true,
        co_op: true,
        clankers: true,
    };

    pub const fn has(self, tab: Tab) -> bool {
        match tab {
            Tab::Matchmaking | Tab::CustomGames => true,
            Tab::Forge => self.forge,
            Tab::Firefight => self.firefight,
            Tab::CoOp => self.co_op,
            Tab::Clankers => self.clankers,
        }
    }

    /// The tabs present, in the strip's order.
    pub fn tabs(self) -> Vec<Tab> {
        Tab::ALL.into_iter().filter(|&t| self.has(t)).collect()
    }
}

static PRESENT: Mutex<Present> = Mutex::new(Present {
    forge: false,
    firefight: false,
    co_op: false,
    clankers: false,
});

/// The tabs present in this run: set once at boot, as each feature is
/// installed.
pub fn present() -> Present {
    *PRESENT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Boot's record of a feature behind a tab: `f` sets its flag.
pub fn set_present(f: impl FnOnce(&mut Present)) {
    f(&mut PRESENT.lock().unwrap_or_else(|e| e.into_inner()));
}

// ---- where each tab runs ----

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
    /// The server lists a matchmaking playlist.
    pub playlists: bool,
    /// The server lists a Clanker playlist.
    pub clanker_playlists: bool,
    /// An installed map carries the Firefight kit (`lobby_start::kit_maps`).
    pub kit_maps: bool,
    /// A member is on another console (`lobby_mode::remote_members`).
    pub remote_members: bool,
}

/// Why a tab is off, or a press does nothing; each its status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    MatchmakingNeedsLive,
    ClankermatchNeedsLive,
    NoPlaylists,
    NoClankerPlaylists,
    CoOpNotOnSystemLink,
    NoKitMap,
    ForgeRemote,
    Starting,
    Searching,
    NoMapList,
    NoForgeMap,
    MapRefused,
    NoSettings,
    MatchmakingNotYet,
    CoOpNotYet,
    ClankersNotYet,
    CustomNotYet,
}

/// The first reason's string id.
const FIRST_REASON: u32 = 8;
/// The White and Black glyphs' string ids.
pub const PREV_GLYPH_ID: u32 = IDS.id(6);
pub const NEXT_GLYPH_ID: u32 = IDS.id(7);

impl Reason {
    /// Every reason, for the string table.
    pub const ALL: [Reason; 17] = [
        Reason::MatchmakingNeedsLive,
        Reason::ClankermatchNeedsLive,
        Reason::NoPlaylists,
        Reason::NoClankerPlaylists,
        Reason::CoOpNotOnSystemLink,
        Reason::NoKitMap,
        Reason::ForgeRemote,
        Reason::Starting,
        Reason::Searching,
        Reason::NoMapList,
        Reason::NoForgeMap,
        Reason::MapRefused,
        Reason::NoSettings,
        Reason::MatchmakingNotYet,
        Reason::CoOpNotYet,
        Reason::ClankersNotYet,
        Reason::CustomNotYet,
    ];

    /// The status line's sentence.
    pub const fn text(self) -> &'static str {
        match self {
            Reason::MatchmakingNeedsLive => "Matchmaking needs Xbox Live.",
            Reason::ClankermatchNeedsLive => "Clankermatch needs Xbox Live.",
            Reason::NoPlaylists => "No playlists are available.",
            Reason::NoClankerPlaylists => "This server has no Clanker playlists.",
            Reason::CoOpNotOnSystemLink => "Co-op campaign is not available on System Link.",
            Reason::NoKitMap => "No installed map carries Firefight.",
            Reason::ForgeRemote => crate::lobby_mode::REMOTE_REFUSAL,
            Reason::Starting => "The game is starting.",
            Reason::Searching => "Cancel the search to change mode.",
            Reason::NoMapList => "The map list is not read yet.",
            Reason::NoForgeMap => "No original map is installed for Forge.",
            Reason::MapRefused => "The game did not take the map for this mode.",
            Reason::NoSettings => "No settings store holds Firefight's options.",
            Reason::MatchmakingNotYet => "Matchmaking from this lobby is not available yet.",
            Reason::CoOpNotYet => "Co-op from this lobby is not available yet.",
            Reason::ClankersNotYet => "Clankermatch from this lobby is not available yet.",
            Reason::CustomNotYet => "Custom games from this lobby are not available yet.",
        }
    }

    /// Its string id, on the lobby's status line (`crate::lobby_status`).
    pub const fn id(self) -> u32 {
        IDS.id(FIRST_REASON + self as u32)
    }
}

/// Whether a tab can be selected here.
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

/// Whether `tab` runs in a lobby at `place` with `facts`.
pub fn available(tab: Tab, place: Place, facts: Facts) -> Avail {
    use Avail::{Off, On};
    match tab {
        Tab::Matchmaking => match place {
            Place::XboxLive if facts.playlists => On,
            Place::XboxLive => Off(Reason::NoPlaylists),
            Place::SplitScreen | Place::SystemLink => Off(Reason::MatchmakingNeedsLive),
        },
        Tab::CustomGames => On,
        Tab::Forge if facts.remote_members => Off(Reason::ForgeRemote),
        Tab::Forge => On,
        Tab::Firefight if !facts.kit_maps => Off(Reason::NoKitMap),
        Tab::Firefight => On,
        Tab::CoOp if place == Place::SystemLink => Off(Reason::CoOpNotOnSystemLink),
        Tab::CoOp => On,
        Tab::Clankers => match place {
            Place::XboxLive if facts.clanker_playlists => On,
            Place::XboxLive => Off(Reason::NoClankerPlaylists),
            Place::SplitScreen | Place::SystemLink => Off(Reason::ClankermatchNeedsLive),
        },
    }
}

/// The strip's tabs present, each with whether it runs here.
pub fn row(present: Present, place: Place, facts: Facts) -> Vec<(Tab, Avail)> {
    present
        .tabs()
        .into_iter()
        .map(|t| (t, available(t, place, facts)))
        .collect()
}

/// How a tab is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    /// White: the lobby's labels.
    Selected,
    /// Pale blue: the lobby's values.
    On,
    /// Pale blue at an unfocused retail row's 55%.
    Off,
}

impl Look {
    /// The looks, in the order of a cell's widgets.
    pub const ALL: [Look; 3] = [Look::Selected, Look::On, Look::Off];
}

/// How `tab` is drawn with `selected` the lobby's tab.
pub fn look(tab: Tab, selected: Tab, avail: Avail) -> Look {
    if tab == selected {
        Look::Selected
    } else if avail.is_on() {
        Look::On
    } else {
        Look::Off
    }
}

// ---- moving ----

/// A bumper's direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    /// White, the left bumper.
    Prev,
    /// Black, the right bumper.
    Next,
}

impl Dir {
    /// Its glyph on the strip.
    pub const fn glyph(self) -> Glyph {
        match self {
            Dir::Prev => Glyph::White,
            Dir::Next => Glyph::Black,
        }
    }
}

/// What a bumper does from a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// To the nearest tab that is on, with the first one skipped's reason.
    Move { to: Tab, skipped: Option<Reason> },
    /// Nowhere: the end of the strip (no reason), or only tabs that are off
    /// that way (the nearest one's).
    Stay(Option<Reason>),
}

/// Where `dir` goes from `from` along `row`.
pub fn step(row: &[(Tab, Avail)], from: Tab, dir: Dir) -> Step {
    let Some(at) = row.iter().position(|(t, _)| *t == from) else {
        return Step::Stay(None);
    };
    let ahead: Box<dyn Iterator<Item = &(Tab, Avail)>> = match dir {
        Dir::Prev => Box::new(row[..at].iter().rev()),
        Dir::Next => Box::new(row[at + 1..].iter()),
    };
    let mut first_off = None;
    for &(tab, avail) in ahead {
        match avail {
            Avail::On => {
                return Step::Move {
                    to: tab,
                    skipped: first_off,
                };
            }
            Avail::Off(r) => {
                first_off.get_or_insert(r);
            }
        }
    }
    Step::Stay(first_off)
}

/// Whether `dir`'s glyph is bright: a tab that is on lies that way.
pub fn glyph_on(row: &[(Tab, Avail)], from: Tab, dir: Dir) -> bool {
    matches!(step(row, from, dir), Step::Move { .. })
}

// ---- the cells ----

/// The strip's band: the top and bottom of every cell, inside the header
/// rectangle `[204 -270 180 70]` the strip takes.
pub const TOP: i16 = 202;
pub const BOTTOM: i16 = 182;
/// The header band the strip lives in.
pub const BAND_TOP: i16 = 204;
pub const BAND_BOTTOM: i16 = 180;
/// The first cell's left edge, and the gap between cells.
pub const RUN_LEFT: i16 = -244;
pub const GAP: i16 = 5;
/// The White and Black glyphs' cells.
pub const PREV: [i16; 4] = [TOP, -270, BOTTOM, -248];
pub const NEXT: [i16; 4] = [TOP, 174, BOTTOM, 196];

/// Each present tab's cell `[top left bottom right]`, left-aligned at
/// [`RUN_LEFT`]: a tab absent closes the run up.
pub fn cells(tabs: &[Tab]) -> Vec<(Tab, [i16; 4])> {
    let mut left = RUN_LEFT;
    tabs.iter()
        .map(|&t| {
            let right = left + t.width();
            let cell = (t, [TOP, left, BOTTOM, right]);
            left = right + GAP;
            cell
        })
        .collect()
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

// ---- the strings ----

/// Every text of the strip, served from our string table
/// (`crate::layout_glue`): the labels, the two glyphs and the reasons.
pub fn strings() -> Vec<(u32, &'static str)> {
    let mut out: Vec<(u32, &'static str)> = Tab::ALL.iter().map(|t| (t.id(), t.label())).collect();
    out.push((PREV_GLYPH_ID, PREV_TEXT));
    out.push((NEXT_GLYPH_ID, NEXT_TEXT));
    out.extend(Reason::ALL.iter().map(|r| (r.id(), r.text())));
    out
}

/// The White and Black glyphs as the font draws them ([`Glyph::char`]).
pub const PREV_TEXT: &str = "\u{e105}";
pub const NEXT_TEXT: &str = "\u{e104}";

#[cfg(test)]
mod tests;
