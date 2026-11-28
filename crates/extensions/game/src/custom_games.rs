//! Custom games over Live (docs/live/live-custom-games.md): the Xbox Live
//! menu's rows (Custom Games and Forge lobbies, a "Custom Game
//! Browser" row that opens the retail System Link browser in its dormant
//! Live mode, a Mods row that opens the mod repository's Mods screen,
//! `crate::mods_screen`, or without it a dialog that explains how to
//! install the Multiplayer Map Pack), and the party privacy the title
//! already has, carried to the server as the session's visibility.
//!
//! **The browser.** The System Link game browser (screen `0x0d`, 0x940
//! bytes, constructor `0x253221`) has two loaders that differ only in the
//! byte at `screen + 0x93c`: `0x253185` (LAN broadcast, what the main menu
//! opens) and `0x2531c9` (Live), which nothing in retail opens. In Live mode
//! the screen searches with `XOnlineMatchSearch` (procedure 1, 50 results,
//! `0x8fa80`), keeps seven-column rows (`0x8fb30`), probes each host with
//! `XNetQosLookup` and decodes the host's QoS listener data into the same
//! game description the LAN reply carries. The HLE already answers all of
//! those from the server's session list, so the screen fills with the
//! sessions the server lists for this player.
//!
//! **The rows.** The Xbox Live menu (screen `0xba`, list handler at
//! `screen + 0x818`, vtable `0x458b48`, constructor `0x230ea2`) builds a
//! four-row datum array named "xbox live menu list" (`0x458c80`) through the
//! shared constructor (`0x1a480d`, count in EAX): Quickmatch, OptiMatch,
//! Custom Game, Content Download. It draws a row with `0x230f36` (stdcall,
//! item at `[esp+4]`, row at `[item+0x70]`, `ret 8`; past row 3 it sets the
//! null id) and handles a press in `0x230f92` (`ret 8`, a pointer to the row
//! at `[esp+8]`, `[[esp+4]+4]` the player index). We grow the count and
//! show the rows in this order ([`Menu::layout`]): rows 0 and 1 as retail,
//! row 2 Clankermatch (OptiMatch's press with `crate::clankermatch::active`
//! set, docs/game/bots.md §8.3), then Co-op Campaign when enabled, Custom
//! Games, Forge, the browser and Mods. Custom Games and Forge each run retail's Custom Game press (a Live squad session and
//! the custom lobby) and ask for their mode ([`Row::mode`]), which the
//! lobby switches to once it is up (`crate::lobby_tabs::request`,
//! docs/game/party-lobby.md). The browser's row opens the Live browser with
//! `0x149f1e(1 << player, 0x2531c9)`, the request the System Link path
//! builds at `0x252951`; Mods (retail's Content Download, renamed) opens
//! the Mods screen or the game's one-button dialog with our text
//! ([`MAPPACK_ERROR`]) instead of retail's leave-for-the-dashboard
//! confirmation (`0x19b5af`, error `0xa3`).
//!
//! **Back.** The browser's back press (`0x2536a6`) always opens the System
//! Link screen (`0x252433`), its LAN parent. While a Live-mode browser
//! handles an event, that call goes to the Live menu's loader (`0x2310b7`)
//! instead, and our row press leaves the Live menu's cursor on our row
//! ([`browser_is_live`], [`back_loader`], [`remember_row`]).
//!
//! **The visible item widgets.** The handler embeds four item widgets
//! (`+0x88`, 0x80 bytes each, built by `0x24abae`), and a list scrolls when
//! it has more rows than child item widgets (`0x24c515` counts children with
//! `0x24c177`). So, as for the Controls list, the shared "add items" hook
//! (`0x24bb60`) links our additional item widgets after the four. The
//! list lays the widgets out in order (`0x24bc12`). The tag's
//! list shows at most eight items ([`VISIBLE_ITEMS`]), the whole menu, so no
//! row scrolls out of sight; a menu of more rows would scroll, with retail's
//! own scroll arrows.
//! [`Menu::extend_tag`] gives the added rows their panes.
//!
//! **Privacy.** The party privacy screen (screen `0x19`, list "squad privacy
//! setting list") offers 0 open, 1 friends, 2 invite only (the last two only
//! signed in), and its press calls `0x19a148` with the item id in EAX, which
//! stores it in the squad session at `+0x498c` (`0x64230`, `0x5bde0`). The
//! host's join check (`0x62fe0`) refuses everyone above 1; the server's
//! lists do the rest. We follow the press into the `live.visibility`
//! setting (which updates an open lobby through the HLE) and give the HLE
//! a probe that reads the squad session's privacy whenever it advertises.
//!
//! The logic is over [`Gm`] and tested without a guest; the hooks are
//! registered by `layout_glue::install` (the rows: they share the list
//! constructor, "add items", the screen tag, the pane setter and the string
//! lookup) and [`install_privacy`].

pub mod browser_filters;

use std::sync::Arc;

use crate::layout_glue::{Gm, ID_BASE};
use crate::lobby_tabs::Tab;
use crate::setting_defs::{
    LIVE_VISIBILITY, VISIBILITY_FRIENDS, VISIBILITY_INVITE, VISIBILITY_PUBLIC,
};

/// The Xbox Live menu's list name and retail row count.
pub const LIST_NAME: u32 = 0x0045_8c80;
pub const VANILLA_ROWS: u32 = 4;
/// The rows of the whole menu ([`Menu::ALL`], every row on), in order:
/// Quickmatch and OptiMatch as retail, Clankermatch (docs/game/bots.md
/// §8.3), Co-op Campaign (docs/live/live-coop.md), Custom Games, Forge, our Custom Game Browser, and Mods (retail's Content
/// Download, its row 3). A row that is off (the co-op row with
/// `H2_NO_COOP_ROW=1`, Forge with its feature off) is left
/// out and the rows under it close up ([`Menu::layout`]).
pub const CLANKERMATCH_ROW: i16 = 2;
pub const COOP_ROW: i16 = 3;
pub const CUSTOM_GAMES_ROW: i16 = 4;
pub const FORGE_ROW: i16 = 5;
pub const BROWSER_ROW: i16 = 6;
pub const MODS_ROW: i16 = 7;
const RETAIL_DOWNLOAD_ROW: i16 = 3;
/// The Quickmatch row, using the pressing player’s saved selection.
pub const RETAIL_QUICKMATCH_ROW: i16 = 0;
/// Retail row 1, OptiMatch (`0x231025`: the playlist screen's loader
/// `0x230c8d` through `0x149f1e`). The Clankermatch row runs it with
/// `crate::clankermatch::active` set.
pub const RETAIL_OPTIMATCH_ROW: i16 = 1;
/// Retail row 2, Custom Game (`0x231001`: `0x199e2e(1)`, a Live squad
/// session `0x59890(2, 0, 0)`, then the lobby mode `0x199a03(2)`). The
/// Custom Games and Forge rows run it and ask for their mode;
/// the Co-op Campaign row runs it with the mode turned to co-op.
pub const RETAIL_CUSTOM_GAME_ROW: i16 = 2;
/// Its row text (stdcall, `ret 8`) and row chosen (`ret 8`) functions.
pub const ROW_TEXT: u32 = 0x0023_0f36;
pub const ROW_CHOSEN: u32 = 0x0023_0f92;
/// The list handler's layout (`0x24bc12`: the handler in ESI, a flag byte
/// on the stack, `ret 4`): each child item one item height below the
/// last. Shared by every list; [`repitch`] follows it for this one.
pub const LIST_LAYOUT: u32 = 0x0024_bc12;
/// The System Link browser's Live-mode loader (stdcall, request, `ret 4`).
pub const LIVE_BROWSER_LOADER: u32 = 0x0025_31c9;
/// The browser screen's vtable and its Live byte (0 LAN, 1 Live).
pub const BROWSER_VTABLE: u32 = 0x0045_a558;
const BROWSER_LIVE: u32 = 0x93c;
/// The browser's event handler (screen slot 10, thiscall, the event at
/// `[esp+4]`, `ret 4`). On B or Back (event 5, button 1 or 13) it opens
/// the System Link screen's loader with a back request, whatever the mode.
pub const BROWSER_EVENT: u32 = 0x0025_36a6;
/// The loader that back press opens (stdcall, request, `ret 4`): the
/// System Link screen in its LAN mode (`[screen+0x610] = 2`).
pub const BROWSER_BACK_LOADER: u32 = 0x0025_2433;
/// The Xbox Live menu's loader (stdcall, request, `ret 4`); `0x19b0ad`
/// goes back to it with the same kind of request.
pub const LIVE_MENU_LOADER: u32 = 0x0023_10b7;
/// A request's kind word (`+0`) when it is a back transition, and where the
/// request keeps its loader.
const REQUEST_BACK: u16 = 7;
const REQUEST_LOADER: u32 = 0x1c;
/// The row the Live menu puts its cursor on when built (`0x231170` reads
/// it when 0 to 3, `cmp eax,4; jge` at `0x0023119e`; the retail press
/// stores rows 0 to 2).
const LAST_ROW: u32 = 0x0051_0a18;
/// The rows `0x231170` restores the cursor to.
pub const RESTORED_ROWS: i16 = 4;
/// The rows a press leaves the cursor on: every row above the whole
/// menu's Mods row (Mods opens a dialog or the Mods screen over the menu,
/// and never stores its row). Rows from [`RESTORED_ROWS`] on are restored
/// by [`LiveMenuCursor`].
pub const REMEMBERED_ROWS: i16 = MODS_ROW;
/// Our row's label. It fits: the list skin's text rectangle is 190 units
/// wide, and the label renders 136 wide in conduit-12 (180 in conduit-16).
pub const TEXT_ID: u32 = ID_BASE + 0x32;
pub const TEXT: &str = "Custom Game Browser";
/// The help text on our row's pane.
pub const HELP_ID: u32 = ID_BASE + 0x33;
pub const HELP: &str = "Find custom games hosted on your Live server and join one.";
/// The co-op row's label and help (docs/live/live-coop.md, "Co-op over
/// Live"). "Co-op Campaign" renders narrower than "Custom Game Browser".
pub const COOP_TEXT_ID: u32 = ID_BASE + 0x36;
pub const COOP_TEXT: &str = "Co-op Campaign";
pub const COOP_HELP_ID: u32 = ID_BASE + 0x37;
pub const COOP_HELP: &str =
    "Play the campaign with a friend over Xbox Live, one player per console.";
/// The Mods row's help when it opens the Mods screen
/// (`crate::mods_screen`), in place of retail's on its pane.
pub const MODS_HELP_ID: u32 = ID_BASE + 0x38;
pub const MODS_HELP: &str = "Download maps other players made and shared on your Live server.";
/// The Clankermatch row's label and help (docs/game/bots.md §8.1). It
/// renders narrower than "Custom Game Browser".
pub const CLANKERMATCH_TEXT_ID: u32 = ID_BASE + 0x39;
pub const CLANKERMATCH_TEXT: &str = "Clankermatch";
pub const CLANKERMATCH_HELP_ID: u32 = ID_BASE + 0x3a;
pub const CLANKERMATCH_HELP: &str =
    "Matchmaking against bots. Pick a playlist; your console hosts.";
/// The lobby's "Switch To: Clankermatch" row (`crate::clankermatch`'s
/// switch row, under retail's "Switch To: Matchmaking").
pub const SWITCH_CLANKERMATCH_TEXT_ID: u32 = ID_BASE + 0x3b;
pub const SWITCH_CLANKERMATCH_TEXT: &str = "Switch To: Clankermatch";

/// The mode rows' and the Mods row's texts: their own family, since "live
/// menu rows" is full.
const MODE_IDS: crate::ui_kit::ids::Family = crate::ui_kit::ids::family("live menu modes");
/// The Custom Games and Forge rows: the lobby's own names for
/// its modes (`crate::lobby_tabs::Tab::title_case`), each with a line of
/// help on its pane.
pub const CUSTOM_GAMES_TEXT_ID: u32 = MODE_IDS.id(0);
pub const CUSTOM_GAMES_TEXT: &str = Tab::CustomGames.title_case();
pub const CUSTOM_GAMES_HELP_ID: u32 = MODE_IDS.id(1);
pub const CUSTOM_GAMES_HELP: &str =
    "Set up a game your way: pick the map, the rules and who plays.";
pub const FORGE_TEXT_ID: u32 = MODE_IDS.id(2);
pub const FORGE_TEXT: &str = Tab::Forge.title_case();
pub const FORGE_HELP_ID: u32 = MODE_IDS.id(3);
pub const FORGE_HELP: &str = "Edit a map with your party in Forge, then play on it.";
/// The Mods row's label when the mod repository has none of its own
/// (`crate::mods_screen`), and its help when it opens the Map Pack dialog.
pub const MODS_TEXT_ID: u32 = MODE_IDS.id(6);
pub const MODS_TEXT: &str = "Mod Downloader";
pub const MAPPACK_HELP_ID: u32 = MODE_IDS.id(7);
pub const MAPPACK_HELP: &str = "Add the Multiplayer Map Pack's maps to your game.";

/// The retail playlist screen and the header string on its screen definition.
pub const PLAYLIST_SCREEN: i16 = 0xdb;
const PLAYLIST_HEADER: u32 = 0x2c;
const QUICKMATCH_TITLE: u32 = MODE_IDS.id(8);
const OPTIMATCH_TITLE: u32 = MODE_IDS.id(9);

pub fn playlist_header<M: Gm>(m: &mut M, tag: u32, scope: crate::clankermatch::Scope) {
    use crate::clankermatch::Scope;
    if m.u16(tag + 4) == Some(PLAYLIST_SCREEN as u16) {
        let id = match scope {
            Scope::QuickMatch => QUICKMATCH_TITLE,
            Scope::Clankers => CLANKERMATCH_TEXT_ID,
            Scope::All | Scope::Players => OPTIMATCH_TITLE,
        };
        m.set_u32(tag + PLAYLIST_HEADER, id);
    }
}

/// The screen, its list handler's vtable and its retail item widgets.
pub const SCREEN_ID: i16 = 0xba;
pub const HANDLER_VTABLE: u32 = 0x0045_8b48;
pub const VANILLA_ITEMS: u32 = 4;
/// The list fits eight item widgets above its hint block (the whole menu);
/// more rows would scroll.
pub const VISIBLE_ITEMS: u32 = 8;
pub const MAX_EXTRA_ITEMS: u32 = VISIBLE_ITEMS - VANILLA_ITEMS;

// ---- the Map Pack dialog ----

/// One-button dialog (stdcall: kind 1, error id, 4, player mask, on press,
/// on update; `ret 0x18`), called that way by `0x2308ab` for the game's own
/// errors. With no press callback the button closes the dialog (`0x19b97a`).
pub const OK_DIALOG: u32 = 0x0019_b527;
/// The dialog error lookup: ESI = the 0x20-byte entry it fills, `[esp+4]` =
/// the error id, `ret 4`. The entry's string ids are the title at `+0xc`,
/// the message at `+0x10` and two button labels; an id the table lacks gets
/// error 1's entry. The dialog's text fill (`0x19b72a`) resolves those ids
/// through the localized-string lookup, which serves ours.
pub const ERROR_LOOKUP: u32 = 0x0023_661f;
/// Our error id, one the table does not have.
pub const MAPPACK_ERROR: u32 = 0x6832_0001;
const ENTRY_TITLE: u32 = 0xc;
const ENTRY_MESSAGE: u32 = 0x10;
/// Our dialog text (docs/game/map-pack.md section 4). The message box holds about
/// five lines of 240 units; each line renders under 220 in conduit-12.
pub const MAPPACK_TITLE_ID: u32 = ID_BASE + 0x34;
pub const MAPPACK_TITLE: &str = "Multiplayer Map Pack";
pub const MAPPACK_MESSAGE_ID: u32 = ID_BASE + 0x35;
pub const MAPPACK_MESSAGE: &str = "Put mappack.iso in the same folder\r\n\
                                   as halo2.iso, then start the game.\r\n\
                                   The maps install on that start.";

/// The party privacy press (`0x2b533e` calls it, item id in EAX).
pub const PRIVACY_CHOSEN: u32 = 0x0019_a148;
/// The network life cycle's ready byte and the squad session pointer.
const NETWORK_READY: u32 = 0x0052_7330;
const SQUAD_SESSION: u32 = 0x0052_7364;
const SESSION_STATE: u32 = 0x741c;
const SESSION_PRIVACY: u32 = 0x498c;

/// Whether the Co-op Campaign row is on (`H2_NO_COOP_ROW=1` takes it out;
/// it needs our Live service, see `crate::coop_live`).
pub fn coop_row_enabled() -> bool {
    COOP_ROW_ON.load(std::sync::atomic::Ordering::SeqCst)
        && std::env::var_os("H2_NO_COOP_ROW").is_none()
}

/// Set when the co-op screens are installed (Live on).
static COOP_ROW_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_coop_row(on: bool) {
    COOP_ROW_ON.store(on, std::sync::atomic::Ordering::SeqCst);
}

/// Whether the Mods row opens the Mods screen (`crate::mods_screen`,
/// installed with Live and a map gate).
pub fn mods_row_enabled() -> bool {
    MODS_ROW_ON.load(std::sync::atomic::Ordering::SeqCst)
}

static MODS_ROW_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_mods_row(on: bool) {
    MODS_ROW_ON.store(on, std::sync::atomic::Ordering::SeqCst);
}

/// The Live menu's optional rows. Read when the hooks run ([`Menu::now`]),
/// since the lobby and its modes are registered after the layout hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Menu {
    pub coop: bool,
    pub forge: bool,
}

impl Menu {
    /// Every row on: the eight-row menu.
    pub const ALL: Menu = Menu {
        coop: true,
        forge: true,
    };

    /// This run's menu: the co-op row's switch and the lobby's modes
    /// (`crate::lobby_tabs::present`).
    pub fn now() -> Menu {
        let modes = crate::lobby_tabs::present();
        Menu {
            coop: coop_row_enabled(),
            forge: modes.forge,
        }
    }

    /// The menu's rows, top to bottom: the whole menu less the rows that
    /// are off.
    pub fn layout(self) -> Vec<Row> {
        [
            (Row::Retail, true),
            (Row::Retail, true),
            (Row::Clankermatch, true),
            (Row::Coop, self.coop),
            (Row::CustomGames, true),
            (Row::Forge, self.forge),
            (Row::Browser, true),
            (Row::Mods, true),
        ]
        .into_iter()
        .filter_map(|(row, on)| on.then_some(row))
        .collect()
    }

    pub fn rows(self) -> u32 {
        self.layout().len() as u32
    }

    /// The rows the list shows at once; the rest scroll.
    pub fn visible(self) -> u32 {
        self.rows().min(VISIBLE_ITEMS)
    }

    pub fn count(self, name: u32, count: u32) -> u32 {
        if name == LIST_NAME && count == VANILLA_ROWS {
            self.rows()
        } else {
            count
        }
    }

    /// What row `row` is; past the last, or negative, a retail row.
    pub fn row_kind(self, row: i16) -> Row {
        usize::try_from(row)
            .ok()
            .and_then(|r| self.layout().get(r).copied())
            .unwrap_or(Row::Retail)
    }

    /// Where `kind` is in this menu, if it is there.
    pub fn row_of(self, kind: Row) -> Option<i16> {
        self.layout()
            .iter()
            .position(|&r| r == kind)
            .map(|r| r as i16)
    }

    pub fn row_text_id(self, row: i16) -> Option<u32> {
        let mods = mods_row_enabled()
            .then(crate::mods_screen::repo)
            .flatten()
            .map(|r| r.menu_label());
        text_id(self.row_kind(row), mods)
    }

    pub fn pane_of(self, row: i16, panes: u32) -> Pane {
        pane_for(self.row_kind(row), panes)
    }

    /// Serve this root only to the Live menu. Other tag lookups keep the
    /// map's pane count, pane array and original list layout.
    pub fn copy_tag<M: Gm>(self, m: &mut M, source: u32, root: u32, panes: u32) -> Option<bool> {
        let mut header = [0; crate::list_class::SCREEN_ROOT_SIZE as usize];
        if !m.rd(source, &mut header) || !m.wr(root, &header) {
            return None;
        }
        self.extend_tag(m, root, panes)
    }

    pub fn extend_tag<M: Gm>(self, m: &mut M, wgit: u32, base: u32) -> Option<bool> {
        extend_tag_with(m, wgit, base, TAG_SPACE, self, mods_row_enabled())
    }
}

/// What a row of the grown Xbox Live menu is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// The retail Quickmatch or OptiMatch row, or an unknown row.
    Retail,
    /// Ours: OptiMatch's path, with the clanker hoppers
    /// (`crate::clankermatch`).
    Clankermatch,
    /// Ours: opens the co-op screen (`crate::coop_live`).
    Coop,
    /// Retail's Custom Game press, the lobby in CUSTOM GAMES.
    CustomGames,
    /// Retail's Custom Game press, the lobby switched to FORGE.
    Forge,
    /// Ours: opens the Live browser.
    Browser,
    /// Retail's Content Download, renamed and moved down: opens the Mods
    /// screen (`crate::mods_screen`), else the Map Pack dialog.
    Mods,
}

impl Row {
    /// The retail row whose press this row's press runs (the press record
    /// rewritten to it): OptiMatch for Clankermatch, retail row 2 (Custom
    /// Game) for the mode rows. `None` for the retail rows themselves and
    /// the rest of ours.
    pub fn retail_press(self) -> Option<i16> {
        match self {
            Row::Clankermatch => Some(RETAIL_OPTIMATCH_ROW),
            Row::CustomGames | Row::Forge => Some(RETAIL_CUSTOM_GAME_ROW),
            _ => None,
        }
    }

    /// The lobby mode a press of this row asks for
    /// (`crate::lobby_tabs::request`). Every other row drops a mode asked
    /// for and not yet taken.
    pub fn mode(self) -> Option<Tab> {
        match self {
            Row::CustomGames => Some(Tab::CustomGames),
            Row::Forge => Some(Tab::Forge),
            _ => None,
        }
    }

    /// The pane of ours this row shows, in an extended tag.
    pub fn pane(self) -> Option<i16> {
        match self {
            Row::Clankermatch => Some(CLANKERMATCH_PANE),
            Row::Browser => Some(BROWSER_PANE),
            Row::Coop => Some(COOP_PANE),
            Row::CustomGames => Some(CUSTOM_GAMES_PANE),
            Row::Forge => Some(FORGE_PANE),
            Row::Retail | Row::Mods => None,
        }
    }
}

/// The displayed menu owns the salted handles passed to its press callback.
pub(crate) fn press_handle(m: &runtime::Guest, receiver: u32, player: u32, row: i16) -> Option<u32> {
    let list = if m.u32(receiver).ok() == Some(HANDLER_VTABLE) {
        receiver
    } else {
        [crate::ui_watch::SHARED, player].into_iter().find_map(|slot| {
            let stack = crate::ui_watch::stack(m, 5, slot)?;
            let &(screen, _) = stack.list.iter().find(|(screen, id)| {
                *screen == stack.active && *id == Some(SCREEN_ID as u32)
            })?;
            let list = screen.checked_add(MENU_LIST)?;
            (m.u32(list).ok()? == HANDLER_VTABLE).then_some(list)
        })?
    };
    crate::retail_rows::retail_handle(m, list, row)
}

/// The profile field read by retail Quickmatch. Without a saved descriptor
/// Quickmatch needs the playlist picker before it can start a search.
pub fn quickmatch_selection<M: Gm>(m: &M, player: u32) -> Option<u32> {
    use crate::skulls::{LOCAL_PLAYERS, SLOTS, SLOT_SIGNED_IN, SLOT_STRIDE};
    if player >= LOCAL_PLAYERS {
        return None;
    }
    let slot = SLOTS + player * SLOT_STRIDE;
    if m.u32(slot)? & SLOT_SIGNED_IN == 0 {
        return None;
    }
    m.u32(slot + 0xfc).filter(|&id| id < live_proto::hopper::HOPPERS as u32)
}

/// Labels for our rows; `mods` is the mod repository's label for the Mods
/// row.
fn text_id(kind: Row, mods: Option<u32>) -> Option<u32> {
    match kind {
        Row::Clankermatch => Some(CLANKERMATCH_TEXT_ID),
        Row::Coop => Some(COOP_TEXT_ID),
        Row::CustomGames => Some(CUSTOM_GAMES_TEXT_ID),
        Row::Forge => Some(FORGE_TEXT_ID),
        Row::Browser => Some(TEXT_ID),
        Row::Mods => Some(mods.unwrap_or(MODS_TEXT_ID)),
        Row::Retail => None,
    }
}

/// What the pane setter (`0x230427`, pane = base + row) should do for a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// Let it set pane `base + row`.
    Retail,
    /// Show this pane instead.
    Show(i16),
    /// Set nothing: the shown pane stays.
    Keep,
}

/// The Live menu's panes for a row, with `panes` the tag's count. Mods
/// shows retail's Content Download pane (its help ours in an extended
/// tag); in a tag we did not extend, Custom Games shows retail's Custom
/// Game pane.
fn pane_for(kind: Row, panes: u32) -> Pane {
    match (kind, kind.pane()) {
        (Row::Retail, _) => Pane::Retail,
        (Row::Mods, _) => Pane::Show(RETAIL_DOWNLOAD_ROW),
        (_, Some(p)) if panes > p as u32 => Pane::Show(p),
        (Row::CustomGames, _) => Pane::Show(RETAIL_CUSTOM_GAME_ROW),
        // No pane of ours: keep the one shown rather than show another
        // row's help for ours.
        _ => Pane::Keep,
    }
}

/// Our panes, after the retail four.
pub const CLANKERMATCH_PANE: i16 = 4;
pub const BROWSER_PANE: i16 = 5;
pub const COOP_PANE: i16 = 6;
pub const CUSTOM_GAMES_PANE: i16 = 7;
pub const FORGE_PANE: i16 = 8;
/// The extended tag's panes: the retail four and ours, every one built
/// whichever rows are on, so a row's pane does not move.
pub const ALL_PANES: u32 = 9;

// ---- back from the Live browser ----
//
// The game has no history of screens: a screen's back handler opens its
// parent by loader with a back request (kind 7). The browser's handler
// (`0x2536a6`) always names the System Link screen (`0x252433`), its parent
// on the LAN path. Opened from our row, that is not where the player came
// from (in game, back landed on the controller screen). So while a Live-mode
// browser handles an event, the System Link loader is swapped for the Live
// menu's, the way `0x19b0ad` goes back to it; and our row press leaves the
// Live menu's cursor on our row.

/// Whether `screen` is the browser in its Live mode, the one only our row
/// opens. A LAN browser (byte 0) or another screen: `false`.
pub fn browser_is_live<M: Gm>(m: &M, screen: u32) -> bool {
    m.u32(screen) == Some(BROWSER_VTABLE)
        && m.u8(screen.wrapping_add(BROWSER_LIVE))
            .is_some_and(|b| b != 0)
}

/// For a call to the System Link loader made while a Live browser handles
/// an event: when `request` is a back request, point it at the Live menu's
/// loader and return that loader to call instead. Anything else: `None`,
/// nothing written.
pub fn back_loader<M: Gm>(m: &mut M, request: u32) -> Option<u32> {
    (m.u16(request)? == REQUEST_BACK
        && m.set_u32(request.wrapping_add(REQUEST_LOADER), LIVE_MENU_LOADER))
    .then_some(LIVE_MENU_LOADER)
}

/// Leave the Live menu's cursor on `row` for when a screen goes back to it
/// (the retail press stores rows 0 to 2 the same way). Rows below
/// [`REMEMBERED_ROWS`] are written: `0x231170` restores the first four,
/// [`LiveMenuCursor`] the rest (the mode rows, the browser). Returns
/// whether it wrote.
pub fn remember_row<M: Gm>(m: &mut M, row: i16) -> bool {
    (0..REMEMBERED_ROWS).contains(&row) && m.set_u32(LAST_ROW, row as u32)
}

/// The row the menu's build should put the cursor on that `0x231170`
/// will not: a stored row from [`RESTORED_ROWS`] to [`REMEMBERED_ROWS`].
/// Rows it restores itself, and anything else (-1, nothing stored): `None`.
pub fn cursor_row<M: Gm>(m: &M) -> Option<u32> {
    let row = m.u32(LAST_ROW)? as i32;
    (i32::from(RESTORED_ROWS)..i32::from(REMEMBERED_ROWS))
        .contains(&row)
        .then_some(row as u32)
}

// ---- the cursor past row 3 ----
//
// `0x231170` (the Live menu's build, vtable slot `0x458bec`, thiscall, no
// arguments) is: `[0x54e7bc] = -1; byte [0x54e7b8] = 0;
// 0x22ee92(screen, 0x1901fc()); 0x22f672(screen);` then, for a stored row
// of 0 to 3, `0x24c591(list = screen+0x818 in EAX, row)`. For a stored row
// [`cursor_row`] names, a `before` runs the same steps and sets that row.

const SEARCH_STATE: u32 = 0x0054_e7bc;
const SEARCH_FLAG: u32 = 0x0054_e7b8;
/// The menu's local players' mask (no arguments, EAX).
const MENU_PLAYERS: u32 = 0x0019_01fc;
/// Fills the menu (thiscall, the mask on the stack, `ret 4`).
const MENU_FILL: u32 = 0x0022_ee92;
/// The menu's second build step (thiscall, no arguments).
const MENU_FINISH: u32 = 0x0022_f672;
/// Puts a list's cursor on a row (the list in EAX, the row on the stack,
/// `ret 4`); a row the list does not have does nothing.
const LIST_SET_ROW: u32 = 0x0024_c591;
/// The menu screen's list.
const MENU_LIST: u32 = 0x818;

/// `0x231170`'s steps on `screen`, with the cursor put on `row`.
async fn build_menu(cx: engine::entry::Cx, screen: u32, row: u32) -> engine::entry::EntryResult {
    use engine::entry::{GuestCall, Ret};
    cx.write(|m| {
        m.set_u32(SEARCH_STATE, u32::MAX)?;
        m.set_u8(SEARCH_FLAG, 0)
    })?;
    let players = cx.call(GuestCall::new(MENU_PLAYERS).ecx(screen)).await?;
    cx.call(GuestCall::new(MENU_FILL).ecx(screen).args([players.eax]))
        .await?;
    cx.call(GuestCall::new(MENU_FINISH).ecx(screen)).await?;
    cx.call(
        GuestCall::new(LIST_SET_ROW)
            .eax(screen.wrapping_add(MENU_LIST))
            .args([row]),
    )
    .await?;
    Ok(Ret::new(0, 0))
}

/// The feature (`features.live-menu-cursor`).
pub const CURSOR_FEATURE: &str = "live-menu-cursor";

/// Back to the Xbox Live menu from a lobby, the browser or co-op, the
/// cursor is on the row the player chose, as retail's is for its rows.
pub struct LiveMenuCursor;

impl hooks::Feature for LiveMenuCursor {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            CURSOR_FEATURE,
            "The Xbox Live menu's cursor back on the row the player chose",
        )
        .doc("docs/game/bots.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use engine::entry::EntryFuture;
        use hooks::Flow;
        b.raw().before(
            &engine::points::LIVE_MENU_CURSOR,
            "the cursor on our row",
            |c| {
                let screen = c.cpu.ecx;
                let row = {
                    let rt = Arc::clone(c.ctx.runtime());
                    let mem = rt.memory();
                    cursor_row(&*mem)
                };
                let Some(row) = row else {
                    return Ok(Flow::Continue);
                };
                c.answer_async(move |cx| -> EntryFuture { Box::pin(build_menu(cx, screen, row)) })
            },
        );
        // It reads the stored row afresh at each build.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds [`LiveMenuCursor`].
pub fn install_cursor(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry
        .add(&LiveMenuCursor)
        .map_err(|e| anyhow::anyhow!("live menu cursor: {e}"))?;
    Ok(())
}

/// The strings the rows and the dialog show, as `(id, text)`.
pub fn strings() -> Vec<(u32, &'static str)> {
    vec![
        (QUICKMATCH_TITLE, "Quickmatch"),
        (OPTIMATCH_TITLE, "OptiMatch"),
        (TEXT_ID, TEXT),
        (HELP_ID, HELP),
        (COOP_TEXT_ID, COOP_TEXT),
        (COOP_HELP_ID, COOP_HELP),
        (CLANKERMATCH_TEXT_ID, CLANKERMATCH_TEXT),
        (CLANKERMATCH_HELP_ID, CLANKERMATCH_HELP),
        (SWITCH_CLANKERMATCH_TEXT_ID, SWITCH_CLANKERMATCH_TEXT),
        (MODS_HELP_ID, MODS_HELP),
        (MAPPACK_TITLE_ID, MAPPACK_TITLE),
        (MAPPACK_MESSAGE_ID, MAPPACK_MESSAGE),
        (CUSTOM_GAMES_TEXT_ID, CUSTOM_GAMES_TEXT),
        (CUSTOM_GAMES_HELP_ID, CUSTOM_GAMES_HELP),
        (FORGE_TEXT_ID, FORGE_TEXT),
        (FORGE_HELP_ID, FORGE_HELP),
        (MODS_TEXT_ID, MODS_TEXT),
        (MAPPACK_HELP_ID, MAPPACK_HELP),
    ]
}

/// After the error lookup filled `entry` for `error`: our title and
/// message when it is [`MAPPACK_ERROR`]. Returns whether it wrote. The rest
/// of the entry (string table, button labels, dialog kind) stays as the
/// fallback set it; the one-button dialog takes its button label from its
/// own screen tag.
pub fn answer_error<M: Gm>(m: &mut M, entry: u32, error: u32) -> bool {
    error == MAPPACK_ERROR
        && m.set_u32(entry.wrapping_add(ENTRY_TITLE), MAPPACK_TITLE_ID)
        && m.set_u32(entry.wrapping_add(ENTRY_MESSAGE), MAPPACK_MESSAGE_ID)
}

// ---- the screen's tag: our panes; the rows' pitch ----
//
// The tag (`wgit`, docs/formats/ui.md) has four panes, one per row, each with the
// hint texts under the list, the "Y" callout, the row's help text and
// picture; pane 0 also holds the list (anchor, up to eight visible items,
// the default list skin). [`Menu::extend_tag`] adds a pane per row of ours (a
// copy of the Custom Game pane, its picture included, with our help text),
// puts our help on the Content Download pane the Mods row shows, and
// ensures the list can show up to eight rows. It moves nothing: the list's
// top and the hint block stay where retail has them.
//
// The list lays items out from the anchor, each one item height below the
// last (`0x24bc12`): the height of the item's bounding box (`0x2bafa4`),
// 43 units for the default skin. More than four rows at that pitch run into
// the hint block, so after each layout of this list [`repitch`] moves the
// rows up to [`pitch`] apart, inside the room the retail four rows and
// the gap under them take. Eight items fit at 21 apart, the whole menu; nine
// would need 18, under [`MIN_PITCH`], so a longer menu shows eight and
// scrolls.

const VANILLA_PANES: u32 = 4;
/// The pane ours copies: Custom Game's, for its picture.
const TEMPLATE_PANE: u32 = 2;
const PANE_SIZE: u32 = 0x4c;
const LIST_SIZE: u32 = 0x18;
const TEXT_SIZE: u32 = 0x2c;
const BITMAP_SIZE: u32 = 0x38;
/// Offsets in a pane of the list, text and bitmap blocks (count, address).
const PANE_LISTS: usize = 0xc;
const PANE_TEXTS: usize = 0x1c;
const PANE_BITMAPS: usize = 0x24;
/// The default skin's item height, top and bottom, relative to the item
/// origin (its text's top, its background bitmap's bottom).
pub const ITEM_HEIGHT: i16 = 43;
pub const ITEM_TOP: i16 = 28;
pub const ITEM_BOTTOM: i16 = -15;
/// What the retail tag leaves between the fourth row's bottom and the hint
/// block's top (`hint_top` at -70, the list anchored at 95).
pub const RETAIL_GAP: i16 = 21;
/// The least gap kept under the last row.
pub const MIN_GAP: i16 = 2;
/// The smallest pitch allowed (the skin's text is 20 units tall).
pub const MIN_PITCH: i16 = 21;

/// The pitch for `rows` rows inside the room the retail four take, from
/// the first row's top to the hint block less [`MIN_GAP`]: the retail 43
/// for four, 37 for five, 29 for six, 24 for seven, 21 for eight, never under [`MIN_PITCH`].
pub fn pitch(rows: u32) -> i16 {
    if rows <= VANILLA_ITEMS {
        return ITEM_HEIGHT;
    }
    let room = VANILLA_ITEMS as i16 * ITEM_HEIGHT + RETAIL_GAP - ITEM_HEIGHT - MIN_GAP;
    (room / (rows as i16 - 1)).clamp(MIN_PITCH, ITEM_HEIGHT)
}

/// After the list handler laid out its rows (`0x24bc12`), moves row `i`'s
/// widgets up by `i` times (the laid-out pitch less `pitch`), so the rows
/// stand `pitch` apart from the first, which stays. A widget's rectangle is
/// at `+0x20` (top, left) and `+0x24` (bottom, right), as `0x2bacbc` writes
/// it; an item's widgets are `[item + 0x14]`, then each one's `+0x18`.
/// Returns how many rows moved: none for another list, four rows or fewer,
/// or a layout whose pitch is not a plain item height (20 to 60 units).
pub fn repitch<M: Gm>(m: &mut M, handler: u32, vtable: u32, pitch: i16) -> Option<usize> {
    if m.u32(handler)? != vtable {
        return Some(0);
    }
    let items = crate::speaker_row::children(&*m, handler)?;
    if items.len() <= VANILLA_ITEMS as usize {
        return Some(0);
    }
    use crate::speaker_row::children as widgets;
    let top = |m: &M, item: u32| -> Option<i16> {
        let first = *widgets(m, item)?.first()?;
        Some(m.u16(first + 0x20)? as i16)
    };
    let laid = top(&*m, items[0])?.saturating_sub(top(&*m, items[1])?);
    if !(20..=60).contains(&laid) || laid <= pitch {
        return Some(0);
    }
    let mut moved = 0;
    for (i, item) in items.iter().enumerate().skip(1) {
        let up = (laid - pitch).saturating_mul(i as i16);
        for w in widgets(&*m, *item)? {
            for at in [w + 0x20, w + 0x24] {
                let v = m.u16(at)? as i16;
                if !m.set_u16(at, v.saturating_add(up) as u16) {
                    return None;
                }
            }
        }
        moved += 1;
    }
    Some(moved)
}
/// Bytes [`Menu::extend_tag`] may use for its copies: [`ALL_PANES`] panes,
/// each with its list, text and bitmap blocks.
pub const TAG_SPACE: u32 = 0x2000;

fn get_i16(e: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([e[at], e[at + 1]])
}

fn put_i16(e: &mut [u8], at: usize, v: i16) {
    e[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// Copy the block of `size`-byte elements a pane names at `field` to
/// `*next`, with `edit` applied to each element, and point the pane at the
/// copy. An empty block is left alone.
fn copy_block<M: Gm>(
    m: &mut M,
    pane: &mut [u8],
    field: usize,
    size: u32,
    next: &mut u32,
    end: u32,
    mut edit: impl FnMut(usize, &mut [u8]),
) -> Option<()> {
    let count = u32::from_le_bytes(pane[field..field + 4].try_into().ok()?);
    let src = u32::from_le_bytes(pane[field + 4..field + 8].try_into().ok()?);
    if count as i32 <= 0 {
        return Some(());
    }
    if count > 64 {
        return None;
    }
    let len = count * size;
    if next.checked_add(len)? > end {
        return None;
    }
    let mut buf = vec![0; len as usize];
    if !m.rd(src, &mut buf) {
        return None;
    }
    for (i, e) in buf.chunks_mut(size as usize).enumerate() {
        edit(i, e);
    }
    if !m.wr(*next, &buf) {
        return None;
    }
    pane[field + 4..field + 8].copy_from_slice(&next.to_le_bytes());
    *next += (len + 3) & !3;
    Some(())
}

/// The string ids of a pane's text blocks.
fn text_ids<M: Gm>(m: &M, pane: u32) -> Option<Vec<u32>> {
    let (count, texts) = (m.u32(pane + PANE_TEXTS as u32)?, m.u32(pane + 0x20)?);
    (0..count.min(64))
        .map(|i| m.u32(texts + i * TEXT_SIZE + 0x24))
        .collect()
}

/// Give the added Xbox Live rows panes ([`ALL_PANES`], whichever rows are
/// on) and room in the list. Only the retail four-pane tag is extended, and
/// the pane array is published last.
fn extend_tag_with<M: Gm>(
    m: &mut M,
    wgit: u32,
    base: u32,
    space: u32,
    menu: Menu,
    mods: bool,
) -> Option<bool> {
    let panes = ALL_PANES;
    if m.u16(wgit.wrapping_add(4))? as i16 != SCREEN_ID || m.u32(wgit + 0x20)? != VANILLA_PANES {
        return Some(false);
    }
    let old = m.u32(wgit + 0x24)?;
    if m.u32(old + PANE_LISTS as u32)? as i32 <= 0 {
        return Some(false);
    }
    // Our help text replaces the one text that differs between the Custom
    // Game pane and the next (the row's help); none if that is unclear.
    let template = text_ids(m, old + TEMPLATE_PANE * PANE_SIZE)?;
    let next_pane = text_ids(m, old + (TEMPLATE_PANE + 1) * PANE_SIZE)?;
    let differing: Vec<usize> = (0..template.len().min(next_pane.len()))
        .filter(|&i| template[i] != next_pane[i])
        .collect();
    let help = (template.len() == next_pane.len() && differing.len() == 1).then(|| differing[0]);

    let end = base.checked_add(space)?;
    let mut next = base + panes * PANE_SIZE;
    for p in 0..panes {
        let ours = p >= VANILLA_PANES;
        let help_id = match p as i16 {
            CLANKERMATCH_PANE => CLANKERMATCH_HELP_ID,
            COOP_PANE => COOP_HELP_ID,
            CUSTOM_GAMES_PANE => CUSTOM_GAMES_HELP_ID,
            FORGE_PANE => FORGE_HELP_ID,
            RETAIL_DOWNLOAD_ROW if mods => MODS_HELP_ID,
            RETAIL_DOWNLOAD_ROW => MAPPACK_HELP_ID,
            _ => HELP_ID,
        };
        let ours_help = ours || p == RETAIL_DOWNLOAD_ROW as u32;
        let src = old + if ours { TEMPLATE_PANE } else { p } * PANE_SIZE;
        let mut pane = vec![0; PANE_SIZE as usize];
        if !m.rd(src, &mut pane) {
            return None;
        }
        copy_block(
            m,
            &mut pane,
            PANE_LISTS,
            LIST_SIZE,
            &mut next,
            end,
            |_, e| {
                let rows = menu.visible() as i16;
                if get_i16(e, 6) < rows {
                    put_i16(e, 6, rows);
                }
            },
        )?;
        copy_block(
            m,
            &mut pane,
            PANE_TEXTS,
            TEXT_SIZE,
            &mut next,
            end,
            |i, e| {
                if ours_help && help == Some(i) {
                    e[0x24..0x28].copy_from_slice(&help_id.to_le_bytes());
                }
            },
        )?;
        copy_block(
            m,
            &mut pane,
            PANE_BITMAPS,
            BITMAP_SIZE,
            &mut next,
            end,
            |_, _| {},
        )?;
        if !m.wr(base + p * PANE_SIZE, &pane) {
            return None;
        }
    }
    // Publish last: a reader sees the old array or the complete new one.
    (m.set_u32(wgit + 0x24, base) && m.set_u32(wgit + 0x20, panes)).then_some(true)
}

/// The `live.visibility` value of a privacy item id, as `0x19a148` maps
/// it: 0 open, 1 friends, anything else invite only.
pub fn visibility_of_privacy(id: u32) -> i64 {
    match id {
        0 => VISIBILITY_PUBLIC,
        1 => VISIBILITY_FRIENDS,
        _ => VISIBILITY_INVITE,
    }
}

/// The squad session's privacy as a `live.visibility` value, when there is
/// a session to read: the same gates as `0x64230`/`0x64260` (life cycle
/// ready, a session, state 3 to 8).
pub fn squad_visibility<M: Gm>(m: &M) -> Option<i64> {
    if m.u8(NETWORK_READY)? == 0 {
        return None;
    }
    let session = m.u32(SQUAD_SESSION)?;
    if session == 0 {
        return None;
    }
    let state = m.u32(session.wrapping_add(SESSION_STATE))? as i32;
    if !(3..=8).contains(&state) {
        return None;
    }
    Some(visibility_of_privacy(
        m.u32(session.wrapping_add(SESSION_PRIVACY))?,
    ))
}

/// Follow the party privacy screen into the `live.visibility` setting (and
/// so the HLE and an open lobby), and let the HLE read the squad session's
/// privacy whenever it advertises. Live off: nothing.
pub fn install_privacy(
    rt: &runtime::Runtime,
    hle: Option<&Arc<live_hle::Hle>>,
    store: Option<&Arc<settings::Store>>,
) -> anyhow::Result<()> {
    let Some(hle) = hle else { return Ok(()) };
    if let Some(store) = store {
        crate::live_glue::bind_visibility(hle, store);
    }
    hle.set_visibility_probe(Some(Arc::new(|g: &runtime::Guest| {
        squad_visibility(g).map(crate::live_glue::visibility)
    })));
    let Some(o) = rt.resolve_original(PRIVACY_CHOSEN) else {
        crate::say_once(format!("custom games: no generated entry at {PRIVACY_CHOSEN:#x}; privacy not followed"));
        return Ok(());
    };
    let o = Arc::new(o);
    let store = store.cloned();
    let hle = Arc::downgrade(hle);
    rt.edit_dispatch(|d| {
        d.register_override(
            PRIVACY_CHOSEN,
            "custom-games:privacy",
            move |cpu, ctx, _| {
                let v = visibility_of_privacy(cpu.eax);
                // Guest memory is not held here: the store's callbacks reach the
                // HLE, whose lock comes after guest memory.
                match &store {
                    Some(s) => match s.set(LIVE_VISIBILITY.key, settings::Value::Int(v)) {
                        Ok(o) => eprintln!("{}", o.describe(&LIVE_VISIBILITY)),
                        Err(e) => eprintln!("live.visibility: {e}"),
                    },
                    None => {
                        if let Some(h) = hle.upgrade() {
                            h.set_session_visibility(crate::live_glue::visibility(v));
                        }
                    }
                }
                Ok(o.invoke(cpu, ctx, PRIVACY_CHOSEN))
            },
        );
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_glue::{chosen_row, is_ours};
    use crate::video_screen::{player_mask, text_row};
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake(HashMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    #[test]
    fn quickmatch_uses_only_a_saved_descriptor_of_the_pressing_player() {
        let mut m = Fake::default();
        let slot = crate::skulls::SLOTS + crate::skulls::SLOT_STRIDE;
        m.set_u32(slot + 0xfc, 3);
        assert_eq!(quickmatch_selection(&m, 1), None);
        m.set_u32(slot, crate::skulls::SLOT_SIGNED_IN);
        assert_eq!(quickmatch_selection(&m, 1), Some(3));
        assert_eq!(quickmatch_selection(&m, 0), None);
        m.set_u32(slot + 0xfc, 16);
        assert_eq!(quickmatch_selection(&m, 1), None);
    }

    #[test]
    fn playlist_picker_header_follows_its_opener() {
        use crate::clankermatch::Scope;
        let mut m = Fake::default();
        let tag = 0x1000;
        m.set_u16(tag + 4, PLAYLIST_SCREEN as u16);
        for (scope, title) in [(Scope::Clankers, CLANKERMATCH_TEXT_ID),
            (Scope::QuickMatch, QUICKMATCH_TITLE), (Scope::All, OPTIMATCH_TITLE)] {
            playlist_header(&mut m, tag, scope);
            assert_eq!(m.u32(tag + PLAYLIST_HEADER), Some(title));
        }
    }

    fn regular_menu(coop: bool) -> Menu {
        Menu {
            coop,
            ..Menu::ALL
        }
    }

    /// The whole menu, and without the co-op row: rows and order.
    #[test]
    fn the_live_menu_rows_in_order_with_and_without_co_op() {
        use Row::*;
        let all = [
            Retail,
            Retail,
            Clankermatch,
            Coop,
            CustomGames,
            Forge,
            Browser,
            Mods,
        ];
        assert_eq!(Menu::ALL.layout(), all);
        assert_eq!(Menu::ALL.rows(), 8);
        let no_coop: Vec<Row> = all.into_iter().filter(|&r| r != Coop).collect();
        assert_eq!(regular_menu(false).layout(), no_coop);
        assert_eq!(regular_menu(false).rows(), 7);
        // The row constants are the whole menu's.
        for (row, kind) in [
            (CLANKERMATCH_ROW, Clankermatch),
            (COOP_ROW, Coop),
            (CUSTOM_GAMES_ROW, CustomGames),
            (FORGE_ROW, Forge),
            (BROWSER_ROW, Browser),
            (MODS_ROW, Mods),
        ] {
            assert_eq!(Menu::ALL.row_kind(row), kind, "row {row}");
            assert_eq!(Menu::ALL.row_of(kind), Some(row));
            // Without co-op, every row under it one higher.
            let up = if row > COOP_ROW { row - 1 } else { row };
            if kind != Coop {
                assert_eq!(regular_menu(false).row_kind(up), kind, "row {up}, no co-op");
            }
        }
        assert_eq!(regular_menu(false).row_of(Coop), None);
        assert_eq!(CLANKERMATCH_ROW, RETAIL_OPTIMATCH_ROW + 1);
        for menu in [Menu::ALL, regular_menu(false)] {
            for r in 0..2 {
                assert_eq!(menu.row_kind(r), Retail);
                assert_eq!(menu.row_text_id(r), None, "row {r} keeps the retail text");
            }
            let rows = menu.rows() as i16;
            assert_eq!(menu.row_kind(rows - 1), Mods, "Mods last");
            assert_eq!(menu.row_kind(rows), Retail, "past the last row");
            assert_eq!(menu.row_kind(-1), Retail);
        }
        // Forge off: its row is left out.
        let no_forge = Menu {
            forge: false,
            ..Menu::ALL
        };
        assert_eq!(no_forge.rows(), 7);
        assert_eq!(no_forge.row_of(Forge), None);
        assert_eq!(no_forge.row_kind(FORGE_ROW), Browser);
        let neither = Menu {
            forge: false,
            coop: false,
        };
        assert_eq!(neither.rows(), 6);
        assert_eq!(neither.row_kind(4), Browser);
    }

    /// Each row's label: ours for every row of ours, the lobby's own names
    /// for its modes, and Mods for retail's Content Download.
    #[test]
    fn the_rows_say_what_they_are() {
        let menu = Menu::ALL;
        for (row, id, text) in [
            (CLANKERMATCH_ROW, CLANKERMATCH_TEXT_ID, CLANKERMATCH_TEXT),
            (COOP_ROW, COOP_TEXT_ID, COOP_TEXT),
            (CUSTOM_GAMES_ROW, CUSTOM_GAMES_TEXT_ID, "Custom Games"),
            (FORGE_ROW, FORGE_TEXT_ID, "Forge"),
            (BROWSER_ROW, TEXT_ID, "Custom Game Browser"),
            (MODS_ROW, MODS_TEXT_ID, "Mod Downloader"),
        ] {
            assert_eq!(text_id(menu.row_kind(row), None), Some(id), "row {row}");
            let served = strings().into_iter().find(|(i, _)| *i == id).map(|(_, t)| t);
            assert_eq!(served, Some(text), "row {row}");
        }
        assert_eq!(CUSTOM_GAMES_TEXT, Tab::CustomGames.title_case());
        assert!(CLANKERMATCH_TEXT.len() < TEXT.len());
    }

    /// Custom Games and Forge each run retail's Custom Game
    /// press and ask for their own mode; no other row asks for one.
    #[test]
    fn each_mode_row_lands_on_its_mode() {
        for menu in [Menu::ALL, regular_menu(false)] {
            for (kind, mode) in [
                (Row::CustomGames, Tab::CustomGames),
                (Row::Forge, Tab::Forge),
            ] {
                let row = menu.row_of(kind).unwrap();
                assert_eq!(menu.row_kind(row).mode(), Some(mode), "{kind:?}");
                assert_eq!(
                    menu.row_kind(row).retail_press(),
                    Some(RETAIL_CUSTOM_GAME_ROW),
                    "{kind:?}"
                );
            }
            for row in 0..menu.rows() as i16 {
                let kind = menu.row_kind(row);
                if !matches!(kind, Row::CustomGames | Row::Forge) {
                    assert_eq!(kind.mode(), None, "row {row}");
                }
            }
        }
        // The press a mode row's request is taken by: once, in the lobby.
        crate::lobby_tabs::cancel_request();
        if let Some(mode) = Menu::ALL.row_kind(FORGE_ROW).mode() {
            crate::lobby_tabs::request(mode);
        }
        assert_eq!(crate::lobby_tabs::take_request(), Some(Tab::Forge));
        assert_eq!(crate::lobby_tabs::take_request(), None);
    }

    #[test]
    fn the_moved_rows_press_their_retail_rows() {
        // Clankermatch runs OptiMatch's press; Co-op Campaign rewrites its
        // own press (the layout glue), the browser and Mods open screens:
        // none of those is a retail_press.
        assert_eq!(
            Menu::ALL.row_kind(CLANKERMATCH_ROW).retail_press(),
            Some(RETAIL_OPTIMATCH_ROW)
        );
        for r in [0, 1, COOP_ROW, BROWSER_ROW, MODS_ROW] {
            assert_eq!(Menu::ALL.row_kind(r).retail_press(), None, "row {r}");
        }
    }

    #[test]
    fn panes_follow_the_rows() {
        let menu = Menu::ALL;
        // Extended tag: each row of ours its own pane, Mods retail's
        // Content Download pane (3).
        for (row, pane) in [
            (CLANKERMATCH_ROW, CLANKERMATCH_PANE),
            (COOP_ROW, COOP_PANE),
            (CUSTOM_GAMES_ROW, CUSTOM_GAMES_PANE),
            (FORGE_ROW, FORGE_PANE),
            (BROWSER_ROW, BROWSER_PANE),
            (MODS_ROW, 3),
        ] {
            assert_eq!(menu.pane_of(row, ALL_PANES), Pane::Show(pane), "row {row}");
        }
        // Without co-op the panes follow the rows, not the places.
        let no_coop = regular_menu(false);
        assert_eq!(no_coop.pane_of(COOP_ROW, ALL_PANES), Pane::Show(CUSTOM_GAMES_PANE));
        assert_eq!(no_coop.pane_of(BROWSER_ROW, ALL_PANES), Pane::Show(3));
        for r in 0..2 {
            assert_eq!(menu.pane_of(r, ALL_PANES), Pane::Retail);
            assert_eq!(menu.pane_of(r, VANILLA_PANES), Pane::Retail);
        }
        // A tag we did not extend has none of ours: keep the shown one;
        // Custom Games shows retail's Custom Game pane, Mods its own.
        assert_eq!(menu.pane_of(CUSTOM_GAMES_ROW, VANILLA_PANES), Pane::Show(2));
        assert_eq!(menu.pane_of(MODS_ROW, VANILLA_PANES), Pane::Show(3));
        for row in [CLANKERMATCH_ROW, COOP_ROW, FORGE_ROW, BROWSER_ROW] {
            assert_eq!(menu.pane_of(row, VANILLA_PANES), Pane::Keep, "row {row}");
        }
    }

    #[test]
    fn the_map_pack_dialog_gets_our_title_and_message_only() {
        let entry = 0x6000;
        let mut m = Fake::default();
        for off in (0..0x20).step_by(4) {
            m.set_u32(entry + off, 0x0bad_0000 + off);
        }
        assert!(!answer_error(&mut m, entry, 0xa3), "another error");
        assert_eq!(m.u32(entry + ENTRY_TITLE), Some(0x0bad_000c));
        assert!(answer_error(&mut m, entry, MAPPACK_ERROR));
        assert_eq!(m.u32(entry + ENTRY_TITLE), Some(MAPPACK_TITLE_ID));
        assert_eq!(m.u32(entry + ENTRY_MESSAGE), Some(MAPPACK_MESSAGE_ID));
        // The string table, buttons and kind stay as the lookup left them.
        for off in [0, 4, 8, 0x14, 0x18, 0x1c] {
            assert_eq!(m.u32(entry + off), Some(0x0bad_0000 + off), "+{off:#x}");
        }
    }

    #[test]
    fn our_texts_are_ours_short_and_plain() {
        let ids: Vec<u32> = strings().iter().map(|(i, _)| *i).collect();
        assert!(ids.iter().all(|i| is_ours(*i)));
        // Clear of the other rows' ids and the model's range.
        let mut taken: Vec<u32> = crate::layout_glue::strings()
            .iter()
            .map(|(i, _)| *i)
            .collect();
        taken.extend([
            crate::speaker_row::SMALL_ON_ID,
            crate::speaker_row::SMALL_OFF_ID,
        ]);
        for i in &ids {
            assert!(!taken.contains(i), "{i:#x}");
            assert!(!crate::video_screen::is_model_id(*i), "{i:#x}");
            assert_eq!(ids.iter().filter(|j| *j == i).count(), 1);
        }
        // A title and three lines, broken as the game's strings are, each
        // short enough for the dialog's message box.
        let lines: Vec<&str> = MAPPACK_MESSAGE.split("\r\n").collect();
        assert_eq!(lines.len(), 3);
        assert!(
            lines
                .iter()
                .all(|l| l.len() <= 36 && !l.contains(['\r', '\n', '|']))
        );
        assert!(MAPPACK_MESSAGE.contains("mappack.iso") && MAPPACK_MESSAGE.contains("halo2.iso"));
        assert!(MAPPACK_TITLE.len() <= 24 && TEXT.len() <= 20);
    }

    #[test]
    fn the_fifth_item_widget_is_ours_on_the_live_list_only() {
        use crate::speaker_row::{ITEM_SIZE, children, item_needed};
        let mut m = Fake::default();
        let handler = 0x30_0000;
        let retail: Vec<u32> = (0..VANILLA_ITEMS)
            .map(|i| handler + 0x88 + i * ITEM_SIZE)
            .collect();
        let link = |m: &mut Fake, kids: &[u32]| {
            m.set_u32(handler + 0x14, kids[0]);
            for (i, k) in kids.iter().enumerate() {
                m.set_u32(k + 0x18, kids.get(i + 1).copied().unwrap_or(0));
            }
        };
        m.set_u32(handler, HANDLER_VTABLE);
        link(&mut m, &retail);
        let ours = 0x40_0000;
        assert_eq!(item_needed(&m, handler, HANDLER_VTABLE, ours), Some(true));
        let mut five = retail.clone();
        five.push(ours);
        link(&mut m, &five);
        assert_eq!(
            children(&m, handler).unwrap().len(),
            5,
            "one child per row: no scroll"
        );
        assert_eq!(item_needed(&m, handler, HANDLER_VTABLE, ours), Some(false));
        // The Controls hook's vtable does not match this list, nor ours it.
        assert_eq!(
            item_needed(&m, handler, crate::speaker_row::HANDLER_VTABLE, ours),
            None
        );
    }

    // ---- a synthetic Live menu tag (invented numbers, the retail shape) ----

    const WGIT: u32 = 0x10_0000;
    const BASE: u32 = 0x50_0000;
    /// Anchor, visible items.
    const ANCHOR: (i16, i16) = (-200, 100);

    fn put_i16s(m: &mut Fake, at: u32, v: &[i16]) {
        for (i, x) in v.iter().enumerate() {
            m.set_u16(at + 2 * i as u32, *x as u16);
        }
    }

    /// Four panes. Each has: a hint text under the list in its column, a
    /// panel text under the list's bottom but to the right, a help text
    /// (its sid differs per pane), a callout bitmap under the list and a
    /// decoration above. Pane 0 also has the list.
    fn live_tag(m: &mut Fake, visible: i16) -> u32 {
        m.set_u16(WGIT + 4, SCREEN_ID as u16);
        m.set_u32(WGIT + 0x20, VANILLA_PANES);
        let panes = 0x11_0000;
        m.set_u32(WGIT + 0x24, panes);
        for p in 0..VANILLA_PANES {
            let pane = panes + p * PANE_SIZE;
            m.wr(pane, &[0; PANE_SIZE as usize]);
            let texts = 0x12_0000 + p * 0x1000;
            let bitmaps = 0x13_0000 + p * 0x1000;
            m.set_u32(pane + 0x1c, 3);
            m.set_u32(pane + 0x20, texts);
            m.set_u32(pane + 0x24, 2);
            m.set_u32(pane + 0x28, bitmaps);
            // top, left, bottom, right; then the sid.
            for (i, (rect, sid)) in [
                ([-60, -210, -80, -10], 0x0800_0001),
                ([-50, 0, -170, 260], 0x0900_0002),
                ([150, 0, -20, 120], 0x0a00_0100 + p),
            ]
            .into_iter()
            .enumerate()
            {
                let t = texts + i as u32 * TEXT_SIZE;
                m.wr(t, &[0x5a; TEXT_SIZE as usize]);
                put_i16s(m, t + 0x1c, &rect);
                m.set_u32(t + 0x24, sid);
            }
            // top-left x, y.
            for (i, xy) in [[-190, -150], [-220, 150]].into_iter().enumerate() {
                let b = bitmaps + i as u32 * BITMAP_SIZE;
                m.wr(b, &[0x3c; BITMAP_SIZE as usize]);
                put_i16s(m, b + 0xc, &xy);
            }
        }
        let list = 0x14_0000;
        m.set_u32(panes + 0xc, 1);
        m.set_u32(panes + 0x10, list);
        m.wr(list, &[0; LIST_SIZE as usize]);
        put_i16s(m, list + 6, &[visible, ANCHOR.0, ANCHOR.1]);
        WGIT
    }

    fn bytes(m: &Fake, at: u32, len: u32) -> Vec<u8> {
        let mut b = vec![0; len as usize];
        assert!(m.rd(at, &mut b));
        b
    }

    fn i16_at(m: &Fake, at: u32) -> i16 {
        m.u16(at).unwrap() as i16
    }

    #[test]
    fn the_tag_gets_a_pane_per_row_of_ours_with_its_help() {
        let mut m = Fake::default();
        let wgit = live_tag(&mut m, 7);
        let old_panes = m.u32(wgit + 0x24).unwrap();
        let retail_texts = |m: &Fake| -> Vec<Vec<u8>> {
            (0..VANILLA_PANES)
                .map(|p| bytes(m, 0x12_0000 + p * 0x1000, 3 * TEXT_SIZE))
                .collect()
        };
        let retail = retail_texts(&m);
        assert_eq!(regular_menu(false).extend_tag(&mut m, wgit, BASE), Some(true));
        // Every pane, whichever rows are on.
        assert_eq!(m.u32(wgit + 0x20), Some(ALL_PANES));
        assert_eq!(m.u32(wgit + 0x24), Some(BASE));
        // The retail arrays are untouched; everything new is in our space.
        assert_eq!(retail_texts(&m), retail);
        assert_eq!(m.u32(old_panes + 0x1c), Some(3));
        assert!(m.0.keys().all(|a| *a < BASE + TAG_SPACE));

        let pane = |p: u32| BASE + p * PANE_SIZE;
        let text = |m: &Fake, p: u32, i: u32| m.u32(pane(p) + 0x20).unwrap() + i * TEXT_SIZE;
        let bitmap = |m: &Fake, p: u32, i: u32| m.u32(pane(p) + 0x28).unwrap() + i * BITMAP_SIZE;
        // The list stays where retail anchors it; it shows its seven.
        let list = m.u32(pane(0) + 0x10).unwrap();
        assert_eq!(i16_at(&m, list + 6), 7);
        assert_eq!(i16_at(&m, list + 8), ANCHOR.0);
        assert_eq!(i16_at(&m, list + 0xa), ANCHOR.1);
        for p in 0..ALL_PANES {
            // Nothing moves: the hint, the panel, the help, the callout.
            let hint = text(&m, p, 0);
            assert_eq!(i16_at(&m, hint + 0x1c), -60, "pane {p}");
            assert_eq!(i16_at(&m, hint + 0x20), -80);
            assert_eq!(i16_at(&m, text(&m, p, 1) + 0x1c), -50);
            assert_eq!(i16_at(&m, text(&m, p, 2) + 0x1c), 150);
            assert_eq!(i16_at(&m, bitmap(&m, p, 0) + 0xe), -150);
            assert_eq!(i16_at(&m, bitmap(&m, p, 1) + 0xe), 150);
            // The other bytes of each element are copied.
            assert_eq!(m.u8(text(&m, p, 0) + 0x2a), Some(0x5a));
            assert_eq!(m.u8(bitmap(&m, p, 0) + 0x30), Some(0x3c));
        }
        // Panes 0 to 2 keep their help; 3 (Mods) and ours have ours.
        for p in 0..3 {
            assert_eq!(m.u32(text(&m, p, 2) + 0x24), Some(0x0a00_0100 + p));
        }
        for (p, help) in [
            (3, MAPPACK_HELP_ID),
            (CLANKERMATCH_PANE, CLANKERMATCH_HELP_ID),
            (BROWSER_PANE, HELP_ID),
            (COOP_PANE, COOP_HELP_ID),
            (CUSTOM_GAMES_PANE, CUSTOM_GAMES_HELP_ID),
            (FORGE_PANE, FORGE_HELP_ID),
        ] {
            assert_eq!(m.u32(text(&m, p as u32, 2) + 0x24), Some(help), "pane {p}");
        }
        for p in VANILLA_PANES..ALL_PANES {
            // All of ours are the Custom Game pane.
            assert_eq!(m.u32(text(&m, p, 0) + 0x24), Some(0x0800_0001));
            assert_eq!(m.u32(pane(p) + 0xc), Some(0), "no list on our pane");
        }
        // Once done, never again.
        assert_eq!(regular_menu(false).extend_tag(&mut m, wgit, BASE + TAG_SPACE), Some(false));
        assert_eq!(m.u32(wgit + 0x24), Some(BASE));
    }

    /// Every row of the menu has a line of help on the pane it shows.
    #[test]
    fn every_row_has_its_help() {
        let help_of = |m: &Fake, p: i16| {
            let texts = m.u32(BASE + p as u32 * PANE_SIZE + 0x20).unwrap();
            m.u32(texts + 2 * TEXT_SIZE + 0x24).unwrap()
        };
        for mods in [false, true] {
            let mut m = Fake::default();
            let wgit = live_tag(&mut m, 7);
            let menu = Menu::ALL;
            assert_eq!(
                extend_tag_with(&mut m, wgit, BASE, TAG_SPACE, menu, mods),
                Some(true)
            );
            let served: Vec<(u32, &str)> = strings();
            for row in 2..menu.rows() as i16 {
                let Pane::Show(p) = menu.pane_of(row, ALL_PANES) else {
                    panic!("row {row} shows no pane");
                };
                let id = help_of(&m, p);
                let text = served.iter().find(|(i, _)| *i == id).map(|(_, t)| *t);
                let text = text.unwrap_or_else(|| panic!("row {row}: help {id:#x} not served"));
                assert!(
                    !text.is_empty() && text.len() <= 72 && !text.contains('\n'),
                    "row {row}: {text:?}"
                );
            }
            let mods_help = help_of(&m, 3);
            assert_eq!(mods_help, if mods { MODS_HELP_ID } else { MAPPACK_HELP_ID });
        }
    }

    #[test]
    fn the_live_menu_shows_every_row_without_scrolling() {
        for (menu, rows) in [(Menu::ALL, 8), (regular_menu(false), 7)] {
            assert_eq!(menu.count(LIST_NAME, VANILLA_ROWS), rows);
            assert_eq!(menu.visible(), rows, "{rows} rows");
            assert!(menu.rows() <= VISIBLE_ITEMS, "no row scrolls");
            // Item widgets of ours after the retail four: one per extra row.
            assert!(menu.visible() - VANILLA_ITEMS <= MAX_EXTRA_ITEMS);

            let mut m = Fake::default();
            let wgit = live_tag(&mut m, 4);
            assert_eq!(menu.extend_tag(&mut m, wgit, BASE), Some(true));
            let list = m.u32(BASE + 0x10).unwrap();
            assert_eq!(i16_at(&m, list + 6), rows as i16);
        }
    }

    #[test]
    fn eight_rows_fit_with_the_retail_gaps_and_more_scroll() {
        assert_eq!(pitch(4), ITEM_HEIGHT, "retail: as it was");
        assert_eq!(pitch(5), 37);
        assert_eq!(pitch(6), 29);
        assert_eq!(pitch(7), 24);
        assert_eq!(pitch(8), 21);
        // The retail tag: the list anchored at 95, `hint_top` at -70 (21
        // under the fourth row's bottom). Every row ends above it, the
        // first row's top where retail's is, and the text (20 units) fits
        // in a row.
        let (anchor, hint) = (95i16, -70i16);
        assert_eq!(hint, anchor - 3 * ITEM_HEIGHT + ITEM_BOTTOM - RETAIL_GAP);
        let room = anchor + ITEM_BOTTOM - (hint + MIN_GAP);
        assert_eq!(room, 148);
        for rows in [5u32, 6, 7, 8] {
            let p = pitch(rows);
            let last_bottom = anchor - (rows as i16 - 1) * p + ITEM_BOTTOM;
            assert!(
                last_bottom >= hint + MIN_GAP,
                "{rows} rows end at {last_bottom}"
            );
            assert!(p >= MIN_PITCH && MIN_PITCH > 20);
        }
        // Nine at once would need 18 apart, under the least pitch: it does
        // not fit, so eight show and the rest scroll.
        let rows = 9i16;
        assert!(room / (rows - 1) < MIN_PITCH, "{rows} rows");
        // The tag: the list shows eight; its anchor and the hints stay.
        let mut m = Fake::default();
        let wgit = live_tag(&mut m, 4);
        assert_eq!(Menu::ALL.extend_tag(&mut m, wgit, BASE), Some(true));
        let list = m.u32(BASE + 0x10).unwrap();
        assert_eq!(i16_at(&m, list + 6), 8, "the list shows eight");
        assert_eq!(i16_at(&m, list + 0xa), ANCHOR.1);
        let hint = m.u32(BASE + 0x20).unwrap();
        assert_eq!(i16_at(&m, hint + 0x1c), -60);
    }

    /// Six item widgets laid out 43 apart from `anchor`, two widgets each
    /// (text and bar), as `0x24bc12` leaves them.
    fn laid_out(m: &mut Fake, handler: u32, vtable: u32, rows: u32, anchor: i16) -> Vec<u32> {
        m.set_u32(handler, vtable);
        let items: Vec<u32> = (0..rows).map(|i| 0x60_0000 + i * 0x1000).collect();
        m.set_u32(handler + 0x14, items[0]);
        for (i, item) in items.iter().enumerate() {
            m.set_u32(item + 0x18, items.get(i + 1).copied().unwrap_or(0));
            let origin = anchor - i as i16 * ITEM_HEIGHT;
            let (text, bar) = (item + 0x100, item + 0x200);
            m.set_u32(item + 0x14, text);
            m.set_u32(text + 0x18, bar);
            m.set_u32(bar + 0x18, 0);
            put_i16s(m, text + 0x20, &[origin + ITEM_TOP, -236, origin + 8, -46]);
            put_i16s(
                m,
                bar + 0x20,
                &[origin + 6, -246, origin + ITEM_BOTTOM, -46],
            );
        }
        items
    }

    #[test]
    fn the_rows_are_moved_to_the_pitch_and_end_above_the_hints() {
        let mut m = Fake::default();
        let handler = 0x50_0000;
        let items = laid_out(&mut m, handler, HANDLER_VTABLE, 6, 95);
        let p = pitch(6);
        assert_eq!(repitch(&mut m, handler, HANDLER_VTABLE, p), Some(5));
        let top = |m: &Fake, i: usize| i16_at(m, items[i] + 0x100 + 0x20);
        let bar_bottom = |m: &Fake, i: usize| i16_at(m, items[i] + 0x200 + 0x24);
        assert_eq!(top(&m, 0), 95 + ITEM_TOP, "the first row stays");
        for i in 1..6 {
            assert_eq!(top(&m, i - 1) - top(&m, i), p, "row {i}");
            // Both edges of every widget move; left and right do not.
            assert_eq!(i16_at(&m, items[i] + 0x100 + 0x24), top(&m, i) - 20);
            assert_eq!(i16_at(&m, items[i] + 0x100 + 0x22), -236);
        }
        assert!(bar_bottom(&m, 5) >= -70 + MIN_GAP, "{}", bar_bottom(&m, 5));
        // Another list, or four rows: left alone.
        let mut other = Fake::default();
        laid_out(&mut other, handler, 0x0045_0000, 6, 95);
        assert_eq!(repitch(&mut other, handler, HANDLER_VTABLE, p), Some(0));
        let mut four = Fake::default();
        laid_out(&mut four, handler, HANDLER_VTABLE, 4, 95);
        assert_eq!(repitch(&mut four, handler, HANDLER_VTABLE, p), Some(0));
        // Run after each layout, never twice on one: a layout already at
        // the pitch is left alone.
        assert_eq!(repitch(&mut m, handler, HANDLER_VTABLE, p), Some(0));
        assert_eq!(top(&m, 5), 95 + ITEM_TOP - 5 * p);
    }

    #[test]
    fn the_mods_row_says_mods_and_its_pane_says_why() {
        let label = 0x20ff_d100;
        let menu = Menu::ALL;
        assert_eq!(text_id(menu.row_kind(MODS_ROW), Some(label)), Some(label));
        assert_eq!(
            text_id(regular_menu(false).row_kind(BROWSER_ROW), Some(label)),
            Some(label),
            "Mods, one higher without co-op"
        );
        assert_eq!(text_id(menu.row_kind(MODS_ROW), None), Some(MODS_TEXT_ID));
        assert_eq!(text_id(menu.row_kind(BROWSER_ROW), Some(label)), Some(TEXT_ID));
        assert_eq!(text_id(menu.row_kind(1), Some(label)), None, "retail rows");
        let text =
            |m: &Fake, p: u32, i: u32| m.u32(BASE + p * PANE_SIZE + 0x20).unwrap() + i * TEXT_SIZE;
        for (mods, want) in [(true, MODS_HELP_ID), (false, MAPPACK_HELP_ID)] {
            let mut m = Fake::default();
            let wgit = live_tag(&mut m, 7);
            assert_eq!(
                extend_tag_with(&mut m, wgit, BASE, TAG_SPACE, menu, mods),
                Some(true)
            );
            assert_eq!(m.u32(text(&m, 3, 2) + 0x24), Some(want), "mods {mods}");
            for p in 0..3 {
                assert_eq!(m.u32(text(&m, p, 2) + 0x24), Some(0x0a00_0100 + p));
            }
            assert_eq!(
                m.u32(text(&m, 3, 0) + 0x24),
                Some(0x0800_0001),
                "only the help"
            );
        }
        assert!(MODS_HELP.len() <= HELP.len() + 8);
    }

    /// Mods is retail's Content Download row under a new name: its own
    /// press (the Mods screen, else the Map Pack dialog, the layout glue's
    /// `Row::Mods` arm), not a retail press rewritten nor a lobby mode, on
    /// Content Download's pane.
    #[test]
    fn mods_opens_what_content_download_opened() {
        for menu in [Menu::ALL, regular_menu(false)] {
            let row = menu.row_of(Row::Mods).unwrap();
            let kind = menu.row_kind(row);
            assert_eq!(kind, Row::Mods);
            assert_eq!(kind.retail_press(), None);
            assert_eq!(kind.mode(), None);
            assert_eq!(menu.pane_of(row, ALL_PANES), Pane::Show(RETAIL_DOWNLOAD_ROW));
            assert_eq!(menu.pane_of(row, VANILLA_PANES), Pane::Show(RETAIL_DOWNLOAD_ROW));
            assert!(!remember_row(&mut Fake::default(), MODS_ROW), "opens over the menu");
        }
        // Without the mod repository, the dialog keeps our title and text.
        let mut m = Fake::default();
        for off in (0..0x20).step_by(4) {
            m.set_u32(0x6000 + off, 0);
        }
        assert!(answer_error(&mut m, 0x6000, MAPPACK_ERROR));
        assert_eq!(m.u32(0x6000 + ENTRY_TITLE), Some(MAPPACK_TITLE_ID));
    }

    #[test]
    fn a_list_showing_fewer_than_seven_is_raised_to_seven() {
        let mut m = Fake::default();
        let wgit = live_tag(&mut m, 4);
        assert_eq!(regular_menu(false).extend_tag(&mut m, wgit, BASE), Some(true));
        let list = m.u32(BASE + 0x10).unwrap();
        assert_eq!(i16_at(&m, list + 6), 7);
    }

    #[test]
    fn opening_live_repeatedly_only_extends_the_private_tag() {
        let mut m = Fake::default();
        m.wr(WGIT, &[0; crate::list_class::SCREEN_ROOT_SIZE as usize]);
        let source = live_tag(&mut m, 4);
        let original = m.u32(source + 0x24);
        let root = BASE + TAG_SPACE;
        for coop in [true, false, true] {
            assert_eq!(regular_menu(coop).copy_tag(&mut m, source, root, BASE), Some(true));
            assert_eq!(m.u32(root + 0x24), Some(BASE));
            assert_eq!(m.u32(root + 0x20), Some(ALL_PANES));
            assert_eq!(m.u32(source + 0x24), original);
            assert_eq!(m.u32(source + 0x20), Some(VANILLA_PANES));
        }
    }

    #[test]
    fn other_tags_are_left_alone() {
        let mut m = Fake::default();
        let wgit = live_tag(&mut m, 7);
        m.set_u16(wgit + 4, 39);
        assert_eq!(regular_menu(false).extend_tag(&mut m, wgit, BASE), Some(false));
        m.set_u16(wgit + 4, SCREEN_ID as u16);
        m.set_u32(wgit + 0x20, 3);
        assert_eq!(regular_menu(false).extend_tag(&mut m, wgit, BASE), Some(false));
        m.set_u32(wgit + 0x20, VANILLA_PANES);
        let panes = m.u32(wgit + 0x24).unwrap();
        m.set_u32(panes + 0xc, 0);
        assert_eq!(regular_menu(false).extend_tag(&mut m, wgit, BASE), Some(false), "no list");
        assert_eq!(m.u32(wgit + 0x24), Some(panes));
        assert!(m.0.keys().all(|a| *a < BASE), "nothing written");
        // Too little space: nothing published.
        m.set_u32(panes + 0xc, 1);
        let mut tight = Fake(m.0.clone());
        assert_eq!(
            extend_tag_with(&mut tight, wgit, BASE, PANE_SIZE * 6, regular_menu(false), false),
            None
        );
        assert_eq!(tight.u32(wgit + 0x20), Some(VANILLA_PANES));
    }

    #[test]
    fn only_the_live_menu_grows_by_our_rows() {
        assert_eq!(regular_menu(false).count(LIST_NAME, 4), 7);
        assert_eq!(regular_menu(true).count(LIST_NAME, 4), 8);
        assert_eq!(regular_menu(false).count(LIST_NAME, 8), 8, "not twice");
        assert_eq!(regular_menu(false).count(LIST_NAME, 3), 3, "not another size");
        assert_eq!(regular_menu(false).count(crate::speaker_row::LIST_NAME, 4), 4);
        assert!(is_ours(TEXT_ID));
    }

    #[test]
    fn the_row_hooks_read_the_retail_arguments() {
        // Row text: the item at [esp+4], its row at +0x70.
        let mut m = Fake::default();
        let esp = 0x1000;
        m.set_u32(esp + 4, 0x2000);
        m.set_u16(0x2000 + 0x70, BROWSER_ROW as u16);
        assert_eq!(text_row(&m, esp), Some(BROWSER_ROW));
        // Row chosen: the row word through [esp+8], the player through
        // [[esp+4]+4], the mask the browser is opened for.
        m.set_u32(esp + 8, 0x3000);
        m.set_u16(0x3000, BROWSER_ROW as u16);
        m.set_u32(esp + 4, 0x4000);
        m.set_u32(0x4000, 0x5000);
        m.set_u32(0x5000 + 4, 2);
        assert_eq!(chosen_row(&m, esp), Some(BROWSER_ROW));
        assert_eq!(player_mask(&m, esp), Some(1 << 2));
    }

    /// A browser screen as its constructor leaves it: the vtable, and the
    /// Live byte its loader passed (0 from `0x253185`, 1 from `0x2531c9`).
    fn browser(m: &mut Fake, screen: u32, live: u8) {
        m.set_u32(screen, BROWSER_VTABLE);
        m.wr(screen + BROWSER_LIVE, &[live]);
    }

    /// The back request the browser's handler builds (`0x149f49` with kind
    /// 7) for the System Link loader: kind, mask, the screen's +0x74/+0x78,
    /// three -1 words, the loader.
    fn back_request(m: &mut Fake, at: u32, kind: u16) {
        m.set_u16(at, kind);
        m.set_u16(at + 2, 1);
        m.set_u32(at + 4, 5);
        m.set_u32(at + 8, 4);
        m.set_u32(at + 0xc, 0);
        for off in [0x10, 0x14, 0x18] {
            m.set_u32(at + off, u32::MAX);
        }
        m.set_u32(at + REQUEST_LOADER, BROWSER_BACK_LOADER);
    }

    #[test]
    fn only_the_live_browser_goes_back_to_the_live_menu() {
        let mut m = Fake::default();
        let (lan, live, other) = (0x20_0000, 0x21_0000, 0x22_0000);
        browser(&mut m, lan, 0);
        browser(&mut m, live, 1);
        m.set_u32(other, 0x0045_be30);
        m.wr(other + BROWSER_LIVE, &[1]);
        // The LAN browser (main menu, System Link) is left as retail.
        assert!(!browser_is_live(&m, lan));
        assert!(browser_is_live(&m, live));
        assert!(!browser_is_live(&m, other), "another screen with a 1 there");
        assert!(!browser_is_live(&m, 0x30_0000), "nothing mapped");
    }

    #[test]
    fn a_back_request_is_sent_to_the_live_menu() {
        let mut m = Fake::default();
        let req = 0x7000;
        back_request(&mut m, req, REQUEST_BACK);
        let before = bytes(&m, req, 0x1c);
        assert_eq!(back_loader(&mut m, req), Some(LIVE_MENU_LOADER));
        assert_eq!(m.u32(req + REQUEST_LOADER), Some(LIVE_MENU_LOADER));
        // Kind, mask, the window words and the rest go to the Live menu's
        // loader as the browser built them, as `0x19b0ad` builds its own.
        assert_eq!(bytes(&m, req, 0x1c), before);
        // An opening request (kind 0, `0x149f1e`) is not a back press.
        let open = 0x7100;
        back_request(&mut m, open, 0);
        assert_eq!(back_loader(&mut m, open), None);
        assert_eq!(m.u32(open + REQUEST_LOADER), Some(BROWSER_BACK_LOADER));
        assert_eq!(back_loader(&mut m, 0x9_0000), None, "unmapped");
    }

    #[test]
    fn a_press_leaves_the_cursor_on_its_row_and_the_build_restores_it() {
        let mut m = Fake::default();
        m.set_u32(LAST_ROW, 1);
        // `0x231170` restores rows 0 to 3: Clankermatch and Co-op Campaign
        // (Clankermatch's retail press stored OptiMatch's 1); the build
        // hook leaves those to it.
        for row in [CLANKERMATCH_ROW, COOP_ROW] {
            assert!(remember_row(&mut m, row));
            assert_eq!(m.u32(LAST_ROW), Some(row as u32));
            assert_eq!(cursor_row(&m), None, "row {row}: retail's");
        }
        // The mode rows (their retail press stored 2) and the browser are
        // past what it restores: stored, and the build hook puts the
        // cursor there.
        for row in [CUSTOM_GAMES_ROW, FORGE_ROW, BROWSER_ROW] {
            assert!(remember_row(&mut m, row));
            assert_eq!(m.u32(LAST_ROW), Some(row as u32));
            assert_eq!(cursor_row(&m), Some(row as u32));
        }
        // Mods opens over the menu; nothing is stored for it, nor for no
        // row.
        assert!(!remember_row(&mut m, MODS_ROW));
        assert!(!remember_row(&mut m, -1));
        assert_eq!(m.u32(LAST_ROW), Some(BROWSER_ROW as u32));
        // Retail's "none" (-1) and anything out of range: retail's build.
        m.set_u32(LAST_ROW, u32::MAX);
        assert_eq!(cursor_row(&m), None);
        m.set_u32(LAST_ROW, MODS_ROW as u32);
        assert_eq!(cursor_row(&m), None);
    }

    #[test]
    fn the_build_puts_the_cursor_on_our_row() {
        use engine::GuestMemory;
        use engine::entry::{Regs, Ret, testing};
        let mut g = engine::testing::FakeGuest::new();
        g.map(SEARCH_FLAG, 8);
        g.write(SEARCH_STATE, &7u32.to_le_bytes()).unwrap();
        g.write(SEARCH_FLAG, &[1]).unwrap();
        const SCREEN: u32 = 0x0080_0000;
        let f = |cx: engine::entry::Cx| -> engine::entry::EntryFuture {
            Box::pin(build_menu(cx, SCREEN, 4))
        };
        let run = testing::run(&f, g, Regs::default(), |c, _| Regs {
            eax: if c.target == MENU_PLAYERS { 0b101 } else { 0 },
            ..Regs::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        // Retail's steps, in order, then the cursor.
        let calls: Vec<_> = run
            .calls
            .iter()
            .map(|c| (c.target, c.eax, c.ecx, c.args.clone()))
            .collect();
        assert_eq!(
            calls,
            vec![
                (MENU_PLAYERS, None, Some(SCREEN), vec![]),
                (MENU_FILL, None, Some(SCREEN), vec![0b101]),
                (MENU_FINISH, None, Some(SCREEN), vec![]),
                (LIST_SET_ROW, Some(SCREEN + MENU_LIST), None, vec![4]),
            ]
        );
        assert_eq!(run.guest.u32(SEARCH_STATE), Ok(u32::MAX));
        assert_eq!(run.guest.u8(SEARCH_FLAG), Ok(0));
    }

    #[test]
    fn privacy_maps_like_the_title_and_reads_only_a_live_squad() {
        assert_eq!(visibility_of_privacy(0), VISIBILITY_PUBLIC);
        assert_eq!(visibility_of_privacy(1), VISIBILITY_FRIENDS);
        assert_eq!(visibility_of_privacy(2), VISIBILITY_INVITE);
        assert_eq!(visibility_of_privacy(7), VISIBILITY_INVITE);
        let mut m = Fake::default();
        assert_eq!(squad_visibility(&m), None, "nothing mapped");
        m.wr(NETWORK_READY, &[0]);
        assert_eq!(squad_visibility(&m), None, "life cycle not ready");
        m.wr(NETWORK_READY, &[1]);
        m.set_u32(SQUAD_SESSION, 0);
        assert_eq!(squad_visibility(&m), None, "no session");
        let s = 0x10_0000;
        m.set_u32(SQUAD_SESSION, s);
        m.set_u32(s + SESSION_PRIVACY, 1);
        for (state, want) in [
            (2, None),
            (3, Some(VISIBILITY_FRIENDS)),
            (5, Some(VISIBILITY_FRIENDS)),
            (8, Some(VISIBILITY_FRIENDS)),
            (9, None),
        ] {
            m.set_u32(s + SESSION_STATE, state);
            assert_eq!(squad_visibility(&m), want, "state {state}");
        }
        m.set_u32(s + SESSION_STATE, 5);
        m.set_u32(s + SESSION_PRIVACY, 2);
        assert_eq!(squad_visibility(&m), Some(VISIBILITY_INVITE));
    }

    #[test]
    fn privacy_install_without_live_or_game_is_harmless() {
        let rt = runtime::Runtime::new(
            runtime::Guest::new(),
            runtime::dispatch::Dispatch::default(),
        );
        install_privacy(&rt, None, None).unwrap();
        let hle = live_hle::Hle::with_address(
            live_hle::LiveConfig {
                enabled: true,
                ..Default::default()
            },
            "127.0.0.1:3074".parse().unwrap(),
        )
        .unwrap();
        let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
        store
            .set(
                LIVE_VISIBILITY.key,
                settings::Value::Int(VISIBILITY_FRIENDS),
            )
            .unwrap();
        install_privacy(&rt, Some(&hle), Some(&store)).unwrap();
        assert_eq!(hle.session_visibility(), live_hle::Visibility::Friends);
    }
}
