//! Clankers in custom games (docs/game/bots.md, section 7 and stage 7): the
//! settings, the Clankers page, its row on a custom game's Game Setup, and
//! the driver that keeps that many local bots.
//!
//! **The settings** (`clankers.*`) are kept in the profile store like any
//! other ([`DEFS`], in [`crate::setting_defs::ALL`]), so they persist and
//! `h2 settings` lists them.
//!
//! **The page** is a kit screen ([`page`]) of seven choice rows
//! ([`crate::ui_kit::choice::ChoiceRow`]), laid out as the Halo 1 Options
//! page is.
//!
//! **The row.** A custom game's Game Setup dialog gets a "Clankers" row,
//! the way it gets the Halo 1 Options row (`crate::h1_options_row`, whose
//! helpers this uses): an `after` on the list's constructor adds a row with
//! [`ROW_ACTION`] to every list that has Change Map; `before` hooks draw
//! its text and take its press, which opens the page on the dialog's own
//! stack. Every hook is ordered after the Halo 1 row's, and each passes any
//! other action on.
//!
//! **The driver** ([`Driver`]) keeps `min(count, free ports)` brains in
//! [`bots::local::global`] while `clankers.enabled` is on, on ports 1 to 3
//! that no other bot asked for, and takes its own off when it goes off.
//! They join on the players screen like `H2_BOTS` bots. A difficulty
//! change reaches the bots it already has at their next frame; a count
//! change adds or takes off bots at once.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use engine::entry::{EntryFuture, GuestCall, Ret};
use engine::memory::GuestMemory;
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use settings::{Apply, Choice, Def, Kind, Scope, Store, Tier, Unit, Value};

use bots::local::{self, BotHandle, Local};
use bots::mind::Skill;
use bots::phantom::{Identity, PhantomHandle, Phantoms};

use crate::h1_options_row::{self as h1row, CHANGE_MAP};
use crate::list_class::{self, Rows};
use crate::ui_kit::choice::ChoiceRow;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::{Glyph, Prompt};
use crate::ui_kit::screen::{KitScreen, KitTexts};
use crate::ui_kit::spec::ValueStyle;

/// The page's feature (`features.clankers`) and screen name.
pub const FEATURE: &str = "clankers";
pub const SCREEN: &str = "clankers";
/// The Game Setup row's feature (`features.clankers-row`).
pub const ROW_FEATURE: &str = "clankers-row";
/// The page's string ids.
pub const IDS: Family = ids::family("clankers");
/// Our Game Setup row's action: after the Halo 1 Options row's 10 (retail
/// uses 0 to 9; the press never reaches the original's jump table).
pub const ROW_ACTION: i16 = 11;

const fn def(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: Kind,
    default: Value,
) -> Def {
    Def {
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
        // On the Clankers page of a custom game's Game Setup, not in
        // Settings: chosen per game.
        menu: None,
    }
}

const fn choice(value: i64, name: &'static str, label: &'static str) -> Choice {
    Choice { value, name, label }
}

pub static ENABLED: Def = def(
    "clankers.enabled",
    "Clankers",
    "Bots join custom games: up to 3 on this machine's free controller ports \
     (docs/game/bots.md).",
    Kind::Bool,
    Value::Bool(false),
);
pub static COUNT: Def = def(
    "clankers.count",
    "Count",
    "How many bots join. Up to 3 play for now, one per free controller port.",
    Kind::Int {
        valid: 1..=15,
        steps: &[],
        unit: Unit::Plain,
    },
    Value::Int(1),
);
/// [`DIFFICULTY`]'s values: the four skills, then Mixed.
pub const MIXED: i64 = 4;
pub static DIFFICULTY: Def = def(
    "clankers.difficulty",
    "Difficulty",
    "How well the bots play. Mixed gives each a difficulty around Normal.",
    Kind::Choice(&[
        choice(0, "easy", "Easy"),
        choice(1, "normal", "Normal"),
        choice(2, "heroic", "Heroic"),
        choice(3, "legendary", "Legendary"),
        choice(MIXED, "mixed", "Mixed"),
    ]),
    Value::Int(1),
);
pub static TEAM: Def = def(
    "clankers.team",
    "Teams",
    "Which team the bots join in a team game: balanced against the players, \
     or all on one.",
    Kind::Choice(&[
        choice(0, "auto", "Auto-balance"),
        choice(1, "red", "Red"),
        choice(2, "blue", "Blue"),
    ]),
    Value::Int(0),
);
pub static AGGRESSION: Def = def(
    "clankers.aggression",
    "Aggression",
    "How eagerly the bots push into a fight.",
    Kind::Choice(&[
        choice(0, "calm", "Calm"),
        choice(1, "normal", "Normal"),
        choice(2, "wild", "Wild"),
    ]),
    Value::Int(1),
);
pub static AIM_STYLE: Def = def(
    "clankers.aim_style",
    "Aim style",
    "Steady tracks smoothly; Twitchy flicks onto targets.",
    Kind::Choice(&[
        choice(0, "steady", "Steady"),
        choice(1, "twitchy", "Twitchy"),
    ]),
    Value::Int(0),
);
pub static TAUNTS: Def = def(
    "clankers.taunts",
    "Taunts",
    "Whether bots taunt after a kill.",
    Kind::Bool,
    Value::Bool(true),
);

/// Every Clankers setting, in the page's order.
pub static DEFS: [&Def; 7] = [
    &ENABLED,
    &COUNT,
    &DIFFICULTY,
    &TEAM,
    &AGGRESSION,
    &AIM_STYLE,
    &TAUNTS,
];

static ROWS: [ChoiceRow; 7] = [
    ChoiceRow::new(
        &ENABLED,
        "Clankers",
        "Bots join this game, on the free controller ports.",
    ),
    ChoiceRow::new(&COUNT, "Count", "How many bots join. Up to 3 play for now."),
    ChoiceRow::new(
        &DIFFICULTY,
        "Difficulty",
        "How well they play. Mixed gives each a difficulty around Normal.",
    ),
    ChoiceRow::new(&TEAM, "Teams", "Which team they join in a team game."),
    ChoiceRow::new(
        &AGGRESSION,
        "Aggression",
        "How eagerly they push into a fight.",
    ),
    ChoiceRow::new(
        &AIM_STYLE,
        "Aim style",
        "Steady tracks smoothly; Twitchy flicks.",
    ),
    ChoiceRow::new(&TAUNTS, "Taunts", "Whether they taunt after a kill."),
];

/// The Clankers page over `store`.
pub fn page(store: Arc<Store>) -> KitScreen {
    let mut s = KitScreen::new(FEATURE, SCREEN, IDS, "Clankers")
        .layout(crate::skulls_menu::LIST_LAYOUT, ValueStyle::Inline)
        .description(crate::skulls_menu::DESCRIPTION_TEXT)
        // Seven rows in a list that shows five.
        .scrolls();
    for r in &ROWS {
        s = s.row(r.row(Arc::clone(&store)));
    }
    s.prompts(&[
        Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
        Prompt::new(&[Glyph::B], "Back"),
    ])
}

// --- The driver -----------------------------------------------------------

/// The skill of a bot on `port` at difficulty `d` (a [`DIFFICULTY`] value):
/// Mixed is Normal, Heroic and Easy on ports 1, 2 and 3.
pub fn skill(d: i64, port: u8) -> Skill {
    let d = if d == MIXED {
        [1, 2, 0][usize::from(port.saturating_sub(1)) % 3]
    } else {
        d
    };
    match d {
        0 => Skill::Easy,
        2 => Skill::Heroic,
        3 => Skill::Legendary,
        _ => Skill::Normal,
    }
}

fn skill_index(s: Skill) -> u8 {
    Skill::ALL.iter().position(|k| *k == s).unwrap_or(1) as u8
}

/// A Clanker: the brain, its skill shared with the driver so a change on
/// the page reaches it at its next frame.
struct Clanker {
    brain: bots::Brain,
    skill: Arc<AtomicU8>,
    /// The page's Taunts (shared by every Clanker of the driver).
    taunts: Arc<AtomicBool>,
}

impl local::PadSource for Clanker {
    fn pad(&mut self, f: &local::Frame<'_>) -> input::host::HostPadState {
        let want = Skill::ALL[usize::from(self.skill.load(Ordering::Relaxed)).min(3)];
        if self.brain.mind.skill != want {
            self.brain.mind.skill = want;
        }
        self.brain.mind.emote.taunt = if self.taunts.load(Ordering::Relaxed) {
            bots::emote::DEFAULT_TAUNT
        } else {
            0.0
        };
        self.brain.pad(f)
    }
}

/// What the page asks for: how many bots (0 when off) at which
/// difficulty, and whether they taunt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    pub count: usize,
    pub difficulty: i64,
    pub taunts: bool,
}

impl Want {
    pub fn read(store: &Store) -> Want {
        let on = matches!(store.get(ENABLED.key), Some(Value::Bool(true)));
        let count = match store.get(COUNT.key) {
            Some(Value::Int(n)) => n.clamp(0, 15) as usize,
            _ => 1,
        };
        let difficulty = match store.get(DIFFICULTY.key) {
            Some(Value::Int(d)) => d,
            _ => 1,
        };
        let taunts = !matches!(store.get(TAUNTS.key), Some(Value::Bool(false)));
        Want {
            count: if on { count } else { 0 },
            difficulty,
            taunts,
        }
    }
}

/// Where one of the page's bots is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    /// A local bot on this controller port.
    Port(u8, BotHandle),
    /// A phantom peer, the `n`th of the page's bots (from 1).
    Phantom(u32, PhantomHandle),
}

/// Keeps the page's bots: local bots on free ports first, then, with
/// phantoms, phantom peers for the rest (docs/game/bots.md, stage 6).
pub struct Driver {
    local: Local,
    phantoms: Option<Phantoms>,
    own: Mutex<Vec<(Seat, Arc<AtomicU8>)>>,
    taunts: Arc<AtomicBool>,
}

impl Driver {
    /// Local bots only.
    pub fn new(local: Local) -> Self {
        Driver {
            local,
            phantoms: None,
            own: Mutex::new(Vec::new()),
            taunts: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Local bots, then phantoms.
    pub fn with_phantoms(local: Local, phantoms: Phantoms) -> Self {
        Driver {
            phantoms: Some(phantoms),
            ..Driver::new(local)
        }
    }

    /// Adds or takes off bots to match `want`; what it did, for the log.
    pub fn apply(&self, want: Want) -> Vec<String> {
        let mut own = self.own.lock().unwrap_or_else(|e| e.into_inner());
        let mut log = Vec::new();
        self.taunts.store(want.taunts, Ordering::Relaxed);
        // Fewer: the last added go first.
        while own.len() > want.count {
            let (seat, _) = own.pop().expect("more than wanted");
            let (name, r) = match seat {
                Seat::Port(port, h) => (
                    format!("Clanker {port}"),
                    self.local.remove(h).map_err(|e| e.to_string()),
                ),
                Seat::Phantom(n, h) => (
                    phantom_name(n),
                    self.phantoms
                        .as_ref()
                        .map_or(Ok(()), |p| p.remove(h).map_err(|e| e.to_string())),
                ),
            };
            match r {
                Ok(()) => log.push(format!("{name} off")),
                Err(e) => log.push(format!("{name}: {e}")),
            }
        }
        for (seat, s) in own.iter() {
            s.store(
                skill_index(skill(want.difficulty, seat_port(*seat))),
                Ordering::Relaxed,
            );
        }
        // More: on the lowest port no bot asked for, then as phantoms.
        while own.len() < want.count {
            let taken: Vec<u8> = self.local.list().iter().map(|b| b.wanted).collect();
            let n = own.len() as u32 + 1;
            let free = (1..=3u8).find(|p| !taken.contains(p));
            let port = free.unwrap_or(((n - 1) % 3 + 1) as u8);
            let k = skill(want.difficulty, port);
            let s = Arc::new(AtomicU8::new(skill_index(k)));
            let src = Clanker {
                brain: bots::Brain::new(k, 0xc1a4 ^ u64::from(n)),
                skill: Arc::clone(&s),
                taunts: Arc::clone(&self.taunts),
            };
            let added = match (free, &self.phantoms) {
                (Some(port), _) => self
                    .local
                    .add(port, &format!("Clanker {port}"), Box::new(src))
                    .map(|h| (format!("Clanker {port}"), Seat::Port(port, h)))
                    .map_err(|e| e.to_string()),
                (None, Some(ph)) => {
                    let id = Identity {
                        xuid: live_proto::bots::local_xuid(n).0,
                        gamertag: phantom_name(n),
                        ..Default::default()
                    };
                    ph.add(id, None, Box::new(src))
                        .map(|h| {
                            (
                                format!("{} (phantom)", phantom_name(n)),
                                Seat::Phantom(n, h),
                            )
                        })
                        .map_err(|e| e.to_string())
                }
                (None, None) => {
                    log.push(format!(
                        "{} asked for, {} on this machine: no free port",
                        want.count,
                        own.len()
                    ));
                    break;
                }
            };
            match added {
                Ok((name, seat)) => {
                    log.push(format!("{name} on ({k:?})"));
                    own.push((seat, s));
                }
                Err(e) => {
                    log.push(format!("Clanker {n}: {e}"));
                    break;
                }
            }
        }
        log
    }

    /// The ports of the local bots it keeps.
    pub fn ports(&self) -> Vec<u8> {
        self.seats()
            .into_iter()
            .filter_map(|s| match s {
                Seat::Port(p, _) => Some(p),
                Seat::Phantom(..) => None,
            })
            .collect()
    }

    /// Every bot it keeps, in the order added.
    pub fn seats(&self) -> Vec<Seat> {
        self.own
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|b| b.0)
            .collect()
    }
}

fn phantom_name(n: u32) -> String {
    format!("Clanker {n}")
}

/// The port a seat's Mixed skill follows: its own, or for the `n`th bot
/// as a phantom, 1 to 3 in turn.
fn seat_port(s: Seat) -> u8 {
    match s {
        Seat::Port(p, _) => p,
        Seat::Phantom(n, _) => ((n - 1) % 3 + 1) as u8,
    }
}

/// Starts `driver` over `store`: matches it now and on every change.
pub fn drive(store: &Arc<Store>, driver: Driver) -> Arc<Driver> {
    let d = Arc::new(driver);
    let run = {
        let (d, store) = (Arc::clone(&d), Arc::downgrade(store));
        move |_: &Value| {
            if let Some(store) = store.upgrade() {
                for line in d.apply(Want::read(&store)) {
                    eprintln!("clankers: {line}");
                }
            }
        }
    };
    for key in [ENABLED.key, COUNT.key, DIFFICULTY.key, TAUNTS.key] {
        store.on_change(key, run.clone());
    }
    for line in d.apply(Want::read(store)) {
        eprintln!("clankers: {line}");
    }
    d
}

// --- The Game Setup row ---------------------------------------------------

/// Whether a Game Setup list with these actions gets our row: a custom
/// game's (it has Change Map), once.
pub fn wants_row(actions: &[i16]) -> bool {
    actions.contains(&CHANGE_MAP) && !actions.contains(&ROW_ACTION)
}

/// Adds a row with [`ROW_ACTION`] to the list at `list`; the new count.
pub fn add_row(m: &mut dyn GuestMemory, list: u32) -> Result<usize, String> {
    let e = |e: engine::memory::MemFault| format!("{e:?}");
    let actions = h1row::setup_actions(m, list).map_err(e)?;
    let array =
        engine::datum::DatumArray::at(m.u32(list.wrapping_add(h1row::SETUP_ROWS)).map_err(e)?);
    let h = array.new_datum(m).map_err(e)?.ok_or("the list is full")?;
    let elem = array
        .try_get(m, h)
        .map_err(e)?
        .ok_or("the new row is not there")?;
    m.set_u16(elem.wrapping_add(h1row::SETUP_ACTION), ROW_ACTION as u16)
        .map_err(e)?;
    Ok(actions.len() + 1)
}

/// The row on the Game Setup dialog, and what its press opens.
pub struct SetupRow {
    pub page: Arc<KitScreen>,
}

fn row_text(c: &mut Call, page: &KitScreen, seen: &AtomicBool) -> Result<Flow, GuestFault> {
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
        // The row says the page's title, from its first text slot, written
        // before it is drawn.
        let id = page.title();
        if let Err(e) = page.write_texts(&mut *mem) {
            eprintln!("clankers: the row's text: {e:?}");
        }
        (item, id)
    };
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("clankers: Game Setup row text");
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
        eprintln!("clankers: page not opened (player {player:?}, stack {stack:?})");
        return c.ret(8).map(Flow::Return);
    };
    eprintln!(
        "clankers: opening the Clankers page (player {player}) on window {window}, slot {slot}"
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

impl feature::Feature for SetupRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ROW_FEATURE,
            "The Clankers row on a custom game's Game Setup",
        )
        .doc("docs/game/bots.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use engine::points::{
            SQUAD_SETTINGS_LIST_CTOR as CTOR, SQUAD_SETTINGS_ROW_CHOSEN, SQUAD_SETTINGS_ROW_TEXT,
        };
        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        let building: Arc<Mutex<Vec<u32>>> = Arc::default();
        let b2 = Arc::clone(&building);
        b.raw()
            .before(&CTOR, "note the Game Setup list", move |c| {
                b2.lock().unwrap_or_else(|e| e.into_inner()).push(c.cpu.esi);
                Ok(Flow::Continue)
            })
            .after_feature(h1row::ROW_FEATURE);
        b.raw()
            .after(&CTOR, "add the Clankers row", move |c| {
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
                    Ok(n) => eprintln!("clankers: Game Setup list: Clankers row added ({n} rows)"),
                    Err(why) => eprintln!("clankers: Game Setup list: row not added: {why}"),
                }
                Ok(())
            })
            .after_feature(h1row::ROW_FEATURE);
        let (page, seen) = (Arc::clone(&self.page), Arc::new(AtomicBool::new(false)));
        b.raw()
            .before(
                &SQUAD_SETTINGS_ROW_TEXT,
                "the Clankers row's text",
                move |c| row_text(c, &page, &seen),
            )
            .after_feature(h1row::ROW_FEATURE);
        b.raw()
            .before(
                &SQUAD_SETTINGS_ROW_CHOSEN,
                "the Clankers row's press",
                move |c| row_chosen(c, request_at),
            )
            .after_feature(h1row::ROW_FEATURE);
        Ok(())
    }
}

/// Adds the page, its texts, the Game Setup row and the driver.
pub fn install(registry: &mut feature::Registry, store: &Arc<Store>) -> anyhow::Result<()> {
    let page = Arc::new(page(Arc::clone(store)));
    registry
        .add(&KitTexts(Arc::clone(&page)))
        .map_err(|e| anyhow::anyhow!("clankers texts: {e}"))?;
    registry
        .add(&page.class())
        .map_err(|e| anyhow::anyhow!("clankers page: {e}"))?;
    registry
        .add(&SetupRow { page })
        .map_err(|e| anyhow::anyhow!("clankers row: {e}"))?;
    // The driver lives as long as the store's change callbacks hold it.
    let _ = drive(
        store,
        Driver::with_phantoms(local::global().clone(), bots::phantom::global().clone()),
    );
    Ok(())
}

#[cfg(test)]
mod tests;
