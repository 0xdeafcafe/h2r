//! Our own list screen class (docs/feature-engine.md "UI step b"): a screen
//! of ours with no retail function patched for it.
//!
//! **What it is.** A Controls-shaped screen (the Controls tag's layout,
//! built by the Controls constructor) whose screen and list handler point
//! at our copies of the Controls vtables, with these entries of ours (async
//! entries at arena addresses, docs/ui.md "Screen and list classes"):
//!
//! - **loader**: what `0x149f1e` is handed. It does what the Controls loader
//!   does (allocate, construct, push), and between construct and push it
//!   adopts the screen: our vtables, our row chosen in the press record, and
//!   the rows trimmed to the source's.
//! - **row text** (list slot 20): the source's texts for the item's row
//!   (label and value), and, for a class with a description, the focused
//!   row's description when the focus moved.
//! - **row chosen** (the press record's function): the source's press.
//! - **set up pane** (screen slot 18): the retail one, then our title on the
//!   header text widget and, for a class with a description, its text widget
//!   (made by the pane builder's own text step from a block of ours).
//! - **loader slot** (screen slot 26): our loader, so the screen manager
//!   re-opens it as ours.
//!
//! What a screen shows comes from a [`Source`]: a `ui` model screen
//! ([`ModelSource`], the Video screen) or any other (the Skulls screen's,
//! per profile).
//!
//! Nothing here tells our screens apart from retail ones at run time: the
//! retail functions never see them. Rows: the Controls constructor builds
//! six (seven with the speaker row); while our loader constructs, the list
//! hook (`layout_glue`) builds at least [`building_rows`], so a class can
//! have more rows than the six item widgets, and the list scrolls as retail
//! lists with more rows than items do. Adopting trims the array to the
//! source's count.

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

/// What one of our list screens shows. Every method may take the settings
/// store's lock: none is called with guest memory held.
pub trait Source: Send + Sync {
    /// How many rows the screen has (fixed for the class).
    fn rows(&self) -> usize;
    /// The title's string id.
    fn title(&self) -> u32;
    /// `(text slot, string id)` for each text of row `row`; empty for no
    /// such row.
    fn texts(&self, row: i16) -> Vec<(u32, u32)>;
    /// A press on row `row`: what to log, if anything.
    fn press(&self, row: i16) -> Option<String>;
    /// The description's string id for row `row`, for a class with a
    /// description text.
    fn description(&self, _row: i16) -> Option<u32> {
        None
    }
}

/// A `ui` model screen as a [`Source`].
pub struct ModelSource {
    pub model: Arc<Model>,
    pub screen: &'static str,
}

impl ModelSource {
    fn list(&self) -> List {
        List::Screen(self.screen)
    }
}

impl Source for ModelSource {
    fn rows(&self) -> usize {
        self.model.rows(self.list()).len()
    }
    fn title(&self) -> u32 {
        self.model.title_id(self.screen).unwrap_or(0)
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        self.model
            .row(self.list(), row)
            .map(|r| self.model.texts(r).to_vec())
            .unwrap_or_default()
    }
    fn press(&self, row: i16) -> Option<String> {
        match self
            .model
            .row(self.list(), row)
            .map(|r| self.model.press(r))
        {
            Some(Chosen::Stepped(msg)) => Some(msg),
            Some(Chosen::Open(id)) => {
                Some(format!("ui: {id} is not opened from a screen of ours yet"))
            }
            Some(Chosen::Nothing) | None => None,
        }
    }
}

/// A description text of ours on the screen: a text block definition
/// (docs/ui.md "Text block") the pane builder's text step makes a widget
/// from. Its string starts empty; row text sets the focused row's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBlock {
    /// Top, left, bottom, right (Y up, origin at the screen centre).
    pub rect: [i16; 4],
    pub font: i16,
    /// Red, green, blue.
    pub colour: [f32; 3],
}

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

/// A text block definition's size.
pub const TEXT_BLOCK_SIZE: u32 = 0x2c;

/// The description widget of each open screen of a class and the row it
/// shows, so row text sets it only when the focus moved.
#[derive(Default)]
pub struct Shown(Mutex<Vec<ShownEntry>>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShownEntry {
    screen: u32,
    widget: u32,
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
            row: None,
        });
        if v.len() > SHOWN_KEPT {
            v.remove(0);
        }
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

/// Where the description's text block goes in the class's table page.
const DESCRIPTION_BLOCK: u32 = 0x100;

/// A screen of ours as our own list class. See the module docs.
pub struct ListClass {
    /// The feature id (`features.<id>` switches it).
    pub id: &'static str,
    source: Arc<dyn Source>,
    description: Option<TextBlock>,
    loader: Arc<AtomicU32>,
}

impl ListClass {
    /// The `ui` model screen `screen` as a list class.
    pub fn new(id: &'static str, screen: &'static str, model: Arc<Model>) -> Self {
        Self::with_source(id, Arc::new(ModelSource { model, screen }))
    }
    /// A list class showing `source`.
    pub fn with_source(id: &'static str, source: Arc<dyn Source>) -> Self {
        ListClass {
            id,
            source,
            description: None,
            loader: Arc::new(AtomicU32::new(0)),
        }
    }
    /// With a description text: the focused row's
    /// [`Source::description`].
    pub fn description(mut self, block: TextBlock) -> Self {
        self.description = Some(block);
        self
    }
    /// The loader address to open it with (`0x149f1e(mask, loader)`), once
    /// built; 0 before.
    pub fn loader(&self) -> u32 {
        self.loader.load(Ordering::SeqCst)
    }
}

/// The rows our loader is constructing a list for (0: none). The list hook
/// builds the Controls list with at least this many.
static BUILDING: AtomicU32 = AtomicU32::new(0);

/// The row count the Controls list our loader is constructing needs, or 0
/// when no loader of ours is constructing one.
pub fn building_rows() -> u32 {
    BUILDING.load(Ordering::SeqCst)
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
/// rows trimmed to `rows` (as many as the array has, at most). The loaded
/// byte is set as the Controls loader sets it.
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
pub async fn load(cx: Cx, e: Entries, rows: usize, name: &'static str) -> Result<Ret, EntryError> {
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
    BUILDING.store(rows as u32, Ordering::SeqCst);
    let built = cx
        .call(
            GuestCall::new(points::CONTROLS_SCREEN_CTOR.base)
                .edi(screen)
                .args([a, b, player]),
        )
        .await;
    BUILDING.store(0, Ordering::SeqCst);
    built?;
    match cx.write(|m| adopt(m, screen, &e, rows))? {
        Adopted::Ours { rows: n } if n < rows => eprintln!(
            "{name}: opened with {n} rows of {rows} (our list class; the list was built with {n})"
        ),
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

/// Sets text widget `text` (a child of `parent`) to string `id`.
async fn set_text(cx: &Cx, text: u32, parent: u32, id: u32) -> Result<(), EntryError> {
    cx.call(
        GuestCall::new(points::TEXT_SET.base)
            .edi(text)
            .esi(parent)
            .args([id]),
    )
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
    source: Arc<dyn Source>,
    shown: Option<Arc<Shown>>,
) -> Result<Ret, EntryError> {
    let item = cx.arg(0)?;
    let row = cx.read(|m| item_row(m, item))?;
    // The source reads the settings store; no guest memory is held here.
    let texts = row.map(|r| source.texts(r)).unwrap_or_default();
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
            set_text(&cx, text, item, id).await?;
        }
    }
    if let Some(shown) = shown {
        let list = cx.entry().ecx;
        let screen = list.wrapping_sub(ui::LIST);
        let focused = cx
            .call(GuestCall::new(points::LIST_FOCUSED_ROW.base).eax(list))
            .await?
            .eax;
        let focused = (focused != u32::MAX).then(|| DatumHandle(focused).index() as i16);
        if let Some(widget) = shown.focus(screen, focused)
            && let Some(id) = focused.and_then(|r| source.description(r))
        {
            set_text(&cx, widget, screen, id).await?;
        }
    }
    Ok(Ret::new(0, 8))
}

/// Row chosen (the press record's function): thiscall, the player chain and
/// a pointer to the row, `ret 8`. The source's press; nothing is called.
pub async fn row_chosen(cx: Cx, source: Arc<dyn Source>) -> Result<Ret, EntryError> {
    let at = cx.arg(1)?;
    let row = cx.read(|m| m.u16(at))? as i16;
    // The store is written with no guest memory held.
    if let Some(msg) = source.press(row) {
        eprintln!("{msg}");
    }
    Ok(Ret::new(0, 8))
}

/// Where a class's description block goes, the block, and where to keep
/// each screen's widget.
#[derive(Clone)]
pub struct Description {
    pub at: u32,
    pub block: TextBlock,
    pub shown: Arc<Shown>,
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
    set_text(&cx, Screen::at(screen).header(), screen, title).await?;
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
        }
    }
    Ok(Ret::new(r.eax, 4))
}

impl feature::Feature for ListClass {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(self.id, "A screen of ours as our own list class")
            .doc("docs/feature-engine.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let rows = self.source.rows();
        let title = self.source.title();
        let tables = b.guest_alloc(0x1000, Permissions::RW)?;
        let name = self.id;
        let shown = self.description.map(|_| Arc::new(Shown::default()));
        // The loader needs every address, so the entries are made first and
        // the loader reads them from a cell filled at the end.
        let cell = Arc::new(std::sync::OnceLock::<Entries>::new());
        let loader = {
            let cell = Arc::clone(&cell);
            b.entry("loader", move |cx| -> EntryFuture {
                let e = *cell.get().expect("entries are set at build");
                Box::pin(load(cx, e, rows, name))
            })?
        };
        let row_text_at = {
            let (source, shown) = (Arc::clone(&self.source), shown.clone());
            let seen = Arc::new(AtomicBool::new(false));
            b.entry("row text", move |cx| -> EntryFuture {
                once(&seen, &format!("{name}: row text"));
                Box::pin(row_text(cx, Arc::clone(&source), shown.clone()))
            })?
        };
        let row_chosen_at = {
            let source = Arc::clone(&self.source);
            b.entry("row chosen", move |cx| -> EntryFuture {
                Box::pin(row_chosen(cx, Arc::clone(&source)))
            })?
        };
        let description = self
            .description
            .zip(shown)
            .map(|(block, shown)| Description {
                at: tables + DESCRIPTION_BLOCK,
                block,
                shown,
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

    fn model() -> Arc<Model> {
        Arc::new(video_screen::model(Arc::new(Store::defaults(ALL)), Switches::ALL).unwrap())
    }

    fn video(m: &Arc<Model>) -> Arc<dyn Source> {
        Arc::new(ModelSource {
            model: Arc::clone(m),
            screen: video_screen::SCREEN,
        })
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

    /// More rows than the six item widgets stay: the list scrolls, as the
    /// retail difficulty list does with our fifth row on four items. The
    /// array has what the list hook built, at most.
    #[test]
    fn more_rows_than_item_widgets_are_kept_up_to_the_array() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 15);
        assert_eq!(
            adopt(&mut g, SCREEN, &entries(), 15),
            Ok(Adopted::Ours { rows: 15 })
        );
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 7);
        assert_eq!(
            adopt(&mut g, SCREEN, &entries(), 9),
            Ok(Adopted::Ours { rows: 7 }),
            "the list was built with seven"
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

    /// The loader against fake callees: allocate, construct (the fake builds
    /// what the Controls constructor builds), adopt, push.
    #[test]
    fn the_loader_allocates_constructs_adopts_and_pushes() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        g.put_u16(REQUEST + 2, 0x4)
            .put_u32(REQUEST + 4, 5)
            .put_u32(REQUEST + 8, 4);
        g.put_u32(STACK, 0xdead).put_u32(STACK + 4, REQUEST);
        let e = entries();
        let f = move |cx| -> EntryFuture { Box::pin(load(cx, e, 4, "test")) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let mut building = Vec::new();
        let run = testing::run(&f, g, regs, |c, m| {
            building.push(building_rows());
            if c.target == points::CONTROLS_SCREEN_CTOR.base {
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
        // The list hook is told the rows while the constructor runs only.
        assert_eq!(building, [0, 4, 0]);
        assert_eq!(building_rows(), 0);
    }

    #[test]
    fn a_failed_allocation_opens_nothing() {
        let mut g = guest();
        g.put_u32(STACK + 4, REQUEST);
        let f = move |cx| -> EntryFuture { Box::pin(load(cx, entries(), 4, "test")) };
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
            let s = video(&m);
            move |cx| -> EntryFuture { Box::pin(row_text(cx, Arc::clone(&s), None)) }
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
            let s = video(&m);
            move |cx| -> EntryFuture { Box::pin(row_chosen(cx, Arc::clone(&s))) }
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

    /// A source with a description per row, for the description tests.
    struct Described;
    impl Source for Described {
        fn rows(&self) -> usize {
            15
        }
        fn title(&self) -> u32 {
            0x77
        }
        fn texts(&self, row: i16) -> Vec<(u32, u32)> {
            if (0..15).contains(&row) {
                vec![(0, 0x100 + row as u32)]
            } else {
                Vec::new()
            }
        }
        fn press(&self, row: i16) -> Option<String> {
            Some(format!("pressed {row}"))
        }
        fn description(&self, row: i16) -> Option<u32> {
            (0..15).contains(&row).then_some(0x200 + row as u32)
        }
    }

    #[test]
    fn a_model_source_is_the_model_screen() {
        let m = model();
        let s = video(&m);
        let list = video_screen::LIST;
        assert_eq!(s.rows(), m.rows(list).len());
        assert_eq!(s.title(), m.title_id(video_screen::SCREEN).unwrap());
        assert_eq!(s.texts(1), m.texts(m.row(list, 1).unwrap()).to_vec());
        assert!(s.texts(9).is_empty());
        assert_eq!(s.description(1), None);
        assert_eq!(s.press(1).as_deref(), Some("graphics.fps: 60"));
        assert_eq!(s.press(9), None);
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
        let source: Arc<dyn Source> = Arc::new(Described);
        let draw = |focused: u32| {
            let mut g = guest();
            g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
            g.map(ITEM, Item::SIZE);
            Item::at(ITEM)
                .set_row(&mut g, Some(DatumHandle::new(0x8001, 9)))
                .unwrap();
            let f = {
                let (s, shown) = (Arc::clone(&source), Arc::clone(&shown));
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
    fn row_chosen_logs_the_sources_press() {
        let mut g = guest();
        g.put_u32(STACK + 8, 0x0070_0000).put_u16(0x0070_0000, 4);
        let f = |cx| -> EntryFuture { Box::pin(row_chosen(cx, Arc::new(Described))) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| unreachable!("no guest call"));
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
    }

    #[test]
    fn a_class_with_a_source_and_description_places_the_same_entries() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let class =
            ListClass::with_source("described", Arc::new(Described)).description(TextBlock {
                rect: [1, 2, 3, 4],
                font: 1,
                colour: [1.0; 3],
            });
        r.add(&class).unwrap();
        let plan = r.build(&feature::Base).unwrap();
        assert_eq!(plan.entries.len(), 5);
        assert_eq!(class.loader(), plan.entries[0].addr);
    }
}
