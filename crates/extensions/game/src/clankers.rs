//! Clankers in custom games (docs/game/bots.md, section 7 and stage 7): the
//! settings, the Clankers page, its row on a custom game's Game Setup, and
//! the driver that keeps that many phantom bots.
//!
//! **The settings** (`clankers.*`) are kept in the profile store like any
//! other ([`DEFS`], in [`crate::setting_defs::ALL`]), so they persist and
//! `h2 settings` lists them.
//!
//! **The page** is the shared settings dialog ([`page`],
//! `crate::ui_kit::settings`), declared by `content/ui/screens/clankers.yaml`:
//! eight rows over [`DEFS`] on the lobby's own settings dialog, Quick
//! Options, as the Halo 1 Options page is. Its header (icon, name and
//! what the window is for) is the same whatever has the focus; the
//! focused row's help is under the rows, and the value picker A opens
//! says what the row does and what each choice means
//! (docs/game/ui-builder.md "The settings dialog").
//!
//! **The row.** A custom game's Game Setup dialog gets a "Clanker Options..." row,
//! the way it gets the Halo 1 Options row (`crate::h1_options_row`, whose
//! helpers this uses): an `after` on the list's constructor adds a row with
//! [`ROW_ACTION`] to every list that has Change Map; `before` hooks draw
//! its text and take its press, which opens the page on the dialog's own
//! stack. Every hook is ordered after the Halo 1 row's, and each passes any
//! other action on.
//!
//! **The driver** ([`Driver`]) keeps `count` brains in
//! [`bots::phantom::global`] while `clankers.enabled` is on, each a remote
//! peer of a session this console hosts, and takes its own off when it
//! goes off. A difficulty change reaches the bots it already has at their
//! next frame; a count change adds or takes off bots at once.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use settings::{Apply, Choice, Def, Kind, Scope, Store, Tier, Unit, Value};

use bots::mind::Skill;
use bots::pad_source::{Frame, PadSource, Sampled};
use bots::phantom::{Identity, PhantomHandle, Phantoms};

use crate::h1_options_row::{self as h1row, CHANGE_MAP};
use crate::list_class::{self, Rows};
use crate::telemetry;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::screen::{KitScreen, KitTexts};
use crate::ui_kit::settings::Bindings;

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
    "Bots join custom games as remote peers of this machine's session, never on a \
     controller port; all show in the pregame lobby (docs/game/bots.md).",
    Kind::Bool,
    Value::Bool(false),
);
pub static COUNT: Def = def(
    "clankers.count",
    "Count",
    "How many bots join, up to 15, all remote peers of this machine's session.",
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

pub static GLITCHES: Def = def(
    "clankers.glitches",
    "Glitches",
    "Whether bots use Halo 2's glitches: BXR, double shots, sword flying, \
     super bounces. Some is the occasional show-off; Lots whenever they can.",
    Kind::Choice(&[
        choice(0, "off", "Off"),
        choice(1, "some", "Some"),
        choice(2, "lots", "Lots"),
    ]),
    Value::Int(0),
);

/// Every Clankers setting, in the page's order.
pub static DEFS: [&Def; 8] = [
    &ENABLED,
    &COUNT,
    &DIFFICULTY,
    &TEAM,
    &AGGRESSION,
    &AIM_STYLE,
    &TAUNTS,
    &GLITCHES,
];

/// The Clankers page over `store`: the shared settings dialog
/// (`crate::ui_kit::settings`) its doc declares,
/// `content/ui/screens/clankers.yaml`: its header, its eight rows over
/// [`DEFS`], their help and picker copy, and the Game Setup row's text
/// and help (`settings.caller`).
pub fn page(store: Arc<Store>) -> KitScreen {
    crate::ui_kit::settings::screen(FEATURE, SCREEN, IDS, Some(store), Bindings::new())
        .unwrap_or_else(|e| panic!("{e}"))
}

// --- The driver -----------------------------------------------------------

/// The skill of the bot with Mixed slot `slot` (1 to 3 in turn) at
/// difficulty `d` (a [`DIFFICULTY`] value): Mixed is Normal, Heroic and
/// Easy in turn.
pub fn skill(d: i64, slot: u8) -> Skill {
    let d = if d == MIXED {
        [1, 2, 0][usize::from(slot.saturating_sub(1)) % 3]
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
    /// Seed for moment-to-moment play. The gamertag determines trait weights.
    pub seed: u64,
    /// The level (1 to 50) Live rostered it at in the playlist: its own
    /// standing for a persistent bot, the humans' for a made-up one.
    /// `None` for a host-made bot.
    pub level: Option<u8>,
}

/// The level (1 to 50) a host-made Clanker starts at in a ranked hopper,
/// when Live has no record of it: a stable draw from its name inside its
/// difficulty's band (Easy 1 to 10, Normal 11 to 25, Heroic 26 to 40,
/// Legendary 41 to 50). Never saved; the name gives the same level every
/// game.
pub fn starting_level(name: &str, skill: Skill) -> u8 {
    let (low, high) = match skill {
        Skill::Easy => (1, 10),
        Skill::Normal => (11, 25),
        Skill::Heroic => (26, 40),
        Skill::Legendary => (41, 50),
    };
    let mut h = 2_166_136_261u32;
    for b in name.to_lowercase().split_whitespace().flat_map(|w| w.bytes().chain(*b" ")) {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    low + (h % (u32::from(high - low) + 1)) as u8
}

/// A checked roster supplies the identity, team and brain. The player
/// configuration derives the emblem and colours from this gamertag (§9.1).
pub fn roster_seats(roster: &live_proto::bots::Roster) -> Vec<RosterSeat> {
    roster
        .bots
        .iter()
        .map(|b| RosterSeat {
            identity: Identity {
                xuid: b.xuid.0,
                gamertag: b.gamertag.as_str().to_owned(),
            },
            team: b.team,
            skill: skill_of(b.skill),
            seed: b.personality.seed,
            level: b.rank.as_ref().map(|r| r.level),
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
    /// The bot accounts in the hosted game now ([`Driver::session_bots`]),
    /// rostered for `session` (the roster in force's, `None` with none):
    /// that session is told of them as players, and no other. Never
    /// waits.
    fn players(&self, _session: Option<String>, _bots: Vec<live_proto::Xuid>) {}
    /// The hosted session and its hopper's mark while this console hosts
    /// a clanker hopper's lobby: a Clankermatch (docs/game/bots.md §8.4),
    /// which the driver fills with the playlist's bots. Never waits.
    fn clanker_match(&self) -> Option<(String, live_proto::hopper::clankers::ClankerHopper)> {
        None
    }
}

impl Rosters for live_hle::Hle {
    fn session(&self) -> Option<String> {
        self.bot_roster_hosted_session()
    }
    fn playlist(&self) -> Option<String> {
        let hosted = self.bot_roster_hosted_session()?;
        self.matchmade()
            .filter(|(_, session)| *session == hosted)
            .map(|(playlist, _)| playlist.to_string())
            // A Clankermatch has no server match: its playlist is the
            // clanker hopper it hosts (docs/game/bots.md §8.4 item 2), so
            // the server matches the bots to the party's level there.
            .or_else(|| {
                self.clanker_hosted()
                    .filter(|(session, _)| *session == hosted)
                    .map(|(_, mark)| mark.hopper_id.to_string())
            })
    }
    fn ask(
        &self,
        request: live_proto::bots::RosterRequest,
        done: Box<dyn FnOnce(Result<live_proto::bots::Roster, String>) + Send>,
    ) -> Result<(), String> {
        self.roster_bots(request, done)
    }
    fn players(&self, session: Option<String>, bots: Vec<live_proto::Xuid>) {
        self.set_session_players(session.as_deref(), bots)
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
    /// The people in the party ([`Driver::set_party`]): the bots fill the
    /// playlist's most players less these. Followed until the first bot
    /// sits, then kept.
    humans: usize,
    /// The bots the playlist holds besides the party.
    bots: usize,
    /// The schedule's seed (from the session id).
    seed: u64,
    /// The playlist's teams (0 free for all).
    teams: u32,
    schedule: Vec<std::time::Duration>,
    started: std::time::Instant,
    /// How many are in now.
    seated: usize,
    /// When this console stopped hosting a session (a handoff, or the
    /// player left): the fill ends [`FILL_GONE`] after.
    gone: Option<std::time::Instant>,
    /// This console has served the match's game ([`Driver::set_hosts_game`]):
    /// from then on the match session it still holds
    /// ([`Driver::set_holds_match`]) no longer keeps the bots, so they go
    /// [`FILL_GONE`] after the game.
    played: bool,
    /// The host player's team ([`Driver::set_host_team`]), followed until
    /// the first bot sits, then kept: a bot's team does not change.
    host_team: Option<u8>,
    /// The hopper's mark says it ranks its games.
    ranked: bool,
}

/// A Clankermatch ends this long after its console last hosted a session:
/// longer than a handoff's gap between the lobby's session and the game's
/// (a few seconds, §9.5), short enough that leaving ends it.
const FILL_GONE: std::time::Duration = std::time::Duration::from_secs(30);

/// A Clankermatch lobby whose bots are all due and whose game has not
/// started says why this long after the last bot's seat
/// ([`Driver::stalled`]): the hopper's start wait at its most players is
/// none, so a full lobby starts well inside it.
const STALL_AFTER: std::time::Duration = std::time::Duration::from_secs(20);
/// The same reason is said again this often while it holds.
const STALL_REPEAT: std::time::Duration = std::time::Duration::from_secs(30);

/// What [`Driver::stalled`] said last.
#[derive(Default)]
struct StallWatch {
    said: Option<(String, std::time::Instant)>,
}

/// The fill's plan for a party of `humans` in `mark`'s playlist, for the
/// log: the seats, the party, the bots.
fn fill_line(
    mark: &live_proto::hopper::clankers::ClankerHopper,
    humans: usize,
    bots: usize,
) -> String {
    let people = if humans == 1 { "person" } else { "people" };
    if bots == 0 {
        format!(
            "clankermatch: the party of {humans} {people} fills clanker playlist {}'s {} seats: no bots",
            mark.hopper_id, mark.players
        )
    } else {
        format!(
            "clankermatch: {} seats, the party of {humans} {people}, {bots} bots",
            mark.players
        )
    }
}

/// What the driver seats: the page's bots, or a Clankermatch's. Every bot
/// is a phantom peer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seating {
    /// Bots wanted in all (the roster's count).
    count: usize,
    /// Of those, how many are in now.
    seated: usize,
    /// A Clankermatch: nobody is seated before its roster comes (or is
    /// refused).
    fill: bool,
    /// The teams to balance the phantoms over (0: none).
    teams: u32,
    /// The roster's skill: the page's difficulty's, or none for a
    /// Clankermatch (the server sets it from the human's level, §8.2.3).
    skill: Option<f32>,
    /// The host player's team, the bots balance around (0 when unknown or
    /// not one of the game's teams).
    host_team: u8,
    /// The people the bots balance around, all on `host_team` (a party
    /// keeps together in a team playlist).
    humans: usize,
    /// The hopper ranks its games: its phantoms' lobby rows show a rank.
    ranked: bool,
}

impl Seating {
    /// The seats for `want`, or `fill`'s while a Clankermatch fills; the
    /// page's bots only where the lobby `admits` them ([`admits_page`]).
    fn of(want: Want, fill: Option<&Fill>, admits: bool) -> Seating {
        let count = if admits { want.count } else { 0 };
        match fill {
            Some(f) => Seating {
                count: f.bots,
                seated: f.seated.min(f.bots),
                fill: true,
                teams: f.teams,
                skill: None,
                host_team: f.host_team.filter(|t| u32::from(*t) < f.teams).unwrap_or(0),
                humans: f.humans.max(1),
                ranked: f.ranked,
            },
            None => Seating {
                count,
                seated: count,
                fill: false,
                teams: 0,
                skill: request_skill(want.difficulty),
                host_team: 0,
                humans: 1,
                ranked: false,
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
    /// One atomic sample of both combat choices.
    style: Arc<AtomicU8>,
    /// The page's Glitches (a [`GLITCHES`] value; shared by every
    /// Clanker of the driver).
    glitches: Arc<AtomicU8>,
}

impl PadSource for Clanker {
    fn combat_stats(&self) -> Option<&bots::stats::CombatStats> {
        Some(self.brain.combat_stats())
    }

    fn brain_view(&self) -> Option<bots::BrainView> {
        Some(self.brain.view())
    }

    fn reset_input(&mut self) {
        self.brain.reset_input();
    }

    fn order(&mut self, order: Option<bots::brain::Order>) -> bool {
        self.brain.set_order(order);
        true
    }

    // `superbounce.verify`'s trials on a Clanker seat hold its pad.
    fn hold_pad(&mut self, pad: Option<input::host::HostPadState>) -> bool {
        self.brain.hold_pad(pad)
    }

    fn phantom_latency(&self) -> bots::tuning::PhantomLatency {
        self.brain.phantom_latency()
    }

    fn pad(&mut self, f: &Frame<'_>) -> input::host::HostPadState {
        self.follow_page();
        self.brain.pad(f)
    }

    // The split sample (bots::think): the page is read with the guest, the
    // brain thinks wherever it is sent.
    fn observe(&mut self, f: &Frame<'_>) -> Sampled {
        self.follow_page();
        self.brain.observe(f)
    }

    fn think(&mut self) -> input::host::HostPadState {
        self.brain.think()
    }

    fn conclude(&mut self) {
        self.brain.conclude();
    }
}

impl Clanker {
    /// Apply current custom-game choices before sampling this seat.
    fn follow_page(&mut self) {
        let want = Skill::ALL[usize::from(self.skill.load(Ordering::Relaxed)).min(3)];
        self.brain.set_skill(want);
        let style = self.style.load(Ordering::Relaxed);
        let bias = match style & 3 {
            0 => -0.35,
            2 => 0.35,
            _ => 0.0,
        };
        self.brain.set_clanker_style(bias, style & TWITCHY_BIT != 0);
        self.brain.mind.emote.taunt = if self.taunts.load(Ordering::Relaxed) {
            bots::emote::DEFAULT_TAUNT
        } else {
            0.0
        };
        self.brain.mind.glitches =
            bots::glitches::Glitches::from_index(self.glitches.load(Ordering::Relaxed));
    }
}

/// The host's custom-game roster size (zero when off) and combat options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    pub count: usize,
    pub difficulty: i64,
    pub aggression: u8,
    pub twitchy: bool,
    pub taunts: bool,
    pub glitches: u8,
}

/// The low two bits hold the Aggression row. The next bit selects Twitchy.
const TWITCHY_BIT: u8 = 4;

impl Want {
    fn combat_style(self) -> u8 {
        self.aggression | if self.twitchy { TWITCHY_BIT } else { 0 }
    }

    /// No bots.
    pub const OFF: Want = Want {
        count: 0,
        difficulty: 1,
        aggression: 1,
        twitchy: false,
        taunts: true,
        glitches: 0,
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
        let aggression = match store.get(AGGRESSION.key) {
            Some(Value::Int(n)) => n.clamp(0, 2) as u8,
            _ => 1,
        };
        let twitchy = matches!(store.get(AIM_STYLE.key), Some(Value::Int(1)));
        let taunts = !matches!(store.get(TAUNTS.key), Some(Value::Bool(false)));
        let glitches = match store.get(GLITCHES.key) {
            Some(Value::Int(g)) => g.clamp(0, 2) as u8,
            _ => 0,
        };
        Want {
            count: if on { count } else { 0 },
            difficulty,
            aggression,
            twitchy,
            taunts,
            glitches,
        }
    }
}

/// Where one of the page's bots is: a phantom peer, the `n`th of the
/// page's bots (from 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seat {
    number: u32,
    handle: PhantomHandle,
}

/// One of the driver's bots.
struct Own {
    seat: Seat,
    skill: Arc<AtomicU8>,
    /// A phantom's xuid, once a roster gave it one (0 none).
    xuid: u64,
    /// A roster bot: its skill is the server's, not the page's.
    rostered: bool,
}

/// Keeps the page's bots: phantom peers, every one (docs/game/bots.md,
/// stage 6).
///
/// With Live ([`Driver::with_rosters`]) and a session this console hosts,
/// every bot is one of the server's bot accounts (§9.5): the driver asks
/// for a roster of the whole count (`POST /v1/games/bots`) and, once it
/// comes, the phantoms run its bots in place of the host-made
/// roster. It asks again when the count, the difficulty or the session
/// changes; a new roster replaces the old. Without Live, or until a
/// roster comes (or when none does), the phantoms are host-made: local
/// xuids, which no game record ever carries.
pub struct Driver {
    phantoms: Phantoms,
    local_names: Vec<String>,
    own: Mutex<Vec<Own>>,
    style: Arc<AtomicU8>,
    taunts: Arc<AtomicBool>,
    glitches: Arc<AtomicU8>,
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
    /// The people in the party of the match this console hosts, as
    /// [`frame`] last read it (`bots::lobby::party_players`).
    party: Mutex<Option<u32>>,
    /// The stalled Clankermatch's last line ([`Driver::stalled`]).
    stall: Mutex<StallWatch>,
    /// The lobby up takes the page's bots ([`admits_page`]); as [`frame`]
    /// last read it, and true before any lobby is read.
    admits: AtomicBool,
    /// This console runs a multiplayer game as its distributed server, as
    /// [`frame`] last read it: a Clankermatch's fill outlives the lobby's
    /// advertisement while it does ([`Driver::tick_match`]).
    hosts_game: AtomicBool,
    /// This console hosts a matchmade match's session, as [`frame`] last
    /// read it ([`Driver::set_holds_match`]).
    holds_match: AtomicBool,
}

/// Whether a lobby in retail mode `retail` takes the page's Clankers: a
/// custom game's lobby (the party lobby's CUSTOM GAMES, FORGE and
/// FIREFIGHT tabs), whose Game Setup has the Clanker Options row. A co-op
/// campaign lobby has no row and keeps its seats for people; a matchmaking
/// lobby (MULTIPLAYER) gets a Clanker playlist's own bots, from its roster
/// once its search hosts ([`Driver::tick_match`]), never the page's
/// (docs/game/party-lobby.md, "Clankers by tab").
pub fn admits_page(retail: crate::lobby_tabs::RetailMode) -> bool {
    retail == crate::lobby_tabs::RetailMode::Custom
}

/// The bot accounts among `own`: rostered phantoms whose players the game
/// made (`joined`).
fn session_bots_of(own: &[Own], joined: impl Fn(PhantomHandle) -> bool) -> Vec<live_proto::Xuid> {
    own.iter()
        .filter(|o| o.xuid != 0 && o.rostered && joined(o.seat.handle))
        .map(|o| live_proto::Xuid(o.xuid))
        .collect()
}

impl Driver {
    /// Phantom peers only.
    pub fn new(phantoms: Phantoms) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Driver {
            phantoms,
            local_names: live_proto::bots::names::Draw::new(seed).local_roster(),
            own: Mutex::new(Vec::new()),
            style: Arc::new(AtomicU8::new(Want::OFF.combat_style())),
            taunts: Arc::new(AtomicBool::new(true)),
            glitches: Arc::new(AtomicU8::new(0)),
            rosters: None,
            roster: Mutex::default(),
            last: Mutex::new(None),
            wants_roster: AtomicBool::new(false),
            me: std::sync::Weak::new(),
            reported: AtomicBool::new(false),
            fill: Mutex::new(None),
            host_team: Mutex::new(None),
            party: Mutex::new(None),
            stall: Mutex::default(),
            admits: AtomicBool::new(true),
            hosts_game: AtomicBool::new(false),
            holds_match: AtomicBool::new(false),
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
    /// §6): every Clanker goes out as a bot, each a phantom by its machine
    /// id. While a server roster is in force (its session, while
    /// this console still hosts it), the game names that session and each
    /// rostered phantom carries its rostered xuid; no other bot has one.
    /// `None` when there are no bots and no roster.
    pub fn bots_in_force(&self) -> Option<telemetry::Bots> {
        let session_id = self.roster_in_force();
        let own = self.own.lock().unwrap_or_else(|e| e.into_inner());
        let mut machines = Vec::new();
        for o in own.iter() {
            let xuid = if o.rostered && session_id.is_some() {
                o.xuid
            } else {
                0
            };
            machines.push((bots::phantom::machine_id(o.seat.handle), live_proto::Xuid(xuid)));
        }
        drop(own);
        // Phantoms someone else added (`sim.player.add`): bots, no xuid.
        for p in self.phantoms.list() {
            if !machines.iter().any(|(m, _)| *m == p.machine) {
                machines.push((p.machine, live_proto::Xuid(0)));
            }
        }
        if machines.is_empty() && session_id.is_none() {
            return None;
        }
        Some(telemetry::Bots {
            session_id,
            machines,
        })
    }

    /// The bot accounts in the hosted game while a server roster is in
    /// force: rostered phantoms with in-game players. Empty with no roster in force. The hosted Live
    /// session is told of them as players (§9.5), so it counts every
    /// player the game has.
    pub fn session_bots(&self) -> Vec<live_proto::Xuid> {
        if self.roster_in_force().is_none() {
            return Vec::new();
        }
        let joined: Vec<PhantomHandle> = self
            .phantoms
            .list()
            .into_iter()
            .filter(|p| p.state == bots::phantom::State::Joined)
            .map(|p| p.handle)
            .collect();
        let own = self.own.lock().unwrap_or_else(|e| e.into_inner());
        session_bots_of(&own, |h| joined.contains(&h))
    }

    /// Tells the hosted session its players ([`Driver::session_bots`]),
    /// while a roster is in force and once more after, so the bots it
    /// was told of leave it. The bots go to the session their roster is
    /// for ([`Driver::roster_in_force`]) and no other: a Clankermatch
    /// host advertises its party's lobby session beside the match, and
    /// the server refuses a roster's bots anywhere else.
    fn report_players(&self) {
        let Some(rosters) = &self.rosters else {
            return;
        };
        let bots = self.session_bots();
        let had = self.reported.swap(!bots.is_empty(), Ordering::Relaxed);
        if had || !bots.is_empty() {
            let session = if bots.is_empty() {
                None
            } else {
                self.roster_in_force()
            };
            rosters.players(session, bots);
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
            self.admits.load(Ordering::Relaxed),
        );
        let mut own = self.own.lock().unwrap_or_else(|e| e.into_inner());
        let mut log = Vec::new();
        // A Clankermatch takes its skill and style from its roster.
        // Existing seats see a page edit on the next input sample.
        let style = if seating.fill { Want::OFF } else { want };
        self.style.store(style.combat_style(), Ordering::Relaxed);
        self.taunts.store(want.taunts, Ordering::Relaxed);
        self.glitches.store(want.glitches, Ordering::Relaxed);
        // Fewer: the last added go first.
        while own.len() > seating.seated {
            let o = own.pop().expect("more than wanted");
            self.take_off(o.seat, &mut log);
        }
        for o in own.iter().filter(|o| !o.rostered) {
            o.skill.store(
                skill_index(skill(want.difficulty, mixed_slot(o.seat.number))),
                Ordering::Relaxed,
            );
        }
        self.wants_roster
            .store(seating.count > 0, Ordering::Relaxed);
        // With Live, every bot is a server bot account.
        let roster = if seating.count > 0 {
            self.rostered(seating.count, seating.skill, seating.teams > 1, &mut log)
        } else {
            None
        }
        .map(Vec::into_iter);
        // A Clankermatch waits for its roster rather than seat host-made
        // bots that the roster would replace; refused, it takes them.
        let hold = seating.fill
            && roster.is_none()
            && !self
                .roster
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .refused;
        let wanted = if hold { 0 } else { seating.seated };
        if wanted == 0 {
            return log;
        }
        // The phantoms: the server's roster's bots, else host-made ones.
        let mut seats: Vec<(RosterSeat, bool)> = match roster {
            Some(r) => r.map(|s| (s, true)).collect(),
            None => self.local_names.iter().take(seating.count).enumerate()
                .map(|(index, name)| {
                    let n = index as u32 + 1;
                    let k = skill(want.difficulty, mixed_slot(n));
                    (
                        RosterSeat {
                            identity: Identity {
                                xuid: live_proto::bots::local_xuid(n).0,
                                gamertag: name.clone(),
                            },
                            team: None,
                            skill: k,
                            seed: 0xc1a4_0000 ^ u64::from(n),
                            level: None,
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
            let mut fixed = vec![(seating.host_team, mean); seating.humans];
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
        let keep = own.len() <= seats.len()
            && own.iter().zip(&seats).all(|(have, (seat, rostered))| {
                have.xuid == seat.identity.xuid && have.rostered == *rostered
            });
        if !keep {
            while let Some(bot) = own.pop() {
                self.take_off(bot.seat, &mut log);
            }
        }
        let start = own.len();
        for (seat, rostered) in seats.into_iter().skip(start) {
            let n = own.len() as u32 + 1;
            let (src, s) = self.clanker(seat.skill, seat.seed, &seat.identity.gamertag);
            let name = format!(
                "{} (phantom{})",
                seat.identity.gamertag,
                if rostered { ", rostered" } else { "" }
            );
            let xuid = seat.identity.xuid;
            // A ranked hopper's lobby shows a rank for each: Live's level
            // for it, else one from its name and difficulty.
            let rank = seating.ranked.then(|| {
                let level = seat
                    .level
                    .unwrap_or_else(|| starting_level(&seat.identity.gamertag, seat.skill));
                bots::phantom::session::Rank::of_level(level)
            });
            match self.phantoms.add(seat.identity, seat.team, Box::new(src)) {
                Ok(h) => {
                    let _ = self.phantoms.set_rank(h, rank);
                    log.push(format!("{name} on ({:?})", seat.skill));
                    own.push(Own {
                        seat: Seat { number: n, handle: h },
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

    fn clanker(&self, k: Skill, seed: u64, name: &str) -> (Clanker, Arc<AtomicU8>) {
        let s = Arc::new(AtomicU8::new(skill_index(k)));
        let c = Clanker {
            brain: bots::Brain::new(k, seed).with_name_profile(name),
            skill: Arc::clone(&s),
            taunts: Arc::clone(&self.taunts),
            style: Arc::clone(&self.style),
            glitches: Arc::clone(&self.glitches),
        };
        (c, s)
    }

    fn take_off(&self, seat: Seat, log: &mut Vec<String>) {
        let name = self
            .phantoms
            .list()
            .into_iter()
            .find(|bot| bot.handle == seat.handle)
            .map_or_else(|| self.local_names[(seat.number - 1) as usize].clone(), |bot| bot.identity.gamertag);
        match self.phantoms.remove(seat.handle) {
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

    /// A Clankermatch lobby that has not started [`STALL_AFTER`] after
    /// its last bot was due: one line saying why, again when the reason
    /// changes or every [`STALL_REPEAT`] while it holds. `read` reads the
    /// lobby (`bots::lobby::match_wait`): `None` when there is nothing to
    /// wait for (the countdown or the game runs), else the reason and the
    /// whole line. Called only once the wait is long enough.
    pub fn stalled(
        &self,
        now: std::time::Instant,
        read: impl FnOnce() -> Option<(String, String)>,
    ) -> Option<String> {
        let due = self
            .fill
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|f| f.gone.is_none() && f.seated >= f.bots)
            .map(|f| (f.hopper, f.started + f.schedule.last().copied().unwrap_or_default()));
        let mut watch = self.stall.lock().unwrap_or_else(|e| e.into_inner());
        let Some((hopper, due)) = due else {
            *watch = StallWatch::default();
            return None;
        };
        let waited = now.saturating_duration_since(due);
        if waited < STALL_AFTER {
            return None;
        }
        let Some((reason, line)) = read() else {
            watch.said = None;
            return None;
        };
        // No bot sits while the fill waits for its roster: say that first.
        let (reason, line) = {
            let r = self.roster.lock().unwrap_or_else(|e| e.into_inner());
            if r.asked.is_some() && r.roster.is_none() && !r.refused {
                let why = "Live has not answered the bots' roster (no bot is seated until it \
                           answers or refuses; see the \"clankers: asking Live\" and \"live: roster:\" lines)";
                (format!("{why}; {reason}"), format!("{why}; {line}"))
            } else {
                (reason, line)
            }
        };
        if watch
            .said
            .as_ref()
            .is_some_and(|(r, at)| *r == reason && now.saturating_duration_since(*at) < STALL_REPEAT)
        {
            return None;
        }
        watch.said = Some((reason, now));
        Some(format!(
            "clankermatch: clanker playlist {hopper}'s match has not started {:.0} s after its last bot was due: {line}",
            waited.as_secs_f32()
        ))
    }

    /// The people in the party of the matchmaking lobby this console
    /// hosts (the squad session's players), or `None` while it cannot be
    /// read: a Clankermatch's bots fill the seats the party does not.
    pub fn set_party(&self, humans: Option<u32>) {
        *self.party.lock().unwrap_or_else(|e| e.into_inner()) = humans;
    }

    /// Whether this console runs a multiplayer game as its distributed
    /// server: the game deletes a matchmaking lobby's advertisement once
    /// its match session takes over, so a Clankermatch's fill holds while
    /// the game runs and ends [`FILL_GONE`] after it.
    pub fn set_hosts_game(&self, hosts: bool) {
        self.hosts_game.store(hosts, Ordering::Relaxed);
    }

    /// Whether this console hosts a matchmade match's session, in any
    /// state (`bots::phantom::session::hosts_match`): from the lobby's
    /// advertisement going until the game is first served, through
    /// arbitration and the session's state 7, a Clankermatch's fill holds
    /// while it does. After the game it no longer counts.
    pub fn set_holds_match(&self, holds: bool) {
        self.holds_match.store(holds, Ordering::Relaxed);
    }

    /// The lobby up, in retail mode `retail`: the page's bots stand down in
    /// a lobby that does not take them ([`admits_page`]) and come back in
    /// one that does, the last [`Want`] applied again on a change. Lines
    /// for the log.
    pub fn set_lobby(&self, retail: crate::lobby_tabs::RetailMode) -> Vec<String> {
        let admits = admits_page(retail);
        if self.admits.swap(admits, Ordering::Relaxed) == admits {
            return Vec::new();
        }
        let mut log = vec![if admits {
            "a custom game's lobby: the Clanker Options bots are seated".to_owned()
        } else {
            format!(
                "a {} lobby: the Clanker Options bots stand down",
                match retail {
                    crate::lobby_tabs::RetailMode::CoOp => "co-op campaign",
                    _ => "matchmaking",
                }
            )
        }];
        let want = *self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(want) = want {
            log.extend(self.apply(want));
        }
        log
    }

    /// Follows the Clankermatch this console hosts, at `now`: a clanker
    /// hopper's lobby starts a fill, its bots join on their schedule, and
    /// it ends when the console hosts another kind of session, or none and
    /// no game ([`Driver::set_hosts_game`]) for [`FILL_GONE`] (before the
    /// game, the match session it hosts, [`Driver::set_holds_match`],
    /// counts as the game). Applies the last [`Want`] again when that changes
    /// what is seated. Lines for the log.
    pub fn tick_match(&self, now: std::time::Instant) -> Vec<String> {
        let Some(rosters) = &self.rosters else {
            return Vec::new();
        };
        let hosted = rosters.clanker_match();
        let hosting = hosted.is_some() || rosters.session().is_some();
        let host_team = *self.host_team.lock().unwrap_or_else(|e| e.into_inner());
        // One human when the party cannot be read.
        let party = *self.party.lock().unwrap_or_else(|e| e.into_inner());
        let humans = party.map_or(1, |n| n.max(1)) as usize;
        let mut log = Vec::new();
        let changed = {
            let mut fill = self.fill.lock().unwrap_or_else(|e| e.into_inner());
            let current = fill.as_ref().map(|f| f.hopper);
            match (current, hosted) {
                (Some(h), Some((_, mark))) if h == mark.hopper_id => {
                    let f = fill.as_mut().expect("a fill");
                    f.gone = None;
                    f.played |= self.hosts_game.load(Ordering::Relaxed);
                    let mut replanned = false;
                    if f.seated == 0 {
                        f.host_team = host_team;
                        // The party read late or changed before any bot
                        // sat: the bots are the seats it leaves. Only a
                        // party read counts (out of the lobby, in the
                        // game, it reads as none).
                        if party.is_some() && humans != f.humans {
                            let bots = mark.bots_for(humans as u32) as usize;
                            log.push(fill_line(&mark, humans, bots));
                            f.humans = humans;
                            f.bots = bots;
                            f.schedule = crate::clankermatch::schedule(bots, f.seed);
                            replanned = true;
                        }
                    }
                    let n = crate::clankermatch::seated(
                        &f.schedule,
                        now.saturating_duration_since(f.started),
                    );
                    let more = n != f.seated;
                    f.seated = n;
                    more || replanned
                }
                (_, Some((session, mark))) => {
                    let bots = mark.bots_for(humans as u32) as usize;
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
                    log.push(fill_line(&mark, humans, bots));
                    // Every bot a phantom (docs/game/bots.md §8.4,
                    // "Phantoms only"): they join the match's target
                    // session as peers, never the party, and count toward
                    // the playlist's fewest players, so the game starts it.
                    log.push(format!(
                        "clankermatch: {bots} phantoms join the lobby as peers \
                         (\"bots: phantom ... joins\"), in the match only"
                    ));
                    *fill = Some(Fill {
                        hopper: mark.hopper_id,
                        humans,
                        bots,
                        seed,
                        teams: mark.teams,
                        schedule,
                        started: now,
                        seated: 0,
                        gone: None,
                        played: false,
                        host_team,
                        ranked: mark.ranked,
                    });
                    true
                }
                (Some(_), None) if hosting => {
                    log.push("clankermatch: this console hosts another game: the bots go".into());
                    *fill = None;
                    true
                }
                (Some(_), None) if self.hosts_game.load(Ordering::Relaxed) => {
                    // The game deleted the lobby's advertisement when its
                    // match session took over: the match runs on.
                    let f = fill.as_mut().expect("a fill");
                    f.gone = None;
                    f.played = true;
                    false
                }
                (Some(_), None)
                    if self.holds_match.load(Ordering::Relaxed)
                        && fill.as_ref().is_some_and(|f| !f.played) =>
                {
                    // Arbitration and the handoff into the match: the
                    // advertisement is gone and the game not yet served,
                    // its session out of state 5 (7) while the map loads,
                    // but this console still hosts the match and the
                    // phantoms are its peers. However long that takes,
                    // the bots stay for the game.
                    fill.as_mut().expect("a fill").gone = None;
                    false
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
        // While a fill runs, its phantoms join the match alone: never the
        // party's lobby before the search or after the game. Set before
        // any phantom is seated, and cleared only once they are gone.
        let filling = self.filling();
        if filling {
            self.phantoms.set_match_only(true);
        }
        if changed {
            let want = *self.last.lock().unwrap_or_else(|e| e.into_inner());
            log.extend(self.apply(want.unwrap_or(Want::OFF)));
        }
        if !filling {
            self.phantoms.set_match_only(false);
        }
        log
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

/// The Mixed-difficulty slot a seat follows: the `n`th bot's, 1 to 3 in
/// turn.
fn mixed_slot(number: u32) -> u8 {
    ((number - 1) % 3 + 1) as u8
}

/// The driver [`install`] started, for [`frame`] and telemetry.
static DRIVER: Mutex<Option<std::sync::Weak<Driver>>> = Mutex::new(None);

/// The settings store [`frame`] reads the Firefight options from, for
/// [`firefight_seat_teams`] (set with [`DRIVER`]).
static STORE: Mutex<Option<std::sync::Weak<Store>>> = Mutex::new(None);

/// The player teams new Clankers are seated on in a lobby in `mode`
/// ([`bots::firefight::set_seat_teams`]): Firefight's
/// ([`crate::firefight::options::Options::player_teams`], one when the
/// options cannot be read), none in any other mode. The phantom session
/// then puts each new Clanker on a player team, the one with fewer
/// players with two ([`bots::firefight::seat_team`]), never the AI's
/// team, which the Firefight host keeps clear of the player teams
/// (docs/game/firefight.md).
pub fn firefight_seat_teams(
    mode: crate::lobby_start::Mode,
    options: impl FnOnce() -> Result<crate::firefight::options::Options, String>,
) -> Option<u8> {
    (mode == crate::lobby_start::Mode::Firefight)
        .then(|| options().map_or(1, |o| o.player_teams()))
}

/// Once a frame, from the bots' frame hook: every second or so, asks
/// for a roster again if the hosted Live session changed
/// ([`Driver::refresh`]). Never waits on the server. `host_team` reads
/// the host player's team from the game, `party` the people in the party
/// of the matchmaking lobby this console hosts
/// (`bots::lobby::party_players`, [`Driver::set_party`]) and `lobby` the
/// lobby's retail mode (`None` while it cannot be read: left as it was,
/// [`Driver::set_lobby`]) and `hosts_game` whether this console runs a
/// multiplayer game as its distributed server ([`Driver::set_hosts_game`])
/// and `holds_match` whether it hosts a matchmade match's session
/// ([`Driver::set_holds_match`]); each is called twice a second. `match_wait`
/// reads why a Clankermatch lobby has not started, only once it has
/// waited long ([`Driver::stalled`]).
pub fn frame(
    n: u64,
    host_team: impl FnOnce() -> Option<u8>,
    party: impl FnOnce() -> Option<u32>,
    lobby: impl FnOnce() -> Option<crate::lobby_tabs::RetailMode>,
    hosts_game: impl FnOnce() -> bool,
    holds_match: impl FnOnce() -> bool,
    match_wait: impl FnOnce() -> Option<(String, String)>,
) {
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
    let store = STORE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|w| w.upgrade());
    bots::firefight::set_seat_teams(firefight_seat_teams(
        crate::lobby_start::state().mode,
        || match &store {
            Some(store) => crate::firefight::options::Options::read(store),
            None => Err("no settings store".into()),
        },
    ));
    d.set_host_team(host_team());
    d.set_party(party());
    d.set_hosts_game(hosts_game());
    d.set_holds_match(holds_match());
    if let Some(retail) = lobby() {
        for line in d.set_lobby(retail) {
            eprintln!("clankers: {line}");
        }
    }
    let now = std::time::Instant::now();
    for line in d.tick_match(now) {
        eprintln!("clankers: {line}");
    }
    if let Some(line) = d.stalled(now, match_wait) {
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

/// The settings whose change the driver applies at once ([`drive`]), in
/// the lobby and in a game: every page row but Teams, which no code reads.
pub const LIVE_KEYS: [&str; 7] = [
    "clankers.enabled",
    "clankers.count",
    "clankers.difficulty",
    "clankers.aggression",
    "clankers.aim_style",
    "clankers.taunts",
    "clankers.glitches",
];

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
    for key in LIVE_KEYS {
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

impl hooks::Feature for SetupRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ROW_FEATURE,
            "The Clankers row on a custom game's Game Setup",
        )
        .doc("docs/game/bots.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use guest_layout::points::{
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
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            h1row::building_live(&live)
        }));
        Ok(())
    }
}

/// Adds the page, its texts, the Game Setup row and the driver. With
/// Live, the driver's phantoms are the server's bots. Telemetry marks every
/// bot (docs/game/bots.md §6, stage 6c).
pub fn install(
    registry: &mut hooks::Registry,
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
    // The row's picture and description on Game Setup: a Spartan over a
    // roster (an image of the set no retail row uses), and what the page
    // is for.
    let help = Arc::clone(&page);
    crate::game_setup_help::register(
        ROW_ACTION,
        crate::game_setup_help::image::ROSTER,
        Arc::new(move |m: &mut dyn guest_layout::memory::GuestMemory| {
            let id = help.row_help_id()?;
            help.write_texts(m).ok()?;
            Some(id)
        }),
    );
    registry
        .add(&SetupRow { page })
        .map_err(|e| anyhow::anyhow!("clankers row: {e}"))?;
    // The driver lives as long as the store's change callbacks hold it.
    let mut driver = Driver::new(bots::phantom::global().clone());
    if let Some(hle) = live {
        driver = driver.with_rosters(hle);
    }
    let d = drive(store, driver);
    *DRIVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(&d));
    *STORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(store));
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
