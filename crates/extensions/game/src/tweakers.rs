//! Tweakers: a game type row of ours at the bottom of the custom game's
//! game type list, under Territories, for variants with tweaked rules
//! (docs/game/community-gametypes.md "Tweakers"). Superbounce is the first.
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
//! - The row: an `after` hook on the screen constructor adds a datum with
//!   [`TWEAKERS_WORD`] last, except in the create mode (a new variant of a
//!   game type). `before` hooks draw its label and take its press.
//! - The press runs retail's press with the datum's word swapped for
//!   Slayer's, so retail opens Slayer's variant list in the right mode on
//!   the right stack. While it runs, the variant list constructor's hook
//!   marks the new screen ([`SCREEN_MARK`]); every other variant list is
//!   marked 0.
//! - The fill: the variant list's call to `0x215900` is answered by the
//!   original, then the handles are kept or dropped by
//!   [`community_variants::is_tweaker`] on each record (loaded with
//!   `0x212bc0`, as the fill itself does). A Tweakers list keeps the
//!   tweakers and drops Create New; Slayer's drops them.
//! - The look: the row shows Slayer's pane with our title and description
//!   over its texts ([`pane`], from `layout_glue`'s pane hook); the variant
//!   list's update sets our title and description where retail set
//!   Slayer's; its own loader, which a screen stack reopens it with, is an
//!   entry of ours that opens it marked again.
//!
//! Every tweaker is a Slayer variant underneath (the test
//! `every_tweaker_is_a_slayer_variant`), so Slayer's list, its new-variant
//! defaults and its copies are the right ones for Tweakers. The built-ins'
//! records get their extension block ([`patch_builtins`]) so a game on one,
//! and a copy of one, names the tweak's rules script.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use variants::ext::{self, Block, Ext};

use crate::community_variants::{self as community, Category, VARIANTS};
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
/// variant of the game type chosen, so it gets no Tweakers row.
pub const CREATE_FLAG: u32 = 0xb39;
/// The retail rows' words, in list order (the row text table at
/// `0x2c5823`): Slayer, King of the Hill, Oddball, Juggernaut, CTF,
/// Assault, Territories. A pane is a row's index, not its word.
pub const GAME_TYPE_WORDS: [i16; 7] = [0, 1, 3, 4, 6, 7, 8];
pub const SLAYER_WORD: i16 = 0;
/// Our row's word: past retail's, never seen by its press.
pub const TWEAKERS_WORD: i16 = 9;
/// Slayer's pane (index 0), the one our row shows with our texts.
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
/// `0x4fe0` (no retail code reads it); 1 on a Tweakers list.
pub const SCREEN_MARK: u32 = 0x4fde;
/// Its category (`+0x4fd8`, 1 Slayer) and mode bytes (`+0x4fdc` settings,
/// `+0x4fdd` pick).
pub const SCREEN_CATEGORY: u32 = 0x4fd8;
pub const SCREEN_SETTINGS: u32 = 0x4fdc;
pub const SCREEN_PICK: u32 = 0x4fdd;
/// Slayer's content category.
pub const SLAYER_CATEGORY: u32 = 1;
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

/// Whether a game type screen in this mode gets our row.
pub fn wants_row(screen_id: i16, create: bool) -> bool {
    CATEGORY_SCREENS.contains(&screen_id) && !create
}

/// The rows' words with ours last, if it is not there yet.
pub fn with_row(words: &[i16]) -> Vec<i16> {
    let mut out = words.to_vec();
    if !out.contains(&TWEAKERS_WORD) {
        out.push(TWEAKERS_WORD);
    }
    out
}

/// Whether a variant list keeps a handle: a Tweakers list the tweakers
/// only, any other list everything else. A record that did not load is
/// retail's business, so it stays on retail's lists.
pub fn keeps(tweakers_list: bool, loaded: bool, tweaker: bool) -> bool {
    let tweaker = loaded && tweaker;
    tweakers_list == tweaker
}

/// Whether the fill of a list of `category` is ours to filter: Slayer's
/// (every tweaker is a Slayer variant).
pub fn filtered(category: u32) -> bool {
    category == SLAYER_CATEGORY
}

/// The retail loader a Tweakers list reopens with, by its mode bytes.
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

fn rows(m: &dyn GuestMemory, list: u32) -> Result<engine::datum::DatumArray, MemFault> {
    Ok(engine::datum::DatumArray::at(
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

/// Whether a pane change on `screen` (screen id `id`) for the row at
/// `index` is for our row: `layout_glue`'s pane hook asks.
pub fn is_our_pane(m: &dyn GuestMemory, screen: u32, id: i16, index: i16) -> bool {
    CATEGORY_SCREENS.contains(&id)
        && word_at(m, screen.wrapping_add(SCREEN_LIST), i32::from(index)) == Some(TWEAKERS_WORD)
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

/// Whether the variant list screen `screen` is a Tweakers list.
pub fn is_tweakers_screen(m: &dyn GuestMemory, screen: u32) -> bool {
    m.u8(screen.wrapping_add(SCREEN_MARK)).ok() == Some(1)
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

/// Whether the variant record at `record` is listed under Tweakers.
pub fn record_is_tweaker(m: &dyn GuestMemory, record: u32) -> bool {
    match (record_name(m, record), record_block(m, record)) {
        (Some(name), Some(block)) => community::is_tweaker(&name, &block),
        _ => false,
    }
}

/// Writes each Tweakers built-in's extension block into its record in the
/// built-in list (`[0x51ea14]`), where it is not there yet: a game on it,
/// and a copy made from it, then name its script. How many changed.
pub fn patch_builtins(m: &mut dyn GuestMemory) -> Result<usize, MemFault> {
    let list = m.u32(BUILTIN_LIST)?;
    if list == 0 {
        return Ok(0);
    }
    let n = m.u32(list)?.min(MAX_BUILTINS);
    let mut changed = 0;
    for i in 0..n {
        let record = list + 4 + i * BUILTIN_RECORD + BUILTIN_VARIANT;
        let Some(want) = record_name(m, record)
            .and_then(|name| community::tweaker_builtin(&name))
            .and_then(|v| v.tweak())
        else {
            continue;
        };
        let block = record_block(m, record).unwrap_or([0; ext::LEN]);
        if Ext::decode(&block) != Block::Ext(want) {
            m.write(record + ext::AT as u32, &want.encode())?;
            changed += 1;
        }
    }
    Ok(changed)
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
/// (Settings' lists have one): a new variant made from Tweakers would be a
/// plain Slayer variant. Whether one was dropped.
pub fn drop_create_new(m: &mut dyn GuestMemory, handler: u32) -> Result<bool, MemFault> {
    if m.u8(handler.wrapping_add(HANDLER_CREATE))? == 0 {
        return Ok(false);
    }
    let array = rows(m, handler)?;
    let hw = array.header_field(m, engine::datum::layout::HIGH_WATER)?;
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

/// Retail's press or a reopen loader is running for our row: the variant
/// list it builds is a Tweakers list.
static OPENING: AtomicBool = AtomicBool::new(false);
/// The variant list update being let through to retail's (0: none).
static UPDATING: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

/// Retail's press on our row, as on Slayer's: the word swapped for the
/// call, the screen it opens marked.
async fn press(cx: Cx, list: u32, chain: u32, row: u32) -> Result<Ret, EntryError> {
    let elem = cx.read(|m| {
        let handle = engine::DatumHandle::from_raw(m.u32(row).ok()?)?;
        rows(m, list).ok()?.try_get(m, handle).ok()?
    });
    let Some(elem) = elem else {
        return Ok(Ret::new(0, 8));
    };
    cx.write(|m| m.set_u16(elem + ROW_WORD, SLAYER_WORD as u16))?;
    OPENING.store(true, Ordering::SeqCst);
    let r = cx
        .call(
            GuestCall::new(points::VARIANT_CATEGORY_ROW_CHOSEN.base)
                .ecx(list)
                .args([chain, row]),
        )
        .await;
    OPENING.store(false, Ordering::SeqCst);
    cx.write(|m| m.set_u16(elem + ROW_WORD, TWEAKERS_WORD as u16))?;
    r?;
    Ok(Ret::new(0, 8))
}

/// A reopen of a Tweakers list: Slayer's `loader`, marked.
async fn reopen(cx: Cx, loader: u32) -> Result<Ret, EntryError> {
    let request = cx.arg(0)?;
    OPENING.store(true, Ordering::SeqCst);
    let r = cx.call(GuestCall::new(loader).args([request])).await;
    OPENING.store(false, Ordering::SeqCst);
    Ok(Ret::new(r?.eax, 4))
}

/// The variant list's fill of `0x215900`'s handles, filtered.
async fn fill(cx: Cx, scratch: u32) -> Result<Ret, EntryError> {
    let args = [cx.arg(0)?, cx.arg(1)?, cx.arg(2)?, cx.arg(3)?, cx.arg(4)?];
    let handler = cx.entry().ebx;
    let tweakers = cx.read(|m| is_tweakers_screen(m, handler.wrapping_sub(SCREEN_LIST)));
    let patched = cx.write(|m| patch_builtins(m)).unwrap_or(0);
    if patched > 0 {
        eprintln!("tweakers: {patched} built-in records given their extension block");
    }
    let r = cx
        .call(GuestCall::new(points::VARIANTS_ENUMERATE.base).args(args))
        .await?;
    if !filtered(args[1]) {
        return Ok(Ret::new(r.eax, 20));
    }
    let (count, handles) = (args[2], args[3]);
    let got = cx.read(|m| read_handles(m, count, handles))?;
    let mut keep = Vec::with_capacity(got.len());
    for h in &got {
        let loaded = cx
            .call(GuestCall::new(LOAD_VARIANT).eax(scratch).edx(*h))
            .await?
            .eax
            & 0xff
            != 0;
        let tweaker = loaded && cx.read(|m| record_is_tweaker(m, scratch));
        if keeps(tweakers, loaded, tweaker) {
            keep.push(*h);
        }
    }
    cx.write(|m| write_handles(m, count, handles, &keep))?;
    if tweakers {
        cx.write(|m| drop_create_new(m, handler))?;
    }
    eprintln!(
        "tweakers: {} list: {} of {} variants",
        if tweakers { "Tweakers" } else { "Slayer" },
        keep.len(),
        got.len()
    );
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

/// A Tweakers list's update: retail's, then our title and description
/// where retail set Slayer's.
async fn update(cx: Cx, screen: u32) -> Result<Ret, EntryError> {
    UPDATING.store(screen, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(points::VARIANT_LIST_UPDATE.base).ecx(screen))
        .await;
    UPDATING.store(0, Ordering::SeqCst);
    let eax = r?.eax;
    for (index, id) in [
        (LIST_DESCRIPTION_TEXT, community::TWEAKERS_DESCRIPTION_ID),
        (LIST_TITLE_TEXT, community::TWEAKERS_TITLE_ID),
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

/// Our row's pane, for `layout_glue`'s pane hook ([`is_our_pane`]): Slayer's
/// pane through `original` (the pane change, thiscall on the screen, a
/// pointer to the row, `ret 4`), the row handed over in our `word`, then
/// our description and title over its texts.
pub async fn pane(
    cx: Cx,
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
        (PANE_DESCRIPTION_BLOCK, community::TWEAKERS_DESCRIPTION_ID),
        (PANE_TITLE_BLOCK, community::TWEAKERS_TITLE_ID),
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
    let (Some(item), Some(TWEAKERS_WORD)) = (item, word) else {
        return Ok(Flow::Continue);
    };
    once(seen, "tweakers: row text");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, community::TWEAKERS_LABEL_ID)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call) -> Result<Flow, GuestFault> {
    if OPENING.load(Ordering::SeqCst) {
        return Ok(Flow::Continue);
    }
    let (list, chain, row, word) = {
        let mem = c.ctx.runtime().memory();
        let chain = mem.u32(c.cpu.esp + 4).unwrap_or(0);
        let row = mem.u32(c.cpu.esp + 8).unwrap_or(0);
        let word = mem.u32(row).ok().and_then(|h| word_of(&*mem, c.cpu.ecx, h));
        (c.cpu.ecx, chain, row, word)
    };
    if word != Some(TWEAKERS_WORD) {
        return Ok(Flow::Continue);
    }
    eprintln!("tweakers: opening the Tweakers variants");
    c.answer_async(move |cx| -> EntryFuture { Box::pin(press(cx, list, chain, row)) })
}

/// The Tweakers row on the game type list and its variant lists.
pub struct Tweakers;

impl hooks::Feature for Tweakers {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "The Tweakers game type row and its variant lists")
            .doc("docs/game/community-gametypes.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let scratch = b.guest_alloc(0x1000, Permissions::RW)?;
        // A Tweakers list's own loader, one for each of Slayer's.
        let reopen_settings = b.entry(
            "tweakers: reopen a Tweakers list (settings)",
            |cx| -> EntryFuture { Box::pin(reopen(cx, SLAYER_SETTINGS_LOADER)) },
        )?;
        let reopen_lobby = b.entry(
            "tweakers: reopen a Tweakers list (lobby)",
            |cx| -> EntryFuture { Box::pin(reopen(cx, SLAYER_LOBBY_LOADER)) },
        )?;
        let reopen_pick = b.entry(
            "tweakers: reopen a Tweakers list (pick)",
            |cx| -> EntryFuture { Box::pin(reopen(cx, SLAYER_PICK_LOADER)) },
        )?;

        let added = Arc::new(AtomicBool::new(false));
        b.raw().after(
            &points::VARIANT_CATEGORY_SCREEN_CTOR,
            "add the Tweakers row",
            move |c| {
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
                        &format!("tweakers: row added to screen {id} ({n} rows)"),
                    ),
                    Err(why) => eprintln!("tweakers: row not added to screen {id}: {why}"),
                }
                Ok(())
            },
        );
        let seen = Arc::new(AtomicBool::new(false));
        b.raw().before(
            &points::VARIANT_CATEGORY_ROW_TEXT,
            "the Tweakers row's text",
            move |c| row_text(c, &seen),
        );
        b.raw().before(
            &points::VARIANT_CATEGORY_ROW_CHOSEN,
            "the Tweakers row's press",
            row_chosen,
        );
        b.raw().after(
            &points::VARIANT_LIST_SCREEN_CTOR,
            "mark a Tweakers list",
            |c| {
                let screen = c.cpu.eax;
                if screen != 0 {
                    let mark = u8::from(OPENING.load(Ordering::SeqCst));
                    let rt = Arc::clone(c.ctx.runtime());
                    let _ = rt.memory().set_u8(screen + SCREEN_MARK, mark);
                }
                Ok(())
            },
        );
        b.raw().before(
            &points::VARIANTS_ENUMERATE,
            "keep a list's tweakers apart",
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
            "a Tweakers list's title and description",
            |c| {
                let screen = c.cpu.ecx;
                if screen != 0
                    && UPDATING
                        .compare_exchange(screen, 0, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    return Ok(Flow::Continue);
                }
                if !is_tweakers_screen(&*c.ctx.runtime().memory(), screen) {
                    return Ok(Flow::Continue);
                }
                c.answer_async(move |cx| -> EntryFuture { Box::pin(update(cx, screen)) })
            },
        );
        b.raw().before(
            &points::VARIANT_LIST_LOADER,
            "a Tweakers list reopens as one",
            move |c| {
                let screen = c.cpu.ecx;
                let mode = {
                    let mem = c.ctx.runtime().memory();
                    is_tweakers_screen(&*mem, screen).then(|| {
                        (
                            mem.u8(screen + SCREEN_SETTINGS).unwrap_or(0) != 0,
                            mem.u8(screen + SCREEN_PICK).unwrap_or(0) != 0,
                        )
                    })
                };
                let Some((settings, pick)) = mode else {
                    return Ok(Flow::Continue);
                };
                c.cpu.eax = match slayer_loader(settings, pick) {
                    SLAYER_SETTINGS_LOADER => reopen_settings,
                    SLAYER_PICK_LOADER => reopen_pick,
                    _ => reopen_lobby,
                };
                c.ret(0).map(Flow::Return)
            },
        );
        b.snapshot(hooks::SnapshotPolicy::forbid_while(live));
        Ok(())
    }
}

/// Why `tweakers` can't be snapshotted now: our row's press or a reopen is
/// inside retail's, or a Tweakers list's update is.
pub fn live() -> Option<String> {
    if OPENING.load(Ordering::SeqCst) {
        return Some("a Tweakers variant list is opening".into());
    }
    (UPDATING.load(Ordering::SeqCst) != 0).then(|| "a Tweakers list is updating".into())
}

/// Whether `v` is one of the Tweakers built-ins.
pub fn is_tweakers_variant(v: &community::Variant) -> bool {
    matches!(v.category, Category::Tweakers(_))
}

/// The Tweakers built-ins, in list order.
pub fn tweakers_variants() -> impl Iterator<Item = &'static community::Variant> {
    VARIANTS.iter().filter(|v| is_tweakers_variant(v))
}

#[cfg(test)]
mod tests;
