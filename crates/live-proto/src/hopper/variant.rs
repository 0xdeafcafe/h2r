//! The game variant a hopper entry carries: the last 0x130 bytes of a record
//! slot (`+0x24c..+0x37c`), the same record as a saved variant and the
//! active variant at `G+0x13c` (docs/variants.md, which has the full table
//! and the evidence). Session setup (`0x00192eb0`) copies these bytes out
//! whole for the entry it picks; the record clean-up (`0x0019d650`) is what
//! the game holds a variant to.
//!
//! This module is the slice of the format a server needs to author entries
//! without a disc: the stock record for each game type (a port of the
//! game's own defaults writer `0x0019d220`), typed setters for the settings
//! the default playlists change, and [`VariantRecord::problems`], a port of
//! the clean-up's limits. The editor's `crates/variants` reads and edits
//! every setting of the same bytes (`GameVariant::from_bytes`).

/// The record's size.
pub const SIZE: usize = 0x130;
/// UTF-16 units in the name at `+0x04`; the last is always 0.
pub const NAME_UNITS: usize = 32;
/// Longest name, in UTF-16 units.
pub const MAX_NAME_UNITS: usize = NAME_UNITS - 1;

// Offsets in the record (docs/variants.md, "Where each setting lives";
// the setting numbers are the game's own, `0x470828`).
/// u16 header: bit 0 the built-in flag, nothing else may be set.
pub const HEADER: usize = 0x00;
/// u8 built-in description number; the stock writer leaves 0xff.
pub const DESCRIPTION: usize = 0x03;
/// UTF-16 name, [`NAME_UNITS`] units.
pub const NAME: usize = 0x04;
/// i32 game-engine index ([`GameType`]).
pub const ENGINE: usize = 0x44;
/// u32 flags: bit 0 team play; the other bits are on/off settings.
pub const FLAGS: usize = 0x48;
/// i32 setting 0, rounds (0..=6).
pub const ROUNDS: usize = 0x4c;
/// i32 settings 1..=9, the type's score to win.
pub const SCORE_TO_WIN: usize = 0x50;
/// i32 setting 10, the time limit (seconds as the editor shows it).
pub const TIME_LIMIT: usize = 0x54;
/// i32 setting 14 (0..=2); the hopper defaults writer (`0x00193010`)
/// clears it on every entry.
pub const SETTING_14: usize = 0x58;
/// i32 setting 15 (0..=16).
pub const SETTING_15: usize = 0x74;
/// i32 setting 90, maximum players (0..=16).
pub const MAX_PLAYERS: usize = 0x78;
/// i32 setting 16, lives per round (0 = unlimited).
pub const LIVES: usize = 0x7c;
/// i32 setting 17, respawn time in seconds.
pub const RESPAWN_TIME: usize = 0x80;
/// i32 setting 18, suicide penalty in seconds.
pub const SUICIDE_PENALTY: usize = 0x84;
/// i32 setting 19, shields (0..=2).
pub const SHIELDS: usize = 0x88;
/// i32 setting 23 (0..=2).
pub const SETTING_23: usize = 0xa4;
/// i32 setting 25 (0..=2).
pub const SETTING_25: usize = 0xa8;
/// i32 setting 26 (0..=0x7fff).
pub const SETTING_26: usize = 0xac;
/// u8 settings 75..=83: the vehicle, turret and weapons-on-map sets.
pub const VEHICLE_SET: usize = 0xcc;
pub const LIGHT_VEHICLE: usize = 0xcd;
pub const SECOND_LIGHT_VEHICLE: usize = 0xce;
pub const HEAVY_VEHICLE: usize = 0xcf;
pub const FLYING_VEHICLE: usize = 0xd0;
pub const SECOND_HEAVY_VEHICLE: usize = 0xd1;
pub const PRIMARY_TURRET: usize = 0xd2;
pub const SECONDARY_TURRET: usize = 0xd3;
pub const WEAPONS_ON_MAP: usize = 0xd4;
/// u8 setting 87.
pub const WEAPON_RESPAWN: usize = 0xd5;
/// u8 setting 89.
pub const STARTING_WEAPON: usize = 0xd6;
/// u8 setting 94.
pub const SECONDARY_WEAPON: usize = 0xd7;
/// The game-type area, `0xf0..0x10c`; its meaning depends on the engine.
pub const TYPE_AREA: usize = 0xf0;

/// Flag bits (`+0x48`), by the setting that stores them.
pub mod flag {
    pub const TEAM_PLAY: u32 = 1 << 0;
    /// Setting 20.
    pub const MOTION_SENSOR: u32 = 1 << 1;
    /// Setting 21.
    pub const ACTIVE_CAMO: u32 = 1 << 2;
    /// Settings 12 and 13: the hopper defaults writer clears both.
    pub const SETTING_12: u32 = 1 << 4;
    pub const SETTING_13: u32 = 1 << 5;
    /// Setting 24.
    pub const FRIENDLY_FIRE: u32 = 1 << 7;
}

/// A game type by its game-engine index (`+0x44`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameType {
    Ctf,
    Slayer,
    Oddball,
    King,
    Juggernaut,
    Territories,
    Assault,
}

impl GameType {
    pub const ALL: [GameType; 7] = [
        GameType::Slayer,
        GameType::Oddball,
        GameType::Juggernaut,
        GameType::King,
        GameType::Ctf,
        GameType::Assault,
        GameType::Territories,
    ];

    pub fn engine(self) -> u32 {
        match self {
            Self::Ctf => 1,
            Self::Slayer => 2,
            Self::Oddball => 3,
            Self::King => 4,
            Self::Juggernaut => 7,
            Self::Territories => 8,
            Self::Assault => 9,
        }
    }

    pub fn from_engine(e: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.engine() == e)
    }

    /// The stock writer (`0x0019d220`) turns team play on for these.
    pub fn team_by_default(self) -> bool {
        matches!(self, Self::Ctf | Self::Assault | Self::Territories)
    }

    /// The stock score to win.
    fn stock_score(self) -> i32 {
        match self {
            Self::Slayer => 25,
            Self::Oddball | Self::King => 120,
            Self::Juggernaut => 15,
            Self::Ctf | Self::Assault => 3,
            Self::Territories => 300,
        }
    }
}

/// One game variant record. The bytes are the source of truth.
#[derive(Clone, PartialEq, Eq)]
pub struct VariantRecord {
    raw: [u8; SIZE],
}

impl std::fmt::Debug for VariantRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VariantRecord")
            .field("name", &self.name())
            .field("engine", &self.engine())
            .field("flags", &format_args!("{:#x}", self.flags()))
            .finish_non_exhaustive()
    }
}

impl VariantRecord {
    /// Adopts 0x130 bytes as they are.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(VariantRecord {
            raw: bytes.try_into().ok()?,
        })
    }

    pub fn bytes(&self) -> &[u8; SIZE] {
        &self.raw
    }

    /// The stock record of a game type: what the game's defaults writer
    /// (`0x0019d220`) builds before any built-in variant's changes, with
    /// `name` in place of its localized type name. Every other byte is 0.
    pub fn stock(game: GameType, name: &str) -> Self {
        let mut v = VariantRecord { raw: [0; SIZE] };
        let team = game.team_by_default();
        v.raw[DESCRIPTION] = 0xff;
        v.set_name(name);
        v.put_i32(ENGINE, game.engine() as i32);
        // `& 0xffff8f9b | 0xf9a`, team play by type.
        v.put_u32(FLAGS, 0xf9a | u32::from(team));
        v.put_i32(ROUNDS, 0);
        v.put_i32(SCORE_TO_WIN, game.stock_score());
        v.put_i32(TIME_LIMIT, 480);
        v.put_i32(SETTING_15, 16);
        v.put_i32(MAX_PLAYERS, 16);
        v.put_i32(LIVES, 0);
        let respawn = if team { 10 } else { 5 };
        v.put_i32(RESPAWN_TIME, respawn);
        v.put_i32(SUICIDE_PENALTY, respawn);
        v.put_i32(SETTING_25, 2);
        v.put_i32(SETTING_26, 10);
        let area = TYPE_AREA;
        match game {
            GameType::Slayer => v.put_u32(area, 0x2),
            GameType::Oddball => v.put_i16(area + 4, 1),
            GameType::Juggernaut => {
                v.put_u32(area, 0x1b);
                v.put_i16(area + 4, 2);
            }
            GameType::King => v.put_i16(area + 4, 60),
            GameType::Ctf => {
                v.put_u32(area, 0x2);
                v.put_i32(area + 4, 30);
            }
            GameType::Assault => {
                v.put_u32(area, 0x22);
                v.put_i32(area + 4, 30);
                v.put_i32(area + 0x10, 1);
                v.put_i16(area + 0x18, 5);
                v.put_i16(area + 0x1a, 3);
            }
            GameType::Territories => {
                v.put_i16(area, 3);
                v.put_i16(area + 2, 5);
                v.put_i16(area + 4, 5);
            }
        }
        v
    }

    /// The name, up to the first NUL.
    pub fn name(&self) -> String {
        let units: Vec<u16> = (0..NAME_UNITS)
            .map(|k| self.u16_at(NAME + 2 * k))
            .take_while(|u| *u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    }

    /// Writes the name, zero-filled; longer names are cut at
    /// [`MAX_NAME_UNITS`] units.
    pub fn set_name(&mut self, name: &str) {
        self.raw[NAME..NAME + 2 * NAME_UNITS].fill(0);
        for (k, u) in name.encode_utf16().take(MAX_NAME_UNITS).enumerate() {
            self.raw[NAME + 2 * k..][..2].copy_from_slice(&u.to_le_bytes());
        }
    }

    pub fn engine(&self) -> u32 {
        self.u32_at(ENGINE)
    }

    pub fn flags(&self) -> u32 {
        self.u32_at(FLAGS)
    }

    pub fn team(&self) -> bool {
        self.flags() & flag::TEAM_PLAY != 0
    }

    /// Sets or clears flag bits ([`flag`]).
    pub fn set_flag(&mut self, bits: u32, on: bool) {
        let f = self.flags();
        self.put_u32(FLAGS, if on { f | bits } else { f & !bits });
    }

    pub fn set_team(&mut self, on: bool) {
        self.set_flag(flag::TEAM_PLAY, on);
    }

    /// What the hopper defaults writer (`0x00193010`) does to every entry
    /// after building its variant: clears settings 12, 13 and 14.
    pub fn for_matchmaking(mut self) -> Self {
        self.set_flag(flag::SETTING_12 | flag::SETTING_13, false);
        self.put_i32(SETTING_14, 0);
        self
    }

    pub fn u8_at(&self, at: usize) -> u8 {
        self.raw[at]
    }

    pub fn u16_at(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.raw[at], self.raw[at + 1]])
    }

    pub fn i16_at(&self, at: usize) -> i16 {
        self.u16_at(at) as i16
    }

    pub fn u32_at(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.raw[at..at + 4].try_into().expect("4 bytes"))
    }

    pub fn i32_at(&self, at: usize) -> i32 {
        self.u32_at(at) as i32
    }

    pub fn put_u8(&mut self, at: usize, v: u8) {
        self.raw[at] = v;
    }

    pub fn put_i16(&mut self, at: usize, v: i16) {
        self.raw[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }

    pub fn put_u32(&mut self, at: usize, v: u32) {
        self.raw[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn put_i32(&mut self, at: usize, v: i32) {
        self.put_u32(at, v as u32);
    }

    /// Everything the record clean-up (`0x0019d650`) would change, as
    /// `(offset, what)`; empty when the game keeps the record as it is.
    /// The rules are docs/variants.md's "The clean-up", the same as
    /// `variants::record::check`.
    pub fn problems(&self) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        let mut bad = |at: usize, what: String| out.push((at, what));
        if self.raw[0] > 1 || self.raw[1] != 0 {
            bad(HEADER, "the header word may only have bit 0".into());
        }
        if self.u16_at(NAME + 2 * MAX_NAME_UNITS) != 0 {
            bad(
                NAME + 2 * MAX_NAME_UNITS,
                "the name's last unit must be 0".into(),
            );
        }
        let engine = self.i32_at(ENGINE);
        if !matches!(engine, 1..=4 | 7..=9) {
            bad(ENGINE, format!("engine {engine} is not a playable type"));
        }
        if self.flags() & !0x7fff != 0 {
            bad(FLAGS, "flags above bit 14 are set".into());
        }
        let i32_in = |at: usize, lo: i32, hi: i32, bad: &mut dyn FnMut(usize, String)| {
            let v = self.i32_at(at);
            if !(lo..=hi).contains(&v) {
                bad(at, format!("{v} is outside {lo}..={hi}"));
            }
        };
        i32_in(ROUNDS, 0, 6, &mut bad);
        for at in [
            SCORE_TO_WIN,
            TIME_LIMIT,
            LIVES,
            RESPAWN_TIME,
            SUICIDE_PENALTY,
            SETTING_26,
        ] {
            i32_in(at, 0, 0x7fff, &mut bad);
        }
        for at in [SETTING_14, SHIELDS, SETTING_23, SETTING_25] {
            i32_in(at, 0, 2, &mut bad);
        }
        for at in [SETTING_15, MAX_PLAYERS] {
            i32_in(at, 0, 16, &mut bad);
        }
        if self.u32_at(0xb0) != 0 {
            bad(0xb0, "must be 0".into());
        }
        i32_in(0xb4, 0, 8, &mut bad);
        for (at, hi) in [
            (VEHICLE_SET, 3),
            (LIGHT_VEHICLE, 7),
            (SECOND_LIGHT_VEHICLE, 7),
            (HEAVY_VEHICLE, 4),
            (FLYING_VEHICLE, 4),
            (SECOND_HEAVY_VEHICLE, 4),
            (PRIMARY_TURRET, 6),
            (SECONDARY_TURRET, 6),
            (WEAPONS_ON_MAP, 0x13),
            (WEAPON_RESPAWN, 3),
            (STARTING_WEAPON, 0x14),
            (SECONDARY_WEAPON, 0x14),
        ] {
            if self.raw[at] > hi {
                bad(at, format!("{} is above {hi}", self.raw[at]));
            }
        }
        let i16_in = |at: usize, lo: i16, hi: i16, bad: &mut dyn FnMut(usize, String)| {
            let v = self.i16_at(at);
            if !(lo..=hi).contains(&v) {
                bad(at, format!("{v} is outside {lo}..={hi}"));
            }
        };
        let mask = |m: u32, bad: &mut dyn FnMut(usize, String)| {
            if self.u32_at(TYPE_AREA) & !m != 0 {
                bad(TYPE_AREA, format!("type flags outside {m:#x}"));
            }
        };
        let ctf = |bad: &mut dyn FnMut(usize, String)| {
            if !self.team() {
                bad(FLAGS, "this type is always team play".into());
            }
            mask(0xff, bad);
            for (at, hi) in [(0xf4, 0x7fff), (0xf8, 2), (0xfc, 1), (0x100, 3), (0x104, 2)] {
                let v = self.i32_at(at);
                if !(0..=hi).contains(&v) {
                    bad(at, format!("{v} is outside 0..={hi}"));
                }
            }
        };
        match engine {
            1 => ctf(&mut bad),
            9 => {
                i16_in(0x108, 0, 15, &mut bad);
                i16_in(0x10a, 0, 15, &mut bad);
                ctf(&mut bad);
            }
            2 => mask(7, &mut bad),
            3 => {
                mask(7, &mut bad);
                i16_in(0xf4, 0, 3, &mut bad);
                i16_in(0xf6, 0, 1, &mut bad);
                i16_in(0xf8, 0, 2, &mut bad);
                i16_in(0xfa, 0, 3, &mut bad);
            }
            4 => {
                mask(0x1f, &mut bad);
                i16_in(0xf4, 0, 0x7fff, &mut bad);
            }
            7 => {
                if self.team() {
                    bad(FLAGS, "Juggernaut is never team play".into());
                }
                mask(0x7f, &mut bad);
                i16_in(0xf4, 0, 2, &mut bad);
            }
            8 => {
                i16_in(0xf0, 1, 8, &mut bad);
                i16_in(0xf2, 1, 0x7fff, &mut bad);
                i16_in(0xf4, 1, 0x7fff, &mut bad);
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stock_record_passes_the_clean_up() {
        for t in GameType::ALL {
            let v = VariantRecord::stock(t, "Stock");
            assert_eq!(v.problems(), vec![], "{t:?}");
            assert_eq!(v.engine(), t.engine());
            assert_eq!(v.team(), t.team_by_default());
            assert_eq!(GameType::from_engine(v.engine()), Some(t));
            assert_eq!(v.name(), "Stock");
            let m = v.clone().for_matchmaking();
            assert_eq!(m.problems(), vec![]);
            assert_eq!(m.flags() & (flag::SETTING_12 | flag::SETTING_13), 0);
        }
        assert_eq!(GameType::from_engine(5), None);
    }

    #[test]
    fn the_clean_up_port_catches_each_kind_of_rule() {
        let mut v = VariantRecord::stock(GameType::Juggernaut, "J");
        v.set_team(true);
        v.put_i32(ROUNDS, 7);
        v.put_u8(WEAPONS_ON_MAP, 0x14);
        v.put_u32(0xb0, 1);
        v.put_i16(TYPE_AREA + 4, 3);
        let at: Vec<usize> = v.problems().into_iter().map(|(at, _)| at).collect();
        assert_eq!(at, vec![ROUNDS, 0xb0, WEAPONS_ON_MAP, FLAGS, TYPE_AREA + 4]);
        let mut ctf = VariantRecord::stock(GameType::Ctf, "C");
        ctf.set_team(false);
        assert_eq!(ctf.problems().len(), 1);
        let mut t = VariantRecord::stock(GameType::Territories, "T");
        t.put_i16(TYPE_AREA, 0);
        assert_eq!(t.problems()[0].0, TYPE_AREA);
    }

    #[test]
    fn names_are_cut_and_zero_terminated() {
        let mut v = VariantRecord::stock(GameType::Slayer, "x");
        v.set_name(&"n".repeat(40));
        assert_eq!(v.name().len(), MAX_NAME_UNITS);
        assert_eq!(v.u16_at(NAME + 2 * MAX_NAME_UNITS), 0);
        assert_eq!(VariantRecord::from_bytes(v.bytes()).unwrap(), v);
        assert!(VariantRecord::from_bytes(&[0; 3]).is_none());
    }
}
