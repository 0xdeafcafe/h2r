//! Our own list screen class (docs/engine/feature-engine.md "UI step b"): a screen
//! of the `ui` model with no retail function patched for it.
//!
//! **What it is.** A Controls-shaped screen (the Controls tag's layout,
//! built by the Controls constructor) whose screen and list handler point
//! at our copies of the Controls vtables, with these entries of ours (async
//! entries at arena addresses, docs/formats/ui.md "Screen and list classes"):
//!
//! - **loader**: what `0x149f1e` is handed. It does what the Controls loader
//!   does (allocate, construct, push), and between construct and push it
//!   adopts the screen: our vtables, our row chosen in the press record, and
//!   the rows trimmed to the model's.
//! - **row text** (list slot 20): the model's label and value for the item's
//!   row, and, for a class with a description, the focused row's
//!   description when the focus moved.
//! - **row chosen** (the press record's function): the model's press.
//! - **set up pane** (screen slot 18): the retail one, then our title on the
//!   header text widget and, for a class with a description, its text widget
//!   (made by the pane builder's own text step from a block of ours) and,
//!   for a class with a picture, a bitmap widget made by the pane builder's
//!   bitmap step from a copy of a retail screen's bitmap block.
//! - **loader slot** (screen slot 26): our loader, so the screen manager
//!   re-opens it as ours.
//!
//! Nothing here tells our screens apart from retail ones at run time: the
//! retail functions never see them. The rows' datum array is built at our
//! count: while our loader runs the Controls constructor, the datum array
//! constructor's hook (`layout_glue`, `0x1a480d`) asks [`array_count`].
//! Past six rows the list scrolls through the Controls list's six item
//! widgets.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use engine::DatumHandle;
use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points::{self, CONTROLS_LIST_VTABLE, CONTROLS_SCREEN_VTABLE};
use engine::view::ui::{self, Item, Screen, list_slot, screen_slot};
use runtime::guest::Permissions;

use crate::ui::{Chosen, List, Model};

/// The Controls list's row chosen, which the press record holds until we
/// adopt the screen.
pub const CONTROLS_ROW_CHOSEN: u32 = 0x002c_8719;

/// Our entries and vtable copies, once built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Entries {
    pub loader: u32,
    pub row_text: u32,
    pub row_chosen: u32,
    pub setup_pane: u32,
    pub loader_slot: u32,
    /// Our copy of the Controls screen vtable, then of the list's.
    pub screen_vtable: u32,
    pub list_vtable: u32,
}

/// What a screen of our class shows. Every call is made with no guest
/// memory held (the settings store may be read or written).
pub trait Rows: Send + Sync {
    /// The title's string id.
    fn title(&self) -> u32;
    /// The rows when the screen opens.
    fn count(&self) -> usize;
    /// `(text index, string id)` for each text of row `row`.
    fn texts(&self, row: i16) -> Vec<(u32, u32)>;
    /// A press on row `row`.
    fn press(&self, row: i16) -> Chosen;
    /// The description's string id for row `row`, for a class with a
    /// description text ([`ListClass::description`]).
    fn description(&self, _row: i16) -> Option<u32> {
        None
    }
    /// The picture's tint (red, green, blue multipliers) for row `row`, for
    /// a class with a picture ([`ListClass::picture`]); `None` leaves it.
    fn tint(&self, _row: i16) -> Option<[f32; 3]> {
        None
    }
}

/// A picture of ours on the screen: a retail screen's bitmap block, copied
/// (docs/formats/ui.md "Bitmap block") and placed with its top-left at `at`. The
/// block is found at run time through the interface's screen list, so the
/// bitmap reference is the loaded map's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureSpec {
    /// The retail screen whose pane holds the bitmap (`wgit +4`).
    pub screen_id: u32,
    /// Its pane, and the bitmap's index in that pane's bitmaps.
    pub pane: u32,
    pub bitmap: u32,
    /// The image index the block must show, as a check that it is the one
    /// meant.
    pub image: i16,
    /// Top-left, `x, y` (Y up, origin at the screen centre).
    pub at: [i16; 2],
}

/// A bitmap block definition's size.
pub const BITMAP_BLOCK_SIZE: u32 = 0x38;
/// A pane definition's size; its bitmaps block (count, address) at `+0x24`.
const PANE_SIZE: u32 = 0x4c;
const PANE_BITMAPS: u32 = 0x24;
/// A `wgit`'s panes block (count, address).
const SCREEN_PANES: u32 = 0x20;
/// A bitmap block's animation, image index, top-left point and `bitm`
/// reference (group, then datum handle).
const BITMAP_ANIMATION: usize = 0x04;
const BITMAP_IMAGE: usize = 0x0a;
const BITMAP_POINT: usize = 0x0c;
const BITMAP_GROUP: usize = 0x18;
const BITMAP_HANDLE: usize = 0x1c;
/// A widget's tint: three floats the bitmap draw (`0x2b0565`) multiplies
/// into its colour with the alpha at `+0x64`; the widget header
/// (`0x22e27b`) sets them to 1.
pub const WIDGET_TINT: u32 = 0x28;

/// The bitmap block `spec` names, from the screen tag whose datum handle
/// is `handle` (`SCREEN_TAG_BY_ID`), ready for the pane builder: no
/// animation, at `spec.at`. Says why when there is none.
pub fn picture_block(
    m: &dyn GuestMemory,
    handle: u32,
    spec: &PictureSpec,
) -> Result<[u8; BITMAP_BLOCK_SIZE as usize], String> {
    let fault = |e: MemFault| format!("unreadable ({e:?})");
    if handle == u32::MAX {
        return Err(format!(
            "screen {} is not in this interface",
            spec.screen_id
        ));
    }
    let entries = m.u32(points::TAG_ENTRIES.base).map_err(fault)?;
    let tag = m
        .u32(entries.wrapping_add((handle & 0xffff) * 0x10 + 8))
        .map_err(fault)?;
    let (panes, pane_at) = (
        m.u32(tag.wrapping_add(SCREEN_PANES)).map_err(fault)?,
        m.u32(tag.wrapping_add(SCREEN_PANES + 4)).map_err(fault)?,
    );
    if spec.pane >= panes {
        return Err(format!("screen {} has {panes} panes", spec.screen_id));
    }
    let pane = pane_at.wrapping_add(spec.pane * PANE_SIZE);
    let (bitmaps, bitmap_at) = (
        m.u32(pane.wrapping_add(PANE_BITMAPS)).map_err(fault)?,
        m.u32(pane.wrapping_add(PANE_BITMAPS + 4)).map_err(fault)?,
    );
    if spec.bitmap >= bitmaps {
        return Err(format!(
            "pane {} of screen {} has {bitmaps} bitmaps",
            spec.pane, spec.screen_id
        ));
    }
    let mut b = [0u8; BITMAP_BLOCK_SIZE as usize];
    m.read(
        bitmap_at.wrapping_add(spec.bitmap * BITMAP_BLOCK_SIZE),
        &mut b,
    )
    .map_err(fault)?;
    let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let image = i16::from_le_bytes([b[BITMAP_IMAGE], b[BITMAP_IMAGE + 1]]);
    if word(BITMAP_GROUP) != engine::tags::GroupTag::from_chars(*b"bitm").0
        || word(BITMAP_HANDLE) == u32::MAX
        || image != spec.image
    {
        return Err(format!(
            "bitmap {} of pane {} of screen {} is not the one meant (image {image})",
            spec.bitmap, spec.pane, spec.screen_id
        ));
    }
    b[BITMAP_ANIMATION..BITMAP_ANIMATION + 4].fill(0);
    b[BITMAP_POINT..BITMAP_POINT + 2].copy_from_slice(&spec.at[0].to_le_bytes());
    b[BITMAP_POINT + 2..BITMAP_POINT + 4].copy_from_slice(&spec.at[1].to_le_bytes());
    Ok(b)
}

/// Sets widget `widget`'s tint.
pub fn set_tint(m: &mut dyn GuestMemory, widget: u32, tint: [f32; 3]) -> Result<(), MemFault> {
    for (i, c) in tint.iter().enumerate() {
        m.set_u32(widget + WIDGET_TINT + 4 * i as u32, c.to_bits())?;
    }
    Ok(())
}

/// A description text of ours on the screen: a text block definition
/// (docs/formats/ui.md "Text block") the pane builder's text step makes a widget
/// from. Its string starts empty; row text sets the focused row's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBlock {
    /// Top, left, bottom, right (Y up, origin at the screen centre).
    pub rect: [i16; 4],
    pub font: i16,
    /// Red, green, blue.
    pub colour: [f32; 3],
}

/// A text block definition's size.
pub const TEXT_BLOCK_SIZE: u32 = 0x2c;

impl TextBlock {
    /// The block's bytes: left-justified (flags bit 0), the larger text
    /// widget (bit 4 clear), no animation, the null string id.
    pub fn bytes(&self) -> [u8; TEXT_BLOCK_SIZE as usize] {
        let mut b = [0u8; TEXT_BLOCK_SIZE as usize];
        b[0..4].copy_from_slice(&1u32.to_le_bytes());
        b[0xa..0xc].copy_from_slice(&self.font.to_le_bytes());
        for (i, c) in self.colour.iter().enumerate() {
            b[0x10 + 4 * i..0x14 + 4 * i].copy_from_slice(&c.to_le_bytes());
        }
        for (i, v) in self.rect.iter().enumerate() {
            b[0x1c + 2 * i..0x1e + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        b
    }
}

/// The description widget of each open screen of a class and the row it
/// shows, so row text sets it only when the focus moved.
#[derive(Default)]
pub struct Shown(Mutex<Vec<ShownEntry>>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShownEntry {
    screen: u32,
    widget: u32,
    picture: Option<u32>,
    row: Option<i16>,
}

/// Screens remembered at once; a screen is set up again before it draws, so
/// an older entry is never needed.
const SHOWN_KEPT: usize = 8;

impl Shown {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<ShownEntry>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Screen `screen`'s description widget is `widget`, showing nothing.
    pub fn set_widget(&self, screen: u32, widget: u32) {
        let mut v = self.lock();
        v.retain(|e| e.screen != screen);
        v.push(ShownEntry {
            screen,
            widget,
            picture: None,
            row: None,
        });
        if v.len() > SHOWN_KEPT {
            v.remove(0);
        }
    }
    /// Screen `screen`'s picture widget is `widget` (after its description
    /// widget was set).
    pub fn set_picture(&self, screen: u32, widget: u32) {
        if let Some(e) = self.lock().iter_mut().find(|e| e.screen == screen) {
            e.picture = Some(widget);
        }
    }
    /// Screen `screen`'s picture widget.
    pub fn picture(&self, screen: u32) -> Option<u32> {
        self.lock()
            .iter()
            .find(|e| e.screen == screen)
            .and_then(|e| e.picture)
    }
    /// Row `row` is focused on `screen`: its description widget when that is
    /// another row than it shows (it shows `row` from now), else `None`.
    pub fn focus(&self, screen: u32, row: Option<i16>) -> Option<u32> {
        let mut v = self.lock();
        let e = v.iter_mut().find(|e| e.screen == screen)?;
        if e.row == row {
            return None;
        }
        e.row = row;
        Some(e.widget)
    }
}

/// Where the description's text block goes in the class's table page.
const DESCRIPTION_BLOCK: u32 = 0x100;
/// Where the picture's bitmap block goes, after the text block.
const PICTURE_BLOCK: u32 = 0x140;
const _: () = assert!(PICTURE_BLOCK >= DESCRIPTION_BLOCK + TEXT_BLOCK_SIZE);

/// Where a class's description block goes, the block, and where to keep
/// each screen's widget.
#[derive(Clone)]
pub struct Description {
    pub at: u32,
    pub block: TextBlock,
    pub shown: Arc<Shown>,
    /// A picture beside it, its block written at `picture_at`.
    pub picture: Option<PictureSpec>,
    pub picture_at: u32,
}

/// A screen of the `ui` model.
pub struct ModelRows {
    pub model: Arc<Model>,
    pub screen: &'static str,
}

impl Rows for ModelRows {
    fn title(&self) -> u32 {
        self.model.title_id(self.screen).unwrap_or(0)
    }
    fn count(&self) -> usize {
        self.model.rows(List::Screen(self.screen)).len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        self.model
            .row(List::Screen(self.screen), row)
            .map(|r| self.model.texts(r).to_vec())
            .unwrap_or_default()
    }
    fn press(&self, row: i16) -> Chosen {
        self.model
            .row(List::Screen(self.screen), row)
            .map_or(Chosen::Nothing, |r| self.model.press(r))
    }
}

/// The loaders of the classes built so far, by screen id, so an `Open` row
/// (on a screen of ours or on a retail list) finds the screen it names.
static LOADERS: Mutex<BTreeMap<&'static str, u32>> = Mutex::new(BTreeMap::new());

/// The loader of our screen `screen`, once its class is built.
pub fn loader_of(screen: &str) -> Option<u32> {
    LOADERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(screen)
        .copied()
}

/// The rows our loader is building a screen with (0: no loader of ours is
/// running the Controls constructor).
static BUILDING: AtomicU32 = AtomicU32::new(0);

/// The count the datum array named `name` is built with, `count` being what
/// the game asks for: while one of our loaders runs the Controls
/// constructor, the Controls rows' array gets the screen's row count. No
/// list destructor frees the array at `list + 0x70` (`0x2b7563`, `0x2376c8`,
/// `0x1474e8`, `0x22e34b` read 2026-11-16), so it is sized when it is made
/// rather than replaced after.
pub fn array_count(name: u32, count: u32) -> u32 {
    match BUILDING.load(Ordering::SeqCst) {
        n if n != 0 && name == crate::speaker_row::LIST_NAME => n,
        _ => count,
    }
}

/// Clears [`BUILDING`] when the constructor call ends, however it ends.
struct Building;

impl Building {
    fn start(rows: usize) -> Self {
        BUILDING.store(u32::try_from(rows).unwrap_or(0), Ordering::SeqCst);
        Building
    }
}

impl Drop for Building {
    fn drop(&mut self) {
        BUILDING.store(0, Ordering::SeqCst);
    }
}

/// A screen of ours as our own list class. See the module docs.
pub struct ListClass {
    /// The feature id (`features.<id>` switches it).
    pub id: &'static str,
    /// The screen id an `Open` row names.
    pub screen: &'static str,
    rows: Arc<dyn Rows>,
    description: Option<TextBlock>,
    picture: Option<PictureSpec>,
    loader: Arc<AtomicU32>,
}

impl ListClass {
    /// The model's screen `screen`.
    pub fn new(id: &'static str, screen: &'static str, model: Arc<Model>) -> Self {
        Self::with_rows(id, screen, Arc::new(ModelRows { model, screen }))
    }
    /// A screen whose rows come from elsewhere.
    pub fn with_rows(id: &'static str, screen: &'static str, rows: Arc<dyn Rows>) -> Self {
        ListClass {
            id,
            screen,
            rows,
            description: None,
            picture: None,
            loader: Arc::new(AtomicU32::new(0)),
        }
    }
    /// With a description text: the focused row's [`Rows::description`].
    pub fn description(mut self, block: TextBlock) -> Self {
        self.description = Some(block);
        self
    }
    /// With a picture beside the description, tinted per focused row
    /// ([`Rows::tint`]). Only a class with a description shows one.
    pub fn picture(mut self, spec: PictureSpec) -> Self {
        self.picture = Some(spec);
        self
    }
    /// The loader address to open it with (`0x149f1e(mask, loader)`), once
    /// built; 0 before.
    pub fn loader(&self) -> u32 {
        self.loader.load(Ordering::SeqCst)
    }
}

/// Writes our vtables: the Controls ones as the game has them, with our
/// slots in. Once: a screen adopted later finds them written.
pub fn write_vtables(m: &mut dyn GuestMemory, e: &Entries) -> Result<(), MemFault> {
    if m.u32(e.screen_vtable)? != 0 {
        return Ok(());
    }
    for (from, to, n) in [
        (
            CONTROLS_SCREEN_VTABLE.base,
            e.screen_vtable,
            screen_slot::COUNT,
        ),
        (CONTROLS_LIST_VTABLE.base, e.list_vtable, list_slot::COUNT),
    ] {
        for i in 0..n {
            let v = ui::slot(m, from, i)?;
            m.set_u32(to + 4 * i, v)?;
        }
    }
    m.set_u32(e.screen_vtable + 4 * screen_slot::SETUP_PANE, e.setup_pane)?;
    m.set_u32(e.screen_vtable + 4 * screen_slot::LOADER, e.loader_slot)?;
    m.set_u32(e.list_vtable + 4 * list_slot::ROW_TEXT, e.row_text)?;
    Ok(())
}

/// What adopting a freshly constructed Controls screen did.
#[derive(Debug, PartialEq, Eq)]
pub enum Adopted {
    /// Ours now, showing `rows` rows.
    Ours { rows: usize },
    /// Not what the Controls constructor builds; left as it is.
    NotControls(String),
}

/// Makes the Controls screen at `screen` ours: vtables, row chosen, and the
/// rows trimmed to `rows`. Rows cannot be added here: the array is built
/// with the count the loader asked for ([`array_count`]), and a screen
/// opened some other way keeps what its constructor made. The loaded byte
/// is set as the Controls loader sets it.
pub fn adopt(
    m: &mut dyn GuestMemory,
    screen: u32,
    e: &Entries,
    rows: usize,
) -> Result<Adopted, MemFault> {
    let s = Screen::at(screen);
    let list = s.list();
    let press = list.press(ui::CONTROLS_PRESS);
    let (sv, lv, pf, pt) = (
        s.vtable(m)?,
        list.vtable(m)?,
        press.function(m)?,
        press.target(m)?,
    );
    if sv != CONTROLS_SCREEN_VTABLE.base
        || lv != CONTROLS_LIST_VTABLE.base
        || pf != CONTROLS_ROW_CHOSEN
        || pt != list.addr
    {
        return Ok(Adopted::NotControls(format!(
            "screen vtable {sv:#x}, list vtable {lv:#x}, press {pf:#x} on {pt:#x}"
        )));
    }
    write_vtables(m, e)?;
    s.set_vtable(m, e.screen_vtable)?;
    list.set_vtable(m, e.list_vtable)?;
    press.set_function(m, e.row_chosen)?;
    let array = list.row_array(m)?;
    let live = array.live(m, i32::MAX)?;
    for (h, _) in live.iter().skip(rows) {
        array.array.delete(m, *h)?;
    }
    s.set_loaded(m, 1)?;
    Ok(Adopted::Ours {
        rows: rows.min(live.len()),
    })
}

/// The row an item shows.
fn item_row(m: &dyn GuestMemory, item: u32) -> Result<Option<i16>, MemFault> {
    Ok(Item::at(item)
        .row(m)?
        .map(|h: DatumHandle| h.index() as i16))
}

/// Says `what` on stderr once per `seen`.
fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

/// The loader: the Controls loader's steps with the adoption in between.
/// Stdcall, the request, `ret 4`; the screen in EAX (0 when allocation
/// failed, where the retail loader would write through null).
pub async fn load(
    cx: Cx,
    e: Entries,
    rows: Arc<dyn Rows>,
    name: &'static str,
) -> Result<Ret, EntryError> {
    // The rows are counted with no guest memory held.
    let rows = rows.count();
    let request = cx.arg(0)?;
    let screen = cx
        .call(GuestCall::new(points::UI_ALLOC.base).args([ui::CONTROLS_SCREEN_SIZE]))
        .await?
        .eax;
    if screen == 0 {
        eprintln!("{name}: no memory for the screen; not opened");
        return Ok(Ret::new(0, 4));
    }
    let (a, b, player) = cx.read(|m| -> Result<_, MemFault> {
        Ok((
            m.u32(request + 4)?,
            m.u32(request + 8)?,
            u32::from(m.u16(request + 2)?),
        ))
    })?;
    {
        let _building = Building::start(rows);
        cx.call(
            GuestCall::new(points::CONTROLS_SCREEN_CTOR.base)
                .edi(screen)
                .args([a, b, player]),
        )
        .await?;
    }
    match cx.write(|m| adopt(m, screen, &e, rows))? {
        Adopted::Ours { rows } => eprintln!("{name}: opened with {rows} rows (our list class)"),
        Adopted::NotControls(why) => {
            eprintln!(
                "{name}: the Controls constructor built something else ({why}); shown as Controls"
            )
        }
    }
    cx.call(
        GuestCall::new(points::SCREEN_PUSH.base)
            .edi(screen)
            .ebx(request),
    )
    .await?;
    Ok(Ret::new(screen, 4))
}

/// Sets each `(text index, string id)` on the item widget `item`'s text
/// widget of that index (`ITEM_TEXT`, then `TEXT_SET`), as row text does.
pub async fn draw_texts(cx: &Cx, item: u32, texts: Vec<(u32, u32)>) -> Result<(), EntryError> {
    for (index, id) in texts {
        let text = cx
            .call(
                GuestCall::new(points::ITEM_TEXT.base)
                    .ecx(item)
                    .args([6, index, 0]),
            )
            .await?
            .eax;
        if text != 0 {
            cx.call(
                GuestCall::new(points::TEXT_SET.base)
                    .edi(text)
                    .esi(item)
                    .args([id]),
            )
            .await?;
        }
    }
    Ok(())
}

/// The player mask of a list callback whose first argument is the player
/// chain (`[[chain]]+4` the local player index): `1 << index`, or `None`
/// past the four local players.
pub fn player_mask(m: &dyn GuestMemory, chain: u32) -> Result<Option<u32>, MemFault> {
    let record = m.u32(chain)?;
    let index = m.u32(record.wrapping_add(4))?;
    Ok((index < 4).then(|| 1 << index))
}

/// Opens our screen `screen` for the player of `chain` (`0x149f1e(mask,
/// loader)`). Says why when it cannot.
pub async fn open(cx: &Cx, chain: u32, screen: &str) -> Result<(), EntryError> {
    let Some(loader) = loader_of(screen) else {
        eprintln!("ui: {screen} is not a screen of ours (or its class is off); not opened");
        return Ok(());
    };
    let Some(mask) = cx.read(|m| player_mask(m, chain))? else {
        eprintln!("ui: no local player for the press; {screen} not opened");
        return Ok(());
    };
    eprintln!("ui: opening {screen}");
    cx.call(GuestCall::new(points::OPEN_SCREEN.base).args([mask, loader]))
        .await?;
    Ok(())
}

/// Row text (list slot 20): thiscall on the list, the item and a word,
/// `ret 8`. Each of the row's texts goes to the item's text widget of that
/// index. With `shown` (a class with a description), the focused row's
/// description goes to the screen's description widget when the focus has
/// moved since the last draw.
pub async fn row_text(
    cx: Cx,
    rows: Arc<dyn Rows>,
    shown: Option<Arc<Shown>>,
) -> Result<Ret, EntryError> {
    let item = cx.arg(0)?;
    let row = cx.read(|m| item_row(m, item))?;
    // The rows read the settings store; no guest memory is held here.
    let texts = row.map(|r| rows.texts(r)).unwrap_or_default();
    draw_texts(&cx, item, texts).await?;
    if let Some(shown) = shown {
        let list = cx.entry().ecx;
        let screen = list.wrapping_sub(ui::LIST);
        let focused = cx
            .call(GuestCall::new(points::LIST_FOCUSED_ROW.base).eax(list))
            .await?
            .eax;
        let focused = (focused != u32::MAX).then(|| DatumHandle(focused).index() as i16);
        if let Some(widget) = shown.focus(screen, focused) {
            if let Some(id) = focused.and_then(|r| rows.description(r)) {
                cx.call(
                    GuestCall::new(points::TEXT_SET.base)
                        .edi(widget)
                        .esi(screen)
                        .args([id]),
                )
                .await?;
            }
            if let (Some(picture), Some(tint)) =
                (shown.picture(screen), focused.and_then(|r| rows.tint(r)))
            {
                cx.write(|m| set_tint(m, picture, tint))?;
            }
        }
    }
    Ok(Ret::new(0, 8))
}

/// Row chosen (the press record's function): thiscall, the player chain and
/// a pointer to the row, `ret 8`. A setting row steps; an open row opens
/// the screen of ours it names.
pub async fn row_chosen(cx: Cx, rows: Arc<dyn Rows>) -> Result<Ret, EntryError> {
    let (chain, at) = (cx.arg(0)?, cx.arg(1)?);
    let row = cx.read(|m| m.u16(at))? as i16;
    // The store is written with no guest memory held.
    match rows.press(row) {
        Chosen::Stepped(msg) => eprintln!("{msg}"),
        Chosen::Open(id) => open(&cx, chain, id).await?,
        _ => {}
    }
    Ok(Ret::new(0, 8))
}

/// Screen slot 18: the retail pane set-up, then our title on the header,
/// then our description text widget (written at `description.at`, made by
/// the pane builder's text step). Thiscall, one argument, `ret 4`; its EAX
/// is the retail one's.
pub async fn setup_pane(
    cx: Cx,
    title: u32,
    description: Option<Description>,
    name: &'static str,
) -> Result<Ret, EntryError> {
    let screen = cx.entry().ecx;
    let arg = cx.arg(0)?;
    let r = cx
        .call(
            GuestCall::new(points::SCREEN_SETUP_PANE.base)
                .ecx(screen)
                .args([arg]),
        )
        .await?;
    cx.call(
        GuestCall::new(points::TEXT_SET.base)
            .edi(Screen::at(screen).header())
            .esi(screen)
            .args([title]),
    )
    .await?;
    if let Some(d) = description {
        cx.write(|m| m.write(d.at, &d.block.bytes()))?;
        let widget = cx
            .call(
                GuestCall::new(points::PANE_TEXT_ADD.base)
                    .ebx(screen)
                    .args([0, d.at]),
            )
            .await?
            .eax;
        if widget == 0 {
            eprintln!("{name}: no description text (no memory for the widget)");
        } else {
            d.shown.set_widget(screen, widget);
            if let Some(spec) = d.picture {
                add_picture(&cx, screen, &spec, d.picture_at, &d.shown, name).await?;
            }
        }
    }
    Ok(Ret::new(r.eax, 4))
}

/// Said once per run: the picture could not be made (the line says why).
static PICTURE_MISSING: AtomicBool = AtomicBool::new(false);

/// The picture: its block found through the screen list, copied to `at`,
/// and made a bitmap widget of `screen` by the pane builder's bitmap step.
async fn add_picture(
    cx: &Cx,
    screen: u32,
    spec: &PictureSpec,
    at: u32,
    shown: &Shown,
    name: &str,
) -> Result<(), EntryError> {
    let handle = cx
        .call(GuestCall::new(points::SCREEN_TAG_BY_ID.base).args([spec.screen_id]))
        .await?
        .eax;
    let block = match cx.read(|m| picture_block(m, handle, spec)) {
        Ok(b) => b,
        Err(why) => {
            once(&PICTURE_MISSING, &format!("{name}: no picture: {why}"));
            return Ok(());
        }
    };
    cx.write(|m| m.write(at, &block))?;
    let widget = cx
        .call(
            GuestCall::new(points::PANE_BITMAP_ADD.base)
                .esi(screen)
                .args([at]),
        )
        .await?
        .eax;
    if widget == 0 {
        once(
            &PICTURE_MISSING,
            &format!("{name}: no picture (no memory for the widget)"),
        );
    } else {
        shown.set_picture(screen, widget);
    }
    Ok(())
}

impl feature::Feature for ListClass {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(self.id, "A screen of ours as our own list class")
            .doc("docs/engine/feature-engine.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let rows = Arc::clone(&self.rows);
        let title = rows.title();
        let tables = b.guest_alloc(0x1000, Permissions::RW)?;
        let name = self.id;
        let shown = self.description.map(|_| Arc::new(Shown::default()));
        // The loader needs every address, so the entries are made first and
        // the loader reads them from a cell filled at the end.
        let cell = Arc::new(std::sync::OnceLock::<Entries>::new());
        let loader = {
            let cell = Arc::clone(&cell);
            let rows = Arc::clone(&rows);
            b.entry("loader", move |cx| -> EntryFuture {
                let e = *cell.get().expect("entries are set at build");
                Box::pin(load(cx, e, Arc::clone(&rows), name))
            })?
        };
        let row_text_at = {
            let (rows, shown) = (Arc::clone(&rows), shown.clone());
            let seen = Arc::new(AtomicBool::new(false));
            b.entry("row text", move |cx| -> EntryFuture {
                once(&seen, &format!("{name}: row text"));
                Box::pin(row_text(cx, Arc::clone(&rows), shown.clone()))
            })?
        };
        let row_chosen_at = {
            let rows = Arc::clone(&rows);
            b.entry("row chosen", move |cx| -> EntryFuture {
                Box::pin(row_chosen(cx, Arc::clone(&rows)))
            })?
        };
        let description = self
            .description
            .zip(shown)
            .map(|(block, shown)| Description {
                at: tables + DESCRIPTION_BLOCK,
                block,
                shown,
                picture: self.picture,
                picture_at: tables + PICTURE_BLOCK,
            });
        let setup_pane_at = b.entry("set up pane", move |cx| -> EntryFuture {
            Box::pin(setup_pane(cx, title, description.clone(), name))
        })?;
        let loader_slot = b.entry("loader slot", move |_| -> EntryFuture {
            Box::pin(async move { Ok(Ret::new(loader, 0)) })
        })?;
        let e = Entries {
            loader,
            row_text: row_text_at,
            row_chosen: row_chosen_at,
            setup_pane: setup_pane_at,
            loader_slot,
            screen_vtable: tables,
            list_vtable: tables + 4 * screen_slot::COUNT,
        };
        cell.set(e).ok();
        self.loader.store(loader, Ordering::SeqCst);
        LOADERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.screen, loader);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting_defs::{ALL, FPS};
    use crate::video_screen::{self, Switches};
    use engine::entry::{GuestCall as Gc, Regs, testing};
    use engine::testing::FakeGuest;
    use engine::view::fake;
    use settings::Store;

    const SCREEN: u32 = 0x0010_0000;
    const REQUEST: u32 = 0x0020_0000;
    const STACK: u32 = 0x0030_0800;
    const ROWS: u32 = 0x0040_0000;
    const TABLES: u32 = 0x0050_0000;

    fn entries() -> Entries {
        Entries {
            loader: 0xa0f0_0000,
            row_text: 0xa0f0_0010,
            row_chosen: 0xa0f0_0020,
            setup_pane: 0xa0f0_0030,
            loader_slot: 0xa0f0_0040,
            screen_vtable: TABLES,
            list_vtable: TABLES + 0x70,
        }
    }

    fn video_rows() -> Arc<dyn Rows> {
        Arc::new(ModelRows {
            model: model(),
            screen: video_screen::SCREEN,
        })
    }

    /// One row that opens `target`.
    struct Opens(&'static str);
    impl Rows for Opens {
        fn title(&self) -> u32 {
            0
        }
        fn count(&self) -> usize {
            1
        }
        fn texts(&self, _: i16) -> Vec<(u32, u32)> {
            Vec::new()
        }
        fn press(&self, _: i16) -> Chosen {
            Chosen::Open(self.0)
        }
    }

    /// A press on row 0 by local player `player` of a screen whose row opens
    /// `target`: the calls made.
    fn press_open(target: &'static str, player: u32) -> Vec<Gc> {
        let mut g = guest();
        g.put_u32(STACK + 4, 0x0071_0000)
            .put_u32(0x0071_0000, 0x0072_0000)
            .put_u32(0x0072_0004, player);
        g.put_u32(STACK + 8, 0x0070_0000).put_u16(0x0070_0000, 0);
        let f = move |cx| -> EntryFuture { Box::pin(row_chosen(cx, Arc::new(Opens(target)))) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| Regs::default());
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        run.calls
    }

    #[test]
    fn an_open_row_opens_another_screen_of_ours_for_the_pressing_player() {
        LOADERS
            .lock()
            .unwrap()
            .insert("test-open-target", 0xa0f0_5000);
        assert_eq!(
            press_open("test-open-target", 2),
            [Gc::new(points::OPEN_SCREEN.base).args([1 << 2, 0xa0f0_5000])]
        );
        assert_eq!(press_open("test-open-missing", 0), [], "not ours");
        assert_eq!(press_open("test-open-target", 4), [], "no local player");
    }

    #[test]
    fn a_built_class_is_found_by_its_screen_id() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let class = ListClass::with_rows("test-found", "test-found-screen", Arc::new(Opens("x")));
        r.add(&class).unwrap();
        assert_eq!(loader_of("test-found-screen"), Some(class.loader()));
        assert_eq!(loader_of("test-never-built"), None);
    }

    fn model() -> Arc<Model> {
        Arc::new(video_screen::model(Arc::new(Store::defaults(ALL)), Switches::ALL).unwrap())
    }

    /// The retail vtables (synthetic slot values) and a stack.
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        for i in 0..screen_slot::COUNT {
            g.put_u32(CONTROLS_SCREEN_VTABLE.base + 4 * i, 0x5000 + i);
        }
        for i in 0..list_slot::COUNT {
            g.put_u32(CONTROLS_LIST_VTABLE.base + 4 * i, 0x6000 + i);
        }
        g.map(TABLES, 0x1000).map(STACK - 0x800, 0x1000);
        g
    }

    /// What the Controls constructor leaves: vtables, the press record, and
    /// `n` rows in the list's datum array.
    fn construct(g: &mut dyn GuestMemory, n: usize) {
        let s = Screen::at(SCREEN);
        s.set_vtable(g, CONTROLS_SCREEN_VTABLE.base).unwrap();
        let list = s.list();
        list.set_vtable(g, CONTROLS_LIST_VTABLE.base).unwrap();
        let press = list.press(ui::CONTROLS_PRESS);
        press.set_target(g, list.addr).unwrap();
        press.set_function(g, CONTROLS_ROW_CHOSEN).unwrap();
        // The ported allocator lays out the array as the game's does.
        let mut f = FakeGuest::new();
        let d = fake::datums::<ui::Row>(&mut f, ROWS, n as i32);
        for _ in 0..n {
            fake::push(&mut f, &d);
        }
        let bytes = f.bytes(ROWS - 0x20, 0x200).unwrap();
        g.write(ROWS - 0x20, &bytes).unwrap();
        list.set_rows(g, ROWS).unwrap();
    }

    #[test]
    fn adopting_points_the_screen_at_our_class_and_trims_the_rows() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 7); // grown by the speaker row's list hook
        let e = entries();
        assert_eq!(adopt(&mut g, SCREEN, &e, 4), Ok(Adopted::Ours { rows: 4 }));
        let s = Screen::at(SCREEN);
        assert_eq!(s.vtable(&g), Ok(TABLES));
        assert_eq!(s.list().vtable(&g), Ok(TABLES + 0x70));
        assert_eq!(s.loaded(&g), Ok(1));
        assert_eq!(
            s.list().press(ui::CONTROLS_PRESS).function(&g),
            Ok(e.row_chosen)
        );
        let live = s.list().row_array(&g).unwrap().live(&g, 16).unwrap();
        assert_eq!(
            live.iter().map(|(h, _)| h.index()).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        // Our slots, the retail ones everywhere else.
        let slot = |vt, i| ui::slot(&g, vt, i).unwrap();
        assert_eq!(slot(TABLES, screen_slot::SETUP_PANE), e.setup_pane);
        assert_eq!(slot(TABLES, screen_slot::LOADER), e.loader_slot);
        assert_eq!(slot(TABLES, 0), 0x5000);
        assert_eq!(slot(TABLES, 27), 0x5000 + 27);
        assert_eq!(slot(TABLES + 0x70, list_slot::ROW_TEXT), e.row_text);
        assert_eq!(slot(TABLES + 0x70, list_slot::ITEM_COUNT), 0x6000 + 19);
        assert_eq!(slot(TABLES + 0x70, list_slot::ADD_ITEMS), 0x6000 + 17);
        assert_eq!(slot(TABLES + 0x70, 21), 0x6000 + 21);
        // A second screen finds the tables written and changes nothing.
        g.put_u32(TABLES + 4, 0xbad);
        write_vtables(&mut g, &e).unwrap();
        assert_eq!(ui::slot(&g, TABLES, 1), Ok(0xbad));
    }

    #[test]
    fn more_rows_than_the_array_holds_are_cut_to_its_size() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 7);
        assert_eq!(
            adopt(&mut g, SCREEN, &entries(), 9),
            Ok(Adopted::Ours { rows: 7 })
        );
    }

    #[test]
    fn a_screen_that_is_not_controls_is_left_alone() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 6);
        Screen::at(SCREEN)
            .list()
            .press(ui::CONTROLS_PRESS)
            .set_function(&mut g, 0x1234)
            .unwrap();
        assert!(matches!(
            adopt(&mut g, SCREEN, &entries(), 4),
            Ok(Adopted::NotControls(_))
        ));
        assert_eq!(
            Screen::at(SCREEN).vtable(&g),
            Ok(CONTROLS_SCREEN_VTABLE.base)
        );
        assert_eq!(g.u32(TABLES), Ok(0), "no tables written");
    }

    /// Loader tests run one at a time: the array count is process-wide.
    static LOADING: Mutex<()> = Mutex::new(());

    /// The loader against fake callees: allocate, construct (the fake builds
    /// what the Controls constructor builds), adopt, push.
    #[test]
    fn the_loader_allocates_constructs_adopts_and_pushes() {
        let _serial = LOADING.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        g.put_u16(REQUEST + 2, 0x4)
            .put_u32(REQUEST + 4, 5)
            .put_u32(REQUEST + 8, 4);
        g.put_u32(STACK, 0xdead).put_u32(STACK + 4, REQUEST);
        let e = entries();
        let f = move |cx| -> EntryFuture { Box::pin(load(cx, e, video_rows(), "test")) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let during = std::sync::Mutex::new(None);
        let run = testing::run(&f, g, regs, |c, m| {
            if c.target == points::CONTROLS_SCREEN_CTOR.base {
                *during.lock().unwrap() = Some((
                    array_count(crate::speaker_row::LIST_NAME, 7),
                    array_count(0x1234, 7),
                ));
                construct(m, 7);
            }
            Regs {
                eax: if c.target == points::UI_ALLOC.base {
                    // The fake allocator maps the screen.
                    SCREEN
                } else {
                    0
                },
                ..Default::default()
            }
        });
        let calls: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
        assert_eq!(
            calls,
            [
                points::UI_ALLOC.base,
                points::CONTROLS_SCREEN_CTOR.base,
                points::SCREEN_PUSH.base
            ]
        );
        assert_eq!(run.calls[0].args, [0x9b4]);
        assert_eq!(
            run.calls[1],
            Gc::new(points::CONTROLS_SCREEN_CTOR.base)
                .edi(SCREEN)
                .args([5, 4, 4])
        );
        assert_eq!(
            run.calls[2],
            Gc::new(points::SCREEN_PUSH.base).edi(SCREEN).ebx(REQUEST)
        );
        assert_eq!(run.result, Ok(Ret::new(SCREEN, 4)));
        assert_eq!(Screen::at(SCREEN).vtable(&run.guest), Ok(TABLES));
        // While the constructor ran, the Controls rows' array was asked for
        // our seven rows; any other array, and anything after, as asked.
        assert_eq!(*during.lock().unwrap(), Some((7, 7)));
        assert_eq!(array_count(crate::speaker_row::LIST_NAME, 7), 7);
    }

    #[test]
    fn a_screen_of_fifteen_rows_gets_an_array_of_fifteen() {
        let _serial = LOADING.lock().unwrap_or_else(|e| e.into_inner());
        struct Fifteen;
        impl Rows for Fifteen {
            fn title(&self) -> u32 {
                0
            }
            fn count(&self) -> usize {
                15
            }
            fn texts(&self, _: i16) -> Vec<(u32, u32)> {
                Vec::new()
            }
            fn press(&self, _: i16) -> Chosen {
                Chosen::Nothing
            }
        }
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x400);
        g.put_u16(REQUEST + 2, 0)
            .put_u32(REQUEST + 4, 5)
            .put_u32(REQUEST + 8, 4);
        g.put_u32(STACK, 0xdead).put_u32(STACK + 4, REQUEST);
        let e = entries();
        let f = move |cx| -> EntryFuture { Box::pin(load(cx, e, Arc::new(Fifteen), "test")) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, m| {
            if c.target == points::CONTROLS_SCREEN_CTOR.base {
                // The fake constructor builds what the hook would have it
                // build.
                construct(m, array_count(crate::speaker_row::LIST_NAME, 6) as usize);
            }
            Regs {
                eax: if c.target == points::UI_ALLOC.base {
                    SCREEN
                } else {
                    0
                },
                ..Default::default()
            }
        });
        assert_eq!(run.result, Ok(Ret::new(SCREEN, 4)));
        let live = Screen::at(SCREEN)
            .list()
            .row_array(&run.guest)
            .unwrap()
            .live(&run.guest, 32)
            .unwrap();
        assert_eq!(live.len(), 15);
    }

    #[test]
    fn a_failed_allocation_opens_nothing() {
        let mut g = guest();
        g.put_u32(STACK + 4, REQUEST);
        let f = move |cx| -> EntryFuture { Box::pin(load(cx, entries(), video_rows(), "test")) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| Regs::default());
        assert_eq!(run.calls.len(), 1);
        assert_eq!(run.result, Ok(Ret::new(0, 4)));
    }

    /// Row text draws the model's label and value into the item's text
    /// widgets 0 and 1.
    #[test]
    fn row_text_sets_each_text_of_the_items_row() {
        const ITEM: u32 = 0x0060_0000;
        let m = model();
        let mut g = guest();
        g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
        g.map(ITEM, Item::SIZE);
        Item::at(ITEM)
            .set_row(&mut g, Some(DatumHandle::new(0x8001, 1)))
            .unwrap();
        let list = video_screen::LIST;
        let want = m.texts(m.row(list, 1).unwrap()).to_vec();
        let f = {
            let m = Arc::clone(&m);
            move |cx| -> EntryFuture {
                Box::pin(row_text(
                    cx,
                    Arc::new(ModelRows {
                        model: Arc::clone(&m),
                        screen: video_screen::SCREEN,
                    }),
                    None,
                ))
            }
        };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            // Text widget `index` of the item.
            eax: if c.target == points::ITEM_TEXT.base {
                0x7000 + c.args[1]
            } else {
                0
            },
            ..Default::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        let mut expect = Vec::new();
        for (index, id) in want {
            expect.push(
                Gc::new(points::ITEM_TEXT.base)
                    .ecx(ITEM)
                    .args([6, index, 0]),
            );
            expect.push(
                Gc::new(points::TEXT_SET.base)
                    .edi(0x7000 + index)
                    .esi(ITEM)
                    .args([id]),
            );
        }
        assert_eq!(run.calls, expect);
    }

    #[test]
    fn row_chosen_steps_the_setting_and_calls_nothing() {
        let store = Arc::new(Store::defaults(ALL));
        let fps = store.handle::<u32>(FPS.key);
        let m = Arc::new(video_screen::model(Arc::clone(&store), Switches::ALL).unwrap());
        let mut g = guest();
        g.put_u32(STACK + 8, 0x0070_0000).put_u16(0x0070_0000, 1); // Frame Rate
        let f = {
            let m = Arc::clone(&m);
            move |cx| -> EntryFuture {
                Box::pin(row_chosen(
                    cx,
                    Arc::new(ModelRows {
                        model: Arc::clone(&m),
                        screen: video_screen::SCREEN,
                    }),
                ))
            }
        };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| unreachable!("no guest call"));
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        assert_eq!(fps.get(), 60);
    }

    #[test]
    fn the_pane_set_up_runs_the_retail_one_then_sets_our_title() {
        let mut g = guest();
        g.put_u32(STACK + 4, 0x55);
        let f = |cx| -> EntryFuture { Box::pin(setup_pane(cx, 0x20ff_f077, None, "test")) };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: if c.target == points::SCREEN_SETUP_PANE.base {
                0x99
            } else {
                0
            },
            ..Default::default()
        });
        assert_eq!(
            run.calls,
            [
                Gc::new(points::SCREEN_SETUP_PANE.base)
                    .ecx(SCREEN)
                    .args([0x55]),
                Gc::new(points::TEXT_SET.base)
                    .edi(SCREEN + 0x80)
                    .esi(SCREEN)
                    .args([0x20ff_f077]),
            ]
        );
        assert_eq!(run.result, Ok(Ret::new(0x99, 4)));
    }

    #[test]
    fn the_class_places_five_entries_and_one_table_page() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let class = ListClass::new("video-screen", video_screen::SCREEN, model());
        assert_eq!(class.loader(), 0);
        r.add(&class).unwrap();
        let plan = r.build(&feature::Base).unwrap();
        assert_eq!(plan.entries.len(), 5);
        assert_eq!(class.loader(), plan.entries[0].addr);
        let text = plan.text();
        for l in [
            "loader",
            "row text",
            "row chosen",
            "set up pane",
            "loader slot",
        ] {
            assert!(text.contains(&format!("entry video-screen:{l}")), "{text}");
        }
    }

    /// Rows with a description each, for the description tests.
    struct Described;
    impl Rows for Described {
        fn title(&self) -> u32 {
            0x77
        }
        fn count(&self) -> usize {
            15
        }
        fn texts(&self, row: i16) -> Vec<(u32, u32)> {
            if (0..15).contains(&row) {
                vec![(0, 0x100 + row as u32)]
            } else {
                Vec::new()
            }
        }
        fn press(&self, _row: i16) -> Chosen {
            Chosen::Nothing
        }
        fn description(&self, row: i16) -> Option<u32> {
            (0..15).contains(&row).then_some(0x200 + row as u32)
        }
        fn tint(&self, row: i16) -> Option<[f32; 3]> {
            (row == 3).then_some([0.5, 0.25, 1.0])
        }
    }

    #[test]
    fn a_text_block_is_laid_out_as_the_pane_builder_reads_it() {
        let b = TextBlock {
            rect: [200, 80, 140, 310],
            font: 1,
            colour: [0.5, 0.25, 1.0],
        }
        .bytes();
        assert_eq!(b.len(), 0x2c);
        assert_eq!(
            &b[0..4],
            &1u32.to_le_bytes(),
            "left justified, larger widget"
        );
        assert_eq!(&b[4..6], &[0, 0], "no animation");
        assert_eq!(&b[0xa..0xc], &1i16.to_le_bytes());
        assert_eq!(&b[0x10..0x14], &0.5f32.to_le_bytes());
        assert_eq!(&b[0x18..0x1c], &1.0f32.to_le_bytes());
        assert_eq!(&b[0x1c..0x1e], &200i16.to_le_bytes());
        assert_eq!(&b[0x22..0x24], &310i16.to_le_bytes());
        assert_eq!(
            &b[0x24..0x28],
            &[0; 4],
            "the null string until a row is focused"
        );
    }

    #[test]
    fn the_description_is_set_only_when_the_focus_moves() {
        let s = Shown::default();
        assert_eq!(s.focus(SCREEN, Some(0)), None, "no widget yet");
        s.set_widget(SCREEN, 0x9000);
        assert_eq!(s.focus(SCREEN, Some(0)), Some(0x9000));
        assert_eq!(s.focus(SCREEN, Some(0)), None);
        assert_eq!(s.focus(SCREEN, Some(3)), Some(0x9000));
        assert_eq!(s.focus(SCREEN, None), Some(0x9000));
        assert_eq!(s.focus(SCREEN + 0x1000, Some(3)), None, "another screen");
        // Set up again (a new screen at the same address): shows nothing.
        s.set_widget(SCREEN, 0x9100);
        assert_eq!(s.focus(SCREEN, Some(3)), Some(0x9100));
        for i in 0..20 {
            s.set_widget(0x10 * i, 1);
        }
        assert_eq!(s.lock().len(), SHOWN_KEPT);
    }

    /// The pane set-up with a description: retail set-up, title, the block
    /// written, then the pane builder's text step on it.
    #[test]
    fn the_pane_set_up_adds_the_description_widget() {
        let mut g = guest();
        g.put_u32(STACK + 4, 0x55);
        let block = TextBlock {
            rect: [1, 2, 3, 4],
            font: 1,
            colour: [1.0; 3],
        };
        let shown = Arc::new(Shown::default());
        let d = Description {
            at: TABLES + DESCRIPTION_BLOCK,
            block,
            shown: Arc::clone(&shown),
            picture: None,
            picture_at: 0,
        };
        let f =
            move |cx| -> EntryFuture { Box::pin(setup_pane(cx, 0x77, Some(d.clone()), "test")) };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: if c.target == points::PANE_TEXT_ADD.base {
                0x9000
            } else {
                0
            },
            ..Default::default()
        });
        assert_eq!(
            run.calls,
            [
                Gc::new(points::SCREEN_SETUP_PANE.base)
                    .ecx(SCREEN)
                    .args([0x55]),
                Gc::new(points::TEXT_SET.base)
                    .edi(SCREEN + 0x80)
                    .esi(SCREEN)
                    .args([0x77]),
                Gc::new(points::PANE_TEXT_ADD.base)
                    .ebx(SCREEN)
                    .args([0, TABLES + DESCRIPTION_BLOCK]),
            ]
        );
        assert_eq!(
            run.guest.bytes(TABLES + DESCRIPTION_BLOCK, 0x2c).unwrap(),
            block.bytes()
        );
        assert_eq!(shown.focus(SCREEN, Some(2)), Some(0x9000));
    }

    /// Row text on a class with a description: the row's texts, then the
    /// focused row; its description only when the focus moved.
    #[test]
    fn row_text_sets_the_focused_rows_description_once() {
        const ITEM: u32 = 0x0060_0000;
        let list = SCREEN + ui::LIST;
        let shown = Arc::new(Shown::default());
        shown.set_widget(SCREEN, 0x9000);
        let rows: Arc<dyn Rows> = Arc::new(Described);
        let draw = |focused: u32| {
            let mut g = guest();
            g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
            g.map(ITEM, Item::SIZE);
            Item::at(ITEM)
                .set_row(&mut g, Some(DatumHandle::new(0x8001, 9)))
                .unwrap();
            let f = {
                let (s, shown) = (Arc::clone(&rows), Arc::clone(&shown));
                move |cx| -> EntryFuture {
                    Box::pin(row_text(cx, Arc::clone(&s), Some(Arc::clone(&shown))))
                }
            };
            let regs = Regs {
                esp: STACK,
                ecx: list,
                ..Default::default()
            };
            testing::run(&f, g, regs, move |c, _| Regs {
                eax: match c.target {
                    t if t == points::ITEM_TEXT.base => 0x7000,
                    t if t == points::LIST_FOCUSED_ROW.base => focused,
                    _ => 0,
                },
                ..Default::default()
            })
        };
        let item_calls = [
            Gc::new(points::ITEM_TEXT.base).ecx(ITEM).args([6, 0, 0]),
            Gc::new(points::TEXT_SET.base)
                .edi(0x7000)
                .esi(ITEM)
                .args([0x109]),
            Gc::new(points::LIST_FOCUSED_ROW.base).eax(list),
        ];
        let run = draw(DatumHandle::new(0x8001, 3).raw());
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        let mut want = item_calls.to_vec();
        want.push(
            Gc::new(points::TEXT_SET.base)
                .edi(0x9000)
                .esi(SCREEN)
                .args([0x203]),
        );
        assert_eq!(run.calls, want);
        // The same focus on the next draw: no description call.
        assert_eq!(draw(DatumHandle::new(0x8001, 3).raw()).calls, item_calls);
        // Nothing focused: nothing set; focus back on row 3 sets it again.
        assert_eq!(draw(u32::MAX).calls, item_calls);
        assert_eq!(draw(DatumHandle::new(0x8001, 3).raw()).calls.len(), 4);
    }

    #[test]
    fn a_class_with_a_description_places_the_same_entries() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let class = ListClass::with_rows("described", "described", Arc::new(Described))
            .description(TextBlock {
                rect: [1, 2, 3, 4],
                font: 1,
                colour: [1.0; 3],
            });
        r.add(&class).unwrap();
        let plan = r.build(&feature::Base).unwrap();
        assert_eq!(plan.entries.len(), 5);
        assert_eq!(class.loader(), plan.entries[0].addr);
    }

    const ENTRIES: u32 = 0x0080_0000;
    const TAG: u32 = 0x0081_0000;
    const PANES: u32 = 0x0082_0000;
    const BITMAPS: u32 = 0x0083_0000;

    fn spec() -> PictureSpec {
        PictureSpec {
            screen_id: 12,
            pane: 3,
            bitmap: 1,
            image: 3,
            at: [56, 150],
        }
    }

    /// A screen tag (datum 0x1234_0005) with four panes; pane 3 has two
    /// bitmap blocks, the second showing image 3 of a `bitm`.
    fn screen_tag() -> FakeGuest {
        let mut g = guest();
        g.map(ENTRIES, 0x100)
            .map(TAG, 0x100)
            .map(PANES, 0x200)
            .map(BITMAPS, 0x100)
            .map(points::TAG_ENTRIES.base, 4);
        g.put_u32(points::TAG_ENTRIES.base, ENTRIES)
            .put_u32(ENTRIES + 5 * 0x10 + 8, TAG)
            .put_u32(TAG + 0x20, 4)
            .put_u32(TAG + 0x24, PANES)
            .put_u32(PANES + 3 * 0x4c + 0x24, 2)
            .put_u32(PANES + 3 * 0x4c + 0x28, BITMAPS);
        let b = BITMAPS + 0x38;
        g.put_u32(b + 4, 11)
            .put_u16(b + 0xa, 3)
            .put_u16(b + 0xc, 44)
            .put_u16(b + 0xe, 124)
            .put_u32(b + 0x18, engine::tags::GroupTag::from_chars(*b"bitm").0)
            .put_u32(b + 0x1c, 0xe588_0368)
            .put_u16(b + 0x20, 2);
        g
    }

    /// The block is the retail one, placed at ours with no animation; one
    /// that is not the bitmap meant is refused, with why.
    #[test]
    fn the_picture_is_a_copy_of_the_retail_bitmap_block_placed_at_ours() {
        let g = screen_tag();
        let b = picture_block(&g, 0x1234_0005, &spec()).unwrap();
        let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let half = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        assert_eq!(word(4), 0, "no animation");
        assert_eq!((half(0xc), half(0xe)), (56, 150));
        assert_eq!(half(0xa), 3);
        assert_eq!(word(0x1c), 0xe588_0368, "the map's own bitmap");
        assert_eq!(half(0x20), 2, "the rest as the tag has it");
        assert!(
            picture_block(&g, u32::MAX, &spec())
                .unwrap_err()
                .contains("not in this")
        );
        let wrong = PictureSpec { image: 2, ..spec() };
        assert!(
            picture_block(&g, 0x1234_0005, &wrong)
                .unwrap_err()
                .contains("not the one")
        );
        let pane = PictureSpec { pane: 4, ..spec() };
        assert!(
            picture_block(&g, 0x1234_0005, &pane)
                .unwrap_err()
                .contains("4 panes")
        );
        let bitmap = PictureSpec {
            bitmap: 2,
            ..spec()
        };
        assert!(
            picture_block(&g, 0x1234_0005, &bitmap)
                .unwrap_err()
                .contains("2 bitmaps")
        );
    }

    /// The pane set-up with a picture: after the description, the screen's
    /// tag is looked up, the block written, then the bitmap step on it.
    #[test]
    fn the_pane_set_up_adds_the_picture_after_the_description() {
        let mut g = screen_tag();
        g.put_u32(STACK + 4, 0x55);
        let shown = Arc::new(Shown::default());
        let d = Description {
            at: TABLES + DESCRIPTION_BLOCK,
            block: TextBlock {
                rect: [1, 2, 3, 4],
                font: 1,
                colour: [1.0; 3],
            },
            shown: Arc::clone(&shown),
            picture: Some(spec()),
            picture_at: TABLES + PICTURE_BLOCK,
        };
        let f =
            move |cx| -> EntryFuture { Box::pin(setup_pane(cx, 0x77, Some(d.clone()), "test")) };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: match c.target {
                t if t == points::PANE_TEXT_ADD.base => 0x9000,
                t if t == points::SCREEN_TAG_BY_ID.base => 0x1234_0005,
                t if t == points::PANE_BITMAP_ADD.base => 0x9800,
                _ => 0,
            },
            ..Default::default()
        });
        assert_eq!(
            &run.calls[3..],
            [
                Gc::new(points::SCREEN_TAG_BY_ID.base).args([12]),
                Gc::new(points::PANE_BITMAP_ADD.base)
                    .esi(SCREEN)
                    .args([TABLES + PICTURE_BLOCK]),
            ]
        );
        let want = picture_block(&screen_tag(), 0x1234_0005, &spec()).unwrap();
        assert_eq!(run.guest.bytes(TABLES + PICTURE_BLOCK, 0x38).unwrap(), want);
        assert_eq!(shown.picture(SCREEN), Some(0x9800));
    }

    /// Row text tints the picture for the focused row when the focus moves.
    #[test]
    fn row_text_tints_the_picture_for_the_focused_row() {
        const ITEM: u32 = 0x0060_0000;
        const PICTURE: u32 = 0x0061_0000;
        let list = SCREEN + ui::LIST;
        let shown = Arc::new(Shown::default());
        shown.set_widget(SCREEN, 0x9000);
        shown.set_picture(SCREEN, PICTURE);
        let mut g = guest();
        g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
        g.map(ITEM, Item::SIZE).map(PICTURE, 0x90);
        Item::at(ITEM)
            .set_row(&mut g, Some(DatumHandle::new(0x8001, 3)))
            .unwrap();
        let f = {
            let shown = Arc::clone(&shown);
            move |cx| -> EntryFuture {
                Box::pin(row_text(cx, Arc::new(Described), Some(Arc::clone(&shown))))
            }
        };
        let regs = Regs {
            esp: STACK,
            ecx: list,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: match c.target {
                t if t == points::ITEM_TEXT.base => 0x7000,
                t if t == points::LIST_FOCUSED_ROW.base => DatumHandle::new(0x8001, 3).raw(),
                _ => 0,
            },
            ..Default::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        let tint: Vec<f32> = (0..3)
            .map(|i| f32::from_bits(run.guest.u32(PICTURE + WIDGET_TINT + 4 * i).unwrap()))
            .collect();
        assert_eq!(tint, [0.5, 0.25, 1.0]);
        let mut m = FakeGuest::new();
        m.map(PICTURE, 0x90);
        set_tint(&mut m, PICTURE, [1.0, 0.78, 0.3]).unwrap();
        assert_eq!(f32::from_bits(m.u32(PICTURE + 0x2c).unwrap()), 0.78);
    }
}
