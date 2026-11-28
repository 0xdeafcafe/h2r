//! A screen declared rather than wired (docs/game/ui-screens.md "Adding a
//! screen"): rows of four kinds, the legend from named glyphs, its texts
//! served from a string-id family of the registry, B back, A on the row,
//! left and right on value rows, the opener's press dropped, and closing
//! with work that runs once the screen is freed.
//!
//! ```ignore
//! const IDS: Family = ids::family("kit.example");
//! // Title, legend, layout and labels: content/ui/screens/kit.example.yaml.
//! let s = KitScreen::new("kit-example", "kit.example", IDS)
//!     .row(Row::value("Grunts", &["On", "Off"], get, set))
//!     .row(Row::info("Status", status))
//!     .row(Row::action("Start", || Then::close_then(start)));
//! let s = Arc::new(s);
//! registry.add(&KitTexts(Arc::clone(&s)))?;
//! registry.add(&s.class())?;
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::memory::{GuestMemory, MemFault};
use runtime::guest::Permissions;

use crate::list_class::{
    Action, Button, ListClass, ListLayout, LiveText, OnFree, Panel, PanelText, Rows, TextBlock,
};
use crate::ui::{Chosen, LABEL_SLOT, VALUE_SLOT};
use crate::ui_kit::animation::TransitionStyle;
use crate::ui_kit::dialog::{DialogBase, DialogSpec};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::{Glyph, Input, OpenerGuard, Prompt};
use crate::ui_kit::lifecycle::AfterClose;
use crate::ui_kit::spec::{self, Describe, ListPlace, PaneText, RowSpec, ScreenSpec, ValueStyle};
use crate::ui_kit::view::Opener;

/// Work to run once a closed screen is freed: async, may call the guest.
pub type After =
    Arc<dyn Fn(Cx) -> Pin<Box<dyn Future<Output = Result<(), EntryError>> + Send>> + Send + Sync>;

/// What an action row's press does.
pub enum Then {
    /// Stay; log the line.
    Stay(String),
    /// Open the named screen of ours on top.
    Open(&'static str),
    /// Close this screen.
    Close,
    /// Close this screen; once it is freed, run the work (on the screen
    /// underneath, never on this one).
    CloseThen(After),
}

impl Then {
    pub fn close_then(f: After) -> Self {
        Then::CloseThen(f)
    }
}

type Get<T> = Arc<dyn Fn() -> T + Send + Sync>;

struct LiveHelp {
    get: Get<String>,
    drawn: Mutex<String>,
}

enum Kind {
    Action(Get<Then>),
    Value {
        /// The offered values, made as the row is drawn or pressed (the
        /// wave sets a lobby's map offers, the choices a setting has).
        values: Get<Vec<String>>,
        get: Get<usize>,
        set: Arc<dyn Fn(usize) -> String + Send + Sync>,
    },
    Info(Get<(String, bool)>),
    /// A opens the picker over a list made as A is pressed.
    Pick {
        shown: Get<String>,
        offer: Get<Result<crate::ui_kit::picker::Pick, String>>,
    },
    Text {
        get: Get<String>,
        max_chars: u16,
        typed: Typed,
    },
    /// A pick row whose label, value and help its binding gives
    /// ([`Row::slot`]); not drawn while it gives none.
    Slot {
        fill: Get<Option<Filled>>,
        offer: Get<Result<crate::ui_kit::picker::Pick, String>>,
    },
    /// An action row whose label and help its binding gives
    /// ([`Row::card`]); not drawn while it gives none.
    Card {
        fill: Get<Option<Filled>>,
        press: Get<Then>,
    },
}

type Typed = Arc<dyn Fn(Option<&str>) -> String + Send + Sync>;

/// What a slot row shows ([`Row::slot`]): its label, the value beside
/// it, and its help under the rows (none: the help block is empty).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filled {
    pub label: String,
    pub shown: String,
    pub help: Option<String>,
}

/// One row: a label, its kind, its description.
pub struct Row {
    label: String,
    kind: Kind,
    description: Option<String>,
    /// What the value picker says (a value row), else its description.
    explain: Option<String>,
    /// The screen of ours A pushes from a value row in place of the value
    /// picker ([`Row::opens`]).
    opens: Option<&'static str>,
}

impl Row {
    /// A runs `press`.
    pub fn action(label: &str, press: impl Fn() -> Then + Send + Sync + 'static) -> Self {
        Self::of(label, Kind::Action(Arc::new(press)))
    }
    /// A opens the value picker on `values`. Left and right can step through
    /// them. `get` is the current index and `set` stores one.
    pub fn value(
        label: &str,
        values: &[&str],
        get: impl Fn() -> usize + Send + Sync + 'static,
        set: impl Fn(usize) -> String + Send + Sync + 'static,
    ) -> Self {
        let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
        Self::menu(label, Arc::new(move || values.clone()), get, set)
    }
    /// [`Row::value`] whose offered values are made as the row is drawn or
    /// pressed (the wave sets a lobby's map offers): `values` is the list
    /// now. Left and right step through it as they come.
    pub fn menu(
        label: &str,
        values: Get<Vec<String>>,
        get: impl Fn() -> usize + Send + Sync + 'static,
        set: impl Fn(usize) -> String + Send + Sync + 'static,
    ) -> Self {
        Self::of(
            label,
            Kind::Value {
                values,
                get: Arc::new(get),
                set: Arc::new(set),
            },
        )
    }
    /// Read-only: `value` beside the label.
    pub fn info(label: &str, value: impl Fn() -> String + Send + Sync + 'static) -> Self {
        Self::info_from_source(label, move || (value(), true))
    }
    /// A read-only value and whether its source supplied one in this state.
    /// Its fallback remains visible while the content check records that
    /// the source has no value for this row.
    pub fn info_from_source(
        label: &str,
        value: impl Fn() -> (String, bool) + Send + Sync + 'static,
    ) -> Self {
        Self::of(label, Kind::Info(Arc::new(value)))
    }
    /// A opens the value picker on a list made when A is pressed (the
    /// maps installed now, the drafts saved now): `shown` is the value
    /// beside the label, and `offer` the pick, or why there is none (the
    /// reason is the press's log line). Left and right do nothing.
    pub fn pick(
        label: &str,
        shown: impl Fn() -> String + Send + Sync + 'static,
        offer: impl Fn() -> Result<crate::ui_kit::picker::Pick, String> + Send + Sync + 'static,
    ) -> Self {
        Self::of(
            label,
            Kind::Pick {
                shown: Arc::new(shown),
                offer: Arc::new(offer),
            },
        )
    }
    /// A opens the game's keyboard; `typed` gets the text (`None`: backed
    /// out) and says what to log.
    pub fn text(
        label: &str,
        max_chars: u16,
        get: impl Fn() -> String + Send + Sync + 'static,
        typed: impl Fn(Option<&str>) -> String + Send + Sync + 'static,
    ) -> Self {
        Self::of(
            label,
            Kind::Text {
                get: Arc::new(get),
                max_chars,
                typed: Arc::new(typed),
            },
        )
    }
    /// A pick row filled by its binding as the page shows it: `fill` is
    /// its label, the value beside it and its help, or `None` for a slot
    /// with nothing in it, which is not drawn (the rows after it move
    /// up). A opens the value picker on `offer`, titled by the label and
    /// saying the help. Left and right do nothing.
    pub fn slot(
        fill: impl Fn() -> Option<Filled> + Send + Sync + 'static,
        offer: impl Fn() -> Result<crate::ui_kit::picker::Pick, String> + Send + Sync + 'static,
    ) -> Self {
        Self::of(
            "",
            Kind::Slot {
                fill: Arc::new(fill),
                offer: Arc::new(offer),
            },
        )
    }
    /// A card of a carousel (`crate::ui_kit::card_screen`): `fill` is its
    /// name and help as the screen shows it, or `None` for a slot with no
    /// card, which is not drawn. A runs `press`.
    pub fn card(
        fill: impl Fn() -> Option<Filled> + Send + Sync + 'static,
        press: impl Fn() -> Then + Send + Sync + 'static,
    ) -> Self {
        Self::of(
            "",
            Kind::Card {
                fill: Arc::new(fill),
                press: Arc::new(press),
            },
        )
    }
    /// On a value row: A pushes `screen` (a screen of ours that picks the
    /// value its own way, a card carousel) in place of the value picker.
    /// Left and right still step the values.
    pub fn opens(mut self, screen: &'static str) -> Self {
        self.opens = Some(screen);
        self
    }
    pub fn describe(mut self, text: &str) -> Self {
        self.description = Some(text.into());
        self
    }
    /// What the value picker A opens says about a value row: what it does
    /// and what its values mean. The description when not given.
    pub fn explain(mut self, text: &str) -> Self {
        self.explain = Some(text.into());
        self
    }
    fn of(label: &str, kind: Kind) -> Self {
        Row {
            label: label.into(),
            kind,
            description: None,
            explain: None,
            opens: None,
        }
    }
    fn spec(&self) -> RowSpec {
        self.spec_with_source().0
    }

    /// A slot row's fill now; `None` for any other row.
    fn filled(&self) -> Option<Option<Filled>> {
        match &self.kind {
            Kind::Slot { fill, .. } | Kind::Card { fill, .. } => Some(fill()),
            _ => None,
        }
    }

    /// Whether the row is drawn: every row but a slot with nothing in it.
    fn drawn(&self) -> bool {
        self.filled().is_none_or(|f| f.is_some())
    }

    fn spec_with_source(&self) -> (RowSpec, bool) {
        let (row, loaded) = match &self.kind {
            Kind::Action(_) => (RowSpec::action(&self.label), true),
            Kind::Value { values, get, .. } => {
                let values = (values)();
                let choices: Vec<&str> = values.iter().map(String::as_str).collect();
                let row = RowSpec::value(&self.label, &choices, get());
                match self.opens {
                    Some(screen) => (row.press(spec::Press::Open(screen.into())), true),
                    None => (row, true),
                }
            }
            Kind::Info(value) => {
                let (text, loaded) = value();
                (RowSpec::info(&self.label, &text), loaded)
            }
            Kind::Pick { shown, .. } => {
                let text = shown();
                (
                    RowSpec::value(&self.label, &[text.as_str()], 0)
                        .press(spec::Press::Open(crate::ui_kit::picker::NAME.into())),
                    true,
                )
            }
            Kind::Text { get, max_chars, .. } => {
                (RowSpec::text(&self.label, &get(), *max_chars), true)
            }
            Kind::Slot { fill, .. } => {
                let f = fill().unwrap_or_default();
                let row = RowSpec::value(&f.label, &[f.shown.as_str()], 0)
                    .press(spec::Press::Open(crate::ui_kit::picker::NAME.into()));
                let row = match &f.help {
                    Some(help) => row.describe(help),
                    None => row,
                };
                return (row, true);
            }
            Kind::Card { fill, .. } => {
                let f = fill().unwrap_or_default();
                let row = RowSpec::action(&f.label);
                let row = match &f.help {
                    Some(help) => row.describe(help),
                    None => row,
                };
                return (row, true);
            }
        };
        let row = match &self.description {
            Some(description) => row.describe(description),
            None => row,
        };
        (row, loaded)
    }
}

/// What A on a row left for the class's action to do with the screen.
enum Pending {
    Close,
    CloseThen(After),
    Keyboard(usize),
}

/// Bytes per text slot (UTF-8 and its NUL).
pub const SLOT_LEN: u32 = 0x100;
/// The keyboard's UTF-16 buffer, after the slots.
const KEYBOARD_UNITS: u32 = 0x100;
/// The game's keyboard opener, and its context with no check on Done
/// (`crate::signin_keyboard`).
const KEYBOARD_CONTEXT: u32 = crate::signin_keyboard::INVITE_CONTEXT;

/// Text slots in guest memory: the family's ids, one slot each, written
/// when they change.
pub struct SlotTexts {
    pub family: Family,
    pub base: AtomicU32,
    slots: Mutex<Vec<(Vec<u8>, bool)>>,
    /// With a pool, the slots' places in it: (offset, capacity) per slot,
    /// each taken when its text first needs the room ([`TextPool`]).
    pool: Option<Arc<TextPool>>,
    extents: Mutex<Vec<(u32, u32)>>,
}

/// One guest region that several [`SlotTexts`] share, each slot holding
/// only as much of it as its text needs (a capacity of 0x20, 0x40, 0x80
/// or [`SLOT_LEN`]; growing takes a new place and leaves the old one
/// unused). Screens that would each reserve `family.len * SLOT_LEN`
/// bytes for slots mostly short or unused take far less of the arena.
pub struct TextPool {
    pub base: AtomicU32,
    len: u32,
    next: Mutex<u32>,
}

impl TextPool {
    pub fn new(len: u32) -> Self {
        TextPool {
            base: AtomicU32::new(0),
            len,
            next: Mutex::new(0),
        }
    }
    pub fn len(&self) -> u32 {
        self.len
    }
    /// Whether the pool has no room at all.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn take(&self, cap: u32) -> Option<u32> {
        let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
        (*next + cap <= self.len).then(|| {
            let at = *next;
            *next += cap;
            at
        })
    }
}

impl SlotTexts {
    pub fn new(family: Family) -> Self {
        SlotTexts {
            family,
            base: AtomicU32::new(0),
            slots: Mutex::new(Vec::new()),
            pool: None,
            extents: Mutex::new(Vec::new()),
        }
    }
    /// Slots placed in `pool` instead of a region of their own.
    pub fn pooled(family: Family, pool: Arc<TextPool>) -> Self {
        SlotTexts {
            pool: Some(pool),
            ..SlotTexts::new(family)
        }
    }
    /// Room in the pool for slot `slot`'s `need` bytes: its place, grown
    /// when the text outgrew it. `None` when the pool is full and the
    /// slot has none.
    fn extent(&self, slot: usize, need: u32) -> Option<(u32, u32)> {
        let pool = self.pool.as_ref()?;
        let mut ex = self.extents.lock().unwrap_or_else(|e| e.into_inner());
        if ex.len() <= slot {
            ex.resize(self.family.len as usize, (0, 0));
        }
        let (off, cap) = ex[slot];
        if cap >= need {
            return Some((off, cap));
        }
        let want = need.next_power_of_two().clamp(0x20, SLOT_LEN);
        if let Some(at) = pool.take(want) {
            ex[slot] = (at, want);
        } else if cap == 0 {
            return None;
        }
        Some(ex[slot])
    }
    pub fn set(&self, slot: u32, text: &str) -> u32 {
        let id = self.family.id(slot);
        let mut cut = text.len().min(SLOT_LEN as usize - 1);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut b = text.as_bytes()[..cut].to_vec();
        b.push(0);
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.len() < self.family.len as usize {
            slots.resize(self.family.len as usize, (vec![0], false));
        }
        if self.pool.is_some() && slots[slot as usize].0 != b {
            match self.extent(slot as usize, b.len() as u32) {
                Some((_, cap)) if (b.len() as u32) > cap => {
                    b.truncate(cap as usize - 1);
                    b.push(0);
                }
                Some(_) => {}
                None => return id,
            }
        }
        let s = &mut slots[slot as usize];
        if s.0 != b {
            *s = (b, true);
        }
        id
    }
    pub fn address(&self, id: u32) -> Option<u32> {
        if let Some(pool) = &self.pool {
            let base = pool.base.load(Ordering::SeqCst);
            if base == 0 || !self.family.owns(id) {
                return None;
            }
            let slot = (id - self.family.base) as usize;
            let known = self.extents.lock().unwrap_or_else(|e| e.into_inner());
            let (off, cap) = known.get(slot).copied().unwrap_or((0, 0));
            drop(known);
            // A slot never set holds an empty text in the first bytes of
            // its own place.
            let (off, _) = if cap == 0 { self.extent(slot, 1)? } else { (off, cap) };
            return Some(base + off);
        }
        let base = self.base.load(Ordering::SeqCst);
        (base != 0 && self.family.owns(id)).then(|| base + (id - self.family.base) * SLOT_LEN)
    }
    fn keyboard(&self) -> Option<u32> {
        if self.pool.is_some() {
            return None;
        }
        let base = self.base.load(Ordering::SeqCst);
        (base != 0).then(|| base + self.family.len * SLOT_LEN)
    }
    /// Every slot written again at the next [`Self::write`]: after a
    /// snapshot load the page holds the saved run's texts, which the
    /// cache can't tell from its own.
    pub fn rewrite_all(&self) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for (_, dirty) in slots.iter_mut() {
            *dirty = true;
        }
    }
    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        let base = match &self.pool {
            Some(pool) => pool.base.load(Ordering::SeqCst),
            None => self.base.load(Ordering::SeqCst),
        };
        if base == 0 {
            return Ok(());
        }
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for (i, (b, dirty)) in slots.iter_mut().enumerate() {
            if *dirty {
                let at = if self.pool.is_some() {
                    let ex = self.extents.lock().unwrap_or_else(|e| e.into_inner());
                    match ex.get(i) {
                        Some(&(off, cap)) if cap != 0 => base + off,
                        _ => continue,
                    }
                } else {
                    base + i as u32 * SLOT_LEN
                };
                m.write(at, b)?;
                *dirty = false;
            }
        }
        Ok(())
    }
}

/// A settings dialog's header (docs/game/ui-builder.md "The settings
/// dialog"): its icon and its lines, from its doc's `settings:` block,
/// the same in every state. Nothing row-indexed reaches it: the icon is
/// [`Rows::picture_image`], asked once as the screen opens, and the lines
/// are panel texts ([`Rows::panel_text`], which takes no row).
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// The icon of `mp_games2.bitm` in the base's picture
    /// ([`crate::ui_kit::dialog::icon`]); the menu host only.
    pub icon: Option<u16>,
    /// Its lines in order: the name (`header.name`), when the host has
    /// one, then what the window is for (`header.about`).
    pub lines: Vec<HeaderLine>,
}

/// One line of a [`Header`]: its widget id, its text and its block (the
/// `design.yaml` component's place, font and colour).
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderLine {
    pub id: &'static str,
    pub text: String,
    pub block: TextBlock,
}

/// A button besides the rows' ([`KitScreen::extra`]): its input, and what
/// it does, as a line to log.
struct Extra {
    input: Input,
    press: Get<String>,
}

/// A declared screen. See the module docs.
pub struct KitScreen {
    feature: &'static str,
    name: &'static str,
    title: String,
    /// The text of a row elsewhere that opens it ([`KitScreen::row_label`]).
    row_label: Option<String>,
    /// What that row says it does ([`KitScreen::row_help`]).
    row_help: Option<String>,
    rows: Vec<Row>,
    layout: Option<ListLayout>,
    animation: TransitionStyle,
    style: ValueStyle,
    description: Option<TextBlock>,
    live_help: Option<LiveHelp>,
    prompts: Vec<Prompt>,
    scrolls: bool,
    texts: SlotTexts,
    pending: Mutex<Option<Pending>>,
    after: AfterClose<After>,
    typing: Mutex<Option<usize>>,
    guard: OpenerGuard,
    /// Shown as a dialog of rows on a retail dialog, over its opener.
    dialog: Option<(DialogBase, Opener)>,
    /// Its doc's fixed rows (`row.N`): each row's label and description,
    /// by place ([`KitScreen::row`]).
    doc_rows: Vec<Option<(String, Option<String>)>>,
    /// A settings dialog's static header ([`Header`]).
    header: Option<Header>,
    /// The header's name line from a binding ([`KitScreen::header_name`]).
    header_name: Option<Get<String>>,
    /// The slot rows' fills last drawn: a change redraws the help.
    slots_drawn: Mutex<Vec<Option<Filled>>>,
    /// Buttons besides the rows' ([`KitScreen::extra`]).
    extras: Vec<Extra>,
    /// [`KitTexts`]' feature id, made once.
    texts_id: OnceLock<&'static str>,
}

impl KitScreen {
    /// `feature` switches it (`features.<id>`), `name` is its screen in
    /// [`crate::ui_kit::screens::NAMES`] and what `Open` rows name, `ids`
    /// its family in [`crate::ui_kit::ids::FAMILIES`]. Its presentation is
    /// its doc's, `content/ui/screens/<name>.yaml` ([`KitScreen::from_doc`]).
    /// Panics with the doc's named error when the screen has none or its
    /// doc can't be shown: the tests load every doc first.
    pub fn new(feature: &'static str, name: &'static str, ids: Family) -> Self {
        let doc = crate::ui_kit::data::require(name).unwrap_or_else(|e| panic!("{e}"));
        Self::from_doc(feature, name, ids, &doc).unwrap_or_else(|e| panic!("{e}"))
    }

    /// The screen as doc `doc` presents it (docs/game/ui-builder.md
    /// "Screens in YAML"): its title, its legend, its kind and layout (the
    /// Controls list, the level select's, or a dialog on Quick Options or
    /// the large dialog over a named opener), its description block (a
    /// dialog's is its base's help line unless the doc places it
    /// elsewhere), whether it scrolls, and its fixed rows' labels and
    /// descriptions. The rows, what they do and any live text stay the
    /// caller's.
    pub fn from_doc(
        feature: &'static str,
        name: &'static str,
        ids: Family,
        doc: &crate::ui_kit::data::ScreenDoc,
    ) -> Result<Self, crate::ui_kit::data::DataError> {
        use crate::ui_kit::data::{self, DataError};
        let file = format!("screens/{}.yaml", doc.screen);
        let bad = |at: &str, what: &str| DataError::new(&file, at, what);
        let mut s = Self::bare(feature, name, ids, &doc.text("title", &[])?);
        if doc.widget("legend").is_some() {
            s.prompts = doc.prompts("legend")?;
        }
        s.scrolls = doc.scrolls;
        let layout = doc.layout.as_deref().unwrap_or("");
        let level_select = (doc.kind.as_str(), layout) == ("list", "level_select");
        match (doc.kind.as_str(), layout) {
            ("list", "controls") => {}
            ("list", "level_select") => {
                s = s.layout(crate::skulls_menu::LIST_LAYOUT, ValueStyle::Inline);
            }
            ("dialog", "settings" | "large") => {
                let base = if layout == "settings" {
                    DialogBase::QuickOptions
                } else {
                    DialogBase::Large
                };
                let opener = match (doc.backing.as_deref(), doc.opener.as_deref()) {
                    (Some("over"), Some(o)) => data::opener_named(o)
                        .ok_or_else(|| bad("opener", &format!("no opener {o:?}")))?,
                    _ => return Err(bad("backing", "a kit dialog is over an opener (backing: over)")),
                };
                s = s.dialog(base, opener);
            }
            (k, l) => {
                return Err(bad(
                    "layout",
                    &format!("a kit screen is a list (controls, level_select) or a dialog (settings, large), not {k} {l}"),
                ));
            }
        }
        if let Some(w) = doc.widget("description")
            && let Some(c) = &w.component
        {
            let design = data::active_design();
            let preset = design.component(c)?;
            let (font, colour) = design.text_style(c)?;
            let [x, y] = w
                .at
                .or(preset.at)
                .ok_or_else(|| bad("widgets.description.at", "the description has a place"))?;
            let [width, height] = w
                .size
                .or(preset.size)
                .ok_or_else(|| bad("widgets.description.size", "the description has a size"))?;
            let block = TextBlock {
                rect: [y, x, y - height as i16, x + width as i16],
                font,
                colour,
                // The level select's description fades in with its list.
                animation: if level_select {
                    crate::skulls_menu::DESCRIPTION_TEXT.animation
                } else {
                    0
                },
            };
            let rgb = crate::ui_kit::components::rgb;
            let base_help = s.dialog.and_then(|(b, _)| b.help());
            let is_base_help = base_help.is_some_and(|h| {
                h.rect == block.rect && h.font == block.font && rgb(h.colour) == rgb(block.colour)
            });
            if !is_base_help {
                s.description = Some(block);
            }
        }
        for (id, r) in &doc.rows {
            let Some(n) = id.strip_prefix("row.").and_then(|n| n.parse::<usize>().ok()) else {
                continue;
            };
            if s.doc_rows.len() <= n {
                s.doc_rows.resize(n + 1, None);
            }
            // A lone slot (`{label}`) is the row's binding's to fill.
            if data::lone_slot(&r.text) {
                continue;
            }
            s.doc_rows[n] = Some((r.text.clone(), r.help.clone()));
        }
        if let Some(st) = &doc.settings {
            s.settings_from(doc, st)?;
        }
        Ok(s)
    }

    /// A settings dialog's parts from its doc's `settings:` block
    /// (docs/game/ui-builder.md "The settings dialog"): the header, the
    /// help block under the rows, the legend derived from its rows, and
    /// the caller's row. The component places every one of them
    /// (`design.yaml` `components.settings.*`); the doc gives none.
    fn settings_from(
        &mut self,
        doc: &crate::ui_kit::data::ScreenDoc,
        st: &crate::ui_kit::data::SettingsDoc,
    ) -> Result<(), crate::ui_kit::data::DataError> {
        use crate::ui_kit::settings::block;
        let file = format!("screens/{}.yaml", doc.screen);
        if st.host != "menu" {
            return Err(crate::ui_kit::data::DataError::new(
                &file,
                "settings.host",
                format!("{:?}: a settings dialog of ours is on the menu host", st.host),
            ));
        }
        let animation = crate::ui_kit::dialog::QUICK_ANIMATION;
        let mut lines = Vec::new();
        if let Some(name) = &st.name {
            lines.push(HeaderLine {
                id: "header.name",
                text: name.clone(),
                block: block("settings.name", animation)?,
            });
        }
        lines.push(HeaderLine {
            id: "header.about",
            text: st.about().to_string(),
            block: block("settings.about", animation)?,
        });
        self.header = Some(Header {
            icon: st.icon.as_deref().and_then(crate::ui_kit::dialog::icon::named),
            lines,
        });
        self.description = Some(block("settings.help", animation)?);
        self.prompts = crate::ui_kit::settings::legend(doc);
        if let Some(c) = &st.caller {
            self.row_label = Some(c.label.clone());
            self.row_help = Some(c.help.clone());
        }
        Ok(())
    }

    pub(super) fn bare(feature: &'static str, name: &'static str, ids: Family, title: &str) -> Self {
        KitScreen {
            feature,
            name,
            title: title.into(),
            row_label: None,
            row_help: None,
            rows: Vec::new(),
            layout: None,
            animation: TransitionStyle::default(),
            style: ValueStyle::Column,
            description: None,
            live_help: None,
            prompts: Vec::new(),
            scrolls: false,
            texts: SlotTexts::new(ids),
            pending: Mutex::new(None),
            after: AfterClose::default(),
            typing: Mutex::new(None),
            guard: OpenerGuard::default(),
            dialog: None,
            doc_rows: Vec::new(),
            header: None,
            header_name: None,
            slots_drawn: Mutex::new(Vec::new()),
            extras: Vec::new(),
            texts_id: OnceLock::new(),
        }
    }
    /// The next row: its label (and description, when the doc gives one)
    /// the doc's when the doc has the row.
    pub fn row(mut self, mut row: Row) -> Self {
        if let Some(Some((label, help))) = self.doc_rows.get(self.rows.len()) {
            row.label = label.clone();
            if let Some(h) = help {
                row.description = Some(h.clone());
            }
        }
        self.rows.push(row);
        self
    }
    /// The list moved and reskinned; a one-text skin shows values inline.
    pub fn layout(mut self, layout: ListLayout, style: ValueStyle) -> Self {
        self.layout = Some(layout);
        self.style = style;
        self
    }
    /// Use a page transition other than the template's native default.
    pub fn animation(mut self, style: TransitionStyle) -> Self {
        self.animation = style;
        self
    }

    /// A dialog of rows (docs/game/ui-builder.md "The Dialog kind"): the
    /// rows in `base`'s list, on its panel and header, opened over the
    /// screen beneath on the dialogs' window and closed by B, as the
    /// game's own dialogs are. `opener` is what the preview draws under
    /// it. The list is the base's own unless [`KitScreen::layout`] moves
    /// it; [`DialogBase::Large`] is the one the game maps have too. Values
    /// go as the base's list skin shows them ([`DialogBase::style`]), and
    /// the focused row's description goes in the base's help line
    /// ([`DialogBase::help`]) unless [`KitScreen::description`] names
    /// another block. A page on [`DialogBase::QuickOptions`] is that
    /// dialog inside as well as out: its list, skin, value column, help
    /// line, header and footer.
    pub fn dialog(mut self, base: DialogBase, opener: Opener) -> Self {
        self.dialog = Some((base, opener));
        self.style = base.style();
        self
    }
    /// A button besides the rows' (a settings dialog's `actions:`, the
    /// browser filters' X CLEAR FILTERS): `input` runs `press`, which says
    /// what to log, and the legend gets `words` after the rows' prompts.
    /// X and Start then no longer run the class's action
    /// ([`ListClass::action_buttons`]): A on a row still does.
    pub fn extra(
        mut self,
        input: Input,
        words: &str,
        press: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        self.prompts.push(Prompt::new(&[Glyph::of(input)], words));
        self.extras.push(Extra {
            input,
            press: Arc::new(press),
        });
        self
    }
    /// A dialog's list on `base`: the screen's own layout, else the
    /// base's (a settings dialog's is Quick Options' own,
    /// [`crate::ui_kit::dialog::QUICK_LIST`]).
    fn list_layout(&self, base: DialogBase, rows: usize) -> ListLayout {
        self.layout
            .unwrap_or_else(|| crate::ui_kit::dialog_tag::base_layout(base, rows))
    }
    /// The settings dialog's header, when it is one.
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }
    /// The header's name line from `name`, read each time it is drawn
    /// (a settings doc whose `settings.name` is a lone slot, such as
    /// Quick Options' variant name). It does not change with the focus.
    pub fn header_name(mut self, name: impl Fn() -> String + Send + Sync + 'static) -> Self {
        self.header_name = Some(Arc::new(name));
        self
    }
    /// Header line `line`'s text now.
    fn header_text(&self, line: &HeaderLine) -> String {
        match (&self.header_name, line.id) {
            (Some(name), "header.name") => name(),
            _ => line.text.clone(),
        }
    }
    /// The rows drawn, by their index among all the rows: every row but a
    /// slot with nothing in it ([`Row::slot`]).
    fn shown(&self) -> Vec<usize> {
        (0..self.rows.len()).filter(|&i| self.rows[i].drawn()).collect()
    }
    /// The drawn row `row` (the list's index): its index among all the
    /// rows, and the row.
    fn at(&self, row: i16) -> Option<(usize, &Row)> {
        let i = *self.shown().get(usize::try_from(row).ok()?)?;
        Some((i, &self.rows[i]))
    }
    /// The extra button `input` pressed: what it logged, `None` when the
    /// screen has no such button.
    pub fn press_extra(&self, input: Input) -> Option<String> {
        let e = self.extras.iter().find(|e| e.input == input)?;
        let line = (e.press)();
        eprintln!("{}: {input:?}: {line}", self.name);
        Some(line)
    }
    /// The focused row's description (and the legend) in `block`.
    pub fn description(mut self, block: TextBlock) -> Self {
        self.description = Some(block);
        self
    }
    /// A page-wide line, in the doc's description block, that can change
    /// while the same row stays focused.
    pub fn live_help(mut self, text: impl Fn() -> String + Send + Sync + 'static) -> Self {
        self.live_help = Some(LiveHelp {
            get: Arc::new(text),
            drawn: Mutex::default(),
        });
        self
    }

    /// Where the focused row's description goes: the screen's own block
    /// (a settings dialog's is under its rows), else its dialog base's
    /// help block (the value picker's).
    fn description_block(&self) -> Option<TextBlock> {
        self.description
            .or_else(|| self.dialog.and_then(|(base, _)| base.help()))
    }
    pub fn prompts(mut self, prompts: &[Prompt]) -> Self {
        self.prompts = prompts.to_vec();
        self
    }
    /// The description block's animation (its widget's track), where the
    /// doc's block shows with one of its own.
    pub fn description_animation(mut self, animation: u16) -> Self {
        if let Some(d) = &mut self.description {
            d.animation = animation;
        }
        self
    }
    /// The opener-press guard ([`crate::ui_kit::input::OPENER_GUARD`] by
    /// default).
    pub fn guard(mut self, guard: std::time::Duration) -> Self {
        self.guard = OpenerGuard::new(guard);
        self
    }
    /// More rows than the list shows is meant: it scrolls.
    pub fn scrolls(mut self) -> Self {
        self.scrolls = true;
        self
    }
    /// The text of a row on another screen that opens this one ("Clanker
    /// Options..."), in a slot of its own after the footer, so the title
    /// keeps its name ([`KitScreen::row_label_id`]).
    pub fn row_label(mut self, text: &str) -> Self {
        self.row_label = Some(text.into());
        self
    }
    /// What the row elsewhere that opens it does, for that screen's
    /// description ("Bots that join this game..."), in a slot of its own
    /// after the row's text ([`KitScreen::row_help_id`]).
    pub fn row_help(mut self, text: &str) -> Self {
        self.row_help = Some(text.into());
        self
    }
    /// The string id of [`KitScreen::row_help`]'s text, written with the
    /// other texts; `None` when there is none.
    pub fn row_help_id(&self) -> Option<u32> {
        let slot = self.footer_slot() + 1 + u32::from(self.row_label.is_some());
        self.row_help.as_ref().map(|t| self.texts.set(slot, t))
    }
    /// The string id of the opening row's text: the [`KitScreen::row_label`]
    /// slot, or the title's when there is none. Written with the other
    /// texts ([`Rows::write_texts`]).
    pub fn row_label_id(&self) -> u32 {
        match &self.row_label {
            Some(text) => self.texts.set(self.footer_slot() + 1, text),
            None => self.title(),
        }
    }

    // Slots: 0 the title, then a label and a value per row, then a
    // description per row, the footer, the opening row's text when there
    // is one, its help when there is one, and the header's lines.
    fn label_slot(row: usize) -> u32 {
        1 + 2 * row as u32
    }
    fn description_slot(&self, row: usize) -> u32 {
        1 + 2 * self.rows.len() as u32 + row as u32
    }
    fn footer_slot(&self) -> u32 {
        self.description_slot(self.rows.len())
    }
    fn header_slot(&self) -> u32 {
        self.footer_slot()
            + 1
            + u32::from(self.row_label.is_some())
            + u32::from(self.row_help.is_some())
    }
    /// The header's lines.
    fn header_lines(&self) -> &[HeaderLine] {
        self.header.as_ref().map_or(&[], |h| h.lines.as_slice())
    }
    /// The header's icon and the base's picture it shows in.
    fn header_icon(&self) -> Option<(crate::list_class::PictureSpec, u16)> {
        Some((self.dialog?.0.picture()?, self.header.as_ref()?.icon?))
    }
    /// The slots the screen needs: fails the build of its class when its
    /// family is too small.
    pub fn slots(&self) -> u32 {
        self.header_slot() + self.header_lines().len() as u32
    }
    /// Sets text slot `slot` of the screen's family, one past its own
    /// ([`KitScreen::slots`]) for a class that draws more than its rows
    /// (`crate::ui_kit::card_screen`'s stats and count). Its string id.
    pub(super) fn set_text(&self, slot: u32, text: &str) -> u32 {
        self.texts.set(slot, text)
    }
    /// Its screen's name in [`crate::ui_kit::screens::NAMES`].
    pub fn name(&self) -> &'static str {
        self.name
    }
    /// The family the screen's text slots are in.
    pub(super) fn family(&self) -> Family {
        self.texts.family
    }
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Why the screen can't be snapshotted now: a press waits for the
    /// action, our keyboard is open, or work waits for a screen's free.
    /// Each is host state (a closure, a row) the guest doesn't hold.
    pub fn snapshot_live(&self) -> Option<String> {
        let name = self.name;
        if Self::lock(&self.pending).is_some() {
            return Some(format!("{name}: a press waits for the screen's action"));
        }
        if let Some(row) = *Self::lock(&self.typing) {
            return Some(format!("{name}: the keyboard for row {row} is open"));
        }
        self.after
            .waiting()
            .then(|| format!("{name}: work waits for a screen to be freed"))
    }

    /// A on the class's action (or X, Start): what the row left, or the
    /// guard's drop.
    async fn act(&self, cx: &Cx, screen: u32) -> Result<(), EntryError> {
        let name = self.name;
        if let Some(since) = self.guard.too_soon(Instant::now()) {
            eprintln!(
                "{name}: a confirm {} ms after opening: the press that opened it; ignored",
                since.as_millis()
            );
            Self::lock(&self.pending).take();
            return Ok(());
        }
        let pending = Self::lock(&self.pending).take();
        match pending {
            None => Ok(()),
            Some(Pending::Close) => {
                eprintln!("{name}: closing");
                crate::ui_kit::dialog_tag::close(cx, screen).await
            }
            Some(Pending::CloseThen(work)) => {
                if self.after.then(screen, work) {
                    eprintln!("{name}: closing; the rest runs once {screen:#x} is freed");
                    crate::ui_kit::dialog_tag::close(cx, screen).await
                } else {
                    eprintln!("{name}: already closing");
                    Ok(())
                }
            }
            Some(Pending::Keyboard(row)) => self.open_keyboard(cx, row).await,
        }
    }

    async fn open_keyboard(&self, cx: &Cx, row: usize) -> Result<(), EntryError> {
        let (Some(buffer), Some(Kind::Text { get, max_chars, .. })) =
            (self.texts.keyboard(), self.rows.get(row).map(|r| &r.kind))
        else {
            return Ok(());
        };
        let text = get();
        cx.write(|m| m.write(buffer, &crate::signin_keyboard::units(&text)))?;
        *Self::lock(&self.typing) = Some(row);
        eprintln!(
            "{}: opening the keyboard for row {row} ({} characters)",
            self.name, max_chars
        );
        cx.call(GuestCall::new(crate::signin_keyboard::OPEN).ecx(0).args([
            KEYBOARD_CONTEXT,
            buffer,
            u32::from(*max_chars) + 1,
        ]))
        .await?;
        Ok(())
    }

    /// The keyboard's text once it closed, to its row.
    fn poll_keyboard(&self, m: &dyn GuestMemory) -> Result<(), MemFault> {
        let Some(buffer) = self.texts.keyboard() else {
            return Ok(());
        };
        let mut typing = Self::lock(&self.typing);
        let Some(row) = *typing else { return Ok(()) };
        if crate::signin_keyboard::open_on(m, buffer)? {
            return Ok(());
        }
        *typing = None;
        let text = crate::signin_keyboard::read_text(m, buffer, KEYBOARD_UNITS)?;
        if let Some(Kind::Text { typed, .. }) = self.rows.get(row).map(|r| &r.kind) {
            let text = text.trim();
            eprintln!("{}", typed((!text.is_empty()).then_some(text)));
        }
        Ok(())
    }

    /// The screen was freed: its waiting work, run now.
    async fn freed(&self, cx: Cx, screen: u32) -> Result<(), EntryError> {
        match self.after.freed(screen) {
            Some(work) => {
                eprintln!("{}: {screen:#x} is freed; running what waited", self.name);
                work(cx).await
            }
            None => {
                eprintln!("{}: {screen:#x} is freed (B)", self.name);
                Ok(())
            }
        }
    }

    /// The class's action: A on a row (through its press), X and Start.
    pub fn action(self: &Arc<Self>) -> Action {
        let me = Arc::clone(self);
        Arc::new(move |cx, screen| {
            let me = Arc::clone(&me);
            Box::pin(async move { me.act(&cx, screen).await })
        })
    }
    /// The class's destructor step: what waited for the free runs.
    pub fn on_free(self: &Arc<Self>) -> OnFree {
        let me = Arc::clone(self);
        Arc::new(move |cx, screen| {
            let me = Arc::clone(&me);
            Box::pin(async move { me.freed(cx, screen).await })
        })
    }

    /// The list class showing it: A on the row (or X, Start) through the
    /// action, left and right on value rows, the free told.
    pub fn class(self: &Arc<Self>) -> ListClass {
        assert!(
            self.slots() <= self.texts.family.len,
            "{}: {} text slots in a family of {}",
            self.name,
            self.slots(),
            self.texts.family.len
        );
        let mut c =
            ListClass::with_rows(self.feature, self.name, Arc::clone(self) as Arc<dyn Rows>)
                .animation(self.animation)
                .action(self.action())
                .sideways()
                .on_free(self.on_free());
        if let Some(b) = self.description_block() {
            c = c.description(b);
        }
        if let Some((picture, _)) = self.header_icon() {
            c = c.picture(picture);
        }
        if !self.header_lines().is_empty() {
            c = c.panel(Panel {
                art: Vec::new(),
                texts: self
                    .header_lines()
                    .iter()
                    .map(|l| PanelText {
                        block: l.block,
                        right: false,
                        bias: 0,
                    })
                    .collect(),
            });
        }
        if !self.extras.is_empty() {
            let me = Arc::clone(self);
            c = c
                .action_buttons(&[])
                .buttons(Arc::new(move |kind, button| {
                    match Input::from_event(kind, button).and_then(|i| me.press_extra(i)) {
                        Some(_) => Button::Handled,
                        None => Button::Pass,
                    }
                }));
        }
        if let Some(l) = self.layout {
            c = c.layout(l);
        }
        if let Some((base, _)) = self.dialog {
            // Laid out for every row: a slot filled later draws on it.
            c = c.dialog(base).layout(self.list_layout(base, self.rows.len()));
        }
        if let Some(text) = self.spec().footer_text() {
            c = c.footer(self.texts.set(self.footer_slot(), &text));
        }
        c
    }
}

impl Rows for KitScreen {
    fn title(&self) -> u32 {
        self.texts.set(0, &self.title)
    }
    fn count(&self) -> usize {
        self.shown().len()
    }
    fn content_spec(&self) -> Option<ScreenSpec> {
        Some(self.spec())
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some((i, r)) = self.at(row) else {
            return Vec::new();
        };
        let (label, value) = r.spec().cells(self.style);
        let mut v = vec![(LABEL_SLOT, self.texts.set(Self::label_slot(i), &label))];
        if self.style.has_column() {
            let value = value.unwrap_or_default();
            v.push((VALUE_SLOT, self.texts.set(Self::label_slot(i) + 1, &value)));
        }
        v
    }
    fn press(&self, row: i16) -> Chosen {
        let Some((at, r)) = self.at(row) else {
            return Chosen::Nothing;
        };
        let pend = |p: Pending| {
            *Self::lock(&self.pending) = Some(p);
            Chosen::Act
        };
        match &r.kind {
            Kind::Action(press) | Kind::Card { press, .. } => match press() {
                Then::Stay(line) => Chosen::Stepped(line),
                Then::Open(name) => Chosen::Open(name),
                Then::Close => pend(Pending::Close),
                Then::CloseThen(work) => pend(Pending::CloseThen(work)),
            },
            // A value row that picks on a screen of its own: pushed, so B
            // comes back to this one.
            Kind::Value { .. } if r.opens.is_some() => {
                r.opens.map_or(Chosen::Nothing, Chosen::Push)
            }
            Kind::Value { values, get, set } => {
                let pick = crate::ui_kit::picker::Pick::new(
                    &r.label,
                    (values)(),
                    get(),
                    r.explain
                        .as_deref()
                        .or(r.description.as_deref())
                        .unwrap_or_default(),
                    Arc::clone(set),
                )
                .over(self.dialog.map_or(Opener::MAIN_MENU, |(_, o)| o));
                crate::ui_kit::picker::offer_list(pick).map_or(Chosen::Nothing, Chosen::Open)
            }
            Kind::Info(_) => Chosen::Nothing,
            Kind::Pick { offer, .. } => match offer() {
                Ok(mut pick) => {
                    // A settings dialog's picker is titled by the row and
                    // says the setting's help: the row's explain, else its
                    // help (`settings:` docs). Its per-value lines stay.
                    if self.header.is_some() {
                        pick.title = crate::ui_kit::picker::title(&r.label);
                        if let Some(text) = r.explain.as_deref().or(r.description.as_deref()) {
                            pick.description = text.into();
                        }
                    }
                    crate::ui_kit::picker::offer_list(
                    pick.over(self.dialog.map_or(Opener::MAIN_MENU, |(_, o)| o)),
                )
                    .map_or_else(
                        || Chosen::Stepped(format!("{}: nothing to pick from", r.label)),
                        Chosen::Open,
                    )
                }
                Err(why) => Chosen::Stepped(why),
            },
            Kind::Text { .. } => pend(Pending::Keyboard(at)),
            Kind::Slot { fill, offer } => {
                let Some(f) = fill() else {
                    return Chosen::Nothing;
                };
                match offer() {
                    Ok(mut pick) => {
                        // Titled by the label its binding gave, saying
                        // its help, as a settings row's picker is.
                        pick.title = crate::ui_kit::picker::title(&f.label);
                        pick.description = f.help.clone().unwrap_or_default();
                        crate::ui_kit::picker::offer_list(
                            pick.over(self.dialog.map_or(Opener::MAIN_MENU, |(_, o)| o)),
                        )
                        .map_or_else(
                            || Chosen::Stepped(format!("{}: nothing to pick from", f.label)),
                            Chosen::Open,
                        )
                    }
                    Err(why) => Chosen::Stepped(why),
                }
            }
        }
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        let (_, r) = self.at(row)?;
        let Kind::Value { values, get, set } = &r.kind else {
            return None;
        };
        let n = (values)().len().max(1);
        let next = if right {
            (get() + 1) % n
        } else {
            (get() + n - 1) % n
        };
        Some(format!(
            "{}: {} on row {row}: {}",
            self.name,
            if right { "right" } else { "left" },
            set(next)
        ))
    }
    fn description(&self, row: i16) -> Option<u32> {
        let i = usize::try_from(row).ok().filter(|i| *i < self.count())?;
        let spec = ScreenSpec {
            focus: i,
            ..self.spec()
        };
        let text = spec.description_text()?;
        Some(self.texts.set(self.description_slot(i), &text))
    }
    fn live_text(&self) -> LiveText {
        let redraw = self.live_help.as_ref().is_some_and(|help| {
            let current = (help.get)();
            let mut drawn = Self::lock(&help.drawn);
            if *drawn == current {
                return false;
            }
            *drawn = current;
            true
        });
        // A slot's fill changed (a value set, a refusal said): its help
        // is drawn again with the focus where it is.
        let fills: Vec<Option<Filled>> = self.rows.iter().filter_map(Row::filled).collect();
        let slots = {
            let mut drawn = Self::lock(&self.slots_drawn);
            let changed = *drawn != fills;
            *drawn = fills;
            changed
        };
        LiveText {
            redraw: redraw || slots,
            ..Default::default()
        }
    }

    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.poll_keyboard(m)?;
        self.texts.write(m)
    }
    /// The header's icon, the same in every state.
    fn picture_image(&self) -> Option<u16> {
        self.header_icon().map(|(_, icon)| icon)
    }
    /// The header's line `i`: its name, then what the window is for. No
    /// row reaches it.
    fn panel_text(&self, i: usize) -> Option<u32> {
        let line = self.header_lines().get(i)?;
        Some(self.texts.set(self.header_slot() + i as u32, &self.header_text(line)))
    }
    /// Asked as the screen opens: the opener guard starts, and the log
    /// gets the screen as [`ScreenSpec::log_line`] writes it.
    fn describe(&self, _count: usize) -> Option<String> {
        self.guard.opened();
        Some(self.spec().log_line())
    }
}

impl Describe for KitScreen {
    fn spec(&self) -> ScreenSpec {
        let mut s = ScreenSpec::new(self.name, &self.title);
        s.animation = self.animation;
        s.list = ListPlace::from(self.layout);
        s.style = self.style;
        let shown = self.shown();
        for (i, row) in shown.iter().map(|&r| &self.rows[r]).enumerate() {
            let (row, loaded) = row.spec_with_source();
            if !loaded {
                s.content.unavailable_values.push(format!("row.{i}.value"));
            }
            s.rows.push(row);
        }
        s.description = self.description_block().map(spec::DescriptionBlock::from);
        if s.description.is_none() {
            // The large dialog has no help line: a row's description would
            // have nowhere to show.
            for row in &mut s.rows {
                row.description = None;
            }
        }
        s.content.row_help = self.description.is_some() && self.live_help.is_none();
        if let Some((_, icon)) = self.header_icon() {
            s.art = vec![crate::ui_kit::dialog::quick_icon(icon)];
            s.content.required_icons.push("art.0".into());
        }
        s.texts = self
            .header_lines()
            .iter()
            .map(|l| PaneText {
                id: l.id.into(),
                rect: l.block.rect,
                font: l.block.font,
                colour: l.block.colour,
                right: false,
                text: self.header_text(l),
            })
            .collect();
        s.help = self.live_help.as_ref().map(|help| (help.get)());
        s.prompts = self.prompts.clone();
        s.inputs = vec![
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::A,
            Input::B,
            Input::X,
            Input::Start,
        ];
        s.scrolls = self.scrolls;
        s.footer = true;
        if let Some((base, opener)) = self.dialog {
            let list = self.list_layout(base, shown.len());
            s.list = ListPlace::from(Some(list));
            s.dialog = Some(DialogSpec {
                base,
                rows: s.rows.clone(),
                ..DialogSpec::new("", opener)
            });
        }
        s
    }
}

/// The screen's texts as a locale source: its family's ids, from a page
/// of ours. Its feature id is `ui-kit-texts.<name>`, so each kit screen's
/// texts are a feature of their own.
pub struct KitTexts(pub Arc<KitScreen>);

impl KitTexts {
    /// `ui-kit-texts.<name>`: leaked once per screen, as a manifest's id
    /// is `'static`.
    pub fn id(&self) -> &'static str {
        let s = &self.0;
        s.texts_id
            .get_or_init(|| Box::leak(format!("ui-kit-texts.{}", s.name).into_boxed_str()))
    }
}

impl hooks::Feature for KitTexts {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(self.id(), "A kit screen's texts").doc("docs/game/ui-screens.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let s = &self.0;
        let len = s.texts.family.len * SLOT_LEN + KEYBOARD_UNITS * 2;
        let base = b.guest_alloc(len.next_multiple_of(0x1000), Permissions::RW)?;
        s.texts.base.store(base, Ordering::SeqCst);
        let me = Arc::clone(s);
        b.locale().source(s.name, move |id| me.texts.address(id));
        // The screen's state is its rows' (settings, kept outside the
        // snapshot) and the class's; a load only has to write the texts
        // again over the saved run's.
        let (live, load) = (Arc::clone(s), Arc::clone(s));
        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || live.snapshot_live(),
            Vec::new,
            move |bytes, _| {
                if !bytes.is_empty() {
                    return Err(format!("{} unexpected bytes", bytes.len()));
                }
                load.texts.rewrite_all();
                Ok(())
            },
        ));
        Ok(())
    }
}

const IDS: Family = ids::family("kit.example");

/// The example screen of docs/game/ui-screens.md.
pub fn example() -> (KitScreen, Arc<AtomicUsize>) {
    let grunts = Arc::new(AtomicUsize::new(0));
    let (g, s) = (Arc::clone(&grunts), Arc::clone(&grunts));
    let screen = KitScreen::new("kit-example", "kit.example", IDS)
        .row(
            Row::value(
                "Grunts",
                &["On", "Off"],
                move || g.load(Ordering::SeqCst),
                move |v| {
                    s.store(v, Ordering::SeqCst);
                    format!("grunts {v}")
                },
            )
            .describe("Grunts' heads pop."),
        )
        .row(Row::info("Status", || "Ready".into()).describe("The match is ready."))
        .row(
            Row::text("Name", 15, || "Chief".into(), |t| format!("typed {t:?}"))
                .describe("Choose a display name."),
        )
        .row(Row::action("Start", || Then::Close).describe("Start the level."));
    (screen, grunts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::lifecycle;

    #[test]
    fn the_page_animation_override_is_in_its_authored_model() {
        let (page, _) = example();
        let page = page.animation(TransitionStyle::Off);
        assert_eq!(Describe::spec(&page).animation, TransitionStyle::Off);
    }

    /// Each screen's texts are a feature of their own, so a second kit
    /// screen in the game no longer fails the plan with DuplicateFeature.
    #[test]
    fn two_kit_screens_register_together() {
        let a = Arc::new(example().0);
        let b = Arc::new(crate::ui_kit::screens::choices_example(
            crate::ui_kit::choice::example::store(),
        ));
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        for s in [&a, &b] {
            assert!(r.add(&KitTexts(Arc::clone(s))).unwrap());
            assert!(r.add(&s.class()).unwrap());
        }
        assert_eq!(KitTexts(Arc::clone(&a)).id(), "ui-kit-texts.kit.example");
        assert_eq!(KitTexts(Arc::clone(&b)).id(), "ui-kit-texts.kit.choices");
        assert!(
            std::ptr::eq(KitTexts(Arc::clone(&a)).id(), KitTexts(Arc::clone(&a)).id()),
            "made once per screen"
        );
        if let Err(e) = r.build(&hooks::Base) {
            panic!("two kit screens conflict: {e:?}");
        }
    }

    /// A dialog screen's spec is a dialog of its rows in its base's list,
    /// values as the base's skin has room for them; its class is a
    /// dialog class on the same list.
    #[test]
    fn a_dialog_screen_is_its_rows_in_its_bases_list() {
        for (base, skin, anchor, style, help) in [
            (
                DialogBase::QuickOptions,
                2,
                crate::ui_kit::dialog::QUICK_LIST.anchor,
                ValueStyle::Setting,
                Some(crate::ui_kit::dialog::SETTINGS_HELP_TEXT),
            ),
            (DialogBase::Large, 0, [-235, 30], ValueStyle::Inline, None),
        ] {
            let k = Arc::new(crate::ui_kit::screens::dialog_example(
                crate::ui_kit::choice::example::store(),
                base,
            ));
            let s = k.spec();
            let d = s.dialog.clone().expect("a dialog");
            assert_eq!(d.base, base);
            assert_eq!(d.rows, s.rows);
            assert_eq!(d.rows.len(), 2);
            assert!(d.body.is_empty(), "rows, no message");
            assert_eq!(d.opener, Opener::MAIN_MENU);
            assert_eq!(s.style, style, "{base:?}");
            // Quick Options' sample is a settings dialog: its focused
            // help under the rows. The large dialog has no help block.
            assert_eq!(
                s.description,
                help.map(spec::DescriptionBlock::from),
                "{base:?}"
            );
            assert_eq!(
                s.list,
                ListPlace::Moved {
                    skin,
                    anchor,
                    bitmaps_left_of: i16::MIN,
                    visible: 2,
                },
                "{base:?}"
            );
            assert_eq!(k.class().dialog_base(), Some(base));
        }
        let plain = Arc::new(example().0);
        assert_eq!(plain.spec().dialog, None);
        assert_eq!(plain.class().dialog_base(), None);
    }

    #[test]
    fn rows_show_through_the_familys_slots() {
        let (s, _) = example();
        assert_eq!(s.count(), 4);
        assert_eq!(s.title(), IDS.id(0));
        assert_eq!(
            s.texts(0),
            vec![(LABEL_SLOT, IDS.id(1)), (VALUE_SLOT, IDS.id(2))]
        );
        let t = |id: u32| {
            let slots = s.texts.slots.lock().unwrap();
            let b = &slots[(id - IDS.base) as usize].0;
            String::from_utf8_lossy(&b[..b.len() - 1]).into_owned()
        };
        assert_eq!(t(IDS.id(2)), "On");
        s.texts(1);
        assert_eq!(t(IDS.id(4)), "Ready");
        let d = Rows::description(&s, 0).unwrap();
        assert_eq!(t(d), "Grunts' heads pop.", "the legend is the footer's");
        assert_eq!(s.spec().footer_text().unwrap(), "\u{e101} BACK");
        assert!(s.slots() <= IDS.len);
    }

    /// A settings dialog's header is its doc's and the same whatever has
    /// the focus: its icon asked once, its lines panel texts, which take
    /// no row. Its help goes under the rows.
    #[test]
    fn a_settings_dialogs_header_is_its_docs_whatever_has_the_focus() {
        use crate::ui_kit::dialog::{QUICK_NAME_TEXT, SETTINGS_ABOUT_TEXT, SETTINGS_HELP_TEXT, icon};
        let d = doc(
            "screen: t\nkind: dialog\nlayout: settings\nbacking: over\nopener: game-setup\nsettings:\n  host: menu\n  icon: crown\n  name: \"The name\"\n  about: \"What it is for.\"\nwidgets:\n  title:\n    component: title\n    text: \"T\"\n",
        );
        let s = Arc::new(
            KitScreen::from_doc("t", "t", IDS, &d)
                .unwrap()
                .row(Row::info("One", || "1".into()).describe("First."))
                .row(Row::info("Two", || "2".into()).describe("Second.")),
        );
        let h = s.header().expect("a header");
        assert_eq!(h.icon, Some(icon::CROWN));
        let lines: Vec<(&str, &str)> = h.lines.iter().map(|l| (l.id, l.text.as_str())).collect();
        assert_eq!(lines, [("header.name", "The name"), ("header.about", "What it is for.")]);
        assert_eq!(h.lines[0].block.rect, QUICK_NAME_TEXT.rect);
        assert_eq!(h.lines[1].block.rect, SETTINGS_ABOUT_TEXT.rect);
        assert_eq!(s.description_block(), Some(SETTINGS_HELP_TEXT));
        assert_eq!(Rows::picture_image(&*s), Some(icon::CROWN));
        let t = |id: u32| {
            let slots = s.texts.slots.lock().unwrap();
            let b = &slots[(id - IDS.base) as usize].0;
            String::from_utf8_lossy(&b[..b.len() - 1]).into_owned()
        };
        let name = Rows::panel_text(&*s, 0).unwrap();
        let about = Rows::panel_text(&*s, 1).unwrap();
        assert_eq!((t(name), t(about)), ("The name".into(), "What it is for.".into()));
        assert_eq!(Rows::panel_text(&*s, 2), None);
        assert!(s.slots() <= IDS.len);
        let class = s.class();
        assert_eq!(class.panel_of().texts.len(), 2);
        assert_eq!(class.panel_of().texts[0].block, QUICK_NAME_TEXT);
        // Each focus: the same art and lines; only the help moves.
        let mut helps = Vec::new();
        for focus in 0..s.count() {
            let f = ScreenSpec { focus, ..s.spec() };
            assert_eq!(f.art, [crate::ui_kit::dialog::quick_icon(icon::CROWN)]);
            let texts: Vec<&str> = f.texts.iter().map(|t| t.text.as_str()).collect();
            assert_eq!(texts, ["The name", "What it is for."]);
            assert_eq!((Rows::panel_text(&*s, 0), Rows::panel_text(&*s, 1)), (Some(name), Some(about)));
            helps.push(f.description_text().unwrap());
        }
        assert_eq!(helps, ["First.", "Second."]);
        assert_eq!(s.spec().content.required_icons, ["art.0"]);
        // The legend: no value row, so no CHANGE.
        let spec = s.spec();
        let words: Vec<&str> = spec.prompts.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(words, ["SELECT", "BACK"]);
    }

    /// An extra button runs its press, says it in the legend, and takes X
    /// from the class's action.
    #[test]
    fn an_extra_button_runs_its_press_and_shows_in_the_legend() {
        let pressed = Arc::new(AtomicUsize::new(0));
        let p = Arc::clone(&pressed);
        let s = Arc::new(example().0.extra(Input::X, "CLEAR", move || {
            p.fetch_add(1, Ordering::SeqCst);
            "cleared".into()
        }));
        assert_eq!(s.press_extra(Input::X).as_deref(), Some("cleared"));
        assert_eq!(s.press_extra(Input::Y), None);
        assert_eq!(pressed.load(Ordering::SeqCst), 1);
        let last = s.spec().prompts.last().cloned().unwrap();
        assert_eq!(last, Prompt::new(&[Glyph::X], "CLEAR"));
        let class = s.class();
        let b = class.behaviour();
        assert!(b.action_buttons.is_empty(), "X is the extra's, not the action's");
        let (kind, button) = Input::X.event();
        assert_eq!(b.hook.map(|h| h(kind, button)), Some(Button::Handled));
        let (kind, button) = Input::A.event();
        assert_eq!(b.hook.map(|h| h(kind, button)), Some(Button::Pass));
        assert_eq!(pressed.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn an_opening_rows_text_has_its_own_slot_and_the_title_keeps_its_name() {
        let (s, _) = example();
        assert_eq!(s.row_label_id(), s.title(), "none: the title's");
        let before = s.slots();
        let s = s.row_label("Example Options...");
        assert_eq!(s.slots(), before + 1);
        let id = s.row_label_id();
        assert_eq!(id, IDS.id(before));
        assert_ne!(id, s.title());
        let t = |id: u32| {
            let slots = s.texts.slots.lock().unwrap();
            let b = &slots[(id - IDS.base) as usize].0;
            String::from_utf8_lossy(&b[..b.len() - 1]).into_owned()
        };
        assert_eq!(t(id), "Example Options...");
        assert_eq!(t(s.title()), s.title);
        assert!(s.slots() <= IDS.len);
    }

    #[test]
    fn left_right_step_values_and_a_leaves_work_for_the_action() {
        let (s, grunts) = example();
        assert_eq!(
            s.sideways(0, true).as_deref(),
            Some("kit.example: right on row 0: grunts 1")
        );
        assert_eq!(grunts.load(Ordering::SeqCst), 1);
        assert!(s.sideways(0, true).unwrap().ends_with("grunts 0"), "wraps");
        assert!(s.sideways(0, false).unwrap().ends_with("grunts 1"));
        assert_eq!(s.sideways(1, true), None, "an info row has no value");
        assert_eq!(s.press(1), Chosen::Nothing);
        assert_eq!(s.press(3), Chosen::Act);
        assert!(matches!(*s.pending.lock().unwrap(), Some(Pending::Close)));
        assert_eq!(s.press(2), Chosen::Act);
        assert!(matches!(
            *s.pending.lock().unwrap(),
            Some(Pending::Keyboard(2))
        ));
    }

    #[test]
    fn the_example_passes_the_static_checks() {
        let (s, _) = example();
        let spec = s.spec();
        assert_eq!(spec::check_fallback(&spec), vec![]);
        assert!(
            s.describe(4)
                .unwrap()
                .starts_with("kit.example: \"Example\"; rows: [0] \"Grunts\" = \"On\"")
        );
        assert!(s.guard.last().is_some(), "opening starts the guard");
    }

    /// The screen on the runner, as the game drives it: opened (the
    /// loader asks `describe`), right on the value row through the event
    /// slot, B handed to the Controls handler, A on Start through the row
    /// press (closing, nothing else), then the destructor slot, where the
    /// work that waited runs; and it runs once.
    #[test]
    fn a_full_press_sequence_on_the_runner() {
        use crate::list_class::{BUTTON_EVENT, RIGHT_EVENT, destructor, event, row_chosen};
        use guest_layout::entry::{EntryFuture, GuestCall as Gc, Regs, Ret, testing};
        use guest_layout::points;
        use guest_layout::testing::FakeGuest;
        use guest_layout::view::ui as vui;

        const SCREEN: u32 = 0x0010_0000;
        const EV: u32 = 0x0020_0000;
        const STACK: u32 = 0x0030_0800;
        const CHAIN: u32 = 0x0040_0000;
        const HANDLER: u32 = 0x0000_5010;
        const DTOR: u32 = 0x0000_5000;
        const STARTED: u32 = 0x0077_0000;

        let (s, grunts) = example();
        let work: After = Arc::new(|cx| {
            Box::pin(async move {
                cx.call(Gc::new(STARTED)).await?;
                Ok(())
            })
        });
        let s = Arc::new(
            KitScreen {
                rows: Vec::new(),
                ..s
            }
            .row(Row::value(
                "Grunts",
                &["On", "Off"],
                {
                    let g = Arc::clone(&grunts);
                    move || g.load(Ordering::SeqCst)
                },
                {
                    let g = Arc::clone(&grunts);
                    move |v| {
                        g.store(v, Ordering::SeqCst);
                        format!("grunts {v}")
                    }
                },
            ))
            .row(Row::action("Start", move || {
                Then::CloseThen(Arc::clone(&work))
            }))
            .guard(std::time::Duration::ZERO),
        );
        let guest = || {
            let mut g = FakeGuest::new();
            g.map(SCREEN, 0x1000)
                .map(EV, 0x100)
                .map(STACK - 0x800, 0x1000)
                .map(CHAIN, 0x100);
            g.put_u32(SCREEN + lifecycle::SCREEN_WINDOW, 5)
                .put_u32(SCREEN + lifecycle::SCREEN_SLOT, 4);
            g
        };
        let on = |ecx: u32| Regs {
            ecx,
            esp: STACK,
            ..Default::default()
        };

        // Opened.
        assert!(
            s.describe(2)
                .unwrap()
                .starts_with("kit.example: \"Example\"")
        );

        // Right on row 0: the focused row is asked, the value steps.
        let mut g = guest();
        g.put_u32(EV, RIGHT_EVENT).put_u32(STACK + 4, EV);
        let (rows, act) = (Arc::clone(&s) as Arc<dyn Rows>, s.action());
        let f = move |cx| -> EntryFuture {
            Box::pin(event(
                cx,
                HANDLER,
                Some(act.clone()),
                None,
                Some(rows.clone()),
            ))
        };
        let run = testing::run(&f, g, on(SCREEN), |c, _| Regs {
            eax: if c.target == points::LIST_FOCUSED_ROW.base {
                0xe000_0000
            } else {
                0
            },
            ..Default::default()
        });
        assert_eq!(run.result, Ok(Ret::new(1, 4)));
        assert_eq!(
            run.calls,
            [Gc::new(points::LIST_FOCUSED_ROW.base).eax(SCREEN + vui::LIST)]
        );
        assert_eq!(grunts.load(Ordering::SeqCst), 1);

        // B: not ours; the Controls handler closes as on any screen.
        let mut g = guest();
        g.put_u32(EV, BUTTON_EVENT)
            .put_u32(EV + 8, 1)
            .put_u32(STACK + 4, EV);
        let (rows, act) = (Arc::clone(&s) as Arc<dyn Rows>, s.action());
        let f = move |cx| -> EntryFuture {
            Box::pin(event(
                cx,
                HANDLER,
                Some(act.clone()),
                None,
                Some(rows.clone()),
            ))
        };
        let run = testing::run(&f, g, on(SCREEN), |_, _| Regs::default());
        assert_eq!(run.calls, [Gc::new(HANDLER).ecx(SCREEN).args([EV])]);

        // A on Start (row 1): the press leaves the close to the action,
        // which closes the screen and nothing else.
        let mut g = guest();
        g.put_u32(STACK + 4, CHAIN).put_u32(STACK + 8, CHAIN + 0x40);
        g.put_u16(CHAIN + 0x40, 1);
        let (rows, act) = (Arc::clone(&s) as Arc<dyn Rows>, s.action());
        let f =
            move |cx| -> EntryFuture { Box::pin(row_chosen(cx, rows.clone(), Some(act.clone()))) };
        let run = testing::run(&f, g, on(SCREEN + vui::LIST), |_, _| Regs::default());
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        assert_eq!(run.calls, [Gc::new(lifecycle::SCREEN_CLOSE).eax(4).ecx(5)]);
        assert!(s.after.closing(SCREEN));

        // Freed: the Controls destructor, then what waited; a second free
        // runs nothing more.
        for expect in [
            vec![Gc::new(DTOR).ecx(SCREEN).args([1]), Gc::new(STARTED)],
            vec![Gc::new(DTOR).ecx(SCREEN).args([1])],
        ] {
            let mut g = guest();
            g.put_u32(STACK + 4, 1);
            let free = s.on_free();
            let f = move |cx| -> EntryFuture { Box::pin(destructor(cx, DTOR, Some(free.clone()))) };
            let run = testing::run(&f, g, on(SCREEN), |_, _| Regs::default());
            assert_eq!(run.result, Ok(Ret::new(0, 4)));
            assert_eq!(run.calls, expect);
        }
    }

    /// A snapshot waits for a press's action, an open keyboard and work
    /// waiting for a free; after a load every text is written again.
    #[test]
    fn a_snapshot_waits_for_the_screen_and_rewrites_its_texts() {
        use guest_layout::testing::FakeGuest;
        let s = Arc::new(example().0);
        assert_eq!(s.snapshot_live(), None);
        assert_eq!(s.press(3), Chosen::Act);
        assert!(s.snapshot_live().is_some(), "Close waits for the action");
        s.pending.lock().unwrap().take();
        *s.typing.lock().unwrap() = Some(2);
        assert!(s.snapshot_live().unwrap().contains("keyboard"));
        s.typing.lock().unwrap().take();
        let work: After = Arc::new(|_| Box::pin(async { Ok::<(), EntryError>(()) }));
        assert!(s.after.then(0x0010_0000, work));
        assert!(s.snapshot_live().is_some(), "work waits for the free");
        assert!(s.after.freed(0x0010_0000).is_some());
        assert_eq!(s.snapshot_live(), None);

        const BASE: u32 = 0x0050_0000;
        let mut g = FakeGuest::new();
        g.map(BASE, IDS.len * SLOT_LEN);
        s.texts.base.store(BASE, Ordering::SeqCst);
        s.title();
        s.texts.write(&mut g).unwrap();
        let mut title = vec![0; s.title.len()];
        g.read(BASE, &mut title).unwrap();
        assert_eq!(title, s.title.as_bytes());
        // A load puts the saved run's page back: the cache would not see it.
        g.put(BASE, b"Saved\0");
        s.title();
        s.texts.write(&mut g).unwrap();
        g.read(BASE, &mut title[..5]).unwrap();
        assert_eq!(&title[..5], b"Saved");
        s.texts.rewrite_all();
        s.texts.write(&mut g).unwrap();
        g.read(BASE, &mut title).unwrap();
        assert_eq!(title, s.title.as_bytes());
    }

    fn doc(yaml: &str) -> crate::ui_kit::data::ScreenDoc {
        crate::ui_kit::data::ScreenDoc::parse("t", yaml, crate::ui_kit::data::design())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// A kit screen is its doc's: the title, the legend, the dialog and
    /// its opener, the description block and the fixed rows' labels; the
    /// rows' values and presses stay the caller's.
    #[test]
    fn a_kit_screen_is_presented_as_its_doc_says() {
        let d = doc(concat!(
            "screen: t\nkind: dialog\nlayout: large\nbacking: over\nopener: main-menu\n",
            "widgets:\n",
            "  title:\n    component: title\n    text: \"FROM THE DOC\"\n",
            "  description:\n    component: description\n    at: [10, 60]\n    size: [200, 30]\n",
            "  legend:\n    component: legend\n    prompts: [[B, \"BACK\"]]\n",
            "rows:\n  row.0:\n    text: \"Doc Label\"\n    help: \"Doc help.\"\n",
        ));
        let s = KitScreen::from_doc("t", "t", IDS, &d)
            .unwrap()
            .row(Row::action("Rust Label", || Then::Close).describe("Rust help."))
            .row(Row::action("Second", || Then::Close))
            .description_animation(7);
        let spec = s.spec();
        assert_eq!(spec.title, "FROM THE DOC");
        assert_eq!(spec.prompts.len(), 1);
        assert_eq!(spec.prompts[0].text, "BACK");
        assert_eq!(spec.dialog.as_ref().unwrap().base, DialogBase::Large);
        assert_eq!(spec.dialog.as_ref().unwrap().opener, Opener::MAIN_MENU);
        assert_eq!(spec.description.unwrap().rect, [60, 10, 30, 210]);
        assert_eq!(s.description.unwrap().animation, 7);
        assert_eq!(spec.rows[0].label, "Doc Label");
        assert_eq!(spec.rows[0].description.as_deref(), Some("Doc help."));
        assert_eq!(spec.rows[1].label, "Second", "a row the doc doesn't have keeps its label");
    }

    /// A dialog's description on its base's help block is the base's: the
    /// screen names no block of its own. Quick Options has none: a
    /// settings dialog's help goes under its rows.
    #[test]
    fn a_dialogs_help_block_stays_its_bases() {
        assert_eq!(DialogBase::QuickOptions.help(), None);
        assert_eq!(DialogBase::Large.help(), None);
        assert_eq!(
            DialogBase::ValuePicker.help(),
            Some(crate::ui_kit::dialog::PICKER_HELP_TEXT)
        );
        let d = doc(
            "screen: t\nkind: dialog\nlayout: large\nbacking: over\nopener: main-menu\nwidgets:\n  title:\n    component: title\n    text: \"T\"\n",
        );
        let s = KitScreen::from_doc("t", "t", IDS, &d).unwrap();
        assert_eq!(s.description, None);
        assert_eq!(s.description_block(), None);
        assert!(s.header().is_none());
    }

    #[test]
    fn a_doc_a_kit_screen_cannot_take_is_a_named_error() {
        let d = doc("screen: t\nkind: picker\nlayout: picker\nwidgets:\n  title:\n    text: \"T\"\n");
        let e = KitScreen::from_doc("t", "t", IDS, &d).err().unwrap();
        assert_eq!(e.at, "layout");
        let d = doc("screen: t\nkind: list\nlayout: controls\n");
        assert_eq!(KitScreen::from_doc("t", "t", IDS, &d).err().unwrap().at, "widgets.title.text");
    }
}
