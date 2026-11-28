//! The Forge screens in the game (docs/game/forge-firefight-ui.md "The
//! screens"): the object menu, Properties, Tools, Tool settings, the Link
//! dialog, the questions, the Forge menu and Controls, each a list class
//! of ours on a retail screen tag ([`sheet::Look`]), drawn by the game's
//! UI over the running world. The game's GAME MENU keeps its own rows: the
//! Forge menu is Back's, while the player flies the monitor ([`menu`]).
//!
//! - **Who builds what.** The session builds the sheets that need its
//!   document ([`open`]: the object menu, Properties, Link, a question).
//!   The rest are built here from what the session publishes ([`Facts`],
//!   [`request`]): Tools, Tool settings, the Forge menu, Controls, and the
//!   Waves screens from the set the session publishes ([`waves`]).
//! - **Opening.** A sheet asked for opens at the next UI update, for player
//!   one, once nothing is pending on the dialogs' window: the large
//!   dialog's classes over the screen beneath (`dialog_tag`), the list
//!   pages through the game's screen opener (`0x149f1e`), as the in-game
//!   Controller Settings opens.
//! - **Choices.** A press sends its [`sheet::Choice`] to its screen's
//!   outbox; [`take_choices`] drains them for the session, oldest first.
//!   GAME MENU's Leave Game asks first while edits are not saved; Yes is
//!   this module's: the session is left as the title leaves a game
//!   (`host_lobby::LEAVE_SESSIONS`, then the main menu).
//! - **Input.** While any screen is up, ours or the game's ([`ui_up`]), the
//!   session publishes it to `crate::forge_controls`, which hands the pad
//!   and the keys to the game's UI.

pub mod blockout;
pub mod games;
pub(crate) mod map_limits;
pub mod object_menu;
pub mod menu;
pub mod pages;
pub mod props;
pub mod screen;
pub mod sheet;
pub mod variants;
pub mod waves;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use runtime::guest::Permissions;

use crate::layout_glue::ReadGm;
use crate::list_class;
use crate::ui_kit::dialog_tag;
use crate::ui_kit::screen::{SlotTexts, TextPool};
use screen::ForgeScreen;
use sheet::{Ask, Choice, Kind, Look, NumberField, Sheet, TypeSpec};

/// The object actions menu's screen ([`Kind::ObjectActions`]).
pub const OBJECT_ACTIONS_SCREEN: &str = "forge.object-actions";

/// What the session tells the screens it does not build each tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// A local Forge session runs.
    pub active: bool,
    /// The player flies their Forge body, not their biped: only then is
    /// the Forge menu up.
    pub monitor: bool,
    /// Edits not saved: Leave Game asks first.
    pub dirty: bool,
    pub pending_rebuild: usize,
    /// The rotation snap, an index into `sheet::ANGLE_STEPS`.
    pub rotation_snap: usize,
    /// The movement snap, an index into `sheet::MOVE_STEPS`.
    pub movement_snap: usize,
    /// Local rather than World axes.
    pub local_axes: bool,
    /// The block-out grid in units (0 before the session says).
    pub block_grid: u16,
    /// Dev view is on.
    pub dev_view: bool,
    pub tool: Option<sheet::Tool>,
    /// The map variants of this map, for Load.
    pub variants: Vec<variants::Row>,
    /// The name of the variant the session is, for Save and Save as's
    /// keyboard; none for a session opened from Original.
    pub filename: Option<String>,
    /// The added objects, for the placed objects browser
    /// ([`sheet::placed_rows`]).
    pub placed: Vec<sheet::Placed>,
    /// The active groups, listed first by the browser
    /// ([`sheet::folder_rows`]).
    pub folders: Vec<sheet::Folder>,
    pub(crate) map_limits: map_limits::Usage,
    /// A wave test runs or is starting: the Forge menu's Test waves row
    /// reads Stop waves.
    pub testing: bool,
    /// The wave set and what the Waves screens read with it; `None` with
    /// no waves to edit, and the Forge menu has no Waves row.
    pub waves: Option<waves::Facts>,
    /// The Enemy types page's rows (a class of the retail table each);
    /// empty when there are none, and the Forge menu has no Enemy types
    /// row.
    pub enemy_types: Vec<waves::EnemyType>,
}

/// Bytes of guest memory for every Forge screen's texts together: slots
/// take what their text needs ([`TextPool`]), not a full slot each.
const TEXT_POOL_LEN: u32 = 0x10000;

/// The longest name Save as takes: a map variant's.
pub const NAME_CHARS: u32 = forge_doc::MAX_VARIANT_NAME as u32;

/// GAME MENU's Leave Game action word (the multiplayer pause screen,
/// `mp_menu`, screen 195): the row asks first while edits are not saved.
pub const LEAVE_GAME: u16 = 0;

/// A keyboard asked for and not yet opened.
#[derive(Clone, Debug, PartialEq)]
enum Keyboard {
    /// A map variant's name: what is typed is [`Choice::SaveAs`].
    Name,
    /// A typed number, or a variant's new name.
    Field(TypeSpec),
}

struct Hub {
    screens: Vec<Arc<ForgeScreen>>,
    /// The screen to open at the next UI update.
    request: Mutex<Option<Kind>>,
    /// Choices made outside a screen's own rows (a keyboard's text, a
    /// notice).
    inbox: Mutex<VecDeque<Choice>>,
    facts: Mutex<Facts>,
    /// The one guest region every screen's texts and the Forge menu's
    /// strip are placed in ([`TextPool`]).
    text_pool: Arc<TextPool>,
    /// The Forge menu's strip of tabs ([`menu::strip`]).
    tab_texts: SlotTexts,
    /// Save as's UTF-16 buffer, and whether the keyboard is on it.
    keyboard: AtomicU32,
    /// A keyboard asked for and not yet opened.
    keyboard_request: Mutex<Option<Keyboard>>,
    /// Which number a keyboard edits; Save as has none.
    keyboard_field: Mutex<Option<NumberField>>,
    /// The file stem of the map variant whose dialog is up: what Rename
    /// and Duplicate act on.
    variant_file: Mutex<Option<String>>,
    typing: AtomicBool,
    /// Yes to Leave Game: leave at the next UI update.
    leave: AtomicBool,
    /// Our UI update is calling retail's: let it through.
    passing_update: AtomicBool,
    /// The driver's hooks are built: a screen asked for opens.
    built: AtomicBool,
    /// The page each Waves screen was opened on ([`waves::open`]).
    wave_pages: Mutex<Vec<(Kind, waves::Page)>>,
    /// A Waves edit was refused: build the pages again ([`waves::stale`]).
    waves_stale: AtomicBool,
}

impl Hub {
    fn new() -> Hub {
        let text_pool = Arc::new(TextPool::new(TEXT_POOL_LEN));
        Hub {
            screens: Kind::ALL
                .iter()
                .map(|k| Arc::new(ForgeScreen::pooled(*k, Arc::clone(&text_pool))))
                .collect(),
            request: Mutex::new(None),
            inbox: Mutex::new(VecDeque::new()),
            facts: Mutex::new(Facts::default()),
            tab_texts: SlotTexts::pooled(
                crate::ui_kit::ids::family("forge menu tabs"),
                Arc::clone(&text_pool),
            ),
            text_pool,
            keyboard: AtomicU32::new(0),
            keyboard_request: Mutex::new(None),
            keyboard_field: Mutex::new(None),
            variant_file: Mutex::new(None),
            typing: AtomicBool::new(false),
            leave: AtomicBool::new(false),
            passing_update: AtomicBool::new(false),
            // A test's screens open without a game.
            built: AtomicBool::new(cfg!(test)),
            wave_pages: Mutex::new(Vec::new()),
            waves_stale: AtomicBool::new(false),
        }
    }
}

#[cfg(not(test))]
static HUB: std::sync::LazyLock<Hub> = std::sync::LazyLock::new(Hub::new);

#[cfg(not(test))]
fn hub() -> &'static Hub {
    &HUB
}

// Tests run side by side, each on its own thread: each has its own hub,
// as the value picker's pick is each test's own.
#[cfg(test)]
thread_local! {
    static HUB: &'static Hub = Box::leak(Box::new(Hub::new()));
}

#[cfg(test)]
fn hub() -> &'static Hub {
    HUB.with(|h| *h)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The screen of `kind`.
pub fn screen(kind: Kind) -> Arc<ForgeScreen> {
    let i = Kind::ALL
        .iter()
        .position(|k| *k == kind)
        .expect("every kind has a screen");
    Arc::clone(&hub().screens[i])
}

/// The session's facts, from this tick. A change of the wave set, or a
/// Waves edit refused since the last publish, builds the Waves pages again
/// ([`waves::refresh`]). The Forge menu is the monitor's: a swap back to
/// the player's body drops one asked for and closes one that is up.
pub fn publish(facts: Facts) {
    if !facts.active {
        *lock(&hub().request) = None;
        lock(&hub().inbox).clear();
        hub().typing.store(false, Ordering::SeqCst);
        hub().leave.store(false, Ordering::SeqCst);
    }
    if !facts.monitor {
        let mut request = lock(&hub().request);
        if *request == Some(Kind::Menu) {
            *request = None;
        }
        drop(request);
        let menu = screen(Kind::Menu);
        if menu.is_up() {
            menu.close_with_dialog();
        }
    }
    let set = facts.waves.clone();
    let changed = {
        let mut held = lock(&hub().facts);
        let changed = held.waves != facts.waves;
        *held = facts;
        changed
    };
    if waves::take_stale() || changed {
        waves::refresh(set.as_ref());
    }
}

fn facts() -> Facts {
    lock(&hub().facts).clone()
}

/// The screen asked for and not yet opened.
pub fn requested() -> Option<Kind> {
    *lock(&hub().request)
}

/// Drops the screen asked for: a test's stand-in for its opening and
/// closing.
#[cfg(test)]
pub(crate) fn take_request() -> Option<Kind> {
    lock(&hub().request).take()
}

/// Whether one of ours is up or about to open.
pub fn ours_up() -> bool {
    lock(&hub().request).is_some() || hub().screens.iter().any(|s| s.is_up())
}

/// The sheet of the screen of ours that is up, for `forge.status`.
pub fn shown() -> Option<Sheet> {
    hub().screens.iter().find(|s| s.is_up()).map(|s| s.sheet())
}

/// Whether a screen is up over the world, ours or the game's (GAME MENU,
/// the value picker, the keyboard): the monitor then takes no input.
pub fn ui_up(m: &dyn GuestMemory) -> bool {
    let g = ReadGm(m);
    ours_up()
        || hub().typing.load(Ordering::SeqCst)
        || crate::ui_watch::top(&g).is_some()
        || crate::ui_watch::dialog(&g).is_some()
}

/// Shows `sheet` on its screen and opens it at the next UI update. Refused
/// by name while another of ours is up or a sheet does not fit.
pub fn open(sheet: Sheet) -> Result<(), String> {
    usable()?;
    if ours_up() {
        return Err("A Forge screen is up.".into());
    }
    let kind = sheet.kind;
    screen(kind).show(sheet)?;
    *lock(&hub().request) = Some(kind);
    Ok(())
}

/// Shows the question `sheet` over the screen that is up, ours or the
/// game's: a Waves page's Start from asks over WAVES. Refused by name while
/// a question is up or a screen is about to open.
pub(crate) fn ask_over(sheet: Sheet) -> Result<(), String> {
    usable()?;
    let question = screen(Kind::Question);
    if question.is_up() || requested().is_some() {
        return Err("A Forge screen is up.".into());
    }
    question.show(sheet)?;
    *lock(&hub().request) = Some(Kind::Question);
    Ok(())
}

/// Shows `sheet` above the screen beneath: B comes back to it.
pub(crate) fn push(sheet: Sheet) -> Result<crate::ui::Chosen, String> {
    usable()?;
    let kind = sheet.kind;
    screen(kind).show(sheet)?;
    *lock(&hub().request) = Some(kind);
    Ok(crate::ui::Chosen::Push(kind.screen()))
}

/// Refused by name when nothing would open a screen: the driver is off.
fn usable() -> Result<(), String> {
    if hub().built.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err("The Forge screens are off (forge-screens).".into())
    }
}

/// Opens a screen this module builds ([`prepare`]) at the next UI update.
pub fn request(kind: Kind) -> Result<(), String> {
    usable()?;
    if ours_up() {
        return Err("A Forge screen is up.".into());
    }
    prepare(kind)?;
    *lock(&hub().request) = Some(kind);
    Ok(())
}

/// The sheet of a screen this module builds, from the session's facts,
/// shown on its screen. The others come from the session, refused by name.
pub(crate) fn prepare(kind: Kind) -> Result<(), String> {
    let f = facts();
    let sheet = match kind {
        Kind::Tools => sheet::tools(f.tool.unwrap_or(sheet::Tool::Objects)),
        Kind::Settings => sheet::settings_with(
            f.rotation_snap,
            f.movement_snap,
            f.local_axes,
            sheet::BLOCK_GRIDS
                .iter()
                .position(|g| *g == f.block_grid)
                .unwrap_or(sheet::DEFAULT_GRID_INDEX),
        ),
        // The tab it last showed, its rows as the session stands now.
        Kind::Menu if !f.monitor => {
            return Err("The Forge menu opens while you fly the monitor.".into());
        }
        Kind::Menu => menu::sheet(menu::tab_of(&screen(Kind::Menu).sheet()), &f),
        Kind::Waves => return waves::open(waves::Page::Waves),
        Kind::EnemyTypes => {
            waves::enemy_types_sheet(&f.enemy_types).ok_or("The AI ground names no enemy types.")?
        }
        Kind::Variants => variants::list_sheet(&f.variants, f.filename.as_deref()),
        Kind::Controls => sheet::controls(),
        Kind::MapLimits => map_limits::sheet(&f.map_limits),
        Kind::Objects
        | Kind::Properties
        | Kind::Coordinates
        | Kind::Link
        | Kind::Question
        | Kind::Games
        | Kind::Variant
        | Kind::ObjectActions => {
            return Err(format!(
                "{} shows the session's objects; the session opens it",
                kind.screen()
            ));
        }
        Kind::Rounds
        | Kind::Round
        | Kind::Wave
        | Kind::Squads
        | Kind::Squad
        | Kind::WaveProblems => {
            return Err(format!("{} opens from its row on WAVES", kind.screen()));
        }
    };
    screen(kind).show(sheet)
}

/// The choices made since the last take, oldest first: the screens' rows,
/// then the keyboards'. Yes to Leave Game is kept here (module docs).
pub fn take_choices() -> Vec<Choice> {
    let mut all: Vec<Choice> = hub()
        .screens
        .iter()
        .flat_map(|s| s.take_choices())
        .collect();
    all.extend(lock(&hub().inbox).drain(..));
    all.retain(|c| match c {
        Choice::Answer {
            ask: Ask::Leave,
            yes,
        } => {
            if *yes {
                hub().leave.store(true, Ordering::SeqCst);
            }
            false
        }
        _ => true,
    });
    all
}

fn send(c: Choice) {
    lock(&hub().inbox).push_back(c);
}

/// The string id of text `i` of the Forge menu's strip, reading `text`
/// ([`menu::strip`]).
pub(crate) fn tab_text(i: usize, text: &str) -> u32 {
    hub().tab_texts.set(i as u32, text)
}

/// Writes the strip's texts that changed.
pub(crate) fn write_tab_texts(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    hub().tab_texts.write(m)
}

/// Opens `kind`'s screen for player one: a dialog class over the screen
/// beneath, a list page through the game's opener.
async fn open_screen(cx: &Cx, kind: Kind) -> Result<(), EntryError> {
    let name = kind.screen();
    let Some(loader) = list_class::loader_of(name) else {
        eprintln!("{name}: not a screen of ours (or its class is off); not opened");
        send(Choice::Notice(format!("{name} is off.")));
        return Ok(());
    };
    const PLAYER_ONE: u32 = 1;
    match (kind.look(), dialog_tag::by_loader(loader)) {
        (Look::Picker | Look::Question, Some(d)) => {
            if dialog_tag::open_over_screen(cx, &d, PLAYER_ONE).await? == 0 {
                eprintln!("{name}: the game could not make the screen");
            }
        }
        (Look::List, _) => {
            cx.call(GuestCall::new(points::OPEN_SCREEN.base).args([PLAYER_ONE, loader]))
                .await?;
        }
        (Look::Floating, _) => {
            // The list is placed at the object's point as the screen opens.
            object_menu::place(screen(kind).sheet().rows.len());
            cx.call(GuestCall::new(points::OPEN_SCREEN.base).args([PLAYER_ONE, loader]))
                .await?;
        }
        (look, None) => eprintln!("{name}: a {look:?} screen with no dialog class; not opened"),
    }
    Ok(())
}

/// Whether a press of GAME MENU's row with action word `word` asks before
/// it acts: Leave Game, in a Forge session with edits not saved. Nothing
/// else on GAME MENU is ours.
fn asks_before_leaving(word: Option<u16>, f: &Facts) -> bool {
    f.active && f.dirty && word == Some(LEAVE_GAME)
}

/// What the UI update does first this frame.
enum Step {
    Leave,
    Close(u32),
    Open(Kind),
    Keyboard(Keyboard),
    /// Nothing before the game's update: the object actions menu is up,
    /// and follows its object after it.
    Follow,
}

/// A map variant's row on Map variants was pressed: its dialog's Rename
/// and Duplicate act on `file`.
fn target_variant(file: String) {
    *lock(&hub().variant_file) = Some(file);
}

/// A Coordinates or item-property row asks for the game's keyboard.
fn ask_keyboard(spec: TypeSpec) -> crate::ui::Chosen {
    if hub().typing.load(Ordering::SeqCst) {
        return crate::ui::Chosen::Stepped("A keyboard is already open.".into());
    }
    *lock(&hub().keyboard_request) = Some(Keyboard::Field(spec));
    crate::ui::Chosen::Act
}

/// The Forge menu's Save as (and Save, for a session with no name) asks
/// for the game's keyboard on a map variant's name. Refused by name while
/// a keyboard is open.
pub(crate) fn ask_name() -> Result<(), String> {
    if hub().typing.load(Ordering::SeqCst) {
        return Err("A keyboard is already open.".into());
    }
    *lock(&hub().keyboard_request) = Some(Keyboard::Name);
    Ok(())
}

fn step(m: &dyn GuestMemory) -> Option<Step> {
    if !lock(&hub().facts).active {
        return None;
    }
    if hub().leave.swap(false, Ordering::SeqCst) {
        return Some(Step::Leave);
    }
    // The object menu closes after a pick; the Forge menu after its Load
    // picked, or when the player is back in their body (`publish`); Map
    // variants after its dialog chose a Load or a Delete, whose question
    // needs no screen of ours up; a Waves page when the part it shows is
    // gone (`waves::refresh`).
    for kind in [
        Kind::Objects,
        Kind::Menu,
        Kind::Variants,
        Kind::Round,
        Kind::Wave,
        Kind::Squads,
        Kind::Squad,
    ] {
        let beneath = screen(kind);
        if beneath.take_close_after_pick()
            && let Some(s) = beneath.screen()
        {
            return Some(Step::Close(s));
        }
    }
    let g = ReadGm(m);
    let pending = (0..=crate::ui_watch::SHARED).any(|slot| {
        crate::ui_watch::stack(&g, dialog_tag::DIALOG_WINDOW, slot).is_some_and(|s| s.pending != 0)
    });
    if pending {
        return None;
    }
    if let Some(spec) = lock(&hub().keyboard_request).take() {
        return Some(Step::Keyboard(spec));
    }
    lock(&hub().request)
        .take()
        .map(Step::Open)
        .or_else(|| screen(Kind::ObjectActions).is_up().then_some(Step::Follow))
}

/// A keyboard, once it closed: Save as's name or the typed number.
fn poll_keyboard(m: &dyn GuestMemory) -> Result<(), MemFault> {
    let buffer = hub().keyboard.load(Ordering::SeqCst);
    if buffer == 0
        || !hub().typing.load(Ordering::SeqCst)
        || crate::signin_keyboard::open_on(m, buffer)?
    {
        return Ok(());
    }
    hub().typing.store(false, Ordering::SeqCst);
    let text = crate::signin_keyboard::read_text(m, buffer, NAME_CHARS + 1)?;
    let text = text.trim();
    if let Some(field) = lock(&hub().keyboard_field).take() {
        let file = lock(&hub().variant_file).clone();
        send(match (field, file) {
            (NumberField::RenameVariant, Some(file)) => {
                Choice::RenameVariant(file, text.to_string())
            }
            (NumberField::DuplicateVariant, Some(file)) => {
                Choice::DuplicateVariant(file, text.to_string())
            }
            (NumberField::RenameVariant | NumberField::DuplicateVariant, None) => {
                Choice::Notice("That map variant is gone.".into())
            }
            (field, _) => Choice::Number(field, text.into()),
        });
    } else {
        send(if text.is_empty() {
            Choice::Notice("Not saved: the name was empty.".into())
        } else {
            Choice::SaveAs(text.to_string())
        });
    }
    Ok(())
}

async fn open_keyboard(cx: &Cx, ask: Keyboard) -> Result<(), EntryError> {
    let buffer = hub().keyboard.load(Ordering::SeqCst);
    if buffer == 0 {
        send(Choice::Notice("The keyboard has no memory.".into()));
        return Ok(());
    }
    let start = match ask {
        Keyboard::Field(spec) => {
            *lock(&hub().keyboard_field) = Some(spec.field);
            spec.initial
        }
        Keyboard::Name => {
            *lock(&hub().keyboard_field) = None;
            facts().filename.unwrap_or_default()
        }
    };
    cx.write(|m| m.write(buffer, &crate::signin_keyboard::units(&start)))?;
    hub().typing.store(true, Ordering::SeqCst);
    cx.call(GuestCall::new(crate::signin_keyboard::OPEN).ecx(0).args([
        crate::signin_keyboard::INVITE_CONTEXT,
        buffer,
        NAME_CHARS + 1,
    ]))
    .await?;
    Ok(())
}

struct Driver;

fn manifest() -> Manifest {
    Manifest::new(
        "forge-screens",
        "Forge screens in the game",
    )
    .doc("docs/game/forge-firefight-ui.md")
}

impl Feature for Driver {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        hub().built.store(true, Ordering::SeqCst);
        hub()
            .text_pool
            .base
            .store(b.guest_alloc(TEXT_POOL_LEN, Permissions::RW)?, Ordering::SeqCst);
        for s in &hub().screens {
            let me = Arc::clone(s);
            b.locale()
                .source(s.kind().screen(), move |id| me.texts.address(id));
        }
        b.locale()
            .source("forge menu tabs", |id| hub().tab_texts.address(id));
        hub().keyboard.store(
            b.guest_alloc((NAME_CHARS + 1) * 2, Permissions::RW)?,
            Ordering::SeqCst,
        );
        b.snapshot(SnapshotPolicy::section_forbid_while(
            || {
                (ours_up() || hub().typing.load(Ordering::SeqCst))
                    .then(|| "a Forge screen is up".into())
            },
            Vec::new,
            |_, _| {
                for s in &hub().screens {
                    s.texts.rewrite_all();
                }
                hub().tab_texts.rewrite_all();
                Ok(())
            },
        ));

        b.raw()
            .before(&points::UI_UPDATE, "open, close and leave for Forge", |c| {
                if hub().passing_update.load(Ordering::SeqCst) {
                    return Ok(Flow::Continue);
                }
                let next = {
                    let mem = c.ctx.runtime().memory();
                    if let Err(e) = poll_keyboard(&*mem) {
                        eprintln!("forge screens: reading Save as's keyboard: {e:?}");
                    }
                    step(&*mem)
                };
                let Some(next) = next else {
                    return Ok(Flow::Continue);
                };
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        let elapsed = cx.arg(0)?;
                        let done = match next {
                            Step::Leave => {
                                eprintln!("forge screens: leaving the game without saving");
                                match cx
                                    .call(GuestCall::new(crate::host_lobby::LEAVE_SESSIONS))
                                    .await
                                {
                                    Ok(_) => cx
                                        .call(
                                            GuestCall::new(crate::level_boot::REQUEST_MAINMENU)
                                                .args([0]),
                                        )
                                        .await
                                        .map(|_| ()),
                                    Err(e) => Err(e),
                                }
                            }
                            Step::Close(s) => dialog_tag::close(&cx, s).await,
                            Step::Open(kind) => open_screen(&cx, kind).await,
                            Step::Keyboard(ask) => open_keyboard(&cx, ask).await,
                            Step::Follow => Ok(()),
                        };
                        if let Err(e) = done {
                            eprintln!("forge screens: {e}");
                        }
                        hub().passing_update.store(true, Ordering::SeqCst);
                        let out = cx
                            .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                            .await;
                        hub().passing_update.store(false, Ordering::SeqCst);
                        // After the game's update, which is where a
                        // widget's animation writes its offset.
                        let menu = screen(Kind::ObjectActions);
                        if let Some((at, by)) = menu.screen().and_then(|at| {
                            object_menu::follow(menu.sheet().rows.len()).map(|by| (at, by))
                        }) && let Err(e) = cx.write(|m| list_class::offset_list(m, at, by))
                        {
                            eprintln!("forge screens: moving the object menu: {e:?}");
                        }
                        Ok(Ret::new(out?.eax, 4))
                    })
                })
            })
            .after_feature("live-join-link")
            .after_feature("theater-link")
            .optional();

        b.raw()
            .before(&points::MP_MENU_ROW_CHOSEN, "ask before Leave Game in Forge", |c| {
                let Some(at) = c.arg_u32("row") else {
                    return Ok(Flow::Continue);
                };
                let list = c.cpu.ecx;
                let word = {
                    let mem = c.ctx.runtime().memory();
                    crate::pause_menu::chosen_action(&*mem, list, at)
                };
                // Every other row is the game's, as in any game.
                if !asks_before_leaving(word, &facts()) {
                    return Ok(Flow::Continue);
                }
                if let Err(why) = screen(Kind::Question).show(sheet::question(Ask::Leave, "")) {
                    eprintln!("forge screens: {why}");
                    return Ok(Flow::Continue);
                }
                *lock(&hub().request) = Some(Kind::Question);
                c.answer_async(|_| -> EntryFuture { Box::pin(async { Ok(Ret::new(0, 8)) }) })
            });
        Ok(())
    }
}

/// Registers each screen's class and the driver.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    for s in &hub().screens {
        registry.add(&s.class())?;
    }
    registry.add(&Driver)?;
    Ok(())
}
