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
//! **The view.** [`view`] is a pure function of a [`Mode`], the viewing
//! player's roster slot and the [`Scope`]: a match, or a wave test in a
//! Forge session seen from the biped or from the monitor
//! (docs/game/forge-firefight-ui.md, "The Firefight HUD", while editing).
//! It reads no game memory and no clock, so the host and a client agree by
//! construction, and a test can show it.
//!
//! **The HUD.** The HUD kit's `firefight` HUD
//! (`hud_kit_core::views::firefight`, docs/game/forge-firefight-ui.md,
//! "The Firefight HUD") draws it on Halo 2's own HUD: [`view_of`] turns a
//! blob into the kit's `View` for one viewer, [`view`] into its `Values`.
//! The kit's layout, its checks (`h2 hud preview firefight`) and its
//! runtime (`crate::hud_kit`) are the kit's. [`install`] registers the HUD
//! and adds its feed, which each tick publishes [`current`] for each local
//! player, its own values for each split-screen player, and sends each
//! the notes a change makes ([`Feed`], [`tick_all`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ::firefight::director::Phase;
use ::firefight::{MAX_TEAMS, Outcome, ScriptDirector, Skull};
use engine::memory::{GuestMemory, MemFault};
use hud_kit_core::views::firefight::{self as kit, Firefight};

pub use hud_kit_core::views::firefight::{BANNER, ENEMIES_COUNT, LIVES_COUNT, NOTE, Stage, View};
pub use hud_kit_core::model::{Style, Value, Values};

use crate::hud_kit::{self, Handle};

/// The blob's format.
pub const VERSION: u8 = 3;
/// The actor channel's mode state holds this many bytes at most.
pub const MAX_BLOB: usize = 63;
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
    pub lives: Lives,
    /// Two teams' points and members; none with one team.
    pub teams: Option<Teams>,
}

/// Every skull Firefight turns on, by its session skull index.
pub const FIREFIGHT_SKULLS: [Skull; 9] = [
    Skull::Assassins,
    Skull::Thunderstorm,
    Skull::Famine,
    Skull::Blind,
    Skull::Ghost,
    Skull::Catch,
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
/// waves, flags, left (2), the skulls (4), the team count.
const HEAD: usize = 14;
/// Where the team count is.
const TEAM_COUNT: usize = HEAD - 1;
/// The two teams' part, when the team count is 2: each team's points (4
/// each) and the second team's slot bits (2).
const TEAMS: usize = 4 * MAX_TEAMS + 2;
const FLAG_BOSS: u8 = 1;
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
            lives,
            teams,
        })
    }

    /// The blob's bytes, at most [`MAX_BLOB`].
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![
            VERSION,
            self.stage as u8,
            self.round,
            self.rounds,
            self.wave,
            self.waves,
            if self.boss { FLAG_BOSS } else { 0 },
        ];
        out.extend_from_slice(&self.left.to_le_bytes());
        out.extend_from_slice(&self.skulls.to_le_bytes());
        match &self.teams {
            None => out.push(1),
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
                out.push(LIVES_EACH);
                out.push(each.len() as u8);
                for &(player, count) in each {
                    out.push(player);
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
        if bytes[6] & !FLAG_BOSS != 0 {
            return Err(Error::Field("flags"));
        }
        let left = u16::from_le_bytes([bytes[7], bytes[8]]);
        let skulls = u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]);
        if skulls & !SKULL_BITS != 0 {
            return Err(Error::Field("skulls"));
        }
        let (teams, at) = match bytes[TEAM_COUNT] {
            1 => (None, HEAD),
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
                let n = usize::from(*bytes.get(at).ok_or_else(|| short(at + 1))?);
                if n > MAX_EACH {
                    return Err(Error::Field("lives"));
                }
                let want = at + 1 + 3 * n;
                if bytes.len() < want {
                    return Err(short(want));
                }
                let each = bytes[at + 1..want]
                    .chunks_exact(3)
                    .map(|c| (c[0], u16::from_le_bytes([c[1], c[2]])))
                    .collect::<Vec<_>>();
                if each.windows(2).any(|w| w[0].0 >= w[1].0) {
                    return Err(Error::Field("lives"));
                }
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
/// `local` seen in `scope`: the host's own blob or else the one received,
/// decoded and viewed the same way. None while no mode runs; an error is a
/// blob this build cannot read.
pub fn current(local: u8, scope: Scope) -> Option<Result<View, Error>> {
    let blob = slot().get().or_else(|| received().get())?;
    Some(Mode::decode(&blob).map(|mode| view_of(&mode, local, scope)))
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

/// What the player at roster slot `local` sees of `mode` in `scope`: the
/// kit's view. A pure function of the blob, the slot and the scope: the
/// host and a client agree. A test shows no lives.
pub fn view_of(mode: &Mode, local: u8, scope: Scope) -> View {
    View {
        stage: mode.stage,
        round: mode.round,
        rounds: mode.rounds,
        wave: mode.wave,
        boss: mode.boss,
        left: mode.left,
        skulls: mode.skulls,
        lives: match scope {
            Scope::Match => lives_of(mode, local),
            Scope::Biped | Scope::Monitor => None,
        },
    }
}

/// The kit's values for a view seen in `scope`: the monitor's are the
/// banner alone (`kit::monitor_values`).
fn values_in(v: &View, scope: Scope) -> Values {
    match scope {
        Scope::Monitor => kit::monitor_values(v),
        Scope::Match | Scope::Biped => kit::values(v),
    }
}

/// The HUD for `mode`, as the player at roster slot `local` sees it in
/// `scope`: the kit's values for [`view_of`].
pub fn view(mode: &Mode, local: u8, scope: Scope) -> Values {
    values_in(&view_of(mode, local, scope), scope)
}

// ---- the feed ----

/// The feed's feature id in the plan.
pub const FEED: &str = "firefight-hud";

/// What the feed keeps from tick to tick: the last view, for the notes a
/// change sends, and the last unreadable blob's reason, logged once.
#[derive(Default)]
pub struct Feed {
    last: Option<View>,
    error: Option<String>,
}

impl Feed {
    /// One tick: the values to publish and the notes to send for `now`,
    /// what [`current`] gives for the local player, seen in `scope`. No
    /// match, no local player and a blob this build cannot read hide every
    /// element and forget the last view; the reason for an unreadable blob
    /// is logged when it changes.
    pub fn tick(&mut self, now: Option<Result<View, Error>>, scope: Scope) -> (Values, Vec<String>) {
        let view = match now {
            Some(Ok(v)) => {
                self.error = None;
                Some(v)
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
        let notes = match &view {
            Some(v) => kit::notes(self.last.as_ref(), v, skull_name),
            None => Vec::new(),
        };
        let values = view.as_ref().map_or_else(kit::hidden, |v| values_in(v, scope));
        self.last = view;
        (values, notes)
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
    use engine::view::players;
    let mut out = [None; USERS];
    let Some(array) = players::array(m)? else {
        return Ok(out);
    };
    if m.u32(array.array.addr.wrapping_add(engine::datum::layout::DATA))? == 0 {
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

/// One tick for every local user: user `u`'s view of the blob at its
/// roster slot (`slots[u]`) in its scope (`scopes[u]`, through `current`),
/// its values and its notes. A user with no slot is hidden and its feed
/// forgets its last view.
pub fn tick_all(
    feeds: &mut Feeds,
    slots: &[Option<u8>; USERS],
    scopes: &[Scope; USERS],
    current: impl Fn(u8, Scope) -> Option<Result<View, Error>>,
) -> [(Values, Vec<String>); USERS] {
    std::array::from_fn(|u| {
        feeds[u].tick(slots[u].and_then(|slot| current(slot, scopes[u])), scopes[u])
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

/// The feed on the plan: each tick, [`current`] for each local user's
/// roster slot and scope ([`scopes_now`]) through its [`Feed`]: user 0's values are the HUD's shared
/// ones (the banner, the channels), every other local user's its own
/// (`Handle::publish_for`), so each split-screen player sees its own
/// lives; each user's notes go to that user.
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
        // Rebuilt from the mode blob each tick; the last view only words
        // the next tick's notes.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let (handle, feeds) = (self.handle, Arc::clone(&self.feeds));
        b.frame().on_tick("firefight hud: values", move |o| {
            let slots = local_slots(&*o.guest()).unwrap_or_default();
            let all = tick_all(
                &mut feeds.lock().unwrap_or_else(|e| e.into_inner()),
                &slots,
                &scopes_now(),
                current,
            );
            for (user, (values, notes)) in all.into_iter().enumerate() {
                let user = user as u32;
                if user == 0 {
                    handle.publish(values);
                } else {
                    handle.publish_for(user, slots[user as usize].map(|_| values));
                }
                for text in notes {
                    handle.notify(user, &text);
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
