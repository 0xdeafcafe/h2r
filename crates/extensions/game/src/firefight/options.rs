//! Firefight in Game Setup (docs/game/firefight.md, "What the player
//! gets"): the settings, the options page, and the row on a custom game's
//! Game Setup that opens it.
//!
//! **The settings** (`firefight.*`) are kept in the profile store like any
//! other ([`DEFS`], in [`crate::setting_defs::ALL`]). [`Options::read`]
//! reads them and the runtime starts the match from them
//! ([`Options::definition`], [`Options::difficulty`]): on or off, which
//! wave set, one player team or two, how hard, how many lives to start
//! with, and whether each round switches on a skull.
//!
//! **The page** is the shared settings dialog ([`page`],
//! `crate::ui_kit::settings`), declared by
//! `content/ui/screens/firefight.yaml`: seven rows over [`DEFS`] on the
//! lobby's own settings dialog, Quick Options, as the Clankers page is
//! (`crate::clankers`). A row opens the value picker on A, and left and
//! right step through the choices as a shortcut. The Wave set row's A
//! pushes a carousel of cards instead ([`wave_sets_page`]), a card a set
//! with the map's picture and its rounds, waves and lives. No row is a
//! "< All >" arrow row.
//!
//! **The row** is "Firefight Options..." on a custom game's Game Setup, the
//! way "Clanker Options..." is ([`crate::clankers::SetupRow`], whose
//! helpers this uses): an `after` on the list's constructor adds a row with
//! [`ROW_ACTION`] to every list that has Change Map, `before` hooks draw its
//! text and take its press, which opens the page on the dialog's own
//! stack. Its hooks sit after the Clankers row's and Clankermatch's, so the
//! plan orders them. The lobby's mode (`crate::lobby_mode`) moves it under
//! Change Map in a Firefight lobby and hides it in a Forge one.
//!
//! **Not here.** The choices are the host's. A client of a distributed
//! session learns them from the session data (FF9), not from its own page.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ::firefight::{Def, Difficulty, builtin};
use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use settings::{Apply, Choice, Def as Setting, Kind, Scope, Store, Tier, Value};

use crate::h1_options_row::{self as h1row, CHANGE_MAP};
use crate::list_class::{self, Rows};
use crate::lobby_start;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::screen::{KitScreen, KitTexts};
use crate::ui_kit::settings::Bindings;

/// The page's feature (`features.firefight-options`) and screen name.
pub const FEATURE: &str = "firefight-options";
pub const SCREEN: &str = "firefight";
/// The Game Setup row's feature (`features.firefight-row`).
pub const ROW_FEATURE: &str = "firefight-row";
/// The page's string ids.
pub const IDS: Family = ids::family("firefight");
/// The Wave set row's carousel (docs/game/ui-screens.md, "Cards and the
/// card row"): its screen, its feature (`features.firefight-wavesets`)
/// and its string ids.
pub const WAVE_SETS_SCREEN: &str = "firefight.wavesets";
pub const WAVE_SETS_FEATURE: &str = "firefight-wavesets";
pub const WAVE_SETS_IDS: Family = ids::family("firefight wave sets");
/// Our Game Setup row's action: after the Clankers row's 11 and the
/// Clankermatch switch's 12 (retail uses 0 to 9; the press never reaches
/// the original's jump table).
pub const ROW_ACTION: i16 = 13;

const fn setting(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: Kind,
    default: Value,
) -> Setting {
    Setting {
        key,
        label,
        help,
        kind,
        default,
        env: None,
        flag: None,
        aliases: &[],
        parse_env: None,
        apply: Apply::Live,
        scope: Scope::Sim,
        tier: Tier::User,
        // On the Firefight page of a custom game's Game Setup, not in
        // Settings: chosen per game.
        menu: None,
    }
}

const fn choice(value: i64, name: &'static str, label: &'static str) -> Choice {
    Choice { value, name, label }
}

pub static ENABLED: Setting = setting(
    "firefight.enabled",
    "Firefight",
    "Whether a game on a Firefight map plays Firefight: waves of enemies, \
     shared lives and a score.",
    Kind::Bool,
    Value::Bool(true),
);
/// The values the store admits: the built-in definitions' positions
/// ([`builtin::definitions`]), then the map-specific sets' from
/// [`builtin::MAP_SET_BASE`]. The row offers the lobby map's own first
/// ([`wave_menu`]: Endless first, then the map's); a value the map has no
/// set for plays the built-ins ([`builtin::resolve`]).
pub static WAVE_SET: Setting = setting(
    "firefight.waveset",
    "Wave set",
    "Which set of waves is fought.",
    Kind::Choice(&[
        choice(4, "endless", "Endless"),
        choice(0, "holdout", "Holdout"),
        choice(1, "attrition", "Attrition"),
        choice(2, "last_stand", "Last Stand"),
        choice(3, "crossfire", "Crossfire"),
        choice(4, "endless", "Endless"),
        choice(5, "outbreak", "Outbreak"),
        choice(builtin::MAP_SET_BASE, "basin", "Basin"),
    ]),
    Value::Int(0),
);
/// The Wave set row's choices on `map` (`None`: none known), each with the
/// store value that names it: Endless first, then [`builtin::menu`]'s order
/// (the map's own sets, then the other built-ins).
pub fn wave_menu(map: Option<&str>) -> Vec<(i64, Def)> {
    let mut menu = builtin::menu(map);
    // Stable: only Endless moves.
    menu.sort_by_key(|(_, def)| !def.set.endless);
    menu
}

/// A Wave set row entry on a kit with `classes` (`None`: not known): the
/// text it shows, and why it cannot be played when the kit lacks a class
/// its waves or allies name ([`builtin::missing_classes`]).
pub fn wave_entry(def: &Def, classes: Option<&[String]>) -> (String, Option<String>) {
    let have: std::collections::BTreeSet<String> = classes.unwrap_or_default().iter().cloned().collect();
    let missing = match classes {
        Some(_) => builtin::missing_classes(def, &have),
        None => Vec::new(),
    };
    if missing.is_empty() {
        return (def.name.clone(), None);
    }
    let why = format!("the map's kit lacks {}", missing.join(", "));
    (format!("{} ({why})", def.name), Some(why))
}

/// A wave set's line of stats on its card: its rounds (an endless set
/// says so; a set for two teams says that instead), its waves and the
/// lives a team starts with.
pub fn wave_stats(def: &Def) -> String {
    let waves: usize = def.rounds.iter().map(|r| r.waves.len()).sum();
    let first = if def.set.endless {
        "Endless".to_owned()
    } else if def.teams > 1 {
        format!("{} teams", def.teams)
    } else {
        format!("{} rounds", def.rounds.len())
    };
    format!("{first}, {waves} waves, {} lives", def.lives.pool_start)
}

/// The Wave set row's choices on `map` as cards, in [`wave_menu`]'s
/// order: each set's name, the map's picture, its stats ([`wave_stats`])
/// and its help, which is the carousel's doc's for a card of that name
/// (`content/ui/screens/firefight.wavesets.yaml`), or why the set is not
/// offered on a kit with `classes` ([`wave_entry`]).
pub fn wave_cards(map: Option<&str>, classes: Option<&[String]>) -> Vec<crate::ui_kit::card::Card> {
    let doc = crate::ui_kit::data::screen(WAVE_SETS_SCREEN);
    let written = |name: &str| {
        doc.as_ref()
            .and_then(|d| d.cards.as_ref())
            .and_then(|c| c.items.values().find(|card| card.name == name))
            .and_then(|card| card.help.clone())
    };
    wave_menu(map)
        .into_iter()
        .map(|(value, def)| {
            let help = match wave_entry(&def, classes).1 {
                Some(why) => format!("Not offered: {why}."),
                None if value >= builtin::MAP_SET_BASE => "This map's own set.".to_owned(),
                None => written(&def.name).unwrap_or_else(|| {
                    format!("{} rounds of waves for {} team{}.", def.rounds.len(), def.teams, if def.teams == 1 { "" } else { "s" })
                }),
            };
            crate::ui_kit::card::Card {
                stats: wave_stats(&def),
                name: def.name,
                level: map.map(str::to_owned),
                help: Some(help),
                open: None,
            }
        })
        .collect()
}

/// The place in [`wave_menu`] of the set `store` holds on the lobby's map.
pub fn held_wave(store: &Store) -> usize {
    let held = match store.get(WAVE_SET.key) {
        Some(Value::Int(v)) => v,
        _ => 0,
    };
    wave_menu(lobby_start::kit_map().as_deref())
        .iter()
        .position(|(value, _)| *value == held)
        .unwrap_or(0)
}

/// Stores choice `i` of [`wave_menu`] on the lobby's map as the wave set;
/// what to log. A set the kit lacks a class for is refused, not stored.
pub fn set_wave(store: &Store, i: usize) -> String {
    let menu = wave_menu(lobby_start::kit_map().as_deref());
    let Some((value, def)) = menu.get(i) else {
        return format!("{}: no choice {i}", WAVE_SET.key);
    };
    if let (_, Some(why)) = wave_entry(def, lobby_start::kit_classes().as_deref()) {
        return format!("{} -> {}: not offered: {why}", WAVE_SET.key, def.name);
    }
    match store.set(WAVE_SET.key, Value::Int(*value)) {
        Ok(o) => match o.not_saved {
            None => format!("{} -> {}", WAVE_SET.key, def.name),
            Some(why) => format!("{} -> {}; not saved: {why}", WAVE_SET.key, def.name),
        },
        Err(e) => format!("{}: {e}", WAVE_SET.key),
    }
}

/// The Wave set row's carousel over `store`
/// (`content/ui/screens/firefight.wavesets.yaml`): a card for each set
/// the lobby's map offers, the held one focused as it opens; A stores the
/// focused card's set and closes, back to the Firefight page.
pub fn wave_sets_page(store: Arc<Store>) -> KitScreen {
    let (held, pick) = (Arc::clone(&store), store);
    crate::ui_kit::card_screen::screen(
        WAVE_SETS_FEATURE,
        WAVE_SETS_SCREEN,
        WAVE_SETS_IDS,
        || wave_cards(lobby_start::kit_map().as_deref(), lobby_start::kit_classes().as_deref()),
        move || held_wave(&held),
        move |i| {
            eprintln!("{WAVE_SETS_SCREEN}: {}", set_wave(&pick, i));
            crate::ui_kit::screen::Then::Close
        },
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

/// [`TEAMS`]'s first choice: the wave set's own team count.
pub const DEFAULT_TEAMS: i64 = 0;
pub static TEAMS: Setting = setting(
    "firefight.teams",
    "Teams",
    "How many player teams fight: one together against the waves, or two \
     that also fight each other; Default is the wave set's own.",
    Kind::Choice(&[
        choice(DEFAULT_TEAMS, "default", "Default"),
        choice(1, "one", "One team"),
        choice(2, "two", "Two teams"),
    ]),
    Value::Int(DEFAULT_TEAMS),
);
pub static DIFFICULTY: Setting = setting(
    "firefight.difficulty",
    "Difficulty",
    "How much a kill scores: harder scores more.",
    Kind::Choice(&[
        choice(0, "easy", "Easy"),
        choice(1, "normal", "Normal"),
        choice(2, "heroic", "Heroic"),
        choice(3, "legendary", "Legendary"),
    ]),
    Value::Int(1),
);
/// [`LIVES`]'s first choice: the wave set's own pool.
pub const DEFAULT_LIVES: i64 = 0;
pub static LIVES: Setting = setting(
    "firefight.lives",
    "Starting lives",
    "How many lives each team starts with; Default is the wave set's own.",
    Kind::Choice(&[
        choice(DEFAULT_LIVES, "default", "Default"),
        choice(1, "1", "1"),
        choice(3, "3", "3"),
        choice(5, "5", "5"),
        choice(8, "8", "8"),
        choice(12, "12", "12"),
        choice(20, "20", "20"),
        choice(30, "30", "30"),
    ]),
    Value::Int(DEFAULT_LIVES),
);
pub static SKULLS: Setting = setting(
    "firefight.skulls",
    "Skulls",
    "Whether each round switches on a skull.",
    Kind::Bool,
    Value::Bool(true),
);

pub static PODS: Setting = setting(
    "firefight.pods",
    "Drop pods",
    "Whether players fall to the ground in drop pods when they spawn.",
    Kind::Bool,
    Value::Bool(true),
);

/// Every Firefight setting, in the page's order.
pub static DEFS: [&Setting; 7] =
    [&ENABLED, &WAVE_SET, &TEAMS, &DIFFICULTY, &LIVES, &SKULLS, &PODS];

/// The Firefight page over `store`: the shared settings dialog
/// (`crate::ui_kit::settings`) its doc declares,
/// `content/ui/screens/firefight.yaml`: its header, its seven rows over
/// [`DEFS`], their help and picker copy, and the Game Setup row's text
/// and help (`settings.caller`). The Wave set row offers the lobby map's
/// own sets first ([`builtin::menu`]), so it is bound, not read from the
/// setting's fixed choices; A on it pushes its carousel of cards
/// ([`wave_sets_page`]), and left and right step it in place.
pub fn page(store: Arc<Store>) -> KitScreen {
    let binds = Bindings::new().menu(
        "waveset",
        || {
            let classes = lobby_start::kit_classes();
            wave_menu(lobby_start::kit_map().as_deref())
                .into_iter()
                .map(|(_, def)| wave_entry(&def, classes.as_deref()).0)
                .collect()
        },
        {
            let store = Arc::clone(&store);
            move || held_wave(&store)
        },
        {
            let store = Arc::clone(&store);
            move |i: usize| set_wave(&store, i)
        },
    );
    crate::ui_kit::settings::screen(FEATURE, SCREEN, IDS, Some(store), binds)
        .unwrap_or_else(|e| panic!("{e}"))
}

// --- What the match starts from ---------------------------------------------

/// The page's choices, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Firefight runs on a kit map.
    pub enabled: bool,
    /// The map the choices play on (`None`: none known): a map-specific
    /// set plays only on its own map ([`builtin::resolve`]).
    pub map: Option<String>,
    /// The Wave set row's value ([`builtin::resolve`]: a built-in by its
    /// position, a map-specific set from [`builtin::MAP_SET_BASE`]).
    pub wave_set: usize,
    /// The set Map Waves plays: the running map variant's own, which
    /// [`Options::wave_set`] then does not choose (`runtime::record_options`).
    pub waves: Option<Def>,
    /// The player teams, or the wave set's own.
    pub teams: Option<u8>,
    pub difficulty: Difficulty,
    /// The starting pool, or the wave set's own.
    pub lives: Option<u16>,
    /// Each round switches on its skull.
    pub skulls: bool,
    /// Players fall to their spawns in drop pods, where the kit has them
    /// (`super::pods`).
    pub pods: bool,
}

impl Default for Options {
    /// The settings' defaults: on, no map, Holdout, its own teams, Normal,
    /// its own lives, skulls.
    fn default() -> Self {
        Options {
            enabled: true,
            map: None,
            wave_set: 0,
            waves: None,
            teams: None,
            difficulty: Difficulty::Normal,
            lives: None,
            skulls: true,
            pods: true,
        }
    }
}

impl Options {
    /// The store's choices. A setting the store does not hold, or holds in
    /// a form it cannot be, is an error naming it.
    pub fn read(store: &Store) -> Result<Options, String> {
        Self::for_map(store, None)
    }

    /// The store's choices for `map`, whose own sets the Wave set value
    /// may name ([`builtin::menu`]).
    pub fn for_map(store: &Store, map: Option<&str>) -> Result<Options, String> {
        let int = |def: &Setting| -> Result<i64, String> {
            match store.get(def.key) {
                Some(Value::Int(v)) => Ok(v),
                other => Err(format!("{} is {other:?}, not a choice", def.key)),
            }
        };
        let enabled = match store.get(ENABLED.key) {
            Some(Value::Bool(v)) => v,
            other => return Err(format!("{} is {other:?}, not on or off", ENABLED.key)),
        };
        let skulls = match store.get(SKULLS.key) {
            Some(Value::Bool(v)) => v,
            other => return Err(format!("{} is {other:?}, not on or off", SKULLS.key)),
        };
        let pods = match store.get(PODS.key) {
            Some(Value::Bool(v)) => v,
            other => return Err(format!("{} is {other:?}, not on or off", PODS.key)),
        };
        let wave_set = usize::try_from(int(&WAVE_SET)?)
            .ok()
            .filter(|&v| builtin::names_set(v as i64))
            .ok_or_else(|| format!("{} names no wave set", WAVE_SET.key))?;
        let difficulty = match int(&DIFFICULTY)? {
            0 => Difficulty::Easy,
            1 => Difficulty::Normal,
            2 => Difficulty::Heroic,
            3 => Difficulty::Legendary,
            v => return Err(format!("{} is {v}, not a difficulty", DIFFICULTY.key)),
        };
        let teams = match int(&TEAMS)? {
            DEFAULT_TEAMS => None,
            n @ (1 | 2) => Some(n as u8),
            n => return Err(format!("{} is {n}, not one team or two", TEAMS.key)),
        };
        let lives = match int(&LIVES)? {
            DEFAULT_LIVES => None,
            n => Some(
                u16::try_from(n)
                    .map_err(|_| format!("{} is {n}, not a pool of lives", LIVES.key))?,
            ),
        };
        Ok(Options {
            enabled,
            map: map.map(str::to_owned),
            wave_set,
            waves: None,
            teams,
            difficulty,
            lives,
            skulls,
            pods,
        })
    }

    /// The set before the page's teams, lives and skulls: Map Waves' own
    /// ([`Options::waves`]), else the one the value names on
    /// [`Options::map`] ([`builtin::resolve`]).
    fn base_set(&self) -> Option<Def> {
        self.waves.clone().or_else(|| {
            i64::try_from(self.wave_set)
                .ok()
                .and_then(|value| builtin::resolve(value, self.map.as_deref()))
        })
    }

    /// The definition the match plays: the set [`Options::base_set`]
    /// names, played by the teams the
    /// page chose (`builtin::with_teams`), with the starting pool it chose
    /// and, with skulls off, no skull on any round and none a set brings.
    pub fn definition(&self) -> Result<Def, String> {
        let mut def = self
            .base_set()
            .ok_or_else(|| format!("no wave set {}", self.wave_set))?;
        if let Some(teams) = self.teams {
            def = builtin::with_teams(def, teams);
        }
        if let Some(n) = self.lives {
            def.lives.pool_start = n;
        }
        if !self.skulls {
            for round in &mut def.rounds {
                round.skulls_on.clear();
            }
            def.set.skulls.clear();
        }
        Ok(def)
    }

    /// The player teams the match has: the page's choice, else the wave
    /// set's own (1 for a wave set the build does not have). An endless
    /// set is one team whatever the page chose ([`builtin::with_teams`]).
    pub fn player_teams(&self) -> u8 {
        match self.base_set() {
            Some(def) if def.set.endless => def.teams,
            def => self.teams.unwrap_or_else(|| def.map_or(1, |def| def.teams)),
        }
    }

    /// The Wave set row's label: the set the value names on
    /// [`Options::map`], or the built-in a map set falls back to.
    pub fn wave_set_label(&self) -> String {
        self.base_set()
            .map(|def| def.name)
            .unwrap_or_else(|| format!("wave set {}", self.wave_set))
    }
}

/// Where the runtime reads the page's choices when a game starts.
pub type Source = dyn Fn() -> Result<Options, String> + Send + Sync;

/// The runtime's source over `store`.
pub fn source(store: &Arc<Store>) -> Box<Source> {
    let store = Arc::clone(store);
    Box::new(move || Options::read(&store))
}

// --- The Game Setup row -----------------------------------------------------

/// Whether a Game Setup list with these actions gets our row: a custom
/// game's (it has Change Map), once.
pub fn wants_row(actions: &[i16]) -> bool {
    actions.contains(&CHANGE_MAP) && !actions.contains(&ROW_ACTION)
}

/// Adds a row with [`ROW_ACTION`] to the list at `list`, under Quick
/// Options (and the rows before it) and above the "Switch To:" rows; the
/// new count.
pub fn add_row(m: &mut dyn GuestMemory, list: u32) -> Result<usize, String> {
    h1row::insert_row(m, list, ROW_ACTION)
}

/// The row on the Game Setup dialog, and what its press opens.
pub struct SetupRow {
    pub page: Arc<KitScreen>,
}

fn row_text(c: &mut Call, screen: &KitScreen, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let rt = Arc::clone(c.ctx.runtime());
    let (item, id) = {
        let mut mem = rt.memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        let action = item
            .and_then(|i| mem.u32(i + 0x70).ok())
            .and_then(|row| h1row::row_action(&*mem, c.cpu.ecx, row));
        let Some(item) = item.filter(|_| action == Some(ROW_ACTION)) else {
            return Ok(Flow::Continue);
        };
        // The row's own text ("Firefight Options..."), from its slot after
        // the page's footer, written before it is drawn.
        let id = screen.row_label_id();
        if let Err(e) = screen.write_texts(&mut *mem) {
            eprintln!("firefight: the row's text: {e:?}");
        }
        (item, id)
    };
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("firefight: Game Setup row text");
    }
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, id)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call, request_at: u32) -> Result<Flow, GuestFault> {
    let (action, player, stack) = {
        let mem = c.ctx.runtime().memory();
        let row = mem.u32(c.cpu.esp + 8).ok().and_then(|p| mem.u32(p).ok());
        let action = row.and_then(|row| h1row::row_action(&*mem, c.cpu.ecx, row));
        let player = crate::skulls_menu::player_index(&*mem, c.cpu.esp);
        let stack = h1row::screen_of(&*mem, c.cpu.ecx).and_then(|s| {
            Some((
                mem.u32(s + crate::ui_kit::lifecycle::SCREEN_WINDOW).ok()?,
                mem.u32(s + crate::ui_kit::lifecycle::SCREEN_SLOT).ok()?,
            ))
        });
        (action, player, stack)
    };
    if action != Some(ROW_ACTION) {
        return Ok(Flow::Continue);
    }
    let (Some(player), Some((window, slot)), Some(loader)) =
        (player, stack, list_class::loader_of(SCREEN))
    else {
        eprintln!("firefight: page not opened (player {player:?}, stack {stack:?})");
        return c.ret(8).map(Flow::Return);
    };
    eprintln!(
        "firefight: opening the Firefight page (player {player}) on window {window}, slot {slot}"
    );
    let request = crate::skulls_mp::open_request(1 << player, window, slot, loader);
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl hooks::Feature for SetupRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ROW_FEATURE,
            "The Firefight row on a custom game's Game Setup",
        )
        .doc("docs/game/firefight.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use guest_layout::points::{
            SQUAD_SETTINGS_LIST_CTOR as CTOR, SQUAD_SETTINGS_ROW_CHOSEN, SQUAD_SETTINGS_ROW_TEXT,
        };
        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        let building: Arc<std::sync::Mutex<Vec<u32>>> = Arc::default();
        let (noted, live) = (Arc::clone(&building), Arc::clone(&building));
        b.raw()
            .before(&CTOR, "note the Game Setup list", move |c| {
                noted
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(c.cpu.esi);
                Ok(Flow::Continue)
            })
            .after_feature(crate::clankers::ROW_FEATURE)
            .after_feature(crate::clankermatch::SWITCH_FEATURE)
            .after_feature(h1row::ROW_FEATURE);
        b.raw()
            .after(&CTOR, "add the Firefight row", move |c| {
                let Some(list) = building.lock().unwrap_or_else(|e| e.into_inner()).pop() else {
                    return Ok(());
                };
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let actions = h1row::setup_actions(&*mem, list).unwrap_or_default();
                if !wants_row(&actions) {
                    return Ok(());
                }
                match add_row(&mut *mem, list) {
                    Ok(n) => {
                        eprintln!("firefight: Game Setup list: Firefight row added ({n} rows)")
                    }
                    Err(why) => eprintln!("firefight: Game Setup list: row not added: {why}"),
                }
                Ok(())
            })
            .after_feature(crate::clankers::ROW_FEATURE)
            .after_feature(crate::clankermatch::SWITCH_FEATURE)
            .after_feature(h1row::ROW_FEATURE);
        let (screen, seen) = (Arc::clone(&self.page), Arc::new(AtomicBool::new(false)));
        b.raw()
            .before(
                &SQUAD_SETTINGS_ROW_TEXT,
                "the Firefight row's text",
                move |c| row_text(c, &screen, &seen),
            )
            .after_feature(crate::clankers::ROW_FEATURE)
            .after_feature(crate::clankermatch::SWITCH_FEATURE)
            .after_feature(h1row::ROW_FEATURE);
        b.raw()
            .before(
                &SQUAD_SETTINGS_ROW_CHOSEN,
                "the Firefight row's press",
                move |c| row_chosen(c, request_at),
            )
            .after_feature(crate::clankers::ROW_FEATURE)
            .after_feature(crate::clankermatch::SWITCH_FEATURE)
            .after_feature(h1row::ROW_FEATURE);
        // The page's state is its kit screen's (`ui-kit-texts.*`), the
        // choices the settings store's.
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            h1row::building_live(&live)
        }));
        Ok(())
    }
}

/// Adds the page, its texts and the Game Setup row.
pub fn install(registry: &mut hooks::Registry, store: &Arc<Store>) -> anyhow::Result<()> {
    let screen = Arc::new(page(Arc::clone(store)));
    registry
        .add(&KitTexts(Arc::clone(&screen)))
        .map_err(|e| anyhow::anyhow!("firefight texts: {e}"))?;
    registry
        .add(&screen.class())
        .map_err(|e| anyhow::anyhow!("firefight page: {e}"))?;
    // The Wave set row's carousel.
    let sets = Arc::new(wave_sets_page(Arc::clone(store)));
    registry
        .add(&KitTexts(Arc::clone(&sets)))
        .map_err(|e| anyhow::anyhow!("firefight wave set texts: {e}"))?;
    registry
        .add(&crate::ui_kit::card_screen::class(&sets))
        .map_err(|e| anyhow::anyhow!("firefight wave sets: {e}"))?;
    // The row's picture and description on Game Setup: the skull and crown
    // over crossed flags, and what the page is for.
    let help = Arc::clone(&screen);
    crate::game_setup_help::register(
        ROW_ACTION,
        crate::game_setup_help::image::CUSTOM_GAME,
        Arc::new(move |m: &mut dyn GuestMemory| {
            let id = help.row_help_id()?;
            help.write_texts(m).ok()?;
            Some(id)
        }),
    );
    registry
        .add(&SetupRow { page: screen })
        .map_err(|e| anyhow::anyhow!("firefight row: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
