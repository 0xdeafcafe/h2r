//! **Change Map** on the multiplayer pause menu (GAME MENU, screen 195),
//! for the party leader of a custom game: pick a map, the game ends, and
//! the next game starts on that map with no further press
//! (docs/game/settings-menu.md "Change Map from the pause menu").
//!
//! - **The row.** Retail shows End Game (action 5) only on the console
//!   that leads the party (`0x592f0`) outside matchmaking (lobby mode
//!   `0x19989d` not 6). An `after` hook on the list's constructor
//!   (`MP_MENU_LIST_CTOR`) adds our row ([`CHANGE_MAP_ACTION`]) just above
//!   End Game when the list has it, this console hosts the game (or plays
//!   it offline) and the map list has maps to offer ([`offered`]). The
//!   rows' array has room for it: `layout_glue` asks [`list_count`] when
//!   the array is made.
//! - **Text and press.** `MP_MENU_ROW_TEXT` draws our label;
//!   `MP_MENU_ROW_CHOSEN` runs action 5's press (End Game) for any word it
//!   does not know, so ours is always taken before the original.
//! - **The picker.** The same categories and maps as Game Setup's Change
//!   Map (`crate::map_select`), as two list classes of ours on the pause
//!   menu's own window and slot ([`CATEGORIES_SCREEN`], [`MAPS_SCREEN`]):
//!   a Forge or Firefight lobby opens its own category's maps
//!   ([`opening`]). B goes back a step, then to the pause menu, with
//!   nothing changed.
//! - **A map.** [`choose`] records the map ([`Pending`]) and ends the game
//!   through the engine's round-over path
//!   (`crate::firefight::guest::end_game`). Back in the pregame lobby
//!   ([`Lobby::pregame`]) the UI update hook sets the lobby's map
//!   (`LOBBY_SET_MAP`, as retail's Change Map does, so it replicates), and
//!   once retail's start check (`LOBBY_START_STATE`) answers ready, starts
//!   the countdown for the pressing controller
//!   (`host_lobby::CONTROLLER_COUNTDOWN`). The variant stays the lobby's.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use engine::datum::DatumArray;
use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::view::ui::{self, ListHandler};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use runtime::guest::Permissions;

use crate::list_class::{self, ListClass, Rows};
use crate::lobby_start;
use crate::map_select::{self, Catalogue, Category, Map};
use crate::ui::Chosen;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::screen::{SLOT_LEN, SlotTexts};

/// The feature (`features.pause-change-map` switches the row, the picker
/// and the start).
pub const FEATURE: &str = "pause-change-map";
/// The picker's two list classes, by the names `Push` rows use.
pub const CATEGORIES_SCREEN: &str = "pause-map-categories";
pub const MAPS_SCREEN: &str = "pause-map-list";

/// GAME MENU's rows' array (`crate::forge_screens::game_menu`).
pub const ARRAY_NAME: u32 = crate::forge_screens::game_menu::ARRAY_NAME;
/// Retail's End Game action word.
pub const END_GAME: u16 = 5;
/// Our row's action word: clear of retail's 0 to 5 and the Forge rows'
/// `0x20` to `0x24`.
pub const CHANGE_MAP_ACTION: u16 = 0x30;
const ACTION_WORD: u32 = 2;

/// Our strings: the row's label, the two titles, then the categories'
/// labels and one a map row.
pub const IDS: Family = ids::family("pause change map");
const ROW_SLOT: u32 = 0;
const CATEGORIES_TITLE_SLOT: u32 = 1;
const MAPS_TITLE_SLOT: u32 = 2;
const CATEGORY_SLOTS: u32 = 3;
const MAP_SLOTS: u32 = CATEGORY_SLOTS + Category::ALL.len() as u32;
/// The most maps a list shows: one slot each.
pub const MAX_ROWS: usize = (IDS.len - MAP_SLOTS) as usize;

pub const ROW_LABEL: &str = "Change Map";
const CATEGORIES_TITLE: &str = "Select Map";

static TEXTS: LazyLock<SlotTexts> = LazyLock::new(|| SlotTexts::new(IDS));
/// Set once the feature is built: GAME MENU's array grows by our row.
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// The GAME MENU list being built (ESI at the constructor's entry).
static BUILDING: AtomicU32 = AtomicU32::new(0);
/// The map picked and how far the next game is.
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
/// Our own UI update call, which the hook lets through.
static PASSING: AtomicBool = AtomicBool::new(false);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- the row ----

/// GAME MENU's rows' array grows by our row while the feature is built;
/// any other array as asked.
pub fn list_count(name: u32, n: u32) -> u32 {
    grown(name, n, INSTALLED.load(Ordering::SeqCst))
}

/// [`list_count`] with the feature's state given.
pub fn grown(name: u32, n: u32, installed: bool) -> u32 {
    if installed && name == ARRAY_NAME {
        n + 1
    } else {
        n
    }
}

/// What this console is to the game, for the row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    pub networking: bool,
    /// The squad session's host peer is this console's.
    pub host: bool,
    /// The life cycle is matchmaking's.
    pub matchmaking: bool,
}

impl Menu {
    pub fn read(m: &dyn GuestMemory) -> Menu {
        use bots::lobby;
        let f = lobby::membership(m).unwrap_or_default();
        let host = (|| {
            let s = m.u32(lobby::SESSION).ok().filter(|s| *s != 0)?;
            Some(m.u32(s + lobby::HOST_PEER).ok()? == m.u32(s + lobby::PEER).ok()?)
        })()
        .unwrap_or(false);
        Menu {
            networking: f.networking,
            host,
            matchmaking: lobby::matchmaking_cycle(m),
        }
    }

    /// Whether this console can end the game for everyone and start the
    /// next: not matchmaking, and played offline or hosted here. A leader
    /// that does not host (after a host migration) gets no row: the
    /// round-over path ends the game only on the host.
    pub fn runs_the_game(self) -> bool {
        !self.matchmaking && (!self.networking || self.host)
    }
}

/// Whether a GAME MENU with these action words gets our row: it has End
/// Game (the party leader's, outside matchmaking), not ours yet, this
/// console runs the game, and the picker has something to open.
pub fn offered(actions: &[u16], menu: Menu, can_open: bool) -> bool {
    actions.contains(&END_GAME)
        && !actions.contains(&CHANGE_MAP_ACTION)
        && menu.runs_the_game()
        && can_open
}

/// The words with ours just above End Game (at the end without it).
pub fn placed(actions: &[u16]) -> Vec<u16> {
    let at = actions
        .iter()
        .position(|a| *a == END_GAME)
        .unwrap_or(actions.len());
    let mut out = actions.to_vec();
    out.insert(at, CHANGE_MAP_ACTION);
    out
}

fn elements(m: &dyn GuestMemory, a: DatumArray) -> Result<Vec<u32>, MemFault> {
    let high = a.header_field(m, engine::datum::layout::HIGH_WATER)?;
    let mut out = Vec::new();
    for i in 0..high.max(0) {
        if let Some(e) = a.get_absolute(m, i)? {
            out.push(e);
        }
    }
    Ok(out)
}

/// The action words of the rows' array at `array`, in slot order.
pub fn actions(m: &dyn GuestMemory, array: u32) -> Result<Vec<u16>, MemFault> {
    elements(m, DatumArray::at(array))?
        .into_iter()
        .map(|e| m.u16(e + ACTION_WORD))
        .collect()
}

/// Adds our row to the rows' array at `array`, above End Game: a new
/// datum, then every word written back in slot order ([`placed`]). The
/// rows the list then has; refused by name when the array is full.
pub fn add_row(m: &mut dyn GuestMemory, array: u32) -> Result<usize, String> {
    let fault = |e: MemFault| format!("the GAME MENU rows' array: {e:?}");
    let a = DatumArray::at(array);
    let words = placed(&actions(m, array).map_err(fault)?);
    a.new_datum(m)
        .map_err(fault)?
        .ok_or("the GAME MENU rows' array is full; its room was not grown")?;
    let mut elems = elements(m, a).map_err(fault)?;
    elems.sort_unstable();
    if elems.len() != words.len() {
        return Err(format!("{} rows after adding, not {}", elems.len(), words.len()));
    }
    for (e, w) in elems.iter().zip(&words) {
        m.set_u16(e + ACTION_WORD, *w).map_err(fault)?;
    }
    Ok(words.len())
}

// ---- the picker ----

/// What the row opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    /// The categories, then a category's maps.
    Categories,
    /// One category's maps straight away.
    Maps(Category),
}

impl Opening {
    pub fn screen(self) -> &'static str {
        match self {
            Opening::Categories => CATEGORIES_SCREEN,
            Opening::Maps(_) => MAPS_SCREEN,
        }
    }
}

/// The categories with maps a Multiplayer lobby lists, in order.
pub fn categories(cat: &Catalogue) -> Vec<Category> {
    cat.categories()
        .into_iter()
        .filter(|c| !cat.maps(*c).is_empty())
        .collect()
}

/// What the row opens for a lobby in `mode`: a Forge or Firefight lobby's
/// own category ([`map_select::mode_category`]), else the categories, or
/// the one category's maps when there is one. `None` with nothing to pick.
pub fn opening(mode: lobby_start::Mode, cat: &Catalogue) -> Option<Opening> {
    if let Some(c) = map_select::mode_category(mode) {
        return (!cat.maps(c).is_empty()).then_some(Opening::Maps(c));
    }
    match categories(cat).as_slice() {
        [] => None,
        [one] => Some(Opening::Maps(*one)),
        _ => Some(Opening::Categories),
    }
}

/// The maps a category's list shows, in the catalogue's order.
pub fn maps(cat: &Catalogue, c: Category) -> Vec<Map> {
    cat.maps(c).iter().take(MAX_ROWS).cloned().collect()
}

fn category_slot(c: Category) -> u32 {
    CATEGORY_SLOTS + Category::ALL.iter().position(|k| *k == c).unwrap_or(0) as u32
}

/// What the two screens share: the category opened, the map pressed and
/// the controller that pressed the row.
#[derive(Default)]
pub struct Picker {
    category: Mutex<Option<Category>>,
    pressed: Mutex<Option<u32>>,
    port: AtomicU32,
}

impl Picker {
    pub fn open(&self, opening: Opening, port: u32) {
        self.port.store(port, Ordering::SeqCst);
        *lock(&self.pressed) = None;
        if let Opening::Maps(c) = opening {
            *lock(&self.category) = Some(c);
        }
    }
    pub fn category(&self) -> Option<Category> {
        *lock(&self.category)
    }
    pub fn take_pressed(&self) -> Option<u32> {
        lock(&self.pressed).take()
    }
    pub fn port(&self) -> u32 {
        self.port.load(Ordering::SeqCst)
    }
}

fn catalogue() -> Arc<Catalogue> {
    map_select::current().unwrap_or_default()
}

/// The categories' screen.
pub struct CategoryRows(pub Arc<Picker>);

impl Rows for CategoryRows {
    fn title(&self) -> u32 {
        IDS.id(CATEGORIES_TITLE_SLOT)
    }
    fn count(&self) -> usize {
        TEXTS.set(CATEGORIES_TITLE_SLOT, CATEGORIES_TITLE);
        categories(&catalogue()).len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let cats = categories(&catalogue());
        match usize::try_from(row).ok().and_then(|r| cats.get(r)) {
            Some(c) => vec![(0, TEXTS.set(category_slot(*c), c.label()))],
            None => Vec::new(),
        }
    }
    fn press(&self, row: i16) -> Chosen {
        let cats = categories(&catalogue());
        let Some(&c) = usize::try_from(row).ok().and_then(|r| cats.get(r)) else {
            return Chosen::Nothing;
        };
        *lock(&self.0.category) = Some(c);
        Chosen::Push(MAPS_SCREEN)
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        TEXTS.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let labels: Vec<&str> = categories(&catalogue()).iter().map(|c| c.label()).collect();
        Some(format!("{CATEGORIES_TITLE}: {count} rows ({})", labels.join(", ")))
    }
}

/// The maps' screen: the opened category's maps. A on one runs [`choose`].
pub struct MapRows(pub Arc<Picker>);

impl MapRows {
    fn maps(&self) -> Vec<Map> {
        self.0
            .category()
            .map_or_else(Vec::new, |c| maps(&catalogue(), c))
    }
}

impl Rows for MapRows {
    fn title(&self) -> u32 {
        IDS.id(MAPS_TITLE_SLOT)
    }
    fn count(&self) -> usize {
        let label = self.0.category().map_or(CATEGORIES_TITLE, Category::label);
        TEXTS.set(MAPS_TITLE_SLOT, label);
        self.maps().len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let maps = self.maps();
        match usize::try_from(row).ok().and_then(|r| maps.get(r)) {
            Some(m) => vec![(0, TEXTS.set(MAP_SLOTS + row as u32, &m.name))],
            None => Vec::new(),
        }
    }
    fn press(&self, row: i16) -> Chosen {
        let maps = self.maps();
        let Some(m) = usize::try_from(row).ok().and_then(|r| maps.get(r)) else {
            return Chosen::Nothing;
        };
        *lock(&self.0.pressed) = Some(m.id);
        Chosen::Act
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        TEXTS.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let names: Vec<String> = self.maps().into_iter().map(|m| m.name).collect();
        let c = self.0.category().map_or("none", Category::label);
        Some(format!("{c}: {count} maps ({})", names.join(", ")))
    }
}

type ActionFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), EntryError>> + Send>>;

/// The two list classes. They open in a game, on the in-game Controller
/// Settings' tag (`crate::list_class`, module docs). X and Start do
/// nothing on the maps: only A on a map picks it.
pub fn classes(picker: &Arc<Picker>) -> [ListClass; 2] {
    let categories = ListClass::with_rows(
        CATEGORIES_SCREEN,
        CATEGORIES_SCREEN,
        Arc::new(CategoryRows(Arc::clone(picker))),
    );
    let p = Arc::clone(picker);
    let pick: list_class::Action = Arc::new(move |cx: Cx, _screen: u32| -> ActionFuture {
        let p = Arc::clone(&p);
        Box::pin(async move {
            choose(&cx, &p).await;
            Ok(())
        })
    });
    let maps = ListClass::with_rows(MAPS_SCREEN, MAPS_SCREEN, Arc::new(MapRows(Arc::clone(picker))))
        .action(pick)
        .action_buttons(&[]);
    [categories, maps]
}

/// A on a map: remember it, then end the game. Nothing changes when the
/// game cannot be ended.
pub async fn choose(cx: &Cx, picker: &Picker) {
    let Some(id) = picker.take_pressed() else {
        return;
    };
    let cat = catalogue();
    let map = cat.all().find(|m| m.id == id).cloned();
    let name = map
        .as_ref()
        .map_or_else(|| format!("{id:#x}"), |m| m.name.clone());
    if let Some(m) = &map {
        // With Clankers on, the map's nav mesh is asked for now, as Game
        // Setup's Change Map does (docs/game/bots.md §10 stage 2).
        crate::clankers::map_picked(&m.short_name());
    }
    *lock(&PENDING) = Some(Pending::new(id, name.clone(), picker.port(), Instant::now()));
    match crate::firefight::guest::end_game(cx).await {
        Ok(()) => eprintln!(
            "pause change map: {name} ({id:#x}) picked; the game ends, then the lobby takes the map and starts"
        ),
        Err(e) => {
            *lock(&PENDING) = None;
            eprintln!("pause change map: the game was not ended ({e:#}); nothing changed");
        }
    }
}

// ---- the next game ----

/// Give up on starting the next game this long after the pick: the lobby
/// keeps the map, and the leader presses Start.
pub const TIMEOUT: Duration = Duration::from_secs(300);
/// Wait this long after the lobby's map is set before the start check,
/// so the change reaches the other consoles first.
pub const SETTLE: Duration = Duration::from_secs(2);
/// The least time between two of our guest calls.
pub const RETRY: Duration = Duration::from_millis(500);
/// What the countdown is asked with: the lobby's Start Game's arguments
/// (docs/live/dedicated-host.md, "Playerless lobby").
pub const COUNTDOWN_ARGS: [u32; 2] = [10, 3];

/// The lobby as the driver reads it each UI update.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lobby {
    /// The game's options mode is the main menu's.
    pub menu: bool,
    pub lobby_on: bool,
    pub networking: bool,
    /// The life cycle `[0x527334]`: 1 and 2 a custom game's lobby, 3 its
    /// game, 4 after it.
    pub session_type: u32,
    /// The lobby's map (`LOBBY_MAP`).
    pub map: u32,
}

impl Lobby {
    pub fn read(m: &dyn GuestMemory) -> Lobby {
        use bots::lobby;
        let f = lobby::membership(m).unwrap_or_default();
        Lobby {
            menu: crate::level_boot::active_mode(&crate::layout_glue::ReadGm(m))
                == Some(crate::level_boot::MODE_MAINMENU),
            lobby_on: f.lobby_on,
            networking: f.networking,
            session_type: f.session_type,
            map: m.u32(crate::h1_options_row::LOBBY_MAP).unwrap_or(u32::MAX),
        }
    }

    /// Back in the pregame lobby: the main menu with a lobby, a custom
    /// game's pregame life cycle (1 or 2) when networked. The game (3) and
    /// the postgame (4) wait.
    pub fn pregame(self) -> bool {
        self.menu && self.lobby_on && (!self.networking || matches!(self.session_type, 1 | 2))
    }
}

/// Where the next game is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The game is ending; the lobby's map is set once it is back.
    Ending,
    /// The lobby has the map (since `at`); the countdown is next.
    MapSet { at: Instant },
}

/// The map picked from the pause menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub map: u32,
    pub name: String,
    /// The controller that pressed the row: the countdown's.
    pub port: u32,
    pub since: Instant,
    pub step: Step,
    /// No guest call before this.
    pub next_try: Instant,
}

/// What the driver does this UI update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    Wait,
    /// Set the lobby's map to this id (`LOBBY_SET_MAP`).
    SetMap(u32),
    /// Ask retail's start check (`LOBBY_START_STATE`).
    StartCheck,
    /// Stop, and why.
    GiveUp(String),
}

/// What the start check's answer leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// Ask for the countdown for this controller.
    Countdown(u32),
    /// Not yet.
    Wait,
    /// The countdown runs: finished.
    Done(String),
    GiveUp(String),
}

impl Pending {
    pub fn new(map: u32, name: String, port: u32, now: Instant) -> Pending {
        Pending {
            map,
            name,
            port,
            since: now,
            step: Step::Ending,
            next_try: now,
        }
    }

    pub fn next(&mut self, lobby: &Lobby, now: Instant) -> Next {
        if now.duration_since(self.since) >= TIMEOUT {
            return Next::GiveUp(format!(
                "the next game did not start within {} s",
                TIMEOUT.as_secs()
            ));
        }
        if !lobby.pregame() || now < self.next_try {
            return Next::Wait;
        }
        match self.step {
            Step::Ending if lobby.map == self.map => {
                self.step = Step::MapSet { at: now };
                Next::Wait
            }
            Step::Ending => {
                self.next_try = now + RETRY;
                Next::SetMap(self.map)
            }
            // Changed again by someone: set it again.
            Step::MapSet { .. } if lobby.map != self.map => {
                self.step = Step::Ending;
                Next::Wait
            }
            Step::MapSet { at } if now.duration_since(at) < SETTLE => Next::Wait,
            Step::MapSet { .. } => {
                self.next_try = now + RETRY;
                Next::StartCheck
            }
        }
    }

    /// `LOBBY_SET_MAP` answered (AL nonzero: set).
    pub fn map_set(&mut self, set: bool, now: Instant) {
        if set {
            self.step = Step::MapSet { at: now };
        }
    }

    /// Retail's start state (`bots::lobby`'s table): 9 ready, 10 a
    /// countdown runs, 8 this console does not lead the party.
    pub fn start_state(&self, state: u32) -> Start {
        match state {
            bots::lobby::READY => Start::Countdown(self.port),
            bots::lobby::COUNTING_DOWN => Start::Done(format!("the countdown runs on {}", self.name)),
            8 => Start::GiveUp("this console does not lead the party".into()),
            _ => Start::Wait,
        }
    }

    /// The countdown answered (AL nonzero: started).
    pub fn countdown(&self, started: bool) -> Start {
        if started {
            Start::Done(format!("the countdown started on {}", self.name))
        } else {
            Start::Wait
        }
    }
}

/// Why a snapshot waits: a pick is under way.
pub fn snapshot_live() -> Option<String> {
    lock(&PENDING)
        .as_ref()
        .map(|p| format!("a Change Map to {} from the pause menu is under way", p.name))
}

fn finish(why: &str, done: bool) {
    let p = lock(&PENDING).take();
    let name = p.map_or_else(String::new, |p| p.name);
    if done {
        eprintln!("pause change map: {why}");
    } else {
        eprintln!(
            "pause change map: gave up ({why}); the lobby keeps {name} if it was set, press Start"
        );
    }
}

fn settle(s: Start) {
    match s {
        Start::Done(why) => finish(&why, true),
        Start::GiveUp(why) => finish(&why, false),
        Start::Countdown(_) | Start::Wait => {}
    }
}

/// The guest calls one driver step makes.
async fn drive(cx: &Cx, next: Next) -> Result<(), EntryError> {
    match next {
        Next::SetMap(id) => {
            let set = cx
                .call(GuestCall::new(points::LOBBY_SET_MAP.base).esi(id).args([u32::MAX]))
                .await?
                .eax as u8
                != 0;
            if let Some(p) = lock(&PENDING).as_mut() {
                eprintln!(
                    "pause change map: the lobby's map {} {}",
                    p.name,
                    if set { "is set" } else { "was refused; trying again" }
                );
                p.map_set(set, Instant::now());
            }
        }
        Next::StartCheck => {
            let state = cx
                .call(GuestCall::new(points::LOBBY_START_STATE.base).args([0]))
                .await?
                .eax;
            let answer = lock(&PENDING).as_ref().map(|p| p.start_state(state));
            match answer {
                Some(Start::Countdown(port)) => {
                    let started = cx
                        .call(
                            GuestCall::new(crate::host_lobby::CONTROLLER_COUNTDOWN)
                                .eax(port)
                                .args(COUNTDOWN_ARGS),
                        )
                        .await?
                        .eax as u8
                        != 0;
                    let s = lock(&PENDING).as_ref().map(|p| p.countdown(started));
                    if let Some(s) = s {
                        if s == Start::Wait {
                            eprintln!("pause change map: retail refused the countdown; trying again");
                        }
                        settle(s);
                    }
                }
                Some(s) => settle(s),
                None => {}
            }
        }
        Next::Wait | Next::GiveUp(_) => {}
    }
    Ok(())
}

/// The hooks (feature [`FEATURE`]).
pub struct PauseChangeMap(pub Arc<Picker>);

impl Feature for PauseChangeMap {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "Change Map on the multiplayer pause menu")
            .doc("docs/game/settings-menu.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let base = b.guest_alloc((IDS.len * SLOT_LEN).next_multiple_of(0x1000), Permissions::RW)?;
        TEXTS.base.store(base, Ordering::SeqCst);
        TEXTS.set(ROW_SLOT, ROW_LABEL);
        TEXTS.set(CATEGORIES_TITLE_SLOT, CATEGORIES_TITLE);
        b.locale().source("pause change map", |id| TEXTS.address(id));
        INSTALLED.store(true, Ordering::SeqCst);

        // The row: noted at entry, added once the retail rows are in.
        b.raw()
            .observe_before(&points::MP_MENU_LIST_CTOR, "note the GAME MENU list", |o| {
                BUILDING.store(o.cpu.esi, Ordering::SeqCst);
            });
        b.raw()
            .after(&points::MP_MENU_LIST_CTOR, "add the Change Map row", |c| {
                let list = BUILDING.swap(0, Ordering::SeqCst);
                if list == 0 {
                    return Ok(());
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let Ok(array) = ListHandler::at(list).rows(&*mem) else {
                    return Ok(());
                };
                let words = actions(&*mem, array).unwrap_or_default();
                let can_open = opening(lobby_start::mode(), &catalogue()).is_some();
                if !offered(&words, Menu::read(&*mem), can_open) {
                    return Ok(());
                }
                match add_row(&mut *mem, array) {
                    Ok(n) => eprintln!("pause change map: GAME MENU has Change Map ({n} rows)"),
                    Err(why) => eprintln!("pause change map: no Change Map row: {why}"),
                }
                Ok(())
            });

        b.raw()
            .before(&points::MP_MENU_ROW_TEXT, "name the Change Map row", |c| {
                let Some(item) = c.arg_u32("item") else {
                    return Ok(Flow::Continue);
                };
                let ours = {
                    let mem = c.ctx.runtime().memory();
                    crate::pause_menu::item_action(&*mem, c.cpu.ecx, item) == Some(CHANGE_MAP_ACTION)
                };
                if !ours {
                    return Ok(Flow::Continue);
                }
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        cx.write(|m| TEXTS.write(m))?;
                        list_class::draw_texts(&cx, item, vec![(0, IDS.id(ROW_SLOT))]).await?;
                        Ok(Ret::new(0, 8))
                    })
                })
            })
            // Each answers only its own action words.
            .after_feature("pause-mp-rows")
            .after_feature("forge-screens");

        let picker = Arc::clone(&self.0);
        b.raw()
            .before(&points::MP_MENU_ROW_CHOSEN, "open the Change Map picker", move |c| {
                let (Some(chain), Some(row)) = (c.arg_u32("chain"), c.arg_u32("row")) else {
                    return Ok(Flow::Continue);
                };
                let list = c.cpu.ecx;
                let (word, mask) = {
                    let mem = c.ctx.runtime().memory();
                    (
                        crate::pause_menu::chosen_action(&*mem, list, row),
                        list_class::player_mask(&*mem, chain).ok().flatten(),
                    )
                };
                if word != Some(CHANGE_MAP_ACTION) {
                    return Ok(Flow::Continue);
                }
                // The original would run End Game's press for our word.
                let open = opening(lobby_start::mode(), &catalogue());
                let (Some(open), Some(mask)) = (open, mask) else {
                    eprintln!("pause change map: nothing to open (picker {open:?}, player {mask:?})");
                    return c.ret(8).map(Flow::Return);
                };
                picker.open(open, mask.trailing_zeros());
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        list_class::push(&cx, chain, list.wrapping_sub(ui::LIST), open.screen())
                            .await?;
                        Ok(Ret::new(0, 8))
                    })
                })
            })
            .after_feature("forge-screens");

        // The next game: each UI update, back in the lobby, the map and
        // then the countdown. The UI update runs in the menus and in a game.
        b.raw()
            .before(&points::UI_UPDATE, "pause change map: start the next game", |c| {
                if PASSING.load(Ordering::SeqCst) {
                    return Ok(Flow::Continue);
                }
                let next = {
                    let mut pending = lock(&PENDING);
                    let Some(p) = pending.as_mut() else {
                        return Ok(Flow::Continue);
                    };
                    let lobby = Lobby::read(&*c.ctx.runtime().memory());
                    p.next(&lobby, Instant::now())
                };
                match next {
                    Next::Wait => return Ok(Flow::Continue),
                    Next::GiveUp(why) => {
                        finish(&why, false);
                        return Ok(Flow::Continue);
                    }
                    Next::SetMap(_) | Next::StartCheck => {}
                }
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        let elapsed = cx.arg(0)?;
                        if let Err(e) = drive(&cx, next).await {
                            eprintln!("pause change map: {e}");
                        }
                        PASSING.store(true, Ordering::SeqCst);
                        let out = cx
                            .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                            .await;
                        PASSING.store(false, Ordering::SeqCst);
                        Ok(Ret::new(out?.eax, 4))
                    })
                })
            })
            // Inside the other UI update drivers: each lets its own call
            // through, so ours runs within theirs.
            .after_feature("live-join-link")
            .after_feature("theater-link")
            .after_feature("forge-screens")
            .after_feature("lab-takeover")
            .optional();

        b.snapshot(SnapshotPolicy::section_forbid_while(
            snapshot_live,
            Vec::new,
            |_, _| {
                TEXTS.rewrite_all();
                Ok(())
            },
        ));
        Ok(())
    }
}

/// Registers the picker's classes and the hooks, unless
/// `features.pause-change-map` is off. Whether they were added.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<bool> {
    if !registry.switches().feature(FEATURE, true) {
        eprintln!("pause change map: features.{FEATURE} is off");
        return Ok(false);
    }
    let picker = Arc::new(Picker::default());
    let [categories, maps] = classes(&picker);
    if !(registry.add(&categories)? && registry.add(&maps)?) {
        eprintln!("pause change map: a picker class is off; no Change Map row");
        return Ok(false);
    }
    Ok(registry.add(&PauseChangeMap(picker))?)
}

#[cfg(test)]
mod tests;
