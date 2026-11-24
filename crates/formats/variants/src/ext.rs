//! Our extension block: the options the game's record has no field for,
//! kept in the record's spare bytes at `+0x10c..+0x12f`, which the game's
//! clean-up never reads or clears (docs/game/game-options.md section 3).
//!
//! All zero means "no extension", which is every retail variant. A block
//! with a bad magic, version or CRC is ignored, so a record whose spare
//! bytes hold anything else is played with retail rules. The game's wire
//! drops these bytes; the host sends the block through Live (slice 4).

use std::ops::RangeInclusive;

/// The block's place in the record.
pub const AT: usize = 0x10c;
/// The block's size, CRC included.
pub const LEN: usize = 0x24;

const MAGIC: [u8; 2] = *b"hx";
const VERSION: u8 = 1;
/// Flag bit 1 is unlimited grenades; the other bits are reserved and read
/// as off (the superbounce game type comes from a rules script, not a flag:
/// docs/game/superbounce.md, "The superbounce game type").
const UNLIMITED_GRENADES: u8 = 1 << 1;
const CRC_AT: usize = LEN - 4;

/// The percent scales' range. 100 is the game's own value.
pub const PERCENT: RangeInclusive<u16> = 10..=400;
/// Gravity's range, a percent of the game's.
pub const GRAVITY: RangeInclusive<u16> = 10..=400;
/// Team size: 0 means any, otherwise at most the engine's 16 players.
pub const TEAM_SIZE: RangeInclusive<u8> = 0..=16;

/// What the record's script reference names
/// (docs/game/rules-engine.md, "Carrying a script").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// No script: the record plays as its own game type.
    #[default]
    None,
    Firefight,
    /// A rules script, named by [`Script::prefix`].
    Rules,
    /// A mode a newer build wrote: kept and written back as it was,
    /// never applied.
    Unknown(u8),
}

impl Mode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::None,
            1 => Self::Firefight,
            2 => Self::Rules,
            n => Self::Unknown(n),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Firefight => 1,
            Self::Rules => 2,
            Self::Unknown(n) => n,
        }
    }
}

/// The script reference in the block's former reserved bytes
/// (`+0x11f..+0x12b`): mode, the first 8 bytes of the definition's
/// SHA-256, and 32 bits of packed option values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Script {
    pub mode: Mode,
    pub prefix: [u8; 8],
    pub options: u32,
}

/// The extension's options. [`Ext::default`] is retail: everything at
/// 100% and nothing else, and it encodes as all zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ext {
    /// Gravity, percent of the game's (Low Gravity multiplies on top).
    pub gravity: u16,
    /// Damage dealt, percent.
    pub damage: u16,
    /// Shield strength, percent.
    pub shield: u16,
    /// Health, percent.
    pub health: u16,
    /// Movement speed, percent.
    pub speed: u16,
    pub unlimited_grenades: bool,
    /// Our weapon id for the primary (0: the record's starting weapon).
    pub primary: u16,
    /// Our weapon id for the secondary (0: the record's).
    pub secondary: u16,
    /// Players per team (0: any).
    pub team_size: u8,
    /// The script reference; edits keep it (it is part of the block).
    pub script: Script,
}

impl Ext {
    /// The game's own rules, as a constant ([`Ext::default`]).
    pub const RETAIL: Ext = Ext {
        gravity: 100,
        damage: 100,
        shield: 100,
        health: 100,
        speed: 100,
        unlimited_grenades: false,
        primary: 0,
        secondary: 0,
        team_size: 0,
        script: Script {
            mode: Mode::None,
            prefix: [0; 8],
            options: 0,
        },
    };
}

impl Default for Ext {
    fn default() -> Self {
        Self::RETAIL
    }
}

/// What a record's spare bytes hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// All zero: no extension (every retail variant).
    None,
    Ext(Ext),
    /// Something else: played with retail rules.
    Ignored(Why),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Why {
    #[error("not our block (no `hx` magic)")]
    Magic,
    #[error("block version {0}, this build reads 1")]
    Version(u8),
    #[error("the block's CRC doesn't match")]
    Crc,
    #[error("{0} is out of range")]
    Range(&'static str),
}

impl Ext {
    /// Whether this is the game's own rules (encodes as all zero). A
    /// script reference is never default.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The first value outside its range, if any.
    pub fn out_of_range(&self) -> Option<&'static str> {
        let scale = |v: u16| v == 100 || PERCENT.contains(&v);
        if !(self.gravity == 100 || GRAVITY.contains(&self.gravity)) {
            return Some("gravity");
        }
        for (v, what) in [
            (self.damage, "damage"),
            (self.shield, "shield"),
            (self.health, "health"),
            (self.speed, "speed"),
        ] {
            if !scale(v) {
                return Some(what);
            }
        }
        (!TEAM_SIZE.contains(&self.team_size)).then_some("team size")
    }

    /// Gravity as a factor of the game's (1.0 for 100%).
    pub fn gravity_scale(&self) -> f32 {
        f32::from(self.gravity) / 100.0
    }

    /// Movement speed as a factor of the game's (1.0 for 100%).
    pub fn speed_scale(&self) -> f32 {
        f32::from(self.speed) / 100.0
    }

    /// The block's bytes. The default is all zero, so a variant without
    /// extras stays byte for byte a retail record.
    pub fn encode(&self) -> [u8; LEN] {
        let mut b = [0u8; LEN];
        if self.is_default() {
            return b;
        }
        let pct = |v: u16| if v == 100 { 0 } else { v };
        b[0..2].copy_from_slice(&MAGIC);
        b[2] = VERSION;
        b[3] = if self.unlimited_grenades {
            UNLIMITED_GRENADES
        } else {
            0
        };
        for (i, v) in [
            pct(self.gravity),
            pct(self.damage),
            pct(self.shield),
            pct(self.health),
            pct(self.speed),
            self.primary,
            self.secondary,
        ]
        .into_iter()
        .enumerate()
        {
            b[4 + 2 * i..6 + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        b[0x12] = self.team_size;
        b[0x13] = self.script.mode.to_u8();
        b[0x14..0x1c].copy_from_slice(&self.script.prefix);
        b[0x1c..0x20].copy_from_slice(&self.script.options.to_le_bytes());
        let crc = crc32(&b[..CRC_AT]);
        b[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Reads a block. All zero is [`Block::None`]; anything that isn't a
    /// whole, valid version-1 block is [`Block::Ignored`].
    pub fn decode(b: &[u8; LEN]) -> Block {
        if b.iter().all(|&x| x == 0) {
            return Block::None;
        }
        if b[0..2] != MAGIC {
            return Block::Ignored(Why::Magic);
        }
        if b[2] != VERSION {
            return Block::Ignored(Why::Version(b[2]));
        }
        let crc = u32::from_le_bytes(b[CRC_AT..].try_into().unwrap());
        if crc != crc32(&b[..CRC_AT]) {
            return Block::Ignored(Why::Crc);
        }
        let u16_at = |i: usize| u16::from_le_bytes([b[4 + 2 * i], b[5 + 2 * i]]);
        let pct = |v: u16| if v == 0 { 100 } else { v };
        let e = Ext {
            gravity: pct(u16_at(0)),
            damage: pct(u16_at(1)),
            shield: pct(u16_at(2)),
            health: pct(u16_at(3)),
            speed: pct(u16_at(4)),
            unlimited_grenades: b[3] & UNLIMITED_GRENADES != 0,
            primary: u16_at(5),
            secondary: u16_at(6),
            team_size: b[0x12],
            script: Script {
                mode: Mode::from_u8(b[0x13]),
                prefix: b[0x14..0x1c].try_into().unwrap(),
                options: u32::from_le_bytes(b[0x1c..0x20].try_into().unwrap()),
            },
        };
        match e.out_of_range() {
            Some(what) => Block::Ignored(Why::Range(what)),
            None => Block::Ext(e),
        }
    }

    /// The options in effect for a block: a valid block's, else the game's.
    pub fn in_effect(b: &[u8; LEN]) -> Ext {
        match Self::decode(b) {
            Block::Ext(e) => e,
            Block::None | Block::Ignored(_) => Ext::default(),
        }
    }

    /// One line for logs and lists: "Gravity 50%, Damage 200%", or
    /// "none" for the default.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for (v, what) in [
            (self.gravity, "Gravity"),
            (self.damage, "Damage"),
            (self.shield, "Shields"),
            (self.health, "Health"),
            (self.speed, "Speed"),
        ] {
            if v != 100 {
                parts.push(format!("{what} {v}%"));
            }
        }
        if self.unlimited_grenades {
            parts.push("Unlimited grenades".into());
        }
        if self.primary != 0 || self.secondary != 0 {
            parts.push(format!("Weapons {}/{}", self.primary, self.secondary));
        }
        if self.team_size != 0 {
            parts.push(format!("Teams of {}", self.team_size));
        }
        match self.script.mode {
            Mode::None => {}
            Mode::Firefight => parts.push("Firefight".into()),
            Mode::Rules => parts.push("Rules script".into()),
            Mode::Unknown(n) => parts.push(format!("Unknown script mode {n}")),
        }
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join(", ")
        }
    }
}

/// CRC-32 (IEEE, reflected, as zlib's `crc32`).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some() -> Ext {
        Ext {
            gravity: 50,
            damage: 200,
            shield: 100,
            health: 150,
            speed: 125,
            unlimited_grenades: true,
            primary: 7,
            secondary: 0,
            team_size: 4,
            script: Script::default(),
        }
    }

    fn scripted(mode: Mode) -> Ext {
        Ext {
            script: Script {
                mode,
                prefix: [1, 2, 3, 4, 5, 6, 7, 8],
                options: 0xa1b2_c3d4,
            },
            ..Ext::default()
        }
    }

    #[test]
    fn crc_matches_zlib() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn the_default_is_all_zero_and_reads_as_none() {
        let b = Ext::default().encode();
        assert_eq!(b, [0; LEN]);
        assert_eq!(Ext::decode(&b), Block::None);
        assert_eq!(Ext::in_effect(&b), Ext::default());
        assert_eq!(Ext::default().summary(), "none");
    }

    #[test]
    fn a_block_round_trips() {
        let e = some();
        let b = e.encode();
        assert_eq!(&b[0..3], b"hx\x01");
        assert_eq!(b[3], 0b10, "bit 1 unlimited grenades, bit 0 reserved");
        assert_eq!(u16::from_le_bytes([b[4], b[5]]), 50, "gravity at +0x110");
        assert_eq!(u16::from_le_bytes([b[8], b[9]]), 0, "100% stores as 0");
        assert_eq!(b[0x12], 4, "team size at +0x11e");
        assert!(b[0x13..CRC_AT].iter().all(|&x| x == 0), "no script");
        assert_eq!(Ext::decode(&b), Block::Ext(e));
        assert_eq!(
            e.summary(),
            "Gravity 50%, Damage 200%, Health 150%, Speed 125%, \
             Unlimited grenades, Weapons 7/0, Teams of 4"
        );
        // One option alone.
        let g = Ext {
            gravity: 25,
            ..Ext::default()
        };
        assert_eq!(Ext::decode(&g.encode()), Block::Ext(g));
    }

    #[test]
    fn bit_2_is_reserved_and_reads_as_off() {
        // A record written with the old superbounce flag still decodes: the
        // bit asks for nothing (a rules script turns the mode on).
        let mut b = some().encode();
        b[3] |= 0b100;
        let crc = crc32(&b[..CRC_AT]);
        b[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(Ext::in_effect(&b), some());
    }

    #[test]
    fn a_damaged_block_is_ignored() {
        let b = some().encode();
        for i in 0..LEN {
            let mut c = b;
            c[i] ^= 0x10;
            assert!(
                matches!(Ext::decode(&c), Block::Ignored(_)),
                "a flipped bit at +{:#x} is caught",
                AT + i
            );
            assert_eq!(Ext::in_effect(&c), Ext::default());
        }
        let mut c = b;
        c[0] = b'H';
        assert_eq!(Ext::decode(&c), Block::Ignored(Why::Magic));
        let mut c = b;
        c[2] = 2;
        assert_eq!(Ext::decode(&c), Block::Ignored(Why::Version(2)));
        let mut c = b;
        c[4] = 0x40;
        assert_eq!(Ext::decode(&c), Block::Ignored(Why::Crc));
    }

    #[test]
    fn a_damaged_block_shows_as_an_issue() {
        use crate::record::{GameType, GameVariant};
        use crate::schema::{Issue, Schema};
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set_ext(&some()).unwrap();
        assert!(Schema::fallback().validate(&v).is_empty());
        let mut b = v.to_bytes();
        b[AT + 4] ^= 1;
        let v = GameVariant::from_bytes(&b).unwrap();
        let issues = Schema::fallback().validate(&v);
        assert_eq!(issues, [Issue::Ext(Why::Crc)]);
        assert!(
            crate::record::refused(&v).is_empty(),
            "a warning, not refused"
        );
    }

    #[test]
    fn out_of_range_values_are_ignored_even_with_a_good_crc() {
        let e = Ext {
            gravity: 5,
            ..Ext::default()
        };
        assert_eq!(e.out_of_range(), Some("gravity"));
        assert_eq!(
            Ext::decode(&e.encode()),
            Block::Ignored(Why::Range("gravity"))
        );
        let e = Ext {
            team_size: 17,
            ..Ext::default()
        };
        assert_eq!(
            Ext::decode(&e.encode()),
            Block::Ignored(Why::Range("team size"))
        );
        let e = Ext {
            speed: 401,
            ..Ext::default()
        };
        assert_eq!(e.out_of_range(), Some("speed"));
    }

    #[test]
    fn a_script_reference_round_trips_in_every_mode() {
        for (mode, text) in [
            (Mode::Firefight, "Firefight"),
            (Mode::Rules, "Rules script"),
            (Mode::Unknown(9), "Unknown script mode 9"),
        ] {
            let e = scripted(mode);
            assert!(!e.is_default());
            let b = e.encode();
            assert_ne!(b, [0; LEN], "a script with default extras has a block");
            assert_eq!(b[0x13], mode.to_u8(), "mode at +0x11f");
            assert_eq!(&b[0x14..0x1c], &[1, 2, 3, 4, 5, 6, 7, 8], "prefix at +0x120");
            assert_eq!(
                u32::from_le_bytes(b[0x1c..0x20].try_into().unwrap()),
                0xa1b2_c3d4,
                "options at +0x128"
            );
            assert_eq!(Ext::decode(&b), Block::Ext(e));
            assert_eq!(Ext::in_effect(&b).script, e.script);
            assert_eq!(e.summary(), text);
        }
        assert_eq!(Mode::from_u8(0), Mode::None);
        assert_eq!(Mode::from_u8(1), Mode::Firefight);
        assert_eq!(Mode::from_u8(2), Mode::Rules);
    }

    #[test]
    fn the_crc_covers_the_script_bytes() {
        let b = scripted(Mode::Rules).encode();
        for i in 0x13..CRC_AT {
            let mut c = b;
            c[i] ^= 1;
            assert_eq!(Ext::decode(&c), Block::Ignored(Why::Crc), "+{:#x}", AT + i);
        }
    }

    #[test]
    fn an_all_zero_block_has_no_script() {
        assert_eq!(Ext::decode(&[0; LEN]), Block::None);
        assert_eq!(Ext::in_effect(&[0; LEN]).script, Script::default());
    }
}
