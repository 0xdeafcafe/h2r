//! The value picker (docs/game/ui-builder.md "The value picker"): A on a
//! setting row opens a dialog of that setting's values, as the game's own
//! variant editor and Quick Options do. Its title is the setting's name in
//! capitals, its list every value (more than six scroll), the current one
//! focused, the setting's description on the right, and `(A) SELECT
//! (B) CANCEL` in the footer. A sets the focused value through the row's
//! own setter (the settings store, so it is saved and its change
//! callbacks run) and closes; B closes and changes nothing. Left and right
//! on the row itself still step, as a shortcut.
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
//! it. Only one screen takes input at a time, so one is enough.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine::entry::{Cx, EntryError};
use engine::memory::{GuestMemory, MemFault};
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
/// The most values a picker lists: its family's slots past the three
/// fixed ones. A row with more keeps stepping on A.
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
    /// The value the setting holds: the one focused when the list is
    /// drawn offline (in the game, see docs/game/ui-builder.md "The value
    /// picker").
    pub current: usize,
    /// What the setting does, on the right.
    pub description: String,
    /// What the picker opened over, for the preview.
    pub opener: Opener,
    /// Stores value `i`; what to log.
    pub set: Arc<dyn Fn(usize) -> String + Send + Sync>,
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
            opener: Opener::MAIN_MENU,
            set,
        }
    }
    pub fn over(mut self, opener: Opener) -> Pick {
        self.opener = opener;
        self
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

#[cfg(not(test))]
static PICK: Mutex<Option<Pick>> = Mutex::new(None);

#[cfg(not(test))]
fn with_pick<T>(f: impl FnOnce(&mut Option<Pick>) -> T) -> T {
    f(&mut PICK.lock().unwrap_or_else(|e| e.into_inner()))
}

// Tests run side by side, each on its own thread: each has its own pick,
// so one test's row can't swap another's.
#[cfg(test)]
thread_local! {
    static PICK: Mutex<Option<Pick>> = const { Mutex::new(None) };
}

#[cfg(test)]
fn with_pick<T>(f: impl FnOnce(&mut Option<Pick>) -> T) -> T {
    PICK.with(|p| f(&mut p.lock().unwrap_or_else(|e| e.into_inner())))
}

/// The row's pick, when the picker can show it ([`MIN_CHOICES`] to
/// [`MAX_CHOICES`] values): it becomes the picker's, and the screen to
/// open is [`NAME`] (`crate::list_class::open` takes [`GAME_NAME`] in a
/// game). `None` leaves A to step the row.
pub fn offer(p: Pick) -> Option<&'static str> {
    if !(MIN_CHOICES..=MAX_CHOICES).contains(&p.values.len()) {
        return None;
    }
    with_pick(|held| *held = Some(p));
    Some(NAME)
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
        let footer = self
            .texts
            .set(FOOTER, &crate::ui_kit::input::footer_legend(&prompts()));
        ListClass::with_rows(self.feature, self.name, Arc::clone(self) as Arc<dyn Rows>)
            .dialog(self.base)
            .layout(Self::layout())
            .description(PICKER_HELP_TEXT)
            .action(action)
            .action_buttons(&[])
            .footer(footer)
    }

    async fn act(&self, cx: &Cx, screen: u32) -> Result<(), EntryError> {
        if self.chosen.swap(false, Ordering::SeqCst) {
            eprintln!("{}: chosen; closing", self.name);
            crate::ui_kit::dialog_tag::close(cx, screen).await?;
        }
        Ok(())
    }

    fn value_slot(row: usize) -> u32 {
        VALUES + row as u32
    }

    /// Every text of the pick showing now in its slot: the title, the
    /// description and each value. The class is built before any pick
    /// (its title id is taken then), and `TEXT_SET` copies a string as it
    /// sets it, so the loader has them all written before the screen is
    /// built ([`Rows::count`] is asked first, then [`Rows::write_texts`]):
    /// a slot set later than it is named shows what it held before,
    /// nothing on the first open (the picker opened blank, 2026-11-19).
    fn refresh(&self) -> Option<Pick> {
        let p = current();
        let (title, description) = p
            .as_ref()
            .map(|p| (p.title.as_str(), p.description.as_str()))
            .unwrap_or_default();
        self.texts.set(TITLE, title);
        self.texts.set(DESCRIPTION, description);
        for (i, v) in p.iter().flat_map(|p| p.values.iter()).enumerate() {
            self.texts.set(Self::value_slot(i), v);
        }
        p
    }

    /// Its text slots (a test points them at guest memory).
    #[cfg(test)]
    pub(crate) fn slot_texts(&self) -> &SlotTexts {
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
    /// The title's slot, the same for every pick: [`Picker::refresh`]
    /// fills it as the screen opens.
    fn title(&self) -> u32 {
        self.refresh();
        self.texts.family.id(TITLE)
    }
    /// Asked first as the screen opens: every text of the pick is set
    /// here, for the loader's write.
    fn count(&self) -> usize {
        self.refresh().map_or(0, |p| p.values.len())
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some(p) = current() else {
            return Vec::new();
        };
        let Some(v) = usize::try_from(row).ok().and_then(|i| p.values.get(i)) else {
            return Vec::new();
        };
        vec![(
            LABEL_SLOT,
            self.texts.set(Self::value_slot(row as usize), v),
        )]
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
        eprintln!("{}: {}", self.name, (p.set)(i));
        with_pick(|held| {
            if let Some(held) = held.as_mut() {
                held.current = i;
            }
        });
        self.chosen.store(true, Ordering::SeqCst);
        Chosen::Act
    }
    fn description(&self, _row: i16) -> Option<u32> {
        let p = current()?;
        Some(self.texts.set(DESCRIPTION, &p.description))
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.texts.write(m)
    }
    fn describe(&self, _count: usize) -> Option<String> {
        self.chosen.store(false, Ordering::SeqCst);
        Some(self.spec().log_line())
    }
}

impl Describe for Picker {
    fn spec(&self) -> ScreenSpec {
        let p = current();
        let title = p.as_ref().map(|p| p.title.clone()).unwrap_or_default();
        let mut s = ScreenSpec::new(self.name, &title);
        s.list = ListPlace::from(Some(Self::layout()));
        s.style = self.base.style();
        s.description = Some(PICKER_HELP_TEXT.into());
        s.prompts = prompts();
        s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
        s.footer = true;
        if let Some(p) = &p {
            s.rows = p
                .values
                .iter()
                .map(|v| RowSpec::action(v).describe(&p.description))
                .collect();
            s.focus = p.current;
            s.scrolls = p.values.len() > PICKER_LIST_ROWS;
        }
        s.dialog = Some(DialogSpec {
            base: self.base,
            rows: s.rows.clone(),
            ..DialogSpec::new("", p.map_or(Opener::MAIN_MENU, |p| p.opener))
        });
        s
    }
}

/// The picker's texts as a locale source, as [`crate::ui_kit::screen::KitTexts`]
/// is a kit screen's.
pub struct PickerTexts(pub Arc<Picker>);

impl feature::Feature for PickerTexts {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(self.0.texts_id, "The value picker's texts")
            .doc("docs/game/ui-builder.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let s = &self.0;
        let len = s.texts.family.len * SLOT_LEN;
        let base = b.guest_alloc(len.next_multiple_of(0x1000), Permissions::RW)?;
        s.texts.base.store(base, Ordering::SeqCst);
        let me = Arc::clone(s);
        b.locale().source(s.name, move |id| me.texts.address(id));
        let load = Arc::clone(s);
        b.snapshot(feature::SnapshotPolicy::section_forbid_while(
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
pub fn install(registry: &mut feature::Registry) -> anyhow::Result<()> {
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
