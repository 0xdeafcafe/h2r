//! The 0x130-byte game variant record (the variant at `G+0x13c` in
//! docs/live/live-gamestate.md, the payload of a saved variant, the tail of a
//! hopper record slot).
//!
//! The record's bytes are the source of truth: [`GameVariant`] wraps them and
//! reads and writes typed values in place, so bytes nobody models survive
//! every edit and `GameVariant::from_bytes(b).to_bytes() == b`.
//!
//! Where each setting lives was read from the game's setting writer
//! (`0x2373be`, the code the editor and the built-in loader both use): it
//! asks a per-setting descriptor (a function from a 0x70-entry pointer
//! table at `0x470828`) for a storage kind, a record offset and a bit or
//! scale, then stores the value. Every scale in this build is 1. [`field`]
//! is our encoding of those answers; the `#[ignore]` test
//! `descriptors_match_the_xbe` decodes the descriptors from `$H2_XBE` and
//! checks it entry by entry. The limits in [`check`] are what the game's
//! record clean-up (`0x19d650`) leaves alone (docs/editor/variants.md).

use std::fmt;

pub const RECORD_SIZE: usize = 0x130;
/// UTF-16 name at `+4`, 32 units; the clean-up zeroes the last one.
pub const NAME_AT: usize = 0x04;
pub const NAME_UNITS: usize = 32;
pub const MAX_NAME_UNITS: usize = NAME_UNITS - 1;
/// Byte `+0` bit 0: the built-in flag (cleared when the game copies a
/// built-in for editing, `0x212b2c`; inferred).
pub const HEADER_FLAGS_AT: usize = 0x00;
/// Byte `+3`: the built-in description number (0..=100).
pub const DESCRIPTION_AT: usize = 0x03;
/// The game-engine index (`i32`).
pub const ENGINE_AT: usize = 0x44;
/// Engine flags (`u32`), bit 0 team play; settings keep booleans here.
pub const FLAGS_AT: usize = 0x48;
/// The game-type-specific area, `0xf0..0x10c`; its meaning depends on
/// the engine index.
pub const TYPE_AREA: std::ops::Range<usize> = 0xf0..0x10c;

/// A game type by its game-engine index (record `+0x44`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GameType {
    Ctf,
    Slayer,
    Oddball,
    King,
    Juggernaut,
    Territories,
    Assault,
    /// Any other engine index (5 Race and 6 Headhunter are in the engine
    /// but the clean-up rejects them).
    Other(i32),
}

impl GameType {
    /// The seven types the variant editor offers, in the game's list order
    /// (the built-in entries' type numbers 0..=6, `0x216ab0`).
    pub const ALL: [GameType; 7] = [
        GameType::Slayer,
        GameType::Oddball,
        GameType::Juggernaut,
        GameType::King,
        GameType::Ctf,
        GameType::Assault,
        GameType::Territories,
    ];

    pub fn from_engine(e: i32) -> Self {
        match e {
            1 => Self::Ctf,
            2 => Self::Slayer,
            3 => Self::Oddball,
            4 => Self::King,
            7 => Self::Juggernaut,
            8 => Self::Territories,
            9 => Self::Assault,
            n => Self::Other(n),
        }
    }

    pub fn engine(self) -> i32 {
        match self {
            Self::Ctf => 1,
            Self::Slayer => 2,
            Self::Oddball => 3,
            Self::King => 4,
            Self::Juggernaut => 7,
            Self::Territories => 8,
            Self::Assault => 9,
            Self::Other(n) => n,
        }
    }

    /// The type number of a built-in entry in the `goof` tag (0..=6).
    pub fn from_list_index(i: u32) -> Option<Self> {
        Self::ALL.get(i as usize).copied()
    }

    /// The save-content type (`0x216b60`): which file a saved variant of
    /// this type is stored in.
    pub fn content_type(self) -> Option<u32> {
        Some(match self {
            Self::Slayer => 1,
            Self::King => 2,
            Self::Oddball => 4,
            Self::Juggernaut => 5,
            Self::Ctf => 7,
            Self::Assault => 8,
            Self::Territories => 9,
            Self::Other(_) => return None,
        })
    }

    pub fn from_content_type(t: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.content_type() == Some(t))
    }

    /// Our name for the type (the game's own label is read from the disc).
    pub fn label(self) -> String {
        match self {
            Self::Ctf => "Capture the Flag".into(),
            Self::Slayer => "Slayer".into(),
            Self::Oddball => "Oddball".into(),
            Self::King => "King of the Hill".into(),
            Self::Juggernaut => "Juggernaut".into(),
            Self::Territories => "Territories".into(),
            Self::Assault => "Assault".into(),
            Self::Other(n) => format!("Engine {n}"),
        }
    }

    /// The setting that holds this type's score to win (all write `+0x50`).
    pub fn score_setting(self) -> Option<SettingId> {
        Some(SettingId(match self {
            Self::Ctf => 1,
            Self::Slayer => 2,
            Self::Oddball => 3,
            Self::King => 4,
            Self::Juggernaut => 7,
            Self::Territories => 8,
            Self::Assault => 9,
            Self::Other(_) => return None,
        }))
    }
}

impl fmt::Display for GameType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// A setting number, as the `sily` definitions and the built-in entries'
/// `(setting, value)` pairs carry it (0..0x70).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct SettingId(pub u32);

/// How the setting writer stores a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One bit of a `u16`.
    Bit16(u8),
    /// One bit of a `u32`.
    Bit32(u8),
    /// A byte, 0 or 1.
    Bool8,
    U8,
    I16,
    I32,
}

/// Where a setting lives in the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub offset: usize,
    pub kind: Kind,
}

const fn f(offset: usize, kind: Kind) -> Option<Field> {
    Some(Field { offset, kind })
}

/// Where setting `id` is stored, or `None` for numbers the writer ignores.
/// See the module notes for the evidence.
pub fn field(id: SettingId) -> Option<Field> {
    use Kind::*;
    let flag = |bit| f(FLAGS_AT, Bit32(bit));
    let ty = |bit| f(0xf0, Bit32(bit));
    match id.0 {
        0 => f(0x4c, I32),
        1..=9 => f(0x50, I32),
        10 => f(0x54, I32),
        11 => flag(3),
        12 => flag(4),
        13 => flag(5),
        14 => f(0x58, I32),
        15 => f(0x74, I32),
        16 => f(0x7c, I32),
        17 => f(0x80, I32),
        18 => f(0x84, I32),
        19 => f(0x88, I32),
        20 => flag(1),
        21 => flag(2),
        22 => flag(6),
        23 => f(0xa4, I32),
        24 => flag(7),
        25 => f(0xa8, I32),
        26 => f(0xac, I32),
        28 | 38 | 46 | 73 => ty(0),
        29 | 39 | 51 | 60 | 71 | 100 => ty(1),
        30 | 41 | 52 | 72 | 92 | 102 => ty(2),
        42 | 53 | 62 | 68 | 103 => ty(3),
        57 | 66 | 69 | 101 => ty(4),
        70 | 97 => ty(5),
        106 | 108 | 109 => ty(6),
        107 | 110 => ty(7),
        40 | 43 | 96 | 99 => f(0xf4, I16),
        44 => f(0xf6, I16),
        45 => f(0xf8, I16),
        47 => f(0xfa, I16),
        50 | 59 => f(0x104, I32),
        54 | 61 => f(0xf4, I32),
        55 | 64 => f(0xf8, I32),
        56 | 65 => f(0xfc, I32),
        58 | 67 => f(0x100, I32),
        63 => f(0x108, I16),
        95 => f(0x10a, I16),
        74 => f(0xf0, I16),
        98 => f(0xf2, I16),
        75..=83 => f(0xcc + (id.0 as usize - 75), U8),
        84 => flag(8),
        85 => flag(9),
        86 => flag(10),
        87 => f(0xd5, U8),
        88 => flag(11),
        89 => f(0xd6, U8),
        90 => f(0x78, I32),
        91 => flag(0),
        93 => f(0xb4, I32),
        94 => f(0xd7, U8),
        104 => flag(12),
        105 => flag(13),
        111 => flag(14),
        _ => None,
    }
}

/// Every setting number the writer stores.
pub fn all_settings() -> impl Iterator<Item = SettingId> {
    (0..0x70).map(SettingId).filter(|s| field(*s).is_some())
}

/// Our names for the settings we can identify (from the community variant
/// model in `crates/extensions/game/src/community_variants.rs` and the engine fields
/// in docs/live/live-gamestate.md). The editor shows the disc's own labels when
/// it has them; these are the fallback.
pub fn symbolic_name(id: SettingId) -> Option<&'static str> {
    Some(match id.0 {
        0 => "Rounds",
        1..=9 => "Score to win",
        10 => "Time limit (seconds)",
        14 => "Join in progress",
        16 => "Lives per round",
        17 => "Respawn time (seconds)",
        18 => "Suicide penalty (seconds)",
        19 => "Shields",
        20 => "Motion sensor",
        21 => "Active camo",
        24 => "Friendly fire",
        40 => "Moving hill (seconds)",
        45 => "Ball speed",
        71 => "Juggernaut overshield",
        72 => "Juggernaut active camo",
        73 => "Juggernaut motion sensor",
        76 => "Light vehicle",
        77 => "Second light vehicle",
        78 => "Heavy vehicle",
        79 => "Flying vehicle",
        81 => "Primary turret",
        82 => "Secondary turret",
        83 => "Weapons on map",
        84 => "Overshield pickups",
        85 => "Active camo pickups",
        86 => "Grenades on map",
        87 => "Weapon respawn",
        88 => "Starting grenades",
        89 => "Starting weapon",
        90 => "Max players",
        91 => "Team play",
        94 => "Secondary weapon",
        96 => "Juggernaut movement",
        102 => "Ball toughness",
        104 => "Extra damage",
        105 => "Damage resistance",
        108 => "Juggernaut damage resistance",
        _ => return None,
    })
}

/// The settings stored in the game-type area belong to one game type each:
/// the one whose type page in the `goof` tag lists them (read from the
/// retail tag's pages; the `#[ignore]` disc test checks it). Settings
/// outside the area apply to every type.
pub fn fallback_owner(id: SettingId) -> Option<GameType> {
    let f = field(id)?;
    if !TYPE_AREA.contains(&f.offset) {
        return None;
    }
    Some(match id.0 {
        28..=30 => GameType::Slayer,
        43..=47 | 100 | 102 => GameType::Oddball,
        38..=42 | 101 => GameType::King,
        50..=58 | 106 | 107 => GameType::Ctf,
        59..=67 | 92 | 95 | 97 | 109 | 110 => GameType::Assault,
        68..=73 | 96 | 108 => GameType::Juggernaut,
        74 | 98 | 99 => GameType::Territories,
        // 103 is stored but on no page.
        _ => GameType::Other(-1),
    })
}

/// One game variant: the record bytes plus typed access.
#[derive(Clone, PartialEq, Eq)]
pub struct GameVariant {
    raw: [u8; RECORD_SIZE],
}

impl fmt::Debug for GameVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GameVariant")
            .field("name", &self.name())
            .field("game_type", &self.game_type())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecordError {
    #[error("a game variant record is {RECORD_SIZE:#x} bytes, got {0:#x}")]
    Size(usize),
    #[error("the name is longer than {MAX_NAME_UNITS} UTF-16 units")]
    NameTooLong,
    #[error("setting {0} has no place in the record")]
    UnknownSetting(u32),
    #[error("setting {id} cannot hold {value}")]
    OutOfRange { id: u32, value: i32 },
    #[error("the {0} option is out of range")]
    ExtRange(&'static str),
}

impl GameVariant {
    /// A zeroed record of `ty` (what the game starts from before applying
    /// a type's defaults; see [`crate::schema::Schema::new_variant`]).
    pub fn blank(ty: GameType) -> Self {
        let mut v = Self {
            raw: [0; RECORD_SIZE],
        };
        v.set_game_type(ty);
        v
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, RecordError> {
        let raw: [u8; RECORD_SIZE] = b.try_into().map_err(|_| RecordError::Size(b.len()))?;
        Ok(Self { raw })
    }

    pub fn to_bytes(&self) -> [u8; RECORD_SIZE] {
        self.raw
    }

    pub fn bytes(&self) -> &[u8; RECORD_SIZE] {
        &self.raw
    }

    /// Our extension block's bytes (`+0x10c..+0x12f`; [`crate::ext`]).
    pub fn ext_block(&self) -> &[u8; crate::ext::LEN] {
        self.raw[crate::ext::AT..crate::ext::AT + crate::ext::LEN]
            .try_into()
            .unwrap()
    }

    /// What the extension block holds.
    pub fn ext_state(&self) -> crate::ext::Block {
        crate::ext::Ext::decode(self.ext_block())
    }

    /// The extension options in effect (the game's own when the block is
    /// absent or ignored).
    pub fn ext(&self) -> crate::ext::Ext {
        crate::ext::Ext::in_effect(self.ext_block())
    }

    /// Writes the extension block; the default clears it to zero.
    pub fn set_ext(&mut self, e: &crate::ext::Ext) -> Result<(), RecordError> {
        if let Some(what) = e.out_of_range() {
            return Err(RecordError::ExtRange(what));
        }
        self.put(crate::ext::AT, &e.encode());
        Ok(())
    }

    fn u16_at(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.raw[at], self.raw[at + 1]])
    }

    fn u32_at(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.raw[at..at + 4].try_into().unwrap())
    }

    fn put(&mut self, at: usize, b: &[u8]) {
        self.raw[at..at + b.len()].copy_from_slice(b);
    }

    /// The name up to its first NUL.
    pub fn name(&self) -> String {
        String::from_utf16_lossy(&self.name_units())
    }

    fn name_units(&self) -> Vec<u16> {
        (0..NAME_UNITS)
            .map(|i| self.u16_at(NAME_AT + 2 * i))
            .take_while(|u| *u != 0)
            .collect()
    }

    /// Sets the name, NUL-filling the rest of the field (as the game's
    /// copy does). Unchanged names leave the field's bytes alone.
    pub fn set_name(&mut self, name: &str) -> Result<(), RecordError> {
        let units: Vec<u16> = name.encode_utf16().collect();
        if units == self.name_units() {
            return Ok(());
        }
        if units.len() > MAX_NAME_UNITS {
            return Err(RecordError::NameTooLong);
        }
        for i in 0..NAME_UNITS {
            let u = units.get(i).copied().unwrap_or(0);
            self.put(NAME_AT + 2 * i, &u.to_le_bytes());
        }
        Ok(())
    }

    pub fn game_type(&self) -> GameType {
        GameType::from_engine(self.u32_at(ENGINE_AT) as i32)
    }

    pub fn set_game_type(&mut self, ty: GameType) {
        self.put(ENGINE_AT, &ty.engine().to_le_bytes());
    }

    pub fn description(&self) -> u8 {
        self.raw[DESCRIPTION_AT]
    }

    pub fn set_description(&mut self, n: u8) {
        self.raw[DESCRIPTION_AT] = n;
    }

    /// Byte `+0` bit 0 (inferred: the built-in flag).
    pub fn builtin_flag(&self) -> bool {
        self.raw[HEADER_FLAGS_AT] & 1 != 0
    }

    pub fn set_builtin_flag(&mut self, on: bool) {
        self.raw[HEADER_FLAGS_AT] = (self.raw[HEADER_FLAGS_AT] & !1) | u8::from(on);
    }

    pub fn team_play(&self) -> bool {
        self.u32_at(FLAGS_AT) & 1 != 0
    }

    /// The stored value of setting `id`, as the editor's value numbers.
    pub fn get(&self, id: SettingId) -> Option<i32> {
        let f = field(id)?;
        let at = f.offset;
        Some(match f.kind {
            Kind::Bit16(b) => i32::from(self.u16_at(at) >> b & 1),
            Kind::Bit32(b) => (self.u32_at(at) >> b & 1) as i32,
            Kind::Bool8 | Kind::U8 => i32::from(self.raw[at]),
            Kind::I16 => i32::from(self.u16_at(at) as i16),
            Kind::I32 => self.u32_at(at) as i32,
        })
    }

    /// Stores `value` for setting `id` as the game's writer would (a bit,
    /// a byte or an integer at the setting's place).
    pub fn set(&mut self, id: SettingId, value: i32) -> Result<(), RecordError> {
        let f = field(id).ok_or(RecordError::UnknownSetting(id.0))?;
        let at = f.offset;
        let range = |lo: i32, hi: i32| {
            if (lo..=hi).contains(&value) {
                Ok(())
            } else {
                Err(RecordError::OutOfRange { id: id.0, value })
            }
        };
        match f.kind {
            Kind::Bit16(b) => {
                range(0, 1)?;
                let w = self.u16_at(at) & !(1 << b) | (value as u16) << b;
                self.put(at, &w.to_le_bytes());
            }
            Kind::Bit32(b) => {
                range(0, 1)?;
                let w = self.u32_at(at) & !(1 << b) | (value as u32) << b;
                self.put(at, &w.to_le_bytes());
            }
            Kind::Bool8 => {
                range(0, 1)?;
                self.raw[at] = value as u8;
            }
            Kind::U8 => {
                range(0, 255)?;
                self.raw[at] = value as u8;
            }
            Kind::I16 => {
                range(i16::MIN.into(), i16::MAX.into())?;
                self.put(at, &(value as i16).to_le_bytes());
            }
            Kind::I32 => self.put(at, &value.to_le_bytes()),
        }
        Ok(())
    }

    /// The settings that apply to this variant's game type, with values:
    /// every setting outside the game-type area, the area's settings of
    /// this type (by [`fallback_owner`]) and its own score setting only.
    pub fn known_settings(&self) -> Vec<(SettingId, i32)> {
        let ty = self.game_type();
        all_settings()
            .filter(|s| match fallback_owner(*s) {
                Some(owner) => owner == ty,
                None => !(1..=9).contains(&s.0) || ty.score_setting() == Some(*s),
            })
            .filter_map(|s| Some((s, self.get(s)?)))
            .collect()
    }
}

/// A rule the game's record clean-up enforces; a record that breaks one is
/// replaced by the clean-up's default (so the game would reject the load).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub offset: usize,
    pub what: String,
}

/// The clean-up's limits (`0x19d650`; docs/editor/variants.md lists them with
/// their evidence). Returns every broken rule; empty means the game keeps
/// the record as it is.
pub fn check(v: &GameVariant) -> Vec<Problem> {
    let mut c = Checker { v, out: Vec::new() };
    if v.raw[0] > 1 || v.raw[1] != 0 {
        c.bad(0, "the header word may only have bit 0".into());
    }
    if v.u16_at(NAME_AT + 2 * MAX_NAME_UNITS) != 0 {
        c.bad(
            NAME_AT + 2 * MAX_NAME_UNITS,
            "the name's last unit must be 0".into(),
        );
    }
    let engine = v.u32_at(ENGINE_AT) as i32;
    if !matches!(engine, 1..=4 | 7..=9) {
        c.bad(
            ENGINE_AT,
            format!("engine {engine} is not a playable game type"),
        );
    }
    if v.u32_at(FLAGS_AT) & !0x7fff != 0 {
        c.bad(FLAGS_AT, "flags above bit 14 are set".into());
    }
    c.i32_in(0x4c, 0, 6);
    for at in [0x50, 0x54, 0x7c, 0x80, 0x84, 0xac] {
        c.i32_in(at, 0, 0x7fff);
    }
    for at in [0x58, 0x88, 0xa4, 0xa8] {
        c.i32_in(at, 0, 2);
    }
    for at in [0x74, 0x78] {
        c.i32_in(at, 0, 16);
    }
    if v.u32_at(0xb0) != 0 {
        c.bad(0xb0, "must be 0".into());
    }
    c.i32_in(0xb4, 0, 8);
    for (at, hi) in [
        (0xcc, 3),
        (0xcd, 7),
        (0xce, 7),
        (0xcf, 4),
        (0xd0, 4),
        (0xd1, 4),
        (0xd2, 6),
        (0xd3, 6),
        (0xd4, 0x13),
        (0xd5, 3),
        (0xd6, 0x14),
        (0xd7, 0x14),
    ] {
        if v.raw[at] > hi {
            c.bad(at, format!("{} is above {hi}", v.raw[at]));
        }
    }
    match engine {
        1 => c.ctf(),
        9 => {
            c.i16_in(0x108, 0, 15);
            c.i16_in(0x10a, 0, 15);
            c.ctf();
        }
        2 => c.mask(7),
        3 => {
            c.mask(7);
            c.i16_in(0xf4, 0, 3);
            c.i16_in(0xf6, 0, 1);
            c.i16_in(0xf8, 0, 2);
            c.i16_in(0xfa, 0, 3);
        }
        4 => {
            c.mask(0x1f);
            c.i16_in(0xf4, 0, 0x7fff);
        }
        7 => {
            if v.team_play() {
                c.bad(FLAGS_AT, "Juggernaut is never team play".into());
            }
            c.mask(0x7f);
            c.i16_in(0xf4, 0, 2);
        }
        8 => {
            c.i16_in(0xf0, 1, 8);
            c.i16_in(0xf2, 1, 0x7fff);
            c.i16_in(0xf4, 1, 0x7fff);
        }
        _ => {}
    }
    c.out
}

/// Team count (setting 93, `+0xb4`): how many teams the game makes. The
/// team set builder (`0x157790`) reads it from the active record
/// (`G+0x1f0`), takes 0 as 8, caps it at the teams the map allows
/// (`[[0x4e9ae8]+4]`, a mask) and fills the lowest unused teams up to it.
/// The retail editor has no row for it (docs/game/game-options.md).
pub const TEAM_COUNT: SettingId = SettingId(93);
/// The team counts our editors offer: 0 (the map's own, up to 8), then 2
/// to 8. One team is storable but not offered.
pub const TEAM_COUNTS: [i32; 8] = [0, 2, 3, 4, 5, 6, 7, 8];

/// Respawn time (setting 17).
pub const RESPAWN: SettingId = SettingId(17);
/// Respawn time modifier (setting 25): 0 inheritance, 1 cycling, 2 none.
pub const RESPAWN_MODIFIER: SettingId = SettingId(25);
/// The modifier's cycling value.
pub const CYCLING: i32 = 1;

/// Our own rules, beyond the game's clean-up: records the clean-up keeps
/// but the game cannot play (docs/game/game-options.md). Cycling rounds
/// each wait up to a multiple of the respawn time, dividing by it
/// (`0x15d11f`): with a respawn time of 0 the host divides by zero.
pub fn ours(v: &GameVariant) -> Vec<Problem> {
    let mut out = Vec::new();
    if v.get(RESPAWN) == Some(0) && v.get(RESPAWN_MODIFIER) == Some(CYCLING) {
        out.push(Problem {
            offset: field(RESPAWN).unwrap().offset,
            what: "a respawn time of 0 with cycling respawns divides by zero in the game".into(),
        });
    }
    out
}

/// Everything our editors refuse to save or link: the clean-up's rules,
/// then [`ours`].
pub fn refused(v: &GameVariant) -> Vec<Problem> {
    let mut out = check(v);
    out.extend(ours(v));
    out
}

struct Checker<'a> {
    v: &'a GameVariant,
    out: Vec<Problem>,
}

impl Checker<'_> {
    fn bad(&mut self, offset: usize, what: String) {
        self.out.push(Problem { offset, what });
    }

    fn i32_in(&mut self, at: usize, lo: i32, hi: i32) {
        let x = self.v.u32_at(at) as i32;
        if !(lo..=hi).contains(&x) {
            self.bad(at, format!("{x} is outside {lo}..={hi}"));
        }
    }

    fn i16_in(&mut self, at: usize, lo: i16, hi: i16) {
        let x = self.v.u16_at(at) as i16;
        if !(lo..=hi).contains(&x) {
            self.bad(at, format!("{x} is outside {lo}..={hi}"));
        }
    }

    /// The game-type flags at `+0xf0` within `m`.
    fn mask(&mut self, m: u32) {
        if self.v.u32_at(0xf0) & !m != 0 {
            self.bad(0xf0, format!("game-type flags outside {m:#x}"));
        }
    }

    /// CTF's rules, which Assault shares.
    fn ctf(&mut self) {
        if !self.v.team_play() {
            self.bad(FLAGS_AT, "this game type is always team play".into());
        }
        self.mask(0xff);
        self.i32_in(0xf4, 0, 0x7fff);
        self.i32_in(0xf8, 0, 2);
        self.i32_in(0xfc, 0, 1);
        self.i32_in(0x100, 0, 3);
        self.i32_in(0x104, 0, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy() -> [u8; RECORD_SIZE] {
        let mut b = [0u8; RECORD_SIZE];
        for (i, x) in b.iter_mut().enumerate() {
            *x = (i as u8).wrapping_mul(37).wrapping_add(11);
        }
        // A valid UTF-16 name of 20 units, then noise after its NUL.
        for i in 0..20 {
            b[NAME_AT + 2 * i..NAME_AT + 2 * i + 2]
                .copy_from_slice(&(u16::from(b'a') + i as u16).to_le_bytes());
        }
        b[NAME_AT + 40..NAME_AT + 42].fill(0);
        b
    }

    #[test]
    fn round_trip_is_exact() {
        let b = noisy();
        let v = GameVariant::from_bytes(&b).unwrap();
        assert_eq!(v.to_bytes(), b);
        assert_eq!(
            GameVariant::from_bytes(&b[..0x12f]),
            Err(RecordError::Size(0x12f))
        );
    }

    #[test]
    fn setting_every_value_back_changes_nothing() {
        let b = noisy();
        let mut v = GameVariant::from_bytes(&b).unwrap();
        for s in all_settings() {
            let x = v.get(s).unwrap();
            // A u8 or bit reads back in range; i16/i32 any value.
            v.set(s, x).unwrap();
        }
        let name = v.name();
        v.set_name(&name).unwrap();
        assert_eq!(v.to_bytes(), b, "rewriting unchanged values is a no-op");
    }

    #[test]
    fn writes_touch_only_their_field() {
        let b = noisy();
        for s in all_settings() {
            let mut v = GameVariant::from_bytes(&b).unwrap();
            let f = field(s).unwrap();
            let x = v.get(s).unwrap();
            let y = match f.kind {
                Kind::Bit16(_) | Kind::Bit32(_) | Kind::Bool8 => 1 - x,
                Kind::U8 => (x + 1) % 256,
                _ => x.wrapping_add(1),
            };
            v.set(s, y).unwrap();
            assert_eq!(v.get(s), Some(y), "setting {}", s.0);
            let width = match f.kind {
                Kind::Bit16(_) | Kind::I16 => 2,
                Kind::Bit32(_) | Kind::I32 => 4,
                _ => 1,
            };
            for (i, (a, c)) in b.iter().zip(v.bytes()).enumerate() {
                if !(f.offset..f.offset + width).contains(&i) {
                    assert_eq!(a, c, "setting {} wrote byte {i:#x}", s.0);
                }
            }
        }
    }

    #[test]
    fn bits_and_bytes_are_where_the_writer_puts_them() {
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set(SettingId(91), 1).unwrap(); // team play: flags bit 0
        assert!(v.team_play());
        v.set(SettingId(24), 1).unwrap(); // flags bit 7
        assert_eq!(v.bytes()[FLAGS_AT], 0x81);
        v.set(SettingId(2), 50).unwrap();
        assert_eq!(&v.bytes()[0x50..0x54], &50i32.to_le_bytes());
        v.set(SettingId(89), 9).unwrap();
        assert_eq!(v.bytes()[0xd6], 9);
        v.set(SettingId(96), 2).unwrap();
        assert_eq!(&v.bytes()[0xf4..0xf6], &2i16.to_le_bytes());
        assert_eq!(
            v.set(SettingId(91), 2),
            Err(RecordError::OutOfRange { id: 91, value: 2 })
        );
        assert_eq!(
            v.set(SettingId(27), 0),
            Err(RecordError::UnknownSetting(27))
        );
        assert_eq!(
            v.set(SettingId(89), 256),
            Err(RecordError::OutOfRange { id: 89, value: 256 })
        );
    }

    #[test]
    fn name_is_utf16_and_bounded() {
        let mut v = GameVariant::blank(GameType::King);
        v.set_name("Sun King \u{e006}").unwrap();
        assert_eq!(v.name(), "Sun King \u{e006}");
        assert_eq!(&v.bytes()[4..6], &[b'S', 0]);
        v.set_name("ab").unwrap();
        assert!(
            v.bytes()[8..0x44].iter().all(|b| *b == 0),
            "the rest is NUL-filled"
        );
        assert_eq!(v.set_name(&"x".repeat(32)), Err(RecordError::NameTooLong));
        v.set_name(&"x".repeat(31)).unwrap();
        assert!(check(&v).is_empty());
    }

    #[test]
    fn game_types_map_engine_and_content() {
        for t in GameType::ALL {
            assert_eq!(GameType::from_engine(t.engine()), t);
            assert_eq!(
                GameType::from_content_type(t.content_type().unwrap()),
                Some(t)
            );
            assert_eq!(field(t.score_setting().unwrap()).unwrap().offset, 0x50);
        }
        assert_eq!(GameType::from_list_index(3), Some(GameType::King));
        assert_eq!(GameType::from_list_index(7), None);
        assert_eq!(GameType::from_engine(5), GameType::Other(5));
    }

    #[test]
    fn the_cleanup_rules() {
        let mut v = GameVariant::blank(GameType::Slayer);
        assert!(check(&v).is_empty(), "{:?}", check(&v));
        v.set(SettingId(19), 3).unwrap(); // shields 0..=2
        v.set(SettingId(89), 0x15).unwrap();
        let p: Vec<usize> = check(&v).iter().map(|p| p.offset).collect();
        assert_eq!(p, vec![0x88, 0xd6]);

        // CTF must be team play; Juggernaut must not.
        let mut c = GameVariant::blank(GameType::Ctf);
        assert_eq!(check(&c).len(), 1);
        c.set(SettingId(91), 1).unwrap();
        assert!(check(&c).is_empty());
        let mut j = GameVariant::blank(GameType::Juggernaut);
        j.set(SettingId(91), 1).unwrap();
        assert_eq!(check(&j).len(), 1);

        // Territories needs 1.. in its three halves.
        let mut t = GameVariant::blank(GameType::Territories);
        assert_eq!(check(&t).len(), 3);
        for s in [74, 98, 99] {
            t.set(SettingId(s), 1).unwrap();
        }
        assert!(check(&t).is_empty());

        let mut r = GameVariant::blank(GameType::Other(5));
        assert_eq!(check(&r).len(), 1);
        r.set_game_type(GameType::Oddball);
        r.raw[1] = 1;
        assert_eq!(check(&r)[0].offset, 0);
    }

    /// docs/game/game-options.md, "The options to add": values the editor
    /// never offers but the clean-up keeps. They load, save and travel
    /// like any other value; one past each limit is refused.
    #[test]
    fn the_cleanup_keeps_values_the_editor_never_offers() {
        let wider: &[(u32, i32, i32)] = &[
            (89, 1, 0x15),  // starting weapon "None"; 0x15 is past the byte's limit
            (89, 20, 0x15), // a weapon slot the editor does not list
            (94, 15, 0x15), // the same for the secondary weapon
            (17, 0, -1),    // respawn time 0 (instant)
            (17, 0x7fff, 0x8000),
            (2, 0x7fff, 0x8000),  // score to win
            (10, 0x7fff, 0x8000), // time limit, seconds
            (16, 0x7fff, 0x8000), // lives
            (18, 0x7fff, 0x8000), // suicide penalty
            (26, 0x7fff, 0x8000), // betrayal penalty
            (93, 8, 9),           // team count: stored, but no editor row
        ];
        for &(id, ok, bad) in wider {
            let mut v = GameVariant::blank(GameType::Slayer);
            v.set(SettingId(id), ok).unwrap();
            assert!(check(&v).is_empty(), "{id}={ok}: {:?}", check(&v));
            let back = GameVariant::from_bytes(&v.to_bytes()).unwrap();
            assert_eq!(back.get(SettingId(id)), Some(ok));
            if v.set(SettingId(id), bad).is_ok() {
                assert!(!check(&v).is_empty(), "{id}={bad} should be refused");
            }
        }
    }

    #[test]
    fn every_offered_team_count_is_kept() {
        for t in TEAM_COUNTS {
            let mut v = GameVariant::blank(GameType::Slayer);
            v.set(TEAM_COUNT, t).unwrap();
            assert_eq!(v.to_bytes()[0xb4..0xb8], t.to_le_bytes());
            assert!(check(&v).is_empty(), "{t}");
            assert!(refused(&v).is_empty(), "{t}");
            let back = GameVariant::from_bytes(&v.to_bytes()).unwrap();
            assert_eq!(back.get(TEAM_COUNT), Some(t));
        }
        assert_eq!(
            GameVariant::blank(GameType::Slayer).get(TEAM_COUNT),
            Some(0)
        );
    }

    #[test]
    fn respawn_zero_with_cycling_is_refused() {
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set(RESPAWN, 0).unwrap();
        for (modifier, refused_) in [(0, false), (CYCLING, true), (2, false)] {
            v.set(RESPAWN_MODIFIER, modifier).unwrap();
            assert!(check(&v).is_empty(), "the game keeps it");
            assert_eq!(!refused(&v).is_empty(), refused_, "modifier {modifier}");
        }
        v.set(RESPAWN, 1).unwrap();
        assert!(
            refused(&v).is_empty(),
            "1 s with cycling divides by one second"
        );
    }

    /// docs/game/game-options.md, "Where the extension lives": the bytes
    /// the clean-up never looks at, for every game type. Anything written
    /// there passes the clean-up and survives a byte round trip.
    #[test]
    fn the_cleanup_ignores_the_spare_bytes() {
        const SPARE: [std::ops::Range<usize>; 5] =
            [0x5c..0x74, 0x8c..0xa4, 0xb8..0xcc, 0xd8..0xf0, 0x10c..0x130];
        for ty in GameType::ALL {
            let mut v = GameVariant::blank(ty);
            match ty {
                GameType::Ctf | GameType::Assault => v.set(SettingId(91), 1).unwrap(),
                GameType::Territories => {
                    for s in [74, 98, 99] {
                        v.set(SettingId(s), 1).unwrap();
                    }
                }
                _ => {}
            }
            assert!(check(&v).is_empty(), "{ty}: {:?}", check(&v));
            for r in SPARE.iter().cloned().chain([2..3]) {
                for i in r {
                    v.raw[i] = 0xa5;
                }
            }
            assert!(check(&v).is_empty(), "{ty}: {:?}", check(&v));
            assert_eq!(GameVariant::from_bytes(&v.to_bytes()).unwrap(), v);
            // No setting the game's writer knows lives there.
            for s in all_settings() {
                let at = field(s).unwrap().offset;
                assert!(!SPARE.iter().any(|r| r.contains(&at)), "setting {}", s.0);
            }
        }
    }

    #[test]
    fn the_extension_block_survives_the_cleanup_and_every_setter() {
        use crate::ext::{Block, Ext};
        let e = Ext {
            gravity: 50,
            unlimited_grenades: true,
            ..Ext::default()
        };
        for ty in GameType::ALL {
            let mut v = GameVariant::blank(ty);
            assert_eq!(v.ext_state(), Block::None, "a retail record has none");
            v.set_ext(&e).unwrap();
            assert_eq!(v.ext(), e);
            v.set_name("Low and loud").unwrap();
            for s in all_settings() {
                if let Some(x) = v.get(s) {
                    v.set(s, x).unwrap();
                }
            }
            let _ = check(&v);
            let w = GameVariant::from_bytes(&v.to_bytes()).unwrap();
            assert_eq!(w.ext_state(), Block::Ext(e), "{ty}");
            v.set_ext(&Ext::default()).unwrap();
            assert!(v.ext_block().iter().all(|&b| b == 0), "default clears");
        }
        let mut v = GameVariant::blank(GameType::Slayer);
        assert_eq!(
            v.set_ext(&Ext {
                gravity: 1,
                ..Ext::default()
            }),
            Err(RecordError::ExtRange("gravity"))
        );
    }

    #[test]
    fn known_settings_follow_the_type() {
        let v = GameVariant::blank(GameType::Juggernaut);
        let ids: Vec<u32> = v.known_settings().iter().map(|(s, _)| s.0).collect();
        assert!(
            ids.contains(&7) && !ids.contains(&2),
            "only its own score setting"
        );
        assert!(
            ids.contains(&96) && !ids.contains(&45),
            "its own type-area settings"
        );
        assert!(ids.contains(&89), "common settings");
    }
}
