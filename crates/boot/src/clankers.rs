//! Clankers in custom games (docs/game/bots.md, section 7 and stage 7): the
//! settings, the Clankers page, its row on a custom game's Game Setup, and
//! the driver that keeps that many local bots.
//!
//! **The settings** (`clankers.*`) are kept in the profile store like any
//! other ([`DEFS`], in [`crate::setting_defs::ALL`]), so they persist and
//! `h2 settings` lists them.
//!
//! **The page** is a kit screen ([`page`]) of seven choice rows
//! ([`crate::ui_kit::choice::ChoiceRow`]) on the lobby's own settings
//! dialog, Quick Options (`DialogBase::QuickOptions`), as the Halo 1
//! Options page is: Quick Options' panel, header and list (its
//! `setting_lists` skin, label left and the value alone in the value
//! column), the focused row's description in its one-line help line, and
//! the legend in its footer (docs/game/ui-builder.md "The Dialog kind").
//!
//! **The row.** A custom game's Game Setup dialog gets a "Clanker Options..." row,
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
use crate::telemetry;
use crate::ui_kit::choice::ChoiceRow;
use crate::ui_kit::dialog::DialogBase;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::{Glyph, Prompt};
use crate::ui_kit::screen::{KitScreen, KitTexts};
use crate::ui_kit::view::Opener;

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
        "How well they play. Mixed varies it around Normal.",
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

/// The Game Setup row's text; the page's title stays [`TITLE`].
pub const ROW_LABEL: &str = "Clanker Options...";
/// The page's title, in capitals as Quick Options' header ("QUICK
/// OPTIONS") is.
pub const TITLE: &str = "CLANKERS";

/// The Clankers page over `store`: a dialog of its rows on Quick Options
/// (its seven rows fit the eleven its list shows).
pub fn page(store: Arc<Store>) -> KitScreen {
    let mut s = KitScreen::new(FEATURE, SCREEN, IDS, TITLE)
        .row_label(ROW_LABEL)
        .dialog(DialogBase::QuickOptions, Opener::GAME_SETUP_CLANKERS);
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

// --- Server rosters (docs/game/bots.md §9.1, stage 6c) --------------------

/// The skill a roster asks for at the page's difficulty: the named
/// difficulty's (§9.1); Mixed asks for none (the server picks Normal).
pub fn request_skill(difficulty: i64) -> Option<f32> {
    use live_proto::bots::{EASY, HEROIC, LEGENDARY, NORMAL};
    match difficulty {
        0 => Some(EASY),
        1 => Some(NORMAL),
        2 => Some(HEROIC),
        3 => Some(LEGENDARY),
        _ => None,
    }
}

/// The brain for a roster's 0-to-1 skill: the nearest of §3.4's four
/// (Easy 0, Normal 0.33, Heroic 0.67, Legendary 1).
pub fn skill_of(k: f32) -> Skill {
    match k {
        k if k < 1.0 / 6.0 => Skill::Easy,
        k if k < 0.5 => Skill::Normal,
        k if k < 5.0 / 6.0 => Skill::Heroic,
        _ => Skill::Legendary,
    }
}

/// One bot of a server roster, as a phantom runs it.
#[derive(Clone, Debug, PartialEq)]
pub struct RosterSeat {
    pub identity: Identity,
    /// The roster's team; `None`: the host balances.
    pub team: Option<u8>,
    pub skill: Skill,
    /// The personality's seed, for the brain: a persistent bot plays the
    /// same way every game.
    pub seed: u64,
    /// The personality's taunt (0 never; the page's Taunts off makes it 0).
    pub taunt: f32,
}

/// A checked roster's bots as phantoms: each bot's xuid, gamertag, emblem
/// and colours, its team and its brain's skill and quirks. The host never
/// changes any of them (§9.1).
pub fn roster_seats(roster: &live_proto::bots::Roster) -> Vec<RosterSeat> {
    roster
        .bots
        .iter()
        .map(|b| RosterSeat {
            identity: Identity {
                xuid: b.xuid.0,
                gamertag: b.gamertag.as_str().to_owned(),
                emblem: [
                    b.emblem.foreground,
                    b.emblem.background,
                    b.emblem.colors[0],
                    b.emblem.colors[1],
                    b.emblem.colors[2],
                ],
                colors: [b.colors.primary, b.colors.secondary],
            },
            team: b.team,
            skill: skill_of(b.skill),
            seed: b.personality.seed,
            taunt: b.personality.taunt,
        })
        .collect()
}

/// Where the driver gets server bots: Live, when it is on
/// ([`live_hle::Hle`]).
pub trait Rosters: Send + Sync {
    /// The Live session this console hosts (16 lowercase hex digits).
    /// Never waits.
    fn session(&self) -> Option<String>;
    /// The playlist (hopper descriptor index, as game records and ranks
    /// name it) when the hosted session is a matchmade one: its roster is
    /// then the playlist's persistent bot accounts nearest the humans'
    /// level (§9.4). `None` for a custom game. Never waits.
    fn playlist(&self) -> Option<String> {
        None
    }
    /// Asks for a roster; `done` gets it, checked, or why there is none,
    /// perhaps on another thread. Never waits.
    fn ask(
        &self,
        request: live_proto::bots::RosterRequest,
        done: Box<dyn FnOnce(Result<live_proto::bots::Roster, String>) + Send>,
    ) -> Result<(), String>;
    /// The bot accounts in the hosted game now ([`Driver::session_bots`]):
    /// the hosted session is told of them as players. Never waits.
    fn players(&self, _bots: Vec<live_proto::Xuid>) {}
    /// The hosted session and its hopper's mark while this console hosts
    /// a clanker hopper's lobby: a Clankermatch (docs/game/bots.md §8.4),
    /// which the driver fills with the playlist's bots. Never waits.
    fn clanker_match(&self) -> Option<(String, live_proto::hopper::clankers::ClankerHopper)> {
        None
    }
}

impl Rosters for live_hle::Hle {
    fn session(&self) -> Option<String> {
        self.hosted_session()
    }
    fn playlist(&self) -> Option<String> {
        let hosted = self.hosted_session()?;
        self.matchmade()
            .filter(|(_, session)| *session == hosted)
            .map(|(playlist, _)| playlist.to_string())
    }
    fn ask(
        &self,
        request: live_proto::bots::RosterRequest,
        done: Box<dyn FnOnce(Result<live_proto::bots::Roster, String>) + Send>,
    ) -> Result<(), String> {
        self.roster_bots(request, done)
    }
    fn players(&self, bots: Vec<live_proto::Xuid>) {
        self.set_session_players(bots)
    }
    fn clanker_match(&self) -> Option<(String, live_proto::hopper::clankers::ClankerHopper)> {
        self.clanker_hosted()
    }
}

/// What a roster was asked for: a new one is asked for when any of it
/// changes.
#[derive(Clone, Debug, PartialEq)]
struct Asked {
    session: String,
    playlist: Option<String>,
    count: u32,
    skill: Option<f32>,
}

impl Asked {
    /// The same bots asked for, whatever the session.
    fn same_bots(&self, other: &Asked) -> bool {
        self.playlist == other.playlist && self.count == other.count && self.skill == other.skill
    }
}

#[derive(Default)]
struct RosterState {
    asked: Option<Asked>,
    /// The answer to `asked`, once it came; or, while `carrying`, the
    /// previous session's roster, which stays in place meanwhile.
    roster: Option<Vec<RosterSeat>>,
    /// `asked` carries the roster of this earlier session (a handoff, §9.5)
    /// and the server has not answered yet: the bots keep who they are,
    /// but the roster is not the new session's until it does.
    carrying: Option<String>,
    /// The server answered `asked` with no roster: a Clankermatch fills
    /// with host-made bots instead of waiting.
    refused: bool,
}

/// A Clankermatch lobby being filled (docs/game/bots.md §8.4): the hosted
/// clanker hopper's bots join one at a time on
/// [`crate::clankermatch::schedule`].
#[derive(Clone, Debug)]
struct Fill {
    hopper: u32,
    /// The bots the playlist holds besides the human.
    bots: usize,
    /// The playlist's teams (0 free for all).
    teams: u32,
    /// How many of the bots may be local bots ([`max_local`]); the rest
    /// are phantoms.
    local: usize,
    schedule: Vec<std::time::Duration>,
    started: std::time::Instant,
    /// How many are in now.
    seated: usize,
    /// When this console stopped hosting a session (a handoff, or the
    /// player left): the fill ends [`FILL_GONE`] after.
    gone: Option<std::time::Instant>,
    /// The host player's team ([`Driver::set_host_team`]), followed until
    /// the first bot sits, then kept: a bot's team does not change.
    host_team: Option<u8>,
}

/// A Clankermatch ends this long after its console last hosted a session:
/// longer than a handoff's gap between the lobby's session and the game's
/// (a few seconds, §9.5), short enough that leaving ends it.
const FILL_GONE: std::time::Duration = std::time::Duration::from_secs(30);

/// How many of a Clankermatch's `bots` go on this console's free
/// controller ports (docs/game/bots.md §8.4): local bots sign in to the
/// lobby as session players, so the lobby reaches the playlist's fewest
/// players and the game starts it, which phantoms (in a game only) cannot
/// do. A console's players are one party, on one team in a team game, so
/// a team playlist takes no more than fill the human's team (a team of
/// `players / teams`); free for all takes up to the three free ports.
pub fn max_local(bots: usize, players: u32, teams: u32) -> usize {
    let party = if teams > 1 {
        (players / teams).saturating_sub(1) as usize
    } else {
        3
    };
    bots.min(party).min(3)
}

/// What the driver seats: the page's bots, or a Clankermatch's.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seating {
    /// Bots wanted in all (the roster's count).
    count: usize,
    /// Of those, how many are in now.
    seated: usize,
    /// A Clankermatch: nobody is seated before its roster comes (or is
    /// refused), and no more than `local` of the seated are local bots.
    fill: bool,
    /// The most local bots: the page's count, or a Clankermatch's
    /// [`max_local`].
    local: usize,
    /// The teams to balance the phantoms over (0: none).
    teams: u32,
    /// The roster's skill: the page's difficulty's, or none for a
    /// Clankermatch (the server sets it from the human's level, §8.2.3).
    skill: Option<f32>,
    /// The host player's team, the bots balance around (0 when unknown or
    /// not one of the game's teams).
    host_team: u8,
}

impl Seating {
    fn of(want: Want, fill: Option<&Fill>) -> Seating {
        match fill {
            Some(f) => Seating {
                count: f.bots,
                seated: f.seated.min(f.bots),
                fill: true,
                local: f.local.min(f.seated),
                teams: f.teams,
                skill: None,
                host_team: f.host_team.filter(|t| u32::from(*t) < f.teams).unwrap_or(0),
            },
            None => Seating {
                count: want.count,
                seated: want.count,
                fill: false,
                local: want.count,
                teams: 0,
                skill: request_skill(want.difficulty),
                host_team: 0,
            },
        }
    }
}

/// What a roster ask for `asked` does, with `r` the roster state before it.
#[derive(Debug, PartialEq)]
enum Ask {
    /// The roster in force, or being carried: nothing to ask.
    Have(Option<Vec<RosterSeat>>),
    /// Ask for a new roster; the bots are host-made until it comes.
    Fresh,
    /// The hosted session changed and the same bots are wanted: ask the
    /// server to carry this earlier session's roster to the new one, the
    /// bots staying as they are meanwhile.
    Carry(String),
}

/// Decides [`Ask`] for `asked` (`None`: this console hosts no session now,
/// as between a deleted session and the next one's advertisement).
fn ask_for(
    r: &RosterState,
    asked: Option<&Asked>,
    count: u32,
    skill: Option<f32>,
    playlist: Option<&String>,
) -> Ask {
    let Some(asked) = asked else {
        // Between sessions: a roster for these bots stays.
        return match (&r.asked, &r.roster) {
            (Some(a), Some(seats))
                if a.count == count && a.skill == skill && a.playlist.as_ref() == playlist =>
            {
                Ask::Have(Some(seats.clone()))
            }
            _ => Ask::Have(None),
        };
    };
    match (&r.asked, &r.roster) {
        (Some(a), _) if a == asked => Ask::Have(r.roster.clone()),
        // The roster's session, or, when its carry is not answered yet and
        // the session changed again, the one it was carried from.
        (Some(a), Some(_)) if a.same_bots(asked) => {
            Ask::Carry(r.carrying.clone().unwrap_or_else(|| a.session.clone()))
        }
        _ => Ask::Fresh,
    }
}

/// A Clanker: the brain, its skill shared with the driver so a change on
/// the page reaches it at its next frame.
struct Clanker {
    brain: bots::Brain,
    skill: Arc<AtomicU8>,
    /// The page's Taunts (shared by every Clanker of the driver).
    taunts: Arc<AtomicBool>,
    /// Its taunt with Taunts on: the default, or a roster bot's own.
    taunt: f32,
}

impl local::PadSource for Clanker {
    fn pad(&mut self, f: &local::Frame<'_>) -> input::host::HostPadState {
        let want = Skill::ALL[usize::from(self.skill.load(Ordering::Relaxed)).min(3)];
        if self.brain.mind.skill != want {
            self.brain.mind.skill = want;
        }
        self.brain.mind.emote.taunt = if self.taunts.load(Ordering::Relaxed) {
            self.taunt
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
    /// No bots.
    pub const OFF: Want = Want {
        count: 0,
        difficulty: 1,
        taunts: true,
    };

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

/// One of the driver's bots.
struct Own {
    seat: Seat,
    skill: Arc<AtomicU8>,
    /// A phantom's xuid; a local bot's bot account's, once a roster gave
    /// it one (0 none).
    xuid: u64,
    /// A roster bot: its skill is the server's, not the page's.
    rostered: bool,
}

/// Keeps the page's bots: local bots on free ports first, then, with
/// phantoms, phantom peers for the rest (docs/game/bots.md, stage 6).
///
/// With Live ([`Driver::with_rosters`]) and a session this console hosts,
/// every bot is one of the server's bot accounts (§9.5): the driver asks
/// for a roster of the whole count (`POST /v1/games/bots`) and, once it
/// comes, the local bots take its first bots as their accounts (they go on
/// Live as them, not as the human's guests) and the phantoms run the rest
/// in place of the host-made "Clanker N"s. It asks again when the count,
/// the difficulty or the session changes; a new roster replaces the old.
/// Without Live, or until a roster comes (or when none does), local bots
/// have no account (on Live, the human's guests) and phantoms are
/// host-made: local xuids, which no game record ever carries.
pub struct Driver {
    local: Local,
    phantoms: Option<Phantoms>,
    own: Mutex<Vec<Own>>,
    taunts: Arc<AtomicBool>,
    rosters: Option<Arc<dyn Rosters>>,
    roster: Mutex<RosterState>,
    /// The last [`Want`] applied, for applying again when a roster comes
    /// or the session changes.
    last: Mutex<Option<Want>>,
    /// The last apply wanted bots (and so a roster, with Live).
    wants_roster: AtomicBool,
    /// Itself, for a roster's answer to reach it ([`drive`] sets it).
    me: std::sync::Weak<Driver>,
    /// The hosted session was last told of some bots
    /// ([`Driver::report_players`]).
    reported: AtomicBool,
    /// The Clankermatch being filled, while this console hosts one.
    fill: Mutex<Option<Fill>>,
    /// The host player's team, as [`frame`] last read it.
    host_team: Mutex<Option<u8>>,
}

/// The bot accounts among `own`: local bots with one, and rostered
/// phantoms whose players the game made (`joined`).
fn session_bots_of(own: &[Own], joined: impl Fn(PhantomHandle) -> bool) -> Vec<live_proto::Xuid> {
    own.iter()
        .filter(|o| {
            o.xuid != 0
                && match o.seat {
                    Seat::Port(..) => true,
                    Seat::Phantom(_, h) => o.rostered && joined(h),
                }
        })
        .map(|o| live_proto::Xuid(o.xuid))
        .collect()
}

impl Driver {
    /// Local bots only.
    pub fn new(local: Local) -> Self {
        Driver {
            local,
            phantoms: None,
            own: Mutex::new(Vec::new()),
            taunts: Arc::new(AtomicBool::new(true)),
            rosters: None,
            roster: Mutex::default(),
            last: Mutex::new(None),
            wants_roster: AtomicBool::new(false),
            me: std::sync::Weak::new(),
            reported: AtomicBool::new(false),
            fill: Mutex::new(None),
            host_team: Mutex::new(None),
        }
    }

    /// Local bots, then phantoms.
    pub fn with_phantoms(local: Local, phantoms: Phantoms) -> Self {
        Driver {
            phantoms: Some(phantoms),
            ..Driver::new(local)
        }
    }

    /// Phantoms from the server's rosters while this console hosts a Live
    /// session.
    pub fn with_rosters(mut self, rosters: Arc<dyn Rosters>) -> Self {
        self.rosters = Some(rosters);
        self
    }

    /// The roster in force for `session` asked for `count` at `skill`, or
    /// `None`, asking for it when it was not asked for yet.
    fn rostered(
        &self,
        count: usize,
        skill: Option<f32>,
        teams: bool,
        log: &mut Vec<String>,
    ) -> Option<Vec<RosterSeat>> {
        let rosters = self.rosters.as_ref()?;
        let playlist = rosters.playlist();
        let session = rosters.session();
        let mut r = self.roster.lock().unwrap_or_else(|e| e.into_inner());
        let Some(session) = session else {
            if let Ask::Have(Some(seats)) =
                ask_for(&r, None, count as u32, skill, playlist.as_ref())
            {
                // A handoff under way: the lobby's session went, the next
                // is not advertised yet. The bots stay who they are.
                return Some(seats);
            }
            // Said so a run shows why no roster was asked for: the game
            // hosts no Xbox Live session (not signed in to Live, Split
            // Screen, System Link). A Live session hosted later asks
            // (`refresh`, every 30 frames).
            log.push(
                "no roster yet: this console hosts no Xbox Live session (host-made bots until it does)"
                    .into(),
            );
            return None;
        };
        let asked = Asked {
            playlist,
            session,
            count: count as u32,
            skill,
        };
        let carry_from = match ask_for(
            &r,
            Some(&asked),
            asked.count,
            skill,
            asked.playlist.as_ref(),
        ) {
            Ask::Have(seats) => return seats,
            Ask::Fresh => {
                r.roster = None;
                r.carrying = None;
                r.refused = false;
                None
            }
            Ask::Carry(from) => {
                // The old roster stays in `roster` meanwhile.
                r.carrying = Some(from.clone());
                Some(from)
            }
        };
        r.asked = Some(asked.clone());
        let kept = r.roster.clone();
        drop(r);
        let request = live_proto::bots::RosterRequest {
            session_id: asked.session.clone(),
            playlist: asked.playlist.clone(),
            count: asked.count,
            skill: asked.skill,
            teams,
            carry_from: carry_from.clone(),
        };
        let session = asked.session.clone();
        let me = self.me.clone();
        let done = Box::new(move |answer: Result<live_proto::bots::Roster, String>| {
            let Some(d) = me.upgrade() else { return };
            for line in d.answered(&asked, answer) {
                eprintln!("clankers: {line}");
            }
        });
        match (rosters.ask(request, done), &carry_from) {
            (Ok(()), None) => log.push(format!(
                "asking Live for {count} bots for session {session}"
            )),
            (Ok(()), Some(from)) => log.push(format!(
                "the game moved from session {from} to {session}: asking Live to carry its bots there"
            )),
            (Err(e), _) => log.push(format!("no roster from Live: {e}")),
        }
        kept
    }

    /// A roster's answer to `asked`: kept and applied when it is still the
    /// one asked for. Lines for the log.
    fn answered(
        &self,
        asked: &Asked,
        answer: Result<live_proto::bots::Roster, String>,
    ) -> Vec<String> {
        {
            let mut r = self.roster.lock().unwrap_or_else(|e| e.into_inner());
            if r.asked.as_ref() != Some(asked) {
                return vec!["an old roster's answer, dropped".into()];
            }
            let carried = r.carrying.take();
            match answer {
                Ok(roster) if roster.match_id == asked.session => {
                    r.roster = Some(roster_seats(&roster));
                }
                answer => {
                    let why = match answer {
                        Ok(roster) => format!(
                            "a roster for session {}, not {}",
                            roster.match_id, asked.session
                        ),
                        Err(e) => format!("no roster from Live ({e})"),
                    };
                    let Some(from) = carried else {
                        r.refused = true;
                        drop(r);
                        let mut log = vec![format!("{why}: host-made bots")];
                        // A Clankermatch waiting on it fills now.
                        if self.filling() {
                            let want = *self.last.lock().unwrap_or_else(|e| e.into_inner());
                            log.extend(self.apply(want.unwrap_or(Want::OFF)));
                        }
                        return log;
                    };
                    // The carry failed: the old roster is not this
                    // session's, so ask for a new one.
                    r.asked = None;
                    r.roster = None;
                    drop(r);
                    let mut log = vec![format!(
                        "{why}: session {from}'s bots did not carry to {}; asking for new ones",
                        asked.session
                    )];
                    // Copied out first: `apply` takes `last` again, and an
                    // `if let` would hold its guard through the body.
                    let want = *self.last.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(want) = want {
                        log.extend(self.apply(want));
                    }
                    return log;
                }
            }
        }
        let want = *self.last.lock().unwrap_or_else(|e| e.into_inner());
        let mut log = vec![format!("Live rostered bots for session {}", asked.session)];
        if let Some(want) = want {
            log.extend(self.apply(want));
        }
        log
    }

    /// Applies the last [`Want`] again if the hosted Live session is not
    /// the one the roster was asked for (a session made after the page was
    /// set). Cheap when nothing changed.
    pub fn refresh(&self) -> Vec<String> {
        let Some(rosters) = &self.rosters else {
            return Vec::new();
        };
        if !self.wants_roster.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let session = rosters.session();
        let asked = self
            .roster
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .asked
            .as_ref()
            .map(|a| a.session.clone());
        if session.is_none() || session == asked {
            return Vec::new();
        }
        let want = *self.last.lock().unwrap_or_else(|e| e.into_inner());
        want.map_or_else(Vec::new, |w| self.apply(w))
    }

    /// The bots the next game runs with, for its record (docs/game/bots.md
    /// §6): every Clanker goes out as a bot. Local bots by their ports (the
    /// driver's and any other on the books, `sim.*`'s too), phantoms by
    /// machine id. While a server roster is in force (its session, while
    /// this console still hosts it), the game names that session and each
    /// rostered phantom carries its rostered xuid; no other bot has one.
    /// `None` when there are no bots and no roster.
    pub fn bots_in_force(&self) -> Option<telemetry::Bots> {
        let session_id = self.roster_in_force();
        let own = self.own.lock().unwrap_or_else(|e| e.into_inner());
        // Only bit 0..7 can be a port.
        let mut ports = (self.local.port_mask() & 0xff) as u8;
        let mut machines = Vec::new();
        let mut accounts = Vec::new();
        for o in own.iter() {
            match o.seat {
                Seat::Port(p, _) => {
                    ports |= 1u8.checked_shl(u32::from(p)).unwrap_or(0);
                    if o.xuid != 0 && session_id.is_some() {
                        accounts.push((p, live_proto::Xuid(o.xuid)));
                    }
                }
                Seat::Phantom(_, h) => {
                    let xuid = if o.rostered && session_id.is_some() {
                        o.xuid
                    } else {
                        0
                    };
                    machines.push((bots::phantom::machine_id(h), live_proto::Xuid(xuid)));
                }
            }
        }
        drop(own);
        // Phantoms someone else added (`sim.player.add`): bots, no xuid.
        for p in self.phantoms.iter().flat_map(|ph| ph.list()) {
            if !machines.iter().any(|(m, _)| *m == p.machine) {
                machines.push((p.machine, live_proto::Xuid(0)));
            }
        }
        if ports == 0 && machines.is_empty() && session_id.is_none() {
            return None;
        }
        Some(telemetry::Bots {
            session_id,
            machines,
            ports,
            accounts,
        })
    }

    /// The bot accounts in the hosted game while a server roster is in
    /// force: the local bots' accounts, and the rostered phantoms the game
    /// made players for. Empty with no roster in force. The hosted Live
    /// session is told of them as players (§9.5), so it counts every
    /// player the game has.
    pub fn session_bots(&self) -> Vec<live_proto::Xuid> {
        if self.roster_in_force().is_none() {
            return Vec::new();
        }
        let joined: Vec<PhantomHandle> = self
            .phantoms
            .iter()
            .flat_map(|ph| ph.list())
            .filter(|p| p.state == bots::phantom::State::Joined)
            .map(|p| p.handle)
            .collect();
        let own = self.own.lock().unwrap_or_else(|e| e.into_inner());
        session_bots_of(&own, |h| joined.contains(&h))
    }

    /// Tells the hosted session its players ([`Driver::session_bots`]),
    /// while a roster is in force and once more after, so the bots it
    /// was told of leave it.
    fn report_players(&self) {
        let Some(rosters) = &self.rosters else {
            return;
        };
        let bots = self.session_bots();
        let had = self.reported.swap(!bots.is_empty(), Ordering::Relaxed);
        if had || !bots.is_empty() {
            rosters.players(bots);
        }
    }

    /// The session of the server roster in force: the one asked for this
    /// console's hosted session, once its answer came.
    fn roster_in_force(&self) -> Option<String> {
        let session = self.rosters.as_ref()?.session()?;
        let r = self.roster.lock().unwrap_or_else(|e| e.into_inner());
        let asked = r.asked.as_ref().filter(|a| a.session == session)?;
        r.roster.as_ref()?;
        // A carried roster is the new session's once the server says so.
        if r.carrying.is_some() {
            return None;
        }
        Some(asked.session.clone())
    }

    /// Adds or takes off bots to match `want`; what it did, for the log.
    pub fn apply(&self, want: Want) -> Vec<String> {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(want);
        let seating = Seating::of(
            want,
            self.fill.lock().unwrap_or_else(|e| e.into_inner()).as_ref(),
        );
        let mut own = self.own.lock().unwrap_or_else(|e| e.into_inner());
        let mut log = Vec::new();
        self.taunts.store(want.taunts, Ordering::Relaxed);
        // Fewer: the last added go first. A Clankermatch's local bots are
        // its first, so they are the last to go.
        while own.len() > seating.seated {
            let o = own.pop().expect("more than wanted");
            self.take_off(o.seat, &mut log);
        }
        let port_count = |own: &[Own]| {
            own.iter()
                .filter(|o| matches!(o.seat, Seat::Port(..)))
                .count()
        };
        while port_count(&own) > seating.local {
            let i = own
                .iter()
                .rposition(|o| matches!(o.seat, Seat::Port(..)))
                .expect("a local bot");
            let o = own.remove(i);
            self.take_off(o.seat, &mut log);
        }
        for o in own.iter().filter(|o| !o.rostered) {
            o.skill.store(
                skill_index(skill(want.difficulty, seat_port(o.seat))),
                Ordering::Relaxed,
            );
        }
        self.wants_roster
            .store(seating.count > 0, Ordering::Relaxed);
        // With Live, every bot is a server bot account: the local bots take
        // the roster's first, the phantoms the rest.
        let mut roster = if seating.count > 0 {
            self.rostered(seating.count, seating.skill, seating.teams > 1, &mut log)
        } else {
            None
        }
        .map(Vec::into_iter);
        // A Clankermatch waits for its roster rather than seat host-made
        // bots that the roster would replace (or local bots that would go
        // on Xbox Live as the human's guests first); refused, it takes them.
        let hold = seating.fill
            && roster.is_none()
            && !self
                .roster
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .refused;
        // More local bots: on the lowest ports no bot asked for. The page's
        // count fills them before any phantom; a Clankermatch fills up to
        // its `local` first.
        while !hold
            && if seating.fill {
                port_count(&own) < seating.local
            } else {
                own.len() < want.count
            }
        {
            let taken: Vec<u8> = self.local.list().iter().map(|b| b.wanted).collect();
            let Some(port) = (1..=3u8).find(|p| !taken.contains(p)) else {
                break;
            };
            let k = skill(want.difficulty, port);
            let (src, s) = self.clanker(k, 0xc1a4 ^ u64::from(port), bots::emote::DEFAULT_TAUNT);
            let name = format!("Clanker {port}");
            match self.local.add(port, &name, Box::new(src)) {
                Ok(h) => {
                    log.push(format!("{name} on ({k:?})"));
                    // A Clankermatch's local bots go before its phantoms,
                    // so they take the roster's first bots.
                    let at = own
                        .iter()
                        .position(|o| matches!(o.seat, Seat::Phantom(..)))
                        .unwrap_or(own.len());
                    own.insert(
                        at,
                        Own {
                            seat: Seat::Port(port, h),
                            skill: s,
                            xuid: 0,
                            rostered: false,
                        },
                    );
                }
                Err(e) => {
                    log.push(format!("{name}: {e}"));
                    break;
                }
            }
        }
        let ports = port_count(&own);
        let wanted = if hold {
            0
        } else {
            seating.seated.saturating_sub(ports)
        };
        for o in own.iter_mut() {
            let Seat::Port(port, h) = o.seat else {
                continue;
            };
            let entry = roster.as_mut().and_then(Iterator::next);
            // A Clankermatch's local bot plays at its roster bot's skill
            // (the server's, from the human's level), as a phantom would.
            if let Some(r) = entry.as_ref().filter(|_| seating.fill) {
                o.skill.store(skill_index(r.skill), Ordering::Relaxed);
            }
            let account = entry.map(|r| bots::guest::BotAccount {
                xuid: r.identity.xuid,
                gamertag: r.identity.gamertag,
            });
            let xuid = account.as_ref().map_or(0, |a| a.xuid);
            if xuid != o.xuid {
                match &account {
                    Some(a) => log.push(format!(
                        "Clanker {port} is {} on Xbox Live (bot account {:016x})",
                        a.gamertag, a.xuid
                    )),
                    None => log.push(format!("Clanker {port} has no bot account")),
                }
            }
            if self.local.set_account(h, account).is_ok() {
                o.xuid = xuid;
            }
        }
        if wanted == 0 {
            return log;
        }
        let Some(ph) = &self.phantoms else {
            log.push(format!(
                "{} asked for, {} on this machine: no free port",
                want.count,
                own.len()
            ));
            return log;
        };
        // The phantoms: the server's roster's bots, else host-made ones.
        let mut seats: Vec<(RosterSeat, bool)> = match roster {
            Some(r) => r.map(|s| (s, true)).collect(),
            None => (0..seating.count.saturating_sub(ports))
                .map(|i| {
                    let n = (ports + i + 1) as u32;
                    let k = skill(want.difficulty, ((n - 1) % 3 + 1) as u8);
                    (
                        RosterSeat {
                            identity: Identity {
                                xuid: live_proto::bots::local_xuid(n).0,
                                gamertag: phantom_name(n),
                                ..Default::default()
                            },
                            team: None,
                            skill: k,
                            seed: 0xc1a4 ^ u64::from(n),
                            taunt: bots::emote::DEFAULT_TAUNT,
                        },
                        false,
                    )
                })
                .collect(),
        };
        // A Clankermatch's teams, balanced over the whole roster so a
        // bot's team does not change as the others join (§9.1: "the host
        // balances by total skill"), around the human's team.
        if seating.teams > 1 {
            let k = |s: &RosterSeat| f32::from(skill_index(s.skill)) / 3.0;
            let mean = seats.iter().map(|(s, _)| k(s)).sum::<f32>() / seats.len().max(1) as f32;
            // The human (at the bots' mean: the server matched them to
            // its level) and the bots the roster put on a team are fixed;
            // the rest are balanced around them.
            let mut fixed = vec![(seating.host_team, mean)];
            // A Clankermatch's local bots are the human's party: on its
            // team (a console's players play on one team).
            if seating.fill {
                fixed.extend(std::iter::repeat_n((seating.host_team, mean), ports));
            }
            fixed.extend(seats.iter().filter_map(|(s, _)| Some((s.team?, k(s)))));
            let free: Vec<f32> = seats
                .iter()
                .filter(|(s, _)| s.team.is_none())
                .map(|(s, _)| k(s))
                .collect();
            let mut teams = crate::clankermatch::balance(seating.teams, &fixed, &free).into_iter();
            for (s, _) in seats.iter_mut().filter(|(s, _)| s.team.is_none()) {
                s.team = teams.next().flatten();
            }
        }
        seats.truncate(wanted);
        // The phantoms there already stay while they are the first of
        // these; otherwise (a new roster) they all go.
        let have: Vec<(u64, bool)> = own
            .iter()
            .filter(|o| matches!(o.seat, Seat::Phantom(..)))
            .map(|o| (o.xuid, o.rostered))
            .collect();
        let keep = have.len() <= seats.len()
            && have
                .iter()
                .zip(&seats)
                .all(|(h, (s, r))| h.0 == s.identity.xuid && h.1 == *r);
        if !keep {
            while let Some(i) = own
                .iter()
                .rposition(|o| matches!(o.seat, Seat::Phantom(..)))
            {
                let o = own.remove(i);
                self.take_off(o.seat, &mut log);
            }
        }
        let start = if keep { have.len() } else { 0 };
        for (seat, rostered) in seats.into_iter().skip(start) {
            let n = own.len() as u32 + 1;
            let (src, s) = self.clanker(seat.skill, seat.seed, seat.taunt);
            let name = format!(
                "{} (phantom{})",
                seat.identity.gamertag,
                if rostered { ", rostered" } else { "" }
            );
            let xuid = seat.identity.xuid;
            match ph.add(seat.identity, seat.team, Box::new(src)) {
                Ok(h) => {
                    log.push(format!("{name} on ({:?})", seat.skill));
                    own.push(Own {
                        seat: Seat::Phantom(n, h),
                        skill: s,
                        xuid,
                        rostered,
                    });
                }
                Err(e) => {
                    log.push(format!("{name}: {e}"));
                    break;
                }
            }
        }
        log
    }

    fn clanker(&self, k: Skill, seed: u64, taunt: f32) -> (Clanker, Arc<AtomicU8>) {
        let s = Arc::new(AtomicU8::new(skill_index(k)));
        let c = Clanker {
            brain: bots::Brain::new(k, seed),
            skill: Arc::clone(&s),
            taunts: Arc::clone(&self.taunts),
            taunt,
        };
        (c, s)
    }

    fn take_off(&self, seat: Seat, log: &mut Vec<String>) {
        let (name, r) = match seat {
            Seat::Port(port, h) => (
                format!("Clanker {port}"),
                self.local.remove(h).map_err(|e| e.to_string()),
            ),
            Seat::Phantom(n, h) => (
                self.phantoms
                    .as_ref()
                    .and_then(|p| p.list().into_iter().find(|i| i.handle == h))
                    .map_or_else(|| phantom_name(n), |i| i.identity.gamertag),
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

    /// Whether the last [`Want`] applied asked for any bot (Clankers on),
    /// or a Clankermatch is being filled.
    pub fn on(&self) -> bool {
        let last = *self.last.lock().unwrap_or_else(|e| e.into_inner());
        last.is_some_and(|w| w.count > 0) || self.filling()
    }

    /// Whether a Clankermatch is being filled.
    fn filling(&self) -> bool {
        self.fill
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// The host player's team, read from the game (its first local
    /// player's configuration), or `None` while it has none. A
    /// Clankermatch's bots balance around it.
    pub fn set_host_team(&self, team: Option<u8>) {
        *self.host_team.lock().unwrap_or_else(|e| e.into_inner()) = team;
    }

    /// Follows the Clankermatch this console hosts, at `now`: a clanker
    /// hopper's lobby starts a fill, its bots join on their schedule, and
    /// it ends when the console hosts another kind of session or none for
    /// [`FILL_GONE`]. Applies the last [`Want`] again when that changes
    /// what is seated. Lines for the log.
    pub fn tick_match(&self, now: std::time::Instant) -> Vec<String> {
        let Some(rosters) = &self.rosters else {
            return Vec::new();
        };
        let hosted = rosters.clanker_match();
        let hosting = hosted.is_some() || rosters.session().is_some();
        let host_team = *self.host_team.lock().unwrap_or_else(|e| e.into_inner());
        let mut log = Vec::new();
        let changed = {
            let mut fill = self.fill.lock().unwrap_or_else(|e| e.into_inner());
            let current = fill.as_ref().map(|f| f.hopper);
            match (current, hosted) {
                (Some(h), Some((_, mark))) if h == mark.hopper_id => {
                    let f = fill.as_mut().expect("a fill");
                    f.gone = None;
                    if f.seated == 0 {
                        f.host_team = host_team;
                    }
                    let n = crate::clankermatch::seated(
                        &f.schedule,
                        now.saturating_duration_since(f.started),
                    );
                    let more = n != f.seated;
                    f.seated = n;
                    more
                }
                (_, Some((session, mark))) => {
                    let bots = mark.bots_for(1) as usize;
                    let seed = u64::from_str_radix(&session, 16).unwrap_or(0) ^ 0xc1a4;
                    let schedule = crate::clankermatch::schedule(bots, seed);
                    log.push(format!(
                        "clankermatch: hosting clanker playlist {} (session {session}): {bots} bots join over {:.1} s{}",
                        mark.hopper_id,
                        schedule.last().map_or(0.0, |t| t.as_secs_f32()),
                        if mark.teams > 1 {
                            format!(
                                ", {} teams, the host on {}",
                                mark.teams,
                                host_team.map_or("an unknown team (0)".into(), |t| format!("team {t}"))
                            )
                        } else {
                            String::new()
                        }
                    ));
                    // Which bots sit in the lobby and which join the game
                    // (docs/game/bots.md §8.4): local bots sign in to the
                    // lobby as session players, so it reaches the
                    // playlist's fewest players and starts; a phantom
                    // enters the session only once the game runs (§5.1).
                    let local = max_local(bots, mark.players, mark.teams);
                    log.push(format!(
                        "clankermatch: {local} local bots join the lobby on free controller ports; \
                         {} phantoms join when the game starts (\"bots: phantom ... joins\")",
                        bots - local
                    ));
                    *fill = Some(Fill {
                        hopper: mark.hopper_id,
                        bots,
                        teams: mark.teams,
                        local,
                        schedule,
                        started: now,
                        seated: 0,
                        gone: None,
                        host_team,
                    });
                    true
                }
                (Some(_), None) if hosting => {
                    log.push("clankermatch: this console hosts another game: the bots go".into());
                    *fill = None;
                    true
                }
                (Some(_), None) => {
                    let f = fill.as_mut().expect("a fill");
                    let since = *f.gone.get_or_insert(now);
                    if now.saturating_duration_since(since) > FILL_GONE {
                        log.push("clankermatch: the game is over: the bots go".into());
                        *fill = None;
                        true
                    } else {
                        false
                    }
                }
                (None, None) => false,
            }
        };
        if changed {
            let want = *self.last.lock().unwrap_or_else(|e| e.into_inner());
            log.extend(self.apply(want.unwrap_or(Want::OFF)));
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
            .map(|b| b.seat)
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

/// The driver [`install`] started, for [`frame`] and telemetry.
static DRIVER: Mutex<Option<std::sync::Weak<Driver>>> = Mutex::new(None);

/// Once a frame, from the bots' frame hook: every second or so, asks
/// for a roster again if the hosted Live session changed
/// ([`Driver::refresh`]). Never waits on the server. `host_team` reads
/// the host player's team from the game; it is called twice a second.
pub fn frame(n: u64, host_team: impl FnOnce() -> Option<u8>) {
    // Twice a second: a Clankermatch's bots join 0.5 to 3 s apart.
    if n % 15 != 0 {
        return;
    }
    let d = DRIVER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|w| w.upgrade());
    let Some(d) = d else { return };
    d.set_host_team(host_team());
    for line in d.tick_match(std::time::Instant::now()) {
        eprintln!("clankers: {line}");
    }
    if n % 30 == 0 {
        for line in d.refresh() {
            eprintln!("clankers: {line}");
        }
        d.report_players();
    }
}

/// The map picked last in the lobby's map list (its short name), for
/// [`build_soon_on`] when Clankers comes on after it.
static PICKED: Mutex<String> = Mutex::new(String::new());

/// A map picked in the lobby's map list (`crate::map_select`): with
/// Clankers on, its nav mesh is asked for now ([`bots::autonav::build_soon`]),
/// so it is usually there when the game starts.
pub fn map_picked(map: &str) {
    *PICKED.lock().unwrap_or_else(|e| e.into_inner()) = map.to_owned();
    let on = DRIVER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|w| w.upgrade())
        .is_some_and(|d| d.on());
    if on {
        bots::autonav::build_soon(map);
    }
}

/// Clankers just came on (`was` off, `now` on): asks for the nav mesh of
/// the map picked last, if one was. Returns at once; a no-op when the
/// automatic build is off.
fn build_soon_on(was: bool, now: bool) {
    if !was && now {
        let map = PICKED.lock().unwrap_or_else(|e| e.into_inner()).clone();
        bots::autonav::build_soon(&map);
    }
}

/// Starts `driver` over `store`: matches it now and on every change.
pub fn drive(store: &Arc<Store>, driver: Driver) -> Arc<Driver> {
    let d = Arc::new_cyclic(|me| Driver {
        me: me.clone(),
        ..driver
    });
    let run = {
        let (d, store) = (Arc::clone(&d), Arc::downgrade(store));
        move |_: &Value| {
            if let Some(store) = store.upgrade() {
                let was = d.on();
                for line in d.apply(Want::read(&store)) {
                    eprintln!("clankers: {line}");
                }
                build_soon_on(was, d.on());
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

/// Adds a row with [`ROW_ACTION`] to the list at `list`, under Quick
/// Options (and the Halo 1 Options row) and above the "Switch To:" rows;
/// the new count.
pub fn add_row(m: &mut dyn GuestMemory, list: u32) -> Result<usize, String> {
    h1row::insert_row(m, list, ROW_ACTION)
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
        // The row's own text ("Clanker Options..."), from its slot after the
        // page's footer, written before it is drawn.
        let id = page.row_label_id();
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
        let (b2, live) = (Arc::clone(&building), Arc::clone(&building));
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
        // The page's state is its kit screen's (`ui-kit-texts.*`), the
        // bots' the settings store's.
        b.snapshot(feature::SnapshotPolicy::forbid_while(move || {
            h1row::building_live(&live)
        }));
        Ok(())
    }
}

/// Adds the page, its texts, the Game Setup row and the driver. With
/// Live, the driver's phantoms are the server's bots. Telemetry marks every
/// bot (docs/game/bots.md §6, stage 6c).
pub fn install(
    registry: &mut feature::Registry,
    store: &Arc<Store>,
    live: Option<Arc<live_hle::Hle>>,
    telemetry: Option<&Arc<telemetry::Telemetry>>,
) -> anyhow::Result<()> {
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
    let mut driver =
        Driver::with_phantoms(local::global().clone(), bots::phantom::global().clone());
    if let Some(hle) = live {
        driver = driver.with_rosters(hle);
    }
    let d = drive(store, driver);
    *DRIVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(&d));
    // Every bot goes out as a bot; a game with server bots names their
    // roster's session, and its phantoms carry their rostered xuids.
    if let Some(t) = telemetry {
        let d = Arc::downgrade(&d);
        t.set_bots_locator(Box::new(move || d.upgrade()?.bots_in_force()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
