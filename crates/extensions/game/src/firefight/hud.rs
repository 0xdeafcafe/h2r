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
//! client reads it back (`actor_net::client::Client::mode_state`,
//! [`Mode::decode`]).
//!
//! **The view.** [`view`] is a pure function of a [`Mode`] and the viewing
//! player's roster slot. It reads no game memory and no clock, so the host
//! and a client agree by construction, and a test can show it.
//!
//! **The layout.** [`LAYOUT`] is one fixed list of elements, as the HUD
//! kit describes a HUD (docs/game/hud-kit.md): a state never adds, removes
//! or moves an element, it only shows, hides and words them. [`check`]
//! runs the kit's layout rules over every state the mode can be in. The
//! type names (`Element`, `Value`, `Values`, `Place`) are the kit's, so the
//! day the kit's model is usable from this crate the layout moves over as
//! it is.
//!
//! **Not drawn here.** The kit's runtime (its slices 3 to 5) is not on
//! this branch, and nothing in the game draws a line of ours on Halo 2's
//! HUD without it. Until it lands the view is published and checked but
//! not drawn; the placements in [`LAYOUT`] are in HUD pixels from named
//! anchors and take the retail rectangles' names (the kit's `rects.rs`)
//! when those exist.

use std::sync::Mutex;

use ::firefight::director::{Director, Phase};
use ::firefight::{MAX_TEAMS, Outcome};

/// The blob's format.
pub const VERSION: u8 = 2;
/// The actor channel's mode state holds this many bytes at most.
pub const MAX_BLOB: usize = 63;
/// Players whose own lives the blob carries when lives are not shared.
pub const MAX_EACH: usize = 8;
/// The most rounds, and waves in a round, the blob admits.
pub const MAX_ROUNDS: u8 = 12;
pub const MAX_WAVES: u8 = 16;

/// Where the match is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// Before the first wave.
    Warmup = 0,
    /// The wave's squads are being placed.
    Spawning = 1,
    Fighting = 2,
    /// The last few actors hunt the players.
    Cleanup = 3,
    /// Between waves or rounds.
    Intermission = 4,
    Won = 5,
    Lost = 6,
    /// The director is done and no outcome was logged.
    Over = 7,
    /// Two teams: the first (red) team won.
    Team0Won = 8,
    /// Two teams: the second (blue) team won.
    Team1Won = 9,
    /// Two teams: level on score and lives.
    Draw = 10,
}

impl Stage {
    fn from_code(code: u8) -> Option<Stage> {
        Some(match code {
            0 => Stage::Warmup,
            1 => Stage::Spawning,
            2 => Stage::Fighting,
            3 => Stage::Cleanup,
            4 => Stage::Intermission,
            5 => Stage::Won,
            6 => Stage::Lost,
            7 => Stage::Over,
            8 => Stage::Team0Won,
            9 => Stage::Team1Won,
            10 => Stage::Draw,
            _ => return None,
        })
    }

    /// The stage of a match `team` won.
    pub fn team_won(team: u8) -> Stage {
        if team == 0 {
            Stage::Team0Won
        } else {
            Stage::Team1Won
        }
    }

    /// A stage only two teams reach.
    fn two_teams_only(self) -> bool {
        matches!(self, Stage::Team0Won | Stage::Team1Won | Stage::Draw)
    }

    /// The wave is on: enemies are being placed, fought or hunted.
    pub fn wave_on(self) -> bool {
        matches!(self, Stage::Spawning | Stage::Fighting | Stage::Cleanup)
    }

    /// The match is over.
    pub fn over(self) -> bool {
        matches!(
            self,
            Stage::Won | Stage::Lost | Stage::Over | Stage::Team0Won | Stage::Team1Won | Stage::Draw
        )
    }
}

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
    pub lives: Lives,
    /// Two teams' points and members; none with one team.
    pub teams: Option<Teams>,
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
/// waves, flags, left (2), the team count.
const HEAD: usize = 10;
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
    pub fn of(director: &Director, players: u8, finished: Option<Outcome>) -> Result<Mode, Error> {
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
        let (teams, at) = match bytes[9] {
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
/// `local`: the host's own blob or else the one received, decoded and
/// viewed the same way. None while no mode runs; an error is a blob this
/// build cannot read.
pub fn current(local: u8) -> Option<Result<Values, Error>> {
    let blob = slot().get().or_else(|| received().get())?;
    Some(Mode::decode(&blob).map(|mode| view(&mode, local)))
}

// ---- the view ----

/// A HUD font, as the kit names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Hud,
}

/// Where a line sits from: a screen edge or its middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopCenter,
    TopRight,
}

/// A line's place in HUD pixels from its anchor, per screen mode; `None`
/// hides it in that mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub anchor: Anchor,
    pub full: (i16, i16),
    pub half: Option<(i16, i16)>,
    pub quarter: Option<(i16, i16)>,
}

/// One line of the HUD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: &'static str,
    pub font: Font,
    pub at: Place,
    /// The widest text any state gives it.
    pub longest: &'static str,
}

pub const BANNER: &str = "banner";
pub const ROUND: &str = "round";
pub const WAVE: &str = "wave";
pub const ENEMIES: &str = "enemies";
pub const LIVES: &str = "lives";
/// Both teams' points, with two teams.
pub const SCORE: &str = "score";

/// The banner at the top centre; the round, wave, enemies, lives and (with
/// two teams) both teams' points stacked at the top right, a line apart.
/// Smaller screens show less.
pub const LAYOUT: [Element; 6] = [
    Element {
        id: BANNER,
        font: Font::Hud,
        at: Place {
            anchor: Anchor::TopCenter,
            full: (0, 60),
            half: Some((0, 48)),
            quarter: None,
        },
        longest: "ROUND COMPLETE",
    },
    Element {
        id: ROUND,
        font: Font::Hud,
        at: Place {
            anchor: Anchor::TopRight,
            full: (-24, 24),
            half: None,
            quarter: None,
        },
        longest: "ROUND 12 OF 12",
    },
    Element {
        id: WAVE,
        font: Font::Hud,
        at: Place {
            anchor: Anchor::TopRight,
            full: (-24, 40),
            half: Some((-20, 24)),
            quarter: None,
        },
        longest: "WAVE 16 OF 16",
    },
    Element {
        id: ENEMIES,
        font: Font::Hud,
        at: Place {
            anchor: Anchor::TopRight,
            full: (-24, 56),
            half: Some((-20, 40)),
            quarter: Some((-16, 20)),
        },
        longest: "ENEMIES 65535",
    },
    Element {
        id: LIVES,
        font: Font::Hud,
        at: Place {
            anchor: Anchor::TopRight,
            full: (-24, 72),
            half: Some((-20, 56)),
            quarter: Some((-16, 34)),
        },
        longest: "LIVES 65535",
    },
    Element {
        id: SCORE,
        font: Font::Hud,
        at: Place {
            anchor: Anchor::TopRight,
            full: (-24, 88),
            half: Some((-20, 72)),
            quarter: Some((-16, 48)),
        },
        longest: "RED 4294967295 BLUE 4294967295",
    },
];

/// How a line is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    Normal,
    /// The last life, or none.
    Warn,
}

/// One element's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Value {
    pub shown: bool,
    pub text: String,
    pub style: Style,
}

/// Every element's state, in [`LAYOUT`]'s order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Values(pub Vec<(&'static str, Value)>);

impl Values {
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.0.iter().find(|(i, _)| *i == id).map(|(_, v)| v)
    }
}

fn banner_text(m: &Mode) -> Option<String> {
    Some(match m.stage {
        Stage::Warmup => "GET READY".into(),
        Stage::Spawning if m.boss => "BOSS WAVE".into(),
        Stage::Spawning => format!("WAVE {}", u16::from(m.wave) + 1),
        Stage::Fighting => return None,
        Stage::Cleanup => "FINISH THEM".into(),
        Stage::Intermission if m.wave == 0 && m.round > 0 => "ROUND COMPLETE".into(),
        Stage::Intermission => "WAVE COMPLETE".into(),
        Stage::Won => "VICTORY".into(),
        Stage::Lost | Stage::Over => "GAME OVER".into(),
        Stage::Team0Won => "RED TEAM WINS".into(),
        Stage::Team1Won => "BLUE TEAM WINS".into(),
        Stage::Draw => "DRAW".into(),
    })
}

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

/// The HUD for `mode`, as the player at roster slot `local` sees it. A pure
/// function of the blob: the host and a client agree.
pub fn view(mode: &Mode, local: u8) -> Values {
    let on = !mode.stage.over();
    let line = |shown: bool, text: String, style: Style| Value { shown, text, style };
    let hidden = || Value::default();
    let banner = banner_text(mode).map_or_else(hidden, |t| line(true, t, Style::Normal));
    let round = if on {
        line(
            true,
            format!("ROUND {} OF {}", u16::from(mode.round) + 1, mode.rounds),
            Style::Normal,
        )
    } else {
        hidden()
    };
    let wave = match (on, mode.boss) {
        (false, _) => hidden(),
        (true, true) => line(true, "BOSS WAVE".into(), Style::Normal),
        (true, false) => line(
            true,
            format!("WAVE {} OF {}", u16::from(mode.wave) + 1, mode.waves),
            Style::Normal,
        ),
    };
    let enemies = if mode.stage.wave_on() {
        line(true, format!("ENEMIES {}", mode.left), Style::Normal)
    } else {
        hidden()
    };
    let lives = match lives_of(mode, local).filter(|_| on) {
        Some(n) => line(
            true,
            format!("LIVES {n}"),
            if n <= 1 { Style::Warn } else { Style::Normal },
        ),
        None => hidden(),
    };
    // Shown through the end too: the result is read against the points.
    let score = match mode.teams {
        Some(t) => line(
            true,
            format!("RED {} BLUE {}", t.scores[0], t.scores[1]),
            Style::Normal,
        ),
        None => hidden(),
    };
    Values(vec![
        (BANNER, banner),
        (ROUND, round),
        (WAVE, wave),
        (ENEMIES, enemies),
        (LIVES, lives),
        (SCORE, score),
    ])
}

// ---- the checks ----

/// A screen mode of the kit's three: whole screen, half (two players) and
/// quarter (three or four).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Full,
    Half,
    Quarter,
}

impl Screen {
    pub const ALL: [Screen; 3] = [Screen::Full, Screen::Half, Screen::Quarter];
}

/// The HUD's base screen, in HUD pixels.
pub const SCREEN_W: i32 = 640;
/// Widths and heights the checks assume of the HUD font, in HUD pixels:
/// generous, so a pass means room.
pub const GLYPH_W: i32 = 10;
pub const LINE_H: i32 = 14;

/// What the checks found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// A state's elements are not the layout's, in its order.
    Layout { state: String },
    /// A text is wider than its element's `longest`.
    Wide {
        state: String,
        id: &'static str,
        text: String,
    },
    /// A shown element has no text.
    Empty { state: String, id: &'static str },
    /// A text has a leading, trailing or doubled space.
    Spacing {
        state: String,
        id: &'static str,
        text: String,
    },
    /// A text has a character the HUD copy does not use.
    Glyph {
        state: String,
        id: &'static str,
        ch: char,
    },
    /// Two elements' widest texts overlap in a screen mode.
    Overlap {
        a: &'static str,
        b: &'static str,
        screen: Screen,
    },
    /// An element's widest text leaves the screen.
    Off { id: &'static str, screen: Screen },
}

/// The characters HUD copy uses: capitals, digits and a space.
fn copy_char(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == ' '
}

fn offset(at: &Place, screen: Screen) -> Option<(i16, i16)> {
    match screen {
        Screen::Full => Some(at.full),
        Screen::Half => at.half,
        Screen::Quarter => at.quarter,
    }
}

/// An element's rectangle for its widest text, `[left, top, right,
/// bottom]` in HUD pixels from the top left; none where it is hidden.
pub fn rect(e: &Element, screen: Screen) -> Option<[i32; 4]> {
    let (dx, dy) = offset(&e.at, screen)?;
    let w = e.longest.chars().count() as i32 * GLYPH_W;
    let (dx, dy) = (i32::from(dx), i32::from(dy));
    let left = match e.at.anchor {
        Anchor::TopCenter => SCREEN_W / 2 + dx - w / 2,
        Anchor::TopRight => SCREEN_W + dx - w,
    };
    Some([left, dy, left + w, dy + LINE_H])
}

fn overlaps(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The kit's layout rules over `all` the states: the layout is the same in each,
/// each text fits its element, the copy is spaced and spelled as HUD copy
/// is, and in every screen mode no two elements' widest texts overlap or
/// leave the screen.
pub fn check(all: &[(String, Values)]) -> Vec<Finding> {
    let mut out = Vec::new();
    for (name, values) in all {
        let ids: Vec<_> = values.0.iter().map(|(i, _)| *i).collect();
        if ids != LAYOUT.iter().map(|e| e.id).collect::<Vec<_>>() {
            out.push(Finding::Layout {
                state: name.clone(),
            });
            continue;
        }
        for (e, &(id, ref v)) in LAYOUT.iter().zip(&values.0) {
            if !v.shown {
                continue;
            }
            if v.text.is_empty() {
                out.push(Finding::Empty {
                    state: name.clone(),
                    id,
                });
            }
            if v.text.chars().count() > e.longest.chars().count() {
                out.push(Finding::Wide {
                    state: name.clone(),
                    id,
                    text: v.text.clone(),
                });
            }
            if v.text.starts_with(' ') || v.text.ends_with(' ') || v.text.contains("  ") {
                out.push(Finding::Spacing {
                    state: name.clone(),
                    id,
                    text: v.text.clone(),
                });
            }
            if let Some(ch) = v.text.chars().find(|&c| !copy_char(c)) {
                out.push(Finding::Glyph {
                    state: name.clone(),
                    id,
                    ch,
                });
            }
        }
    }
    for screen in Screen::ALL {
        let rects: Vec<_> = LAYOUT
            .iter()
            .filter_map(|e| rect(e, screen).map(|r| (e.id, r)))
            .collect();
        for (i, &(a, ra)) in rects.iter().enumerate() {
            if ra[0] < 0 || ra[2] > SCREEN_W || ra[1] < 0 {
                out.push(Finding::Off { id: a, screen });
            }
            for &(b, rb) in &rects[i + 1..] {
                if overlaps(ra, rb) {
                    out.push(Finding::Overlap { a, b, screen });
                }
            }
        }
    }
    out
}

/// Every state the HUD can be in, by name, for the checks and for a
/// preview sheet: each stage, a boss wave, a round's end, the lives at
/// their extremes and the viewer with and without a count of their own.
pub fn states() -> Vec<(String, Values)> {
    let base = Mode {
        stage: Stage::Fighting,
        round: 0,
        rounds: 3,
        wave: 1,
        waves: 5,
        boss: false,
        left: 7,
        lives: Lives::Shared(12),
        teams: None,
    };
    let teams = Teams {
        scores: [1_250, 900],
        second: 0b10,
    };
    let with = |f: &dyn Fn(&mut Mode)| {
        let mut m = base.clone();
        f(&mut m);
        m
    };
    let modes = [
        (
            "warmup",
            with(&|m| {
                m.stage = Stage::Warmup;
                m.wave = 0;
                m.left = 0;
            }),
        ),
        ("spawning", with(&|m| m.stage = Stage::Spawning)),
        (
            "boss",
            with(&|m| {
                m.stage = Stage::Spawning;
                m.wave = 4;
                m.boss = true;
            }),
        ),
        ("fighting", base.clone()),
        (
            "cleanup",
            with(&|m| {
                m.stage = Stage::Cleanup;
                m.left = 2;
            }),
        ),
        (
            "wave-complete",
            with(&|m| {
                m.stage = Stage::Intermission;
                m.left = 0;
            }),
        ),
        (
            "round-complete",
            with(&|m| {
                m.stage = Stage::Intermission;
                m.round = 1;
                m.wave = 0;
                m.left = 0;
            }),
        ),
        ("won", with(&|m| m.stage = Stage::Won)),
        ("lost", with(&|m| m.stage = Stage::Lost)),
        ("over", with(&|m| m.stage = Stage::Over)),
        ("last-life", with(&|m| m.lives = Lives::Shared(1))),
        ("no-lives", with(&|m| m.lives = Lives::Shared(0))),
        (
            "widest",
            with(&|m| {
                m.round = 11;
                m.rounds = 12;
                m.wave = 15;
                m.waves = 16;
                m.left = u16::MAX;
                m.lives = Lives::Shared(u16::MAX);
            }),
        ),
        (
            "own-lives",
            with(&|m| m.lives = Lives::Each(vec![(0, 3), (2, 1)])),
        ),
        (
            "no-own-lives",
            with(&|m| m.lives = Lives::Each(vec![(1, 3)])),
        ),
        (
            "two-teams",
            with(&|m| {
                m.teams = Some(teams);
                m.lives = Lives::Teams([8, 5]);
            }),
        ),
        (
            "two-teams-own-lives",
            with(&|m| {
                m.teams = Some(teams);
                m.lives = Lives::Each(vec![(0, 2), (1, 4)]);
            }),
        ),
        (
            "two-teams-widest",
            with(&|m| {
                m.teams = Some(Teams {
                    scores: [u32::MAX; MAX_TEAMS],
                    second: u16::MAX,
                });
                m.lives = Lives::Teams([u16::MAX; MAX_TEAMS]);
            }),
        ),
        (
            "red-wins",
            with(&|m| {
                m.stage = Stage::Team0Won;
                m.teams = Some(teams);
                m.lives = Lives::Teams([3, 0]);
            }),
        ),
        (
            "blue-wins",
            with(&|m| {
                m.stage = Stage::Team1Won;
                m.teams = Some(teams);
                m.lives = Lives::Teams([0, 3]);
            }),
        ),
        (
            "draw",
            with(&|m| {
                m.stage = Stage::Draw;
                m.teams = Some(Teams {
                    scores: [900, 900],
                    ..teams
                });
                m.lives = Lives::Teams([2, 2]);
            }),
        ),
    ];
    modes
        .into_iter()
        .map(|(name, m)| (name.to_string(), view(&m, 0)))
        .collect()
}

#[cfg(test)]
mod tests;
