//! One Forge screen as our list class (docs/game/forge-firefight-ui.md
//! "The screens"): a [`Sheet`] shown on the
//! retail tag its [`Look`] names, drawn by the game's widgets, with the
//! retail list, skin, sounds and animation tracks of that tag.
//!
//! The sheet is set before the screen opens ([`ForgeScreen::show`]); the
//! loader asks [`Rows::count`] first, which writes every text into the
//! screen's slots, so each `TEXT_SET` reads a filled slot (the value
//! picker's rule, docs/game/ui-builder.md "Strings are in guest memory
//! before the screen is built"). A press sends its [`Choice`] to the
//! screen's outbox, which the session drains ([`super::take_choices`]).
//!
//! The Forge menu ([`Kind::Menu`]) changes its sheet while it is up: LB and
//! RB step its tabs ([`ForgeScreen::turn_tab`], [`super::menu`]), and the
//! list grows or shrinks to the new rows at its next draw
//! ([`Rows::growing_capacity`], [`Rows::live_text`]).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use guest_layout::entry::GuestCall;
use guest_layout::memory::{GuestMemory, MemFault};

use super::sheet::{Act, Choice, Kind, Look, NumberField, PickSpec, Sheet};
use crate::list_class::{
    Action, Button, ListClass, ListLayout, LiveText, OnFree, Panel, PanelText, Rows, TextBlock,
};
use crate::ui::{Chosen, LABEL_SLOT, VALUE_SLOT};
use crate::ui_kit::dialog::{
    self, DialogBase, DialogGeometry, DialogSpec, PICKER_HELP_TEXT, PICKER_LIST_ROWS,
};
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::picker::{self, Pick, Picker};
use crate::ui_kit::screen::{SlotTexts, TextPool};
use crate::ui_kit::spec::{Describe, ListPlace, Press, RowSpec, ScreenSpec, ValueStyle};
use crate::ui_kit::view::Opener;

/// Slots: the title, the body, the footer, the focused row's description,
/// then a label and a value per row.
const TITLE: u32 = 0;
const BODY: u32 = 1;
const FOOTER: u32 = 2;
const DESCRIPTION: u32 = 3;
const ROWS: u32 = 4;

/// What the preview draws the dialogs over. They open over the running
/// game, which no preview draws; the main menu stands in for it.
pub const OPENER: Opener = Opener::MAIN_MENU;

/// Choices a screen's presses made, oldest first.
pub type Outbox = Arc<Mutex<VecDeque<Choice>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A question's or a link's list: the large dialog's, moved down so the
/// body has the room above it (the lab's consent dialog's layout).
pub fn question_layout() -> ListLayout {
    let mut l = crate::ui_kit::dialog_tag::base_layout(DialogBase::Large, 2);
    l.anchor[1] = -24;
    l
}

/// The body's text block on the large dialog: where retail's dialogs put
/// their text (`dialog::BODY_TOP`), down to the status line.
fn body_block() -> PanelText {
    let g = DialogGeometry::fallback();
    PanelText {
        block: TextBlock {
            rect: [
                dialog::BODY_TOP,
                dialog::LEFT,
                dialog::STATUS.top + 8,
                dialog::RIGHT,
            ],
            font: g.body_font,
            colour: g.body_colour,
            animation: 0,
        },
        right: false,
        bias: 0,
    }
}

pub struct ForgeScreen {
    kind: Kind,
    sheet: Arc<Mutex<Sheet>>,
    pub(super) texts: SlotTexts,
    outbox: Outbox,
    /// A press left the screen to close: its action closes it.
    closing: AtomicBool,
    /// A value of a closing pick was chosen: this screen closes too.
    close_after_pick: Arc<AtomicBool>,
    /// The row to focus once the pane has built its list.
    opening_focus: AtomicU32,
    /// Between its opening and its free.
    up: AtomicBool,
    /// Its screen, once its pane is built (0: none).
    screen: AtomicU32,
    /// The sheet changed while the screen is up (the Forge menu's tabs):
    /// the next draw sizes the list and sets the texts again.
    redraw: AtomicBool,
}

impl ForgeScreen {
    pub fn new(kind: Kind) -> Self {
        Self::with_texts(kind, SlotTexts::new(kind.family()))
    }

    /// With its texts placed in `pool`, shared with the other screens.
    pub fn pooled(kind: Kind, pool: Arc<TextPool>) -> Self {
        Self::with_texts(kind, SlotTexts::pooled(kind.family(), pool))
    }

    fn with_texts(kind: Kind, texts: SlotTexts) -> Self {
        ForgeScreen {
            kind,
            sheet: Arc::new(Mutex::new(Sheet {
                kind,
                title: String::new(),
                body: String::new(),
                rows: Vec::new(),
                focus: 0,
            })),
            texts,
            outbox: Arc::default(),
            closing: AtomicBool::new(false),
            close_after_pick: Arc::new(AtomicBool::new(false)),
            opening_focus: AtomicU32::new(u32::MAX),
            up: AtomicBool::new(false),
            screen: AtomicU32::new(0),
            redraw: AtomicBool::new(false),
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// What it shows from its next opening. A sheet of another kind is
    /// refused by name.
    pub fn show(&self, sheet: Sheet) -> Result<(), String> {
        if sheet.kind != self.kind {
            return Err(format!(
                "{}: a {:?} sheet is not this screen's",
                self.kind.screen(),
                sheet.kind
            ));
        }
        if sheet.rows.len() > self.max_rows() {
            return Err(format!(
                "{}: {} rows, {} fit its text slots",
                self.kind.screen(),
                sheet.rows.len(),
                self.max_rows()
            ));
        }
        *lock(&self.sheet) = sheet;
        Ok(())
    }

    pub fn sheet(&self) -> Sheet {
        lock(&self.sheet).clone()
    }

    /// Whether it is up (opened and not yet freed).
    pub fn is_up(&self) -> bool {
        self.up.load(Ordering::SeqCst)
    }

    /// Its screen, once its pane is built.
    pub fn screen(&self) -> Option<u32> {
        Some(self.screen.load(Ordering::SeqCst)).filter(|s| *s != 0)
    }

    /// The choices its presses made since the last take.
    pub fn take_choices(&self) -> Vec<Choice> {
        lock(&self.outbox).drain(..).collect()
    }

    /// Whether a closing pick chose since the last ask: the screen is to
    /// close now (the object menu once its object is chosen).
    pub fn take_close_after_pick(&self) -> bool {
        self.close_after_pick.swap(false, Ordering::SeqCst)
    }

    /// Asks this screen to close at the next UI update, as a closing pick
    /// does: its dialog chose something that leaves it with nothing to show.
    pub fn close_with_dialog(&self) {
        self.close_after_pick.store(true, Ordering::SeqCst);
    }

    /// The Forge menu's tab after (`right`) or before the one it shows,
    /// its rows from the session's facts, shown in place: the tab now
    /// showing.
    pub fn turn_tab(&self, right: bool) -> super::menu::Tab {
        let tab = super::menu::tab_of(&self.sheet()).next(right);
        *lock(&self.sheet) = super::menu::sheet(tab, &super::facts());
        self.redraw.store(true, Ordering::SeqCst);
        tab
    }

    /// The rows its text slots hold, and the in-game list's lines.
    pub fn max_rows(&self) -> usize {
        let slots = ((self.kind.family().len - ROWS) / 2) as usize;
        match self.kind.look() {
            Look::List => slots.min((crate::list_class::LINES_PER_CLASS / 2) as usize),
            Look::Floating => slots
                .min((crate::list_class::LINES_PER_CLASS / 2) as usize)
                .min(crate::ui_kit::floating::MAX_ACTIONS),
            _ => slots,
        }
    }

    fn label_slot(row: usize) -> u32 {
        ROWS + 2 * row as u32
    }

    fn style(&self) -> ValueStyle {
        match self.kind.look() {
            Look::List => ValueStyle::Column,
            // The default skin's items have one text.
            Look::Floating => ValueStyle::Inline,
            Look::Picker | Look::Question => DialogBase::Large.style(),
        }
    }

    /// The legend: A acts on every screen but the Controls page, which
    /// only lists, and a sheet of facts alone (the Map variants page of a
    /// map with none).
    pub fn prompts(&self) -> Vec<Prompt> {
        self.prompts_at(None)
    }

    /// The legend with row `row` focused. On a dialog a fact row takes no
    /// A, so its legend is Back alone; a list keeps the screen's legend.
    /// `None` is the screen's legend as it opens.
    pub fn prompts_at(&self, row: Option<usize>) -> Vec<Prompt> {
        let sheet = self.sheet();
        let on_fact = !matches!(self.kind.look(), Look::List | Look::Floating)
            && row
                .or(Some(sheet.focus))
                .and_then(|i| sheet.rows.get(i))
                .is_some_and(|r| matches!(r.act, Act::Info));
        if on_fact {
            return vec![Prompt::new(&[Glyph::B], "Back")];
        }
        let facts_only = {
            let rows = self.sheet().rows;
            !rows.is_empty() && rows.iter().all(|r| matches!(r.act, Act::Info))
        };
        match self.kind {
            Kind::Controls => vec![Prompt::new(&[Glyph::B], "Back")],
            _ if facts_only => vec![Prompt::new(&[Glyph::B], "Back")],
            _ => vec![
                Prompt::new(&[Glyph::A], "Select"),
                Prompt::new(&[Glyph::B], "Back"),
            ],
        }
    }

    fn inputs(&self) -> Vec<Input> {
        match self.kind {
            Kind::Controls | Kind::MapLimits => vec![Input::Up, Input::Down, Input::B],
            // Left and Right step the tabs, as LB and RB do.
            Kind::Menu => vec![
                Input::Up,
                Input::Down,
                Input::Left,
                Input::Right,
                Input::A,
                Input::B,
            ],
            _ => vec![Input::Up, Input::Down, Input::A, Input::B],
        }
    }

    /// Fills every slot from the sheet. Asked first as the screen opens
    /// ([`Rows::count`]), before the loader writes the texts.
    fn refresh(&self) -> Sheet {
        let s = self.sheet();
        self.texts.set(TITLE, &s.title);
        self.texts.set(BODY, &s.body.replace('\n', "\r\n"));
        for (i, r) in s.rows.iter().enumerate().take(self.max_rows()) {
            self.texts.set(Self::label_slot(i), &r.label);
            self.texts.set(
                Self::label_slot(i) + 1,
                r.value.as_deref().unwrap_or_default(),
            );
        }
        if let Some(r) = s.rows.get(s.focus) {
            self.texts
                .set(DESCRIPTION, r.description.as_deref().unwrap_or_default());
        }
        s
    }

    fn press_pick(&self, row: usize, label: &str, p: &PickSpec) -> Chosen {
        let (outbox, sheet, close) = (
            Arc::clone(&self.outbox),
            Arc::clone(&self.sheet),
            Arc::clone(&self.close_after_pick),
        );
        let (choices, values, closes) = (p.choices.clone(), p.values.clone(), p.closes);
        let set = Arc::new(move |i: usize| -> String {
            let Some(c) = choices.get(i).cloned() else {
                return format!("value {i} has no choice");
            };
            lock(&outbox).push_back(c.clone());
            if let Some(r) = lock(&sheet).rows.get_mut(row) {
                if r.value.is_some() {
                    r.value = values.get(i).cloned();
                }
                if let Act::Pick(p) = &mut r.act {
                    p.current = i;
                }
            }
            if closes {
                close.store(true, Ordering::SeqCst);
            }
            format!("forge: {c:?}")
        });
        let pick = Pick::new(&p.title, p.values.clone(), p.current, &p.description, set)
            .details(p.details.clone())
            .over(OPENER);
        picker::offer_list(pick).map_or_else(
            || Chosen::Stepped(format!("{label}: nothing to pick from")),
            Chosen::Open,
        )
    }

    /// Its class: the [`Look`]'s retail tag, our rows, A through the
    /// action, B closing, the sheet's focus set once the pane is built.
    pub fn class(self: &Arc<Self>) -> ListClass {
        let me = Arc::clone(self);
        let action: Action = Arc::new(move |cx, screen| {
            let close = me.closing.swap(false, Ordering::SeqCst);
            let name = me.kind.screen();
            Box::pin(async move {
                if close {
                    eprintln!("{name}: closing");
                    crate::ui_kit::dialog_tag::close(&cx, screen).await?;
                }
                Ok(())
            })
        });
        let me = Arc::clone(self);
        let ready: Action = Arc::new(move |cx, screen| {
            me.screen.store(screen, Ordering::SeqCst);
            let row = me.opening_focus.swap(u32::MAX, Ordering::SeqCst);
            Box::pin(async move {
                if row != u32::MAX {
                    let list = screen.wrapping_add(guest_layout::view::ui::LIST);
                    cx.call(
                        GuestCall::new(crate::list_class::LIST_SET_ROW)
                            .eax(list)
                            .args([row]),
                    )
                    .await?;
                }
                Ok(())
            })
        });
        let me = Arc::clone(self);
        let freed: OnFree = Arc::new(move |_, screen| {
            // Another opening of the class may have started since; only
            // this screen's free (or one before any pane) ends it.
            let ours = me.screen.load(Ordering::SeqCst);
            if ours == 0 || ours == screen {
                me.screen.store(0, Ordering::SeqCst);
                me.up.store(false, Ordering::SeqCst);
            }
            if me.kind == Kind::Coordinates {
                super::send(super::sheet::Choice::CoordinatesDone);
            }
            if me.kind == Kind::ObjectActions {
                super::object_menu::closed();
            }
            eprintln!("{}: {screen:#x} is freed", me.kind.screen());
            Box::pin(async { Ok(()) })
        });
        let footer = self.texts.set(
            FOOTER,
            &crate::ui_kit::input::footer_legend(&self.prompts()),
        );
        let c = ListClass::with_rows(
            self.kind.feature(),
            self.kind.screen(),
            Arc::clone(self) as Arc<dyn Rows>,
        )
        .action(action)
        .pane_ready(ready)
        .on_free(freed)
        .action_buttons(&[])
        .footer(footer);
        match self.kind.look() {
            Look::Picker => c
                .dialog(DialogBase::Large)
                .layout(Picker::layout())
                .description(PICKER_HELP_TEXT),
            Look::Question => c
                .dialog(DialogBase::Large)
                .layout(question_layout())
                .panel(Panel {
                    art: Vec::new(),
                    texts: vec![body_block()],
                }),
            Look::List if self.kind == Kind::Coordinates => c.lines().sideways(),
            Look::List if self.kind == Kind::Menu => {
                let me = Arc::clone(self);
                c.lines()
                    .panel(super::menu::panel())
                    .buttons(Arc::new(move |kind, button| {
                        match super::menu::turn_of(kind, button) {
                            Some(right) => {
                                let tab = me.turn_tab(right);
                                eprintln!("{}: {}", me.kind.screen(), tab.label());
                                Button::Handled
                            }
                            None => Button::Pass,
                        }
                    }))
            }
            Look::List => c.lines(),
            // No frame art and the rows at the menu's point; the point of
            // each opening is written as it opens (`object_menu::place`).
            Look::Floating => c
                .lines()
                .layout(super::object_menu::layout(crate::ui_kit::floating::MAX_ACTIONS)),
        }
    }
}

impl Rows for ForgeScreen {
    fn title(&self) -> u32 {
        self.refresh();
        self.texts.family.id(TITLE)
    }
    fn count(&self) -> usize {
        self.refresh().rows.len().min(self.max_rows())
    }
    fn content_spec(&self) -> Option<ScreenSpec> {
        Some(self.spec())
    }
    /// The Forge menu's list has room for its longest tab and takes each
    /// tab's rows in place.
    fn growing_capacity(&self) -> Option<usize> {
        (self.kind == Kind::Menu).then(|| self.max_rows())
    }
    fn shrink_growing_rows(&self) -> bool {
        self.kind == Kind::Menu
    }
    fn live_text(&self) -> LiveText {
        LiveText {
            writes: Vec::new(),
            redraw: self.redraw.swap(false, Ordering::SeqCst),
        }
    }
    /// The footer follows the focused row, as the description does: a fact
    /// row on a dialog takes no A, so its footer reads Back alone.
    fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
        let row = row.and_then(|r| usize::try_from(r).ok());
        let legend = crate::ui_kit::input::footer_legend(&self.prompts_at(row));
        Some(self.texts.set(FOOTER, &legend))
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let s = lock(&self.sheet);
        let Some((i, r)) = usize::try_from(row)
            .ok()
            .filter(|i| *i < self.max_rows())
            .and_then(|i| s.rows.get(i).map(|r| (i, r)))
        else {
            return Vec::new();
        };
        let (label, value) = row_spec(r, false).cells(self.style());
        let mut v = vec![(LABEL_SLOT, self.texts.set(Self::label_slot(i), &label))];
        if self.style().has_column() {
            let value = value.unwrap_or_default();
            v.push((VALUE_SLOT, self.texts.set(Self::label_slot(i) + 1, &value)));
        }
        v
    }
    /// In a game the Controller Settings items have one text: label and
    /// value on one line, two spaces apart, as retail's settings rows read.
    fn line(&self, row: i16) -> Option<String> {
        if !matches!(self.kind.look(), Look::List | Look::Floating) {
            return None;
        }
        let s = lock(&self.sheet);
        let r = s.rows.get(usize::try_from(row).ok()?)?;
        Some(match &r.value {
            Some(v) if !v.is_empty() => format!("{}  {v}", r.label),
            _ => r.label.clone(),
        })
    }
    fn press(&self, row: i16) -> Chosen {
        let s = self.sheet();
        let Some((i, r)) = usize::try_from(row)
            .ok()
            .and_then(|i| s.rows.get(i).map(|r| (i, r)))
        else {
            return Chosen::Nothing;
        };
        match &r.act {
            Act::Info => Chosen::Nothing,
            Act::Close => {
                self.closing.store(true, Ordering::SeqCst);
                Chosen::Act
            }
            Act::Choose(c) => {
                if self.kind == Kind::Variant
                    && matches!(c, Choice::Load(_) | Choice::DeleteVariant(_))
                {
                    super::screen(Kind::Variants).close_with_dialog();
                }
                lock(&self.outbox).push_back(c.clone());
                self.closing.store(true, Ordering::SeqCst);
                Chosen::Act
            }
            Act::Pick(p) => self.press_pick(i, &r.label, p),
            Act::Tick(t) => {
                let mut sheet = lock(&self.sheet);
                super::games::tick(&mut sheet, *t);
                let now = sheet.rows.get(i).and_then(|r| r.value.clone());
                Chosen::Stepped(format!("{}: {}", r.label, now.unwrap_or_default()))
            }
            Act::Done => {
                let verdict = super::games::done(&mut lock(&self.sheet));
                match verdict {
                    Ok(set) => {
                        lock(&self.outbox).push_back(Choice::Games(set));
                        self.closing.store(true, Ordering::SeqCst);
                        Chosen::Act
                    }
                    Err(why) => Chosen::Stepped(format!("{}: {why}", self.kind.screen())),
                }
            }
            Act::Variant(file, sheet) => {
                super::target_variant(file.clone());
                match super::push((**sheet).clone()) {
                    Ok(chosen) => chosen,
                    Err(why) => {
                        Chosen::Stepped(format!("{}: not opened: {why}", sheet.kind.screen()))
                    }
                }
            }
            Act::Dialog(sheet) => match super::push((**sheet).clone()) {
                Ok(chosen) => chosen,
                Err(why) => Chosen::Stepped(format!("{}: not opened: {why}", sheet.kind.screen())),
            },
            Act::Type(spec) => super::ask_keyboard(spec.clone()),
            Act::SaveAs => match super::ask_name() {
                Ok(()) => {
                    self.closing.store(true, Ordering::SeqCst);
                    Chosen::Act
                }
                Err(why) => Chosen::Stepped(why),
            },
            // Above this screen on its window and slot, so B comes back.
            Act::Open(k) if *k == Kind::Coordinates => {
                match super::push(super::screen(*k).sheet()) {
                    Ok(chosen) => chosen,
                    Err(why) => Chosen::Stepped(format!("{}: not opened: {why}", k.screen())),
                }
            }
            Act::Open(k) => match super::prepare(*k) {
                Ok(()) => Chosen::Push(k.screen()),
                Err(why) => Chosen::Stepped(format!("{}: not opened: {why}", k.screen())),
            },
            Act::Waves(page) => {
                let name = page.kind().screen();
                match super::waves::open(*page) {
                    Ok(()) => Chosen::Push(name),
                    Err(why) => Chosen::Stepped(format!("{name}: not opened: {why}")),
                }
            }
        }
    }

    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        let s = self.sheet();
        let r = s.rows.get(usize::try_from(row).ok()?)?;
        let Act::Type(spec) = &r.act else {
            return None;
        };
        if let NumberField::Size(field) = spec.field {
            // A size nudges by the block-out grid.
            let current = r.value.as_ref()?.parse::<i64>().ok()?;
            let grid = i64::from(super::facts().block_grid.max(1));
            let value = (current + if right { grid } else { -grid }).max(0);
            lock(&self.sheet)
                .rows
                .get_mut(usize::try_from(row).ok()?)?
                .value = Some(value.to_string());
            lock(&self.outbox).push_back(Choice::NudgeSize(field, right));
            return Some(format!("{}: {value}", r.label));
        }
        let NumberField::Coordinate(axis) = spec.field else {
            return None;
        };
        let current = r.value.as_ref()?.parse::<f32>().ok()?;
        let facts = super::facts();
        let step = if axis.is_angle() {
            super::sheet::ANGLE_STEPS[facts.rotation_snap].max(1.0)
        } else {
            super::sheet::MOVE_STEPS[facts.movement_snap].max(0.01)
        };
        let value = current + if right { step } else { -step };
        lock(&self.sheet)
            .rows
            .get_mut(usize::try_from(row).ok()?)?
            .value = Some(super::sheet::number(value));
        lock(&self.outbox).push_back(Choice::Nudge(axis, right));
        Some(format!("{}: {value}", axis.label()))
    }

    fn description(&self, row: i16) -> Option<u32> {
        // The menu's description widget stands where its strip's first
        // text does (`ListClass::panel`): it stays empty.
        if self.kind == Kind::Menu {
            return None;
        }
        let s = lock(&self.sheet);
        let r = s.rows.get(usize::try_from(row).ok()?)?;
        Some(
            self.texts
                .set(DESCRIPTION, r.description.as_deref().unwrap_or_default()),
        )
    }
    fn panel_text(&self, i: usize) -> Option<u32> {
        if self.kind == Kind::Menu {
            // The strip: every text is set at each change of tab, the
            // ones not showing to an empty string.
            let strip = super::menu::strip(super::menu::tab_of(&lock(&self.sheet)));
            return strip.get(i).map(|text| super::tab_text(i, text));
        }
        (i == 0 && self.kind.look() == Look::Question).then(|| self.texts.family.id(BODY))
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        if self.kind == Kind::Menu {
            super::write_tab_texts(m)?;
        }
        self.texts.write(m)
    }
    /// Asked as the screen opens: it is up, and its focus is set once the
    /// pane is built.
    fn describe(&self, count: usize) -> Option<String> {
        self.up.store(true, Ordering::SeqCst);
        self.closing.store(false, Ordering::SeqCst);
        let spec = self.spec();
        let row = if spec.focus < count {
            spec.focus as u32
        } else {
            u32::MAX
        };
        self.opening_focus.store(row, Ordering::SeqCst);
        Some(spec.log_line())
    }
}

/// A sheet row as the kit's row: a picked value opens the value picker, a
/// fact is an info row, an answer is the screen's action. `described`
/// keeps its description (a screen with a description block).
fn row_spec(r: &super::sheet::SheetRow, described: bool) -> RowSpec {
    let value = r.value.clone().unwrap_or_default();
    let row = match &r.act {
        Act::Pick(_) if r.value.is_some() => {
            RowSpec::value(&r.label, &[value.as_str()], 0).press(Press::Open(picker::NAME.into()))
        }
        Act::Pick(_) => RowSpec::action(&r.label).press(Press::Open(picker::NAME.into())),
        Act::Type(_) if r.value.is_some() => {
            RowSpec::value(&r.label, &[value.as_str()], 0).press(Press::Act)
        }
        Act::Type(_) => RowSpec::action(&r.label).press(Press::Act),
        Act::Open(k) => RowSpec::action(&r.label).press(Press::Open(k.screen().into())),
        Act::Waves(page) if r.value.is_some() => RowSpec::value(&r.label, &[value.as_str()], 0)
            .press(Press::Open(page.kind().screen().into())),
        Act::Waves(page) => {
            RowSpec::action(&r.label).press(Press::Open(page.kind().screen().into()))
        }
        Act::Dialog(sheet) => {
            RowSpec::action(&r.label).press(Press::Open(sheet.kind.screen().into()))
        }
        Act::Variant(_, sheet) => RowSpec::value(&r.label, &[value.as_str()], 0)
            .press(Press::Open(sheet.kind.screen().into())),
        Act::Tick(_) => RowSpec::value(&r.label, &[value.as_str()], 0).press(Press::Act),
        Act::Info => {
            // A fact takes no A: its legend is Back alone.
            let mut row = RowSpec::info(&r.label, &value);
            row.prompts = Some(vec![Prompt::new(&[Glyph::B], "Back")]);
            row
        }
        Act::Choose(_) | Act::Close | Act::Done | Act::SaveAs => {
            RowSpec::action(&r.label).press(Press::Act)
        }
    };
    match (&r.description, described) {
        (Some(d), true) => row.describe(d),
        _ => row,
    }
}

impl Describe for ForgeScreen {
    fn spec(&self) -> ScreenSpec {
        let sheet = self.sheet();
        let look = self.kind.look();
        let mut s = ScreenSpec::new(self.kind.screen(), &sheet.title);
        s.style = self.style();
        for (i, row) in sheet.rows.iter().enumerate() {
            if row.value_unavailable {
                s.content.unavailable_values.push(format!("row.{i}.value"));
            }
            s.rows.push(row_spec(row, look == Look::Picker));
        }
        s.focus = sheet.focus.min(s.rows.len().saturating_sub(1));
        // A dialog's legend follows its focused row: a fact takes no A.
        s.prompts = self.prompts_at(Some(sheet.focus));
        s.inputs = self.inputs();
        s.footer = true;
        match look {
            Look::Picker => {
                s.list = ListPlace::from(Some(Picker::layout()));
                s.description = Some(PICKER_HELP_TEXT.into());
                s.scrolls = s.rows.len() > PICKER_LIST_ROWS;
                s.dialog = Some(DialogSpec {
                    base: DialogBase::Large,
                    rows: s.rows.clone(),
                    ..DialogSpec::new("", OPENER)
                });
            }
            Look::Question => {
                s.list = ListPlace::from(Some(question_layout()));
                s.dialog = Some(DialogSpec {
                    base: DialogBase::Large,
                    rows: s.rows.clone(),
                    ..DialogSpec::new(&sheet.body, OPENER)
                });
            }
            Look::List => {
                s.scrolls = s.rows.len() > ListPlace::Controls.shown();
                if self.kind == Kind::Menu {
                    s.texts = super::menu::pane_texts(super::menu::tab_of(&sheet));
                }
            }
            Look::Floating => {
                s.list = ListPlace::from(Some(super::object_menu::layout(s.rows.len())));
                s.transparent = true;
            }
        }
        s
    }
}
