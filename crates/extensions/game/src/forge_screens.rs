//! The Forge screens in the game (docs/game/forge-firefight-ui.md "The
//! screens"): the object menu, Properties, Tools, Tool settings, the Link
//! dialog, the questions, Forge options and Controls, each a list class of
//! ours on a retail screen tag ([`sheet::Look`]), drawn by the game's UI
//! over the running world; and the Forge rows of the game's GAME MENU
//! ([`game_menu`]).
//!
//! - **Who builds what.** The session builds the sheets that need its
//!   document ([`open`]: the object menu, Properties, Link, a question).
//!   The rest are built here from what the session publishes ([`Facts`],
//!   [`request`]): Tools, Tool settings, Forge options, Controls, and the
//!   Waves screens from the set the session publishes ([`waves`]).
//! - **Opening.** A sheet asked for opens at the next UI update, for player
//!   one, once nothing is pending on the dialogs' window: the large
//!   dialog's classes over the screen beneath (`dialog_tag`), the list
//!   pages through the game's screen opener (`0x149f1e`), as the in-game
//!   Controller Settings opens.
//! - **Choices.** A press sends its [`sheet::Choice`] to its screen's
//!   outbox; [`take_choices`] drains them for the session, oldest first.
//!   Yes to Leave Game is this module's: the session is left as the title
//!   leaves a game (`host_lobby::LEAVE_SESSIONS`, then the main menu).
//! - **Input.** While any screen is up, ours or the game's ([`ui_up`]), the
//!   session publishes it to `crate::forge_controls`, which hands the pad
//!   and the keys to the game's UI.

pub mod game_menu;
pub mod games;
pub(crate) mod map_limits;
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

use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::view::ui::{self, ListHandler};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use runtime::guest::Permissions;

use crate::layout_glue::ReadGm;
use crate::list_class;
use crate::ui_kit::dialog_tag;
use crate::ui_kit::picker::{self, Pick};
use crate::ui_kit::screen::{SlotTexts, TextPool};
use game_menu::MenuRow;
use screen::ForgeScreen;
use sheet::{Ask, Choice, Kind, Look, NumberField, Sheet, TypeSpec};

/// What the session tells the screens it does not build each tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// A local Forge session runs: GAME MENU has its rows.
    pub active: bool,
    /// Edits not saved: Leave Game asks first.
    pub dirty: bool,
    pub pending_rebuild: usize,
    /// The rotation snap, an index into `sheet::ANGLE_STEPS`.
    pub rotation_snap: usize,
    /// The movement snap, an index into `sheet::MOVE_STEPS`.
    pub movement_snap: usize,
    /// Local rather than World axes.
    pub local_axes: bool,
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
    /// A wave test runs or is starting: Forge options' Test waves row reads
    /// Stop waves.
    pub testing: bool,
    /// The wave set and what the Waves screens read with it; `None` with
    /// no waves to edit, and Forge options has no Waves row.
    pub waves: Option<waves::Facts>,
    /// The Enemy types page's rows (a class of the retail table each);
    /// empty when there are none, and Forge options has no Enemy types
    /// row.
    pub enemy_types: Vec<waves::EnemyType>,
}

/// Bytes of guest memory for every Forge screen's texts together: slots
/// take what their text needs ([`TextPool`]), not a full slot each.
const TEXT_POOL_LEN: u32 = 0x10000;

/// The longest name Save as takes: a map variant's.
pub const NAME_CHARS: u32 = forge_doc::MAX_VARIANT_NAME as u32;

struct Hub {
    screens: Vec<Arc<ForgeScreen>>,
    /// The screen to open at the next UI update.
    request: Mutex<Option<Kind>>,
    /// Choices made outside a screen's own rows (GAME MENU, Load's picker,
    /// Save as's keyboard).
    inbox: Mutex<VecDeque<Choice>>,
    facts: Mutex<Facts>,
    /// The one guest region every screen's texts and GAME MENU's labels
    /// are placed in ([`TextPool`]).
    text_pool: Arc<TextPool>,
    /// GAME MENU's labels.
    menu_texts: SlotTexts,
    /// Save as's UTF-16 buffer, and whether the keyboard is on it.
    keyboard: AtomicU32,
    /// A typed-number keyboard asked for and not yet opened.
    keyboard_request: Mutex<Option<TypeSpec>>,
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
    /// The GAME MENU list whose constructor is being let through (0: none).
    passing_ctor: AtomicU32,
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
            menu_texts: SlotTexts::pooled(
                crate::ui_kit::ids::family("forge game menu"),
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
            passing_ctor: AtomicU32::new(0),
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
/// ([`waves::refresh`]).
pub fn publish(facts: Facts) {
    if !facts.active {
        *lock(&hub().request) = None;
        lock(&hub().inbox).clear();
        hub().typing.store(false, Ordering::SeqCst);
        hub().leave.store(false, Ordering::SeqCst);
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
        Kind::Settings => sheet::settings(f.rotation_snap, f.movement_snap, f.local_axes),
        Kind::Options => sheet::options_with_waves(
            &f.folders,
            &f.placed,
            f.testing,
            f.waves.is_some(),
            !f.enemy_types.is_empty(),
        ),
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
        | Kind::Variant => {
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
/// then GAME MENU's. Yes to Leave Game is kept here (module docs).
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

/// The GAME MENU rows' array grows by ours while a session runs
/// (`layout_glue`'s list size chain).
pub fn game_menu_count(name: u32, n: u32) -> u32 {
    game_menu::list_count(name, n, lock(&hub().facts).active)
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
        (look, None) => eprintln!("{name}: a {look:?} screen with no dialog class; not opened"),
    }
    Ok(())
}

/// What the UI update does first this frame.
enum Step {
    Leave,
    Close(u32),
    Open(Kind),
    Keyboard(TypeSpec),
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
    *lock(&hub().keyboard_request) = Some(spec);
    crate::ui::Chosen::Act
}

fn step(m: &dyn GuestMemory) -> Option<Step> {
    if !lock(&hub().facts).active {
        return None;
    }
    if hub().leave.swap(false, Ordering::SeqCst) {
        return Some(Step::Leave);
    }
    // The object menu closes after a pick; Map variants after its dialog
    // chose a Load or a Delete, whose question needs no screen of ours up;
    // a Waves page when the part it shows is gone (`waves::refresh`).
    for kind in [
        Kind::Objects,
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
    lock(&hub().request).take().map(Step::Open)
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

async fn open_keyboard(cx: &Cx, spec: Option<TypeSpec>) -> Result<(), EntryError> {
    let buffer = hub().keyboard.load(Ordering::SeqCst);
    if buffer == 0 {
        send(Choice::Notice("The keyboard has no memory.".into()));
        return Ok(());
    }
    let start = match spec {
        Some(spec) => {
            *lock(&hub().keyboard_field) = Some(spec.field);
            spec.initial
        }
        None => facts().filename.unwrap_or_default(),
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

/// A press of one of our GAME MENU rows, for the player of `chain`, on
/// the list `list`.
async fn menu_press(cx: &Cx, row: MenuRow, chain: u32, list: u32) -> Result<(), EntryError> {
    let f = facts();
    match row {
        MenuRow::Save => match variants::save(f.filename.as_deref()) {
            variants::SaveStep::Keyboard => open_keyboard(cx, None).await?,
            variants::SaveStep::Write { .. } => send(Choice::Save),
        },
        MenuRow::SaveAs => open_keyboard(cx, None).await?,
        MenuRow::Load if variants::loadable(&f.variants).is_empty() => {
            send(Choice::Notice(variants::NONE_SAVED.into()))
        }
        MenuRow::Load => {
            let rows: Vec<variants::Row> = variants::loadable(&f.variants)
                .into_iter()
                .cloned()
                .collect();
            let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
            let current = f
                .filename
                .as_deref()
                .and_then(|n| rows.iter().position(|r| variants::same_name(&r.name, n)))
                .unwrap_or(0);
            let set = Arc::new(move |i: usize| -> String {
                match rows.get(i) {
                    Some(r) => {
                        send(Choice::Load(r.file.clone()));
                        format!("forge: load {}", r.name)
                    }
                    None => format!("forge: no map variant {i}"),
                }
            });
            let pick = Pick::new(
                "Load",
                names,
                current,
                "A map variant of this map.",
                set,
            )
            .over(screen::OPENER);
            if picker::offer_list(pick).is_some() {
                list_class::open(cx, chain, picker::NAME).await?;
            }
        }
        MenuRow::Rebuild => send(Choice::Rebuild),
        MenuRow::Options => match prepare(Kind::Options) {
            Ok(()) => {
                list_class::push(
                    cx,
                    chain,
                    list.wrapping_sub(ui::LIST),
                    Kind::Options.screen(),
                )
                .await?
            }
            Err(why) => send(Choice::Notice(why)),
        },
    }
    Ok(())
}

struct Driver;

fn manifest() -> Manifest {
    Manifest::new(
        "forge-screens",
        "Forge screens and GAME MENU rows in the game",
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
            .source("forge game menu", |id| hub().menu_texts.address(id));
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
                hub().menu_texts.rewrite_all();
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
                            Step::Keyboard(spec) => open_keyboard(&cx, Some(spec)).await,
                        };
                        if let Err(e) = done {
                            eprintln!("forge screens: {e}");
                        }
                        hub().passing_update.store(true, Ordering::SeqCst);
                        let out = cx
                            .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                            .await;
                        hub().passing_update.store(false, Ordering::SeqCst);
                        Ok(Ret::new(out?.eax, 4))
                    })
                })
            })
            .after_feature("live-join-link")
            .after_feature("theater-link")
            .optional();

        b.raw().before(
            &points::MP_MENU_LIST_CTOR,
            "add the Forge rows to GAME MENU",
            |c| {
                let list = c.cpu.esi;
                if list == 0
                    || hub()
                        .passing_ctor
                        .compare_exchange(list, 0, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    || !lock(&hub().facts).active
                {
                    return Ok(Flow::Continue);
                }
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        let a = cx.arg(0)?;
                        hub().passing_ctor.store(list, Ordering::SeqCst);
                        let out = cx
                            .call(
                                GuestCall::new(points::MP_MENU_LIST_CTOR.base)
                                    .esi(list)
                                    .args([a]),
                            )
                            .await;
                        hub().passing_ctor.store(0, Ordering::SeqCst);
                        let eax = out?.eax;
                        let added = cx.write(|m| -> Result<usize, String> {
                            let rows = ListHandler::at(list)
                                .rows(m)
                                .map_err(|e| format!("the GAME MENU list's rows: {e:?}"))?;
                            game_menu::put_first(m, rows, &game_menu::actions())
                        });
                        match added {
                            Ok(n) => eprintln!("forge screens: GAME MENU has {n} rows"),
                            Err(why) => eprintln!("forge screens: GAME MENU rows not added: {why}"),
                        }
                        Ok(Ret::new(eax, 4))
                    })
                })
            },
        );

        b.raw()
            .before(&points::MP_MENU_ROW_TEXT, "name the Forge rows", |c| {
                let Some(item) = c.arg_u32("item") else {
                    return Ok(Flow::Continue);
                };
                let row = {
                    let mem = c.ctx.runtime().memory();
                    crate::pause_menu::item_action(&*mem, c.cpu.ecx, item)
                        .and_then(MenuRow::of_action)
                };
                let Some(row) = row else {
                    return Ok(Flow::Continue);
                };
                let pending = lock(&hub().facts).pending_rebuild;
                let id = hub().menu_texts.set(row.index() as u32, &row.text(pending));
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        cx.write(|m| hub().menu_texts.write(m))?;
                        list_class::draw_texts(&cx, item, vec![(0, id)]).await?;
                        Ok(Ret::new(0, 8))
                    })
                })
            })
            // Player Settings names its own row here too; each answers only
            // for its own action words, so either order draws the same.
            .after_feature("pause-mp-rows");

        b.raw()
            .before(&points::MP_MENU_ROW_CHOSEN, "press the Forge rows", |c| {
                let Some(at) = c.arg_u32("row") else {
                    return Ok(Flow::Continue);
                };
                let list = c.cpu.ecx;
                let word = {
                    let mem = c.ctx.runtime().memory();
                    crate::pause_menu::chosen_action(&*mem, list, at)
                };
                let f = facts();
                if !f.active {
                    return Ok(Flow::Continue);
                }
                if word == Some(game_menu::LEAVE_GAME) && f.dirty {
                    // Leave Game with edits not saved asks first.
                    if let Err(why) = screen(Kind::Question).show(sheet::question(Ask::Leave, "")) {
                        eprintln!("forge screens: {why}");
                        return Ok(Flow::Continue);
                    }
                    *lock(&hub().request) = Some(Kind::Question);
                    return c.answer_async(|_| -> EntryFuture {
                        Box::pin(async { Ok(Ret::new(0, 8)) })
                    });
                }
                let Some(row) = word.and_then(MenuRow::of_action) else {
                    return Ok(Flow::Continue);
                };
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        let chain = cx.arg(0)?;
                        menu_press(&cx, row, chain, list).await?;
                        Ok(Ret::new(0, 8))
                    })
                })
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
