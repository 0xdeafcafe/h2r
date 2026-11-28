//! Our game type rows at the bottom of the custom game's game type list,
//! under Territories: Party Games, Firefight and Siege ([`community::Row`];
//! docs/game/community-gametypes.md "Tweakers").
//!
//! **The list** (read from the XBE, 2026-11-21, static). Change Rules on
//! Game Setup and Settings, Game Variants open the "SELECT GAME" screen
//! (207 and 60, constructor `0x2b1ab3`). Its list at `+0x614` is a datum
//! array of nine with one datum per game type, the word at `+2` the game
//! type's row ([`GAME_TYPE_WORDS`]); the screen's tag has a pane per row
//! (the list's flag bit 0), each with the game type's description, title
//! and picture. A row opens that game type's variant list (screens 208 and
//! 15, `0x2c9ddb`), which lists the content category's variants
//! (`0x215900`) and plays or edits the one chosen.
//!
//! **Ours**, all through retail's own screens:
//!
//! - The rows: an `after` hook on the screen constructor adds a datum for
//!   each with its word ([`word_of_row`]) last, except in the create mode
//!   (a new variant of a game type). `before` hooks draw their labels and
//!   take their presses.
//! - The press runs retail's press with a private handle pointing to
//!   Slayer's row, so retail opens Slayer's variant list in the right mode on
//!   the right stack. While it runs, the variant list constructor's hook
//!   marks the new screen with the row ([`SCREEN_MARK`],
//!   [`community::Row::mark`]); every other variant list is marked 0.
//! - The fill: the variant list's call to `0x215900` is answered by the
//!   original, then the handles are kept or dropped by
//!   [`record_type`] on each record (loaded with `0x212bc0`, as the
//!   fill itself does). A row's list keeps its own variants and drops
//!   Create New; Slayer's drops every row's.
//! - The look: a row shows Slayer's pane with its title and description
//!   over its texts ([`pane`], from `layout_glue`'s pane hook); the variant
//!   list's update sets our title and description where retail set
//!   Slayer's; its own loader, which a screen stack reopens it with, is an
//!   entry of ours that opens it marked again.
//!
//! Content handles retain their physical save category. A list enumerates
//! the seven categories and shows each loaded record under its script's
//! type. A renamed save follows the same rule as a built-in.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use ::rules::types::Type;
use variants::ext;
use variants::record::GameType;

use crate::community_variants::{self as community, Row};
use crate::h1_options_row;
use crate::list_class;

/// The feature (`features.tweakers` switches it).
pub const FEATURE: &str = "tweakers";

/// The game type screens: Game Setup's Change Rules (207) and Settings,
/// Game Variants (60).
pub const CATEGORY_SCREENS: [i16; 2] = [0xcf, 0x3c];
/// The list handler's place in a screen, and its rows' datum array.
pub const SCREEN_LIST: u32 = 0x614;
pub const LIST_ROWS: u32 = 0x70;
/// A row datum's word.
pub const ROW_WORD: u32 = 2;
/// The game type screen's create flag (`+0xb39`): its list makes a new
/// variant of the game type chosen, so it gets none of our rows.
pub const CREATE_FLAG: u32 = 0xb39;
/// The retail rows' words, in list order (the row text table at
/// `0x2c5823`): Slayer, King of the Hill, Oddball, Juggernaut, CTF,
/// Assault, Territories. A pane is a row's index, not its word.
pub const GAME_TYPE_WORDS: [i16; 7] = [0, 1, 3, 4, 6, 7, 8];
pub const SLAYER_WORD: i16 = 0;
/// Our rows' words: past retail's, never seen by its press.
pub const PARTY_WORD: i16 = 9;
pub const FIREFIGHT_WORD: i16 = 10;
pub const SIEGE_WORD: i16 = 11;
/// Slayer's pane (index 0), the one our rows show with their texts.
pub const SLAYER_PANE: i16 = 0;
/// The pane's text blocks: the description (block 0) and the title
/// (block 1). A pane text widget keeps its block's index at `+0xa`
/// (`0x22f9bf` passes it to `0x253765`).
pub const PANE_DESCRIPTION_BLOCK: u16 = 0;
pub const PANE_TITLE_BLOCK: u16 = 1;
const WIDGET_BLOCK_INDEX: u32 = 0xa;
/// A widget's type (`+4`), 6 a text.
const WIDGET_TYPE: u32 = 4;
const TEXT_WIDGET: u32 = 6;
/// The screen's own text widgets, not the pane's: the header at `+0x80`
/// and the one at `+0x338` (`0x22fba9` keeps both).
const SCREEN_TEXTS: [u32; 2] = [0x80, 0x338];

/// The variant list screen: our mark at `+0x4fde`, a padding byte of its
/// `0x4fe0` (no retail code reads it); a row's list holds its
/// [`Row::mark`], every other 0.
pub const SCREEN_MARK: u32 = 0x4fde;
/// Its category (`+0x4fd8`, 1 Slayer) and mode bytes (`+0x4fdc` settings,
/// `+0x4fdd` pick).
pub const SCREEN_CATEGORY: u32 = 0x4fd8;
pub const SCREEN_SETTINGS: u32 = 0x4fdc;
pub const SCREEN_PICK: u32 = 0x4fdd;
/// Slayer's content category.
pub const SLAYER_CATEGORY: u32 = 1;
/// Juggernaut's content category: the save-content type of its variants
/// (`0x216b60`, [`variants::record::GameType::content_type`]).
pub const JUGGERNAUT_CATEGORY: u32 = 5;
/// Slayer's variant list loaders: Settings (screen 15, editing), the
/// lobby (screen 208) and picking (screen 15).
pub const SLAYER_SETTINGS_LOADER: u32 = 0x002c_a4cd;
pub const SLAYER_LOBBY_LOADER: u32 = 0x002c_a525;
pub const SLAYER_PICK_LOADER: u32 = 0x002c_a580;
/// The variant list's update texts retail sets by category: the
/// description (text 2) and the title (text 5).
pub const LIST_DESCRIPTION_TEXT: u32 = 2;
pub const LIST_TITLE_TEXT: u32 = 5;
/// The list handler's create flag (`+0x49a8`): its first row is Create
/// New (a datum whose handle at `+4` is -1).
pub const HANDLER_CREATE: u32 = 0x49a8;
const DATUM_HANDLE: u32 = 4;
/// A list item widget's row handle.
const ITEM_ROW: u32 = 0x70;

/// The variant list's fill calls `0x215900` from here.
pub const FILL_RETURN: u32 = 0x002c_a113;
/// Loads a variant record by content handle: EAX the `0x130`-byte record
/// out, EDX the handle; AL whether it loaded.
pub const LOAD_VARIANT: u32 = 0x0021_2bc0;
/// The built-in variants (`0x217080` files them): a count, then
/// `0x1e4`-byte records, the variant at `+4`; at most 101.
pub const BUILTIN_LIST: u32 = 0x0051_ea14;
pub const BUILTIN_RECORD: u32 = 0x1e4;
pub const BUILTIN_VARIANT: u32 = 4;
pub const MAX_BUILTINS: u32 = 101;
/// The fill's handle buffer (`0x1065` entries).
pub const MAX_HANDLES: u32 = 0x1065;

// ---- the pure rules ----

/// The word of one of our rows.
pub fn word_of_row(row: Row) -> i16 {
    match row {
        Row::PartyGames => PARTY_WORD,
        Row::Firefight => FIREFIGHT_WORD,
        Row::Siege => SIEGE_WORD,
    }
}

/// The row of ours with word `word`.
pub fn row_of_word(word: i16) -> Option<Row> {
    Row::ALL.into_iter().find(|r| word_of_row(*r) == word)
}

/// Whether a game type screen in this mode gets our rows.
pub fn wants_row(screen_id: i16, create: bool) -> bool {
    CATEGORY_SCREENS.contains(&screen_id) && !create
}

/// The rows' words with ours last, in [`Row::ALL`]'s order, each once.
pub fn with_row(words: &[i16]) -> Vec<i16> {
    let mut out = words.to_vec();
    for w in Row::ALL.map(word_of_row) {
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

/// The type of a stock content category, as opposed to a custom row's
/// mark. Both can be the destination of a script on any physical index.
fn category_type(category: u32) -> Option<Type> {
    let engine = GameType::from_content_type(category)?.engine();
    Type::of_engine(engine as u8)
}

/// The retail loader a row's list reopens with, by its mode bytes.
pub fn slayer_loader(settings: bool, pick: bool) -> u32 {
    if settings {
        SLAYER_SETTINGS_LOADER
    } else if pick {
        SLAYER_PICK_LOADER
    } else {
        SLAYER_LOBBY_LOADER
    }
}

// ---- guest memory ----

fn rows(m: &dyn GuestMemory, list: u32) -> Result<guest_layout::datum::DatumArray, MemFault> {
    Ok(guest_layout::datum::DatumArray::at(
        m.u32(list.wrapping_add(LIST_ROWS))?,
    ))
}

/// The word of the row at absolute index `index` of the list at `list`.
pub fn word_at(m: &dyn GuestMemory, list: u32, index: i32) -> Option<i16> {
    let elem = rows(m, list).ok()?.get_absolute(m, index).ok()??;
    m.u16(elem.wrapping_add(ROW_WORD)).ok().map(|v| v as i16)
}

/// The word of the row whose handle is `row`.
pub fn word_of(m: &dyn GuestMemory, list: u32, row: u32) -> Option<i16> {
    h1_options_row::row_action(m, list, row)
}

/// The row of ours a pane change on `screen` (screen id `id`) for the
/// row at `index` is for, if any: `layout_glue`'s pane hook asks.
pub fn our_pane(m: &dyn GuestMemory, screen: u32, id: i16, index: i16) -> Option<Row> {
    if !CATEGORY_SCREENS.contains(&id) {
        return None;
    }
    row_of_word(word_at(m, screen.wrapping_add(SCREEN_LIST), i32::from(index))?)
}

/// The screen's pane text widget made from text block `block`: a type-6
/// child that is not one of the screen's own texts.
pub fn pane_text(m: &dyn GuestMemory, screen: u32, block: u16) -> Option<u32> {
    let mut c = m.u32(screen.wrapping_add(0x14)).ok()?;
    for _ in 0..256 {
        if c == 0 {
            return None;
        }
        let own = SCREEN_TEXTS.iter().any(|o| screen.wrapping_add(*o) == c);
        if !own
            && m.u32(c.wrapping_add(WIDGET_TYPE)).ok()? == TEXT_WIDGET
            && m.u16(c.wrapping_add(WIDGET_BLOCK_INDEX)).ok()? == block
        {
            return Some(c);
        }
        c = m.u32(c.wrapping_add(0x18)).ok()?;
    }
    None
}

/// The row of ours whose list the variant list screen `screen` is.
pub fn screen_row(m: &dyn GuestMemory, screen: u32) -> Option<Row> {
    Row::from_mark(m.u8(screen.wrapping_add(SCREEN_MARK)).ok()?)
}

/// A variant record's name: up to 32 UTF-16 units at `+4`.
pub fn record_name(m: &dyn GuestMemory, record: u32) -> Option<String> {
    let mut b = [0u8; 2 * variants::record::NAME_UNITS];
    m.read(
        record.wrapping_add(variants::record::NAME_AT as u32),
        &mut b,
    )
    .ok()?;
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// A variant record's extension block.
pub fn record_block(m: &dyn GuestMemory, record: u32) -> Option<[u8; ext::LEN]> {
    let mut b = [0u8; ext::LEN];
    m.read(record.wrapping_add(ext::AT as u32), &mut b).ok()?;
    Some(b)
}

/// A variant record's engine (`+0x44`).
pub fn record_engine(m: &dyn GuestMemory, record: u32) -> Option<i32> {
    m.u32(record.wrapping_add(variants::record::ENGINE_AT as u32))
        .ok()
        .map(|e| e as i32)
}

/// The menu type of a loaded variant (script, or its physical fallback).
pub fn record_type(m: &dyn GuestMemory, record: u32) -> Option<Type> {
    community::filed_type(
        &record_name(m, record)?,
        &record_block(m, record)?,
        GameType::from_engine(record_engine(m, record)?),
    )
}

/// The handles the fill got: `*count` (a `u16`) of them at `handles`.
pub fn read_handles(m: &dyn GuestMemory, count: u32, handles: u32) -> Result<Vec<u32>, MemFault> {
    let n = u32::from(m.u16(count)?).min(MAX_HANDLES);
    (0..n).map(|i| m.u32(handles + 4 * i)).collect()
}

/// Writes back the handles kept and their count.
pub fn write_handles(
    m: &mut dyn GuestMemory,
    count: u32,
    handles: u32,
    keep: &[u32],
) -> Result<(), MemFault> {
    for (i, h) in keep.iter().enumerate() {
        m.set_u32(handles + 4 * i as u32, *h)?;
    }
    m.set_u16(count, keep.len() as u16)
}

/// Drops the Create New row from the variant list handler `handler`'s rows
/// (Settings' lists have one): a new variant made from one of our rows
/// would be a plain Slayer variant. Whether one was dropped.
pub fn drop_create_new(m: &mut dyn GuestMemory, handler: u32) -> Result<bool, MemFault> {
    if m.u8(handler.wrapping_add(HANDLER_CREATE))? == 0 {
        return Ok(false);
    }
    let array = rows(m, handler)?;
    let hw = array.header_field(m, guest_layout::datum::layout::HIGH_WATER)?;
    for i in 0..hw.max(0) {
        let Some(elem) = array.get_absolute(m, i)? else {
            continue;
        };
        if m.u32(elem.wrapping_add(DATUM_HANDLE))? == u32::MAX
            && let Some(h) = array.handle_of(m, i)?
        {
            array.delete(m, h)?;
            return Ok(true);
        }
    }
    Ok(false)
}

// ---- the hooks ----

// The shared UI list allocator is called inside the category constructor.
// It allocates nine datums for the stock picker, enough for seven stock
// rows and two custom ones. Grow the allocation before retail fills it.
// The list shows as many rows as it holds
// (`content/ui/retail-edits/game_engine_category_listing*.yaml`), so
// none is off its bottom.
thread_local! {
    static BUILDING_CATEGORY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn list_capacity(count: u32) -> u32 {
    if BUILDING_CATEGORY.get() && count == 9 {
        (GAME_TYPE_WORDS.len() + Row::ALL.len()) as u32
    } else {
        count
    }
}

/// Retail's press or a reopen loader is running for one of our rows (its
/// [`Row::mark`], 0 none): the variant list it builds is that row's.
static OPENING: AtomicU8 = AtomicU8::new(0);
/// The variant list update being let through to retail's (0: none).
static UPDATING: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Release the constructor mark even if its suspended guest call is dropped.
struct Opening(u8);

impl Opening {
    fn new(row: Row) -> Self {
        Self(OPENING.swap(row.mark(), Ordering::SeqCst))
    }
}

impl Drop for Opening {
    fn drop(&mut self) {
        OPENING.store(self.0, Ordering::SeqCst);
    }
}

fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

/// Forward through the actual Slayer datum. The press may dispose of the
/// source screen, so nothing writes back through its list after the call.
async fn press(cx: Cx, ours: Row, list: u32, chain: u32) -> Result<Ret, EntryError> {
    let slayer = cx.read(|m| crate::retail_rows::retail_handle(m, list, SLAYER_PANE));
    let Some(handle) = slayer else {
        return Ok(Ret::new(0, 8));
    };
    let press_at = cx.entry().esp.wrapping_sub(4);
    cx.write(|m| m.set_u32(press_at, handle))?;
    let _opening = Opening::new(ours);
    cx.call(
        GuestCall::new(points::VARIANT_CATEGORY_ROW_CHOSEN.base)
            .ecx(list)
            .reserve(4)
            .args([chain, press_at]),
    )
    .await?;
    Ok(Ret::new(0, 8))
}

/// A reopen of a row's list: Slayer's `loader`, marked with the row.
async fn reopen(cx: Cx, ours: Row, loader: u32) -> Result<Ret, EntryError> {
    let request = cx.arg(0)?;
    let _opening = Opening::new(ours);
    let r = cx.call(GuestCall::new(loader).args([request])).await;
    Ok(Ret::new(r?.eax, 4))
}

/// The variant list's fill of `0x215900`'s handles, filtered.
async fn fill(cx: Cx, scratch: u32) -> Result<Ret, EntryError> {
    let args = [cx.arg(0)?, cx.arg(1)?, cx.arg(2)?, cx.arg(3)?, cx.arg(4)?];
    let handler = cx.entry().ebx;
    let list = cx.read(|m| screen_row(m, handler.wrapping_sub(SCREEN_LIST)));
    // The count is capacity on entry and number written on return. A
    // category with no saves must not give the next enumeration zero room.
    let (count, handles) = (args[2], args[3]);
    let capacity = cx.read(|m| m.u16(count))?.min(MAX_HANDLES as u16);
    let r = cx
        .call(GuestCall::new(points::VARIANTS_ENUMERATE.base).args(args))
        .await?;
    let Some(physical) = category_type(args[1]) else {
        return Ok(Ret::new(r.eax, 20));
    };
    if capacity == 0 {
        return Ok(Ret::new(r.eax, 20));
    }
    let wanted = list.map(Row::game_type).unwrap_or(physical);
    let mut keep = Vec::new();
    // Saves keep the category they were written with. Ask the requested
    // category first so its existing variants retain their order.
    let categories = GameType::ALL.into_iter().filter_map(GameType::content_type);
    'categories: for category in std::iter::once(args[1]).chain(categories.filter(|c| *c != args[1])) {
        if category != args[1] {
            let mut more = args;
            more[1] = category;
            cx.write(|m| m.set_u16(count, capacity))?;
            cx.call(GuestCall::new(points::VARIANTS_ENUMERATE.base).args(more)).await?;
        }
        for h in cx.read(|m| read_handles(m, count, handles))? {
            let loaded = cx
                .call(GuestCall::new(LOAD_VARIANT).eax(scratch).edx(h))
                .await?
                .eax & 0xff != 0;
            let ty = if loaded { cx.read(|m| record_type(m, scratch)) } else { None };
            if ty.or_else(|| category_type(category)) == Some(wanted) {
                keep.push(h);
                if keep.len() >= usize::from(capacity) {
                    break 'categories;
                }
            }
        }
    }
    cx.write(|m| write_handles(m, count, handles, &keep))?;
    if list.is_some() {
        cx.write(|m| drop_create_new(m, handler))?;
    }
    Ok(Ret::new(r.eax, 20))
}

/// Sets the text widget `text` of `screen` to `id`.
async fn set_text(cx: &Cx, screen: u32, text: u32, id: u32) -> Result<(), EntryError> {
    if text != 0 {
        cx.call(
            GuestCall::new(points::TEXT_SET.base)
                .edi(text)
                .esi(screen)
                .args([id]),
        )
        .await?;
    }
    Ok(())
}

/// A filtered list may contain no selectable item. The channel drops all
/// input when its focus is null, including B. Give the active empty list's
/// screen its generic Back handler until retail binds a row.
fn focus_empty_list(m: &mut dyn GuestMemory, screen: u32) -> Result<(), MemFault> {
    use crate::ui_kit::lifecycle::{SCREEN_SLOT, SCREEN_WINDOW};
    use crate::ui_watch::{ACTIVE, PENDING, channel};
    let Some(ch) = channel(m.u32(screen + SCREEN_WINDOW)?, m.u32(screen + SCREEN_SLOT)?) else {
        return Ok(());
    };
    if m.u32(ch + ACTIVE)? == screen && m.u32(ch + PENDING)? == 0 && m.u32(ch + 0x34)? == 0 {
        let array = rows(m, screen + SCREEN_LIST)?;
        if array.addr != 0 && array.header_field(m, guest_layout::datum::layout::ACTIVE_COUNT)? == 0 {
            m.set_u32(ch + 0x34, screen)?;
        }
    }
    Ok(())
}

/// A row's list's update: retail's, then the row's title and description
/// where retail set Slayer's.
async fn update(cx: Cx, ours: Row, screen: u32) -> Result<Ret, EntryError> {
    UPDATING.store(screen, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(points::VARIANT_LIST_UPDATE.base).ecx(screen))
        .await;
    UPDATING.store(0, Ordering::SeqCst);
    let eax = r?.eax;
    cx.write(|m| focus_empty_list(m, screen))?;
    for (index, id) in [
        (LIST_DESCRIPTION_TEXT, ours.description_id()),
        (LIST_TITLE_TEXT, ours.title_id()),
    ] {
        let text = cx
            .call(
                GuestCall::new(points::ITEM_TEXT.base)
                    .ecx(screen)
                    .args([TEXT_WIDGET, index, 0]),
            )
            .await?
            .eax;
        set_text(&cx, screen, text, id).await?;
    }
    Ok(Ret::new(eax, 0))
}

/// One of our rows' pane, for `layout_glue`'s pane hook ([`our_pane`]):
/// Slayer's pane through `original` (the pane change, thiscall on the
/// screen, a pointer to the row, `ret 4`), the row handed over in our
/// `word`, then the row's description and title over its texts.
pub async fn pane(
    cx: Cx,
    ours: Row,
    original: u32,
    screen: u32,
    base: i8,
    word: u32,
) -> Result<Ret, EntryError> {
    let row = SLAYER_PANE.wrapping_sub(i16::from(base));
    cx.write(|m| m.set_u16(word, row as u16))?;
    cx.call(GuestCall::new(original).ecx(screen).args([word]))
        .await?;
    for (block, id) in [
        (PANE_DESCRIPTION_BLOCK, ours.description_id()),
        (PANE_TITLE_BLOCK, ours.title_id()),
    ] {
        let text = cx.read(|m| pane_text(m, screen, block)).unwrap_or(0);
        set_text(&cx, screen, text, id).await?;
    }
    Ok(Ret::new(0, 4))
}

fn row_text(c: &mut Call, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let (item, word) = {
        let mem = c.ctx.runtime().memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        let word = item
            .and_then(|i| mem.u32(i + ITEM_ROW).ok())
            .and_then(|row| word_of(&*mem, c.cpu.ecx, row));
        (item, word)
    };
    let (Some(item), Some(ours)) = (item, word.and_then(row_of_word)) else {
        return Ok(Flow::Continue);
    };
    once(seen, "tweakers: row text");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, ours.label_id())]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call) -> Result<Flow, GuestFault> {
    let (list, chain, word) = {
        let mem = c.ctx.runtime().memory();
        let chain = mem.u32(c.cpu.esp + 4).unwrap_or(0);
        let row = mem.u32(c.cpu.esp + 8).unwrap_or(0);
        let word = mem.u32(row).ok().and_then(|h| word_of(&*mem, c.cpu.ecx, h));
        (c.cpu.ecx, chain, word)
    };
    let Some(ours) = word.and_then(row_of_word) else {
        return Ok(Flow::Continue);
    };
    eprintln!("tweakers: opening the {} variants", ours.label());
    c.answer_async(move |cx| -> EntryFuture { Box::pin(press(cx, ours, list, chain)) })
}

/// The reopen entries' labels: for each row in [`Row::ALL`]'s order, its
/// settings, lobby and pick loaders.
const REOPEN_LABELS: [[&str; 3]; Row::ALL.len()] = [
    [
        "tweakers: reopen a Party Games list (settings)",
        "tweakers: reopen a Party Games list (lobby)",
        "tweakers: reopen a Party Games list (pick)",
    ],
    [
        "tweakers: reopen a Firefight list (settings)",
        "tweakers: reopen a Firefight list (lobby)",
        "tweakers: reopen a Firefight list (pick)",
    ],
    [
        "tweakers: reopen a Siege list (settings)",
        "tweakers: reopen a Siege list (lobby)",
        "tweakers: reopen a Siege list (pick)",
    ],
];

/// Our rows on the game type list and their variant lists.
pub struct Tweakers;

impl hooks::Feature for Tweakers {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "Game type rows and variant lists filed by script type")
            .doc("docs/game/community-gametypes.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let scratch = b.guest_alloc(0x1000, Permissions::RW)?;
        // Each row's list's own loaders, one for each of Slayer's: for each
        // row in [`Row::ALL`]'s order, settings, lobby, pick.
        let mut reopens = Vec::new();
        for (ours, labels) in Row::ALL.into_iter().zip(REOPEN_LABELS) {
            for (label, loader) in labels.into_iter().zip([
                SLAYER_SETTINGS_LOADER,
                SLAYER_LOBBY_LOADER,
                SLAYER_PICK_LOADER,
            ]) {
                let entry = b.entry(label, move |cx| -> EntryFuture {
                    Box::pin(reopen(cx, ours, loader))
                })?;
                reopens.push((ours, loader, entry));
            }
        }

        b.raw().observe_before(
            &points::VARIANT_CATEGORY_SCREEN_CTOR,
            "size our game type rows",
            |o| {
                let id = o.arg_u32("id").map_or(-1, |id| id as i16);
                let create = o.arg_u32("create").is_none_or(|v| v as u8 != 0);
                BUILDING_CATEGORY.set(wants_row(id, create));
            },
        );
        let added = Arc::new(AtomicBool::new(false));
        b.raw().after(
            &points::VARIANT_CATEGORY_SCREEN_CTOR,
            "add our game type rows",
            move |c| {
                BUILDING_CATEGORY.set(false);
                let screen = c.cpu.eax;
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let id = mem.u16(screen + 0x70).map_or(-1, |v| v as i16);
                let create = mem.u8(screen + CREATE_FLAG) != Ok(0);
                if screen == 0 || !wants_row(id, create) {
                    return Ok(());
                }
                let list = screen + SCREEN_LIST;
                match h1_options_row::insert_row_ordered(&mut *mem, list, with_row) {
                    Ok(n) => once(
                        &added,
                        &format!("tweakers: rows added to screen {id} ({n} rows)"),
                    ),
                    Err(why) => eprintln!("tweakers: rows not added to screen {id}: {why}"),
                }
                Ok(())
            },
        );
        let seen = Arc::new(AtomicBool::new(false));
        b.raw().before(
            &points::VARIANT_CATEGORY_ROW_TEXT,
            "our rows' text",
            move |c| row_text(c, &seen),
        );
        b.raw().before(
            &points::VARIANT_CATEGORY_ROW_CHOSEN,
            "our rows' press",
            row_chosen,
        );
        b.raw().after(
            &points::VARIANT_LIST_SCREEN_CTOR,
            "mark a row's list",
            |c| {
                let screen = c.cpu.eax;
                if screen != 0 {
                    let mark = OPENING.load(Ordering::SeqCst);
                    let rt = Arc::clone(c.ctx.runtime());
                    let _ = rt.memory().set_u8(screen + SCREEN_MARK, mark);
                }
                Ok(())
            },
        );
        b.raw().before(
            &points::VARIANTS_ENUMERATE,
            "keep each row's variants apart",
            move |c| {
                let ret = c.ctx.runtime().memory().u32(c.cpu.esp).unwrap_or(0);
                if ret != FILL_RETURN {
                    return Ok(Flow::Continue);
                }
                c.answer_async(move |cx| -> EntryFuture { Box::pin(fill(cx, scratch)) })
            },
        );
        b.raw().before(
            &points::VARIANT_LIST_UPDATE,
            "a row's list's title and description",
            |c| {
                let screen = c.cpu.ecx;
                if screen != 0
                    && UPDATING
                        .compare_exchange(screen, 0, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    return Ok(Flow::Continue);
                }
                let Some(ours) = screen_row(&*c.ctx.runtime().memory(), screen) else {
                    return Ok(Flow::Continue);
                };
                c.answer_async(move |cx| -> EntryFuture { Box::pin(update(cx, ours, screen)) })
            },
        );
        b.raw().before(
            &points::VARIANT_LIST_LOADER,
            "a row's list reopens as one",
            move |c| {
                let screen = c.cpu.ecx;
                let mode = {
                    let mem = c.ctx.runtime().memory();
                    screen_row(&*mem, screen).map(|ours| {
                        (
                            ours,
                            mem.u8(screen + SCREEN_SETTINGS).unwrap_or(0) != 0,
                            mem.u8(screen + SCREEN_PICK).unwrap_or(0) != 0,
                        )
                    })
                };
                let Some((ours, settings, pick)) = mode else {
                    return Ok(Flow::Continue);
                };
                let loader = slayer_loader(settings, pick);
                let Some(&(_, _, entry)) =
                    reopens.iter().find(|(r, l, _)| *r == ours && *l == loader)
                else {
                    return Ok(Flow::Continue);
                };
                c.cpu.eax = entry;
                c.ret(0).map(Flow::Return)
            },
        );
        b.snapshot(hooks::SnapshotPolicy::forbid_while(live));
        Ok(())
    }
}

/// Why `tweakers` can't be snapshotted now: a row's press or a reopen is
/// inside retail's, or a row's list's update is.
pub fn live() -> Option<String> {
    if OPENING.load(Ordering::SeqCst) != 0 {
        return Some("one of our rows' variant lists is opening".into());
    }
    (UPDATING.load(Ordering::SeqCst) != 0).then(|| "one of our rows' lists is updating".into())
}

#[cfg(test)]
mod tests;
