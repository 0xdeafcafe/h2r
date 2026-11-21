//! The modal dialog in game, as far as it can be made without a run
//! (docs/game/ui-builder.md "The modal dialog in game"): the tag pieces a
//! dialog's [`View`] needs, the tag copy that holds them, and the screen
//! request that opens it the way the game opens its own dialogs.
//!
//! **Pieces.** [`pieces`] turns a dialog view into what a tag must carry:
//! header layout 2 in the root flags (the title and the legend are the
//! header's), the panel's and cells' retail bitmap blocks
//! ([`list_class::PictureSpec`]s, found at run time through the screen
//! list), and a text block for the body, the status line and each cell's
//! digit. The alpha each one shows with is kept beside it: a block has no
//! alpha, so the dimmed cells are set on their widgets after the pane is
//! built (`+0x64`, as `list_class` tints do).
//!
//! **The tag copy.** [`write_tag`] writes a `wgit` in guest memory from the
//! Controls tag our classes are built with: its root with the header
//! layout bits set to layout 2, one pane holding only our bitmaps, our text
//! blocks and the one list, and no other block (the Controls pane's own
//! texts and pictures are not a dialog's).
//!
//! **Opening.** The game's dialogs (`0x19b527`, `0x19b5af`) fill their
//! request with window kind 1, not the 5 `0x149f1e` uses, and call the
//! loader themselves. The push (`0x147f6d`) picks the screen stack by that
//! kind: 5 is the main menu's stack, 1 the one dialogs go on over it.
//! [`request`] writes that request; our class loader passes `+4` and `+8`
//! to the Controls constructor and the request to the push, as the
//! retail loaders do.

use std::sync::Mutex;

use engine::entry::{Cx, EntryError, GuestCall};
use engine::memory::{GuestMemory, MemFault};
use engine::points;

use crate::ui_kit::dialog::DialogBase;

use crate::list_class::{
    self, BITMAP_BLOCK_SIZE, LIST_BLOCK_SIZE, ListLayout, PictureSpec, SCREEN_ROOT_SIZE,
    TEXT_BLOCK_SIZE, TextBlock,
};
use crate::ui_kit::view::{Art, Justify, Kind, View};

/// A `wgit` root's flags (`+0`): bits 4, 3 and 0 pick header layouts 1, 2
/// and 3 (`0x22fda6`, `tags::ui::Screen::header_layout`).
pub const LAYOUT_BITS: u32 = 0x19;
/// Header layout 2: the large dialog's title and legend rectangles.
pub const LAYOUT_2: u32 = 0x08;

/// A screen request's size, and the window kinds the push (`0x147f6d`)
/// switches on at `+4`.
pub const REQUEST_SIZE: u32 = 0x20;
/// What `0x149f1e` (`OPEN_SCREEN`) asks for: the main menu's screens.
pub const MAIN_WINDOW: u32 = 5;
/// What the game's dialogs ask for (`0x19b527`, `0x19b5af`).
pub const DIALOG_WINDOW: u32 = 1;
/// The request's `+8`: 4 from `0x149f1e` and from the dialogs alike.
pub const REQUEST_B: u32 = 4;

/// One bitmap of the dialog's pane: the retail block, where it goes, and
/// the alpha its widget shows with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtPiece {
    pub id: &'static str,
    pub index: usize,
    pub spec: PictureSpec,
    pub alpha: u8,
}

/// One text block of ours: the view widget's id, the block, its
/// justification, and the alpha its widget shows with.
#[derive(Clone, Debug, PartialEq)]
pub struct TextPiece {
    pub id: String,
    pub block: TextBlock,
    pub justify: Justify,
    pub alpha: u8,
}

/// What a dialog's tag has to carry.
#[derive(Clone, Debug, PartialEq)]
pub struct Pieces {
    /// The root flags' header layout bits.
    pub layout: u32,
    /// The panel and the cells, in draw order.
    pub art: Vec<ArtPiece>,
    /// The body, the cells' digits and the status, in the view's order.
    pub texts: Vec<TextPiece>,
}

impl Pieces {
    /// The tag copy's size: root, pane, list, bitmaps and texts.
    pub fn tag_size(&self) -> u32 {
        SCREEN_ROOT_SIZE
            + list_class::PANE_SIZE
            + LIST_BLOCK_SIZE
            + self.art.len() as u32 * BITMAP_BLOCK_SIZE
            + self.texts.len() as u32 * TEXT_BLOCK_SIZE
    }
}

/// The header draws the title, and the footer widget the legend; every
/// other text of the view is a block of ours.
const HEADER_TEXTS: [&str; 2] = ["title", "legend"];

/// What dialog view `v` needs in its tag (see the module docs). Art that
/// is not a retail screen's (a list skin's) has no block to copy and is
/// left out, as no dialog view has any.
pub fn pieces(v: &View) -> Pieces {
    let mut art = Vec::new();
    let mut texts = Vec::new();
    for w in &v.widgets {
        let [top, left, bottom, right] = w.rect;
        match &w.kind {
            Kind::Art {
                art:
                    Art::Screen {
                        screen_id,
                        pane,
                        index,
                        image,
                    },
                ..
            } => art.push(ArtPiece {
                id: if w.id.starts_with("cell.") {
                    "cell"
                } else {
                    "panel"
                },
                index: art.len(),
                spec: PictureSpec {
                    screen_id: *screen_id as u32,
                    pane: u32::from(*pane),
                    bitmap: u32::from(*index),
                    image: *image,
                    at: [left, top],
                    animation: 0,
                },
                alpha: w.alpha,
            }),
            Kind::Art { .. } => {}
            Kind::Text {
                font,
                justify,
                colour,
                ..
            } if !HEADER_TEXTS.contains(&w.id.as_str()) => texts.push(TextPiece {
                id: w.id.clone(),
                block: TextBlock {
                    rect: [top, left, bottom, right],
                    font: *font,
                    colour: colour.map(|c| f32::from(c) / 255.0),
                    animation: 0,
                },
                justify: *justify,
                alpha: w.alpha,
            }),
            Kind::Text { .. } => {}
        }
    }
    Pieces {
        layout: LAYOUT_2,
        art,
        texts,
    }
}

/// Text block flag bits for a justification (`tags::ui::justification`:
/// bit 0 left, bit 1 right, neither centred).
pub fn justify_flags(j: Justify) -> u32 {
    match j {
        Justify::Left => 1,
        Justify::Right => 2,
        Justify::Centre => 0,
    }
}

/// A text block's string id (`+0x24`, looked up through the screen's
/// strings, so a locale source of ours serves it).
const TEXT_STRING: usize = 0x24;

/// Text piece `t` as a block showing string id `string`.
pub fn text_block(t: &TextPiece, string: u32) -> [u8; TEXT_BLOCK_SIZE as usize] {
    let mut b = t.block.bytes();
    b[0..4].copy_from_slice(&justify_flags(t.justify).to_le_bytes());
    b[TEXT_STRING..TEXT_STRING + 4].copy_from_slice(&string.to_le_bytes());
    b
}

/// A pane's blocks (count, address), each 8 bytes: buttons, lists, a
/// reserved one, texts, bitmaps, model scenes, a reserved one, shader
/// bitmaps, tables (`tags::ui::Pane`).
const PANE_BLOCKS: [u32; 9] = [0x04, 0x0c, 0x14, 0x1c, 0x24, 0x2c, 0x34, 0x3c, 0x44];
const PANE_LISTS: u32 = 0x0c;
const PANE_TEXTS: u32 = 0x1c;
const PANE_BITMAPS: u32 = 0x24;
const LIST_SKIN: u32 = 0x04;
const LIST_VISIBLE: u32 = 0x06;
const LIST_ANCHOR: u32 = 0x08;

/// Writes at `at` the dialog's tag (see the module docs) from the retail
/// Controls tag at `retail`: its root with `pieces.layout` for the header
/// layout bits, one pane with the retail pane's flags and animation, the
/// first retail list placed by `list`, `art` (the blocks of
/// `pieces.art`, found through the screen list and placed), and the text
/// blocks of `pieces.texts` showing `strings`, one each. `None` when the
/// retail tag has no pane or no list, or `art` and `strings` don't match
/// the pieces.
pub fn write_tag(
    m: &mut dyn GuestMemory,
    retail: u32,
    at: u32,
    pieces: &Pieces,
    art: &[[u8; BITMAP_BLOCK_SIZE as usize]],
    strings: &[u32],
    list: &ListLayout,
) -> Result<Option<u32>, MemFault> {
    if art.len() != pieces.art.len() || strings.len() != pieces.texts.len() {
        return Ok(None);
    }
    let (panes, pane_from) = (
        m.u32(retail + list_class::SCREEN_PANES)?,
        m.u32(retail + list_class::SCREEN_PANES + 4)?,
    );
    if panes == 0 {
        return Ok(None);
    }
    let (lists, list_from) = (
        m.u32(pane_from + PANE_LISTS)?,
        m.u32(pane_from + PANE_LISTS + 4)?,
    );
    if lists == 0 {
        return Ok(None);
    }
    let copy = |m: &mut dyn GuestMemory, from: u32, to: u32, len: u32| -> Result<(), MemFault> {
        let mut b = vec![0u8; len as usize];
        m.read(from, &mut b)?;
        m.write(to, &b)
    };
    let pane = at + SCREEN_ROOT_SIZE;
    let list_at = pane + list_class::PANE_SIZE;
    let bitmaps = list_at + LIST_BLOCK_SIZE;
    let texts = bitmaps + art.len() as u32 * BITMAP_BLOCK_SIZE;

    copy(m, retail, at, SCREEN_ROOT_SIZE)?;
    let flags = m.u32(at)?;
    m.set_u32(at, (flags & !LAYOUT_BITS) | pieces.layout)?;
    m.set_u32(at + list_class::SCREEN_PANES, 1)?;
    m.set_u32(at + list_class::SCREEN_PANES + 4, pane)?;

    m.write(pane, &[0u8; list_class::PANE_SIZE as usize])?;
    copy(m, pane_from, pane, 4)?;
    debug_assert!(PANE_BLOCKS.iter().all(|o| o + 8 <= list_class::PANE_SIZE));

    copy(m, list_from, list_at, LIST_BLOCK_SIZE)?;
    m.set_u16(list_at + LIST_SKIN, list.skin as u16)?;
    if list.visible > 0 {
        m.set_u16(list_at + LIST_VISIBLE, list.visible as u16)?;
    }
    m.set_u16(list_at + LIST_ANCHOR, list.anchor[0] as u16)?;
    m.set_u16(list_at + LIST_ANCHOR + 2, list.anchor[1] as u16)?;
    m.set_u32(pane + PANE_LISTS, 1)?;
    m.set_u32(pane + PANE_LISTS + 4, list_at)?;

    for (i, b) in art.iter().enumerate() {
        m.write(bitmaps + i as u32 * BITMAP_BLOCK_SIZE, b)?;
    }
    m.set_u32(pane + PANE_BITMAPS, art.len() as u32)?;
    m.set_u32(pane + PANE_BITMAPS + 4, bitmaps)?;

    for (i, (t, &s)) in pieces.texts.iter().zip(strings).enumerate() {
        m.write(texts + i as u32 * TEXT_BLOCK_SIZE, &text_block(t, s))?;
    }
    m.set_u32(pane + PANE_TEXTS, pieces.texts.len() as u32)?;
    m.set_u32(pane + PANE_TEXTS + 4, texts)?;
    Ok(Some(at))
}

/// Bitmaps of a base dialog's pane a tag copy keeps at most.
pub const BASE_BITMAPS: u32 = 16;

/// The size of [`write_base_tag`]'s copy with `bitmaps` bitmaps and
/// `texts` text blocks.
pub const fn base_tag_size(bitmaps: u32, texts: u32) -> u32 {
    SCREEN_ROOT_SIZE
        + list_class::PANE_SIZE
        + LIST_BLOCK_SIZE
        + bitmaps * BITMAP_BLOCK_SIZE
        + texts * TEXT_BLOCK_SIZE
}

/// Writes at `at` a dialog of rows built from the retail dialog tag at
/// `base` (a [`crate::ui_kit::dialog::DialogBase`]'s): the root as it is
/// (its header layout and header string), one pane with the base pane's
/// flags, animation and bitmaps (its panel art, up to [`BASE_BITMAPS`],
/// less the block indices in `skip`: the base's own content),
/// its first list showing `list.visible` rows on `list.skin` at
/// `list.anchor`, and `texts` for text blocks in place of the base's (its
/// own are the retail dialog's message). Every other block is empty.
/// Returns the copy's address and size; `None` when the base has no pane
/// or no list.
pub fn write_base_tag(
    m: &mut dyn GuestMemory,
    base: u32,
    at: u32,
    list: &ListLayout,
    skip: &[u16],
    texts: &[[u8; TEXT_BLOCK_SIZE as usize]],
) -> Result<Option<(u32, u32)>, MemFault> {
    let (panes, pane_from) = (
        m.u32(base + list_class::SCREEN_PANES)?,
        m.u32(base + list_class::SCREEN_PANES + 4)?,
    );
    if panes == 0 {
        return Ok(None);
    }
    let (lists, list_from) = (
        m.u32(pane_from + PANE_LISTS)?,
        m.u32(pane_from + PANE_LISTS + 4)?,
    );
    if lists == 0 {
        return Ok(None);
    }
    let (all, bitmaps_from) = (
        m.u32(pane_from + PANE_BITMAPS)?,
        m.u32(pane_from + PANE_BITMAPS + 4)?,
    );
    let kept: Vec<u32> = (0..all)
        .filter(|&i| u16::try_from(i).map_or(true, |i| !skip.contains(&i)))
        .take(BASE_BITMAPS as usize)
        .collect();
    let n = kept.len() as u32;
    let copy = |m: &mut dyn GuestMemory, from: u32, to: u32, len: u32| -> Result<(), MemFault> {
        let mut b = vec![0u8; len as usize];
        m.read(from, &mut b)?;
        m.write(to, &b)
    };
    let pane = at + SCREEN_ROOT_SIZE;
    let list_at = pane + list_class::PANE_SIZE;
    let bitmaps = list_at + LIST_BLOCK_SIZE;
    let text_at = bitmaps + n * BITMAP_BLOCK_SIZE;

    copy(m, base, at, SCREEN_ROOT_SIZE)?;
    m.set_u32(at + list_class::SCREEN_PANES, 1)?;
    m.set_u32(at + list_class::SCREEN_PANES + 4, pane)?;
    m.write(pane, &[0u8; list_class::PANE_SIZE as usize])?;
    copy(m, pane_from, pane, 4)?;

    copy(m, list_from, list_at, LIST_BLOCK_SIZE)?;
    m.set_u16(list_at + LIST_SKIN, list.skin as u16)?;
    if list.visible > 0 {
        m.set_u16(list_at + LIST_VISIBLE, list.visible as u16)?;
    }
    m.set_u16(list_at + LIST_ANCHOR, list.anchor[0] as u16)?;
    m.set_u16(list_at + LIST_ANCHOR + 2, list.anchor[1] as u16)?;
    m.set_u32(pane + PANE_LISTS, 1)?;
    m.set_u32(pane + PANE_LISTS + 4, list_at)?;

    for (to, &from) in kept.iter().enumerate() {
        copy(
            m,
            bitmaps_from + from * BITMAP_BLOCK_SIZE,
            bitmaps + to as u32 * BITMAP_BLOCK_SIZE,
            BITMAP_BLOCK_SIZE,
        )?;
    }
    m.set_u32(pane + PANE_BITMAPS, n)?;
    m.set_u32(pane + PANE_BITMAPS + 4, bitmaps)?;

    for (i, t) in texts.iter().enumerate() {
        m.write(text_at + i as u32 * TEXT_BLOCK_SIZE, t)?;
    }
    m.set_u32(pane + PANE_TEXTS, texts.len() as u32)?;
    m.set_u32(pane + PANE_TEXTS + 4, text_at)?;
    Ok(Some((at, base_tag_size(n, texts.len() as u32))))
}

/// A screen request as `0x149f49` fills it: `+0` a zero word, `+2` the
/// player mask, `+4` the window kind, `+8` [`REQUEST_B`], `+0xc` zero,
/// `+0x10` three words of -1 (no source, EDX 0), `+0x1c` the loader.
pub fn request(window: u32, mask: u16, loader: u32) -> [u8; REQUEST_SIZE as usize] {
    let mut r = [0u8; REQUEST_SIZE as usize];
    r[2..4].copy_from_slice(&mask.to_le_bytes());
    r[4..8].copy_from_slice(&window.to_le_bytes());
    r[8..12].copy_from_slice(&REQUEST_B.to_le_bytes());
    r[0x10..0x1c].fill(0xff);
    r[0x1c..0x20].copy_from_slice(&loader.to_le_bytes());
    r
}

/// A class of ours shown as a dialog of rows: its screen vtable (the
/// class's table page), the page its tag copy and open request go in,
/// its loader, the retail dialog it is built from and its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registered {
    pub name: &'static str,
    pub vtable: u32,
    pub page: u32,
    pub loader: u32,
    pub base: DialogBase,
    pub list: ListLayout,
}

/// The list of dialog `base` showing `rows` rows: where the base has its
/// list, on its skin ([`crate::ui_kit::dialog::DialogGeometry::fallback_for`],
/// held to the disc).
pub fn base_layout(base: DialogBase, rows: usize) -> ListLayout {
    let g = crate::ui_kit::dialog::DialogGeometry::fallback_for(base);
    ListLayout {
        skin: g.list.skin,
        anchor: g.list.anchor,
        bitmaps_left_of: i16::MIN,
        visible: rows.max(1) as i16,
    }
}

/// In a dialog class's page: the base tag the copy was made from (0
/// until made), and the 0x20-byte request [`open_over`] hands the loader.
pub const TAG_SOURCE: u32 = 0xffc;
pub const REQUEST_AT: u32 = 0xfd0;
const _: () = assert!(base_tag_size(BASE_BITMAPS, 0) <= REQUEST_AT);
const _: () = assert!(REQUEST_AT + REQUEST_SIZE <= TAG_SOURCE);

static DIALOGS: Mutex<Vec<Registered>> = Mutex::new(Vec::new());

fn dialogs() -> std::sync::MutexGuard<'static, Vec<Registered>> {
    DIALOGS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Registers (or replaces, by vtable) a dialog class.
pub fn register(r: Registered) {
    let mut v = dialogs();
    v.retain(|d| d.vtable != r.vtable);
    v.push(r);
}

/// The dialog class whose screen vtable is `vtable`.
pub fn by_vtable(vtable: u32) -> Option<Registered> {
    dialogs().iter().find(|d| d.vtable == vtable).copied()
}

/// The dialog class whose loader is `loader`.
pub fn by_loader(loader: u32) -> Option<Registered> {
    dialogs().iter().find(|d| d.loader == loader).copied()
}

/// What the screen-tag function returns for a screen of a dialog class:
/// the copy, once [`prepare`] made it. The retail lookup (the Controls
/// tag, screen 39) is not asked: it is in `mainmenu.map` only, so a
/// screen of ours opened in a game found no tag at all.
pub fn served(m: &dyn GuestMemory, vtable: u32) -> Result<Option<u32>, MemFault> {
    let Some(d) = by_vtable(vtable) else {
        return Ok(None);
    };
    Ok((m.u32(d.page + TAG_SOURCE)? != 0).then_some(d.page))
}

/// The tag of the screen whose datum handle is `handle`, read through the
/// tag entries as [`list_class::picture_block`] does; `None` for no tag.
pub fn tag_of(m: &dyn GuestMemory, handle: u32) -> Result<Option<u32>, MemFault> {
    if handle == u32::MAX {
        return Ok(None);
    }
    let entries = m.u32(points::TAG_ENTRIES.base)?;
    let tag = m.u32(entries.wrapping_add((handle & 0xffff) * 0x10 + 8))?;
    Ok((tag != 0).then_some(tag))
}

/// The screen's localized string-list reference (group and salted datum).
const SCREEN_STRINGS: u32 = 0x18;

fn strings_of(m: &dyn GuestMemory, tag: u32) -> Result<Option<[u8; 8]>, MemFault> {
    let mut reference = [0; 8];
    m.read(tag + SCREEN_STRINGS, &mut reference)?;
    Ok((reference[4..] != u32::MAX.to_le_bytes()).then_some(reference))
}

async fn screen_tag(cx: &Cx, id: u32) -> Result<Option<u32>, EntryError> {
    let handle = cx
        .call(GuestCall::new(points::SCREEN_TAG_BY_ID.base).args([id]))
        .await?
        .eax;
    Ok(cx.read(|m| tag_of(m, handle))?)
}

/// Prepare the class's tag before construction. A retail error dialog
/// receives its text in buffers and may have no string list. A kit screen
/// uses TEXT_SET for its title, footer and rows: the lookup at 0x1a0180
/// exits on a null string-list datum before it consults our locale source.
pub async fn prepare(
    cx: &Cx,
    d: &Registered,
    host_screen: u32,
    name: &str,
) -> Result<(), EntryError> {
    let Some(base) = screen_tag(cx, d.base.screen_id() as u32).await? else {
        eprintln!("{name}: no dialog screen {} in this interface; not drawn as a dialog", d.base.screen_id());
        cx.write(|m| m.set_u32(d.page + TAG_SOURCE, 0))?;
        return Ok(());
    };
    let strings = match cx.read(|m| strings_of(m, base))? {
        Some(reference) => Some(reference),
        None => match screen_tag(cx, host_screen).await? {
            Some(host) => cx.read(|m| strings_of(m, host))?,
            None => None,
        },
    };
    cx.write(|m| -> Result<(), MemFault> {
        let Some(strings) = strings else {
            eprintln!("{name}: neither dialog nor host screen has a localized string list");
            return m.set_u32(d.page + TAG_SOURCE, 0);
        };
        if m.u32(d.page + TAG_SOURCE)? == base && strings_of(m, d.page)? == Some(strings) {
            return Ok(());
        }
        match write_base_tag(m, base, d.page, &d.list, d.base.content_bitmaps(), &[])? {
            Some((at, size)) => {
                m.write(at + SCREEN_STRINGS, &strings)?;
                m.set_u32(d.page + TAG_SOURCE, base)?;
                eprintln!(
                    "{name}: a dialog on screen {}'s tag (copy at {at:#x}, {size:#x} bytes)",
                    d.base.screen_id()
                );
            }
            None => {
                m.set_u32(d.page + TAG_SOURCE, 0)?;
                eprintln!(
                    "{name}: dialog screen {} has no list; not drawn as a dialog",
                    d.base.screen_id()
                );
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// Opens dialog class `d`'s screen for `mask` over whatever is up, as the
/// game's dialogs open (window kind [`DIALOG_WINDOW`]): the request in
/// `d`'s page, handed to its loader.
pub async fn open_over(cx: &Cx, d: &Registered, mask: u32) -> Result<(), EntryError> {
    open_over_screen(cx, d, mask).await.map(|_| ())
}

/// As [`open_over`], returning the screen pointer, or zero on allocation
/// failure.
pub async fn open_over_screen(cx: &Cx, d: &Registered, mask: u32) -> Result<u32, EntryError> {
    let at = d.page + REQUEST_AT;
    cx.write(|m| m.write(at, &request(DIALOG_WINDOW, mask as u16, d.loader)))?;
    Ok(cx.call(GuestCall::new(d.loader).args([at])).await?.eax)
}

/// A screen's message entry (`0x22e957`, ECX the screen, stdcall, the
/// kind), the close message's kind, and the screen's state byte (bit 1
/// while it closes). `0x14800c`, retail's close by window and slot, has
/// no case for window kind 1: the close a dialog's window would send
/// (`0x2351d4` sends this message to the window's top screen) goes to the
/// screen itself.
pub const SCREEN_MESSAGE: u32 = 0x0022_e957;
pub const MESSAGE_CLOSE: u32 = 2;
pub const SCREEN_STATE: u32 = 0x42;

/// Whether `screen` is on the dialogs' window.
pub fn on_dialog_window(m: &dyn GuestMemory, screen: u32) -> Result<bool, MemFault> {
    Ok(m.u32(screen + crate::ui_kit::lifecycle::SCREEN_WINDOW)? == DIALOG_WINDOW)
}

/// Closes `screen` as B does: a screen on the dialogs' window by the close
/// message (once: not while it is closing), any other through
/// [`crate::ui_kit::lifecycle::close`].
pub async fn close(cx: &Cx, screen: u32) -> Result<(), EntryError> {
    let (dialog, closing) = cx.read(|m| -> Result<_, MemFault> {
        Ok((
            on_dialog_window(m, screen)?,
            m.u8(screen + SCREEN_STATE)? & 2 != 0,
        ))
    })?;
    if !dialog {
        return crate::ui_kit::lifecycle::close(cx, screen).await;
    }
    if !closing {
        cx.call(
            GuestCall::new(SCREEN_MESSAGE)
                .ecx(screen)
                .args([MESSAGE_CLOSE]),
        )
        .await?;
    }
    Ok(())
}

/// Whether an event of `kind` and `button` on `screen` is B or Back on
/// the dialogs' window, which the retail back handler (`0x2300ea`, through
/// `0x14800c`) cannot close.
pub fn closes_itself(
    m: &dyn GuestMemory,
    screen: u32,
    kind: u32,
    button: u32,
) -> Result<bool, MemFault> {
    use crate::list_class::BUTTON_EVENT;
    use crate::ui_kit::input::{B_BUTTON, BACK_BUTTON};
    if kind != BUTTON_EVENT || (button != B_BUTTON && button != BACK_BUTTON) {
        return Ok(false);
    }
    on_dialog_window(m, screen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::dialog::{
        BODY_TOP, CELL_TOP, Cells, DialogGeometry, DialogSpec, LARGE_SCREEN_ID, PASSWORD_CELL,
        PASSWORD_SCREEN_ID, dialog_view,
    };
    use crate::ui_kit::input::{Glyph, Prompt};
    use crate::ui_kit::view::{FRAME_DIM, Opener, Synthetic};
    use engine::testing::FakeGuest;

    /// The sign-in code page with an error showing.
    fn view() -> View {
        let mut s = crate::ui_kit::spec::ScreenSpec::new("signin-code", "Sign in as Chief");
        s.prompts = vec![
            Prompt::new(&[Glyph::A], "Sign in"),
            Prompt::new(&[Glyph::B], "Cancel"),
        ];
        let d = DialogSpec {
            body: "Enter the 6-digit code.".into(),
            cells: Some(Cells {
                text: "123456".chars().map(String::from).collect(),
                cursor: Some(2),
                gap_after: Some(2),
            }),
            status: Some("That code didn't work.".into()),
            opener: Opener::MAIN_MENU,
            ..DialogSpec::new("", Opener::MAIN_MENU)
        };
        dialog_view(&s, &d, &DialogGeometry::fallback(), &Synthetic, "code")
    }

    #[test]
    fn the_code_page_needs_layout_2_ten_bitmaps_and_eight_texts() {
        let p = pieces(&view());
        assert_eq!(p.layout, LAYOUT_2);
        let panel: Vec<_> = p.art.iter().filter(|a| a.id == "panel").collect();
        let cells: Vec<_> = p.art.iter().filter(|a| a.id == "cell").collect();
        assert_eq!(panel.len(), 4);
        assert!(
            panel
                .iter()
                .all(|a| a.spec.screen_id == LARGE_SCREEN_ID as u32)
        );
        assert_eq!(
            panel.iter().map(|a| a.spec.bitmap).collect::<Vec<_>>(),
            [0, 1, 2, 3],
            "the tag's draw order"
        );
        assert_eq!(cells.len(), 6);
        for c in &cells {
            assert_eq!(c.spec.screen_id, PASSWORD_SCREEN_ID as u32);
            assert_eq!(c.spec.bitmap, u32::from(PASSWORD_CELL));
            assert_eq!(c.spec.at[1], CELL_TOP);
        }
        assert_eq!(
            cells.iter().filter(|c| c.alpha == 255).count(),
            1,
            "the cursor's cell is lit"
        );
        assert!(
            cells
                .iter()
                .filter(|c| c.alpha != 255)
                .all(|c| c.alpha == FRAME_DIM)
        );
        let ids: Vec<_> = p.texts.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "body",
                "cell.0.text",
                "cell.1.text",
                "cell.2.text",
                "cell.3.text",
                "cell.4.text",
                "cell.5.text",
                "status"
            ],
            "the title and the legend are the header's"
        );
        assert_eq!(p.texts[0].block.rect[0], BODY_TOP);
        assert_eq!(p.texts[0].justify, Justify::Left);
        assert_eq!(p.texts[1].justify, Justify::Centre);
        assert_eq!(p.tag_size(), 0x68 + 0x4c + 0x18 + 10 * 0x38 + 8 * 0x2c);
    }

    #[test]
    fn text_blocks_carry_justification_and_string() {
        let p = pieces(&view());
        let b = text_block(&p.texts[1], 0xdead_0001);
        let flags = u32::from_le_bytes(b[0..4].try_into().unwrap());
        assert_eq!(tags::ui::justification(flags), 2, "centred");
        assert_eq!(tags::ui::justification(justify_flags(Justify::Right)), 1);
        assert_eq!(tags::ui::justification(justify_flags(Justify::Left)), 0);
        assert_eq!(
            u32::from_le_bytes(b[0x24..0x28].try_into().unwrap()),
            0xdead_0001
        );
        assert_eq!(&b[0x1c..0x1e], &p.texts[1].block.rect[0].to_le_bytes());
    }

    const RETAIL: u32 = 0x0010_0000;
    const RETAIL_PANE: u32 = 0x0010_0100;
    const RETAIL_LIST: u32 = 0x0010_0200;
    const AT: u32 = 0x0020_0000;

    /// A Controls-like tag: header layout 0 plus bit 5, one pane with a
    /// list, two texts, a bitmap and a model scene.
    fn retail() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(RETAIL, 0x1000).map(AT, 0x1000);
        g.put_u32(RETAIL, 0x20 | 0x10)
            .put_u16(RETAIL + 4, 39)
            .put_u32(RETAIL + 0x20, 1)
            .put_u32(RETAIL + 0x24, RETAIL_PANE)
            .put_u32(RETAIL + 0x2c, 0x1234_5678);
        g.put_u16(RETAIL_PANE, 3)
            .put_u16(RETAIL_PANE + 2, 7)
            .put_u32(RETAIL_PANE + 0x0c, 1)
            .put_u32(RETAIL_PANE + 0x10, RETAIL_LIST)
            .put_u32(RETAIL_PANE + 0x1c, 2)
            .put_u32(RETAIL_PANE + 0x20, 0x0010_0800)
            .put_u32(RETAIL_PANE + 0x24, 1)
            .put_u32(RETAIL_PANE + 0x28, 0x0010_0900)
            .put_u32(RETAIL_PANE + 0x2c, 1)
            .put_u32(RETAIL_PANE + 0x30, 0x0010_0a00);
        g.put_u32(RETAIL_LIST, 0xaa)
            .put_u16(RETAIL_LIST + 4, 1)
            .put_u16(RETAIL_LIST + 6, 6)
            .put_u32(RETAIL_LIST + 0x10, 0xbb);
        g
    }

    const LIST: ListLayout = ListLayout {
        skin: 0,
        anchor: [-100, -120],
        bitmaps_left_of: i16::MAX,
        visible: 1,
    };

    #[test]
    fn the_tag_copy_holds_only_the_dialogs_blocks() {
        let p = pieces(&view());
        let art: Vec<_> = (0..p.art.len())
            .map(|i| [i as u8 + 1; BITMAP_BLOCK_SIZE as usize])
            .collect();
        let strings: Vec<_> = (0..p.texts.len() as u32).map(|i| 0x5000 + i).collect();
        let mut g = retail();
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art, &strings, &LIST),
            Ok(Some(AT))
        );
        let m: &dyn GuestMemory = &g;
        let u32_at = |a| m.u32(a).unwrap();
        let u16_at = |a| m.u16(a).unwrap();
        assert_eq!(u32_at(AT), 0x20 | LAYOUT_2, "layout 2, the other bits kept");
        assert_eq!(u16_at(AT + 4), 39);
        assert_eq!(u32_at(AT + 0x2c), 0x1234_5678, "the header string stays");
        assert_eq!(u32_at(AT + 0x20), 1);
        let pane = u32_at(AT + 0x24);
        assert_eq!(pane, AT + 0x68);
        assert_eq!(
            (u16_at(pane), u16_at(pane + 2)),
            (3, 7),
            "flags and animation"
        );
        for o in [0x04, 0x14, 0x2c, 0x34, 0x3c, 0x44] {
            assert_eq!(u32_at(pane + o), 0, "block at {o:#x} empty");
        }
        assert_eq!(u32_at(pane + 0x0c), 1);
        let list = u32_at(pane + 0x10);
        assert_eq!(u32_at(list), 0xaa);
        assert_eq!(u32_at(list + 0x10), 0xbb);
        assert_eq!(u16_at(list + 6), 1);
        assert_eq!(
            [u16_at(list + 8) as i16, u16_at(list + 10) as i16],
            LIST.anchor
        );
        assert_eq!(u32_at(pane + 0x24), 10);
        let bitmaps = u32_at(pane + 0x28);
        for i in 0..10u32 {
            assert_eq!(
                m.u8(bitmaps + i * BITMAP_BLOCK_SIZE + 0x37).unwrap(),
                i as u8 + 1
            );
        }
        assert_eq!(u32_at(pane + 0x1c), 8);
        let texts = u32_at(pane + 0x20);
        for (i, t) in p.texts.iter().enumerate() {
            let mut b = [0u8; TEXT_BLOCK_SIZE as usize];
            m.read(texts + i as u32 * TEXT_BLOCK_SIZE, &mut b).unwrap();
            assert_eq!(b, text_block(t, 0x5000 + i as u32), "{}", t.id);
        }
        assert!(texts + 8 * TEXT_BLOCK_SIZE == AT + p.tag_size());
    }

    #[test]
    fn a_tag_copy_needs_a_block_per_piece_and_a_retail_list() {
        let p = pieces(&view());
        let art = vec![[0u8; BITMAP_BLOCK_SIZE as usize]; p.art.len()];
        let strings = vec![0; p.texts.len()];
        let mut g = retail();
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art[1..], &strings, &LIST),
            Ok(None)
        );
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art, &strings[1..], &LIST),
            Ok(None)
        );
        g.put_u32(RETAIL_PANE + 0x0c, 0);
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art, &strings, &LIST),
            Ok(None)
        );
        g.put_u32(RETAIL + 0x20, 0);
        assert_eq!(
            write_tag(&mut g, RETAIL, AT, &p, &art, &strings, &LIST),
            Ok(None)
        );
    }

    /// `0x149f1e` and the dialogs differ only in the window kind.
    #[test]
    fn a_dialog_request_differs_from_open_screens_in_the_window() {
        let menu = request(MAIN_WINDOW, 0b10, 0x002b_7714);
        let dialog = request(DIALOG_WINDOW, 0b10, 0x002b_7714);
        let word = |r: &[u8; 32], o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        assert_eq!(&menu[0..4], &[0, 0, 2, 0], "zero word, then the mask");
        assert_eq!(word(&menu, 4), 5);
        assert_eq!(word(&dialog, 4), 1);
        for r in [&menu, &dialog] {
            assert_eq!(word(r, 8), 4);
            assert_eq!(word(r, 0xc), 0);
            assert_eq!([word(r, 0x10), word(r, 0x14), word(r, 0x18)], [u32::MAX; 3]);
            assert_eq!(word(r, 0x1c), 0x002b_7714);
        }
        let differ: Vec<_> = (0..32).filter(|&i| menu[i] != dialog[i]).collect();
        assert_eq!(differ, [4]);
    }

    /// The retail fixture with `n` bitmaps, each block filled with its
    /// index + 1.
    fn retail_with_bitmaps(n: u32) -> FakeGuest {
        const BITMAPS: u32 = 0x0010_0800;
        let mut g = retail();
        g.put_u32(RETAIL_PANE + 0x24, n)
            .put_u32(RETAIL_PANE + 0x28, BITMAPS);
        for i in 0..n {
            for b in 0..BITMAP_BLOCK_SIZE {
                g.put_u8(BITMAPS + i * BITMAP_BLOCK_SIZE + b, i as u8 + 1);
            }
        }
        g
    }

    #[test]
    fn a_rows_dialog_keeps_the_bases_root_backing_and_list() {
        let mut g = retail_with_bitmaps(3);
        let list = ListLayout {
            skin: 2,
            anchor: [-254, 87],
            bitmaps_left_of: i16::MIN,
            visible: 4,
        };
        let text = [7u8; TEXT_BLOCK_SIZE as usize];
        assert_eq!(
            write_base_tag(&mut g, RETAIL, AT, &list, &[1], &[text]),
            Ok(Some((AT, base_tag_size(2, 1))))
        );
        let m: &dyn GuestMemory = &g;
        let u32_at = |a| m.u32(a).unwrap();
        let u16_at = |a| m.u16(a).unwrap();
        assert_eq!(u32_at(AT), 0x20 | 0x10, "the root's header layout as it is");
        assert_eq!(u16_at(AT + 4), 39);
        assert_eq!(u32_at(AT + 0x2c), 0x1234_5678, "the header string stays");
        let pane = u32_at(AT + 0x24);
        assert_eq!((u32_at(AT + 0x20), pane), (1, AT + 0x68));
        assert_eq!((u16_at(pane), u16_at(pane + 2)), (3, 7));
        for o in [0x04, 0x14, 0x2c, 0x34, 0x3c, 0x44] {
            assert_eq!(u32_at(pane + o), 0, "block at {o:#x} empty");
        }
        let l = u32_at(pane + 0x10);
        assert_eq!(u32_at(pane + 0x0c), 1);
        assert_eq!((u32_at(l), u32_at(l + 0x10)), (0xaa, 0xbb), "the rest kept");
        assert_eq!((u16_at(l + 4), u16_at(l + 6)), (2, 4), "skin and rows");
        assert_eq!([u16_at(l + 8) as i16, u16_at(l + 10) as i16], [-254, 87]);
        // Bitmap 1 is the base's own content: 0 and 2 are kept, in order.
        assert_eq!(u32_at(pane + 0x24), 2);
        let b = u32_at(pane + 0x28);
        assert_eq!(m.u8(b).unwrap(), 1);
        assert_eq!(m.u8(b + BITMAP_BLOCK_SIZE).unwrap(), 3);
        assert_eq!(m.u8(b + 2 * BITMAP_BLOCK_SIZE - 1).unwrap(), 3);
        assert_eq!(u32_at(pane + 0x1c), 1);
        assert_eq!(m.u8(u32_at(pane + 0x20)).unwrap(), 7);
    }

    #[test]
    fn a_rows_dialog_takes_at_most_sixteen_bitmaps_and_needs_a_list() {
        let mut g = retail_with_bitmaps(20);
        assert_eq!(
            write_base_tag(&mut g, RETAIL, AT, &LIST, &[], &[]),
            Ok(Some((AT, base_tag_size(BASE_BITMAPS, 0))))
        );
        let pane = g.u32(AT + 0x24).unwrap();
        assert_eq!(g.u32(pane + 0x24), Ok(BASE_BITMAPS));
        assert_eq!(g.u32(pane + 0x1c), Ok(0), "no texts");
        // Visible 0 keeps the base's count.
        let l = g.u32(pane + 0x10).unwrap();
        g.put_u16(RETAIL_LIST + 6, 6);
        let none = ListLayout { visible: 0, ..LIST };
        write_base_tag(&mut g, RETAIL, AT, &none, &[], &[]).unwrap();
        assert_eq!(g.u16(l + 6), Ok(6));
        g.put_u32(RETAIL_PANE + 0x0c, 0);
        assert_eq!(
            write_base_tag(&mut g, RETAIL, AT, &LIST, &[], &[]),
            Ok(None)
        );
        g.put_u32(RETAIL + 0x20, 0);
        assert_eq!(
            write_base_tag(&mut g, RETAIL, AT, &LIST, &[], &[]),
            Ok(None)
        );
    }

    #[test]
    fn kit_dialog_labels_have_a_locale_even_when_the_error_frame_has_none() {
        use engine::entry::{EntryFuture, Regs, Ret, testing};

        const ENTRIES: u32 = 0x0030_0000;
        const HOST: u32 = 0x0040_0000;
        const DIALOG_TAG: u32 = 0xaaaa_0001;
        const HOST_TAG: u32 = 0xbbbb_0002;
        let d = Registered {
            name: "test card",
            vtable: 0,
            page: AT,
            loader: 0,
            base: DialogBase::Large,
            list: base_layout(DialogBase::Large, 4),
        };
        for (dialog_strings, host_strings, expected) in [
            (u32::MAX, 0xaaaa_1234, Some(0xaaaa_1234)),
            (0xabcd_0003, u32::MAX, Some(0xabcd_0003)),
            (u32::MAX, u32::MAX, None),
        ] {
            let mut g = retail_with_bitmaps(3);
            g.map(points::TAG_ENTRIES.base, 4)
                .map(ENTRIES, 0x30)
                .map(HOST, SCREEN_ROOT_SIZE)
                .map(0x0050_0000, 0x1000)
                .put_u32(points::TAG_ENTRIES.base, ENTRIES)
                .put_u32(ENTRIES + 0x18, RETAIL)
                .put_u32(ENTRIES + 0x28, HOST)
                .put_u32(RETAIL + SCREEN_STRINGS + 4, dialog_strings)
                .put_u32(HOST + SCREEN_STRINGS + 4, host_strings);
            let f = move |cx| -> EntryFuture {
                Box::pin(async move {
                    prepare(&cx, &d, list_class::CONTROLS_SCREEN_ID, d.name).await?;
                    Ok(Ret::new(0, 0))
                })
            };
            let regs = Regs { esp: 0x0050_0800, ..Default::default() };
            let run = testing::run(&f, g, regs, |c, _| {
                assert_eq!(c.target, points::SCREEN_TAG_BY_ID.base);
                let eax = match c.args[0] {
                    list_class::CONTROLS_SCREEN_ID => HOST_TAG,
                    id if id == DialogBase::Large.screen_id() as u32 => DIALOG_TAG,
                    other => panic!("unexpected screen {other}"),
                };
                Regs { eax, ..Default::default() }
            });
            assert_eq!(run.result, Ok(Ret::new(0, 0)));
            if let Some(strings) = expected {
                assert_eq!(run.guest.u32(AT + SCREEN_STRINGS + 4), Ok(strings));
                assert_eq!(run.guest.u32(AT + TAG_SOURCE), Ok(RETAIL));
            } else {
                assert_eq!(run.guest.u32(AT + TAG_SOURCE), Ok(0));
            }
            assert_eq!(run.guest.u32(RETAIL + SCREEN_STRINGS + 4), Ok(dialog_strings));
        }
    }

    /// A dialog class's screens get its copy once [`prepare`] made it, and
    /// nothing before; other vtables are not dialogs.
    #[test]
    fn a_dialog_class_is_served_its_copy_once_made() {
        const PAGE: u32 = 0x0030_0000;
        let d = Registered {
            name: "test",
            vtable: 0x0071_0000,
            page: PAGE,
            loader: 0x0072_0000,
            base: DialogBase::QuickOptions,
            list: base_layout(DialogBase::QuickOptions, 3),
        };
        register(d);
        register(Registered { page: PAGE, ..d });
        assert_eq!(by_vtable(d.vtable), Some(d));
        assert_eq!(by_loader(d.loader), Some(d));
        assert_eq!(by_vtable(0x0071_0004), None);
        let mut g = FakeGuest::new();
        g.map(PAGE, 0x1000);
        assert_eq!(served(&g, d.vtable), Ok(None), "not made yet");
        g.put_u32(PAGE + TAG_SOURCE, 0x0010_0000);
        assert_eq!(served(&g, d.vtable), Ok(Some(PAGE)));
        assert_eq!(served(&g, 0x0071_0004), Ok(None));
    }

    #[test]
    fn a_bases_list_shows_every_row_on_its_skin() {
        let q = base_layout(DialogBase::QuickOptions, 3);
        assert_eq!((q.skin, q.anchor, q.visible), (2, [-254, 87], 3));
        assert_eq!(q.bitmaps_left_of, i16::MIN);
        let l = base_layout(DialogBase::Large, 0);
        assert_eq!((l.skin, l.anchor, l.visible), (0, [-235, 30], 1));
    }

    /// B and Back on the dialogs' window are ours to close; anything else,
    /// or a screen on another window, goes on to the class's handler.
    #[test]
    fn b_and_back_close_a_screen_on_the_dialogs_window() {
        use crate::list_class::BUTTON_EVENT;
        const SCREEN: u32 = 0x0040_0000;
        let mut g = FakeGuest::new();
        g.map(SCREEN, 0x100);
        g.put_u32(
            SCREEN + crate::ui_kit::lifecycle::SCREEN_WINDOW,
            DIALOG_WINDOW,
        );
        assert_eq!(on_dialog_window(&g, SCREEN), Ok(true));
        assert_eq!(closes_itself(&g, SCREEN, BUTTON_EVENT, 1), Ok(true));
        assert_eq!(closes_itself(&g, SCREEN, BUTTON_EVENT, 13), Ok(true));
        assert_eq!(closes_itself(&g, SCREEN, BUTTON_EVENT, 0), Ok(false));
        assert_eq!(closes_itself(&g, SCREEN, 0, 1), Ok(false));
        assert_eq!(
            closes_itself(&g, 0x0050_0000, BUTTON_EVENT, 0),
            Ok(false),
            "A on an unmapped screen: the window is not read"
        );
        g.put_u32(
            SCREEN + crate::ui_kit::lifecycle::SCREEN_WINDOW,
            MAIN_WINDOW,
        );
        assert_eq!(closes_itself(&g, SCREEN, BUTTON_EVENT, 1), Ok(false));
    }

    /// Closing a dialog-window screen sends it the close message, unless
    /// it is closing already; a screen on another window closes through
    /// retail's close by window and slot.
    #[test]
    fn closing_a_dialog_sends_the_close_message_once() {
        use crate::ui_kit::lifecycle::{SCREEN_CLOSE, SCREEN_SLOT, SCREEN_WINDOW};
        use engine::entry::{EntryFuture, GuestCall as Gc, Regs, testing};
        const SCREEN: u32 = 0x0040_0000;
        let guest = |window: u32, state: u8| {
            let mut g = FakeGuest::new();
            g.map(SCREEN, 0x100).map(0x0030_0000, 0x1000);
            g.put_u32(SCREEN + SCREEN_WINDOW, window)
                .put_u32(SCREEN + SCREEN_SLOT, 4)
                .put_u8(SCREEN + SCREEN_STATE, state);
            g
        };
        let regs = Regs {
            esp: 0x0030_0800,
            ..Default::default()
        };
        let f = |cx| -> EntryFuture {
            Box::pin(async move {
                close(&cx, SCREEN).await?;
                Ok(engine::entry::Ret::new(0, 0))
            })
        };
        let run = testing::run(&f, guest(DIALOG_WINDOW, 0), regs, |_, _| Regs::default());
        assert_eq!(
            run.calls,
            [Gc::new(SCREEN_MESSAGE).ecx(SCREEN).args([MESSAGE_CLOSE])]
        );
        let run = testing::run(&f, guest(DIALOG_WINDOW, 2), regs, |_, _| Regs::default());
        assert_eq!(run.calls, [], "already closing");
        let run = testing::run(&f, guest(MAIN_WINDOW, 0), regs, |_, _| Regs::default());
        assert_eq!(run.calls, [Gc::new(SCREEN_CLOSE).eax(4).ecx(MAIN_WINDOW)]);
    }
}
