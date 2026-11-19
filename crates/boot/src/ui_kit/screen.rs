//! A screen declared rather than wired (docs/game/ui-screens.md "Adding a
//! screen"): rows of four kinds, the legend from named glyphs, its texts
//! served from a string-id family of the registry, B back, A on the row,
//! left and right on value rows, the opener's press dropped, and closing
//! with work that runs once the screen is freed.
//!
//! ```ignore
//! const IDS: Family = ids::family("kit.example");
//! let s = KitScreen::new("kit-example", "kit.example", IDS, "Example")
//!     .row(Row::value("Grunts", &["On", "Off"], get, set))
//!     .row(Row::info("Status", status))
//!     .row(Row::action("Start", || Then::close_then(start)))
//!     .prompts(&[Prompt::new(&[Glyph::A], "Start"), Prompt::new(&[Glyph::B], "Back")]);
//! let s = Arc::new(s);
//! registry.add(&KitTexts(Arc::clone(&s)))?;
//! registry.add(&s.class())?;
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use engine::entry::{Cx, EntryError, GuestCall};
use engine::memory::{GuestMemory, MemFault};
use runtime::guest::Permissions;

use crate::list_class::{Action, ListClass, ListLayout, OnFree, Rows, TextBlock};
use crate::ui::{Chosen, LABEL_SLOT, VALUE_SLOT};
use crate::ui_kit::dialog::{DialogBase, DialogSpec};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::{Glyph, Input, OpenerGuard, Prompt};
use crate::ui_kit::lifecycle::{self, AfterClose};
use crate::ui_kit::spec::{self, Describe, ListPlace, RowSpec, ScreenSpec, ValueStyle};
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

enum Kind {
    Action(Get<Then>),
    Value {
        values: Vec<String>,
        get: Get<usize>,
        set: Arc<dyn Fn(usize) -> String + Send + Sync>,
    },
    Info(Get<String>),
    Text {
        get: Get<String>,
        max_chars: u16,
        typed: Typed,
    },
}

type Typed = Arc<dyn Fn(Option<&str>) -> String + Send + Sync>;

/// One row: a label, its kind, its description.
pub struct Row {
    label: String,
    kind: Kind,
    description: Option<String>,
}

impl Row {
    /// A runs `press`.
    pub fn action(label: &str, press: impl Fn() -> Then + Send + Sync + 'static) -> Self {
        Self::of(label, Kind::Action(Arc::new(press)))
    }
    /// Left and right step through `values` (wrapping); A opens the value
    /// picker on them (`crate::ui_kit::picker`, as retail's settings do),
    /// or steps right when the picker can't list them. `get` is the
    /// current index, `set` stores one and says what to log.
    pub fn value(
        label: &str,
        values: &[&str],
        get: impl Fn() -> usize + Send + Sync + 'static,
        set: impl Fn(usize) -> String + Send + Sync + 'static,
    ) -> Self {
        Self::of(
            label,
            Kind::Value {
                values: values.iter().map(|v| v.to_string()).collect(),
                get: Arc::new(get),
                set: Arc::new(set),
            },
        )
    }
    /// Read-only: `value` beside the label.
    pub fn info(label: &str, value: impl Fn() -> String + Send + Sync + 'static) -> Self {
        Self::of(label, Kind::Info(Arc::new(value)))
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
    pub fn describe(mut self, text: &str) -> Self {
        self.description = Some(text.into());
        self
    }
    fn of(label: &str, kind: Kind) -> Self {
        Row {
            label: label.into(),
            kind,
            description: None,
        }
    }
    fn spec(&self) -> RowSpec {
        let r = match &self.kind {
            Kind::Action(_) => RowSpec::action(&self.label),
            Kind::Value { values, get, .. } => {
                let v: Vec<&str> = values.iter().map(String::as_str).collect();
                RowSpec::value(&self.label, &v, get())
            }
            Kind::Info(v) => RowSpec::info(&self.label, &v()),
            Kind::Text { get, max_chars, .. } => RowSpec::text(&self.label, &get(), *max_chars),
        };
        match &self.description {
            Some(d) => r.describe(d),
            None => r,
        }
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
pub(crate) struct SlotTexts {
    pub(crate) family: Family,
    pub(crate) base: AtomicU32,
    slots: Mutex<Vec<(Vec<u8>, bool)>>,
}

impl SlotTexts {
    pub(crate) fn new(family: Family) -> Self {
        SlotTexts {
            family,
            base: AtomicU32::new(0),
            slots: Mutex::new(Vec::new()),
        }
    }
    pub(crate) fn set(&self, slot: u32, text: &str) -> u32 {
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
        let s = &mut slots[slot as usize];
        if s.0 != b {
            *s = (b, true);
        }
        id
    }
    pub(crate) fn address(&self, id: u32) -> Option<u32> {
        let base = self.base.load(Ordering::SeqCst);
        (base != 0 && self.family.owns(id)).then(|| base + (id - self.family.base) * SLOT_LEN)
    }
    fn keyboard(&self) -> Option<u32> {
        let base = self.base.load(Ordering::SeqCst);
        (base != 0).then(|| base + self.family.len * SLOT_LEN)
    }
    /// Every slot written again at the next [`Self::write`]: after a
    /// snapshot load the page holds the saved run's texts, which the
    /// cache can't tell from its own.
    pub(crate) fn rewrite_all(&self) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for (_, dirty) in slots.iter_mut() {
            *dirty = true;
        }
    }
    pub(crate) fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        let base = self.base.load(Ordering::SeqCst);
        if base == 0 {
            return Ok(());
        }
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for (i, (b, dirty)) in slots.iter_mut().enumerate() {
            if *dirty {
                m.write(base + i as u32 * SLOT_LEN, b)?;
                *dirty = false;
            }
        }
        Ok(())
    }
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
    style: ValueStyle,
    description: Option<TextBlock>,
    prompts: Vec<Prompt>,
    scrolls: bool,
    texts: SlotTexts,
    pending: Mutex<Option<Pending>>,
    after: AfterClose<After>,
    typing: Mutex<Option<usize>>,
    guard: OpenerGuard,
    /// Shown as a dialog of rows on a retail dialog, over its opener.
    dialog: Option<(DialogBase, Opener)>,
    /// [`KitTexts`]' feature id, made once.
    texts_id: OnceLock<&'static str>,
}

impl KitScreen {
    /// `feature` switches it (`features.<id>`), `name` is what `Open` rows
    /// name, `ids` its family in [`crate::ui_kit::ids::FAMILIES`].
    pub fn new(feature: &'static str, name: &'static str, ids: Family, title: &str) -> Self {
        KitScreen {
            feature,
            name,
            title: title.into(),
            row_label: None,
            row_help: None,
            rows: Vec::new(),
            layout: None,
            style: ValueStyle::Column,
            description: None,
            prompts: Vec::new(),
            scrolls: false,
            texts: SlotTexts::new(ids),
            pending: Mutex::new(None),
            after: AfterClose::default(),
            typing: Mutex::new(None),
            guard: OpenerGuard::default(),
            dialog: None,
            texts_id: OnceLock::new(),
        }
    }
    pub fn row(mut self, row: Row) -> Self {
        self.rows.push(row);
        self
    }
    /// The list moved and reskinned; a one-text skin shows values inline.
    pub fn layout(mut self, layout: ListLayout, style: ValueStyle) -> Self {
        self.layout = Some(layout);
        self.style = style;
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
    /// The focused row's description (and the legend) in `block`.
    pub fn description(mut self, block: TextBlock) -> Self {
        self.description = Some(block);
        self
    }
    /// Where the focused row's description goes: the screen's own block,
    /// else its dialog base's help line.
    fn description_block(&self) -> Option<TextBlock> {
        self.description
            .or_else(|| self.dialog.and_then(|(base, _)| base.help()))
    }
    pub fn prompts(mut self, prompts: &[Prompt]) -> Self {
        self.prompts = prompts.to_vec();
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
    // is one, and its help when there is one.
    fn label_slot(row: usize) -> u32 {
        1 + 2 * row as u32
    }
    fn description_slot(&self, row: usize) -> u32 {
        1 + 2 * self.rows.len() as u32 + row as u32
    }
    fn footer_slot(&self) -> u32 {
        self.description_slot(self.rows.len())
    }
    /// The slots the screen needs: fails the build of its class when its
    /// family is too small.
    pub fn slots(&self) -> u32 {
        self.footer_slot()
            + 1
            + u32::from(self.row_label.is_some())
            + u32::from(self.row_help.is_some())
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
                .action(self.action())
                .sideways()
                .on_free(self.on_free());
        if let Some(b) = self.description_block() {
            c = c.description(b);
        }
        if let Some(l) = self.layout {
            c = c.layout(l);
        }
        if let Some((base, _)) = self.dialog {
            c =
                c.dialog(base).layout(self.layout.unwrap_or_else(|| {
                    crate::ui_kit::dialog_tag::base_layout(base, self.rows.len())
                }));
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
        self.rows.len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some(r) = usize::try_from(row).ok().and_then(|i| self.rows.get(i)) else {
            return Vec::new();
        };
        let i = row as usize;
        let (label, value) = r.spec().cells(self.style);
        let mut v = vec![(LABEL_SLOT, self.texts.set(Self::label_slot(i), &label))];
        if self.style.has_column() {
            let value = value.unwrap_or_default();
            v.push((VALUE_SLOT, self.texts.set(Self::label_slot(i) + 1, &value)));
        }
        v
    }
    fn press(&self, row: i16) -> Chosen {
        let Some(r) = usize::try_from(row).ok().and_then(|i| self.rows.get(i)) else {
            return Chosen::Nothing;
        };
        let pend = |p: Pending| {
            *Self::lock(&self.pending) = Some(p);
            Chosen::Act
        };
        match &r.kind {
            Kind::Action(press) => match press() {
                Then::Stay(line) => Chosen::Stepped(line),
                Then::Open(name) => Chosen::Open(name),
                Then::Close => pend(Pending::Close),
                Then::CloseThen(work) => pend(Pending::CloseThen(work)),
            },
            Kind::Value { values, get, set } => {
                let pick = crate::ui_kit::picker::Pick::new(
                    &r.label,
                    values.clone(),
                    get(),
                    r.description.as_deref().unwrap_or_default(),
                    Arc::clone(set),
                )
                .over(self.dialog.map_or(Opener::MAIN_MENU, |(_, o)| o));
                match crate::ui_kit::picker::offer(pick) {
                    Some(picker) => Chosen::Open(picker),
                    None => Chosen::Stepped(set((get() + 1) % values.len().max(1))),
                }
            }
            Kind::Info(_) => Chosen::Nothing,
            Kind::Text { .. } => pend(Pending::Keyboard(row as usize)),
        }
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        let r = usize::try_from(row).ok().and_then(|i| self.rows.get(i))?;
        let Kind::Value { values, get, set } = &r.kind else {
            return None;
        };
        let n = values.len().max(1);
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
        let i = usize::try_from(row).ok().filter(|i| *i < self.rows.len())?;
        let spec = ScreenSpec {
            focus: i,
            ..self.spec()
        };
        let text = spec.description_text()?;
        Some(self.texts.set(self.description_slot(i), &text))
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.poll_keyboard(m)?;
        self.texts.write(m)
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
        s.list = ListPlace::from(self.layout);
        s.style = self.style;
        s.rows = self.rows.iter().map(Row::spec).collect();
        s.description = self.description_block().map(spec::DescriptionBlock::from);
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
            let list = self
                .layout
                .unwrap_or_else(|| crate::ui_kit::dialog_tag::base_layout(base, self.rows.len()));
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

impl feature::Feature for KitTexts {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(self.id(), "A kit screen's texts").doc("docs/game/ui-screens.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
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
        b.snapshot(feature::SnapshotPolicy::section_forbid_while(
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
    let screen = KitScreen::new("kit-example", "kit.example", IDS, "Example")
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
        .row(Row::info("Status", || "Ready".into()))
        .row(Row::text(
            "Name",
            15,
            || "Chief".into(),
            |t| format!("typed {t:?}"),
        ))
        .row(Row::action("Start", || Then::Close).describe("Start the level."))
        .prompts(&[
            Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
            Prompt::new(&[Glyph::A], "Start"),
            Prompt::new(&[Glyph::B], "Back"),
        ]);
    (screen, grunts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each screen's texts are a feature of their own, so a second kit
    /// screen in the game no longer fails the plan with DuplicateFeature.
    #[test]
    fn two_kit_screens_register_together() {
        let a = Arc::new(example().0);
        let b = Arc::new(crate::ui_kit::screens::choices_example(
            crate::ui_kit::choice::example::store(),
        ));
        let mut r = feature::Registry::new(feature::Switches::defaults());
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
        if let Err(e) = r.build(&feature::Base) {
            panic!("two kit screens conflict: {e:?}");
        }
    }

    /// A dialog screen's spec is a dialog of its rows in its base's list,
    /// values as the base's skin has room for them; its class is a
    /// dialog class on the same list.
    #[test]
    fn a_dialog_screen_is_its_rows_in_its_bases_list() {
        for (base, skin, anchor, style) in [
            (DialogBase::QuickOptions, 2, [-254, 87], ValueStyle::Setting),
            (DialogBase::Large, 0, [-235, 30], ValueStyle::Inline),
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
            // The description in the base's help line: Quick Options' own.
            assert_eq!(
                s.description,
                base.help().map(spec::DescriptionBlock::from),
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
        assert_eq!(t(IDS.id(2)), "< On >");
        s.texts(1);
        assert_eq!(t(IDS.id(4)), "Ready");
        let d = Rows::description(&s, 0).unwrap();
        assert_eq!(t(d), "Grunts' heads pop.", "the legend is the footer's");
        assert_eq!(
            s.spec().footer_text().unwrap(),
            "\u{e10a}  \u{e10b} CHANGE  \u{e100} START  \u{e101} BACK"
        );
        assert!(s.slots() <= IDS.len);
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
                .starts_with("kit.example: \"Example\"; rows: [0] \"Grunts\" = \"< On >\"")
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
        use engine::entry::{EntryFuture, GuestCall as Gc, Regs, Ret, testing};
        use engine::points;
        use engine::testing::FakeGuest;
        use engine::view::ui as vui;

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
            let f = move |cx| -> EntryFuture { Box::pin(destructor(cx, DTOR, free.clone())) };
            let run = testing::run(&f, g, on(SCREEN), |_, _| Regs::default());
            assert_eq!(run.result, Ok(Ret::new(0, 4)));
            assert_eq!(run.calls, expect);
        }
    }

    /// A snapshot waits for a press's action, an open keyboard and work
    /// waiting for a free; after a load every text is written again.
    #[test]
    fn a_snapshot_waits_for_the_screen_and_rewrites_its_texts() {
        use engine::testing::FakeGuest;
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
}
