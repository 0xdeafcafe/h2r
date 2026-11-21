//! Firefight in Game Setup (docs/game/firefight.md, "What the player
//! gets"): the settings, the options page, and the row on a custom game's
//! Game Setup that opens it.
//!
//! **The settings** (`firefight.*`) are kept in the profile store like any
//! other ([`DEFS`], in [`crate::setting_defs::ALL`]). [`Options::read`]
//! reads them and the runtime starts the match from them
//! ([`Options::definition`], [`Options::difficulty`]): on or off, which
//! wave set, how hard, how many lives to start with, and whether each
//! round switches on a skull.
//!
//! **The page** is a kit screen ([`page`]) of five choice rows on the
//! lobby's own settings dialog, Quick Options, as the Clankers page is
//! (`crate::clankers`): a choice opens the value picker on A, and left and
//! right step through the choices as a shortcut. No row is a "< All >"
//! arrow row.
//!
//! **The row** is "Firefight Options..." on a custom game's Game Setup, the
//! way "Clanker Options..." is ([`crate::clankers::SetupRow`], whose
//! helpers this uses): an `after` on the list's constructor adds a row with
//! [`ROW_ACTION`] to every list that has Change Map, `before` hooks draw its
//! text and take its press, which opens the page on the dialog's own
//! stack. Its hooks sit after the Clankers row's and Clankermatch's, so the
//! plan orders them.
//!
//! **Not here.** The choices are the host's. A client of a distributed
//! session learns them from the session data (FF9), not from its own page.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ::firefight::{Def, Difficulty, builtin};
use engine::entry::{EntryFuture, GuestCall, Ret};
use engine::memory::GuestMemory;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use settings::{Apply, Choice, Def as Setting, Kind, Scope, Store, Tier, Value};

use crate::h1_options_row::{self as h1row, CHANGE_MAP};
use crate::list_class::{self, Rows};
use crate::ui_kit::choice::ChoiceRow;
use crate::ui_kit::dialog::{DialogBase, icon};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::{Glyph, Prompt};
use crate::ui_kit::screen::{KitScreen, KitTexts};
use crate::ui_kit::view::Opener;

/// The page's feature (`features.firefight-options`) and screen name.
pub const FEATURE: &str = "firefight-options";
pub const SCREEN: &str = "firefight";
/// The Game Setup row's feature (`features.firefight-row`).
pub const ROW_FEATURE: &str = "firefight-row";
/// The page's string ids.
pub const IDS: Family = ids::family("firefight");
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

/// [`WAVE_SET`]'s values are the built-in definitions' positions
/// ([`builtin::definitions`]), in menu order.
pub static ENABLED: Setting = setting(
    "firefight.enabled",
    "Firefight",
    "Whether a game on a Firefight map plays Firefight: waves of enemies, \
     shared lives and a score.",
    Kind::Bool,
    Value::Bool(true),
);
pub static WAVE_SET: Setting = setting(
    "firefight.waveset",
    "Wave set",
    "Which set of waves is fought.",
    Kind::Choice(&[
        choice(0, "holdout", "Holdout"),
        choice(1, "attrition", "Attrition"),
        choice(2, "last_stand", "Last Stand"),
    ]),
    Value::Int(0),
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
    "How many lives the team starts with; Default is the wave set's own.",
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

/// Every Firefight setting, in the page's order.
pub static DEFS: [&Setting; 5] = [&ENABLED, &WAVE_SET, &DIFFICULTY, &LIVES, &SKULLS];

static ROWS: [ChoiceRow; 5] = [
    ChoiceRow::new(
        &ENABLED,
        "Firefight",
        "Fight waves of enemies on a Firefight map.",
    )
    .explain(
        "On starts Firefight when the map carries its kit: waves of enemies, \
             shared lives and a score. Off plays the game's own rules.",
    )
    .icon(icon::SPARTAN),
    ChoiceRow::new(&WAVE_SET, "Wave set", "Which set of waves you fight.")
        .explain(
            "Holdout is steady, with lives to spare. Attrition sends more enemies \
             and gives fewer lives. Last Stand sends the most and gives the fewest.",
        )
        .icon(icon::BANNER),
    ChoiceRow::new(
        &DIFFICULTY,
        "Difficulty",
        "How much a kill scores, by difficulty.",
    )
    .explain(
        "Harder difficulties score more for each kill: Easy 75%, Normal 100%, \
         Heroic 125% and Legendary 150%.",
    )
    .icon(icon::SKULL),
    ChoiceRow::new(
        &LIVES,
        "Starting lives",
        "How many lives the team starts with.",
    )
    .explain(
        "The shared pool of lives. Each death takes one, and each round adds a \
         few more. Default uses the wave set's own pool.",
    )
    .icon(icon::HELMET),
    ChoiceRow::new(&SKULLS, "Skulls", "Whether each round switches on a skull.")
        .explain(
            "Each round switches on another skull, which makes the fight harder \
             and scores a little more. Off keeps every round plain.",
        )
        .icon(icon::LIGHTNING),
];

/// The Game Setup row's text; the page's title stays [`TITLE`].
pub const ROW_LABEL: &str = "Firefight Options...";
/// What the Game Setup row does, in Game Setup's description while it has
/// the focus, as retail's rows have theirs (`crate::game_setup_help`).
pub const ROW_HELP: &str =
    "Fight waves of enemies on a Firefight map: the waves, the difficulty and the lives.";
/// The page's title, in capitals as Quick Options' header ("QUICK
/// OPTIONS") is.
pub const TITLE: &str = "FIREFIGHT";

/// The Firefight page over `store`: a dialog of its rows on Quick Options.
pub fn page(store: Arc<Store>) -> KitScreen {
    let mut s = KitScreen::new(FEATURE, SCREEN, IDS, TITLE)
        .row_label(ROW_LABEL)
        .row_help(ROW_HELP)
        .dialog(DialogBase::QuickOptions, Opener::GAME_SETUP_FIREFIGHT)
        .header(icon::SPARTAN);
    for r in &ROWS {
        s = s.row(r.row(Arc::clone(&store)));
    }
    s.prompts(&[
        Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
        Prompt::new(&[Glyph::A], "Select"),
        Prompt::new(&[Glyph::B], "Back"),
    ])
}

// --- What the match starts from ---------------------------------------------

/// The page's choices, read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Firefight runs on a kit map.
    pub enabled: bool,
    /// The position in [`builtin::definitions`].
    pub wave_set: usize,
    pub difficulty: Difficulty,
    /// The starting pool, or the wave set's own.
    pub lives: Option<u16>,
    /// Each round switches on its skull.
    pub skulls: bool,
}

impl Default for Options {
    /// The settings' defaults: on, Holdout, Normal, its own lives, skulls.
    fn default() -> Self {
        Options {
            enabled: true,
            wave_set: 0,
            difficulty: Difficulty::Normal,
            lives: None,
            skulls: true,
        }
    }
}

impl Options {
    /// The store's choices. A setting the store does not hold, or holds in
    /// a form it cannot be, is an error naming it.
    pub fn read(store: &Store) -> Result<Options, String> {
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
        let wave_set = usize::try_from(int(&WAVE_SET)?)
            .ok()
            .filter(|&i| i < builtin::definitions().len())
            .ok_or_else(|| format!("{} names no built-in wave set", WAVE_SET.key))?;
        let difficulty = match int(&DIFFICULTY)? {
            0 => Difficulty::Easy,
            1 => Difficulty::Normal,
            2 => Difficulty::Heroic,
            3 => Difficulty::Legendary,
            v => return Err(format!("{} is {v}, not a difficulty", DIFFICULTY.key)),
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
            wave_set,
            difficulty,
            lives,
            skulls,
        })
    }

    /// The definition the match plays: the wave set, with the starting
    /// pool the page chose and, with skulls off, no skull on any round.
    pub fn definition(&self) -> Result<Def, String> {
        let mut def = builtin::definitions()
            .get(self.wave_set)
            .cloned()
            .ok_or_else(|| format!("no built-in wave set {}", self.wave_set))?;
        if let Some(n) = self.lives {
            def.lives.pool_start = n;
        }
        if !self.skulls {
            for round in &mut def.rounds {
                round.skulls_on.clear();
            }
        }
        Ok(def)
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
        use engine::points::{
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
