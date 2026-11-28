//! The Firefight HUD (docs/game/firefight.md, "The HUD"): the round, the
//! wave banner, the enemies left and the lives, from the mode blob alone.
//!
//! **The blob.** [`Mode`] is what the HUD needs of the director's state:
//! the stage, the round and wave with their totals, whether the wave is a
//! boss wave, the enemies left, the lives, and with two teams each team's
//! points and which roster slots are on the second. It encodes to at most
//! [`MAX_BLOB`] bytes, the actor channel's mode state (`State` and `Key`
//! records, docs/game/ai-networking.md), so a distributed client holding
//! nothing but those bytes draws what the host draws. The host builds it
//! after each tick ([`Mode::of`], published through [`slot`]); the
//! channel's capture sends it (`actor_net::host::Host::set_mode`) and a
//! client extracts the HUD portion in `actor_net::roles` before [`Mode::decode`].
//!
//! **The view.** [`view_on`] is a pure function of a [`Mode`], the viewing
//! player's roster slot, the [`Scope`], the [`layout::Screen`] and the
//! banner up: a match, or a wave test in a Forge session seen from the
//! biped or from the monitor (docs/game/forge-firefight-ui.md, "The
//! Firefight HUD", while editing). It reads no game memory and no clock,
//! so the host and a client agree by construction, and a test can show it.
//!
//! **The layout.** [`layout`] places the elements of the Firefight block
//! in a viewer's viewport from the blob alone, at the bottom right above
//! Halo 2's score plate, with the seven fixed skull slots, and
//! [`layout::check`] holds them off the weapon, grenades, tracker and plate
//! rectangles. [`banners`] reads what began between two blobs (set, round,
//! skull, wave, the ends) and queues it, three seconds each.
//!
//! **The HUD.** The HUD kit's `firefight` HUD
//! (`hud_kit_core::views::firefight`, `content/modes/huds/firefight.hud.yaml`)
//! draws it on Halo 2's own HUD: [`state_of`] turns the layout's elements
//! into the kit's `State` and [`view_on`] into its `Values`. [`install`]
//! registers the HUD and adds its feed, which each tick reads
//! [`current`] once and, for each local player, the layout for that
//! player's roster slot, scope and screen ([`Feed`], [`tick_all`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ::firefight::director::Phase;
use ::firefight::{MAX_TEAMS, Outcome, ScriptDirector, Skull};
use guest_layout::memory::{GuestMemory, MemFault};
use hud_kit_core::views::firefight::{self as kit, Firefight, LivesLook};

pub use hud_kit_core::model::{Style, Value, Values};
pub use hud_kit_core::views::firefight::{BANNER, State, Stage};

use crate::hud_kit::{self, Handle};

/// The blob's format.
pub const VERSION: u8 = 4;
/// The blob's bytes at most: the actor channel's mode state holds 63
/// (`actor_net::wire::MAX_STATE_BYTES`), and the script role's 12-byte
/// header rides in front of the blob (`actor_net::roles`).
pub const MAX_BLOB: usize = 51;
/// Players whose own lives the blob carries when lives are not shared.
pub const MAX_EACH: usize = 8;
/// The most rounds, and waves in a round, the blob admits.
pub const MAX_ROUNDS: u8 = 12;
pub const MAX_WAVES: u8 = 16;

/// The lives the HUD shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lives {
    /// The shared pool.
    Shared(u16),
    /// Each player's own count, by roster slot, in slot order.
    Each(Vec<(u8, u16)>),
    /// Each Firefight team's shared pool, with two teams.
    Teams([u16; MAX_TEAMS]),
}

/// What the blob carries of two teams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Teams {
    /// Each Firefight team's points.
    pub scores: [u32; MAX_TEAMS],
    /// The roster slots on the second team, one bit a slot; every other
    /// slot is on the first.
    pub second: u16,
}

impl Teams {
    /// The Firefight team of the player at roster slot `slot`.
    pub fn team_of(&self, slot: u8) -> usize {
        usize::from(slot < 16 && self.second & (1 << slot) != 0)
    }
}

/// What the HUD reads of the director: the mode blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    pub stage: Stage,
    /// Zero-based; the round and wave that are on or next.
    pub round: u8,
    pub rounds: u8,
    pub wave: u8,
    /// Waves in this round.
    pub waves: u8,
    pub boss: bool,
    /// Actors alive and still to be placed, while a wave is on; else 0.
    pub left: u16,
    /// The skulls on, one bit a session skull index ([`skull_mask`]).
    pub skulls: u32,
    /// The zero-based set; 0 in a finite definition.
    pub set: u8,
    /// The definition's sets go on until the players are out
    /// (`SetRules::endless`): the progress line names the set.
    pub endless: bool,
    /// Ticks left of the bonus round; 0 when none is on.
    pub bonus_left: u16,
    /// The wave's actors, placed and still to be placed, while a wave is
    /// on: the enemies meter's full length; in a bonus round the bonus
    /// round's length in ticks; else 0.
    pub total: u16,
    /// The team's points with one team; 0 with two (`teams` has theirs).
    pub score: u32,
    pub lives: Lives,
    /// Two teams' points and members; none with one team.
    pub teams: Option<Teams>,
}

pub mod banners;
pub mod layout;

/// Every skull Firefight turns on, by its session skull index: nine with an
/// engine effect, and Iron, the bonus round's director rule. Envy, Grunt
/// Birthday Party, IWHBYD, Blind, Sputnik and Bandana have no Halo 2 effect
/// a Firefight match can apply (docs/game/skulls-modes.md, "In Firefight"),
/// so none is in the set.
pub const FIREFIGHT_SKULLS: [Skull; 10] = [
    Skull::Assassins,
    Skull::Thunderstorm,
    Skull::Famine,
    Skull::BlackEye,
    Skull::Ghost,
    Skull::Catch,
    Skull::Iron,
    Skull::Mythic,
    Skull::Angry,
    Skull::LowGravity,
];

/// The bits a blob's skulls may have: [`FIREFIGHT_SKULLS`]'.
pub const SKULL_BITS: u32 = {
    let mut m = 0;
    let mut i = 0;
    while i < FIREFIGHT_SKULLS.len() {
        m |= 1 << FIREFIGHT_SKULLS[i] as u8;
        i += 1;
    }
    m
};

/// The bits of the skulls with an engine effect: [`SKULL_BITS`] without
/// Iron, which only the bonus round turns on. All of these on is "all
/// skulls on" (`banners::lines`).
pub const ENGINE_SKULL_BITS: u32 = SKULL_BITS & !(1 << Skull::Iron as u8);

/// The skulls in `on` as the blob's bits.
pub fn skull_mask<'a>(on: impl IntoIterator<Item = &'a Skull>) -> u32 {
    on.into_iter().fold(0, |m, s| m | 1 << *s as u8)
}

/// Why a blob could not be made or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Lives are not shared and more players hold lives than the blob has
    /// room for.
    TooManyPlayers(usize),
    /// A player's roster slot is past the 16 the team bits hold.
    Slot(u8),
    /// The format byte is not [`VERSION`].
    Version(u8),
    /// The blob ends early, runs on, or is over [`MAX_BLOB`].
    Length { len: usize, want: usize },
    /// A field holds a value it cannot.
    Field(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TooManyPlayers(n) => write!(
                f,
                "{n} players hold their own lives; the mode blob carries {MAX_EACH}"
            ),
            Error::Slot(slot) => write!(
                f,
                "roster slot {slot} is past the mode blob's 16 team bits"
            ),
            Error::Version(v) => write!(f, "mode blob version {v}, this build reads {VERSION}"),
            Error::Length { len, want } => {
                write!(f, "mode blob is {len} bytes, its fields need {want}")
            }
            Error::Field(name) => write!(f, "mode blob field {name} is out of range"),
        }
    }
}

impl std::error::Error for Error {}

/// Bytes before the teams and lives: version, stage, round, rounds, wave,
/// waves, flags, left (2), the skulls (4), the set, the bonus ticks left
/// (2), the total (2), the team count.
const HEAD: usize = 19;
/// Where the team count is.
const TEAM_COUNT: usize = HEAD - 1;
/// The two teams' part, when the team count is 2: each team's points (4
/// each) and the second team's slot bits (2).
const TEAMS: usize = 4 * MAX_TEAMS + 2;
const FLAG_BOSS: u8 = 1;
const FLAG_ENDLESS: u8 = 2;
const LIVES_SHARED: u8 = 0;
const LIVES_EACH: u8 = 1;
const LIVES_TEAMS: u8 = 2;

impl Mode {
    /// The blob for the director's state on the host. `players` is the
    /// player count the reserve is measured at, `finished` the outcome of
    /// the last `Finish` in the host log, if any.
    pub fn of(
        director: &ScriptDirector,
        players: u8,
        finished: Option<Outcome>,
    ) -> Result<Mode, Error> {
        let (state, def) = (director.state(), director.def());
        let two = def.teams > 1;
        let stage = match (finished, state.phase) {
            (Some(Outcome::Won), _) => Stage::Won,
            (Some(Outcome::Lost), _) => Stage::Lost,
            (Some(Outcome::TeamWon { team }), _) => Stage::team_won(team),
            (Some(Outcome::Draw), _) => Stage::Draw,
            (None, Phase::Warmup { .. }) => Stage::Warmup,
            (None, Phase::Spawning) => Stage::Spawning,
            (None, Phase::Fighting) => Stage::Fighting,
            (None, Phase::Cleanup { .. }) => Stage::Cleanup,
            (None, Phase::Intermission { .. }) => Stage::Intermission,
            (None, Phase::Bonus { .. }) => Stage::Fighting,
            (None, Phase::Done) => Stage::Over,
        };
        let round = def.rounds.get(usize::from(state.round));
        let lives = if def.lives.shared && two {
            Lives::Teams(state.pools)
        } else if def.lives.shared {
            Lives::Shared(state.pools[0])
        } else {
            if state.players.len() > MAX_EACH {
                return Err(Error::TooManyPlayers(state.players.len()));
            }
            if let Some(&slot) = state.players.keys().find(|&&s| s >= 16) {
                return Err(Error::Slot(slot));
            }
            Lives::Each(state.players.iter().map(|(&s, p)| (s, p.lives)).collect())
        };
        let teams = if two {
            let mut second = 0u16;
            for (&slot, player) in &state.players {
                if player.team == 0 {
                    continue;
                }
                if slot >= 16 {
                    return Err(Error::Slot(slot));
                }
                second |= 1 << slot;
            }
            Some(Teams {
                scores: state.scores,
                second,
            })
        } else {
            None
        };
        Ok(Mode {
            stage,
            round: state.round,
            rounds: def.rounds.len().min(usize::from(u8::MAX)) as u8,
            wave: state.wave,
            waves: round.map_or(0, |r| r.waves.len().min(usize::from(u8::MAX)) as u8),
            boss: round
                .and_then(|r| r.waves.get(usize::from(state.wave)))
                .is_some_and(|w| w.boss),
            left: if stage.wave_on() {
                director
                    .live_actors()
                    .saturating_add(director.reserve(players))
            } else {
                0
            },
            skulls: skull_mask(&state.skulls),
            set: state.set,
            endless: def.set.endless,
            // The director's bonus round (`ff_bonus`) fills these when the
            // host runs it; until then no bonus round is on.
            bonus_left: 0,
            total: if stage.wave_on() {
                state.wave_actors.saturating_add(director.reserve(players))
            } else {
                0
            },
            score: if two { 0 } else { state.scores[0] },
            lives,
            teams,
        })
    }

    /// The blob's bytes, at most [`MAX_BLOB`].
    pub fn encode(&self) -> Vec<u8> {
        let flags = (u8::from(self.boss) * FLAG_BOSS) | (u8::from(self.endless) * FLAG_ENDLESS);
        let mut out = vec![
            VERSION,
            self.stage as u8,
            self.round,
            self.rounds,
            self.wave,
            self.waves,
            flags,
        ];
        out.extend_from_slice(&self.left.to_le_bytes());
        out.extend_from_slice(&self.skulls.to_le_bytes());
        out.push(self.set);
        out.extend_from_slice(&self.bonus_left.to_le_bytes());
        out.extend_from_slice(&self.total.to_le_bytes());
        match &self.teams {
            None => {
                out.push(1);
                out.extend_from_slice(&self.score.to_le_bytes());
            }
            Some(teams) => {
                out.push(MAX_TEAMS as u8);
                for score in teams.scores {
                    out.extend_from_slice(&score.to_le_bytes());
                }
                out.extend_from_slice(&teams.second.to_le_bytes());
            }
        }
        match &self.lives {
            Lives::Shared(pool) => {
                out.push(LIVES_SHARED);
                out.extend_from_slice(&pool.to_le_bytes());
            }
            Lives::Teams(pools) => {
                out.push(LIVES_TEAMS);
                for pool in pools {
                    out.extend_from_slice(&pool.to_le_bytes());
                }
            }
            Lives::Each(each) => {
                // The slots as bits (slots are under 16, [`Error::Slot`]),
                // then each one's count in slot order.
                out.push(LIVES_EACH);
                let slots = each.iter().fold(0u16, |m, &(player, _)| m | 1 << (player & 15));
                out.extend_from_slice(&slots.to_le_bytes());
                for &(_, count) in each {
                    out.extend_from_slice(&count.to_le_bytes());
                }
            }
        }
        out
    }

    /// Reads a blob, refusing one it does not understand: a wrong version,
    /// a wrong length, a field out of range, or a part of two teams' (their
    /// stages, their pools) in a blob of one.
    pub fn decode(bytes: &[u8]) -> Result<Mode, Error> {
        if bytes.len() > MAX_BLOB {
            return Err(Error::Length {
                len: bytes.len(),
                want: MAX_BLOB,
            });
        }
        let short = |want: usize| Error::Length {
            len: bytes.len(),
            want,
        };
        if bytes.len() < HEAD {
            return Err(short(HEAD));
        }
        if bytes[0] != VERSION {
            return Err(Error::Version(bytes[0]));
        }
        let stage = Stage::from_code(bytes[1]).ok_or(Error::Field("stage"))?;
        let (round, rounds, wave, waves) = (bytes[2], bytes[3], bytes[4], bytes[5]);
        if rounds == 0 || rounds > MAX_ROUNDS || round >= rounds {
            return Err(Error::Field("round"));
        }
        if waves == 0 || waves > MAX_WAVES || wave >= waves {
            return Err(Error::Field("wave"));
        }
        if bytes[6] & !(FLAG_BOSS | FLAG_ENDLESS) != 0 {
            return Err(Error::Field("flags"));
        }
        let left = u16::from_le_bytes([bytes[7], bytes[8]]);
        let skulls = u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]);
        if skulls & !SKULL_BITS != 0 {
            return Err(Error::Field("skulls"));
        }
        let endless = bytes[6] & FLAG_ENDLESS != 0;
        let set = bytes[13];
        if set != 0 && !endless {
            return Err(Error::Field("set"));
        }
        let bonus_left = u16::from_le_bytes([bytes[14], bytes[15]]);
        let total = u16::from_le_bytes([bytes[16], bytes[17]]);
        let mut score = 0;
        let (teams, at) = match bytes[TEAM_COUNT] {
            1 => {
                let end = HEAD + 4;
                if bytes.len() < end {
                    return Err(short(end));
                }
                score = u32::from_le_bytes([bytes[HEAD], bytes[HEAD + 1], bytes[HEAD + 2], bytes[HEAD + 3]]);
                (None, end)
            }
            2 => {
                let end = HEAD + TEAMS;
                if bytes.len() < end {
                    return Err(short(end));
                }
                let part = &bytes[HEAD..end];
                let score = |i: usize| {
                    u32::from_le_bytes([part[4 * i], part[4 * i + 1], part[4 * i + 2], part[4 * i + 3]])
                };
                let teams = Teams {
                    scores: std::array::from_fn(score),
                    second: u16::from_le_bytes([part[TEAMS - 2], part[TEAMS - 1]]),
                };
                (Some(teams), end)
            }
            _ => return Err(Error::Field("teams")),
        };
        if teams.is_none() && stage.two_teams_only() {
            return Err(Error::Field("stage"));
        }
        if teams.is_some() && endless {
            return Err(Error::Field("flags"));
        }
        let kind = *bytes.get(at).ok_or_else(|| short(at + 1))?;
        let at = at + 1;
        let (lives, want) = match kind {
            LIVES_SHARED if teams.is_none() => {
                if bytes.len() < at + 2 {
                    return Err(short(at + 2));
                }
                (
                    Lives::Shared(u16::from_le_bytes([bytes[at], bytes[at + 1]])),
                    at + 2,
                )
            }
            LIVES_TEAMS if teams.is_some() => {
                let want = at + 2 * MAX_TEAMS;
                if bytes.len() < want {
                    return Err(short(want));
                }
                let pool = |i: usize| u16::from_le_bytes([bytes[at + 2 * i], bytes[at + 2 * i + 1]]);
                (Lives::Teams(std::array::from_fn(pool)), want)
            }
            LIVES_EACH => {
                if bytes.len() < at + 2 {
                    return Err(short(at + 2));
                }
                let slots = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                let n = slots.count_ones() as usize;
                if n > MAX_EACH {
                    return Err(Error::Field("lives"));
                }
                let want = at + 2 + 2 * n;
                if bytes.len() < want {
                    return Err(short(want));
                }
                let each = (0..16u8)
                    .filter(|slot| slots & (1 << slot) != 0)
                    .zip(bytes[at + 2..want].chunks_exact(2))
                    .map(|(slot, c)| (slot, u16::from_le_bytes([c[0], c[1]])))
                    .collect::<Vec<_>>();
                (Lives::Each(each), want)
            }
            _ => return Err(Error::Field("lives")),
        };
        if bytes.len() != want {
            return Err(short(want));
        }
        Ok(Mode {
            stage,
            round,
            rounds,
            wave,
            waves,
            boss: bytes[6] & FLAG_BOSS != 0,
            left,
            skulls,
            set,
            endless,
            bonus_left,
            total,
            score,
            lives,
            teams,
        })
    }
}

// ---- the published blob ----

/// The latest blob the host runtime made, for the actor channel's capture.
#[derive(Default)]
pub struct Slot(Mutex<Option<Vec<u8>>>);

impl Slot {
    pub const fn new() -> Slot {
        Slot(Mutex::new(None))
    }
    /// Sets the blob; none when no mode runs.
    pub fn set(&self, blob: Option<Vec<u8>>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = blob;
    }
    pub fn get(&self) -> Option<Vec<u8>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

static SLOT: Slot = Slot::new();
static RECEIVED: Slot = Slot::new();

/// The process's published blob: set by the host runtime after each tick,
/// read by the actor channel's capture.
pub fn slot() -> &'static Slot {
    &SLOT
}

/// The blob a distributed client last received over the actor channel, set
/// by its receive path (`actor_net::client::install`).
pub fn received() -> &'static Slot {
    &RECEIVED
}

/// What the HUD shows on this machine for the player at roster slot
/// The mode now: the host's own blob or else the one received, decoded.
/// None while no mode runs; an error is a blob this build cannot read.
pub fn current() -> Option<Result<Mode, Error>> {
    let blob = slot().get().or_else(|| received().get())?;
    Some(Mode::decode(&blob))
}

// ---- a wave test ----

/// What the director runs is a wave test in a Forge session
/// (`super::runtime::Runtime::start_test`), not a match.
static TESTING: AtomicBool = AtomicBool::new(false);

/// The runtime says a wave test runs, or no longer does.
pub fn set_testing(on: bool) {
    TESTING.store(on, Ordering::Relaxed);
}

/// Whether a wave test runs on this machine.
pub fn testing() -> bool {
    TESTING.load(Ordering::Relaxed)
}

/// Who sees the blob, and in what.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// A match: every element as the blob says.
    #[default]
    Match,
    /// A wave test, from a player's biped: no lives (a test charges none).
    Biped,
    /// A wave test, from the Forge monitor: the banner and the notes; the
    /// monitor HUD's own pill counts the enemies.
    Monitor,
}

impl Scope {
    /// The scope of a local user: a match with no test; in a test the
    /// monitor for a user in it, else the biped.
    pub fn of(testing: bool, in_monitor: bool) -> Scope {
        match (testing, in_monitor) {
            (false, _) => Scope::Match,
            (true, false) => Scope::Biped,
            (true, true) => Scope::Monitor,
        }
    }
}

/// The enemies left `mode` shows while a wave is on: the Forge monitor's
/// pill in a test (`hud_kit_core::views::forge::ENEMIES_COUNT`).
pub fn enemies_left(mode: &Mode) -> Option<u16> {
    mode.stage.wave_on().then_some(mode.left)
}

/// The enemies left of the running wave test, for the monitor HUD; none
/// with no test, between waves, or with no blob this build reads.
pub fn test_enemies() -> Option<u16> {
    if !testing() {
        return None;
    }
    let blob = slot().get()?;
    enemies_left(&Mode::decode(&blob).ok()?)
}

// ---- the view ----

/// The lives the viewer at roster slot `local` sees, if any: the pool, the
/// viewer's own count, or the viewer's team's pool.
fn lives_of(m: &Mode, local: u8) -> Option<u16> {
    match &m.lives {
        Lives::Shared(pool) => Some(*pool),
        Lives::Each(each) => each.iter().find(|(s, _)| *s == local).map(|&(_, n)| n),
        Lives::Teams(pools) => {
            let team = m.teams.map_or(0, |t| t.team_of(local));
            pools.get(team).copied()
        }
    }
}

/// The kit's state for the elements [`layout::layout`] made: each line's
/// words, the meter's fill, the skulls on, the banner's words, the set's
/// numeral and which round and wave pips show and are lit.
pub fn state_of(elements: &[layout::Element]) -> State {
    let mut out = State::default();
    for e in elements.iter().filter(|e| e.shown) {
        let words = || Some(e.text.clone());
        match e.id {
            layout::SCORE => out.score = words(),
            layout::LIVES => {
                let look = match (e.look, e.text.as_str()) {
                    (layout::Look::Warning, _) => LivesLook::Out,
                    (_, layout::LAST_LIFE) => LivesLook::Last,
                    _ => LivesLook::Normal,
                };
                out.lives = Some((e.text.clone(), look));
            }
            layout::PROGRESS => out.progress = words(),
            layout::ENEMIES => {
                let (now, total) = e.fill;
                out.enemies = Some(if total == 0 {
                    0.0
                } else {
                    f32::from(now) / f32::from(total)
                });
            }
            layout::BANNER => out.banner = words(),
            layout::SET => out.set = words(),
            id => {
                let at = |ids: &[&str]| ids.iter().position(|s| *s == id);
                if let Some(i) = at(&layout::SKULL_IDS) {
                    out.skulls[i] = true;
                } else if let Some(i) = at(&layout::ROUND_IDS) {
                    out.rounds[i] = Some(e.fill.0 > 0);
                } else if let Some(i) = at(&layout::WAVE_IDS) {
                    out.waves[i] = Some(e.fill.0 > 0);
                }
            }
        }
    }
    out
}

/// What the player at roster slot `local` sees of `mode` in `scope` on
/// `screen`, with `banner`'s words up if any: the kit's values. A pure
/// function of the blob, the slot, the scope, the screen and the banner:
/// the host and a client agree. A test shows no lives.
pub fn view_on(
    mode: &Mode,
    local: u8,
    scope: Scope,
    screen: layout::Screen,
    banner: Option<&str>,
) -> Values {
    kit::values(&state_of(&layout::layout(mode, local, scope, screen, banner)))
}

/// [`view_on`] at full size with the banner the match's end keeps up, if
/// it is over: the blob alone gives no banner of a set, round, skull or
/// wave, which are events ([`banners::events`]).
pub fn view(mode: &Mode, local: u8, scope: Scope) -> Values {
    let ending = mode.stage.over().then(|| banners::ending(mode.stage).text());
    view_on(mode, local, scope, layout::Screen::Full, ending.as_deref())
}

// ---- the feed ----

/// The feed's feature id in the plan.
pub const FEED: &str = "firefight-hud";

/// What the feed keeps from tick to tick for one local user: the last
/// blob, the banners it began ([`banners::Banners`]) and the last
/// unreadable blob's reason, logged once.
#[derive(Default)]
pub struct Feed {
    last: Option<Mode>,
    banners: banners::Banners,
    error: Option<String>,
    /// The announcer's clips the last tick began ([`banners::lines`]),
    /// until [`Feed::take_lines`].
    lines: Vec<&'static str>,
}

impl Feed {
    /// One tick: the values to publish for the player at roster slot
    /// `local` seeing `now`, the current blob, in `scope` on `screen`. No
    /// match, no local player and a blob this build cannot read hide every
    /// element and forget the last blob and the banners; the reason for an
    /// unreadable blob is logged when it changes. A wave test has no
    /// lives, so "LAST LIFE" is no banner in one.
    pub fn tick(
        &mut self,
        now: Option<Result<Mode, Error>>,
        local: u8,
        scope: Scope,
        screen: layout::Screen,
    ) -> Values {
        let mode = match now {
            Some(Ok(m)) => {
                self.error = None;
                Some(m)
            }
            Some(Err(e)) => {
                let why = e.to_string();
                if self.error.as_ref() != Some(&why) {
                    eprintln!("firefight hud: not drawn: {why}");
                }
                self.error = Some(why);
                None
            }
            None => None,
        };
        let Some(mode) = mode else {
            self.last = None;
            self.banners.clear();
            self.lines.clear();
            return kit::hidden();
        };
        let mut events = banners::events(self.last.as_ref(), &mode, local, skull_name);
        if scope != Scope::Match {
            events.retain(|b| *b != banners::Banner::LastLife);
        }
        self.lines = banners::lines(self.last.as_ref(), &mode, local, &events);
        let shown = self.banners.tick(events);
        let words = shown.map(|s| s.banner.text());
        let values = view_on(&mode, local, scope, screen, words.as_deref());
        self.last = Some(mode);
        values
    }
}

impl Feed {
    /// The announcer's clips the last tick began, once.
    pub fn take_lines(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.lines)
    }
}

/// Say the announcer's lines this tick began: one local user's, since
/// every user's feed reads the same blob.
pub fn announce(feeds: &mut Feeds) {
    let mut said = false;
    for feed in feeds.iter_mut() {
        let lines = feed.take_lines();
        if !said && !lines.is_empty() {
            said = true;
            lines.into_iter().for_each(crate::medal_announcer::announce_id);
        }
    }
}

/// A skull's name by its session skull index, as the skull menu and the
/// pickup message name it (`crate::skulls::SKULLS`).
pub fn skull_name(index: u8) -> Option<&'static str> {
    crate::skulls::SKULLS.get(usize::from(index)).map(|d| d.label)
}

/// The roster slot of each local user's player the game lists, by local
/// user (0 to 3).
pub fn local_slots(m: &dyn GuestMemory) -> Result<[Option<u8>; USERS], MemFault> {
    use guest_layout::view::players;
    let mut out = [None; USERS];
    let Some(array) = players::array(m)? else {
        return Ok(out);
    };
    if m.u32(array.array.addr.wrapping_add(guest_layout::datum::layout::DATA))? == 0 {
        return Ok(out);
    }
    for (handle, p) in array.live(m, 16)? {
        let user = p.local_user(m)?;
        if p.listed(m)? && (0..USERS as i16).contains(&user) {
            out[user as usize] = Some(handle.index() as u8);
        }
    }
    Ok(out)
}

/// Local users the HUD kit draws for.
const USERS: usize = hud_kit::USERS as usize;

/// The feeds of the local users, one each.
pub type Feeds = [Feed; USERS];

/// One tick for every local user: user `u`'s view of `now` at its roster
/// slot (`slots[u]`) in its scope (`scopes[u]`), on `screen`. A user with
/// no slot is hidden and its feed forgets what it kept.
pub fn tick_all(
    feeds: &mut Feeds,
    slots: &[Option<u8>; USERS],
    scopes: &[Scope; USERS],
    screen: layout::Screen,
    now: &Option<Result<Mode, Error>>,
) -> [Values; USERS] {
    let mut feeds = feeds.iter_mut();
    std::array::from_fn(|u| {
        let feed = feeds.next().expect("one feed a user");
        match slots[u] {
            Some(slot) => feed.tick(now.clone(), slot, scopes[u], screen),
            None => feed.tick(None, 0, scopes[u], screen),
        }
    })
}

/// Each local user's scope now: in a wave test, the Forge monitor's user
/// (`crate::forge_hud::USER`) sees it from the monitor while the monitor
/// is up, every other user from the biped.
pub fn scopes_now() -> [Scope; USERS] {
    let testing = testing();
    let monitor = crate::forge_controls::monitor();
    std::array::from_fn(|u| Scope::of(testing, monitor && u as u32 == crate::forge_hud::USER))
}

/// The feed on the plan: each tick, [`current`] once, and for each local
/// user the layout for its roster slot, scope ([`scopes_now`]) and screen
/// ([`layout::Screen::for_users`], from the local user count) through its
/// [`Feed`]: user 0's values are the HUD's shared ones, every other local
/// user's its own (`Handle::publish_for`), so each split-screen player
/// sees its own lives.
struct FeedFeature {
    handle: Handle,
    feeds: Arc<Mutex<Feeds>>,
}

impl hooks::Feature for FeedFeature {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEED, "Firefight HUD on the HUD kit")
            .doc("docs/game/firefight.md")
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Rebuilt from the mode blob each tick; the last blob and the
        // banner queue only word the next tick's banners.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let (handle, feeds) = (self.handle, Arc::clone(&self.feeds));
        b.frame().on_tick("firefight hud: values", move |o| {
            let guest = o.guest();
            let mem: &dyn GuestMemory = &*guest;
            let slots = local_slots(mem).unwrap_or_default();
            let screen = layout::Screen::for_users(hud_kit::local_users(mem).unwrap_or(1));
            let mut feeds = feeds.lock().unwrap_or_else(|e| e.into_inner());
            let all = tick_all(&mut feeds, &slots, &scopes_now(), screen, &current());
            announce(&mut feeds);
            drop(feeds);
            for (user, values) in all.into_iter().enumerate() {
                let user = user as u32;
                if user == 0 {
                    handle.publish(values);
                } else {
                    handle.publish_for(user, slots[user as usize].map(|_| values));
                }
            }
        });
        Ok(())
    }
}

/// Registers the Firefight HUD with the HUD kit, hidden, and adds the
/// feed that publishes it.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    let handle = hud_kit::register(Arc::new(Firefight));
    handle.publish(kit::hidden());
    registry
        .add(&FeedFeature {
            handle,
            feeds: Arc::new(Mutex::new(Default::default())),
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{FEED}: {e}"))
}

#[cfg(test)]
mod tests;
