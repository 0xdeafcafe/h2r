//! **Game Setup** on the multiplayer pause menu (GAME MENU, screen 195),
//! for the party leader of a custom game: change the map, the rules and
//! the Quick Options for the next game, and the Clankers at once
//! (docs/game/settings-menu.md "Game Setup from the pause menu").
//!
//! - **The row.** Retail shows End Game (action 5) only on the console
//!   that leads the party (`0x592f0`) outside matchmaking (lobby mode
//!   `0x19989d` not 6). An `after` hook on the list's constructor
//!   (`MP_MENU_LIST_CTOR`) adds our row ([`SETUP_ACTION`]) just above End
//!   Game when the list has it, this console hosts the game (or plays it
//!   offline) and Game Setup has a row to show ([`offered`],
//!   [`setup_rows`]). One row, not four: GAME MENU's list is not shown to
//!   scroll past its item widgets. The rows' array has room for it:
//!   `layout_glue` asks [`list_count`] when the array is made.
//! - **Text and press.** `MP_MENU_ROW_TEXT` draws our label;
//!   `MP_MENU_ROW_CHOSEN` runs action 5's press (End Game) for any word it
//!   does not know, so ours is always taken before the original. The
//!   press reads what the rows offer (the built-in variants, the lobby's
//!   variant) and opens Game Setup ([`setup`]) on GAME MENU's window.
//! - **Game Setup** lists Change Map, Change Rules, Quick Options and
//!   Clanker Options. The first three fill the visit's [`Draft`]; nothing
//!   changes in the running game. B on Game Setup with a draft asks
//!   **Restart with these settings** or **Keep playing**; Keep playing
//!   drops the draft. Clanker Options' changes apply at once
//!   ([`clanker_rows`]).
//! - **The restart.** [`commit`] records the draft ([`Pending`]) and ends
//!   the game through the engine's round-over path
//!   (`crate::firefight::guest::end_game`). Back in the pregame lobby
//!   ([`Lobby::pregame`]) the UI update hook sets the lobby's map
//!   (`LOBBY_SET_MAP`, as retail's Change Map does, so it replicates), then
//!   its variant (`host_lobby::SET_VARIANT`, the lobby variant setter), and
//!   once retail's start check (`LOBBY_START_STATE`) answers ready, starts
//!   the countdown for the pressing controller
//!   (`host_lobby::CONTROLLER_COUNTDOWN`). What the draft leaves out stays
//!   the lobby's.

pub mod clanker_rows;
pub mod game_types;
pub mod quick;
pub mod setup;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use guest_layout::datum::DatumArray;
use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use guest_layout::view::ui::{self, ListHandler};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use runtime::guest::Permissions;
use variants::record::{GameType, GameVariant, RECORD_SIZE};

use crate::list_class;
use crate::lobby_start;
use crate::map_select::{self, Catalogue, Category, Map};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::screen::{SLOT_LEN, SlotTexts};

/// The feature (`features.pause-change-map` switches the row, Game Setup
/// and the restart).
pub const FEATURE: &str = "pause-change-map";
/// Our list classes, by the names `Push` rows use; each its own feature.
pub const SETUP_SCREEN: &str = "pause-game-setup";
pub const CATEGORIES_SCREEN: &str = "pause-map-categories";
pub const TYPES_SCREEN: &str = "pause-game-types";
pub const RULES_SCREEN: &str = "pause-game-rules";
pub const QUICK_SCREEN: &str = "pause-quick-options";
pub const CLANKERS_SCREEN: &str = "pause-clanker-options";

/// GAME MENU's rows' datum array's name (the constructor's
/// `LIST_DATA_NEW`).
pub const ARRAY_NAME: u32 = 0x0045_93a8;
/// Retail's End Game action word.
pub const END_GAME: u16 = 5;
/// Our row's action word: clear of retail's 0 to 5.
pub const SETUP_ACTION: u16 = 0x30;
const ACTION_WORD: u32 = 2;

/// Our strings: the row's label, the lists' titles, then each list's rows.
pub const IDS: Family = ids::family("pause change map");
const ROW_SLOT: u32 = 0;
const SETUP_TITLE_SLOT: u32 = 1;
const CATEGORIES_TITLE_SLOT: u32 = 2;
const TYPES_TITLE_SLOT: u32 = 3;
const QUICK_TITLE_SLOT: u32 = 4;
const CLANKER_TITLE_SLOT: u32 = 5;
const CATEGORY_SLOTS: u32 = 6;
const TYPE_SLOTS: u32 = CATEGORY_SLOTS + Category::ALL.len() as u32;
const SETUP_SLOTS: u32 = TYPE_SLOTS + game_types::ORDER.len() as u32;
const QUICK_SLOTS: u32 = SETUP_SLOTS + SetupRow::ALL.len() as u32;
const CLANKER_SLOTS: u32 = QUICK_SLOTS + quick::Setting::ALL.len() as u32;
/// Clanker Options' rows: one a setting of `clankers::DEFS` (a static,
/// which a constant can't read; a test holds the two equal).
const CLANKER_ROWS: u32 = 8;
/// Change Rules' list (a game type's variants): its title, the legend of
/// it and of the game types' list, and a slot per row it shows; a list
/// longer than the window reuses them as it scrolls.
const RULES_TITLE_SLOT: u32 = CLANKER_SLOTS + CLANKER_ROWS;
const FOOTER_SLOT: u32 = RULES_TITLE_SLOT + 1;
const RULES_SLOTS: u32 = FOOTER_SLOT + 1;
/// The rows Select Game and Change Rules show at once: Quick Options'
/// dialog's list (`dialog::QUICK_LIST_ROWS`), the most its panel holds
/// inside the 640 x 480 safe area. A variant list longer than this scrolls.
pub const LIST_ROWS: usize = crate::ui_kit::dialog::QUICK_LIST_ROWS;
/// The slots in use.
pub const SLOTS: u32 = RULES_SLOTS + LIST_ROWS as u32;

pub const ROW_LABEL: &str = "Game Setup";
const SETUP_TITLE: &str = "Game Setup";
const CATEGORIES_TITLE: &str = "Select Map";
const TYPES_TITLE: &str = "Select Game";
const QUICK_TITLE: &str = "Quick Options";
const CLANKER_TITLE: &str = "Clanker Options";

static TEXTS: LazyLock<SlotTexts> = LazyLock::new(|| SlotTexts::new(IDS));
/// Set once the feature is built: GAME MENU's array grows by our row.
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// The GAME MENU list being built (ESI at the constructor's entry).
static BUILDING: AtomicU32 = AtomicU32::new(0);
/// The settings picked and how far the next game is.
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
/// Our own UI update call, which the hook lets through.
static PASSING: AtomicBool = AtomicBool::new(false);
/// Guest memory for the record the lobby variant setter is given.
static RECORD_AT: AtomicU32 = AtomicU32::new(0);

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
/// console runs the game, and Game Setup has something to show.
pub fn offered(actions: &[u16], menu: Menu, can_open: bool) -> bool {
    actions.contains(&END_GAME)
        && !actions.contains(&SETUP_ACTION)
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
    out.insert(at, SETUP_ACTION);
    out
}

fn elements(m: &dyn GuestMemory, a: DatumArray) -> Result<Vec<u32>, MemFault> {
    let high = a.header_field(m, guest_layout::datum::layout::HIGH_WATER)?;
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

// ---- what Game Setup offers ----

/// What Change Map opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    /// The categories, then a category's maps.
    Categories,
    /// One category's maps straight away.
    Maps(Category),
}

/// The categories with maps a Multiplayer lobby lists, in order.
pub fn categories(cat: &Catalogue) -> Vec<Category> {
    cat.categories()
        .into_iter()
        .filter(|c| !cat.maps(*c).is_empty())
        .collect()
}

/// What Change Map opens for a lobby in `mode`: the categories, or the one
/// category's maps when there is one, in every mode. `None` with nothing to
/// pick.
pub fn opening(_mode: lobby_start::Mode, cat: &Catalogue) -> Option<Opening> {
    match categories(cat).as_slice() {
        [] => None,
        [one] => Some(Opening::Maps(*one)),
        _ => Some(Opening::Categories),
    }
}

/// The maps a category's picker shows, in the catalogue's order.
pub fn maps(cat: &Catalogue, c: Category) -> Vec<Map> {
    cat.playable_maps(c)
}

fn catalogue() -> Arc<Catalogue> {
    map_select::current().unwrap_or_default()
}

/// Which of Game Setup's rows this visit can show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Avail {
    pub map: Option<Opening>,
    pub rules: bool,
    pub quick: bool,
    pub clankers: bool,
}

/// What the rows offer in a lobby of `mode`: Change Map what [`opening`]
/// opens; Change Rules the built-in variants, except in a Firefight lobby
/// (whose Game Setup has no Change Rules); Quick Options the lobby's
/// variant, when it can be read; Clanker Options the Clankers page's
/// settings, when they are installed.
pub fn avail(
    mode: lobby_start::Mode,
    cat: &Catalogue,
    variants: &[game_types::Listed],
    base: Option<&GameVariant>,
    clankers: bool,
) -> Avail {
    Avail {
        map: opening(map_select::variant_mode(mode, base), cat),
        rules: !matches!(mode, lobby_start::Mode::Firefight) && !variants.is_empty(),
        quick: base.is_some(),
        clankers,
    }
}

/// A row of Game Setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupRow {
    ChangeMap,
    ChangeRules,
    QuickOptions,
    ClankerOptions,
}

impl SetupRow {
    /// In the lobby's Game Setup order.
    pub const ALL: [SetupRow; 4] = [
        SetupRow::ChangeMap,
        SetupRow::ChangeRules,
        SetupRow::QuickOptions,
        SetupRow::ClankerOptions,
    ];

    /// The lobby's own labels.
    pub fn label(self) -> &'static str {
        match self {
            SetupRow::ChangeMap => "Change Map...",
            SetupRow::ChangeRules => "Change Rules...",
            SetupRow::QuickOptions => "Quick Options...",
            SetupRow::ClankerOptions => "Clanker Options...",
        }
    }

    fn shown(self, a: Avail) -> bool {
        match self {
            SetupRow::ChangeMap => a.map.is_some(),
            SetupRow::ChangeRules => a.rules,
            SetupRow::QuickOptions => a.quick,
            SetupRow::ClankerOptions => a.clankers,
        }
    }

    /// The row as one line: its label, or what the draft picked for it,
    /// two spaces after the label as retail's settings rows read.
    pub fn line(self, d: &Draft) -> String {
        let bare = self.label().trim_end_matches("...");
        let value = match self {
            SetupRow::ChangeMap => d.map.as_ref().map(|(_, n)| n.clone()),
            SetupRow::ChangeRules => d.rules.clone(),
            SetupRow::QuickOptions => (!d.quick.is_empty()).then(|| "Changed".to_owned()),
            SetupRow::ClankerOptions => None,
        };
        match value {
            Some(v) => format!("{bare}  {v}"),
            None => self.label().to_owned(),
        }
    }
}

/// Game Setup's rows for a visit that can show `a`, in order.
pub fn setup_rows(a: Avail) -> Vec<SetupRow> {
    SetupRow::ALL.into_iter().filter(|r| r.shown(a)).collect()
}

/// The variant the lobby plays: the squad session's (`+0x4db0`), else the
/// game's own copy (`G+0x13c`); `None` when neither reads as a variant of
/// a listed game type.
pub fn lobby_variant(m: &dyn GuestMemory) -> Option<GameVariant> {
    /// The game's options' variant (docs/game/game-options.md).
    const GAME_VARIANT: u32 = 0x13c;
    let session = m
        .u32(bots::lobby::SESSION)
        .ok()
        .filter(|s| *s != 0)
        .map(|s| s + bots::phantom::session::at::VARIANT);
    let game = m
        .u32(crate::level_boot::GAME_GLOBALS)
        .ok()
        .filter(|g| *g != 0)
        .map(|g| g + GAME_VARIANT);
    [session, game].into_iter().flatten().find_map(|at| {
        let mut raw = [0u8; RECORD_SIZE];
        m.read(at, &mut raw).ok()?;
        GameVariant::from_bytes(&raw)
            .ok()
            .filter(|v| !matches!(v.game_type(), GameType::Other(_)))
    })
}

// ---- the visit ----

/// What a visit to Game Setup changes for the next game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    /// The map picked: its list id and name.
    pub map: Option<(u32, String)>,
    /// The variant the next game plays: the one Change Rules picked, or
    /// the lobby's, with the Quick Options changes on it.
    pub variant: Option<GameVariant>,
    /// The name of the variant Change Rules picked.
    pub rules: Option<String>,
    /// The Quick Options settings changed, by label.
    pub quick: Vec<&'static str>,
}

impl Draft {
    /// Nothing to restart for.
    pub fn is_empty(&self) -> bool {
        self.map.is_none() && self.variant.is_none()
    }

    pub fn pick_map(&mut self, id: u32, name: &str) {
        self.map = Some((id, name.to_owned()));
    }

    /// A variant from Change Rules: earlier Quick Options changes were to
    /// the old one, so they go.
    pub fn pick_rules(&mut self, l: &game_types::Listed) {
        self.variant = Some(l.variant.clone());
        self.rules = Some(l.name.clone());
        self.quick.clear();
    }

    /// The variant Quick Options shows: the draft's, else `base`.
    pub fn working<'a>(&'a self, base: Option<&'a GameVariant>) -> Option<&'a GameVariant> {
        self.variant.as_ref().or(base)
    }

    /// Quick Options' `setting` at `value`, on [`Draft::working`]; what to
    /// log, or why it was refused (the draft unchanged).
    pub fn set_quick(
        &mut self,
        base: Option<&GameVariant>,
        setting: quick::Setting,
        id: variants::record::SettingId,
        value: i32,
    ) -> Result<String, String> {
        let from = self
            .working(base)
            .ok_or("no variant to change")?;
        let v = quick::apply(from, id, value)?;
        self.variant = Some(v);
        if !self.quick.contains(&setting.label()) {
            self.quick.push(setting.label());
        }
        Ok(format!("{} {}", setting.label(), setting.text(value)))
    }

    /// What the restart starts, for the confirm dialog.
    pub fn summary(&self) -> String {
        let with = match (&self.rules, self.quick.is_empty()) {
            (Some(r), true) => Some(r.clone()),
            (Some(r), false) => Some(format!("{r} and your Quick Options")),
            (None, false) => Some("this game's rules and your Quick Options".to_owned()),
            (None, true) => None,
        };
        let next = match (&self.map, with) {
            (Some((_, m)), Some(w)) => format!(" on {m} with {w}"),
            (Some((_, m)), None) => format!(" on {m}"),
            (None, Some(w)) => format!(" with {w}"),
            (None, None) => String::new(),
        };
        format!("The game ends for everyone and the next one starts{next}.")
    }

    /// The next game, pressed by controller `port` at `now`; `None` with
    /// nothing to change.
    pub fn pending(&self, port: u32, now: Instant) -> Option<Pending> {
        if self.is_empty() {
            return None;
        }
        let mut name: Vec<String> = Vec::new();
        if let Some((_, m)) = &self.map {
            name.push(m.clone());
        }
        match (&self.rules, &self.variant) {
            (Some(r), _) => name.push(r.clone()),
            (None, Some(v)) => name.push(game_types::display_name(&v.name())),
            (None, None) => {}
        }
        if !self.quick.is_empty() {
            name.push(format!("Quick Options: {}", self.quick.join(", ")));
        }
        Some(Pending::new(
            self.map.as_ref().map(|(id, _)| *id),
            self.variant.as_ref().map(|v| Box::new(v.to_bytes())),
            name.join(", "),
            port,
            now,
        ))
    }
}

/// The confirm dialog's choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm {
    Restart,
    KeepPlaying,
}

impl Confirm {
    pub const ALL: [Confirm; 2] = [Confirm::Restart, Confirm::KeepPlaying];
    pub fn label(self) -> &'static str {
        match self {
            Confirm::Restart => "Restart with these settings",
            Confirm::KeepPlaying => "Keep playing",
        }
    }
}

/// One visit to Game Setup: what it offers, the draft, and the screens
/// the pickers close.
#[derive(Clone, Debug, Default)]
pub struct VisitState {
    maps: Arc<Catalogue>,
    pub avail: Avail,
    pub rules: Vec<game_types::Listed>,
    /// The lobby's variant when Game Setup opened.
    pub base: Option<GameVariant>,
    pub draft: Draft,
    /// The categories or game types list a picker was opened from.
    pub list_screen: Option<u32>,
    /// The game type whose variants Change Rules lists.
    pub group: Option<game_types::Group>,
    /// Game Setup itself, while the confirm dialog is up.
    pub setup_screen: Option<u32>,
    pub confirm: Option<Confirm>,
}

/// What Game Setup's screens share.
#[derive(Default)]
pub struct Visit {
    port: AtomicU32,
    state: Mutex<VisitState>,
}

impl Visit {
    /// A new visit for controller `port`: the draft starts empty.
    pub fn open(&self, port: u32, mut avail: Avail, rules: Vec<game_types::Listed>, base: Option<GameVariant>) {
        let mode = map_select::variant_mode(lobby_start::mode(), base.as_ref());
        let maps = catalogue().for_variant(mode, base.as_ref());
        if base.is_some() && avail.map.is_some() {
            avail.map = opening(mode, &maps);
        }
        self.port.store(port, Ordering::SeqCst);
        *lock(&self.state) = VisitState {
            maps,
            avail,
            rules,
            base,
            ..VisitState::default()
        };
    }
    pub fn port(&self) -> u32 {
        self.port.load(Ordering::SeqCst)
    }
    pub fn with<T>(&self, f: impl FnOnce(&mut VisitState) -> T) -> T {
        f(&mut lock(&self.state))
    }
    pub fn draft(&self) -> Draft {
        self.with(|s| s.draft.clone())
    }
}

/// Restart with the visit's draft: remember it, then end the game.
/// Nothing changes when the game cannot be ended.
pub async fn commit(cx: &Cx, visit: &Visit) {
    let Some(p) = visit.draft().pending(visit.port(), Instant::now()) else {
        return;
    };
    let name = p.name.clone();
    *lock(&PENDING) = Some(p);
    match crate::firefight::guest::end_game(cx).await {
        Ok(()) => {
            visit.with(|s| s.draft = Draft::default());
            eprintln!(
                "pause game setup: {name}; the game ends, then the lobby takes the settings and starts"
            );
        }
        Err(e) => {
            *lock(&PENDING) = None;
            eprintln!("pause game setup: the game was not ended ({e:#}); nothing changed");
        }
    }
}

// ---- the next game ----

/// Give up on starting the next game this long after the restart: the
/// lobby keeps what was set, and the leader presses Start.
pub const TIMEOUT: Duration = Duration::from_secs(300);
/// Wait this long after the lobby's settings are set before the start
/// check, so the change reaches the other consoles first.
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
    /// The game is ending; the lobby's settings are set once it is back.
    Ending,
    /// The lobby has the settings (since `at`); the countdown is next.
    Set { at: Instant },
}

/// The settings picked from the pause menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// The map's list id, when the draft changed it.
    pub map: Option<u32>,
    /// The variant record, when the draft changed it.
    pub variant: Option<Box<[u8; RECORD_SIZE]>>,
    /// The lobby variant setter took [`Pending::variant`].
    pub variant_set: bool,
    /// What changes, for the log.
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
    /// Set the lobby's variant to this record (`host_lobby::SET_VARIANT`).
    SetVariant(Box<[u8; RECORD_SIZE]>),
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
    pub fn new(
        map: Option<u32>,
        variant: Option<Box<[u8; RECORD_SIZE]>>,
        name: String,
        port: u32,
        now: Instant,
    ) -> Pending {
        Pending {
            map,
            variant,
            variant_set: false,
            name,
            port,
            since: now,
            step: Step::Ending,
            next_try: now,
        }
    }

    /// The map first, then the variant, then the start check once both
    /// have settled.
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
        if let Some(map) = self.map.filter(|m| *m != lobby.map) {
            if matches!(self.step, Step::Set { .. }) {
                // Changed again by someone: set it again.
                self.step = Step::Ending;
                return Next::Wait;
            }
            self.next_try = now + RETRY;
            return Next::SetMap(map);
        }
        if let Some(v) = self.variant.as_ref().filter(|_| !self.variant_set) {
            self.next_try = now + RETRY;
            return Next::SetVariant(v.clone());
        }
        match self.step {
            Step::Ending => {
                self.step = Step::Set { at: now };
                Next::Wait
            }
            Step::Set { at } if now.duration_since(at) < SETTLE => Next::Wait,
            Step::Set { .. } => {
                self.next_try = now + RETRY;
                Next::StartCheck
            }
        }
    }

    /// `LOBBY_SET_MAP` answered (AL nonzero: set).
    pub fn map_set(&mut self, set: bool, now: Instant) {
        if set {
            self.step = Step::Set { at: now };
        }
    }

    /// The lobby variant setter answered (AL nonzero: taken).
    pub fn variant_taken(&mut self, set: bool, now: Instant) {
        if set {
            self.variant_set = true;
            self.step = Step::Set { at: now };
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

/// Why a snapshot waits: a restart is under way.
pub fn snapshot_live() -> Option<String> {
    lock(&PENDING)
        .as_ref()
        .map(|p| format!("a restart from the pause menu's Game Setup ({}) is under way", p.name))
}

fn finish(why: &str, done: bool) {
    let p = lock(&PENDING).take();
    let name = p.map_or_else(String::new, |p| p.name);
    if done {
        eprintln!("pause game setup: {why}");
    } else {
        eprintln!(
            "pause game setup: gave up ({why}); the lobby keeps what was set of {name}, press Start"
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
                    "pause game setup: the lobby's map {}",
                    if set { "is set" } else { "was refused; trying again" }
                );
                p.map_set(set, Instant::now());
            }
        }
        Next::SetVariant(record) => {
            let at = RECORD_AT.load(Ordering::SeqCst);
            if at == 0 {
                finish("no memory for the variant record", false);
                return Ok(());
            }
            cx.write(|m| m.write(at, record.as_slice()))?;
            let set = cx
                .call(GuestCall::new(crate::host_lobby::SET_VARIANT.base).args([at]))
                .await?
                .eax as u8
                != 0;
            if let Some(p) = lock(&PENDING).as_mut() {
                eprintln!(
                    "pause game setup: the lobby's variant {}",
                    if set { "is set" } else { "was refused; trying again" }
                );
                p.variant_taken(set, Instant::now());
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
                            eprintln!("pause game setup: retail refused the countdown; trying again");
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

/// What a visit offers now, read from guest memory: the built-in
/// variants, the lobby's variant, and Game Setup's rows.
fn read_visit(m: &dyn GuestMemory) -> (Avail, Vec<game_types::Listed>, Option<GameVariant>) {
    let variants = game_types::read(m);
    let base = lobby_variant(m);
    let on = |screen: &str| list_class::loader_of(screen).is_some();
    let mut a = avail(lobby_start::mode(), &catalogue(), &variants, base.as_ref(), on(CLANKERS_SCREEN));
    // A row whose screen's class is off is left out.
    if a.map == Some(Opening::Categories) && !on(CATEGORIES_SCREEN) {
        a.map = None;
    }
    a.rules &= on(TYPES_SCREEN) && on(RULES_SCREEN);
    a.quick &= on(QUICK_SCREEN);
    (a, variants, base)
}

/// The hooks (feature [`FEATURE`]).
pub struct PauseChangeMap(pub Arc<Visit>);

impl Feature for PauseChangeMap {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "Game Setup on the multiplayer pause menu")
            .doc("docs/game/settings-menu.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let base = b.guest_alloc((IDS.len * SLOT_LEN).next_multiple_of(0x1000), Permissions::RW)?;
        TEXTS.base.store(base, Ordering::SeqCst);
        TEXTS.set(ROW_SLOT, ROW_LABEL);
        TEXTS.set(SETUP_TITLE_SLOT, SETUP_TITLE);
        TEXTS.set(CATEGORIES_TITLE_SLOT, CATEGORIES_TITLE);
        TEXTS.set(TYPES_TITLE_SLOT, TYPES_TITLE);
        TEXTS.set(QUICK_TITLE_SLOT, QUICK_TITLE);
        TEXTS.set(CLANKER_TITLE_SLOT, CLANKER_TITLE);
        b.locale().source("pause change map", |id| TEXTS.address(id));
        let record = b.guest_alloc(0x1000, Permissions::RW)?;
        RECORD_AT.store(record, Ordering::SeqCst);
        INSTALLED.store(true, Ordering::SeqCst);

        // The row: noted at entry, added once the retail rows are in.
        b.raw()
            .observe_before(&points::MP_MENU_LIST_CTOR, "note the GAME MENU list", |o| {
                BUILDING.store(o.cpu.esi, Ordering::SeqCst);
            });
        b.raw()
            .after(&points::MP_MENU_LIST_CTOR, "add the Game Setup row", |c| {
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
                let (a, _, _) = read_visit(&*mem);
                if !offered(&words, Menu::read(&*mem), !setup_rows(a).is_empty()) {
                    return Ok(());
                }
                match add_row(&mut *mem, array) {
                    Ok(n) => eprintln!("pause game setup: GAME MENU has Game Setup ({n} rows; {a:?})"),
                    Err(why) => eprintln!("pause game setup: no Game Setup row: {why}"),
                }
                Ok(())
            });

        b.raw()
            .before(&points::MP_MENU_ROW_TEXT, "name the Game Setup row", |c| {
                let Some(item) = c.arg_u32("item") else {
                    return Ok(Flow::Continue);
                };
                let ours = {
                    let mem = c.ctx.runtime().memory();
                    crate::pause_menu::item_action(&*mem, c.cpu.ecx, item) == Some(SETUP_ACTION)
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
            .after_feature("pause-mp-rows");

        let visit = Arc::clone(&self.0);
        b.raw()
            .before(&points::MP_MENU_ROW_CHOSEN, "open Game Setup", move |c| {
                let (Some(chain), Some(row)) = (c.arg_u32("chain"), c.arg_u32("row")) else {
                    return Ok(Flow::Continue);
                };
                let list = c.cpu.ecx;
                let (word, mask, (a, variants, base)) = {
                    let mem = c.ctx.runtime().memory();
                    (
                        crate::pause_menu::chosen_action(&*mem, list, row),
                        list_class::player_mask(&*mem, chain).ok().flatten(),
                        read_visit(&*mem),
                    )
                };
                if word != Some(SETUP_ACTION) {
                    return Ok(Flow::Continue);
                }
                // The original would run End Game's press for our word.
                let Some(mask) = mask.filter(|_| !setup_rows(a).is_empty()) else {
                    eprintln!("pause game setup: nothing to open ({a:?}, player {mask:?})");
                    return c.ret(8).map(Flow::Return);
                };
                eprintln!(
                    "pause game setup: opening Game Setup ({} variants listed, lobby variant {})",
                    variants.len(),
                    base.as_ref().map_or("unread".to_owned(), |v| v.name())
                );
                visit.open(mask.trailing_zeros(), a, variants, base);
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        list_class::push(&cx, chain, list.wrapping_sub(ui::LIST), SETUP_SCREEN).await?;
                        Ok(Ret::new(0, 8))
                    })
                })
            })
            .after_feature("forge-screens");

        // The next game: each UI update, back in the lobby, the map, the
        // variant and then the countdown. The UI update runs in the menus
        // and in a game.
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
                    Next::SetMap(_) | Next::SetVariant(_) | Next::StartCheck => {}
                }
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(async move {
                        let elapsed = cx.arg(0)?;
                        if let Err(e) = drive(&cx, next).await {
                            eprintln!("pause game setup: {e}");
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

/// Registers Game Setup's classes and the hooks, unless
/// `features.pause-change-map` is off. Clanker Options is listed when
/// `clankers` (the settings store the Clankers driver follows) is given.
/// Whether the hooks were added.
pub fn install(
    registry: &mut hooks::Registry,
    clankers: Option<Arc<settings::Store>>,
) -> anyhow::Result<bool> {
    if !registry.switches().feature(FEATURE, true) {
        eprintln!("pause change map: features.{FEATURE} is off");
        return Ok(false);
    }
    let visit = Arc::new(Visit::default());
    let classes = setup::classes(&visit, clankers);
    let Some((first, rest)) = classes.split_first() else {
        return Ok(false);
    };
    if !registry.add(first)? {
        eprintln!("pause change map: features.{SETUP_SCREEN} is off; no Game Setup row");
        return Ok(false);
    }
    for c in rest {
        if !registry.add(c)? {
            eprintln!("pause change map: features.{} is off; Game Setup leaves its row out", c.id);
        }
    }
    Ok(registry.add(&PauseChangeMap(visit))?)
}

#[cfg(test)]
mod tests;
