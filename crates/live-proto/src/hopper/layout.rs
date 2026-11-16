//! The whole hopper image, field by field, as the retail game reads it
//! (docs/live/live-matchmaking.md §6 has the table with every address). This is
//! the decoded view: [`Descriptor`] is all 0x614 bytes of a hopper
//! descriptor, [`Entry`] all 0x37c bytes of a record slot, and
//! [`HopperFile::descriptor`] / [`HopperFile::entries`] read them, the
//! setters write them back. The older field API in the parent module
//! (`Hopper`, `Record`) stays for the editors that use it.
//!
//! Descriptor (0x614 bytes; the game's copy of all 16 is at `0x00551ae8`):
//!
//! ```text
//! +0x000  i32        hopper type 1..5 (HopperType)
//! +0x004  i32        status: 0 not checked, 1 ok, 2 error, 3 no hopper
//! +0x008  i32        playlist id 1..100 (stats ids, 0x000741f0)
//! +0x00c  9 x 16     title, UTF-8, one slot per language
//! +0x09c  9 x 128    description, UTF-8, one slot per language
//! +0x51c  128 x i8   skill ceiling: entry i is the highest skill a player of
//!                    skill i is matched with (i <= entry <= 127)
//! +0x59c  u32        search timeout, ms (0x00071c7a)
//! +0x5a0  u32        base wait, ms: only the defaults write it (no reader)
//! +0x5a4  16 x u32   start wait, ms, for 1..16 players (0x001931d0);
//!                    0x7fffffff never, 0 at once
//! +0x5e4  i32        minimum rank (0x00066150, 0x002b2090)
//! +0x5e8  u8         list regardless of rank (0x002b2070)
//! +0x5e9  3 bytes    unused (no reader found)
//! +0x5ec  10 x u32   type parameters (Rules)
//! ```
//!
//! Data block (0x15cb8 bytes): `+0x00..+0x44` unused (no reader found),
//! `+0x44` the entry count, `+0x48` 100 entry slots of 0x37c.
//!
//! Entry (0x37c bytes):
//!
//! ```text
//! +0x000  u32         unused (no reader found)
//! +0x004  u32         map id (must be a multiplayer level, 0x0019c5f0)
//! +0x008  9 x 32 u16  map name per language: only the defaults write it
//!                     (no reader found)
//! +0x248  i32         weight, clamped to 1..=1000 (0x00192f12)
//! +0x24c  0x130       game variant (variant::VariantRecord)
//! ```

use super::variant::VariantRecord;
use super::{
    BLOCK, BLOCKS_AT, DESCRIPTOR, HOPPERS, HopperError, HopperFile, LANGUAGES, MAX_DESCRIPTION,
    MAX_RECORDS, MAX_TITLE, RECORD, get_utf8, le32, put_le32,
};

/// Skill levels in the ceiling table.
pub const SKILL_LEVELS: usize = 128;
/// Player counts the start-wait table covers (1..=16).
pub const PLAYER_COUNTS: usize = 16;
/// Type parameter dwords at `+0x5ec`.
pub const PARAMS: usize = 10;
/// The most players a game takes.
pub const MAX_PLAYERS: u32 = 16;
/// The start-wait value for "never with this many players".
pub const NEVER: u32 = 0x7fff_ffff;
/// Weights session setup takes as they are; others are clamped.
pub const WEIGHTS: std::ops::RangeInclusive<u32> = 1..=1000;
/// Playlist ids the validator accepts.
pub const PLAYLIST_IDS: std::ops::RangeInclusive<u32> = 1..=100;
/// UTF-16 units in each of an entry's map-name slots.
pub const MAP_NAME_UNITS: usize = 32;

// Descriptor fields.
const D_TYPE: usize = 0x000;
const D_STATUS: usize = 0x004;
pub(super) const D_PLAYLIST_ID: usize = 0x008;
const D_TITLE: usize = 0x00c;
const D_TITLE_LEN: usize = 0x10;
const D_DESC: usize = 0x09c;
const D_DESC_LEN: usize = 0x80;
const D_SKILL: usize = 0x51c;
const D_SEARCH_TIMEOUT: usize = 0x59c;
const D_BASE_WAIT: usize = 0x5a0;
const D_START_WAIT: usize = 0x5a4;
const D_MIN_RANK: usize = 0x5e4;
const D_ANY_RANK: usize = 0x5e8;
const D_PARAMS: usize = 0x5ec;
const _: () = assert!(D_DESC + LANGUAGES * D_DESC_LEN == D_SKILL);
const _: () = assert!(D_SKILL + SKILL_LEVELS == D_SEARCH_TIMEOUT);
const _: () = assert!(D_START_WAIT + 4 * PLAYER_COUNTS == D_MIN_RANK);
const _: () = assert!(D_PARAMS + 4 * PARAMS == DESCRIPTOR);
// Block and entry fields.
pub(super) const B_COUNT: usize = 0x44;
const B_ENTRIES: usize = 0x48;
const E_MAP: usize = 0x004;
const E_MAP_NAMES: usize = 0x008;
const E_WEIGHT: usize = 0x248;
const E_VARIANT: usize = 0x24c;
const _: () = assert!(E_MAP_NAMES + LANGUAGES * 2 * MAP_NAME_UNITS == E_WEIGHT);
const _: () = assert!(E_VARIANT + super::variant::SIZE == RECORD);

/// What a hopper is, by its type number. The accessors at
/// `0x00193470`..`0x001934b0` sort them: ranked {2, 4, 5}, unranked {1, 3},
/// teams {3, 4, 5}. The playlist screen's best-skill pass (`0x00065530`)
/// walks the ranked ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HopperType {
    /// 1: free for all, unranked; parties up to the player limit.
    UnrankedFreeForAll,
    /// 2: free for all, ranked; alone only (the party limit is 1).
    RankedFreeForAll,
    /// 3: teams, unranked; the whole party joins.
    UnrankedTeams,
    /// 4: teams, ranked, with party size limits and an optional
    /// party-as-team rule.
    RankedTeams,
    /// 5: clan match: ranked teams where a party is a team.
    Clan,
}

impl HopperType {
    pub fn from_raw(t: u32) -> Option<Self> {
        Some(match t {
            1 => Self::UnrankedFreeForAll,
            2 => Self::RankedFreeForAll,
            3 => Self::UnrankedTeams,
            4 => Self::RankedTeams,
            5 => Self::Clan,
            _ => return None,
        })
    }

    pub fn raw(self) -> u32 {
        match self {
            Self::UnrankedFreeForAll => 1,
            Self::RankedFreeForAll => 2,
            Self::UnrankedTeams => 3,
            Self::RankedTeams => 4,
            Self::Clan => 5,
        }
    }

    /// `0x00193470`.
    pub fn ranked(self) -> bool {
        matches!(
            self,
            Self::RankedFreeForAll | Self::RankedTeams | Self::Clan
        )
    }

    /// `0x001934b0`.
    pub fn teams(self) -> bool {
        matches!(self, Self::UnrankedTeams | Self::RankedTeams | Self::Clan)
    }
}

/// The type parameters at `+0x5ec`, by type. Sizes are players.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rules {
    /// Types 1 and 2: `[0]` fewest players, `[1]` most (`0x00193610`:
    /// 1 <= min <= max <= 16).
    FreeForAll { ranked: bool, players: (u32, u32) },
    /// Type 3: `[0]` teams, `[1]` and `[2]` team size, byte `[3]` uneven
    /// teams allowed (read as a size difference of 0 or 1 by `0x00193400`).
    /// `0x001936a0`: 2..=16 teams, 1 <= min <= max <= 16 / teams.
    UnrankedTeams {
        teams: u32,
        team_size: (u32, u32),
        uneven: bool,
    },
    /// Type 4: `[0]` teams, `[1]`/`[2]` team size, `[3]` largest size
    /// difference between teams, `[4]`/`[5]` party size, byte `[6]` with
    /// `[7]`/`[8]` parties of that size are kept together as one team
    /// (`0x001933a0`, `0x001933d0`, `0x001934d0`); `[9]` unused (no reader
    /// found). `0x00193630`: 2..=16 teams, 2 <= min <= max <= 16 / teams,
    /// 0 <= difference <= max - min, 1 <= party min <= party max <= max.
    RankedTeams {
        teams: u32,
        team_size: (u32, u32),
        max_difference: u32,
        party: (u32, u32),
        party_as_team: Option<(u32, u32)>,
    },
    /// Type 5: `[0]` teams, `[1]`/`[2]` team size (also the party size: a
    /// party is a team), `[3]` largest size difference. Validated as
    /// type 3.
    Clan {
        teams: u32,
        team_size: (u32, u32),
        max_difference: u32,
    },
}

impl Rules {
    pub fn hopper_type(&self) -> HopperType {
        match self {
            Rules::FreeForAll { ranked: false, .. } => HopperType::UnrankedFreeForAll,
            Rules::FreeForAll { ranked: true, .. } => HopperType::RankedFreeForAll,
            Rules::UnrankedTeams { .. } => HopperType::UnrankedTeams,
            Rules::RankedTeams { .. } => HopperType::RankedTeams,
            Rules::Clan { .. } => HopperType::Clan,
        }
    }

    /// The ten parameter dwords; parameters the type does not use are 0.
    pub fn params(&self) -> [u32; PARAMS] {
        let mut p = [0; PARAMS];
        match *self {
            Rules::FreeForAll { players, .. } => {
                p[0] = players.0;
                p[1] = players.1;
            }
            Rules::UnrankedTeams {
                teams,
                team_size,
                uneven,
            } => {
                p[..4].copy_from_slice(&[teams, team_size.0, team_size.1, u32::from(uneven)]);
            }
            Rules::RankedTeams {
                teams,
                team_size,
                max_difference,
                party,
                party_as_team,
            } => {
                let (on, lo, hi) = match party_as_team {
                    Some((lo, hi)) => (1, lo, hi),
                    None => (0, 0, 0),
                };
                p[..9].copy_from_slice(&[
                    teams,
                    team_size.0,
                    team_size.1,
                    max_difference,
                    party.0,
                    party.1,
                    on,
                    lo,
                    hi,
                ]);
            }
            Rules::Clan {
                teams,
                team_size,
                max_difference,
            } => {
                p[..4].copy_from_slice(&[teams, team_size.0, team_size.1, max_difference]);
            }
        }
        p
    }

    /// The rules a type's parameters say (only the bytes the game reads).
    pub fn from_params(t: HopperType, p: &[u32; PARAMS]) -> Self {
        match t {
            HopperType::UnrankedFreeForAll | HopperType::RankedFreeForAll => Rules::FreeForAll {
                ranked: t.ranked(),
                players: (p[0], p[1]),
            },
            HopperType::UnrankedTeams => Rules::UnrankedTeams {
                teams: p[0],
                team_size: (p[1], p[2]),
                uneven: p[3] & 0xff != 0,
            },
            HopperType::RankedTeams => Rules::RankedTeams {
                teams: p[0],
                team_size: (p[1], p[2]),
                max_difference: p[3],
                party: (p[4], p[5]),
                party_as_team: (p[6] & 0xff != 0).then_some((p[7], p[8])),
            },
            HopperType::Clan => Rules::Clan {
                teams: p[0],
                team_size: (p[1], p[2]),
                max_difference: p[3],
            },
        }
    }
}

/// All of a hopper descriptor's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    /// 1..5 ([`HopperType`]); 0 for an unused slot.
    pub hopper_type: u32,
    /// 0 makes the game check the entries' maps when it first needs the
    /// hopper (`0x00194660`) and set 1 or 2; a file should carry 0.
    pub status: u32,
    /// 1..=100, unique per playlist: the base of the playlist's stats ids
    /// (`0x000741f0` reads it with `+0x20` and `+0x40`). Not what a
    /// session advertises: that is the hopper's index (module notes).
    pub playlist_id: u32,
    /// Per language, UTF-8, at most [`MAX_TITLE`] bytes.
    pub titles: [String; LANGUAGES],
    /// Per language, UTF-8, at most [`MAX_DESCRIPTION`] bytes.
    pub descriptions: [String; LANGUAGES],
    /// Entry `i`: the highest skill a player of skill `i` is matched with.
    /// The validator wants `i <= entry <= 127`; `0x001931a0` reads it
    /// backwards for the lowest skill matched.
    pub skill_ceiling: [i8; SKILL_LEVELS],
    /// How long a team arrangement may take before the search gives up on
    /// it, ms (`0x00071c7a`).
    pub search_timeout_ms: u32,
    /// The wait for the smallest game the start-wait table was built from.
    /// Only the defaults write it; no reader found.
    pub base_wait_ms: u32,
    /// Index `n - 1`: how long the search waits before starting a game of
    /// `n` players, ms; [`NEVER`] outside the type's player range
    /// (`0x001931d0`, which itself answers `NEVER` outside the range and 0
    /// at the most players).
    pub start_wait_ms: [u32; PLAYER_COUNTS],
    /// A party whose players' rank is below this cannot search
    /// (`0x00066150`, error 0xf), and the list at `0x002b1ff4` hides the
    /// hopper from a profile below it.
    pub min_rank: i32,
    /// The list at `0x002b1ff4` shows the hopper whatever the rank.
    pub list_any_rank: bool,
    /// The raw type parameters ([`Rules`]).
    pub params: [u32; PARAMS],
}

impl Default for Descriptor {
    fn default() -> Self {
        Descriptor {
            hopper_type: 0,
            status: 0,
            playlist_id: 0,
            titles: Default::default(),
            descriptions: Default::default(),
            skill_ceiling: [0; SKILL_LEVELS],
            search_timeout_ms: 0,
            base_wait_ms: 0,
            start_wait_ms: [0; PLAYER_COUNTS],
            min_rank: 0,
            list_any_rank: false,
            params: [0; PARAMS],
        }
    }
}

impl Descriptor {
    pub fn kind(&self) -> Option<HopperType> {
        HopperType::from_raw(self.hopper_type)
    }

    pub fn rules(&self) -> Option<Rules> {
        Some(Rules::from_params(self.kind()?, &self.params))
    }

    /// Sets the type and its parameters together.
    pub fn set_rules(&mut self, r: &Rules) {
        self.hopper_type = r.hopper_type().raw();
        self.params = r.params();
    }

    /// The same title and description in every language slot.
    pub fn set_text(&mut self, title: &str, description: &str) {
        self.titles = std::array::from_fn(|_| title.to_string());
        self.descriptions = std::array::from_fn(|_| description.to_string());
    }

    fn p(&self, i: usize) -> u32 {
        self.params[i]
    }

    /// Fewest players in a game (`0x001932c0`).
    pub fn min_players(&self) -> Option<u32> {
        let t = self.kind()?;
        Some(if t.teams() {
            self.p(1) * self.p(0)
        } else {
            self.p(0)
        })
    }

    /// Most players in a game (`0x00193300`).
    pub fn max_players(&self) -> Option<u32> {
        let t = self.kind()?;
        Some(if t.teams() {
            self.p(2) * self.p(0)
        } else {
            self.p(1)
        })
    }

    /// Fewest players per team (`0x00193340`): 1 without teams.
    pub fn min_team_size(&self) -> Option<u32> {
        Some(if self.kind()?.teams() { self.p(1) } else { 1 })
    }

    /// Most players per team (`0x00193370`): 1 without teams.
    pub fn max_team_size(&self) -> Option<u32> {
        Some(if self.kind()?.teams() { self.p(2) } else { 1 })
    }

    /// Teams (`0x00193440`): `None` without teams.
    pub fn team_count(&self) -> Option<u32> {
        self.kind()?.teams().then(|| self.p(0))
    }

    /// The largest party that may search (`0x00193250`).
    pub fn max_party(&self) -> Option<u32> {
        Some(match self.kind()? {
            HopperType::UnrankedFreeForAll => self.p(1),
            HopperType::RankedFreeForAll => 1,
            HopperType::UnrankedTeams => self.p(2) * self.p(0),
            HopperType::RankedTeams => self.p(5),
            HopperType::Clan => self.p(2),
        })
    }

    /// The smallest party that may search (`0x0006620d`).
    pub fn min_party(&self) -> Option<u32> {
        Some(match self.kind()? {
            HopperType::RankedTeams => self.p(4),
            HopperType::Clan => self.p(1),
            _ => 1,
        })
    }

    /// The largest team size difference allowed (`0x00193400`).
    pub fn max_difference(&self) -> Option<u32> {
        Some(match self.kind()? {
            HopperType::UnrankedFreeForAll | HopperType::RankedFreeForAll => 0,
            HopperType::UnrankedTeams => u32::from(self.p(3) & 0xff != 0),
            HopperType::RankedTeams | HopperType::Clan => self.p(3),
        })
    }

    /// Party sizes kept together as one team (`0x001934d0` with
    /// `0x001933d0`/`0x001933a0`).
    pub fn party_as_team(&self) -> Option<(u32, u32)> {
        match self.kind()? {
            HopperType::Clan => Some((self.p(1), self.p(2))),
            HopperType::RankedTeams if self.p(6) & 0xff != 0 => Some((self.p(7), self.p(8))),
            _ => None,
        }
    }

    /// How long a search waits before starting with `players` players, as
    /// `0x001931d0` answers: [`NEVER`] outside the player range, 0 at the
    /// most players, else the table.
    pub fn start_wait(&self, players: u32) -> Option<u32> {
        let (lo, hi) = (self.min_players()?, self.max_players()?);
        Some(if players < lo || players > hi {
            NEVER
        } else if players == hi {
            0
        } else {
            self.start_wait_ms[players as usize - 1]
        })
    }

    /// The lowest skill a player of skill `skill` is matched with
    /// (`0x001931a0`, which walks the table down from `skill` while the
    /// entry reaches it; the table there is indexed from `+0x51b`).
    pub fn lowest_matched(&self, skill: u32) -> u32 {
        let mut s = skill.min(SKILL_LEVELS as u32);
        while s > 0 {
            // `+0x51b + s` is ceiling entry `s - 1`.
            if (self.skill_ceiling[s as usize - 1] as i32) < skill as i32 {
                break;
            }
            s -= 1;
        }
        s
    }
}

/// The start-wait table the game's defaults build (`0x001939f0` for free
/// for all, `0x00193a50` for teams, which is the same with `teams` 1): with
/// `n` players, `NEVER` outside `teams * min ..= teams * max`, 0 at the
/// top, else `(max - n / teams) * base / (max - min)`, so the wait falls
/// linearly from `base` at the fewest players.
pub fn start_waits(base_ms: u32, teams: u32, size: (u32, u32)) -> [u32; PLAYER_COUNTS] {
    let (lo, hi) = (teams * size.0, teams * size.1);
    std::array::from_fn(|k| {
        let n = k as u32 + 1;
        if n < lo || n > hi {
            NEVER
        } else if n == hi {
            0
        } else {
            ((size.1 - n / teams) as u64 * base_ms as u64 / (size.1 - size.0) as u64) as u32
        }
    })
}

/// The skill ceiling the game's defaults write (`0x001938bc`): a player is
/// matched with players up to 5 skill levels above at the bottom of the
/// table, widening to 10 from skill 36, capped at 127.
pub fn default_skill_ceiling() -> [i8; SKILL_LEVELS] {
    std::array::from_fn(|i| {
        let spread = match i {
            0..=5 => 5,
            6..=15 => 6,
            16..=25 => 7,
            26..=30 => 8,
            31..=35 => 9,
            _ => 10,
        };
        (i + spread).min(127) as i8
    })
}

/// One playlist entry: a map, its weight and the game variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// A multiplayer level's map id (the globals tag's level list; the
    /// retail ids are [`super::defaults::RETAIL_MAPS`]). An id the game
    /// cannot find marks the whole hopper as an error (`0x00194660`).
    pub map_id: u32,
    /// Session setup picks an entry at random by weight (`0x00192eb0`),
    /// clamping each to 1..=1000.
    pub weight: u32,
    /// The map name slots (`+0x08`, nine languages): written by the
    /// game's defaults, read by nothing found. The English slot is read
    /// here; writes fill all nine, or none when empty.
    pub map_name: String,
    /// The game variant played.
    pub variant: VariantRecord,
}

fn put_text(field: &mut [u8], s: &str, max: usize) -> Result<(), HopperError> {
    if s.contains('\0') {
        return Err(HopperError::Nul(s.into()));
    }
    if s.len() > max {
        return Err(HopperError::TooLong(s.into(), max));
    }
    field.fill(0);
    field[..s.len()].copy_from_slice(s.as_bytes());
    Ok(())
}

/// A problem the game's readers would have with a file, in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub hopper: usize,
    /// The entry, for problems in a data block.
    pub entry: Option<usize>,
    /// Offset into the image.
    pub at: usize,
    pub what: String,
}

/// What the validator (`0x00193560` with the per-type checks behind
/// `0x001934f0`) says about a descriptor, as `(offset, what)`: empty when
/// the game takes it. A descriptor that fails is not a hopper to the game
/// (every reader goes through `0x001934f0`).
pub fn check_descriptor(d: &Descriptor) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let Some(kind) = d.kind() else {
        out.push((D_TYPE, format!("type {} is not 1..=5", d.hopper_type)));
        return out;
    };
    if d.status > 3 {
        out.push((D_STATUS, format!("status {} is not 0..=3", d.status)));
    }
    if !PLAYLIST_IDS.contains(&d.playlist_id) {
        out.push((
            D_PLAYLIST_ID,
            format!("playlist id {} is not 1..=100", d.playlist_id),
        ));
    }
    for (i, e) in d.skill_ceiling.iter().enumerate() {
        if (*e as i32) < i as i32 {
            out.push((
                D_SKILL + i,
                format!("skill ceiling {e} at {i} is below {i}"),
            ));
        }
    }
    let p = |i: usize| d.params[i] as i32;
    let at = |i: usize| D_PARAMS + 4 * i;
    let mut need = |ok: bool, i: usize, what: &str| {
        if !ok {
            out.push((at(i), what.to_string()));
        }
    };
    match kind {
        HopperType::UnrankedFreeForAll | HopperType::RankedFreeForAll => {
            need(p(0) >= 1, 0, "fewest players must be at least 1");
            need(p(1) <= 16, 1, "most players must be at most 16");
            need(p(0) <= p(1), 0, "fewest players must not exceed most");
        }
        HopperType::UnrankedTeams | HopperType::Clan | HopperType::RankedTeams => {
            let min_size = if kind == HopperType::RankedTeams {
                2
            } else {
                1
            };
            need((2..=16).contains(&p(0)), 0, "teams must be 2..=16");
            need(p(1) >= min_size, 1, "the smallest team is too small");
            let cap = if (2..=16).contains(&p(0)) {
                16 / p(0)
            } else {
                0
            };
            need(p(2) <= cap, 2, "most players per team must fit 16 players");
            need(
                p(1) <= p(2),
                1,
                "the smallest team must not exceed the largest",
            );
            if kind == HopperType::RankedTeams {
                need(p(4) >= 1, 4, "the smallest party must be at least 1");
                need(p(5) <= p(2), 5, "the largest party must fit a team");
                need(
                    p(4) <= p(5),
                    4,
                    "the smallest party must not exceed the largest",
                );
                need(
                    p(3) >= 0 && p(3) <= p(2) - p(1),
                    3,
                    "the team size difference must be 0..=max-min",
                );
            }
        }
    }
    out
}

impl HopperFile {
    /// Hopper `i`'s whole descriptor.
    pub fn descriptor(&self, i: usize) -> Result<Descriptor, HopperError> {
        let at = self.descriptor_at(i)?;
        let d = &self.image[at..at + DESCRIPTOR];
        Ok(Descriptor {
            hopper_type: le32(d, D_TYPE),
            status: le32(d, D_STATUS),
            playlist_id: le32(d, D_PLAYLIST_ID),
            titles: std::array::from_fn(|l| {
                get_utf8(&d[D_TITLE + l * D_TITLE_LEN..][..D_TITLE_LEN])
            }),
            descriptions: std::array::from_fn(|l| {
                get_utf8(&d[D_DESC + l * D_DESC_LEN..][..D_DESC_LEN])
            }),
            skill_ceiling: std::array::from_fn(|k| d[D_SKILL + k] as i8),
            search_timeout_ms: le32(d, D_SEARCH_TIMEOUT),
            base_wait_ms: le32(d, D_BASE_WAIT),
            start_wait_ms: std::array::from_fn(|k| le32(d, D_START_WAIT + 4 * k)),
            min_rank: le32(d, D_MIN_RANK) as i32,
            list_any_rank: d[D_ANY_RANK] != 0,
            params: std::array::from_fn(|k| le32(d, D_PARAMS + 4 * k)),
        })
    }

    /// Writes every field of hopper `i`'s descriptor. Text slots are
    /// rewritten only when their text changed, and the unused bytes at
    /// `+0x5e9` are left alone, so writing back what [`Self::descriptor`]
    /// read changes nothing. Checks only what the bytes can hold; the
    /// game's own rules are [`check_descriptor`].
    pub fn set_descriptor(&mut self, i: usize, d: &Descriptor) -> Result<(), HopperError> {
        let at = self.descriptor_at(i)?;
        let now = self.descriptor(i)?;
        let image = &mut self.image[at..at + DESCRIPTOR];
        for l in 0..LANGUAGES {
            if d.titles[l] != now.titles[l] {
                put_text(
                    &mut image[D_TITLE + l * D_TITLE_LEN..][..D_TITLE_LEN],
                    &d.titles[l],
                    MAX_TITLE,
                )?;
            }
            if d.descriptions[l] != now.descriptions[l] {
                put_text(
                    &mut image[D_DESC + l * D_DESC_LEN..][..D_DESC_LEN],
                    &d.descriptions[l],
                    MAX_DESCRIPTION,
                )?;
            }
        }
        put_le32(image, D_TYPE, d.hopper_type);
        put_le32(image, D_STATUS, d.status);
        put_le32(image, D_PLAYLIST_ID, d.playlist_id);
        for (k, e) in d.skill_ceiling.iter().enumerate() {
            image[D_SKILL + k] = *e as u8;
        }
        put_le32(image, D_SEARCH_TIMEOUT, d.search_timeout_ms);
        put_le32(image, D_BASE_WAIT, d.base_wait_ms);
        for (k, w) in d.start_wait_ms.iter().enumerate() {
            put_le32(image, D_START_WAIT + 4 * k, *w);
        }
        put_le32(image, D_MIN_RANK, d.min_rank as u32);
        // A flag byte: rewritten only when its truth changes.
        if (image[D_ANY_RANK] != 0) != d.list_any_rank {
            image[D_ANY_RANK] = u8::from(d.list_any_rank);
        }
        for (k, p) in d.params.iter().enumerate() {
            put_le32(image, D_PARAMS + 4 * k, *p);
        }
        Ok(())
    }

    /// Hopper `i`'s entry count (data block `+0x44`).
    pub fn entry_count(&self, i: usize) -> Result<u32, HopperError> {
        let at = self.block_at(i)?;
        Ok(le32(&self.image, at + B_COUNT))
    }

    /// Entry `j` of hopper `i` (any of the 100 slots).
    pub fn entry(&self, i: usize, j: usize) -> Result<Entry, HopperError> {
        let at = self.slot_at(i, j)?;
        Ok(slot_entry(&self.image[at..at + RECORD]))
    }

    /// Hopper `i`'s entries, up to the count (at most 100).
    pub fn entries(&self, i: usize) -> Result<Vec<Entry>, HopperError> {
        let n = (self.entry_count(i)? as usize).min(MAX_RECORDS);
        (0..n).map(|j| self.entry(i, j)).collect()
    }

    /// Writes entry `j`'s fields into its slot; the unused dword at `+0x00`
    /// is left alone, and the map names are written only when the English
    /// one changed.
    pub fn set_entry(&mut self, i: usize, j: usize, e: &Entry) -> Result<(), HopperError> {
        let at = self.slot_at(i, j)?;
        set_slot_entry(&mut self.image[at..at + RECORD], e)
    }

    /// Replaces hopper `i`'s entries: the count, then each slot in order.
    /// Slots past the new count keep their bytes (the game never reads
    /// them).
    pub fn set_entries(&mut self, i: usize, entries: &[Entry]) -> Result<(), HopperError> {
        if entries.len() > MAX_RECORDS {
            return Err(HopperError::Count(entries.len() as u32));
        }
        let block = self.block_at(i)?;
        put_le32(&mut self.image, block + B_COUNT, entries.len() as u32);
        for (j, e) in entries.iter().enumerate() {
            self.set_entry(i, j, e)?;
        }
        Ok(())
    }

    /// Everything the game's readers would object to, hopper by hopper:
    /// the validator for every slot whose type is not 0 (an all-zero slot
    /// is simply no hopper), and for its entries a count of 1..=100, a map
    /// id in `maps` (the game's level list; `0x00194660` marks the hopper
    /// as an error otherwise), a weight session setup takes unclamped, and
    /// a variant the record clean-up keeps.
    pub fn problems(&self, maps: &[u32]) -> Vec<Problem> {
        let mut out = Vec::new();
        for i in 0..HOPPERS {
            let d = self.descriptor(i).expect("index in range");
            if d.hopper_type == 0 {
                continue;
            }
            let base = i * DESCRIPTOR;
            for (at, what) in check_descriptor(&d) {
                out.push(Problem {
                    hopper: i,
                    entry: None,
                    at: base + at,
                    what,
                });
            }
            let n = self.entry_count(i).expect("index in range");
            if !(1..=MAX_RECORDS as u32).contains(&n) {
                out.push(Problem {
                    hopper: i,
                    entry: None,
                    at: BLOCKS_AT + i * BLOCK + B_COUNT,
                    what: format!("{n} entries; the game needs 1..=100"),
                });
            }
            for (j, e) in self
                .entries(i)
                .expect("index in range")
                .into_iter()
                .enumerate()
            {
                let slot = BLOCKS_AT + i * BLOCK + B_ENTRIES + j * RECORD;
                let mut bad = |at: usize, what: String| {
                    out.push(Problem {
                        hopper: i,
                        entry: Some(j),
                        at: slot + at,
                        what,
                    })
                };
                if !maps.contains(&e.map_id) {
                    bad(
                        E_MAP,
                        format!("map id {} is not a multiplayer level", e.map_id),
                    );
                }
                if !WEIGHTS.contains(&e.weight) {
                    bad(
                        E_WEIGHT,
                        format!("weight {} is clamped to 1..=1000", e.weight),
                    );
                }
                for (at, what) in e.variant.problems() {
                    bad(E_VARIANT + at, what);
                }
            }
        }
        out
    }
}

/// An entry read from a slot's 0x37c bytes alone.
///
/// # Panics
/// If `slot` is shorter than [`RECORD`].
pub fn slot_entry(slot: &[u8]) -> Entry {
    let names = &slot[E_MAP_NAMES..][..2 * MAP_NAME_UNITS];
    let units: Vec<u16> = names
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|u| *u != 0)
        .collect();
    Entry {
        map_id: le32(slot, E_MAP),
        weight: le32(slot, E_WEIGHT),
        map_name: String::from_utf16_lossy(&units),
        variant: VariantRecord::from_bytes(&slot[E_VARIANT..RECORD]).expect("0x130 bytes"),
    }
}

/// Writes an entry into a slot's 0x37c bytes, as [`HopperFile::set_entry`]
/// does in place.
pub fn set_slot_entry(slot: &mut [u8], e: &Entry) -> Result<(), HopperError> {
    if slot.len() != RECORD {
        return Err(HopperError::SlotBytes(slot.len()));
    }
    let units: Vec<u16> = e.map_name.encode_utf16().collect();
    if units.len() > MAP_NAME_UNITS - 1 {
        return Err(HopperError::TooLong(e.map_name.clone(), MAP_NAME_UNITS - 1));
    }
    if e.map_name.contains('\0') {
        return Err(HopperError::Nul(e.map_name.clone()));
    }
    if e.map_name != slot_entry(slot).map_name {
        let names = &mut slot[E_MAP_NAMES..E_WEIGHT];
        names.fill(0);
        if !e.map_name.is_empty() {
            for l in 0..LANGUAGES {
                for (k, u) in units.iter().enumerate() {
                    names[l * 2 * MAP_NAME_UNITS + 2 * k..][..2].copy_from_slice(&u.to_le_bytes());
                }
            }
        }
    }
    put_le32(slot, E_MAP, e.map_id);
    put_le32(slot, E_WEIGHT, e.weight);
    slot[E_VARIANT..RECORD].copy_from_slice(e.variant.bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::variant::GameType;
    use super::*;

    /// A descriptor with a different value in every field.
    fn every_field() -> Descriptor {
        let mut d = Descriptor {
            hopper_type: 4,
            status: 3,
            playlist_id: 42,
            titles: std::array::from_fn(|l| format!("Title {l}")),
            descriptions: std::array::from_fn(|l| format!("Description in language {l}")),
            skill_ceiling: std::array::from_fn(|i| (i as i8).saturating_add(3)),
            search_timeout_ms: 123_456,
            base_wait_ms: 7_000,
            start_wait_ms: std::array::from_fn(|k| 1000 + k as u32),
            min_rank: -2,
            list_any_rank: true,
            params: std::array::from_fn(|k| 100 + k as u32),
        };
        d.titles[0] = "Fifteen bytes!!".into();
        d.descriptions[8] = "d".repeat(MAX_DESCRIPTION);
        d
    }

    #[test]
    fn every_descriptor_field_reads_back_from_its_offset() {
        let mut file = HopperFile::blank();
        let d = every_field();
        file.set_descriptor(5, &d).unwrap();
        assert_eq!(file.descriptor(5).unwrap(), d);
        let at = 5 * DESCRIPTOR;
        let img = file.image();
        let dword = |o: usize| le32(img, at + o);
        assert_eq!(dword(0x000), 4);
        assert_eq!(dword(0x004), 3);
        assert_eq!(dword(0x008), 42);
        assert_eq!(&img[at + 0x00c..][..15], b"Fifteen bytes!!");
        assert_eq!(&img[at + 0x01c..][..7], b"Title 1");
        assert_eq!(&img[at + 0x09c..][..25], b"Description in language 0");
        assert_eq!(img[at + 0x51a], b'd');
        assert_eq!(img[at + 0x51b], 0, "the ninth description's NUL is +0x51b");
        assert_eq!(img[at + 0x51c], 3);
        assert_eq!(img[at + 0x59b], 127);
        assert_eq!(dword(0x59c), 123_456);
        assert_eq!(dword(0x5a0), 7_000);
        assert_eq!(dword(0x5a4), 1000);
        assert_eq!(dword(0x5e0), 1015);
        assert_eq!(dword(0x5e4) as i32, -2);
        assert_eq!(img[at + 0x5e8], 1);
        assert_eq!(dword(0x5ec), 100);
        assert_eq!(dword(0x610), 109);
        // Nothing outside the descriptor moved.
        assert!(img[..at].iter().all(|b| *b == 0));
        assert!(img[at + DESCRIPTOR..].iter().all(|b| *b == 0));
    }

    #[test]
    fn every_entry_field_reads_back_from_its_offset() {
        let mut file = HopperFile::blank();
        let mut variant = VariantRecord::stock(GameType::Assault, "Neutral Bomb");
        variant.put_i32(super::super::variant::ROUNDS, 3);
        let e = Entry {
            map_id: 110,
            weight: 999,
            map_name: "Coagulation".into(),
            variant,
        };
        file.set_entries(2, &[e.clone(), e.clone()]).unwrap();
        assert_eq!(file.entry_count(2).unwrap(), 2);
        assert_eq!(file.block_record_count(2).unwrap(), 2);
        assert_eq!(file.entries(2).unwrap(), vec![e.clone(), e.clone()]);
        let slot = BLOCKS_AT + 2 * BLOCK + B_ENTRIES + RECORD;
        let img = file.image();
        assert_eq!(le32(img, BLOCKS_AT + 2 * BLOCK + 0x44), 2);
        assert_eq!(le32(img, slot), 0, "+0x00 is left alone");
        assert_eq!(le32(img, slot + 0x04), 110);
        assert_eq!(&img[slot + 0x08..][..4], &[b'C', 0, b'o', 0]);
        assert_eq!(
            &img[slot + 0x08 + 8 * 0x40..][..2],
            &[b'C', 0],
            "all nine slots"
        );
        assert_eq!(le32(img, slot + 0x248), 999);
        assert_eq!(&img[slot + 0x24c..slot + RECORD], e.variant.bytes());
        // The old record view sees the same entry.
        let r = file.record(2, 1).unwrap();
        assert_eq!((r.id, r.dwords[0]), (110, 999));
        assert_eq!(file.variant(2, 1).unwrap().name, "Neutral Bomb");
        assert_eq!(file.variant(2, 1).unwrap().engine, 9);

        let mut long = e.clone();
        long.map_name = "m".repeat(MAP_NAME_UNITS);
        assert!(file.set_entry(2, 0, &long).is_err());
        assert!(file.set_entries(2, &vec![e; MAX_RECORDS + 1]).is_err());
    }

    #[test]
    fn writing_back_what_was_read_keeps_every_byte() {
        // Synthetic noise everywhere, unknown bytes included.
        let mut file = HopperFile::blank();
        for (k, b) in file.image_mut().iter_mut().enumerate() {
            *b = (k.wrapping_mul(131) % 251) as u8;
        }
        let before = file.image().to_vec();
        for i in 0..HOPPERS {
            let d = file.descriptor(i).unwrap();
            file.set_descriptor(i, &d).unwrap();
            for j in 0..MAX_RECORDS {
                let at = BLOCKS_AT + i * BLOCK + B_ENTRIES + j * RECORD;
                // An over-long name (no NUL in 32 units) cannot be written;
                // leave the slot alone, as an editor would.
                if let Ok(()) = set_slot_entry(
                    &mut file.image_mut()[at..at + RECORD].to_vec(),
                    &file.entry(i, j).unwrap(),
                ) {
                    let e = file.entry(i, j).unwrap();
                    file.set_entry(i, j, &e).unwrap();
                }
            }
        }
        assert!(
            file.image() == &before[..],
            "a read-write round trip changed bytes"
        );
        let back = HopperFile::parse(&file.build()).unwrap();
        assert!(back.image() == &before[..]);
    }

    #[test]
    fn the_validator_port_takes_the_games_own_default_hoppers() {
        // The seven hoppers the game builds when it has no file
        // (`0x00193c70`): type, playlist id and parameters.
        let games: [(u32, u32, [u32; PARAMS]); 7] = [
            (2, 1, [4, 8, 0, 0, 0, 0, 0, 0, 0, 0]),
            (4, 2, [2, 3, 4, 0, 1, 4, 1, 3, 4, 0]),
            (2, 3, [2, 2, 0, 0, 0, 0, 0, 0, 0, 0]),
            (4, 4, [2, 5, 8, 0, 1, 5, 0, 3, 5, 2]),
            (5, 25, [2, 3, 4, 0, 0, 0, 0, 0, 0, 0]),
            (5, 26, [2, 6, 8, 2, 0, 0, 0, 0, 0, 0]),
            (3, 5, [2, 2, 4, 1, 0, 0, 0, 0, 0, 0]),
        ];
        for (t, id, params) in games {
            let d = Descriptor {
                hopper_type: t,
                playlist_id: id,
                skill_ceiling: default_skill_ceiling(),
                params,
                ..Descriptor::default()
            };
            assert_eq!(check_descriptor(&d), vec![], "type {t} id {id}");
            let r = d.rules().unwrap();
            assert_eq!(Rules::from_params(r.hopper_type(), &r.params()), r);
        }
    }

    fn valid(t: HopperType) -> Descriptor {
        let rules = match t {
            HopperType::UnrankedFreeForAll | HopperType::RankedFreeForAll => Rules::FreeForAll {
                ranked: t.ranked(),
                players: (2, 8),
            },
            HopperType::UnrankedTeams => Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 4),
                uneven: true,
            },
            HopperType::RankedTeams => Rules::RankedTeams {
                teams: 4,
                team_size: (2, 4),
                max_difference: 1,
                party: (1, 3),
                party_as_team: Some((3, 4)),
            },
            HopperType::Clan => Rules::Clan {
                teams: 2,
                team_size: (4, 8),
                max_difference: 0,
            },
        };
        let mut d = Descriptor {
            playlist_id: 9,
            skill_ceiling: default_skill_ceiling(),
            ..Descriptor::default()
        };
        d.set_rules(&rules);
        assert_eq!(d.rules(), Some(rules));
        assert_eq!(check_descriptor(&d), vec![], "{t:?}");
        d
    }

    fn fails_at(d: &Descriptor) -> Vec<usize> {
        check_descriptor(d).into_iter().map(|(at, _)| at).collect()
    }

    #[test]
    fn the_validator_port_enforces_each_rule() {
        let p = |i: usize| D_PARAMS + 4 * i;
        let mut d = valid(HopperType::RankedFreeForAll);
        d.hopper_type = 6;
        assert_eq!(fails_at(&d), vec![D_TYPE]);
        d.hopper_type = 0;
        assert_eq!(fails_at(&d), vec![D_TYPE]);
        let mut d = valid(HopperType::RankedFreeForAll);
        d.status = 4;
        d.playlist_id = 101;
        d.skill_ceiling[7] = 6;
        assert_eq!(fails_at(&d), vec![D_STATUS, D_PLAYLIST_ID, D_SKILL + 7]);
        d = valid(HopperType::RankedFreeForAll);
        d.playlist_id = 0;
        d.params[0] = 0;
        d.params[1] = 17;
        assert_eq!(fails_at(&d), vec![D_PLAYLIST_ID, p(0), p(1)]);
        d = valid(HopperType::UnrankedFreeForAll);
        d.params[0] = 9;
        assert_eq!(fails_at(&d), vec![p(0)]);

        for t in [HopperType::UnrankedTeams, HopperType::Clan] {
            let mut d = valid(t);
            d.params[0] = 1;
            assert_eq!(fails_at(&d), vec![p(0), p(2)], "{t:?}");
            d = valid(t);
            d.params[0] = 4; // 16 / 4 = 4 a team
            d.params[1] = 1;
            d.params[2] = 5;
            assert_eq!(fails_at(&d), vec![p(2)], "{t:?}");
            d.params[2] = 4;
            d.params[1] = 0;
            assert_eq!(fails_at(&d), vec![p(1)], "{t:?}");
            d.params[1] = 4;
            d.params[3] = 99; // not validated for these types
            assert_eq!(fails_at(&d), Vec::<usize>::new(), "{t:?}");
        }

        let mut d = valid(HopperType::RankedTeams);
        d.params[1] = 1; // ranked teams need 2 a team
        assert_eq!(fails_at(&d), vec![p(1)]);
        d = valid(HopperType::RankedTeams);
        d.params[3] = 3; // more than max - min
        assert_eq!(fails_at(&d), vec![p(3)]);
        d = valid(HopperType::RankedTeams);
        d.params[4] = 0;
        assert_eq!(fails_at(&d), vec![p(4)]);
        d = valid(HopperType::RankedTeams);
        d.params[5] = 5;
        assert_eq!(fails_at(&d), vec![p(5)]);
        d.params[5] = 2;
        d.params[4] = 3;
        assert_eq!(fails_at(&d), vec![p(4)]);
        d = valid(HopperType::RankedTeams);
        d.params[7] = 99; // party-as-team sizes are not validated
        d.params[9] = 99;
        assert_eq!(fails_at(&d), Vec::<usize>::new());
    }

    #[test]
    fn the_accessors_answer_as_the_games_do() {
        let ffa = valid(HopperType::RankedFreeForAll);
        assert_eq!((ffa.min_players(), ffa.max_players()), (Some(2), Some(8)));
        assert_eq!((ffa.min_party(), ffa.max_party()), (Some(1), Some(1)));
        assert_eq!((ffa.team_count(), ffa.max_difference()), (None, Some(0)));
        assert_eq!(
            (ffa.min_team_size(), ffa.max_team_size()),
            (Some(1), Some(1))
        );
        let unranked = valid(HopperType::UnrankedFreeForAll);
        assert_eq!(unranked.max_party(), Some(8));
        let teams = valid(HopperType::UnrankedTeams);
        assert_eq!(
            (teams.min_players(), teams.max_players()),
            (Some(2), Some(8))
        );
        assert_eq!(
            (teams.max_party(), teams.max_difference()),
            (Some(8), Some(1))
        );
        assert_eq!(teams.party_as_team(), None);
        let ranked = valid(HopperType::RankedTeams);
        assert_eq!(
            (ranked.min_players(), ranked.max_players()),
            (Some(8), Some(16))
        );
        assert_eq!((ranked.min_party(), ranked.max_party()), (Some(1), Some(3)));
        assert_eq!(
            (ranked.team_count(), ranked.party_as_team()),
            (Some(4), Some((3, 4)))
        );
        let clan = valid(HopperType::Clan);
        assert_eq!((clan.min_party(), clan.max_party()), (Some(4), Some(8)));
        assert_eq!(clan.party_as_team(), Some((4, 8)));
        assert!(HopperType::Clan.ranked() && HopperType::Clan.teams());
        assert!(!HopperType::UnrankedTeams.ranked() && !HopperType::RankedFreeForAll.teams());
        assert_eq!(Descriptor::default().min_players(), None);
        for t in 1..=5 {
            assert_eq!(HopperType::from_raw(t).unwrap().raw(), t);
        }
    }

    #[test]
    fn start_waits_fall_linearly_to_the_most_players() {
        let w = start_waits(30_000, 1, (4, 8));
        assert_eq!(&w[..3], &[NEVER; 3]);
        assert_eq!(&w[3..8], &[30_000, 22_500, 15_000, 7_500, 0]);
        assert_eq!(&w[8..], &[NEVER; 8]);
        // Teams: whole teams step the wait (n / teams).
        let w = start_waits(45_000, 2, (5, 8));
        assert_eq!(w[9], 45_000);
        assert_eq!(w[10], 45_000, "11 players is still 5 a team");
        assert_eq!(w[11], 30_000);
        assert_eq!(w[15], 0);
        assert_eq!(w[8], NEVER);
        // One size only: start at once.
        let w = start_waits(30_000, 2, (2, 2));
        assert_eq!((w[2], w[3], w[4]), (NEVER, 0, NEVER));

        let mut d = valid(HopperType::RankedFreeForAll);
        d.start_wait_ms = start_waits(30_000, 1, (2, 8));
        assert_eq!(d.start_wait(1), Some(NEVER));
        assert_eq!(d.start_wait(2), Some(30_000));
        assert_eq!(d.start_wait(8), Some(0));
        assert_eq!(d.start_wait(9), Some(NEVER));
    }

    #[test]
    fn the_skill_ceiling_keeps_the_validators_order() {
        let t = default_skill_ceiling();
        assert_eq!((t[0], t[5], t[6], t[36], t[127]), (5, 10, 12, 46, 127));
        assert!(t.iter().enumerate().all(|(i, e)| *e as usize >= i));
        let d = Descriptor {
            skill_ceiling: t,
            ..Descriptor::default()
        };
        // Walking down from skill 20 while the ceiling still reaches 20.
        assert_eq!(d.lowest_matched(20), 14);
        assert_eq!(d.lowest_matched(3), 0);
        assert_eq!(d.lowest_matched(0), 0);
    }
}
