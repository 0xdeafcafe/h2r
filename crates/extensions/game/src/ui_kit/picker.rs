//! The value picker (docs/game/ui-builder.md "The value picker"): A on a
//! setting row opens a dialog of that setting's values, as the game's own
//! variant editor and Quick Options do. Its title is the setting's name in
//! capitals, its list every value (more than six scroll), the current one
//! focused, the setting's description on the right, and `(A) SELECT
//! (B) CANCEL` in the footer. A sets the focused value through the row's
//! own setter (the settings store, so it is saved and its change
//! callbacks run) and closes; a pick with an action of its own
//! ([`Pick::after`], the lobby's mode switch) runs it once closed. B closes
//! and changes nothing. Left and right on the row itself still step, as a
//! shortcut.
//!
//! **Retail's.** The dialog is `low_level_settings` (screen 223,
//! `ui\screens\game_shell\settings_screen\variant_settings\
//! editing_format_screens\low_level_settings`), read off `mainmenu.map`
//! (2026-11-19): root flags 8 (header layout 2, the large dialog's title
//! and footer), one pane (animation 23) holding `error_dialog_large`'s
//! four panel blocks (`ul_07`, `br_07`, `dialog1`, `dialog2`), a list on
//! `default6` (skin 22, the default skin with its scroll arrows at its
//! left) showing six rows from (-231, 30), and one text block,
//! `description`, at `[74 15 -80 250]` in font 1 and the pale blue. Every
//! game setting is a `sily` tag with a block of options (on and off ones
//! too: 49 of the 92 have two), and the editor opens this one dialog for
//! each, so a two-value row opens it too ([`offer`]).
//!
//! **Where.** The tag is in `mainmenu.map` only, so the main menu's
//! picker ([`NAME`]) is built on it ([`DialogBase::ValuePicker`]); in a
//! game ([`GAME_NAME`]) it is the large dialog (screen 240, in the game
//! maps' interfaces) with the same list, skin and description block,
//! `low_level_settings`' own layout on the same panel.
//! `crate::list_class::open` picks which.
//!
//! **One at a time.** The row being picked for is a process-wide
//! [`Pick`]: a row sets it as it opens the picker, and both pickers show
//! it. The pane that takes the offer owns it until it is freed or another
//! row offers a replacement.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::entry::{Cx, EntryError};
use guest_layout::memory::{GuestMemory, MemFault};
use runtime::guest::Permissions;

use crate::list_class::{Action, ListClass, ListLayout, Rows, TextBlock};
use crate::ui::{Chosen, LABEL_SLOT};
use crate::ui_kit::dialog::{
    DialogBase, DialogSpec, PICKER_HELP_TEXT, PICKER_LIST, PICKER_LIST_ROWS,
};
use crate::ui_kit::ids;
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::screen::{SLOT_LEN, SlotTexts};
use crate::ui_kit::spec::{Describe, ListPlace, RowSpec, ScreenSpec};
use crate::ui_kit::view::Opener;

/// The main menu's picker: a dialog on `low_level_settings` itself.
pub const NAME: &str = "kit.picker";
/// A game's picker: the large dialog laid out as `low_level_settings`.
pub const GAME_NAME: &str = "kit.picker.game";
const FEATURE: &str = "kit-picker";
const GAME_FEATURE: &str = "kit-picker-game";

/// Slots: the title, the footer, the description, then a value each.
const TITLE: u32 = 0;
const FOOTER: u32 = 1;
const DESCRIPTION: u32 = 2;
const VALUES: u32 = 3;
/// Value slots available after the three fixed texts. A settings row
/// with more choices keeps its existing step-on-A behavior.
pub const MAX_CHOICES: usize = 0x40 - VALUES as usize;
/// The fewest: a two-value row opens it too, as retail's On and Off
/// settings do.
pub const MIN_CHOICES: usize = 2;

/// The legend, as retail's picker has it.
pub fn prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(&[Glyph::A], "Select"),
        Prompt::new(&[Glyph::B], "Cancel"),
    ]
}

/// What the picker is picking for: the row's setting.
#[derive(Clone)]
pub struct Pick {
    /// The dialog's title: the setting's name in capitals.
    pub title: String,
    pub values: Vec<String>,
    /// The held value, focused when the picker opens.
    pub current: usize,
    /// What the setting does, on the right.
    pub description: String,
    /// A line per value, shown on the right while it has the focus in
    /// place of `description` (the object menu's object lines). Empty:
    /// `description` for every value.
    pub details: Vec<String>,
    /// Values that cannot be chosen (the map variant picker's unusable
    /// ones): A on one stays in the picker and says its line. Empty or
    /// shorter than `values`: the rest can be.
    pub blocked: Vec<bool>,
    /// What the picker opened over, for the preview.
    pub opener: Opener,
    /// Stores value `i`; what to log.
    pub set: Arc<dyn Fn(usize) -> String + Send + Sync>,
    /// Run once a value is set and the picker has closed, with the guest
    /// (the lobby's mode switch, `crate::lobby_tabs::picker::after`):
    /// what a value does that needs the game's own routines.
    pub after: Option<Action>,
}

impl std::fmt::Debug for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pick")
            .field("title", &self.title)
            .field("values", &self.values)
            .field("current", &self.current)
            .finish_non_exhaustive()
    }
}

impl Pick {
    /// A pick for a row labelled `label` ("Score to win round"): its
    /// title in capitals, the setting's `values`, the one held now.
    pub fn new(
        label: &str,
        values: Vec<String>,
        current: usize,
        description: &str,
        set: Arc<dyn Fn(usize) -> String + Send + Sync>,
    ) -> Pick {
        Pick {
            title: title(label),
            current: current.min(values.len().saturating_sub(1)),
            values,
            description: description.into(),
            details: Vec::new(),
            blocked: Vec::new(),
            opener: Opener::MAIN_MENU,
            set,
            after: None,
        }
    }
    pub fn over(mut self, opener: Opener) -> Pick {
        self.opener = opener;
        self
    }
    /// What runs once a value is set and the picker has closed
    /// ([`Pick::after`]).
    pub fn after(mut self, after: Action) -> Pick {
        self.after = Some(after);
        self
    }
    /// A line per value ([`Pick::details`]).
    pub fn details(mut self, details: Vec<String>) -> Pick {
        self.details = details;
        self
    }
    /// The values that cannot be chosen ([`Pick::blocked`]).
    pub fn blocked(mut self, blocked: Vec<bool>) -> Pick {
        self.blocked = blocked;
        self
    }
    /// Whether value `i` cannot be chosen.
    pub fn is_blocked(&self, i: usize) -> bool {
        self.blocked.get(i).copied().unwrap_or(false)
    }
    /// What the right side says while value `i` has the focus.
    pub fn line(&self, i: usize) -> &str {
        self.details
            .get(i)
            .map_or(self.description.as_str(), String::as_str)
    }
}

/// A row's label as the picker's title: in capitals, without a trailing
/// colon or ellipsis.
pub fn title(label: &str) -> String {
    label
        .trim_end_matches("...")
        .trim_end_matches(':')
        .trim()
        .to_uppercase()
}

struct Offer {
    pick: Option<Pick>,
    screen: Option<u32>,
}

const EMPTY: Offer = Offer { pick: None, screen: None };

#[cfg(not(test))]
static PICK: Mutex<Offer> = Mutex::new(EMPTY);

#[cfg(not(test))]
fn with_offer<T>(f: impl FnOnce(&mut Offer) -> T) -> T {
    f(&mut PICK.lock().unwrap_or_else(|e| e.into_inner()))
}

// Tests run side by side, each on its own thread: each has its own pick,
// so one test's row can't swap another's.
#[cfg(test)]
thread_local! {
    static PICK: Mutex<Offer> = const { Mutex::new(EMPTY) };
}

#[cfg(test)]
fn with_offer<T>(f: impl FnOnce(&mut Offer) -> T) -> T {
    PICK.with(|p| f(&mut p.lock().unwrap_or_else(|e| e.into_inner())))
}

fn with_pick<T>(f: impl FnOnce(&mut Option<Pick>) -> T) -> T {
    with_offer(|offer| f(&mut offer.pick))
}

fn freed(screen: u32) {
    with_offer(|offer| {
        if offer.screen == Some(screen) {
            *offer = EMPTY;
        }
    });
}

/// The row's pick, when the picker can show it ([`MIN_CHOICES`] to
/// [`MAX_CHOICES`] values): it becomes the picker's, and the screen to
/// open is [`NAME`] (`crate::list_class::open` takes [`GAME_NAME`] in a
/// game). `None` leaves A to step the row.
pub fn offer(p: Pick) -> Option<&'static str> {
    if !(MIN_CHOICES..=MAX_CHOICES).contains(&p.values.len()) {
        return None;
    }
    offer_list(p)
}

/// A named list (for example the Theater map filter) always opens a
/// dialog, even when its only entry is "All". The guest row index is i16.
pub(crate) fn offer_list(p: Pick) -> Option<&'static str> {
    if p.values.is_empty() || p.values.len() > i16::MAX as usize {
        return None;
    }
    with_offer(|offer| *offer = Offer { pick: Some(p), screen: None });
    Some(NAME)
}

/// The picker's screen, from its pane being built to its free: for a
/// caller that opened it and closes it or moves its cursor itself.
pub(crate) fn screen() -> Option<u32> {
    with_offer(|offer| offer.screen)
}

/// The pick showing now.
pub fn current() -> Option<Pick> {
    with_pick(|p| p.clone())
}

/// The picker that opens for [`NAME`] where the game is: the large
/// dialog's in a game (`low_level_settings` is in `mainmenu.map` only).
pub fn screen_for(name: &'static str, ingame: bool) -> &'static str {
    if name == NAME && ingame {
        GAME_NAME
    } else {
        name
    }
}

/// One of the two pickers (see the module docs).
pub struct Picker {
    name: &'static str,
    feature: &'static str,
    base: DialogBase,
    texts: SlotTexts,
    /// The held row at open, consumed once after the pane builds its list.
    opening_focus: AtomicU32,
    /// A press chose a value: the action closes the dialog.
    chosen: AtomicBool,
    /// Its texts' feature id (`ui-kit-texts.<name>`).
    texts_id: &'static str,
}

impl Picker {
    /// The main menu's ([`NAME`], on `low_level_settings`) or a game's
    /// ([`GAME_NAME`], on the large dialog).
    pub fn new(ingame: bool) -> Picker {
        let (name, feature, base, texts_id) = if ingame {
            (
                GAME_NAME,
                GAME_FEATURE,
                DialogBase::Large,
                "ui-kit-texts.kit.picker.game",
            )
        } else {
            (
                NAME,
                FEATURE,
                DialogBase::ValuePicker,
                "ui-kit-texts.kit.picker",
            )
        };
        Picker {
            name,
            feature,
            base,
            texts: SlotTexts::new(ids::family(name_family(ingame))),
            opening_focus: AtomicU32::new(u32::MAX),
            chosen: AtomicBool::new(false),
            texts_id,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
    pub fn base(&self) -> DialogBase {
        self.base
    }

    /// The list: `low_level_settings`' own, six rows on `default6`.
    pub fn layout() -> ListLayout {
        ListLayout {
            skin: PICKER_LIST.skin,
            anchor: PICKER_LIST.anchor,
            bitmaps_left_of: i16::MIN,
            visible: PICKER_LIST_ROWS as i16,
        }
    }

    /// The class showing it: a dialog on its base with
    /// `low_level_settings`' list and description block; A on a value
    /// sets it and closes, B closes, nothing else is taken.
    pub fn class(self: &Arc<Self>) -> ListClass {
        let me = Arc::clone(self);
        let action: Action = Arc::new(move |cx, screen| {
            let me = Arc::clone(&me);
            Box::pin(async move { me.act(&cx, screen).await })
        });
        let me = Arc::clone(self);
        let focus: Action = Arc::new(move |cx, screen| {
            let row = me.opening_focus.swap(u32::MAX, Ordering::SeqCst);
            Box::pin(async move {
                if row != u32::MAX {
                    with_offer(|offer| offer.screen = Some(screen));
                    let list = screen.wrapping_add(guest_layout::view::ui::LIST);
                    cx.call(
                        guest_layout::entry::GuestCall::new(crate::list_class::LIST_SET_ROW)
                            .eax(list)
                            .args([row]),
                    )
                    .await?;
                }
                Ok(())
            })
        });
        let footer = self
            .texts
            .set(FOOTER, &crate::ui_kit::input::footer_legend(&prompts()));
        ListClass::with_rows(self.feature, self.name, Arc::clone(self) as Arc<dyn Rows>)
            .dialog(self.base)
            .layout(Self::layout())
            .description(PICKER_HELP_TEXT)
            .action(action)
            .pane_ready(focus)
            .on_free(Arc::new(|_, screen| {
                freed(screen);
                Box::pin(async { Ok(()) })
            }))
            .action_buttons(&[])
            .footer(footer)
    }

    async fn act(&self, cx: &Cx, screen: u32) -> Result<(), EntryError> {
        if self.chosen.swap(false, Ordering::SeqCst) {
            eprintln!("{}: chosen; closing", self.name);
            let after = current().and_then(|p| p.after);
            crate::ui_kit::dialog_tag::close(cx, screen).await?;
            if let Some(after) = after {
                after(cx.clone(), screen).await?;
            }
        }
        Ok(())
    }

    fn value_slot(row: usize) -> u32 {
        // TEXT_SET copies each label into its widget. More rows than
        // slots can reuse them as they scroll into the visible list.
        VALUES + (row % MAX_CHOICES) as u32
    }

    /// Fill the title, description and the first window of value slots. The class is built before any pick
    /// (its title id is taken then), and `TEXT_SET` copies a string as it
    /// sets it, so the loader has them all written before the screen is
    /// built ([`Rows::count`] is asked first, then [`Rows::write_texts`]):
    /// a slot set later than it is named shows what it held before,
    /// nothing on the first open (the picker opened blank, 2026-11-19).
    fn refresh(&self) -> Option<Pick> {
        let p = current();
        let (title, description) = p
            .as_ref()
            .map(|p| (p.title.as_str(), p.line(p.current)))
            .unwrap_or_default();
        self.texts.set(TITLE, title);
        self.texts.set(DESCRIPTION, description);
        for (i, v) in p
            .iter()
            .flat_map(|p| p.values.iter())
            .take(MAX_CHOICES)
            .enumerate()
        {
            self.texts.set(Self::value_slot(i), v);
        }
        p
    }

    /// Its text slots (a test points them at guest memory).
    #[cfg(test)]
    pub fn slot_texts(&self) -> &SlotTexts {
        &self.texts
    }
}

const fn name_family(ingame: bool) -> &'static str {
    if ingame {
        "kit.picker.game"
    } else {
        "kit.picker"
    }
}

impl Rows for Picker {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(self.spec())
    }

    /// The title's slot, the same for every pick: [`Picker::refresh`]
    /// fills it as the screen opens.
    fn title(&self) -> u32 {
        self.refresh();
        self.texts.family.id(TITLE)
    }
    /// Asked first as the screen opens, before the loader writes its texts.
    fn count(&self) -> usize {
        self.refresh().map_or(0, |p| p.values.len())
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        with_pick(|p| {
            let Some(i) = usize::try_from(row).ok() else {
                return Vec::new();
            };
            let Some(v) = p.as_ref().and_then(|p| p.values.get(i)) else {
                return Vec::new();
            };
            vec![(LABEL_SLOT, self.texts.set(Self::value_slot(i), v))]
        })
    }
    /// A on a value: set through the row's setter (saved), then the action
    /// closes the dialog.
    fn press(&self, row: i16) -> Chosen {
        let Some(p) = current() else {
            return Chosen::Nothing;
        };
        let Some(i) = usize::try_from(row).ok().filter(|i| *i < p.values.len()) else {
            return Chosen::Nothing;
        };
        if p.is_blocked(i) {
            return Chosen::Stepped(format!("{}: {}", self.name, p.line(i)));
        }
        eprintln!("{}: {}", self.name, (p.set)(i));
        with_pick(|held| {
            if let Some(held) = held.as_mut() {
                held.current = i;
            }
        });
        self.chosen.store(true, Ordering::SeqCst);
        Chosen::Act
    }
    fn description(&self, row: i16) -> Option<u32> {
        with_pick(|p| {
            p.as_ref().map(|p| {
                let line = usize::try_from(row).map_or(p.description.as_str(), |i| p.line(i));
                self.texts.set(DESCRIPTION, line)
            })
        })
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.texts.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        self.chosen.store(false, Ordering::SeqCst);
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

impl Describe for Picker {
    fn spec(&self) -> ScreenSpec {
        let pick = current();
        let (title, values, focus, description, opener) = pick.as_ref().map_or(
            ("", &[][..], 0, "", Opener::MAIN_MENU),
            |p| (p.title.as_str(), p.values.as_slice(), p.current, p.description.as_str(), p.opener),
        );
        let mut s = picker_spec(self.name, self.base, title, values, focus, description, opener);
        if let Some(p) = &pick {
            for (i, row) in s.rows.iter_mut().enumerate() {
                row.description = Some(p.line(i).to_string());
            }
            if let Some(d) = &mut s.dialog {
                d.rows.clone_from(&s.rows);
            }
        }
        s
    }
}

/// Playback owns the dialog and its selection while the replayed guest
/// remains paused. Use the same in-game picker geometry and legend.
pub(crate) fn playback_spec(title: &str, values: &[String], focus: usize, help: &str) -> ScreenSpec {
    picker_spec(GAME_NAME, DialogBase::Large, title, values, focus, help, Opener::MAIN_MENU)
}

pub(crate) fn picker_spec(
    name: &str,
    base: DialogBase,
    title: &str,
    values: &[String],
    focus: usize,
    help: &str,
    opener: Opener,
) -> ScreenSpec {
    let mut s = ScreenSpec::new(name, title);
    s.list = ListPlace::from(Some(Picker::layout()));
    s.style = base.style();
    s.description = Some(PICKER_HELP_TEXT.into());
    s.prompts = prompts();
    s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
    s.footer = true;
    s.content.picker = true;
    s.rows = values.iter().map(|v| RowSpec::action(v).describe(help)).collect();
    s.focus = focus;
    s.scrolls = values.len() > PICKER_LIST_ROWS;
    s.dialog = Some(DialogSpec {
        base,
        rows: s.rows.clone(),
        ..DialogSpec::new("", opener)
    });
    s
}

/// The picker's texts as a locale source, as [`crate::ui_kit::screen::KitTexts`]
/// is a kit screen's.
pub struct PickerTexts(pub Arc<Picker>);

impl hooks::Feature for PickerTexts {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(self.0.texts_id, "The value picker's texts")
            .doc("docs/game/ui-builder.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let s = &self.0;
        let len = s.texts.family.len * SLOT_LEN;
        let base = b.guest_alloc(len.next_multiple_of(0x1000), Permissions::RW)?;
        s.texts.base.store(base, Ordering::SeqCst);
        let me = Arc::clone(s);
        b.locale().source(s.name, move |id| me.texts.address(id));
        let load = Arc::clone(s);
        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            || None,
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

/// Adds both pickers and their texts.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    for ingame in [false, true] {
        let p = Arc::new(Picker::new(ingame));
        registry
            .add(&PickerTexts(Arc::clone(&p)))
            .map_err(|e| anyhow::anyhow!("{} texts: {e}", p.name))?;
        registry
            .add(&p.class())
            .map_err(|e| anyhow::anyhow!("{}: {e}", p.name))?;
    }
    Ok(())
}

/// The value picker's description block, for a caller placing it.
pub const HELP: TextBlock = PICKER_HELP_TEXT;

#[cfg(test)]
mod tests;
