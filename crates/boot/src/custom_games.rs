//! Custom games over Live (docs/live-custom-games.md): a "Custom Game
//! Browser" row on the Xbox Live menu that opens the retail System Link
//! browser in its dormant Live mode, a Content Download row that explains how
//! to install the Multiplayer Map Pack, and the party privacy the title
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
//! at `[esp+8]`, `[[esp+4]+4]` the player index). We grow the count to five
//! and show the rows in this order: rows 0 to 2 as retail, row 3 ours, row 4
//! Content Download. Row 3 opens the Live browser with
//! `0x149f1e(1 << player, 0x2531c9)`, the request the System Link path
//! builds at `0x252951`; row 4 opens the game's one-button dialog with our
//! text ([`MAPPACK_ERROR`]) instead of retail's leave-for-the-dashboard
//! confirmation (`0x19b5af`, error `0xa3`).
//!
//! **Back.** The browser's back press (`0x2536a6`) always opens the System
//! Link screen (`0x252433`), its LAN parent. While a Live-mode browser
//! handles an event, that call goes to the Live menu's loader (`0x2310b7`)
//! instead, and our row press leaves the Live menu's cursor on our row
//! ([`browser_is_live`], [`back_loader`], [`remember_row`]).
//!
//! **The fifth item widget.** The handler embeds four item widgets
//! (`+0x88`, 0x80 bytes each, built by `0x24abae`), and a list scrolls when
//! it has more rows than child item widgets (`0x24c515` counts children with
//! `0x24c177`). So, as for the Controls list, the shared "add items" hook
//! (`0x24bb60`) links a fifth item widget of ours after the four, and the
//! list lays it out one item height below the fourth (`0x24bc12`). The tag's
//! list already shows up to seven items. [`extend_tag`] gives the screen's
//! tag a fifth pane for our row and makes room for the fifth item.
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

use std::sync::Arc;

use crate::layout_glue::{Gm, ID_BASE};
use crate::setting_defs::{
    LIVE_VISIBILITY, VISIBILITY_FRIENDS, VISIBILITY_INVITE, VISIBILITY_PUBLIC,
};

/// The Xbox Live menu's list name and retail row count.
pub const LIST_NAME: u32 = 0x0045_8c80;
pub const VANILLA_ROWS: u32 = 4;
/// Our row, directly above Content Download, and where Content Download
/// moves to (it is retail row 3).
pub const ROW: i16 = 3;
pub const DOWNLOAD_ROW: i16 = 4;
const RETAIL_DOWNLOAD_ROW: i16 = 3;
/// The text `0x230f36` gives retail row 3, Content Download, so the moved
/// row keeps the player's language.
pub const DOWNLOAD_TEXT_ID: u32 = 0x1000_0445;
/// Its row text (stdcall, `ret 8`) and row chosen (`ret 8`) functions.
pub const ROW_TEXT: u32 = 0x0023_0f36;
pub const ROW_CHOSEN: u32 = 0x0023_0f92;
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
/// it when 0 to 3; the retail press stores rows 0 to 2).
const LAST_ROW: u32 = 0x0051_0a18;
/// Our row's label. It fits: the list skin's text rectangle is 190 units
/// wide, and the label renders 136 wide in conduit-12 (180 in conduit-16).
pub const TEXT_ID: u32 = ID_BASE + 0x32;
pub const TEXT: &str = "Custom Game Browser";
/// The help text on our row's pane.
pub const HELP_ID: u32 = ID_BASE + 0x33;
pub const HELP: &str = "Find custom games hosted on your Live server and join one.";

/// The screen, its list handler's vtable and its retail item widgets.
pub const SCREEN_ID: i16 = 0xba;
pub const HANDLER_VTABLE: u32 = 0x0045_8b48;
pub const VANILLA_ITEMS: u32 = 4;

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
/// Our dialog text (docs/map-pack.md section 4). The message box holds about
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

/// Whether the row is on (`H2_NO_CUSTOM_GAMES_ROW=1` takes it out).
pub fn row_enabled() -> bool {
    std::env::var_os("H2_NO_CUSTOM_GAMES_ROW").is_none()
}

/// The Live menu's count when it is built with `count` rows: one more for
/// our row, only for that list at its retail size.
pub fn list_count(name: u32, count: u32) -> u32 {
    if name == LIST_NAME && count == VANILLA_ROWS {
        count + 1
    } else {
        count
    }
}

/// What a row of the grown Xbox Live menu is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// Quickmatch, OptiMatch or Custom Game: the retail functions handle it.
    Retail,
    /// Ours: opens the Live browser.
    Browser,
    /// Content Download, moved down one: opens the Map Pack dialog.
    Download,
}

pub fn row_kind(row: i16) -> Row {
    match row {
        ROW => Row::Browser,
        DOWNLOAD_ROW => Row::Download,
        _ => Row::Retail,
    }
}

/// The label of a row the retail row text cannot draw: ours, and Content
/// Download at its new place.
pub fn row_text_id(row: i16) -> Option<u32> {
    match row_kind(row) {
        Row::Browser => Some(TEXT_ID),
        Row::Download => Some(DOWNLOAD_TEXT_ID),
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

/// The pane for `row` when the screen's tag has `panes` panes. The retail
/// tag has one pane per row (the help text and picture of that row); once
/// [`extend_tag`] has run, pane 4 is ours.
pub fn pane_of(row: i16, panes: u32) -> Pane {
    match (row_kind(row), panes == EXTENDED_PANES) {
        (Row::Retail, _) => Pane::Retail,
        (Row::Browser, true) => Pane::Show(EXTENDED_PANES as i16 - 1),
        // No pane of ours: keep the one shown rather than show Content
        // Download's help for our row.
        (Row::Browser, false) => Pane::Keep,
        (Row::Download, _) => Pane::Show(RETAIL_DOWNLOAD_ROW),
    }
}

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

/// Leave the Live menu's cursor on our row for when the browser goes back
/// to it (the retail press stores rows 0 to 2 the same way).
pub fn remember_row<M: Gm>(m: &mut M) -> bool {
    m.set_u32(LAST_ROW, ROW as u32)
}

/// The strings the rows and the dialog show, as `(id, text)`.
pub fn strings() -> Vec<(u32, &'static str)> {
    vec![
        (TEXT_ID, TEXT),
        (HELP_ID, HELP),
        (MAPPACK_TITLE_ID, MAPPACK_TITLE),
        (MAPPACK_MESSAGE_ID, MAPPACK_MESSAGE),
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

// ---- the screen's tag: a fifth pane and room for the fifth item ----
//
// The tag (`wgit`, docs/ui.md) has four panes, one per row, each with the
// hint texts under the list, the "Y" callout, the row's help text and
// picture; pane 0 also holds the list (anchor, up to seven visible items,
// the default list skin). The list lays items out from the anchor, each one
// item height below the last (`0x24bc12`): the height of the item's bounding
// box (`0x2bafa4`), which for the default skin is its text (top 28) down to
// its background bitmap (bottom -15), 43 units; the end cap bitmap is
// flagged out of the box. A fifth item would cover the hint text, which
// starts 21 units below the fourth item. So [`extend_tag`] moves the list
// up by [`LIFT`] and everything under it in its column down by [`DROP`]:
// together one item height, the gap under the list stays 21 units, and the
// first item stays 36 units under the header. It also adds pane 4 for our
// row: a copy of the Custom Game pane (2) with our help text. The copies
// live in our memory; the retail arrays are left as they are.

const VANILLA_PANES: u32 = 4;
pub const EXTENDED_PANES: u32 = 5;
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
/// The default skin's item height and bottom, relative to the item origin.
pub const ITEM_HEIGHT: i16 = 43;
const ITEM_BOTTOM: i16 = -15;
/// How far right of the list's anchor a block may start and still be "in
/// the list's column": half an item's width (the skin is 210 wide). The
/// hints and the callout start at or left of the anchor; the message of the
/// day panel to the right starts 253 units right of it.
const COLUMN: i16 = 105;
pub const LIFT: i16 = 21;
pub const DROP: i16 = ITEM_HEIGHT - LIFT;
/// Bytes [`extend_tag`] may use for its copies.
pub const TAG_SPACE: u32 = 0x1000;

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

/// Give the Xbox Live menu's tag `wgit` a fifth pane and room for a fifth
/// item (see above), writing the copies in [`TAG_SPACE`] bytes at `base`.
/// `Some(false)` and nothing written for any other screen, a tag without
/// exactly the retail four panes (so once done, or another layout), or no
/// list in pane 0; `None` if memory cannot be read or written.
pub fn extend_tag<M: Gm>(m: &mut M, wgit: u32, base: u32) -> Option<bool> {
    extend_tag_in(m, wgit, base, TAG_SPACE)
}

/// [`extend_tag`] within `space` bytes.
fn extend_tag_in<M: Gm>(m: &mut M, wgit: u32, base: u32, space: u32) -> Option<bool> {
    if m.u16(wgit.wrapping_add(4))? as i16 != SCREEN_ID || m.u32(wgit + 0x20)? != VANILLA_PANES {
        return Some(false);
    }
    let old = m.u32(wgit + 0x24)?;
    if m.u32(old + PANE_LISTS as u32)? as i32 <= 0 {
        return Some(false);
    }
    let list = m.u32(old + PANE_LISTS as u32 + 4)?;
    let anchor_x = m.u16(list + 8)? as i16;
    let anchor_y = m.u16(list + 0xa)? as i16;
    let list_bottom = anchor_y
        .saturating_sub((VANILLA_ITEMS as i16 - 1) * ITEM_HEIGHT)
        .saturating_add(ITEM_BOTTOM);
    let below = |left: i16, top: i16| top < list_bottom && left < anchor_x.saturating_add(COLUMN);
    // Our help text replaces the one text that differs between the Custom
    // Game pane and the next (the row's help); none if that is unclear.
    let template = text_ids(m, old + TEMPLATE_PANE * PANE_SIZE)?;
    let next_pane = text_ids(m, old + (TEMPLATE_PANE + 1) * PANE_SIZE)?;
    let differing: Vec<usize> = (0..template.len().min(next_pane.len()))
        .filter(|&i| template[i] != next_pane[i])
        .collect();
    let help = (template.len() == next_pane.len() && differing.len() == 1).then(|| differing[0]);

    let end = base.checked_add(space)?;
    let mut next = base + EXTENDED_PANES * PANE_SIZE;
    for p in 0..EXTENDED_PANES {
        let ours = p == EXTENDED_PANES - 1;
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
                put_i16(e, 0xa, get_i16(e, 0xa).saturating_add(LIFT));
                let rows = (VANILLA_ROWS + 1) as i16;
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
                // Rectangle: top, left, bottom, right.
                if below(get_i16(e, 0x1e), get_i16(e, 0x1c)) {
                    put_i16(e, 0x1c, get_i16(e, 0x1c).saturating_sub(DROP));
                    put_i16(e, 0x20, get_i16(e, 0x20).saturating_sub(DROP));
                }
                if ours && help == Some(i) {
                    e[0x24..0x28].copy_from_slice(&HELP_ID.to_le_bytes());
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
            |_, e| {
                // Top-left point: x, y.
                if below(get_i16(e, 0xc), get_i16(e, 0xe)) {
                    put_i16(e, 0xe, get_i16(e, 0xe).saturating_sub(DROP));
                }
            },
        )?;
        if !m.wr(base + p * PANE_SIZE, &pane) {
            return None;
        }
    }
    // Publish last: a reader sees the old array or the complete new one.
    (m.set_u32(wgit + 0x24, base) && m.set_u32(wgit + 0x20, EXTENDED_PANES)).then_some(true)
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
        eprintln!("custom games: no generated entry at {PRIVACY_CHOSEN:#x}; privacy not followed");
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
    fn our_row_sits_directly_above_content_download() {
        assert_eq!(list_count(LIST_NAME, VANILLA_ROWS), 5);
        for r in 0..3 {
            assert_eq!(row_kind(r), Row::Retail, "row {r}");
            assert_eq!(row_text_id(r), None, "row {r} keeps the retail text");
        }
        assert_eq!(row_kind(ROW), Row::Browser);
        assert_eq!(row_kind(DOWNLOAD_ROW), Row::Download);
        assert_eq!(DOWNLOAD_ROW, ROW + 1);
        assert_eq!(row_text_id(ROW), Some(TEXT_ID));
        // Content Download keeps the game's own (localized) label.
        assert_eq!(row_text_id(DOWNLOAD_ROW), Some(DOWNLOAD_TEXT_ID));
        assert_eq!(row_kind(5), Row::Retail);
        assert_eq!(TEXT, "Custom Game Browser");
    }

    #[test]
    fn panes_follow_the_rows() {
        // Extended tag: our row shows pane 4, Content Download its own pane 3.
        assert_eq!(pane_of(ROW, EXTENDED_PANES), Pane::Show(4));
        assert_eq!(pane_of(DOWNLOAD_ROW, EXTENDED_PANES), Pane::Show(3));
        for r in 0..3 {
            assert_eq!(pane_of(r, EXTENDED_PANES), Pane::Retail);
            assert_eq!(pane_of(r, VANILLA_PANES), Pane::Retail);
        }
        // A tag we did not extend has no pane 4: keep the shown one.
        assert_eq!(pane_of(ROW, VANILLA_PANES), Pane::Keep);
        assert_eq!(pane_of(DOWNLOAD_ROW, VANILLA_PANES), Pane::Show(3));
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
            assert!(!crate::video_screen::MODEL_IDS.contains(i), "{i:#x}");
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
    fn the_tag_gets_a_fifth_pane_and_room_for_the_fifth_row() {
        let mut m = Fake::default();
        let wgit = live_tag(&mut m, 7);
        let old_panes = m.u32(wgit + 0x24).unwrap();
        let retail_texts = |m: &Fake| -> Vec<Vec<u8>> {
            (0..VANILLA_PANES)
                .map(|p| bytes(m, 0x12_0000 + p * 0x1000, 3 * TEXT_SIZE))
                .collect()
        };
        let retail = retail_texts(&m);
        assert_eq!(extend_tag(&mut m, wgit, BASE), Some(true));
        assert_eq!(m.u32(wgit + 0x20), Some(EXTENDED_PANES));
        assert_eq!(m.u32(wgit + 0x24), Some(BASE));
        // The retail arrays are untouched; everything new is in our space.
        assert_eq!(retail_texts(&m), retail);
        assert_eq!(m.u32(old_panes + 0x1c), Some(3));
        assert!(m.0.keys().all(|a| *a < BASE + TAG_SPACE));

        let pane = |p: u32| BASE + p * PANE_SIZE;
        let text = |m: &Fake, p: u32, i: u32| m.u32(pane(p) + 0x20).unwrap() + i * TEXT_SIZE;
        let bitmap = |m: &Fake, p: u32, i: u32| m.u32(pane(p) + 0x28).unwrap() + i * BITMAP_SIZE;
        // The list moves up; it still shows its seven.
        let list = m.u32(pane(0) + 0x10).unwrap();
        assert_eq!(i16_at(&m, list + 6), 7);
        assert_eq!(i16_at(&m, list + 8), ANCHOR.0);
        let anchor_y = i16_at(&m, list + 0xa);
        assert_eq!(anchor_y, ANCHOR.1 + LIFT);
        // Item i spans origin + 28 down to origin - 15, one item height apart.
        let bottom = |y: i16, i: i16| y - i * ITEM_HEIGHT + ITEM_BOTTOM;
        let retail_gap = -60 - bottom(ANCHOR.1, 3);
        for p in 0..EXTENDED_PANES {
            // The hint under the list moves down, and the gap under the
            // fifth item is the retail gap under the fourth.
            let hint = text(&m, p, 0);
            assert_eq!(i16_at(&m, hint + 0x1c), -60 - DROP, "pane {p}");
            assert_eq!(i16_at(&m, hint + 0x20), -80 - DROP);
            assert_eq!(i16_at(&m, hint + 0x1c) - bottom(anchor_y, 4), retail_gap);
            // The panel to the right and the help text above stay.
            assert_eq!(i16_at(&m, text(&m, p, 1) + 0x1c), -50);
            assert_eq!(i16_at(&m, text(&m, p, 2) + 0x1c), 150);
            // The callout moves with the hint; the decoration stays.
            assert_eq!(i16_at(&m, bitmap(&m, p, 0) + 0xe), -150 - DROP);
            assert_eq!(i16_at(&m, bitmap(&m, p, 1) + 0xe), 150);
            // The other bytes of each element are copied.
            assert_eq!(m.u8(text(&m, p, 0) + 0x2a), Some(0x5a));
            assert_eq!(m.u8(bitmap(&m, p, 0) + 0x30), Some(0x3c));
        }
        // The first item stays under where the decoration starts.
        assert!(anchor_y + 28 <= 150);
        // Panes 0 to 3 keep their help; ours is pane 2's with our help text.
        for p in 0..4 {
            assert_eq!(m.u32(text(&m, p, 2) + 0x24), Some(0x0a00_0100 + p));
        }
        assert_eq!(m.u32(text(&m, 4, 2) + 0x24), Some(HELP_ID));
        assert_eq!(m.u32(text(&m, 4, 0) + 0x24), Some(0x0800_0001));
        assert_eq!(m.u32(pane(4) + 0xc), Some(0), "no list on our pane");
        // Once done, never again.
        assert_eq!(extend_tag(&mut m, wgit, BASE + TAG_SPACE), Some(false));
        assert_eq!(m.u32(wgit + 0x24), Some(BASE));
    }

    #[test]
    fn a_list_showing_fewer_than_five_is_raised_to_five() {
        let mut m = Fake::default();
        let wgit = live_tag(&mut m, 4);
        assert_eq!(extend_tag(&mut m, wgit, BASE), Some(true));
        let list = m.u32(BASE + 0x10).unwrap();
        assert_eq!(i16_at(&m, list + 6), 5);
    }

    #[test]
    fn other_tags_are_left_alone() {
        let mut m = Fake::default();
        let wgit = live_tag(&mut m, 7);
        m.set_u16(wgit + 4, 39);
        assert_eq!(extend_tag(&mut m, wgit, BASE), Some(false));
        m.set_u16(wgit + 4, SCREEN_ID as u16);
        m.set_u32(wgit + 0x20, 3);
        assert_eq!(extend_tag(&mut m, wgit, BASE), Some(false));
        m.set_u32(wgit + 0x20, VANILLA_PANES);
        let panes = m.u32(wgit + 0x24).unwrap();
        m.set_u32(panes + 0xc, 0);
        assert_eq!(extend_tag(&mut m, wgit, BASE), Some(false), "no list");
        assert_eq!(m.u32(wgit + 0x24), Some(panes));
        assert!(m.0.keys().all(|a| *a < BASE), "nothing written");
        // Too little space: nothing published.
        m.set_u32(panes + 0xc, 1);
        let mut tight = Fake(m.0.clone());
        assert_eq!(extend_tag_in(&mut tight, wgit, BASE, PANE_SIZE * 6), None);
        assert_eq!(tight.u32(wgit + 0x20), Some(VANILLA_PANES));
    }

    #[test]
    fn only_the_live_menu_grows_by_one() {
        assert_eq!(list_count(LIST_NAME, 4), 5);
        assert_eq!(list_count(LIST_NAME, 5), 5, "not twice");
        assert_eq!(list_count(LIST_NAME, 3), 3, "not another size");
        assert_eq!(list_count(crate::speaker_row::LIST_NAME, 4), 4);
        assert!(is_ours(TEXT_ID));
    }

    #[test]
    fn the_row_hooks_read_the_retail_arguments() {
        // Row text: the item at [esp+4], its row at +0x70.
        let mut m = Fake::default();
        let esp = 0x1000;
        m.set_u32(esp + 4, 0x2000);
        m.set_u16(0x2000 + 0x70, ROW as u16);
        assert_eq!(text_row(&m, esp), Some(ROW));
        // Row chosen: the row word through [esp+8], the player through
        // [[esp+4]+4], the mask the browser is opened for.
        m.set_u32(esp + 8, 0x3000);
        m.set_u16(0x3000, ROW as u16);
        m.set_u32(esp + 4, 0x4000);
        m.set_u32(0x4000, 0x5000);
        m.set_u32(0x5000 + 4, 2);
        assert_eq!(chosen_row(&m, esp), Some(ROW));
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
    fn our_press_leaves_the_cursor_on_our_row() {
        let mut m = Fake::default();
        m.set_u32(LAST_ROW, 1);
        assert!(remember_row(&mut m));
        // `0x231170` restores rows 0 to 3, so ours is restored.
        let row = m.u32(LAST_ROW).unwrap() as i32;
        assert_eq!(row, ROW as i32);
        assert!((0..4).contains(&row));
        assert_eq!(row_kind(row as i16), Row::Browser);
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
